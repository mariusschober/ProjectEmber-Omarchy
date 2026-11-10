use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::drm::DrmSysfs;

/// Parsed EDID identity fields. Raw EDID bytes are never logged or persisted;
/// logs use a salted hash of the derived identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdidInfo {
    pub manufacturer: String,
    pub manufacturer_id: u16,
    pub product_code: u16,
    pub serial_number: u32,
    pub serial_text: Option<String>,
    pub monitor_name: Option<String>,
    pub manufacture_week: u8,
    pub manufacture_year: u16,
    pub width_cm: u8,
    pub height_cm: u8,
    pub sha256: String,
}

impl EdidInfo {
    /// Parses a base EDID block (128 bytes) and optional extension blocks.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 128
            || !bytes.starts_with(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00])
        {
            return None;
        }
        let manufacturer_id = (u16::from(bytes[8]) << 8) | u16::from(bytes[9]);
        let manufacturer = decode_manufacturer(manufacturer_id);
        let product_code = u16::from_le_bytes([bytes[10], bytes[11]]);
        let serial_number = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        let manufacture_week = bytes[16];
        let manufacture_year = 1990 + u16::from(bytes[17]);
        let width_cm = bytes[21];
        let height_cm = bytes[22];

        let mut monitor_name = None;
        let mut serial_text = None;
        for block in bytes[54..126].as_chunks::<18>().0 {
            match block[3] {
                0xFC => monitor_name = Some(decode_text(&block[5..18])),
                0xFF => serial_text = Some(decode_text(&block[5..18])),
                _ => {}
            }
        }

        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        let sha256: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();

        Some(Self {
            manufacturer,
            manufacturer_id,
            product_code,
            serial_number,
            serial_text: serial_text.filter(|value| !value.is_empty()),
            monitor_name: monitor_name.filter(|value| !value.is_empty()),
            manufacture_week,
            manufacture_year,
            width_cm,
            height_cm,
            sha256,
        })
    }

    pub fn label(&self) -> String {
        let name = self
            .monitor_name
            .clone()
            .unwrap_or_else(|| self.manufacturer.clone());
        format!("{name} {:04X}", self.product_code)
    }
}

fn decode_manufacturer(value: u16) -> String {
    let letters = [
        ((value >> 10) & 0x1F) as u8,
        ((value >> 5) & 0x1F) as u8,
        (value & 0x1F) as u8,
    ];
    letters
        .iter()
        .map(|code| {
            if (1..=26).contains(code) {
                (b'A' + code - 1) as char
            } else {
                '?'
            }
        })
        .collect()
}

fn decode_text(bytes: &[u8]) -> String {
    let text: String = bytes
        .iter()
        .take_while(|byte| **byte != 0x0A && **byte != 0x00)
        .map(|byte| *byte as char)
        .collect();
    text.trim().to_string()
}

/// Reads EDID for a connector from DRM sysfs. `connector` is a Wayland/X11
/// output name such as `eDP-1` or `DP-3`.
pub fn edid_for_connector(sysfs: &DrmSysfs, connector: &str) -> Option<EdidInfo> {
    let bytes = sysfs.edid_bytes(connector)?;
    EdidInfo::parse(&bytes)
}

/// Deterministic short identifier for a connector, used as the fallback
/// `unitNumber` in `DisplayIdentity`.
pub fn connector_unit(connector: &str) -> u32 {
    let mut hasher = Sha256::new();
    hasher.update(connector.as_bytes());
    let digest = hasher.finalize();
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) & 0x7FFF_FFFF
}

pub fn read_edid_file(path: &Path) -> Option<Vec<u8>> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() < 128 {
        return None;
    }
    Some(bytes)
}

pub fn sysfs_edid_path(root: &Path, connector: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Kernel names are `card0-DP-1` or `card0-DP-1-1`; match the connector
        // suffix, not a substring, to avoid DP-1 matching DP-10.
        let matches = name == connector
            || name.ends_with(&format!("-{connector}"))
            || name.ends_with(&format!("-{connector}-1"));
        if !matches {
            continue;
        }
        let candidate = entry.path().join("edid");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_edid() -> Vec<u8> {
        let mut bytes = vec![0u8; 256];
        bytes[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        // Manufacturer "DEL" = 0x10AC.
        bytes[8] = 0x10;
        bytes[9] = 0xAC;
        bytes[10] = 0x40;
        bytes[11] = 0xA0;
        bytes[12..16].copy_from_slice(&123_456u32.to_le_bytes());
        bytes[16] = 12;
        bytes[17] = 36; // 2026
        bytes[21] = 60;
        bytes[22] = 34;
        // Monitor name descriptor at 54.
        bytes[54 + 3] = 0xFC;
        bytes[54 + 5..54 + 5 + 13].copy_from_slice(b"DELL U2723QE\0");
        // Serial text descriptor at 72.
        bytes[72 + 3] = 0xFF;
        bytes[72 + 5..72 + 5 + 10].copy_from_slice(b"ABC123456\0");
        bytes
    }

    #[test]
    fn parses_dell_edid() {
        let info = EdidInfo::parse(&base_edid()).unwrap();
        assert_eq!(info.manufacturer, "DEL");
        assert_eq!(info.product_code, 0xA040);
        assert_eq!(info.serial_number, 123_456);
        assert_eq!(info.monitor_name.as_deref(), Some("DELL U2723QE"));
        assert_eq!(info.serial_text.as_deref(), Some("ABC123456"));
        assert_eq!(info.manufacture_year, 2026);
        assert_eq!(info.width_cm, 60);
        assert_eq!(info.sha256.len(), 32);
    }

    #[test]
    fn rejects_invalid_header() {
        assert!(EdidInfo::parse(&[0u8; 128]).is_none());
        assert!(EdidInfo::parse(&[0u8; 64]).is_none());
    }

    #[test]
    fn connector_unit_is_stable() {
        assert_eq!(connector_unit("eDP-1"), connector_unit("eDP-1"));
        assert_ne!(connector_unit("eDP-1"), connector_unit("DP-1"));
    }

    #[test]
    fn sysfs_path_matches_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("card0-DP-1")).unwrap();
        std::fs::write(root.join("card0-DP-1/edid"), base_edid()).unwrap();
        std::fs::create_dir(root.join("card0-DP-10")).unwrap();
        std::fs::write(root.join("card0-DP-10/edid"), base_edid()).unwrap();
        let path = sysfs_edid_path(root, "DP-1").unwrap();
        assert!(path.to_string_lossy().contains("card0-DP-1/edid"));
        assert!(!path.to_string_lossy().contains("DP-10"));
    }
}
