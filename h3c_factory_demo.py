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


def parse_volume(data):
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

    g = sub.add_parser("gen", help="生成布局映射演示镜像")
    g.add_argument("--sn", required=True)
    g.add_argument("--mac", required=True)
    g.add_argument("--cal", help="校准 bin (可选)")
    g.add_argument("--eeprom", help="EEPROM bin (可选)")
    g.add_argument("-o", "--out", default="factory-h3c-demo.bin")

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
        print("  calibration: %d 字节" % len(cal))
        print("  eeprom    : %d 字节" % len(eep))
    else:
        ap.print_help()


if __name__ == "__main__":
    main()
