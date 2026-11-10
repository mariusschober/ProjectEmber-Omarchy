use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::app_version::AppVersion;
use crate::gamma::GammaTable;
use crate::identity::DisplayIdentity;
use crate::solar::CachedSolarLocation;

/// Serializes `DateTime<Utc>` as seconds since the Unix epoch, matching the
/// macOS journal's `JSONEncoder.dateEncodingStrategy = .secondsSince1970`.
/// Deserialization accepts either a JSON number or an RFC 3339 string so that
/// hand-edited journals and future exporters still work.
pub mod epoch_seconds {
    use super::*;
    use serde::de::Error as _;

    pub fn serialize<S>(date: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Millisecond precision keeps round-trips exact while remaining a
        // seconds-since-epoch JSON number (the macOS encoding).
        serializer.serialize_f64(date.timestamp_millis() as f64 / 1000.0)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(f64),
            Text(String),
        }

        match Raw::deserialize(deserializer)? {
            Raw::Number(seconds) => {
                if !seconds.is_finite() {
                    return Err(D::Error::custom("timestamp is not finite"));
                }
                let millis = (seconds * 1000.0).round();
                if millis.abs() > 9.0e15 {
                    return Err(D::Error::custom("timestamp is out of range"));
                }
                DateTime::from_timestamp_millis(millis as i64)
                    .ok_or_else(|| D::Error::custom("timestamp is out of range"))
            }
            Raw::Text(text) => DateTime::parse_from_rfc3339(&text)
                .map(|value| value.with_timezone(&Utc))
                .map_err(D::Error::custom),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationOverride {
    pub filter_enabled: bool,
    #[serde(with = "epoch_seconds")]
    pub expires_at: DateTime<Utc>,
}

impl AutomationOverride {
    pub fn new(filter_enabled: bool, expires_at: DateTime<Utc>) -> Self {
        Self {
            filter_enabled,
            expires_at,
        }
    }

    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        self.expires_at > now
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TrayPrimaryAction {
    #[default]
    OpenControls,
    ToggleEmber,
}

impl TrayPrimaryAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenControls => "openControls",
            Self::ToggleEmber => "toggleEmber",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OpenControls => "Open Controls",
            Self::ToggleEmber => "Toggle Ember",
        }
    }
}

impl fmt::Display for TrayPrimaryAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TrayPrimaryAction {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "openControls" | "open-controls" | "open" => Ok(Self::OpenControls),
            "toggleEmber" | "toggle" => Ok(Self::ToggleEmber),
            other => Err(format!("unknown tray action: {other}")),
        }
    }
}

/// User settings. Missing keys fall back to defaults, so settings files written
/// by older builds (or the macOS app) keep loading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EmberSettings {
    pub warmth: f64,
    pub apparent_brightness: f64,
    pub filter_enabled: bool,
    pub backlight_lock_enabled: bool,
    pub launch_at_login: bool,
    pub sun_schedule_enabled: bool,
    pub automation_override: Option<AutomationOverride>,
    #[serde(alias = "menuBarPrimaryAction")]
    pub tray_primary_action: TrayPrimaryAction,
    /// Rounded (0.1°) local coordinates for the Sun schedule. Only present
    /// after an explicit location grant; cleared on request.
    pub solar_location: Option<CachedSolarLocation>,
    /// When true, Ember warns when Omarchy's nightlight (hyprsunset) is also
    /// active. It never changes another application's state.
    pub warn_on_nightlight: bool,
}

impl Default for EmberSettings {
    fn default() -> Self {
        Self {
            warmth: 0.62,
            apparent_brightness: 0.75,
            filter_enabled: false,
            backlight_lock_enabled: false,
            launch_at_login: false,
            sun_schedule_enabled: false,
            automation_override: None,
            tray_primary_action: TrayPrimaryAction::OpenControls,
            solar_location: None,
            warn_on_nightlight: true,
        }
    }
}

