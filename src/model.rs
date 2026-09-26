//! H3C HM2004-DU factory partition layout.
//!
//! This module mirrors `model.rs` from the original `fiberhome-factory`
//! project, but retargets every offset to the real H3C layout. On the
//! HM2004-DU the "factory" partition is a UBI volume (`ubi-volume-factory`,
//! 2_359_296 bytes) whose raw nvmem cells are:
//!
//! ```text
//! 0x140000  magic 21 43 34 12  (0x12344321)
//! 0x140005  base-MAC placeholder 02:00:00:XX:00:00  (NOT a usable address)
//! 0x14102c  Device SN, 12-byte ASCII   (duplicate at 0x14105f)
//! 0x1c0400  PON calibration, 0x200 bytes (EN7572 / UX3326 BOSA)
//! 0x4c000   Wi-Fi EEPROM, 0x1000 bytes (MT7916 DBDC)
//! 0x80000   gzip of ctromfile.cfg — the real label MAC is AL-MAC="..." inside it
//! ```
//!
//! The label MAC is NOT stored in an addressable field: stock firmware keeps
//! the reservearea slot blank and recovers it from `AL-MAC` inside the gzipped
//! ctromfile at 0x80000 (see `02_network::h3c_hm2004du_base_mac`). This tool
//! therefore reads/writes `AL-MAC` rather than the 0x140005 placeholder.
//!
//! MAC derivation (last byte only, from `02_network::h3c_mac_offset`):
//!   eth0 = label+1, pon0/wan = label+2, 5G = label+4, 2.4G = label+5.
//!
//! The `encode()` export here is a *layout-mapped demo image*: it places every
//! field at its true H3C offset into a single buffer, but it does NOT synthesize
//! UBI EC/VID headers and it writes the AL-MAC region as readable plain text
//! instead of re-gzipping ctromfile.cfg. It is for viewing/comparison only and
//! is NOT directly flashable to NAND.

// Real ubi-volume-factory size.
pub const FACTORY_VOLUME_SIZE: usize = 0x240000; // 2_359_296
// Layout-mapped demo export size: highest used field rounded up to 4 KiB.
pub const DEMO_SIZE: usize = 0x1c1000; // 1_843_200

pub const MAGIC_OFFSET: usize = 0x140000;
pub const MAGIC: [u8; 4] = [0x21, 0x43, 0x34, 0x12];

pub const BASE_MAC_OFFSET: usize = 0x140005;
pub const BASE_MAC_SIZE: usize = 6;

pub const DEVICE_SN_OFFSET: usize = 0x14102c;
pub const DEVICE_SN_DUP_OFFSET: usize = 0x14105f;
pub const DEVICE_SN_SIZE: usize = 0x0c;

pub const ALMAC_REGION_OFFSET: usize = 0x80000;
pub const ALMAC_REGION_SIZE: usize = 0x4000;

pub const CALIBRATION_OFFSET: usize = 0x1c0400;
pub const CALIBRATION_SIZE: usize = 0x200;

pub const WIFI_EEPROM_OFFSET: usize = 0x4c000;
pub const WIFI_EEPROM_SIZE: usize = 0x1000;

// ---- 1 MiB compact layout (self-describing, NOT flashable) ----
//
// The real H3C identity fields sit at 0x140000+, i.e. beyond 1 MiB, so a
// 1 MiB file cannot keep the true offsets. This layout packs every field
// into the first 1 MiB at fixed, documented offsets. Parsing auto-detects
// it via the ASCII magic.
pub const COMPACT_SIZE: usize = 0x100000; // 1_048_576
pub const COMPACT_MAGIC: [u8; 8] = *b"H3C-1MiB";

pub const C_LABEL_MAC_OFFSET: usize = 0x0010; // label / br-lan / LAN
pub const C_ETH0_MAC_OFFSET: usize = 0x0020; // label+1
pub const C_PON0_MAC_OFFSET: usize = 0x0030; // label+2 (pon0/wan)
pub const C_WIFI5G_MAC_OFFSET: usize = 0x0040; // label+4
pub const C_WIFI24_MAC_OFFSET: usize = 0x0050; // label+5
pub const C_DEVICE_SN_OFFSET: usize = 0x0060; // 12-byte ASCII
pub const C_WIFI_EEPROM_OFFSET: usize = 0x1000; // 0x1000, MT7916
pub const C_CALIBRATION_OFFSET: usize = 0x2000; // 0x200, EN7572

