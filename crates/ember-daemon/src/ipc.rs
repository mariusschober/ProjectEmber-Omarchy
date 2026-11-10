use ember_core::solar::SolarSchedule;
use ember_core::{
    AttentionAction, AttentionState, BackendKind, BackendSnapshot, BacklightSnapshot,
    BacklightState, DisplayIdentity, DisplaySnapshot, DisplayState, DisplayTopologySnapshot,
    EmberOperationState, EmberPresentation, EmberSettings, LocationSnapshot, LocationSource,
    LocationState, NightlightSnapshot, Snapshot, SolarRuntimeSnapshot, VerificationLevel,
    IPC_SCHEMA_VERSION,
};
use ember_platform::DisplayInfo;

/// Runtime backlight state shared with the snapshot builder.
#[derive(Debug, Clone)]
pub struct BacklightRuntime {
    pub capability: ember_platform::BacklightCapability,
    pub state: BacklightRuntimeState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SolarRuntime {
    pub location_state: LocationState,
    pub location_detail: Option<String>,
    pub refreshing: bool,
    pub timezone: String,
}

#[derive(Debug, Clone, Copy)]
pub struct VerificationRuntime {
    pub level: VerificationLevel,
    pub readback: bool,
}

/// Builds the single wire/render snapshot from coordinator state.
pub struct SnapshotBuilder;

pub struct SnapshotInput<'a> {
    pub settings: &'a EmberSettings,
    pub operation: EmberOperationState,
    pub presentation: &'a EmberPresentation,
    pub displays: &'a [DisplayInfo],
    pub pending_entries: &'a [DisplayIdentity],
    pub verified: &'a [DisplayIdentity],
    pub failed: &'a [(DisplayIdentity, String)],
    pub journaled_offline: &'a [DisplayIdentity],
    pub topology: Option<&'a DisplayTopologySnapshot>,
    pub backlight: &'a BacklightRuntime,
    pub solar: &'a SolarRuntime,
    pub solar_schedule: Option<&'a SolarSchedule>,
    pub nightlight: Option<&'a NightlightSnapshot>,
    pub verification: &'a VerificationRuntime,
    pub backend_kind: BackendKind,
    pub backend_name: &'a str,
    pub backend_note: Option<&'a str>,
    pub busy: bool,
    pub last_error: Option<&'a str>,
}

impl SnapshotInput<'_> {
    pub fn build(&self) -> Snapshot {
        let online_keys: std::collections::HashSet<String> = self
            .displays
            .iter()
            .map(|info| info.identity.key())
            .collect();

        let mut displays = Vec::new();
        for info in self.displays {
            let key = info.identity.key();
            let state = if !info.gamma_compatible() {
                DisplayState::Unsupported
            } else if self
                .failed
                .iter()
                .any(|(identity, _)| identity.key() == key)
            {
                DisplayState::Failed
            } else if self.verified.iter().any(|identity| identity.key() == key) {
                DisplayState::Active
            } else {
                DisplayState::Idle
            };
            let detail = if !info.gamma_compatible() {
                info.unsupported_reason.clone()
            } else if let Some((_, message)) = self
                .failed
                .iter()
                .find(|(identity, _)| identity.key() == key)
            {
                Some(message.clone())
            } else {
                None
            };
            displays.push(DisplaySnapshot {
                identity_key: key,
                label: info.identity.label(),
                connector: info.connector.clone(),
                built_in: info.built_in,
                mirrored: info.is_mirrored,
                gamma_compatible: info.gamma_compatible(),
                state,
                verification: self.verification.level,
                detail,
            });
        }

        // Journaled displays that are no longer online are pending, not forgotten.
        for identity in self.journaled_offline {
            if online_keys.contains(&identity.key()) {
                continue;
            }
            displays.push(DisplaySnapshot {
                identity_key: identity.key(),
                label: identity.label(),
                connector: identity.connector.clone(),
                built_in: identity.is_built_in,
                mirrored: false,
                gamma_compatible: true,
                state: DisplayState::Pending,
                verification: self.verification.level,
                detail: Some("Saved original state retained until this display reconnects.".into()),
            });
        }

        let unsupported = displays
            .iter()
            .filter(|display| display.state == DisplayState::Unsupported)
            .count();
        let pending = displays
            .iter()
            .filter(|display| display.state == DisplayState::Pending)
            .count();
        let mirrored = displays.iter().filter(|display| display.mirrored).count();
        let failed_count = displays
            .iter()
            .filter(|display| display.state == DisplayState::Failed)
            .count();
        let mut counts = self.presentation.counts;
        counts.unsupported = unsupported;
        counts.pending = pending.max(self.pending_entries.len());
        counts.mirrored = mirrored;
        counts.failed = failed_count.max(self.failed.len());
        counts.verified = self
            .verified
            .iter()
            .filter(|identity| online_keys.contains(&identity.key()))
            .count();

        let next_event = self.solar_schedule.and_then(|schedule| schedule.next_event);
        let location_state = if !self.settings.sun_schedule_enabled {
            LocationState::Disabled
        } else {
            self.solar.location_state
        };
        let solar = SolarRuntimeSnapshot {
            enabled: self.settings.sun_schedule_enabled,
            is_night: self
                .solar_schedule
                .map(|schedule| schedule.is_night)
                .unwrap_or(false),
            next_event,
            override_until: self
                .settings
                .automation_override
                .as_ref()
                .map(|value| value.expires_at.timestamp()),
            location_state,
            location_detail: self.solar.location_detail.clone(),
            refreshing: self.solar.refreshing,
            timezone: self.solar.timezone.clone(),
        };

        let location = self.settings.solar_location.as_ref().map(|cached| {
            let source = if self.solar.location_state == LocationState::Granted {
                LocationSource::Cached
            } else {
                LocationSource::Manual
            };
            LocationSnapshot {
                latitude: cached.coordinate.latitude,
                longitude: cached.coordinate.longitude,
                source,
                age_seconds: chrono::Utc::now()
                    .signed_duration_since(cached.captured_at)
                    .num_seconds(),
            }
        });

        let backlight = BacklightSnapshot {
            available: self.backlight.capability.available,
            ambient_control: self.backlight.capability.ambient_control,
            state: match self.backlight.state {
                BacklightRuntimeState::Disengaged => BacklightState::Disengaged,
                BacklightRuntimeState::Engaged => BacklightState::Engaged,
                BacklightRuntimeState::Unavailable => BacklightState::Unavailable,
                BacklightRuntimeState::Failed => BacklightState::Failed,
            },
            detail: self.backlight.detail.clone(),
            device: self.backlight.capability.device.clone(),
        };

        Snapshot {
            schema_version: IPC_SCHEMA_VERSION,
            version: ember_core::AppVersion::MARKETING.to_string(),
            edition: ember_core::AppVersion::EDITION.to_string(),
            settings: self.settings.clone(),
            runtime_state: self.operation,
            runtime_label: operation_label(self.operation).to_string(),
            status_title: self.presentation.status_title.clone(),
            status_detail: self.presentation.status_detail.clone(),
            observed_active: self.presentation.is_observed_active,
            counts,
            attention: effective_attention(self.presentation, self.last_error),
            displays,
            backlight,
            solar,
            location,
            nightlight: self.nightlight.cloned(),
            backend: BackendSnapshot {
                kind: self.backend_kind,
                name: self.backend_name.to_string(),
                verification: self.verification.level,
                readback: self.verification.readback,
                note: self.backend_note.map(str::to_string),
            },
            busy: self.busy,
            pending_restore_count: pending.max(self.pending_entries.len()),
            last_error: self.last_error.map(str::to_string),
            updated_at: chrono::Utc::now().timestamp(),
        }
    }
}

