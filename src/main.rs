#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! H3C HM2004-DU Factory Editor — egui port of `fiberhome-factory`.
//!
//! The UI deliberately mirrors the upstream FiberHome editor: a top bar with
//! 新建 / 打开 / 保存, a status line, an 身份信息 section with per-field 随机
//! buttons, and framed 替换 / 导出 / 清空 rows for the binary blobs.
//!
//! Note: eframe 0.36 replaced `App::update(ctx, frame)` with
//! `App::ui(&mut self, ui: &mut Ui, frame)` — the panel is provided, so there
//! is no `CentralPanel` here.
//!
//! Build: `cargo build --release`
//! Run:   `cargo run --release`

mod font;
mod import;
mod model;

use eframe::egui;
use model::{
    Factory, CALIBRATION_SIZE, OFF_ETH0, OFF_PON0, OFF_WIFI_24G, OFF_WIFI_5G, WIFI_EEPROM_SIZE,
    derive_mac, mac_to_string, parse_mac, random_mac, random_sn,
};
use rfd::FileDialog;

struct Editor {
    factory: Factory,
    device_sn: String,
    label_mac: String,
    msg: String,
}

impl Editor {
    fn sync_from_factory(&mut self) {
        self.device_sn = self.factory.device_sn.clone();
        self.label_mac = mac_to_string(&self.factory.label_mac);
    }

    fn apply_to_factory(&mut self) {
        if let Some(m) = parse_mac(&self.label_mac) {
            self.factory.label_mac = m;
        }
        self.factory.device_sn = self.device_sn.clone();
    }

    fn save_image(&mut self, img: Vec<u8>, default_name: &str) {
        if let Some(p) = FileDialog::new().set_file_name(default_name).save_file() {
            match std::fs::write(&p, &img) {
                Ok(_) => self.msg = format!("已保存 {} 字节 -> {}", img.len(), p.display()),
                Err(e) => self.msg = format!("保存失败: {e}"),
            }
        }
    }
}

impl eframe::App for Editor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // ---- 顶栏：标题 + 动作 ----
        ui.horizontal(|ui| {
            ui.heading("H3C Factory");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("保存 1 MiB").clicked() {
                    self.apply_to_factory();
                    let img = self.factory.encode_1mib();
                    self.save_image(img, "factory-h3c-1mib.bin");
                }
                if ui.button("保存真实布局").clicked() {
                    self.apply_to_factory();
                    let img = self.factory.encode();
                    self.save_image(img, "factory-h3c-layout.bin");
                }
                if ui.button("打开").clicked() {
                    if let Some(p) = FileDialog::new().add_filter("bin", &["bin"]).pick_file() {
                        match std::fs::read(&p) {
                            Ok(b) => {
                                self.factory = import::parse_factory_volume(&b);
                                self.sync_from_factory();
                                self.msg = format!("已打开: {}", p.display());
                            }
                            Err(e) => self.msg = format!("读取失败: {e}"),
                        }
                    }
                }
                if ui.button("新建").clicked() {
                    self.factory = Factory::new();
                    self.sync_from_factory();
                    self.msg = "新建 Factory".into();
                }
            });
        });
        ui.label(&self.msg);
        ui.separator();

        // ---- 身份信息 ----
        ui.heading("身份信息");
        const FIELD: f32 = 280.0;
        ui.horizontal(|ui| {
            ui.label("标签 MAC");
            ui.add(
                egui::TextEdit::singleline(&mut self.label_mac).desired_width(FIELD),
            );
            if ui.button("随机").clicked() {
                self.factory.label_mac = random_mac();
                self.label_mac = mac_to_string(&self.factory.label_mac);
            }
        });
        ui.horizontal(|ui| {
            ui.label("设备序列号");
            ui.add(
                egui::TextEdit::singleline(&mut self.device_sn).desired_width(FIELD),
            );
            if ui.button("随机").clicked() {
                self.factory.device_sn = random_sn();
                self.device_sn = self.factory.device_sn.clone();
            }
        });

        ui.add_space(6.0);
        ui.label("派生 MAC（末字节偏移，随标签 MAC 实时计算）");
        let base = parse_mac(&self.label_mac).unwrap_or(self.factory.label_mac);
        egui::Grid::new("derived_macs")
            .num_columns(2)
            .spacing([16.0, 3.0])
            .show(ui, |ui| {
                ui.label("eth0  (+1)");
                ui.monospace(mac_to_string(&derive_mac(&base, OFF_ETH0)));
                ui.end_row();
                ui.label("pon0  (+2)");
                ui.monospace(mac_to_string(&derive_mac(&base, OFF_PON0)));
                ui.end_row();
                ui.label("5G    (+4)");
                ui.monospace(mac_to_string(&derive_mac(&base, OFF_WIFI_5G)));
                ui.end_row();
                ui.label("2.4G  (+5)");
                ui.monospace(mac_to_string(&derive_mac(&base, OFF_WIFI_24G)));
                ui.end_row();
            });

        // ---- 数据区 ----
        ui.add_space(8.0);
        blob_section(
            ui,
            "PON 校准 (EN7572/UX3326)",
            &mut self.factory.calibration,
            CALIBRATION_SIZE,
            &mut self.msg,
        );
        blob_section(
            ui,
            "Wi-Fi EEPROM (MT7916)",
            &mut self.factory.wifi_eeprom,
            WIFI_EEPROM_SIZE,
            &mut self.msg,
        );

        ui.add_space(8.0);
        ui.weak("导出为演示镜像（不含 UBI 元数据、不重压缩 ctromfile），不可直接刷写 NAND。");
        ui.weak("真实偏移: SN@0x14102c · magic@0x140000 · AL-MAC@gzip 0x80000 · 校准@0x1c0400 · EEPROM@0x4c000");
    }
}

