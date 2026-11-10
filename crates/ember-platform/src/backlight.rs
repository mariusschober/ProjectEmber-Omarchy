use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ember_core::{EmberError, EmberResult, HardwareBaseline};
use log::{debug, info, warn};

use crate::command::{CommandRunner, SystemCommandRunner};

pub const DEFAULT_BACKLIGHT_ROOT: &str = "/sys/class/backlight";
const PREFERRED_DEVICES: [&str; 6] = [
    "intel_backlight",
    "apple_backlight",
    "amdgpu_bl0",
    "amdgpu_bl1",
    "acpi_video0",
    "acpi_video1",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacklightDevice {
    pub name: String,
    pub max_brightness: u32,
    pub brightness: u32,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacklightCapability {
    pub available: bool,
    pub device: Option<String>,
    pub ambient_control: bool,
    pub sensor_present: bool,
    pub detail: Option<String>,
}

impl BacklightCapability {
    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            available: false,
            device: None,
            ambient_control: false,
            sensor_present: false,
            detail: Some(detail.into()),
        }
    }
}

/// Backlight control contract shared by the real controller and the simulator.
pub trait BacklightControl: Send {
    fn capability(&self, connector: &str) -> BacklightCapability;
    fn capture(&self, connector: &str) -> EmberResult<HardwareBaseline>;
    fn engage(&self, connector: &str) -> EmberResult<()>;
    fn needs_engagement(&self, connector: &str) -> bool;
    fn restore(&self, connector: &str, baseline: &HardwareBaseline) -> EmberResult<()>;
}

pub struct BacklightController {
    root: PathBuf,
    iio_root: PathBuf,
    runner: Arc<dyn CommandRunner>,
    allow_logind: bool,
    allow_brightnessctl: bool,
}

