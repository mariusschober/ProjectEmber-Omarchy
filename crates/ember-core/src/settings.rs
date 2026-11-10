use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::error::{EmberError, EmberResult};
use crate::fsutil;
use crate::models::EmberSettings;

const MAX_SETTINGS_BYTES: u64 = 256 * 1024;

/// Result of loading settings. Corrupt settings never prevent startup; they are
/// quarantined and defaults are used, with the failure surfaced to diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsLoad {
    pub settings: EmberSettings,
    pub recovered: bool,
    pub quarantined: Option<PathBuf>,
    pub error: Option<String>,
}

pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self, now: chrono::DateTime<Utc>) -> SettingsLoad {
        let mut settings = match fsutil::read_limited(&self.path, MAX_SETTINGS_BYTES) {
            Ok(bytes) => match serde_json::from_slice::<EmberSettings>(&bytes) {
                Ok(mut settings) => {
                    settings.normalize(now);
                    return SettingsLoad {
                        settings,
                        recovered: false,
                        quarantined: None,
                        error: None,
                    };
                }
                Err(error) => {
                    let quarantined = fsutil::quarantine(&self.path, "corrupt").ok();
                    let mut settings = EmberSettings::default();
                    settings.normalize(now);
                    return SettingsLoad {
                        settings,
                        recovered: true,
                        quarantined,
                        error: Some(format!("settings could not be read: {error}")),
                    };
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => EmberSettings::default(),
            Err(error) => {
                let quarantined = fsutil::quarantine(&self.path, "unreadable").ok();
                let mut settings = EmberSettings::default();
                settings.normalize(now);
                return SettingsLoad {
                    settings,
                    recovered: true,
                    quarantined,
                    error: Some(format!("settings could not be opened: {error}")),
                };
            }
        };
        settings.normalize(now);
        SettingsLoad {
            settings,
            recovered: false,
            quarantined: None,
            error: None,
        }
    }

    pub fn save(&self, settings: &EmberSettings) -> EmberResult<()> {
        let mut normalized = settings.clone();
        normalized.normalize(Utc::now());
        let bytes = serde_json::to_vec_pretty(&normalized)
            .map_err(|error| EmberError::Io(format!("settings encode failed: {error}")))?;
        fsutil::atomic_write_private(&self.path, &bytes)
            .map_err(|error| EmberError::Io(format!("settings save failed: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn temp_store() -> (tempfile::TempDir, SettingsStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(dir.path().join("config").join("settings.json"));
        (dir, store)
    }

    #[test]
    fn missing_file_yields_defaults() {
        let (_dir, store) = temp_store();
        let load = store.load(Utc::now());
        assert!(!load.recovered);
        assert_eq!(load.settings, EmberSettings::default());
    }

    #[test]
    fn round_trip_preserves_values() {
        let (_dir, store) = temp_store();
        let settings = EmberSettings {
            warmth: 0.31,
            apparent_brightness: 0.55,
            filter_enabled: true,
            tray_primary_action: crate::models::TrayPrimaryAction::ToggleEmber,
            ..Default::default()
        };
        store.save(&settings).unwrap();
        let load = store.load(Utc::now());
        assert_eq!(load.settings, settings);
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn corrupt_file_is_quarantined_and_defaults_used() {
        let (_dir, store) = temp_store();
        std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        std::fs::write(store.path(), b"{not json").unwrap();
        let load = store.load(Utc::now());
        assert!(load.recovered);
        assert!(load.quarantined.is_some());
        assert!(!store.path().exists());
        assert_eq!(load.settings, EmberSettings::default());
    }

    #[test]
    fn expired_override_is_normalized_on_load() {
        let (_dir, store) = temp_store();
        let settings = EmberSettings {
            automation_override: Some(crate::models::AutomationOverride::new(
                true,
                Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
            )),
            ..Default::default()
        };
        store.save(&settings).unwrap();
        let load = store.load(Utc::now() + Duration::seconds(1));
        assert!(load.settings.automation_override.is_none());
    }

    #[test]
    fn symlink_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.json");
        std::fs::write(&victim, b"{\"warmth\":0.1}").unwrap();
        let link = dir.path().join("settings.json");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        let store = SettingsStore::new(&link);
        let load = store.load(Utc::now());
        assert!(load.recovered);
        // The victim is untouched; the symlink itself is quarantined.
        assert_eq!(std::fs::read(&victim).unwrap(), b"{\"warmth\":0.1}");
        assert!(!link.exists());
    }
}
