use std::path::{Path, PathBuf};

use crate::app_version::AppVersion;
use crate::error::{EmberError, EmberResult};
use crate::fsutil;
use crate::models::RecoveryRecord;

/// Hard cap for recovery journals. A legitimate journal is a few hundred KB at
/// most (a handful of displays). Anything larger is treated as corrupt rather
/// than parsed into memory.
pub const MAX_JOURNAL_BYTES: u64 = 1024 * 1024;

/// Explicit load outcome. A corrupt journal is never reported as absent.
#[derive(Debug, Clone, PartialEq)]
pub enum JournalLoadOutcome {
    NoJournal,
    Loaded(RecoveryRecord),
    UnsupportedFutureSchema(u32),
    Corrupt(String),
    IoFailure(String),
}

impl JournalLoadOutcome {
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::NoJournal)
    }

    pub fn record(&self) -> Option<&RecoveryRecord> {
        match self {
            Self::Loaded(record) => Some(record),
            _ => None,
        }
    }
}

/// Schema-v2 recovery journal with atomic writes, a last-known-good backup and
/// quarantine-instead-of-delete semantics.
pub struct RecoveryJournal {
    path: PathBuf,
}

impl RecoveryJournal {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        fsutil::file_exists(&self.path)
    }

    pub fn backup_path(&self) -> PathBuf {
        let stem = self
            .path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("display-recovery-v1");
        self.path.with_file_name(format!("{stem}.last-good.json"))
    }

    pub fn load_outcome(&self) -> JournalLoadOutcome {
        let bytes = match fsutil::read_limited(&self.path, MAX_JOURNAL_BYTES) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return JournalLoadOutcome::NoJournal
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::InvalidInput
                    || error.kind() == std::io::ErrorKind::InvalidData =>
            {
                return JournalLoadOutcome::Corrupt(error.to_string())
            }
            Err(error) => return JournalLoadOutcome::IoFailure(error.to_string()),
        };
        if bytes.is_empty() {
            return JournalLoadOutcome::Corrupt("Recovery journal is empty".into());
        }

        // Inspect the schema before decoding so a future schema is reported
        // explicitly instead of looking like corruption.
        let value: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(error) => return JournalLoadOutcome::Corrupt(error.to_string()),
        };
        if let Some(version) = value.get("schemaVersion").and_then(|v| v.as_u64()) {
            if version > u64::from(AppVersion::MAX_SUPPORTED_SCHEMA) {
                return JournalLoadOutcome::UnsupportedFutureSchema(version as u32);
            }
        }

        match serde_json::from_slice::<RecoveryRecord>(&bytes) {
            Ok(record) => JournalLoadOutcome::Loaded(record),
            Err(error) => {
                let text = error.to_string();
                if text.contains("Unsupported recovery schema") {
                    let version = value
                        .get("schemaVersion")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32)
                        .unwrap_or(0);
                    JournalLoadOutcome::UnsupportedFutureSchema(version)
                } else {
                    JournalLoadOutcome::Corrupt(text)
                }
            }
        }
    }

    pub fn load(&self) -> EmberResult<Option<RecoveryRecord>> {
        match self.load_outcome() {
            JournalLoadOutcome::NoJournal => Ok(None),
            JournalLoadOutcome::Loaded(record) => Ok(Some(record)),
            JournalLoadOutcome::UnsupportedFutureSchema(version) => {
                Err(EmberError::JournalUnsupportedSchema(version))
            }
            JournalLoadOutcome::Corrupt(reason) => Err(EmberError::JournalCorrupt(reason)),
            JournalLoadOutcome::IoFailure(message) => Err(EmberError::RecoveryFailed(message)),
        }
    }

    pub fn save(&self, record: &RecoveryRecord) -> EmberResult<()> {
        let bytes = serde_json::to_vec(record).map_err(|error| {
            EmberError::RecoveryFailed(format!("journal encode failed: {error}"))
        })?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(EmberError::RecoveryFailed(
                "recovery journal would exceed the size limit".into(),
            ));
        }

        // Preserve the last valid journal before overwriting it.
        if let Ok(Some(current)) = self.load() {
            if let Ok(current_bytes) = serde_json::to_vec(&current) {
                let _ = fsutil::atomic_write_private(&self.backup_path(), &current_bytes);
            }
        }

        fsutil::atomic_write_private(&self.path, &bytes)
            .map_err(|error| EmberError::RecoveryFailed(format!("journal write failed: {error}")))
    }

    /// Moves an unreadable journal aside. Returns the preserved path.
    pub fn quarantine_corrupt_journal(&self) -> Option<PathBuf> {
        if !self.exists() {
            return None;
        }
        fsutil::quarantine(&self.path, "corrupt").ok()
    }

    pub fn clear(&self) -> EmberResult<()> {
        fsutil::remove_file(&self.path)
            .map_err(|error| EmberError::RecoveryFailed(format!("journal clear failed: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DisplayRecoveryEntry, EmberSettings};
    use std::io::Write;

    fn sample_record() -> RecoveryRecord {
        RecoveryRecord::new("test", vec![], EmberSettings::default())
    }

    #[test]
    fn absent_journal_is_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RecoveryJournal::new(dir.path().join("j.json"));
        assert_eq!(journal.load_outcome(), JournalLoadOutcome::NoJournal);
        assert!(!journal.exists());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RecoveryJournal::new(dir.path().join("j.json"));
        let record = sample_record();
        journal.save(&record).unwrap();
        assert!(journal.exists());
        match journal.load_outcome() {
            JournalLoadOutcome::Loaded(loaded) => assert_eq!(loaded, record),
            other => panic!("expected loaded, got {other:?}"),
        }
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(journal.path()).unwrap().permissions(),
        ) & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn corrupt_journal_is_not_absent_and_is_quarantined() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("j.json");
        std::fs::write(&path, b"not-json{{{").unwrap();
        let journal = RecoveryJournal::new(&path);
        assert!(matches!(
            journal.load_outcome(),
            JournalLoadOutcome::Corrupt(_)
        ));
        let preserved = journal.quarantine_corrupt_journal().expect("quarantine");
        assert!(!path.exists());
        assert!(preserved.exists());
        assert_eq!(std::fs::read(preserved).unwrap(), b"not-json{{{");
    }

    #[test]
    fn empty_journal_is_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("j.json");
        std::fs::File::create(&path).unwrap();
        let journal = RecoveryJournal::new(&path);
        assert!(matches!(
            journal.load_outcome(),
            JournalLoadOutcome::Corrupt(_)
        ));
    }

    #[test]
    fn future_schema_is_reported_explicitly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("j.json");
        let json = br#"{"schemaVersion":99,"appVersion":"x","createdAt":1.0,"displays":[],"intendedSettings":{}}"#;
        std::fs::write(&path, json).unwrap();
        let journal = RecoveryJournal::new(&path);
        assert_eq!(
            journal.load_outcome(),
            JournalLoadOutcome::UnsupportedFutureSchema(99)
        );
    }

    #[test]
    fn oversize_journal_is_rejected_without_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("j.json");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&vec![b'x'; (MAX_JOURNAL_BYTES + 10) as usize])
            .unwrap();
        drop(file);
        let journal = RecoveryJournal::new(&path);
        assert!(matches!(
            journal.load_outcome(),
            JournalLoadOutcome::Corrupt(_)
        ));
    }

    #[test]
    fn backup_is_written_before_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RecoveryJournal::new(dir.path().join("j.json"));
        let first = RecoveryRecord::new("first", vec![], EmberSettings::default());
        journal.save(&first).unwrap();
        let second = RecoveryRecord::new("second", vec![], EmberSettings::default());
        journal.save(&second).unwrap();
        let backup = journal.backup_path();
        assert!(backup.exists());
        let text = std::fs::read_to_string(backup).unwrap();
        assert!(text.contains("\"first\""));
    }

    #[test]
    fn schema_v1_backup_from_macos_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("j.json");
        let json = br#"{
          "schemaVersion":1,
          "appVersion":"0.3.0",
          "createdAt":1700000000,
          "display":{"displayID":7,"gammaTable":{"red":[0,1],"green":[0,1],"blue":[0,1]},"capturedAt":1700000000},
          "hardware":{"brightness":0.7,"ambientLightCompensationEnabled":false},
          "intendedSettings":{"warmth":0.5,"apparentBrightness":0.8,"filterEnabled":true,"backlightLockEnabled":true,"launchAtLogin":true,"sunScheduleEnabled":false}
        }"#;
        std::fs::write(&path, json).unwrap();
        let journal = RecoveryJournal::new(&path);
        match journal.load_outcome() {
            JournalLoadOutcome::Loaded(record) => {
                assert_eq!(record.schema_version, 1);
                assert_eq!(record.displays.len(), 1);
                assert_eq!(record.displays[0].identity.legacy_display_id, Some(7));
                assert_eq!(record.displays[0].hardware.brightness, Some(0.7));
            }
            other => panic!("expected loaded v1, got {other:?}"),
        }
    }

    #[test]
    fn symlinked_journal_is_corrupt_and_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.json");
        std::fs::write(&victim, b"{\"keep\":true}").unwrap();
        let link = dir.path().join("j.json");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        let journal = RecoveryJournal::new(&link);
        assert!(matches!(
            journal.load_outcome(),
            JournalLoadOutcome::Corrupt(_)
        ));
        let preserved = journal.quarantine_corrupt_journal().unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"{\"keep\":true}");
        assert!(std::fs::symlink_metadata(preserved)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn duplicate_entries_survive_round_trip_for_ambiguity_handling() {
        use crate::gamma::GammaTable;
        use crate::identity::DisplayIdentity;
        use crate::models::DisplayBaseline;
        use chrono::Utc;
        let identity =
            DisplayIdentity::new(Some("dup".into()), 1, 2, 0, 0, false, None, "DP-1".into());
        let baseline = DisplayBaseline::new(
            1,
            Some(identity.clone()),
            GammaTable::identity(8),
            Utc::now(),
        );
        let entry = DisplayRecoveryEntry::without_hardware(identity.clone(), baseline.clone());
        let record =
            RecoveryRecord::new("test", vec![entry.clone(), entry], EmberSettings::default());
        let dir = tempfile::tempdir().unwrap();
        let journal = RecoveryJournal::new(dir.path().join("j.json"));
        journal.save(&record).unwrap();
        match journal.load_outcome() {
            JournalLoadOutcome::Loaded(loaded) => assert_eq!(loaded.displays.len(), 2),
            other => panic!("expected loaded, got {other:?}"),
        }
    }
}