impl BacklightController {
    pub fn new() -> Self {
        Self {
            root: PathBuf::from(DEFAULT_BACKLIGHT_ROOT),
            iio_root: PathBuf::from("/sys/bus/iio/devices"),
            runner: Arc::new(SystemCommandRunner),
            allow_logind: true,
            allow_brightnessctl: true,
        }
    }

    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            iio_root: PathBuf::from("/sys/bus/iio/devices"),
            runner: Arc::new(SystemCommandRunner),
            allow_logind: true,
            allow_brightnessctl: true,
        }
    }

    /// Test/simulator constructor: no D-Bus or external commands.
    pub fn with_options(
        root: impl Into<PathBuf>,
        iio_root: impl Into<PathBuf>,
        runner: Arc<dyn CommandRunner>,
        allow_logind: bool,
        allow_brightnessctl: bool,
    ) -> Self {
        Self {
            root: root.into(),
            iio_root: iio_root.into(),
            runner,
            allow_logind,
            allow_brightnessctl,
        }
    }

    fn is_real_sysfs(&self) -> bool {
        self.root == Path::new(DEFAULT_BACKLIGHT_ROOT)
    }

    pub fn devices(&self) -> Vec<BacklightDevice> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !valid_device_name(&name) {
                continue;
            }
            let path = entry.path();
            let Some(max) = read_u32(&path.join("max_brightness")) else {
                continue;
            };
            let Some(brightness) = read_u32(&path.join("brightness")) else {
                continue;
            };
            let kind = fs::read_to_string(path.join("type"))
                .map(|value| value.trim().to_string())
                .unwrap_or_default();
            out.push(BacklightDevice {
                name,
                max_brightness: max,
                brightness,
                kind,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    fn select_device(&self, connector: &str) -> EmberResult<BacklightDevice> {
        if !ember_core::DisplayIdentity::is_built_in_connector(connector) {
            return Err(EmberError::BacklightUnavailable);
        }
        let devices = self.devices();
        if devices.is_empty() {
            return Err(EmberError::BacklightUnavailable);
        }
        if devices.len() == 1 {
            return Ok(devices.into_iter().next().unwrap());
        }
        let preferred: Vec<&BacklightDevice> = PREFERRED_DEVICES
            .iter()
            .filter_map(|name| devices.iter().find(|device| device.name == *name))
            .collect();
        match preferred.first() {
            Some(device) => Ok((*device).clone()),
            None => Err(EmberError::BacklightUnavailable),
        }
    }

    fn read_raw(&self, device: &str) -> EmberResult<(u32, u32)> {
        let path = self.root.join(device);
        let max =
            read_u32(&path.join("max_brightness")).ok_or(EmberError::BacklightReadFailed(-1))?;
        let current =
            read_u32(&path.join("brightness")).ok_or(EmberError::BacklightReadFailed(-2))?;
        if max == 0 {
            return Err(EmberError::BacklightReadFailed(-3));
        }
        Ok((current, max))
    }

    fn write_sysfs(&self, device: &str, value: u32) -> EmberResult<()> {
        let path = self.root.join(device).join("brightness");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|_| EmberError::BacklightWriteFailed(-1))?;
        file.write_all(value.to_string().as_bytes())
            .map_err(|_| EmberError::BacklightWriteFailed(-2))
    }

    fn write_via_logind(&self, device: &str, value: u32) -> EmberResult<()> {
        let connection =
            zbus::blocking::Connection::system().map_err(|_| EmberError::BacklightUnavailable)?;
        let proxy = zbus::blocking::Proxy::new(
            &connection,
            "org.freedesktop.login1",
            "/org/freedesktop/login1/session/auto",
            "org.freedesktop.login1.Session",
        )
        .map_err(|_| EmberError::BacklightUnavailable)?;
        proxy
            .call::<_, _, ()>("SetBrightness", &("backlight", device, value))
            .map_err(|error| {
                debug!("logind SetBrightness failed: {error}");
                EmberError::BacklightWriteFailed(-1)
            })
    }

    fn write_via_brightnessctl(&self, device: &str, value: u32) -> EmberResult<()> {
        if !self.runner.program_exists("brightnessctl") {
            return Err(EmberError::BacklightUnavailable);
        }
        let value = value.to_string();
        let outcome = self
            .runner
            .run("brightnessctl", &["-d", device, "set", &value])
            .map_err(|_| EmberError::BacklightWriteFailed(-1))?;
        if outcome.success {
            Ok(())
        } else {
            warn!("brightnessctl failed: {}", outcome.stderr);
            Err(EmberError::BacklightWriteFailed(outcome.status))
        }
    }

    fn write_raw(&self, device: &str, value: u32) -> EmberResult<()> {
        if !valid_device_name(device) {
            return Err(EmberError::BacklightUnavailable);
        }
        if !self.is_real_sysfs() {
            // Test / simulator sysfs tree: write directly.
            return self.write_sysfs(device, value);
        }
        if self.allow_logind {
            match self.write_via_logind(device, value) {
                Ok(()) => return Ok(()),
                Err(error) => debug!("logind brightness unavailable: {error}"),
            }
        }
        if self.allow_brightnessctl {
            match self.write_via_brightnessctl(device, value) {
                Ok(()) => return Ok(()),
                Err(error) => debug!("brightnessctl unavailable: {error}"),
            }
        }
        self.write_sysfs(device, value)
    }

    fn verify_fraction(&self, device: &str, minimum: f32) -> EmberResult<()> {
        let (current, max) = self.read_raw(device)?;
        let fraction = current as f32 / max as f32;
        if fraction + 0.01 < minimum {
            return Err(EmberError::BacklightWriteFailed(-4));
        }
        Ok(())
    }

    fn sensor_present(&self) -> bool {
        let Ok(entries) = fs::read_dir(&self.iio_root) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let entry_name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if entry_name.contains("als") || entry_name.contains("light") {
                return true;
            }
            let name = fs::read_to_string(entry.path().join("name"))
                .unwrap_or_default()
                .to_ascii_lowercase();
            name.contains("als") || name.contains("light")
        })
    }
}

