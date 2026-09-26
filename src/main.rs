//! H3C HM2004-DU Factory Editor — egui port of `fiberhome-factory`.
//!
//! Faithful rewrite of the FiberHome editor, retargeted to the H3C layout:
//!   * Device SN, label MAC (AL-MAC) and derived MACs instead of FiberHome's
//!     base/2.4G/5G triple.
//!   * Import reads a raw H3C factory volume (reservearea backup) instead of a
//!     `fhdata` UBI volume / JFFS2 partition.
//!   * Save writes a demo image in either the 1 MiB compact bundle or the
//!     true-offset layout — see `model.rs` for the rationale.
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
    Factory, OFF_ETH0, OFF_PON0, OFF_WIFI_24G, OFF_WIFI_5G, derive_mac, mac_to_string, parse_mac,
};
use rfd::FileDialog;

struct Editor {
    factory: Factory,
    device_sn: String,
    label_mac: String,
    msg: String,
    calib_path: String,
    eeprom_path: String,
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
}

impl eframe::App for Editor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading("H3C HM2004-DU 工厂分区编辑器");
        ui.label(
            "布局映射(演示/对照)：SN@0x14102c, magic@0x140000, AL-MAC(明文)@0x80000, \
             校准@0x1c0400, EEPROM@0x4c000",
        );
        ui.label("注：导出为演示镜像（1MiB 紧凑 / 真实偏移两种布局），不含 UBI 元数据，不可直接刷写 NAND。");

        ui.separator();
        ui.label("设备序列号 (Device SN, 12 字节 ASCII):");
        ui.text_edit_singleline(&mut self.device_sn);

        ui.label("标签 MAC (AL-MAC / label):");
        ui.text_edit_singleline(&mut self.label_mac);

        // Derive live from what is typed, so the preview does not lag behind
        // the text box until the next save/import.
        let base = parse_mac(&self.label_mac).unwrap_or(self.factory.label_mac);
        ui.label(format!(
            "派生 MAC (末字节偏移): eth0=+1 -> {} | pon0/wan=+2 -> {} | 5G=+4 -> {} | 2.4G=+5 -> {}",
            mac_to_string(&derive_mac(&base, OFF_ETH0)),
            mac_to_string(&derive_mac(&base, OFF_PON0)),
            mac_to_string(&derive_mac(&base, OFF_WIFI_5G)),
            mac_to_string(&derive_mac(&base, OFF_WIFI_24G)),
        ));

        ui.separator();
        ui.label("PON 校准 (0x200, EN7572/UX3326):");
        if ui.button("载入校准 bin…").clicked() {
            if let Some(p) = FileDialog::new().pick_file() {
                if let Ok(b) = std::fs::read(&p) {
                    self.factory.calibration = b;
                    self.calib_path = p.display().to_string();
                }
            }
        }
        ui.label(format!(
            "校准: {} 字节  {}",
            self.factory.calibration.len(),
            self.calib_path
        ));

        ui.label("Wi-Fi EEPROM (0x1000, MT7916):");
        if ui.button("载入 EEPROM bin…").clicked() {
            if let Some(p) = FileDialog::new().pick_file() {
                if let Ok(b) = std::fs::read(&p) {
                    self.factory.wifi_eeprom = b;
                    self.eeprom_path = p.display().to_string();
                }
            }
        }
        ui.label(format!(
            "EEPROM: {} 字节  {}",
            self.factory.wifi_eeprom.len(),
            self.eeprom_path
        ));

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("导入工厂卷…").clicked() {
                if let Some(p) = FileDialog::new().add_filter("bin", &["bin"]).pick_file() {
                    match std::fs::read(&p) {
                        Ok(b) => {
                            self.factory = import::parse_factory_volume(&b);
                            self.sync_from_factory();
                            self.msg = format!("已导入: {}", p.display());
                        }
                        Err(e) => self.msg = format!("读取失败: {e}"),
                    }
                }
            }
            if ui.button("随机生成").clicked() {
                self.factory = Factory::new();
                self.sync_from_factory();
                self.msg = "已随机生成".into();
            }
            if ui.button("保存 1MiB 镜像…").clicked() {
                self.apply_to_factory();
                let img = self.factory.encode_1mib();
                if let Some(p) = FileDialog::new()
                    .set_file_name("factory-h3c-1mib.bin")
                    .save_file()
                {
                    match std::fs::write(&p, &img) {
                        Ok(_) => {
                            self.msg = format!("已保存 {} 字节 -> {}", img.len(), p.display());
                        }
                        Err(e) => self.msg = format!("保存失败: {e}"),
                    }
                }
            }
            if ui.button("保存真实布局镜像…").clicked() {
                self.apply_to_factory();
                let img = self.factory.encode();
                if let Some(p) = FileDialog::new()
                    .set_file_name("factory-h3c-layout.bin")
                    .save_file()
                {
                    match std::fs::write(&p, &img) {
                        Ok(_) => {
                            self.msg = format!("已保存 {} 字节 -> {}", img.len(), p.display());
                        }
                        Err(e) => self.msg = format!("保存失败: {e}"),
                    }
                }
            }
        });

        ui.separator();
        ui.label(&self.msg);
        ui.separator();
        ui.monospace(self.factory.summary());
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([680.0, 520.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "H3C HM2004-DU Factory Editor",
        options,
        Box::new(|cc| {
            font::install(&cc.egui_ctx);
            let mut ed = Editor {
                factory: Factory::new(),
                device_sn: String::new(),
                label_mac: String::new(),
                msg: String::new(),
                calib_path: String::new(),
                eeprom_path: String::new(),
            };
            ed.sync_from_factory();
            Ok(Box::new(ed))
        }),
    )
}
