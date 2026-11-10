use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use ember_core::{EmberError, EmberResult, Paths};

use crate::command::CommandRunner;

pub const AUTOSTART_NAME: &str = "project-ember.desktop";

/// XDG autostart integration. The file is written atomically with 0644 and the
/// directory is created 0755; no shell or systemd calls are required.
pub struct Autostart {
    path: PathBuf,
    exec: String,
}

impl Autostart {
    pub fn new(path: impl Into<PathBuf>, exec: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            exec: exec.into(),
        }
    }

    pub fn default_path() -> PathBuf {
        Paths::from_env().autostart_file()
    }

    pub fn default_for_app() -> Self {
        let exec = std::env::current_exe()
            .ok()
            .map(|path| {
                let path = path.display().to_string();
                if path.contains(' ') {
                    format!("\"{path}\" --autostart")
                } else {
                    format!("{path} --autostart")
                }
            })
            .unwrap_or_else(|| "emberd --autostart".into());
        Self::new(Self::default_path(), exec)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_enabled(&self) -> bool {
        fs::symlink_metadata(&self.path)
            .map(|metadata| metadata.is_file())
            .unwrap_or(false)
    }

    pub fn set_enabled(&self, enabled: bool) -> EmberResult<()> {
        if enabled {
            self.write()
        } else {
            ember_core::fsutil::remove_file(&self.path)
                .map_err(|error| EmberError::Io(format!("autostart remove failed: {error}")))
        }
    }

    fn write(&self) -> EmberResult<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| EmberError::Io("autostart path has no parent".into()))?;
        if let Ok(metadata) = fs::symlink_metadata(parent) {
            if metadata.file_type().is_symlink() {
                return Err(EmberError::Io(
                    "refusing symlinked autostart directory".into(),
                ));
            }
        }
        let parent_existed = parent.exists();
        fs::create_dir_all(parent)
            .map_err(|error| EmberError::Io(format!("autostart directory: {error}")))?;
        if !parent_existed {
            // Never loosen an existing directory's permissions (for example a
            // hardened 0700 ~/.config). Only the directory we create is 0755.
            let mut permissions = fs::metadata(parent)
                .map_err(|error| EmberError::Io(error.to_string()))?
                .permissions();
            permissions.set_mode(0o755);
            let _ = fs::set_permissions(parent, permissions);
        }

        // Replace a planted symlink rather than following it.
        if let Ok(metadata) = fs::symlink_metadata(&self.path) {
            if metadata.file_type().is_symlink() {
                fs::remove_file(&self.path)
                    .map_err(|error| EmberError::Io(format!("autostart symlink: {error}")))?;
            }
        }

        let content = desktop_entry(&self.exec);
        let temp = parent.join(format!(".{AUTOSTART_NAME}.tmp.{}", std::process::id()));
        let result = (|| -> std::io::Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o644)
                .open(&temp)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, &self.path)?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temp);
            return Err(EmberError::Io(format!("autostart write failed: {error}")));
        }
        Ok(())
    }
}

pub fn desktop_entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=Project Ember\n\
         GenericName=Display warmth and dimming\n\
         Comment=Reversible warmth and software dimming for every display\n\
         Exec={exec}\n\
         Icon=project-ember\n\
         Terminal=false\n\
         Categories=Utility;\n\
         StartupNotify=false\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

/// True when the packaged systemd user unit is enabled. The autostart toggle
/// reports the union so the UI never claims "off" while a unit still starts it.
pub fn systemd_unit_enabled(runner: &dyn CommandRunner) -> bool {
    if !runner.program_exists("systemctl") {
        return false;
    }
    runner
        .run(
            "systemctl",
            &["--user", "is-enabled", "project-ember.service"],
        )
        .map(|outcome| outcome.success && outcome.stdout.trim() == "enabled")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_and_disable_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let autostart = Autostart::new(
            dir.path().join("autostart/project-ember.desktop"),
            "emberd --autostart",
        );
        assert!(!autostart.is_enabled());
        autostart.set_enabled(true).unwrap();
        assert!(autostart.is_enabled());
        let content = fs::read_to_string(autostart.path()).unwrap();
        assert!(content.contains("Exec=emberd --autostart"));
        assert!(content.contains("X-GNOME-Autostart-enabled=true"));
        let mode = fs::metadata(autostart.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
        autostart.set_enabled(false).unwrap();
        assert!(!autostart.is_enabled());
    }

    #[test]
    fn existing_private_parent_is_not_loosened() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("private-autostart");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let autostart = Autostart::new(parent.join("project-ember.desktop"), "emberd");
        autostart.set_enabled(true).unwrap();
        let mode = fs::metadata(&parent).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "an existing private directory must stay private"
        );
    }

    #[test]
    fn symlinked_target_is_replaced_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.desktop");
        fs::write(&victim, "do not touch").unwrap();
        let path = dir.path().join("project-ember.desktop");
        std::os::unix::fs::symlink(&victim, &path).unwrap();
        let autostart = Autostart::new(&path, "emberd --autostart");
        autostart.set_enabled(true).unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "do not touch");
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("[Desktop Entry]"));
    }

    #[test]
    fn desktop_entry_is_valid_ini_shape() {
        let entry = desktop_entry("emberd --autostart");
        assert!(entry.starts_with("[Desktop Entry]"));
        assert!(entry.contains("Type=Application"));
        assert!(entry.contains("Exec=emberd --autostart"));
    }
}
