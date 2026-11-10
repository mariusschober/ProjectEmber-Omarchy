use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::fsutil;
use crate::identity::DisplayIdentity;

/// Rolling diagnostics. Everything stays local; identifiers are hashed with a
/// per-install salt and control characters are stripped before anything is
/// written or exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticLevel {
    Info,
    Warn,
    Error,
}

impl DiagnosticLevel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEntry {
    pub at: String,
    pub level: DiagnosticLevel,
    pub message: String,
}

struct Inner {
    path: PathBuf,
    salt: String,
    entries: VecDeque<DiagnosticEntry>,
    max_entries: usize,
    max_bytes: u64,
}

#[derive(Clone)]
pub struct DiagnosticLog {
    inner: Arc<Mutex<Inner>>,
}

impl DiagnosticLog {
    const DEFAULT_MAX_ENTRIES: usize = 500;
    const DEFAULT_MAX_BYTES: u64 = 384 * 1024;

    pub fn new(path: impl Into<PathBuf>, salt_path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let salt = load_or_create_salt(&salt_path.into());
        Self {
            inner: Arc::new(Mutex::new(Inner {
                path,
                salt,
                entries: VecDeque::with_capacity(Self::DEFAULT_MAX_ENTRIES),
                max_entries: Self::DEFAULT_MAX_ENTRIES,
                max_bytes: Self::DEFAULT_MAX_BYTES,
            })),
        }
    }

    /// In-memory only, for tests and the visual simulator.
    pub fn in_memory() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                path: PathBuf::new(),
                salt: "test-salt".into(),
                entries: VecDeque::new(),
                max_entries: Self::DEFAULT_MAX_ENTRIES,
                max_bytes: Self::DEFAULT_MAX_BYTES,
            })),
        }
    }

    pub fn with_limits(self, max_entries: usize, max_bytes: u64) -> Self {
        if let Ok(mut inner) = self.inner.lock() {
            inner.max_entries = max_entries.max(1);
            inner.max_bytes = max_bytes.max(4096);
        }
        self
    }

    pub fn salt(&self) -> String {
        self.inner
            .lock()
            .map(|inner| inner.salt.clone())
            .unwrap_or_default()
    }

    pub fn identity_tag(&self, identity: &DisplayIdentity) -> String {
        identity.diagnostic_tag(&self.salt())
    }

    pub fn append(&self, level: DiagnosticLevel, message: impl AsRef<str>) {
        let message = sanitize_line(message.as_ref());
        let entry = DiagnosticEntry {
            at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            level,
            message,
        };
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.entries.len() >= inner.max_entries {
            inner.entries.pop_front();
        }
        inner.entries.push_back(entry.clone());
        if inner.path.as_os_str().is_empty() {
            return;
        }
        let _ = fs::create_dir_all(inner.path.parent().unwrap_or_else(|| Path::new("/tmp")));
        if let Ok(metadata) = fs::metadata(&inner.path) {
            if metadata.len() > inner.max_bytes {
                let rotated = inner.path.with_extension("log.1");
                let _ = fs::remove_file(&rotated);
                let _ = fs::rename(&inner.path, &rotated);
            }
        }
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode_0600()
            .open(&inner.path)
        {
            let _ = writeln!(
                file,
                "{} [{}] {}",
                entry.at,
                entry.level.label(),
                entry.message
            );
        }
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.append(DiagnosticLevel::Info, message);
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        self.append(DiagnosticLevel::Warn, message);
    }

    pub fn error(&self, message: impl AsRef<str>) {
        self.append(DiagnosticLevel::Error, message);
    }

    pub fn entries(&self) -> Vec<DiagnosticEntry> {
        self.inner
            .lock()
            .map(|inner| inner.entries.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn export_text(&self) -> String {
        let entries = self.entries();
        let mut out = String::new();
        out.push_str(&format!(
            "Project Ember diagnostics · {} · {}\n",
            crate::app_version::AppVersion::full(),
            Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
        ));
        out.push_str(
            "This export contains no serial numbers, EDID bytes, screenshots or network data.\n\n",
        );
        for entry in entries {
            out.push_str(&format!(
                "{} [{}] {}\n",
                entry.at,
                entry.level.label(),
                entry.message
            ));
        }
        out
    }

    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.entries.clear();
            if !inner.path.as_os_str().is_empty() {
                let _ = fs::remove_file(&inner.path);
            }
        }
    }
}

trait OpenOptionsMode {
    fn mode_0600(&mut self) -> &mut Self;
}

impl OpenOptionsMode for fs::OpenOptions {
    fn mode_0600(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.mode(0o600)
    }
}

fn sanitize_line(input: &str) -> String {
    let mut out = String::with_capacity(input.len().min(800));
    for character in input.chars().take(800) {
        if character == '\n' || character == '\r' {
            out.push(' ');
        } else if character == '\t' || !character.is_control() {
            out.push(character);
        } else {
            out.push(' ');
        }
    }
    out
}

fn load_or_create_salt(path: &Path) -> String {
    if let Ok(bytes) = fsutil::read_limited(path, 256) {
        let text = String::from_utf8_lossy(&bytes).trim().to_string();
        if !text.is_empty() && text.len() <= 128 {
            return text;
        }
    }
    let mut raw = [0u8; 16];
    let filled = fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut raw))
        .is_ok();
    if !filled {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        raw[..8].copy_from_slice(&(now as u64).to_le_bytes());
        raw[8..].copy_from_slice(&(std::process::id() as u64).to_le_bytes());
    }
    let salt: String = raw.iter().map(|byte| format!("{byte:02x}")).collect();
    let _ = fsutil::atomic_write_private(path, salt.as_bytes());
    salt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_are_sanitized() {
        let log = DiagnosticLog::in_memory();
        log.info("bad\u{0}\u{7}line\twith\nnewline");
        let entries = log.entries();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].message.contains('\u{0}'));
        assert!(!entries[0].message.contains('\u{7}'));
        assert!(!entries[0].message.contains('\n'));
    }

    #[test]
    fn ring_buffer_is_bounded() {
        let log = DiagnosticLog::in_memory().with_limits(5, 4096);
        for index in 0..20 {
            log.info(format!("line {index}"));
        }
        let entries = log.entries();
        assert_eq!(entries.len(), 5);
        assert!(entries.last().unwrap().message.contains("19"));
    }

    #[test]
    fn export_contains_header_and_entries() {
        let log = DiagnosticLog::in_memory();
        log.warn("hello");
        let text = log.export_text();
        assert!(text.contains("Project Ember diagnostics"));
        assert!(text.contains("[WARN] hello"));
        assert!(text.contains("no serial numbers"));
    }

    #[test]
    fn identity_tags_never_contain_raw_serial() {
        let log = DiagnosticLog::in_memory();
        let identity = DisplayIdentity::new(
            Some("secret-uuid".into()),
            1,
            2,
            123456,
            0,
            false,
            None,
            "DP-1".into(),
        );
        let tag = log.identity_tag(&identity);
        assert_eq!(tag.len(), 12);
        assert!(!tag.contains("123456"));
        assert!(!tag.contains("secret"));
    }

    #[test]
    fn file_logging_works_and_salt_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("diagnostics.log");
        let salt = dir.path().join("state").join("diagnostics.salt");
        let log = DiagnosticLog::new(&path, &salt);
        log.info("persisted");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("persisted"));
        let first_salt = log.salt();
        let second = DiagnosticLog::new(&path, &salt);
        assert_eq!(first_salt, second.salt());
    }
}