// MAC derivation offsets (applied to the last byte only).
pub const OFF_ETH0: u8 = 1;
pub const OFF_PON0: u8 = 2;
pub const OFF_WIFI_5G: u8 = 4;
pub const OFF_WIFI_24G: u8 = 5;

#[derive(Clone)]
pub struct Factory {
    pub device_sn: String,
    pub label_mac: [u8; 6],
    pub calibration: Vec<u8>, // 0x200
    pub wifi_eeprom: Vec<u8>, // 0x1000
}

impl Factory {
    pub fn new() -> Self {
        Factory {
            device_sn: random_sn(),
            label_mac: random_mac(),
            calibration: vec![0xff; CALIBRATION_SIZE],
            wifi_eeprom: vec![0xff; WIFI_EEPROM_SIZE],
        }
    }

    /// eth0 (+1), pon0/wan (+2), 5G (+4), 2.4G (+5) — all on the last byte.
    pub fn derive_macs(&self) -> ([u8; 6], [u8; 6], [u8; 6], [u8; 6]) {
        (
            derive_mac(&self.label_mac, OFF_ETH0),
            derive_mac(&self.label_mac, OFF_PON0),
            derive_mac(&self.label_mac, OFF_WIFI_5G),
            derive_mac(&self.label_mac, OFF_WIFI_24G),
        )
    }

