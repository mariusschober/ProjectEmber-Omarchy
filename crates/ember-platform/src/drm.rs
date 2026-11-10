use std::fs;
use std::path::{Path, PathBuf};

/// Read-only access to `/sys/class/drm` for EDID and connector state. The root
/// is injectable so tests and the simulator can use a temporary tree.
#[derive(Debug, Clone)]
pub struct DrmSysfs {
    root: PathBuf,
}

impl DrmSysfs {
    pub fn new() -> Self {
        Self {
            root: PathBuf::from("/sys/class/drm"),
        }
    }

    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn edid_bytes(&self, connector: &str) -> Option<Vec<u8>> {
        let path = crate::edid::sysfs_edid_path(&self.root, connector)?;
        let bytes = fs::read(path).ok()?;
        if bytes.len() < 128 {
            return None;
        }
        Some(bytes)
    }

    pub fn status(&self, connector: &str) -> Option<String> {
        let entries = fs::read_dir(&self.root).ok()?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == connector || name.ends_with(&format!("-{connector}")) {
                let path = entry.path().join("status");
                if let Ok(value) = fs::read_to_string(path) {
                    return Some(value.trim().to_string());
                }
            }
        }
        None
    }

    pub fn is_connected(&self, connector: &str) -> bool {
        matches!(self.status(connector).as_deref(), Some("connected"))
    }

    /// Lists connector names known to DRM. Used only for diagnostics and for
    /// matching a backlight device; Wayland/X11 remain the source of truth for
    /// online outputs.
    pub fn connectors(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&self.root) else {
            return out;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(index) = name.find('-') {
                let connector = &name[index + 1..];
                if !connector.is_empty()
                    && (connector.contains('-') || connector.chars().any(|c| c.is_ascii_digit()))
                    && !out.iter().any(|existing| existing == connector)
                {
                    out.push(connector.to_string());
                }
            }
        }
        out
    }

    pub fn exists(&self) -> bool {
        self.root.is_dir()
    }
}

impl Default for DrmSysfs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_status_and_connectors() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("card0-eDP-1")).unwrap();
        std::fs::write(root.join("card0-eDP-1/status"), "connected\n").unwrap();
        std::fs::create_dir(root.join("card0-DP-3")).unwrap();
        std::fs::write(root.join("card0-DP-3/status"), "disconnected\n").unwrap();
        let sysfs = DrmSysfs::with_root(root);
        assert!(sysfs.is_connected("eDP-1"));
        assert!(!sysfs.is_connected("DP-3"));
        assert_eq!(sysfs.status("DP-3").as_deref(), Some("disconnected"));
        let connectors = sysfs.connectors();
        assert!(connectors.contains(&"eDP-1".to_string()));
        assert!(connectors.contains(&"DP-3".to_string()));
    }

    #[test]
    fn missing_root_is_harmless() {
        let sysfs = DrmSysfs::with_root("/nonexistent/ember");
        assert!(sysfs.edid_bytes("eDP-1").is_none());
        assert!(sysfs.connectors().is_empty());
        assert!(!sysfs.exists());
    }
}
