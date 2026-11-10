use std::collections::HashSet;
use std::sync::Arc;

use serde::Deserialize;

use crate::command::{CommandRunner, SystemCommandRunner};

/// Read-only Hyprland integration. Ember never changes another application's
/// nightlight state; it only reports that it is on so the user understands why
/// warmth may look compounded.
pub struct HyprlandProbe {
    runner: Arc<dyn CommandRunner>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NightlightState {
    pub active: bool,
    pub temperature: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawMonitor {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    make: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    serial: String,
    #[serde(default)]
    mirror_of: String,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyprlandMonitor {
    pub name: String,
    pub description: String,
    pub make: String,
    pub model: String,
    pub serial: String,
    pub mirrored_from: Option<String>,
    pub disabled: bool,
    pub width: u32,
    pub height: u32,
}

impl HyprlandMonitor {
    pub fn is_mirrored(&self) -> bool {
        self.mirrored_from.is_some()
    }
}

impl HyprlandProbe {
    pub fn new() -> Self {
        Self {
            runner: Arc::new(SystemCommandRunner),
        }
    }

    pub fn with_runner(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }

    /// Hyprland sets `HYPRLAND_INSTANCE_SIGNATURE` for every child process.
    pub fn session_present() -> bool {
        std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
    }

    pub fn available(&self) -> bool {
        Self::session_present() && self.runner.program_exists("hyprctl")
    }

    pub fn monitors(&self) -> Option<Vec<HyprlandMonitor>> {
        let outcome = self.runner.run("hyprctl", &["-j", "monitors"]).ok()?;
        if !outcome.success {
            return None;
        }
        let raw: Vec<RawMonitor> = serde_json::from_str(&outcome.stdout).ok()?;
        Some(
            raw.into_iter()
                .map(|monitor| HyprlandMonitor {
                    name: monitor.name,
                    description: monitor.description,
                    make: monitor.make,
                    model: monitor.model,
                    serial: monitor.serial,
                    mirrored_from: {
                        let value = monitor.mirror_of.trim().to_ascii_lowercase();
                        if value.is_empty() || value == "none" {
                            None
                        } else {
                            Some(monitor.mirror_of)
                        }
                    },
                    disabled: monitor.disabled,
                    width: monitor.width,
                    height: monitor.height,
                })
                .collect(),
        )
    }

    pub fn mirrored_connectors(&self) -> HashSet<String> {
        self.monitors()
            .unwrap_or_default()
            .into_iter()
            .filter(|monitor| monitor.is_mirrored())
            .map(|monitor| monitor.name)
            .collect()
    }

    pub fn nightlight(&self) -> Option<NightlightState> {
        let outcome = self
            .runner
            .run("hyprctl", &["hyprsunset", "temperature"])
            .ok()?;
        if !outcome.success {
            return None;
        }
        let temperature = outcome
            .stdout
            .split(|character: char| !character.is_ascii_digit())
            .find(|part| !part.is_empty())
            .and_then(|part| part.parse::<u32>().ok());
        Some(NightlightState {
            active: temperature.is_some_and(|value| value < 6000),
            temperature,
        })
    }

    pub fn version(&self) -> Option<String> {
        let outcome = self.runner.run("hyprctl", &["version", "-j"]).ok()?;
        if !outcome.success {
            return None;
        }
        let value: serde_json::Value = serde_json::from_str(&outcome.stdout).ok()?;
        value
            .get("tag")
            .and_then(|tag| tag.as_str())
            .map(str::to_string)
            .or_else(|| {
                value
                    .get("commit")
                    .and_then(|commit| commit.as_str())
                    .map(str::to_string)
            })
    }
}

impl Default for HyprlandProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandOutcome, RecordingRunner};

    fn outcome(stdout: &str) -> CommandOutcome {
        CommandOutcome {
            success: true,
            status: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn probe_with(stdout: &str) -> HyprlandProbe {
        let runner = RecordingRunner::new().with_response("hyprctl", outcome(stdout));
        HyprlandProbe::with_runner(Arc::new(runner))
    }

    #[test]
    fn monitors_parse_and_mirroring_is_detected() {
        let probe = probe_with(
            r#"[
              {"name":"eDP-1","description":"BOE 0x095F","make":"BOE","model":"0x095F","serial":"","mirrorOf":"none","disabled":false,"width":3024,"height":1964},
              {"name":"DP-1","description":"Dell","make":"Dell","model":"U2723QE","serial":"ABC","mirrorOf":"eDP-1","disabled":false,"width":3840,"height":2160}
            ]"#,
        );
        let monitors = probe.monitors().unwrap();
        assert_eq!(monitors.len(), 2);
        assert!(!monitors[0].is_mirrored());
        assert_eq!(monitors[1].mirrored_from.as_deref(), Some("eDP-1"));
        assert_eq!(monitors[1].width, 3840);
    }

    #[test]
    fn nightlight_parses_temperature() {
        let probe = probe_with("4000");
        let state = probe.nightlight().unwrap();
        assert!(state.active);
        assert_eq!(state.temperature, Some(4000));

        let probe = probe_with("temperature: 6500");
        let state = probe.nightlight().unwrap();
        assert!(!state.active);
        assert_eq!(state.temperature, Some(6500));
    }

    #[test]
    fn malformed_monitor_json_is_ignored() {
        let probe = probe_with("not json");
        assert!(probe.monitors().is_none());
        assert!(probe.mirrored_connectors().is_empty());
    }
}
