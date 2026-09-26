# H3C HM2004-DU Factory Editor

Rust + egui 图形工具，用于**查看 / 编辑 / 对照** H3C HM2004-DU（AN7581 FTTR 主网关）出厂分区（factory）里的设备身份与校准数据。
本项目是 `fiberhome-factory` 的 **H3C 改写版**：布局、导入逻辑、MAC 派生规则全部针对 HM2004-DU 重新实现。

> ⚠️ **重要声明**
> - 本工具仅供 **研究 / 备份查看 / 对照学习** 使用，与新华三（H3C）无任何关联，未包含任何厂商闭源代码。
> - 导出的「布局映射镜像」**不含 UBI 卷元数据、不对 ctromfile 重压缩**，**不可直接刷写 NAND**。要落盘到真机需用 `ubiupdatevol` 写入真实的 `ubi-volume-factory` 卷（见下文「进阶」）。
> - 修改设备出厂数据可能导致设备无法启动或违反当地法规，请仅在自己拥有的设备上操作。

## 出厂分区布局（H3C HM2004-DU）

HM2004-DU 的 factory 是一个 UBI 卷（`ubi-volume-factory`，2_359_296 字节）。可读字段的真实偏移：

| 偏移 | 长度 | 字段 | 说明 |
|------|------|------|------|
| `0x140000` | 4 | magic | `21 43 34 12` (`0x12344321`) |
| `0x140005` | 6 | base MAC 占位 | `02:00:00:XX:00:00`（非真实地址） |
| `0x14102c` | 12 | Device SN | ASCII，重复于 `0x14105f` |
| `0x80000` | — | ctromfile (gzip) | 真实 label MAC 在 `AL-MAC="..."` 内 |
| `0x1c0400` | 0x200 | PON 校准 | EN7572 / UX3326 BOSA |
| `0x4c000` | 0x1000 | Wi-Fi EEPROM | MT7916 DBDC |

真实 label MAC **不**存在可寻址的 factory 字段里：原厂把出厂区相应槽位留空，MAC 只以 `AL-MAC` 形式存在于 `0x80000` 的 gzip ctromfile 中（与 `02_network::h3c_hm2004du_base_mac` 一致）。
因此本工具**导入时解 gzip 取 `AL-MAC`**，**导出时在 `0x80000` 写明文 ctromfile 块**（便于对照，不重压缩）。

## MAC 派生（末字节偏移）

与 `02_network::h3c_mac_offset` 一致：

| 接口 | 偏移 | 示例（label = `44:76:09:65:06:4A`） |
|------|------|------|
| label / br-lan / LAN | 0 | `44:76:09:65:06:4A` |
| eth0 | +1 | `44:76:09:65:06:4B` |
| pon0 / wan | +2 | `44:76:09:65:06:4C` |
| 5G Wi-Fi | +4 | `44:76:09:65:06:4E` |
| 2.4G Wi-Fi | +5 | `44:76:09:65:06:4F` |

（FiberHome 原版用 `2.4G=base+1` / `5G=base+8`，规则不同。）

## 构建与运行（图形界面，需 Rust 工具链）

```bash
cargo build --release
cargo run --release
```

界面：编辑 Device SN / label MAC → 实时显示派生 MAC → 载入校准 / EEPROM bin → 导入真实 factory 卷 → 保存布局映射镜像。

## Python 伴生工具（无需 Rust）

`h3c_factory_demo.py` 是命令行版，逻辑与 `src/model.rs::encode()` 完全一致：

```bash
# 生成演示镜像
python3 h3c_factory_demo.py gen --sn H3CT0005EBF0 --mac 44:76:09:65:06:4a -o factory-h3c-demo.bin

# 从真实 reservearea 备份解析
python3 h3c_factory_demo.py parse --bin mtd14_reservearea.bin
```

示例产物见 [`examples/factory-h3c-demo.bin`](examples/factory-h3c-demo.bin)。

> 导出镜像固定为 **1,839,104 字节**（`0x1c1000`，≈1.75 MiB），这是容纳真实字段的最小尺寸。
> **无法压缩到 1 MiB**：H3C 的身份字段（魔数 / SN / base-MAC @ `0x140000+`、PON 校准 @ `0x1c0400`）全部落在 1 MiB 之后，硬截断会丢失 SN、MAC、魔数与校准。

## 目录结构

```
h3c-hm2004du-factory/
├── Cargo.toml
├── LICENSE                    # GPL-2.0-only
├── README.md
├── .gitignore
├── .github/workflows/build.yml # GitHub Actions：三平台构建 + tag 自动发 Release
├── src/
│   ├── font.rs      # CJK 字体安装（fontdb，移植自上游）
│   ├── main.rs      # egui 编辑器
│   ├── model.rs     # H3C 布局常量 + Factory + encode
│   └── import.rs    # 解析真实 factory 卷
├── h3c_factory_demo.py
└── examples/
    └── factory-h3c-demo.bin
```

## 进阶：生成可刷写卷

当前 `encode()` 输出演示镜像。若要生成可被 `ubiupdatevol` 写入的整卷，需要：

1. 用 `ubinfo` 取得真实卷大小并对齐到 NAND 页 / 块；
2. 在 `0x80000` 处重新 gzip 压缩 ctromfile（保留 `AL-MAC` / `SerialNumber`）；
3. 由 ubi 工具写入卷，而不是裸写文件。

欢迎 PR。

## 许可证

**[GPL-2.0-only](LICENSE)**。

本项目是上游 `fiberhome-factory` 项目的 **H3C 改写 / 重实现**：整体架构与出厂分区编辑思路借鉴自上游，`src/font.rs` 的 CJK 字体安装逻辑直接移植自上游 `font.rs`。上游以 **GPL-2.0-only** 发布，故本派生作品沿用同一许可证。

若需改用其他许可证（如 MIT），请先移除所有源自上游的代码。

感谢原作者的工作。
