use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::app_version::AppVersion;
use crate::models::EmberSettings;
use crate::presentation::{
    AttentionState, BackendKind, DisplayCountSummary, EmberOperationState, VerificationLevel,
};
use crate::solar::SolarEvent;

/// Wire schema for daemon ↔ GUI/CLI snapshots. Bumped only for breaking shape
/// changes; fields are additive.
pub const IPC_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DisplayState {
    Active,
    Pending,
    Unsupported,
    Failed,
    Idle,
}

impl DisplayState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Pending => "Pending reconnection",
            Self::Unsupported => "Unsupported",
            Self::Failed => "Failed",
            Self::Idle => "Idle",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplaySnapshot {
    pub identity_key: String,
    pub label: String,
    pub connector: String,
    pub built_in: bool,
    pub mirrored: bool,
    pub gamma_compatible: bool,
    pub state: DisplayState,
    pub verification: VerificationLevel,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BacklightState {
    Disengaged,
    Engaged,
    Unavailable,
    Failed,
}

impl BacklightState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disengaged => "Off",
            Self::Engaged => "Locked",
            Self::Unavailable => "Unavailable",
            Self::Failed => "Stopped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BacklightSnapshot {
    pub available: bool,
    /// Linux has no portable firmware auto-brightness switch; this is always
    /// false and the UI says so.
    pub ambient_control: bool,
    pub state: BacklightState,
    pub detail: Option<String>,
    pub device: Option<String>,
}

impl Default for BacklightSnapshot {
    fn default() -> Self {
        Self {
            available: false,
            ambient_control: false,
            state: BacklightState::Disengaged,
            detail: None,
            device: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocationState {
    /// A location is available (fresh fix or cached).
    Granted,
    /// Sun schedule is enabled but no usable location has been provided yet.
    NeedsLocation,
    /// The desktop location service denied access.
    Denied,
    /// GeoClue is not installed/available and no manual location exists.
    Unavailable,
    /// Sun schedule is off.
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolarRuntimeSnapshot {
    pub enabled: bool,
    pub is_night: bool,
    pub next_event: Option<SolarEvent>,
    /// Unix seconds. Manual on/off overrides remain in force until this time.
    pub override_until: Option<i64>,
    pub location_state: LocationState,
    pub location_detail: Option<String>,
    pub refreshing: bool,
    pub timezone: String,
}

impl Default for SolarRuntimeSnapshot {
    fn default() -> Self {
        Self {
            enabled: false,
            is_night: false,
            next_event: None,
            override_until: None,
            location_state: LocationState::Disabled,
            location_detail: None,
            refreshing: false,
            timezone: "UTC".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocationSource {
    Manual,
    GeoClue,
    Cached,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationSnapshot {
    pub latitude: f64,
    pub longitude: f64,
    pub source: LocationSource,
    pub age_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NightlightSnapshot {
    pub active: bool,
    pub temperature: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendSnapshot {
    pub kind: BackendKind,
    pub name: String,
    pub verification: VerificationLevel,
    pub readback: bool,
    pub note: Option<String>,
}

impl Default for BackendSnapshot {
    fn default() -> Self {
        Self {
            kind: BackendKind::None,
            name: "none".into(),
            verification: VerificationLevel::Unavailable,
            readback: false,
            note: None,
        }
    }
}

/// Complete render source for every client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub schema_version: u32,
    pub version: String,
    pub edition: String,
    pub settings: EmberSettings,
    pub runtime_state: EmberOperationState,
    pub runtime_label: String,
    pub status_title: String,
    pub status_detail: String,
    pub observed_active: bool,
    pub counts: DisplayCountSummary,
    pub attention: AttentionState,
    pub displays: Vec<DisplaySnapshot>,
    pub backlight: BacklightSnapshot,
    pub solar: SolarRuntimeSnapshot,
    pub location: Option<LocationSnapshot>,
    pub nightlight: Option<NightlightSnapshot>,
    pub backend: BackendSnapshot,
    pub busy: bool,
    pub pending_restore_count: usize,
    pub last_error: Option<String>,
    /// Unix seconds when the snapshot was produced.
    pub updated_at: i64,
}

impl Snapshot {
    pub fn empty() -> Self {
        Self {
            schema_version: IPC_SCHEMA_VERSION,
            version: AppVersion::MARKETING.to_string(),
            edition: AppVersion::EDITION.to_string(),
            settings: EmberSettings::default(),
            runtime_state: EmberOperationState::Off,
            runtime_label: "Off".into(),
            status_title: "Project Ember".into(),
            status_detail: "Display control is not running.".into(),
            observed_active: false,
            counts: DisplayCountSummary::default(),
            attention: AttentionState::none(),
            displays: Vec::new(),
            backlight: BacklightSnapshot::default(),
            solar: SolarRuntimeSnapshot::default(),
            location: None,
            nightlight: None,
            backend: BackendSnapshot::default(),
            busy: false,
            pending_restore_count: 0,
            last_error: None,
            updated_at: Utc::now().timestamp(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    pub fn from_json(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    pub fn tray_tooltip(&self) -> String {
        let mut lines = vec![self.status_title.clone(), self.status_detail.clone()];
        if self.backlight.state == BacklightState::Engaged {
            lines.push("Backlight Lock engaged".into());
        }
        if self.attention.severity >= crate::presentation::AttentionSeverity::Warning {
            lines.push(format!(
                "{}: {}",
                self.attention.title, self.attention.message
            ));
        }
        lines.join("\n")
    }

    pub fn footer_version(&self) -> String {
        format!("v{} · {}", self.version, self.edition)
    }

    pub fn override_until_local(&self) -> Option<DateTime<Utc>> {
        self.solar
            .override_until
            .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_json_round_trips() {
        let mut snapshot = Snapshot::empty();
        snapshot.observed_active = true;
        snapshot.counts.verified = 2;
        snapshot.status_title = "Ember is on".into();
        let json = snapshot.to_json();
        let decoded = Snapshot::from_json(&json).unwrap();
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn tooltip_includes_attention() {
        let mut snapshot = Snapshot::empty();
        snapshot.attention = AttentionState::new(
            crate::presentation::AttentionSeverity::Warning,
            "Careful",
            "Something happened",
            vec![],
        );
        let tooltip = snapshot.tray_tooltip();
        assert!(tooltip.contains("Careful"));
        assert!(tooltip.contains("Something happened"));
    }

    #[test]
    fn version_footer_is_linux_branded() {
        let snapshot = Snapshot::empty();
        assert_eq!(snapshot.footer_version(), "v0.4.1 · Omarchy");
    }
}
