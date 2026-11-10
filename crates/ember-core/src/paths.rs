use std::env;
use std::path::{Path, PathBuf};

/// XDG-compliant locations used by the daemon, CLI and GUI.
///
/// Tests and the visual simulator can override every root with
/// `EMBER_CONFIG_DIR`, `EMBER_STATE_DIR` and `EMBER_RUNTIME_DIR`, which avoids
/// touching the developer's real state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    config_dir: PathBuf,
    state_dir: PathBuf,
    runtime_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> Self {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let config_dir = env::var_os("EMBER_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
            .unwrap_or_else(|| home.join(".config"))
            .join("project-ember");
        let state_root = env::var_os("EMBER_STATE_DIR")
            .map(PathBuf::from)
            .or_else(|| env::var_os("XDG_STATE_HOME").map(PathBuf::from))
            .unwrap_or_else(|| home.join(".local/state"));
        let state_dir = if env::var_os("EMBER_STATE_DIR").is_some() {
            state_root
        } else {
            state_root.join("project-ember")
        };
        let runtime_dir = env::var_os("EMBER_RUNTIME_DIR")
            .map(PathBuf::from)
            .or_else(|| env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
            .unwrap_or_else(|| {
                let uid = unsafe { libc::geteuid() };
                PathBuf::from(format!("/tmp/project-ember-{uid}"))
            })
            .join("project-ember");
        Self {
            config_dir,
            state_dir,
            runtime_dir,
        }
    }

    pub fn for_test(
        config: impl Into<PathBuf>,
        state: impl Into<PathBuf>,
        runtime: impl Into<PathBuf>,
    ) -> Self {
        Self {
            config_dir: config.into(),
            state_dir: state.into(),
            runtime_dir: runtime.into(),
        }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    pub fn journal_file(&self) -> PathBuf {
        self.state_dir.join("display-recovery-v1.json")
    }

    pub fn journal_backup_file(&self) -> PathBuf {
        self.state_dir.join("display-recovery-v1.last-good.json")
    }

    pub fn diagnostics_file(&self) -> PathBuf {
        self.state_dir.join("diagnostics.log")
    }

    pub fn diagnostics_salt_file(&self) -> PathBuf {
        self.state_dir.join("diagnostics.salt")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.runtime_dir.join("daemon.lock")
    }

    pub fn simulator_state_file(&self) -> PathBuf {
        self.state_dir.join("simulator-state.json")
    }

    /// XDG autostart entry. Lives outside the config subdirectory by spec.
    pub fn autostart_file(&self) -> PathBuf {
        self.config_dir
            .parent()
            .map(|parent| parent.join("autostart"))
            .unwrap_or_else(|| self.config_dir.clone())
            .join("project-ember.desktop")
    }

    /// Location of the desktop entry installed by the package (read-only).
    pub fn installed_desktop_file(&self) -> PathBuf {
        PathBuf::from("/usr/share/applications/project-ember.desktop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_live_in_expected_places() {
        let paths = Paths::for_test("/c", "/s", "/r");
        assert_eq!(paths.settings_file(), PathBuf::from("/c/settings.json"));
        assert_eq!(
            paths.journal_file(),
            PathBuf::from("/s/display-recovery-v1.json")
        );
        assert_eq!(
            paths.journal_backup_file(),
            PathBuf::from("/s/display-recovery-v1.last-good.json")
        );
        assert_eq!(
            paths.autostart_file(),
            PathBuf::from("/autostart/project-ember.desktop")
        );
        assert_eq!(paths.lock_file(), PathBuf::from("/r/daemon.lock"));
    }
}