impl Default for BacklightController {
    fn default() -> Self {
        Self::new()
    }
}

impl BacklightControl for BacklightController {
    fn capability(&self, connector: &str) -> BacklightCapability {
        if !ember_core::DisplayIdentity::is_built_in_connector(connector) {
            return BacklightCapability::unavailable(
                "Backlight Lock applies to the built-in display only.",
            );
        }
        match self.select_device(connector) {
            Ok(device) => BacklightCapability {
                available: true,
                device: Some(device.name),
                ambient_control: false,
                sensor_present: self.sensor_present(),
                detail: None,
            },
            Err(_) => BacklightCapability::unavailable(
                "No unambiguous built-in backlight device was found.",
            ),
        }
    }

    fn capture(&self, connector: &str) -> EmberResult<HardwareBaseline> {
        let device = self.select_device(connector)?;
        let (current, max) = self.read_raw(&device.name)?;
        Ok(HardwareBaseline {
            brightness: Some((current as f32 / max as f32).clamp(0.0, 1.0)),
            ambient_light_compensation_enabled: None,
            device: Some(device.name),
        })
    }

    fn engage(&self, connector: &str) -> EmberResult<()> {
        let device = self.select_device(connector)?;
        let (current, max) = self.read_raw(&device.name)?;
        if current >= max {
            self.verify_fraction(&device.name, 0.97)?;
            return Ok(());
        }
        self.write_raw(&device.name, max)?;
        self.verify_fraction(&device.name, 0.97)?;
        info!(
            "backlight: locked {} at full hardware brightness",
            device.name
        );
        Ok(())
    }

    fn needs_engagement(&self, connector: &str) -> bool {
        match self.select_device(connector) {
            Ok(device) => match self.read_raw(&device.name) {
                Ok((current, max)) if max > 0 => ember_core::BacklightDriftCheck::needs_write(
                    Some(current as f32 / max as f32),
                    None,
                    0.97,
                ),
                _ => false,
            },
            Err(_) => false,
        }
    }

    fn restore(&self, connector: &str, baseline: &HardwareBaseline) -> EmberResult<()> {
        let Some(fraction) = baseline.brightness else {
            return Ok(());
        };
        let device = match baseline.device.clone() {
            Some(device) => device,
            None => self.select_device(connector)?.name,
        };
        if !valid_device_name(&device) {
            return Err(EmberError::BacklightUnavailable);
        }
        let (_, max) = self.read_raw(&device)?;
        let target = ((fraction.clamp(0.0, 1.0)) * max as f32).round() as u32;
        self.write_raw(&device, target.min(max))?;
        let (current, max) = self.read_raw(&device)?;
        let expected = target as f32 / max as f32;
        let actual = current as f32 / max as f32;
        if (actual - expected).abs() > 0.02 && current != target {
            return Err(EmberError::BacklightWriteFailed(-5));
        }
        Ok(())
    }
}