impl EmberSettings {
    pub fn normalize(&mut self, now: DateTime<Utc>) {
        self.warmth = self.warmth.clamp(0.0, 1.0);
        self.apparent_brightness = self.apparent_brightness.clamp(0.10, 1.0);
        if let Some(automation_override) = &self.automation_override {
            if automation_override.expires_at <= now {
                self.automation_override = None;
            }
        }
    }

    pub fn warmth_percent(&self) -> f64 {
        (self.warmth * 100.0).round()
    }

    pub fn brightness_percent(&self) -> f64 {
        (self.apparent_brightness * 100.0).round()
    }

    pub fn preset(&self) -> Option<EmberPreset> {
        EmberPreset::from_warmth(self.warmth)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EmberPreset {
    Neutral,
    Evening,
    PureRed,
}

impl EmberPreset {
    pub const ALL: [EmberPreset; 3] = [Self::Neutral, Self::Evening, Self::PureRed];

    pub fn warmth(self) -> f64 {
        match self {
            Self::Neutral => 0.0,
            Self::Evening => 0.62,
            Self::PureRed => 1.0,
        }
    }

    pub fn from_warmth(warmth: f64) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| (warmth - preset.warmth()).abs() < 0.01)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Neutral => "Neutral",
            Self::Evening => "Evening",
            Self::PureRed => "Pure Red",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Evening => "evening",
            Self::PureRed => "pureRed",
        }
    }
}