    /// Build the layout-mapped demo image (not flashable).
    pub fn encode(&self) -> Vec<u8> {
        let mut img: Vec<u8> = vec![0xff; DEMO_SIZE];

        img[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
        img[BASE_MAC_OFFSET..BASE_MAC_OFFSET + 6].copy_from_slice(&self.label_mac);

        let sn = self.device_sn.as_bytes();
        let n = sn.len().min(DEVICE_SN_SIZE);
        if n > 0 {
            img[DEVICE_SN_OFFSET..DEVICE_SN_OFFSET + n].copy_from_slice(&sn[..n]);
            img[DEVICE_SN_DUP_OFFSET..DEVICE_SN_DUP_OFFSET + n].copy_from_slice(&sn[..n]);
        }

        // AL-MAC region: plain-text ctromfile block for easy viewing/comparison.
        let block = format!(
            "<ctromfile>\n  <mapcfg AL-MAC=\"{}\" />\n  <sysinfo SerialNumber=\"{}\" />\n</ctromfile>\n",
            mac_to_string(&self.label_mac),
            self.device_sn
        );
        let b = block.as_bytes();
        let bn = b.len().min(ALMAC_REGION_SIZE);
        img[ALMAC_REGION_OFFSET..ALMAC_REGION_OFFSET + bn].copy_from_slice(&b[..bn]);

        let cn = self.calibration.len().min(CALIBRATION_SIZE);
        if cn > 0 {
            img[CALIBRATION_OFFSET..CALIBRATION_OFFSET + cn].copy_from_slice(&self.calibration[..cn]);
        }
        let en = self.wifi_eeprom.len().min(WIFI_EEPROM_SIZE);
        if en > 0 {
            img[WIFI_EEPROM_OFFSET..WIFI_EEPROM_OFFSET + en].copy_from_slice(&self.wifi_eeprom[..en]);
        }

        img
    }

    /// 1 MiB compact image: every field packed at fixed documented offsets so
    /// the whole bundle fits into 1 MiB. Carries the label MAC plus all four
    /// derived addresses (eth0=+1, pon0=+2, 5G=+4, 2.4G=+5), Device SN,
    /// Wi-Fi EEPROM and PON calibration. NOT flashable.
    pub fn encode_1mib(&self) -> Vec<u8> {
        let mut img: Vec<u8> = vec![0xff; COMPACT_SIZE];
        img[0..COMPACT_MAGIC.len()].copy_from_slice(&COMPACT_MAGIC);

        let (e0, p0, w5, w24) = self.derive_macs();
        for (off, mac) in [
            (C_LABEL_MAC_OFFSET, &self.label_mac),
            (C_ETH0_MAC_OFFSET, &e0),
            (C_PON0_MAC_OFFSET, &p0),
            (C_WIFI5G_MAC_OFFSET, &w5),
            (C_WIFI24_MAC_OFFSET, &w24),
        ] {
            img[off..off + 6].copy_from_slice(mac);
        }

        let sn = self.device_sn.as_bytes();
        let n = sn.len().min(DEVICE_SN_SIZE);
        if n > 0 {
            img[C_DEVICE_SN_OFFSET..C_DEVICE_SN_OFFSET + n].copy_from_slice(&sn[..n]);
        }

        let en = self.wifi_eeprom.len().min(WIFI_EEPROM_SIZE);
        if en > 0 {
            img[C_WIFI_EEPROM_OFFSET..C_WIFI_EEPROM_OFFSET + en]
                .copy_from_slice(&self.wifi_eeprom[..en]);
        }
        let cn = self.calibration.len().min(CALIBRATION_SIZE);
        if cn > 0 {
            img[C_CALIBRATION_OFFSET..C_CALIBRATION_OFFSET + cn]
                .copy_from_slice(&self.calibration[..cn]);
        }

        img
    }

    pub fn summary(&self) -> String {
        let (e0, p0, w5, w24) = self.derive_macs();
        format!(
            "Device SN : {}\nLabel MAC : {}\neth0  (+1): {}\npon0  (+2): {}\n5G    (+4): {}\n2.4G  (+5): {}\ncalibration: {} bytes\neeprom    : {} bytes",
            self.device_sn,
            mac_to_string(&self.label_mac),
            mac_to_string(&e0),
            mac_to_string(&p0),
            mac_to_string(&w5),
            mac_to_string(&w24),
            self.calibration.len(),
            self.wifi_eeprom.len(),
        )
    }
}

/// Parse a MAC from `aa:bb:cc:dd:ee:ff`, `aa-bb-...` or 12 raw hex digits.
/// Rejects all-zero, all-ff and multicast addresses.
pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let s = s.trim();
    let parts: Vec<&str> = if s.contains(':') {
        s.split(':').collect()
    } else if s.contains('-') {
        s.split('-').collect()
    } else if s.len() == 12 {
        (0..12).step_by(2).map(|i| &s[i..i + 2]).collect()
    } else {
        return None;
    };
    if parts.len() != 6 {
        return None;
    }
    let mut m = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        m[i] = u8::from_str_radix(p, 16).ok()?;
    }
    if m == [0u8; 6] || m == [0xff; 6] {
        return None;
    }
    if m[0] & 0x01 != 0 {
        return None; // multicast
    }
    Some(m)
}

/// `aa:BB:cc:DD:ee:FF` (uppercase, colon separated).
pub fn mac_to_string(m: &[u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        m[0], m[1], m[2], m[3], m[4], m[5]
    )
}

/// H3C adds the offset to the last byte only (see `h3c_mac_offset`).
pub fn derive_mac(base: &[u8; 6], off: u8) -> [u8; 6] {
    let mut m = *base;
    m[5] = m[5].wrapping_add(off);
    m
}

/// Locally-administered, unicast random MAC.
pub fn random_mac() -> [u8; 6] {
    let mut m = [0u8; 6];
    // getrandom 0.3 exposes `fill`, not `getrandom`.
    let _ = getrandom::fill(&mut m);
    // Set the locally administered bit and clear the multicast bit.
    m[0] = (m[0] & 0xfc) | 0x02;
    m
}

/// `H3CT` + 8 random uppercase hex = 12 chars, exactly matching DEVICE_SN_SIZE
/// and the real device serial format (`H3CT0005EBF0`).
pub fn random_sn() -> String {
    let mut b = [0u8; 4];
    let _ = getrandom::fill(&mut b);
    format!("H3CT{:08X}", u32::from_be_bytes(b))
}