fn effective_attention(
    presentation: &EmberPresentation,
    last_error: Option<&str>,
) -> AttentionState {
    if !presentation.attention.is_none() {
        return presentation.attention.clone();
    }
    if let Some(error) = last_error {
        return AttentionState::new(
            ember_core::AttentionSeverity::Error,
            "Needs attention",
            error,
            vec![AttentionAction::Retry, AttentionAction::Reset],
        );
    }
    AttentionState::none()
}

fn operation_label(state: EmberOperationState) -> &'static str {
    match state {
        EmberOperationState::Off => "Off",
        EmberOperationState::Activating => "Applying",
        EmberOperationState::Active => "Active",
        EmberOperationState::Reconciling => "Reconciling",
        EmberOperationState::Restoring => "Restoring",
        EmberOperationState::Suspended => "Suspended",
        EmberOperationState::Degraded => "Needs attention",
    }
}

/// Backlight runtime state used by the snapshot builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BacklightRuntimeState {
    Disengaged,
    Engaged,
    Unavailable,
    Failed,
}

/// Helper used by the GUI demo and tests.
pub fn demo_snapshot(active: bool) -> Snapshot {
    let mut snapshot = Snapshot::empty();
    snapshot.settings.filter_enabled = active;
    snapshot.observed_active = active;
    snapshot.runtime_state = if active {
        EmberOperationState::Active
    } else {
        EmberOperationState::Off
    };
    snapshot.runtime_label = if active { "Active" } else { "Off" }.into();
    snapshot.status_title = if active { "Ember is on" } else { "Ready" }.into();
    snapshot.status_detail = if active {
        "Color and software brightness are active on every compatible display.".into()
    } else {
        "2 compatible displays ready; original state untouched.".into()
    };
    snapshot.counts.verified = if active { 2 } else { 0 };
    snapshot.displays = vec![
        DisplaySnapshot {
            identity_key: "uuid:demo-edp".into(),
            label: "eDP-1 (built-in)".into(),
            connector: "eDP-1".into(),
            built_in: true,
            mirrored: false,
            gamma_compatible: true,
            state: if active {
                DisplayState::Active
            } else {
                DisplayState::Idle
            },
            verification: VerificationLevel::Applied,
            detail: None,
        },
        DisplaySnapshot {
            identity_key: "uuid:demo-dp".into(),
            label: "DP-1 (external)".into(),
            connector: "DP-1".into(),
            built_in: false,
            mirrored: false,
            gamma_compatible: true,
            state: if active {
                DisplayState::Active
            } else {
                DisplayState::Idle
            },
            verification: VerificationLevel::Applied,
            detail: None,
        },
    ];
    snapshot.backend = BackendSnapshot {
        kind: BackendKind::WaylandWlrGamma,
        name: "wayland-wlr-gamma".into(),
        verification: VerificationLevel::Applied,
        readback: false,
        note: None,
    };
    snapshot
}
