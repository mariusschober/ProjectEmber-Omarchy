use ember_core::{
    AttentionAction, AttentionSeverity, AttentionState, BackendKind, BackendSnapshot,
    BacklightSnapshot, BacklightState, DisplayCountSummary, DisplaySnapshot, DisplayState,
    EmberOperationState, EmberSettings, LocationState, Snapshot, SolarRuntimeSnapshot,
    VerificationLevel,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoState {
    Active,
    Off,
    Degraded,
}

pub fn snapshot(state: DemoState) -> Snapshot {
    let active = matches!(state, DemoState::Active | DemoState::Degraded);
    let mut snapshot = Snapshot::empty();
    snapshot.settings = EmberSettings {
        warmth: 0.62,
        apparent_brightness: 0.75,
        filter_enabled: active,
        backlight_lock_enabled: true,
        launch_at_login: true,
        ..Default::default()
    };
    if let Some(location) = snapshot.settings.solar_location.as_mut() {
        location.coordinate.latitude = 52.5;
        location.coordinate.longitude = 13.4;
    }
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
    snapshot.counts = DisplayCountSummary {
        verified: if active { 2 } else { 0 },
        unsupported: 0,
        pending: 0,
        failed: 0,
        mirrored: 0,
    };
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
    snapshot.backlight = BacklightSnapshot {
        available: true,
        ambient_control: false,
        state: if active {
            BacklightState::Engaged
        } else {
            BacklightState::Disengaged
        },
        detail: None,
        device: Some("intel_backlight".into()),
    };
    snapshot.solar = SolarRuntimeSnapshot {
        enabled: true,
        is_night: true,
        next_event: Some(ember_core::SolarEvent::new(
            ember_core::SolarEventKind::Sunrise,
            chrono::Utc::now() + chrono::Duration::hours(9),
        )),
        override_until: None,
        location_state: LocationState::Granted,
        location_detail: None,
        refreshing: false,
        timezone: "Europe/Berlin".into(),
    };
    snapshot.nightlight = Some(ember_core::NightlightSnapshot {
        active: false,
        temperature: Some(6500),
    });
    if state == DemoState::Degraded {
        snapshot.attention = AttentionState::new(
            AttentionSeverity::Error,
            "Needs attention",
            "Another display service repeatedly replaced Ember's color table on DP-1.",
            vec![AttentionAction::Retry, AttentionAction::Reset],
        );
        snapshot.counts.verified = 1;
        snapshot.counts.failed = 1;
        if let Some(display) = snapshot.displays.get_mut(1) {
            display.state = DisplayState::Failed;
            display.detail = Some("Color table was replaced by another service.".into());
        }
        snapshot.status_title = "Needs attention".into();
        snapshot.status_detail =
            "Another display service repeatedly replaced Ember's color table.".into();
        snapshot.observed_active = false;
    }
    snapshot
}
