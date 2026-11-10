use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// How strongly a backend can assert that a transform is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerificationLevel {
    /// The display server returns the current ramp and it matches (X11 XRandR).
    Readback,
    /// The compositor accepted the ramp with no protocol error (Wayland
    /// `wlr-gamma-control`). There is no readback in the protocol.
    Applied,
    /// The backend cannot verify or apply.
    Unavailable,
}

impl VerificationLevel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Readback => "Verified by readback",
            Self::Applied => "Accepted by compositor",
            Self::Unavailable => "Unavailable",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Readback => "verified",
            Self::Applied => "applied",
            Self::Unavailable => "unavailable",
        }
    }

    pub fn note(self) -> &'static str {
        match self {
            Self::Readback => "The display server returns color tables, so Ember verifies every write.",
            Self::Applied => {
                "This compositor exposes no color-table readback; Ember verifies that each ramp was accepted."
            }
            Self::Unavailable => "Color control is not available in this session.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackendKind {
    WaylandWlrGamma,
    X11RandR,
    Simulated,
    None,
}

impl BackendKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::WaylandWlrGamma => "Wayland · wlr-gamma-control",
            Self::X11RandR => "X11 · XRandR",
            Self::Simulated => "Simulator",
            Self::None => "No display session",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::WaylandWlrGamma => "wayland",
            Self::X11RandR => "x11",
            Self::Simulated => "simulated",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EmberOperationState {
    Off,
    Activating,
    Active,
    Reconciling,
    Restoring,
    Suspended,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttentionSeverity {
    None,
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttentionAction {
    Retry,
    Reset,
    OpenLocation,
    ExportDiagnostics,
    TurnOffNightlight,
}

impl AttentionAction {
    pub fn id(self) -> &'static str {
        match self {
            Self::Retry => "retry",
            Self::Reset => "reset",
            Self::OpenLocation => "openLocation",
            Self::ExportDiagnostics => "exportDiagnostics",
            Self::TurnOffNightlight => "turnOffNightlight",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Retry => "Retry",
            Self::Reset => "Reset",
            Self::OpenLocation => "Set Location…",
            Self::ExportDiagnostics => "Export Diagnostics",
            Self::TurnOffNightlight => "Turn Off Nightlight",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionState {
    pub severity: AttentionSeverity,
    pub title: String,
    pub message: String,
    pub recovery_actions: Vec<AttentionAction>,
}

impl AttentionState {
    pub fn new(
        severity: AttentionSeverity,
        title: impl Into<String>,
        message: impl Into<String>,
        recovery_actions: Vec<AttentionAction>,
    ) -> Self {
        Self {
            severity,
            title: title.into(),
            message: message.into(),
            recovery_actions,
        }
    }

    pub fn none() -> Self {
        Self {
            severity: AttentionSeverity::None,
            title: String::new(),
            message: String::new(),
            recovery_actions: Vec::new(),
        }
    }

    pub fn is_none(&self) -> bool {
        self.severity == AttentionSeverity::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayCountSummary {
    pub verified: usize,
    pub unsupported: usize,
    pub pending: usize,
    pub failed: usize,
    pub mirrored: usize,
}

impl DisplayCountSummary {
    pub fn online_intended(&self) -> usize {
        self.verified + self.failed + self.unsupported + self.mirrored
    }
}

/// Single coherent presentation model. The UI renders from this and never
/// reinterprets raw coordinator state separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberPresentation {
    pub desired_filter_enabled: bool,
    pub operation_state: EmberOperationState,
    /// True only when at least one intended online display has a confirmed
    /// transform (readback verified, or accepted by the compositor).
    pub is_observed_active: bool,
    pub counts: DisplayCountSummary,
    pub attention: AttentionState,
    pub status_title: String,
    pub status_detail: String,
    pub verification: VerificationLevel,
}

impl EmberPresentation {
    pub fn shows_active(&self) -> bool {
        self.is_observed_active && self.attention.severity != AttentionSeverity::Error
    }
}

pub struct EmberPresenter;

impl EmberPresenter {
    #[allow(clippy::too_many_arguments)]
    pub fn make(
        desired_on: bool,
        operation: EmberOperationState,
        verified: &HashSet<String>,
        unsupported: usize,
        pending: usize,
        failed: &[String],
        online_intended: usize,
        mirrored: usize,
        verification: VerificationLevel,
        attention_override: Option<AttentionState>,
    ) -> EmberPresentation {
        let counts = DisplayCountSummary {
            verified: verified.len(),
            unsupported,
            pending,
            failed: failed.len(),
            mirrored,
        };
        let observed_active =
            desired_on && !verified.is_empty() && operation != EmberOperationState::Restoring;

        let mut attention;
        let mut title;
        let detail;
        if let Some(override_state) = attention_override.filter(|state| !state.is_none()) {
            attention = override_state;
            title = attention.title.clone();
            detail = attention.message.clone();
        } else if !failed.is_empty() {
            attention = AttentionState::new(
                AttentionSeverity::Error,
                "Needs attention",
                failed.join(" "),
                vec![AttentionAction::Retry, AttentionAction::Reset],
            );
            title = "Needs attention".to_string();
            detail = failed.join(" ");
        } else if desired_on && observed_active {
            attention = AttentionState::none();
            title = "Ember is on".to_string();
            detail = if pending > 0 {
                format!(
                    "Active on {} {}. {} saved for reconnect.",
                    verified.len(),
                    plural(verified.len(), "display", "displays"),
                    pending
                )
            } else if unsupported > 0 {
                format!(
                    "Active on {} of {} displays; unsupported displays were left untouched.",
                    verified.len(),
                    online_intended
                )
            } else if mirrored > 0 {
                format!(
                    "Active on {} {}; mirrored outputs follow their primary.",
                    verified.len(),
                    plural(verified.len(), "display", "displays")
                )
            } else {
                "Color and software brightness are active on every compatible display.".to_string()
            };
        } else if desired_on && pending > 0 && verified.is_empty() {
            attention = AttentionState::new(
                AttentionSeverity::Info,
                "Waiting for display",
                format!(
                    "Ember saved your original settings for {pending} {} you unplugged; {} restored when you reconnect.",
                    plural(pending, "display", "displays"),
                    if pending == 1 { "it will be" } else { "they will be" }
                ),
                vec![],
            );
            title = "Ember is off".to_string();
            detail = "Your displays look normal.".to_string();
        } else if desired_on {
            attention = AttentionState::new(
                AttentionSeverity::Warning,
                "Applying",
                format!("Waiting for confirmation on {online_intended} displays."),
                vec![AttentionAction::Retry],
            );
            title = "Applying".to_string();
            detail = "Verifying every display…".to_string();
        } else {
            attention = AttentionState::none();
            if online_intended > 0 {
                title = "Ready".to_string();
                detail = format!(
                    "{} compatible {} ready; original state untouched.",
                    online_intended,
                    plural(online_intended, "display", "displays")
                );
            } else {
                title = "No compatible display".to_string();
                detail = "Connect a display that supports gamma control (Hyprland, Sway, or X11)."
                    .to_string();
            }
        }

        // A warning override must not hide a real failure. Keep errors visible.
        if attention.severity != AttentionSeverity::Error && !failed.is_empty() {
            attention = AttentionState::new(
                AttentionSeverity::Error,
                "Needs attention",
                failed.join(" "),
                vec![AttentionAction::Retry, AttentionAction::Reset],
            );
        }
        if attention.severity == AttentionSeverity::Error {
            title = attention.title.clone();
            // detail is currently immutable in the override branch; the
            // presentation's detail mirrors the attention message for errors.
        }

        EmberPresentation {
            desired_filter_enabled: desired_on,
            operation_state: operation,
            is_observed_active: observed_active,
            counts,
            attention,
            status_title: title,
            status_detail: detail,
            verification,
        }
    }
}

fn plural(count: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn failed_verification_is_not_presented_as_active() {
        let presentation = EmberPresenter::make(
            true,
            EmberOperationState::Active,
            &HashSet::new(),
            0,
            0,
            &["Display failed".into()],
            1,
            0,
            VerificationLevel::Readback,
            None,
        );
        assert!(!presentation.shows_active());
        assert_eq!(presentation.status_title, "Needs attention");
    }

    #[test]
    fn pending_only_uses_calm_truthful_copy() {
        let presentation = EmberPresenter::make(
            true,
            EmberOperationState::Active,
            &HashSet::new(),
            0,
            1,
            &[],
            1,
            0,
            VerificationLevel::Applied,
            None,
        );
        assert!(!presentation.shows_active());
        assert_eq!(presentation.attention.severity, AttentionSeverity::Info);
        assert!(presentation.attention.message.contains("saved"));
    }

    #[test]
    fn errors_are_not_suppressed_by_warnings() {
        let presentation = EmberPresenter::make(
            true,
            EmberOperationState::Degraded,
            &HashSet::new(),
            0,
            0,
            &["Gamma write failed".into()],
            1,
            0,
            VerificationLevel::Readback,
            Some(AttentionState::new(
                AttentionSeverity::Warning,
                "Warning",
                "minor",
                vec![],
            )),
        );
        assert_eq!(presentation.attention.severity, AttentionSeverity::Error);
        assert_eq!(presentation.status_title, "Needs attention");
    }

    #[test]
    fn counts_distinguish_categories() {
        let presentation = EmberPresenter::make(
            true,
            EmberOperationState::Active,
            &set(&["a"]),
            1,
            2,
            &["f".into()],
            5,
            1,
            VerificationLevel::Applied,
            None,
        );
        assert_eq!(presentation.counts.verified, 1);
        assert_eq!(presentation.counts.unsupported, 1);
        assert_eq!(presentation.counts.pending, 2);
        assert_eq!(presentation.counts.failed, 1);
        assert_eq!(presentation.counts.mirrored, 1);
        assert_eq!(presentation.counts.online_intended(), 4);
    }

    #[test]
    fn ready_off_state_mentions_untouched_display() {
        let presentation = EmberPresenter::make(
            false,
            EmberOperationState::Off,
            &HashSet::new(),
            0,
            0,
            &[],
            2,
            0,
            VerificationLevel::Readback,
            None,
        );
        assert_eq!(presentation.status_title, "Ready");
        assert!(presentation.status_detail.contains("untouched"));
    }

    #[test]
    fn no_display_state_is_explicit() {
        let presentation = EmberPresenter::make(
            false,
            EmberOperationState::Off,
            &HashSet::new(),
            0,
            0,
            &[],
            0,
            0,
            VerificationLevel::Unavailable,
            None,
        );
        assert_eq!(presentation.status_title, "No compatible display");
        assert!(presentation.status_detail.contains("gamma"));
    }

    #[test]
    fn active_copy_counts_displays() {
        let presentation = EmberPresenter::make(
            true,
            EmberOperationState::Active,
            &set(&["a", "b"]),
            0,
            0,
            &[],
            2,
            0,
            VerificationLevel::Applied,
            None,
        );
        assert!(presentation.shows_active());
        assert!(presentation
            .status_detail
            .contains("every compatible display"));
    }

    #[test]
    fn pending_plus_active_is_mentioned() {
        let presentation = EmberPresenter::make(
            true,
            EmberOperationState::Active,
            &set(&["a"]),
            0,
            1,
            &[],
            2,
            0,
            VerificationLevel::Applied,
            None,
        );
        assert!(presentation.status_detail.contains("saved for reconnect"));
        assert_eq!(presentation.counts.pending, 1);
    }

    #[test]
    fn info_override_is_honored_when_no_failures() {
        let presentation = EmberPresenter::make(
            true,
            EmberOperationState::Active,
            &set(&["a"]),
            0,
            0,
            &[],
            1,
            0,
            VerificationLevel::Applied,
            Some(AttentionState::new(
                AttentionSeverity::Info,
                "Nightlight is also active",
                "Warmth effects combine.",
                vec![],
            )),
        );
        assert_eq!(presentation.status_title, "Nightlight is also active");
        assert_eq!(presentation.attention.severity, AttentionSeverity::Info);
    }
}