/// One framed blob row, styled like the upstream editor: bold title, a
/// 未设置/已载入 status, and right-aligned 替换 / 导出 / 清空 buttons.
fn blob_section(
    ui: &mut egui::Ui,
    title: &str,
    data: &mut Vec<u8>,
    cap: usize,
    msg: &mut String,
) {
    let loaded = data.len() > cap || data.iter().any(|&b| b != 0xff);
    let status = if loaded {
        format!("已载入 {} 字节", data.len())
    } else {
        "未设置".to_string()
    };
    ui.group(|ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.strong(title);
            ui.weak(status);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("清空").clicked() {
                    *data = vec![0xff; cap];
                    *msg = format!("已清空: {title}");
                }
                if ui.button("导出").clicked() {
                    if let Some(p) = FileDialog::new()
                        .set_file_name("blob.bin")
                        .save_file()
                    {
                        match std::fs::write(&p, data.as_slice()) {
                            Ok(_) => {
                                *msg = format!("已导出 {} 字节 -> {}", data.len(), p.display())
                            }
                            Err(e) => *msg = format!("导出失败: {e}"),
                        }
                    }
                }
                if ui.button("替换").clicked() {
                    if let Some(p) = FileDialog::new().pick_file() {
                        match std::fs::read(&p) {
                            Ok(mut b) => {
                                b.truncate(cap);
                                *data = b;
                                *msg = format!("已载入: {}", p.display());
                            }
                            Err(e) => *msg = format!("读取失败: {e}"),
                        }
                    }
                }
            });
        });
    });
    ui.add_space(4.0);
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([640.0, 480.0])
            .with_min_inner_size([560.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "H3C Factory 编辑器",
        options,
        Box::new(|cc| {
            font::install(&cc.egui_ctx);
            let mut ed = Editor {
                factory: Factory::new(),
                device_sn: String::new(),
                label_mac: String::new(),
                msg: "新建 Factory".to_string(),
            };
            ed.sync_from_factory();
            Ok(Box::new(ed))
        }),
    )
}
