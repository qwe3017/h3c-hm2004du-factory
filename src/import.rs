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
/// Auto-detects the 1 MiB compact bundle (magic `H3C-1MiB`) written by
/// [`Factory::encode_1mib`] versus the real-layout demo image / true backup.
pub fn parse_factory_volume(data: &[u8]) -> Factory {
    if data.starts_with(&COMPACT_MAGIC) {
        return parse_compact_1mib(data);
    }
    let mut device_sn = read_device_sn(data);

    let almac = read_almac_bytes(data);

    // Some units ship the 0x14102c slot blank; ctromfile carries the same
    // serial, so fall back to it instead of leaving the field empty.
    if device_sn.is_empty() {
        if let Some(text) = &almac {
            if let Some(sn) = find_serial_number(text) {
                device_sn = sn;
            }
        }
    }

    let label_mac = almac
        .as_deref()
        .and_then(find_al_mac)
        .or_else(|| read_base_mac_placeholder(data))
        .unwrap_or([0u8; 6]);

    let calibration = read_region(data, CALIBRATION_OFFSET, CALIBRATION_SIZE);
    let wifi_eeprom = read_region(data, WIFI_EEPROM_OFFSET, WIFI_EEPROM_SIZE);

    Factory {
        device_sn,
        label_mac,
        calibration,
        wifi_eeprom,
    }
}

/// Parse the 1 MiB compact bundle written by `Factory::encode_1mib`.
fn parse_compact_1mib(data: &[u8]) -> Factory {
    let label_mac = read_mac6(data, C_LABEL_MAC_OFFSET).unwrap_or([0u8; 6]);
    let mut device_sn = String::new();
    if let Some(raw) = data.get(C_DEVICE_SN_OFFSET..C_DEVICE_SN_OFFSET + DEVICE_SN_SIZE) {
        device_sn = String::from_utf8_lossy(raw)
            .trim_matches(|c| c == '\0' || c == ' ' || c == '\u{fffd}')
            .to_string();
    }
    let calibration = read_region(data, C_CALIBRATION_OFFSET, CALIBRATION_SIZE);
    let wifi_eeprom = read_region(data, C_WIFI_EEPROM_OFFSET, WIFI_EEPROM_SIZE);
    Factory {
        device_sn,
        label_mac,
        calibration,
        wifi_eeprom,
    }
}

fn read_mac6(data: &[u8], off: usize) -> Option<[u8; 6]> {
    let m = data.get(off..off + 6)?;
    let mut a = [0u8; 6];
    a.copy_from_slice(m);
    Some(a)
}

/// The AL-MAC region contents: the *inflated* gzip ctromfile for a real backup,
/// or the raw region when it is already plain text (our own demo export).
fn read_almac_bytes(data: &[u8]) -> Option<Vec<u8>> {
    let region = data.get(ALMAC_REGION_OFFSET..)?;
    if let Some(start) = find_subseq(region, &[0x1f, 0x8b, 0x08]) {
        let mut out = Vec::new();
        // GzDecoder stops at the end of the gzip member; the 0xff padding that
        // follows is never read.
        if flate2::read::GzDecoder::new(&region[start..])
            .read_to_end(&mut out)
            .is_ok()
        {
            return Some(out);
        }
    }
    Some(region.to_vec())
}

fn read_region(data: &[u8], off: usize, size: usize) -> Vec<u8> {
    match data.get(off..off + size) {
        Some(slice) => slice.to_vec(),
        None => vec![0xff; size],
    }
}

fn read_device_sn(data: &[u8]) -> String {
    let Some(raw) = data.get(DEVICE_SN_OFFSET..DEVICE_SN_OFFSET + DEVICE_SN_SIZE) else {
        return String::new();
    };
    String::from_utf8_lossy(raw)
        .trim_matches(|c| c == '\0' || c == ' ' || c == '\u{fffd}')
        .to_string()
}

/// Last resort: the base-MAC placeholder at 0x140005.
fn read_base_mac_placeholder(data: &[u8]) -> Option<[u8; 6]> {
    let m = data.get(BASE_MAC_OFFSET..BASE_MAC_OFFSET + BASE_MAC_SIZE)?;
    if m.iter().any(|&b| b != 0xff && b != 0x00) {
        let mut a = [0u8; 6];
        a.copy_from_slice(m);
        Some(a)
    } else {
        None
    }
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

/// Scan `buf` for `SerialNumber="..."` (used to cross-check / recover the SN).
pub fn find_serial_number(buf: &[u8]) -> Option<String> {
    let tag = b"SerialNumber=\"";
    let rel = find_subseq(buf, tag)?;
    let at = rel + tag.len();
    let end = buf.get(at..)?.iter().position(|&b| b == b'"')?;
    if end == 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&buf[at..at + end]).to_string())
}