fn valid_device_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn read_u32(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::RecordingRunner;
    use std::sync::Arc;

    fn fixture(brightness: u32, max: u32) -> (tempfile::TempDir, BacklightController) {
        let dir = tempfile::tempdir().unwrap();
        let device = dir.path().join("intel_backlight");
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("max_brightness"), max.to_string()).unwrap();
        fs::write(device.join("brightness"), brightness.to_string()).unwrap();
        fs::write(device.join("type"), "firmware\n").unwrap();
        let iio = dir.path().join("iio");
        fs::create_dir_all(&iio).unwrap();
        let controller = BacklightController::with_options(
            dir.path().join("backlight"),
            iio,
            Arc::new(RecordingRunner::new()),
            false,
            false,
        );
        // Move the fixture under backlight/.
        fs::create_dir_all(dir.path().join("backlight")).unwrap();
        fs::rename(device, dir.path().join("backlight/intel_backlight")).unwrap();
        (dir, controller)
    }

    #[test]
    fn lists_and_selects_single_device() {
        let (_dir, controller) = fixture(128, 255);
        let devices = controller.devices();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "intel_backlight");
        let capability = controller.capability("eDP-1");
        assert!(capability.available);
        assert_eq!(capability.device.as_deref(), Some("intel_backlight"));
        assert!(!capability.ambient_control);
    }

    #[test]
    fn external_connector_is_refused() {
        let (_dir, controller) = fixture(128, 255);
        let capability = controller.capability("DP-1");
        assert!(!capability.available);
        assert!(matches!(
            controller.capture("DP-1"),
            Err(EmberError::BacklightUnavailable)
        ));
    }

    #[test]
    fn engage_sets_full_brightness_and_restore_returns_baseline() {
        let (_dir, controller) = fixture(128, 255);
        let baseline = controller.capture("eDP-1").unwrap();
        assert!((baseline.brightness.unwrap() - 128.0 / 255.0).abs() < 0.001);
        assert!(controller.needs_engagement("eDP-1"));
        controller.engage("eDP-1").unwrap();
        assert!(!controller.needs_engagement("eDP-1"));
        controller.restore("eDP-1", &baseline).unwrap();
        let after = controller.capture("eDP-1").unwrap();
        assert!((after.brightness.unwrap() - baseline.brightness.unwrap()).abs() < 0.01);
    }

    #[test]
    fn already_full_does_not_change() {
        let (_dir, controller) = fixture(255, 255);
        controller.engage("eDP-1").unwrap();
        assert!(!controller.needs_engagement("eDP-1"));
    }

    #[test]
    fn multiple_unknown_devices_are_ambiguous() {
        let (dir, controller) = fixture(128, 255);
        fs::rename(
            dir.path().join("backlight/intel_backlight"),
            dir.path().join("backlight/panel0-backlight"),
        )
        .unwrap();
        let second = dir.path().join("backlight/panel1-backlight");
        fs::create_dir_all(&second).unwrap();
        fs::write(second.join("max_brightness"), "100").unwrap();
        fs::write(second.join("brightness"), "50").unwrap();
        let capability = controller.capability("eDP-1");
        assert!(!capability.available);
        assert!(capability.detail.unwrap().contains("unambiguous"));
    }

    #[test]
    fn preferred_device_wins_when_multiple_exist() {
        let (dir, controller) = fixture(128, 255);
        let second = dir.path().join("backlight/acpi_video0");
        fs::create_dir_all(&second).unwrap();
        fs::write(second.join("max_brightness"), "100").unwrap();
        fs::write(second.join("brightness"), "20").unwrap();
        let capability = controller.capability("eDP-1");
        assert!(capability.available);
        assert_eq!(capability.device.as_deref(), Some("intel_backlight"));
    }

    #[test]
    fn device_name_validation_blocks_path_traversal() {
        assert!(valid_device_name("intel_backlight"));
        assert!(!valid_device_name("../etc/passwd"));
        assert!(!valid_device_name("a/b"));
        assert!(!valid_device_name(""));
    }

    #[test]
    fn sensor_detection_is_optional() {
        let (dir, controller) = fixture(128, 255);
        assert!(!controller.sensor_present());
        let sensor = dir.path().join("iio/iio:device0");
        fs::create_dir_all(&sensor).unwrap();
        fs::write(sensor.join("name"), "als").unwrap();
        assert!(controller.sensor_present());
        let capability = controller.capability("eDP-1");
        assert!(capability.sensor_present);
        assert!(!capability.ambient_control);
    }

    #[test]
    fn missing_sysfs_is_unavailable() {
        let controller = BacklightController::with_options(
            "/nonexistent/ember/backlight",
            "/nonexistent/iio",
            Arc::new(RecordingRunner::new()),
            false,
            false,
        );
        assert!(!controller.capability("eDP-1").available);
    }
}
