//! Import an H3C factory UBI-volume image (e.g. a `reservearea` backup `.bin`).
//!
//! This is the H3C analog of the original `import.rs`. The original dug a
//! `fhdata` UBI volume or a JFFS2 partition out of a FiberHome dump; H3C gives
//! us the factory volume directly, so import is just "parse the raw bytes at
//! the known offsets". The only non-trivial part is recovering `AL-MAC` from the
//! gzip member at 0x80000 — that mirrors `02_network::h3c_hm2004du_base_mac`.

use crate::model::*;
use std::io::Read;

/// Parse a raw H3C factory volume and recover the editable fields.
pub fn parse_factory_volume(data: &[u8]) -> Factory {
    let device_sn = read_device_sn(data);
    let label_mac = read_label_mac(data).unwrap_or([0u8; 6]);
    let calibration = read_region(data, CALIBRATION_OFFSET, CALIBRATION_SIZE);
    let wifi_eeprom = read_region(data, WIFI_EEPROM_OFFSET, WIFI_EEPROM_SIZE);
    Factory {
        device_sn,
        label_mac,
        calibration,
        wifi_eeprom,
    }
}

fn read_region(data: &[u8], off: usize, size: usize) -> Vec<u8> {
    if data.len() >= off + size {
        data[off..off + size].to_vec()
    } else {
        vec![0xff; size]
    }
}

fn read_device_sn(data: &[u8]) -> String {
    if data.len() < DEVICE_SN_OFFSET + DEVICE_SN_SIZE {
        return String::new();
    }
    let raw = &data[DEVICE_SN_OFFSET..DEVICE_SN_OFFSET + DEVICE_SN_SIZE];
    String::from_utf8_lossy(raw)
        .trim_matches(|c| c == '\0' || c == ' ' || c == '\u{fffd}')
        .to_string()
}

/// Recover the label MAC. Tries, in order:
///   1. `AL-MAC="..."` inside the gzip ctromfile at 0x80000
///   2. `AL-MAC="..."` as plain text (our own demo export)
///   3. the 0x140005 base-MAC placeholder
pub fn read_label_mac(data: &[u8]) -> Option<[u8; 6]> {
    if data.len() > ALMAC_REGION_OFFSET {
        let region = &data[ALMAC_REGION_OFFSET..];
        if let Some(start) = find_subseq(region, &[0x1f, 0x8b, 0x08]) {
            let slice = &region[start..];
            let mut out = Vec::new();
            if flate2::read::GzDecoder::new(slice).read_to_end(&mut out).is_ok() {
                if let Some(m) = find_al_mac(&out) {
                    return Some(m);
                }
            }
        }
        // Plain-text fallback (our own demo export, or any uncompressed copy).
        if let Some(m) = find_al_mac(region) {
            return Some(m);
        }
    }
    if data.len() >= BASE_MAC_OFFSET + BASE_MAC_SIZE {
        let m = &data[BASE_MAC_OFFSET..BASE_MAC_OFFSET + BASE_MAC_SIZE];
        if m.iter().any(|&b| b != 0xff && b != 0x00) {
            let mut a = [0u8; 6];
            a.copy_from_slice(m);
            return Some(a);
        }
    }
    None
}

pub fn find_subseq(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Scan `buf` for the first `AL-MAC="xx:xx:xx:xx:xx:xx"` and parse it.
pub fn find_al_mac(buf: &[u8]) -> Option<[u8; 6]> {
    let tag = b"AL-MAC=\"";
    let mut from = 0;
    while from + tag.len() + 17 <= buf.len() {
        match find_subseq(&buf[from..], tag) {
            Some(rel) => {
                let at = from + rel + tag.len();
                if let Some(m) = parse_mac(&String::from_utf8_lossy(&buf[at..at + 17])) {
                    return Some(m);
                }
                from = at; // skip past this occurrence and keep scanning
            }
            None => break,
        }
    }
    None
}

/// Scan `buf` for `SerialNumber="..."` (used for cross-checking the SN).
pub fn find_serial_number(buf: &[u8]) -> Option<String> {
    let tag = b"SerialNumber=\"";
    if let Some(rel) = find_subseq(buf, tag) {
        let at = rel + tag.len();
        if let Some(end) = buf[at..].iter().position(|&b| b == b'"') {
            if end > 0 {
                return Some(String::from_utf8_lossy(&buf[at..at + end]).to_string());
            }
        }
    }
    None
}