impl fmt::Display for EmberPreset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for EmberPreset {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "neutral" => Ok(Self::Neutral),
            "evening" => Ok(Self::Evening),
            "purered" | "red" => Ok(Self::PureRed),
            other => Err(format!("unknown preset: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorGains {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
}

impl ColorGains {
    pub fn new(red: f32, green: f32, blue: f32) -> Self {
        Self {
            red: red.clamp(0.0, 1.0),
            green: green.clamp(0.0, 1.0),
            blue: blue.clamp(0.0, 1.0),
        }
    }

    pub const fn neutral() -> Self {
        Self {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
        }
    }
}

impl Default for ColorGains {
    fn default() -> Self {
        Self::neutral()
    }
}

/// Native hardware controls captured before Ember changes them. Linux has no
/// firmware "automatic brightness" flag; the optional field is kept for journal
/// compatibility and is always `None` on Linux. `device` records which
/// `/sys/class/backlight` device the baseline came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct HardwareBaseline {
    pub brightness: Option<f32>,
    pub ambient_light_compensation_enabled: Option<bool>,
    pub device: Option<String>,
}

impl HardwareBaseline {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn has_values(&self) -> bool {
        self.brightness.is_some() || self.ambient_light_compensation_enabled.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum GammaSource {
    /// The table was read back from the display server (X11 XRandR).
    #[default]
    Readback,
    /// Wayland exposes no gamma readback. The saved table is the compositor's
    /// default ramp (identity); restoring means releasing gamma control.
    CompositorDefault,
    /// Simulation backend.
    Simulated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayBaseline {
    #[serde(rename = "displayID")]
    pub display_id: u32,
    pub identity: Option<DisplayIdentity>,
    pub gamma_table: GammaTable,
    #[serde(with = "epoch_seconds")]
    pub captured_at: DateTime<Utc>,
    #[serde(default)]
    pub gamma_source: GammaSource,
}

impl DisplayBaseline {
    pub fn new(
        display_id: u32,
        identity: Option<DisplayIdentity>,
        gamma_table: GammaTable,
        captured_at: DateTime<Utc>,
    ) -> Self {
        Self {
            display_id,
            identity,
            gamma_table,
            captured_at: truncate_to_millis(captured_at),
            gamma_source: GammaSource::Readback,
        }
    }

    pub fn with_source(mut self, source: GammaSource) -> Self {
        self.gamma_source = source;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayRecoveryEntry {
    pub identity: DisplayIdentity,
    pub display: DisplayBaseline,
    #[serde(default)]
    pub hardware: HardwareBaseline,
}

impl DisplayRecoveryEntry {
    pub fn new(
        identity: DisplayIdentity,
        display: DisplayBaseline,
        hardware: HardwareBaseline,
    ) -> Self {
        Self {
            identity,
            display,
            hardware,
        }
    }

    pub fn without_hardware(identity: DisplayIdentity, display: DisplayBaseline) -> Self {
        Self {
            identity,
            display,
            hardware: HardwareBaseline::empty(),
        }
    }
}

/// Schema-v2 recovery record. Serialized with camelCase keys and epoch-second
/// timestamps for structural compatibility with the macOS journal.
///
/// Deserialization also accepts the schema-v1 single-display shape
/// (`display` + `hardware`) and explicitly rejects future schemas instead of
/// silently treating them as v2.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryRecord {
    pub schema_version: u32,
    pub app_version: String,
    #[serde(with = "epoch_seconds")]
    pub created_at: DateTime<Utc>,
    pub displays: Vec<DisplayRecoveryEntry>,
    pub intended_settings: EmberSettings,
}

impl RecoveryRecord {
    pub fn new(
        app_version: impl Into<String>,
        displays: Vec<DisplayRecoveryEntry>,
        intended_settings: EmberSettings,
    ) -> Self {
        Self {
            schema_version: AppVersion::MAX_SUPPORTED_SCHEMA,
            app_version: app_version.into(),
            created_at: truncate_to_millis(Utc::now()),
            displays,
            intended_settings,
        }
    }

    pub fn with_created_at(mut self, created_at: DateTime<Utc>) -> Self {
        self.created_at = truncate_to_millis(created_at);
        self
    }

    pub fn intended_identities(&self) -> Vec<DisplayIdentity> {
        self.displays
            .iter()
            .map(|entry| entry.identity.clone())
            .collect()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRecoveryRecord {
    #[serde(default)]
    schema_version: Option<u32>,
    app_version: String,
    #[serde(with = "epoch_seconds")]
    created_at: DateTime<Utc>,
    #[serde(default)]
    displays: Option<Vec<DisplayRecoveryEntry>>,
    intended_settings: EmberSettings,
    #[serde(default)]
    display: Option<DisplayBaseline>,
    #[serde(default)]
    hardware: Option<HardwareBaseline>,
}

impl<'de> Deserialize<'de> for RecoveryRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error as _;
        let raw = RawRecoveryRecord::deserialize(deserializer)?;
        if let Some(version) = raw.schema_version {
            if !AppVersion::supports_schema(version) {
                return Err(D::Error::custom(format!(
                    "Unsupported recovery schema v{version}; this build supports up to v{}.",
                    AppVersion::MAX_SUPPORTED_SCHEMA
                )));
            }
        }
        if let Some(displays) = raw.displays {
            let version = raw
                .schema_version
                .unwrap_or(AppVersion::MAX_SUPPORTED_SCHEMA);
            return Ok(Self {
                schema_version: version,
                app_version: raw.app_version,
                created_at: raw.created_at,
                displays,
                intended_settings: raw.intended_settings,
            });
        }
        // v1: one display plus hardware at the top level.
        let display = raw
            .display
            .ok_or_else(|| D::Error::custom("recovery record has neither displays nor display"))?;
        let hardware = raw.hardware.unwrap_or_default();
        let identity = display
            .identity
            .clone()
            .unwrap_or_else(|| DisplayIdentity::legacy(display.display_id));
        Ok(Self {
            schema_version: 1,
            app_version: raw.app_version,
            created_at: raw.created_at,
            displays: vec![DisplayRecoveryEntry::new(identity, display, hardware)],
            intended_settings: raw.intended_settings,
        })
    }
}

/// Journal timestamps serialize with millisecond precision; normalize at
/// construction so round-trips compare equal.
pub fn truncate_to_millis(value: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(value.timestamp_millis()).unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_defaults_are_stable() {
        let settings = EmberSettings::default();
        assert_eq!(settings.warmth, 0.62);
        assert_eq!(settings.apparent_brightness, 0.75);
        assert!(!settings.filter_enabled);
        assert_eq!(
            settings.tray_primary_action,
            TrayPrimaryAction::OpenControls
        );
    }

    #[test]
    fn legacy_settings_decode_with_open_controls() {
        let json = json!({
            "warmth": 0.4,
            "apparentBrightness": 0.6,
            "filterEnabled": true,
            "backlightLockEnabled": false,
            "launchAtLogin": true
        });
        let decoded: EmberSettings = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.tray_primary_action, TrayPrimaryAction::OpenControls);
        assert!(decoded.launch_at_login);
        assert_eq!(decoded.warmth, 0.4);
    }

    #[test]
    fn macos_menu_bar_key_is_accepted() {
        let decoded: EmberSettings =
            serde_json::from_value(json!({"menuBarPrimaryAction": "toggleEmber"})).unwrap();
        assert_eq!(decoded.tray_primary_action, TrayPrimaryAction::ToggleEmber);
    }

    #[test]
    fn normalize_clamps_and_expires() {
        let now = Utc::now();
        let mut settings = EmberSettings {
            warmth: 12.0,
            apparent_brightness: 0.0,
            automation_override: Some(AutomationOverride::new(
                true,
                now - chrono::Duration::seconds(1),
            )),
            ..Default::default()
        };
        settings.normalize(now);
        assert_eq!(settings.warmth, 1.0);
        assert_eq!(settings.apparent_brightness, 0.10);
        assert!(settings.automation_override.is_none());
    }

    #[test]
    fn presets_round_trip() {
        for preset in EmberPreset::ALL {
            assert_eq!(
                preset.warmth(),
                EmberPreset::from_warmth(preset.warmth()).unwrap().warmth()
            );
            assert_eq!(preset.as_str().parse::<EmberPreset>().unwrap(), preset);
        }
        assert!(EmberPreset::from_warmth(0.33).is_none());
    }

    #[test]
    fn record_round_trips_as_schema_two() {
        let record = RecoveryRecord::new("test", vec![], EmberSettings::default());
        let text = serde_json::to_string(&record).unwrap();
        assert!(text.contains("\"schemaVersion\":2"));
        assert!(text.contains("\"createdAt\":"));
        let decoded: RecoveryRecord = serde_json::from_str(&text).unwrap();
        assert_eq!(decoded.schema_version, 2);
    }

    #[test]
    fn v1_record_migrates() {
        let json = json!({
            "schemaVersion": 1,
            "appVersion": "legacy",
            "createdAt": 1.0,
            "display": {
                "displayID": 42,
                "gammaTable": {"red": [0.0, 1.0], "green": [0.0, 1.0], "blue": [0.0, 1.0]},
                "capturedAt": 1.0
            },
            "hardware": {"brightness": null, "ambientLightCompensationEnabled": null},
            "intendedSettings": {}
        });
        let decoded: RecoveryRecord = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.schema_version, 1);
        assert_eq!(decoded.displays.len(), 1);
        assert_eq!(decoded.displays[0].identity.legacy_display_id, Some(42));
        assert!(decoded.displays[0].identity.is_built_in);
    }

    #[test]
    fn future_schema_is_rejected() {
        let json = json!({
            "schemaVersion": 99,
            "appVersion": "future",
            "createdAt": 1.0,
            "displays": [],
            "intendedSettings": {}
        });
        let error = serde_json::from_value::<RecoveryRecord>(json).unwrap_err();
        assert!(error.to_string().contains("Unsupported recovery schema"));
    }

    #[test]
    fn epoch_seconds_accepts_rfc3339() {
        let json = json!({
            "appVersion": "x",
            "createdAt": "2026-11-08T20:00:00Z",
            "displays": [],
            "intendedSettings": {}
        });
        let decoded: RecoveryRecord = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.created_at.timestamp(), 1_794_168_000);
    }

    #[test]
    fn hardware_baseline_empty_semantics() {
        assert!(!HardwareBaseline::empty().has_values());
        assert!(HardwareBaseline {
            brightness: Some(0.8),
            ..Default::default()
        }
        .has_values());
    }
}
