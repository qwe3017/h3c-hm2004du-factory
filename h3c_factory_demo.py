#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
H3C HM2004-DU factory 工具 (Rust egui 编辑器的 Python 伴生版 / CLI)。

与 src/model.rs::encode() 完全一致地生成「布局映射演示镜像」:
  - 所有字段落在 H3C 真实偏移上
  - 不含 UBI EC/VID 头, AL-MAC 区以明文(非 gzip)写入
  - 仅用于查看/对照, 不可直接刷写 NAND

同时支持从真实 reservearea 备份 (.bin) 解析 SN / AL-MAC / 校准 / EEPROM,
等价于 src/import.rs::parse_factory_volume。

用法:
  # 用已知设备身份生成演示镜像
  python3 h3c_factory_demo.py gen --sn H3CT0005EBF0 --mac 44:76:09:65:06:4a -o factory-h3c-demo.bin

  # 从真实备份解析字段
  python3 h3c_factory_demo.py parse --bin "G:/下载/H3C-HM2004-DU-TELNET备份/mtd14_reservearea.bin"
"""
import argparse
import struct
import sys
import zlib

FACTORY_VOLUME_SIZE = 0x240000
DEMO_SIZE = 0x1C1000

MAGIC_OFFSET = 0x140000
MAGIC = bytes([0x21, 0x43, 0x34, 0x12])
BASE_MAC_OFFSET = 0x140005
DEVICE_SN_OFFSET = 0x14102C
DEVICE_SN_DUP_OFFSET = 0x14105F
DEVICE_SN_SIZE = 0x0C
ALMAC_REGION_OFFSET = 0x80000
ALMAC_REGION_SIZE = 0x4000
CALIBRATION_OFFSET = 0x1C0400
CALIBRATION_SIZE = 0x200
WIFI_EEPROM_OFFSET = 0x4C000
WIFI_EEPROM_SIZE = 0x1000

# 1MiB 紧凑布局（自描述，不可刷写）：真实 H3C 身份字段在 0x140000+（超 1MiB），
# 故按文档化偏移重排，magic "H3C-1MiB" 用于解析时自动识别。
COMPACT_SIZE = 0x100000
COMPACT_MAGIC = b"H3C-1MiB"
C_LABEL_MAC_OFFSET = 0x0010
C_ETH0_MAC_OFFSET = 0x0020
C_PON0_MAC_OFFSET = 0x0030
C_WIFI5G_MAC_OFFSET = 0x0040
C_WIFI24_MAC_OFFSET = 0x0050
C_DEVICE_SN_OFFSET = 0x0060
C_WIFI_EEPROM_OFFSET = 0x1000
C_CALIBRATION_OFFSET = 0x2000

# MAC 派生偏移 (末字节)
OFF_ETH0, OFF_PON0, OFF_WIFI_5G, OFF_WIFI_24G = 1, 2, 4, 5


def parse_mac(s):
    s = s.strip()
    if ":" in s:
        parts = s.split(":")
    elif "-" in s:
        parts = s.split("-")
    elif len(s) == 12:
        parts = [s[i:i + 2] for i in range(0, 12, 2)]
    else:
        return None
    if len(parts) != 6:
        return None
    try:
        m = bytes(int(p, 16) for p in parts)
    except ValueError:
        return None
    if m == b"\x00" * 6 or m == b"\xff" * 6:
        return None
    if m[0] & 0x01:
        return None
    return m


def mac_to_string(m):
    return ":".join(f"{b:02X}" for b in m)


def derive_mac(base, off):
    m = bytearray(base)
    m[5] = (m[5] + off) & 0xFF
    return bytes(m)


# 厂商名（无数字、不代表芯片），识别型号时跳过
VENDOR_TOKENS = ("ECONET",)


def _take_token(run):
    s = bytes(run).decode("ascii", "ignore").strip()
    run.clear()
    if len(s) < 4:
        return None
    has_alpha = any(c.isalpha() for c in s)
    has_digit = any(c.isdigit() for c in s)
    if has_alpha and has_digit and s.upper() not in VENDOR_TOKENS:
        return s
    return None


def detect_pon_chip(cal):
    """从校准 blob 识别 PON 前端芯片（硬件由导入数据决定）。

    - FiberHome/APONCAL 风格：0x0c 处 u32 LE 芯片 ID（1=GN28L95, 2=UX3363）
    - H3C 自有格式：blob 内填充的 ASCII 型号名（实测 0x1c0428 = "EN7572"）
    - 两者都不是 -> None（未设置 / 未识别）
    """
    if not cal or all(b == 0xFF for b in cal):
        return None
    if cal[:8] == b"APONCAL\x00" and len(cal) >= 0x10:
        cid = int.from_bytes(cal[0x0C:0x10], "little")
        return {1: "GN28L95", 2: "UX3363"}.get(cid, "未知芯片 #%d" % cid)
    run = bytearray()
    for b in list(cal) + [0x0A]:
        if 0x20 <= b < 0x7F:
            run.append(b)
        else:
            t = _take_token(run)
            if t:
                return t
    return None


def encode(sn, label_mac, calibration=None, eeprom=None):
    img = bytearray(b"\xff" * DEMO_SIZE)
    img[MAGIC_OFFSET:MAGIC_OFFSET + 4] = MAGIC
    img[BASE_MAC_OFFSET:BASE_MAC_OFFSET + 6] = label_mac
    snb = sn.encode("ascii")[:DEVICE_SN_SIZE]
    img[DEVICE_SN_OFFSET:DEVICE_SN_OFFSET + len(snb)] = snb
    img[DEVICE_SN_DUP_OFFSET:DEVICE_SN_DUP_OFFSET + len(snb)] = snb
    block = (
        '<ctromfile>\n  <mapcfg AL-MAC="%s" />\n'
        '  <sysinfo SerialNumber="%s" />\n</ctromfile>\n'
        % (mac_to_string(label_mac), sn)
    ).encode("utf-8")
    img[ALMAC_REGION_OFFSET:ALMAC_REGION_OFFSET + len(block)] = block
    if calibration:
        img[CALIBRATION_OFFSET:CALIBRATION_OFFSET + len(calibration)] = calibration
    if eeprom:
        img[WIFI_EEPROM_OFFSET:WIFI_EEPROM_OFFSET + len(eeprom)] = eeprom
    return bytes(img)


def encode_1mib(sn, label_mac, calibration=None, eeprom=None):
    """1MiB 紧凑镜像：全部字段按文档化偏移重排进 1MiB，含 5 条 MAC 表。"""
    img = bytearray(b"\xff" * COMPACT_SIZE)
    img[0:len(COMPACT_MAGIC)] = COMPACT_MAGIC
    for off, mac in (
        (C_LABEL_MAC_OFFSET, label_mac),
        (C_ETH0_MAC_OFFSET, derive_mac(label_mac, OFF_ETH0)),
        (C_PON0_MAC_OFFSET, derive_mac(label_mac, OFF_PON0)),
        (C_WIFI5G_MAC_OFFSET, derive_mac(label_mac, OFF_WIFI_5G)),
        (C_WIFI24_MAC_OFFSET, derive_mac(label_mac, OFF_WIFI_24G)),
    ):
        img[off:off + 6] = mac
    snb = sn.encode("ascii")[:DEVICE_SN_SIZE]
    img[C_DEVICE_SN_OFFSET:C_DEVICE_SN_OFFSET + len(snb)] = snb
    if eeprom:
        img[C_WIFI_EEPROM_OFFSET:C_WIFI_EEPROM_OFFSET + len(eeprom)] = eeprom
    if calibration:
        img[C_CALIBRATION_OFFSET:C_CALIBRATION_OFFSET + len(calibration)] = calibration
    return bytes(img)


def _find_subseq(hay, needle):
    return hay.find(needle)


def _find_al_mac(buf):
    tag = b'AL-MAC="'
    from_ = 0
    while from_ + len(tag) + 17 <= len(buf):
        rel = buf.find(tag, from_)
        if rel < 0:
            break
        at = rel + len(tag)
        m = parse_mac(buf[at:at + 17].decode("utf-8", "ignore"))
        if m:
            return m
        from_ = at


def read_almac_bytes(data):
    """AL-MAC 区内容：真实备份为 gzip ctromfile(解压后)，演示镜像为明文原文。

    用 decompressobj 而非 zlib.decompress：前者在 gzip 成员结束处停下，
    不会因为成员后紧随的 0xff 填充而报错。
    """
    if len(data) <= ALMAC_REGION_OFFSET:
        return None
    region = data[ALMAC_REGION_OFFSET:]
    idx = region.find(b"\x1f\x8b\x08")
    if idx >= 0:
        d = zlib.decompressobj(16 + zlib.MAX_WBITS)
        try:
            out = d.decompress(region[idx:]) + d.flush()
        except Exception:
            out = b""
        if out:
            return out
    return region


def _find_serial_number(buf):
    tag = b'SerialNumber="'
    i = buf.find(tag)
    if i < 0:
        return None
    at = i + len(tag)
    j = buf.find(b'"', at)
    if j < 0 or j == at:
        return None
    return buf[at:j].decode("utf-8", "ignore")


def read_label_mac(data):
    text = read_almac_bytes(data)
    if text:
        m = _find_al_mac(text)
        if m:
            return m
    if len(data) >= BASE_MAC_OFFSET + 6:
        m = data[BASE_MAC_OFFSET:BASE_MAC_OFFSET + 6]
        if any(b not in (0xFF, 0x00) for b in m):
            return bytes(m)
    return None


def parse_compact(data):
    """解析 encode_1mib 生成的 1MiB 紧凑镜像。"""
    label = data[C_LABEL_MAC_OFFSET:C_LABEL_MAC_OFFSET + 6]
    label = bytes(label) if any(b != 0xFF for b in label) else None
    sn = ""
    if len(data) >= C_DEVICE_SN_OFFSET + DEVICE_SN_SIZE:
        sn = data[C_DEVICE_SN_OFFSET:C_DEVICE_SN_OFFSET + DEVICE_SN_SIZE].decode(
            "utf-8", "ignore"
        ).strip("\x00 ").strip()
    cal = data[C_CALIBRATION_OFFSET:C_CALIBRATION_OFFSET + CALIBRATION_SIZE]
    eep = data[C_WIFI_EEPROM_OFFSET:C_WIFI_EEPROM_OFFSET + WIFI_EEPROM_SIZE]
    return sn, label, cal, eep


def parse_volume(data):
    # 1MiB 紧凑镜像自动识别
    if data[:8] == COMPACT_MAGIC:
        return parse_compact(data)
    sn = ""
    if len(data) >= DEVICE_SN_OFFSET + DEVICE_SN_SIZE:
        sn = data[DEVICE_SN_OFFSET:DEVICE_SN_OFFSET + DEVICE_SN_SIZE].decode(
            "utf-8", "ignore"
        ).strip("\x00 ").strip()
    # 0x14102c 槽位可能为空，回退到 ctromfile 里的 SerialNumber
    if not sn:
        text = read_almac_bytes(data)
        if text:
            found = _find_serial_number(text)
            if found:
                sn = found
    label = read_label_mac(data)
    cal = data[CALIBRATION_OFFSET:CALIBRATION_OFFSET + CALIBRATION_SIZE] if len(data) >= CALIBRATION_OFFSET + CALIBRATION_SIZE else b""
    eep = data[WIFI_EEPROM_OFFSET:WIFI_EEPROM_OFFSET + WIFI_EEPROM_SIZE] if len(data) >= WIFI_EEPROM_OFFSET + WIFI_EEPROM_SIZE else b""
    return sn, label, cal, eep


def main():
    ap = argparse.ArgumentParser(description="H3C HM2004-DU factory 工具")
    sub = ap.add_subparsers(dest="cmd")

    g = sub.add_parser("gen", help="生成演示镜像 (1mib 紧凑 / true 真实布局)")
    g.add_argument("--sn", required=True)
    g.add_argument("--mac", required=True)
    g.add_argument("--cal", help="校准 bin (可选)")
    g.add_argument("--eeprom", help="EEPROM bin (可选)")
    g.add_argument(
        "--layout",
        choices=["1mib", "true"],
        default="1mib",
        help="1mib=1MiB 紧凑镜像(默认), true=真实 H3C 偏移布局(~1.75MiB)",
    )
    g.add_argument("-o", "--out", default="factory-h3c-1mib.bin")

    p = sub.add_parser("parse", help="解析真实 reservearea 备份")
    p.add_argument("--bin", required=True)

    args = ap.parse_args()
    if args.cmd == "gen":
        mac = parse_mac(args.mac)
        if not mac:
            sys.exit("MAC 无效: %s" % args.mac)
        if len(args.sn) != DEVICE_SN_SIZE:
            print(
                "警告: SN 长度 %d != %d, 高位将被截断"
                % (len(args.sn), DEVICE_SN_SIZE)
            )
        cal = open(args.cal, "rb").read() if args.cal else None
        eep = open(args.eeprom, "rb").read() if args.eeprom else None
        if args.layout == "1mib":
            img = encode_1mib(args.sn, mac, cal, eep)
        else:
            img = encode(args.sn, mac, cal, eep)
        open(args.out, "wb").write(img)
        e0 = derive_mac(mac, OFF_ETH0)
        p0 = derive_mac(mac, OFF_PON0)
        w5 = derive_mac(mac, OFF_WIFI_5G)
        w24 = derive_mac(mac, OFF_WIFI_24G)
        print("已写出 %d 字节 -> %s" % (len(img), args.out))
        print("  Device SN : %s" % args.sn)
        print("  Label MAC : %s" % mac_to_string(mac))
        print("  eth0  +1  : %s" % mac_to_string(e0))
        print("  pon0  +2  : %s" % mac_to_string(p0))
        print("  5G    +4  : %s" % mac_to_string(w5))
        print("  2.4G  +5  : %s" % mac_to_string(w24))
    elif args.cmd == "parse":
        data = open(args.bin, "rb").read()
        sn, label, cal, eep = parse_volume(data)
        print("文件: %s (%d 字节)" % (args.bin, len(data)))
        print("  Device SN : %r" % sn)
        print("  Label MAC : %s" % (mac_to_string(label) if label else "<未找到>"))
        if label:
            print("  eth0  +1  : %s" % mac_to_string(derive_mac(label, OFF_ETH0)))
            print("  pon0  +2  : %s" % mac_to_string(derive_mac(label, OFF_PON0)))
            print("  5G    +4  : %s" % mac_to_string(derive_mac(label, OFF_WIFI_5G)))
            print("  2.4G  +5  : %s" % mac_to_string(derive_mac(label, OFF_WIFI_24G)))
        chip = detect_pon_chip(cal)
        print("  PON 芯片   : %s" % (chip if chip else "<未识别/未设置>"))
        print("  calibration: %d 字节" % len(cal))
        print("  eeprom    : %d 字节" % len(eep))
    else:
        ap.print_help()


if __name__ == "__main__":
    main()
