use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use crate::models::{AutomationOverride, EmberPreset, TrayPrimaryAction};

/// Tracks external resets per display. Three resets within 60 seconds enter a
/// truthful degraded state instead of fighting another color service forever.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverrideTracker {
    history: HashMap<String, Vec<DateTime<Utc>>>,
}

impl OverrideTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a detected reset and returns true when the bounded threshold is
    /// crossed.
    pub fn record_reset(&mut self, key: &str, at: DateTime<Utc>) -> bool {
        let list = self.history.entry(key.to_string()).or_default();
        list.push(at);
        list.retain(|timestamp| at.signed_duration_since(*timestamp) < Duration::seconds(60));
        list.len() >= 3
    }

    pub fn count(&self, key: &str, at: DateTime<Utc>) -> usize {
        self.history
            .get(key)
            .map(|list| {
                list.iter()
                    .filter(|timestamp| {
                        at.signed_duration_since(**timestamp) < Duration::seconds(60)
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    pub fn reset(&mut self) {
        self.history.clear();
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.history.keys()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingResult {
    OpenControls,
    ToggleOn,
    ToggleOff,
    IgnoredBusy,
}

/// Tray click routing (pure; the daemon enforces busy coalescing).
pub struct TrayRouting;

impl TrayRouting {
    pub fn route(
        action: TrayPrimaryAction,
        is_busy: bool,
        is_secondary_click: bool,
        is_observed_active: bool,
        filter_enabled: bool,
    ) -> RoutingResult {
        if is_busy {
            return RoutingResult::IgnoredBusy;
        }
        if is_secondary_click {
            return RoutingResult::OpenControls;
        }
        match action {
            TrayPrimaryAction::OpenControls => RoutingResult::OpenControls,
            TrayPrimaryAction::ToggleEmber => {
                let turning_on = !filter_enabled || !is_observed_active;
                if turning_on {
                    RoutingResult::ToggleOn
                } else {
                    RoutingResult::ToggleOff
                }
            }
        }
    }
}

/// Backlight drift check: write only below threshold, never unconditionally.
pub struct BacklightDriftCheck;

impl BacklightDriftCheck {
    pub const THRESHOLD: f32 = 0.97;

    pub fn needs_write(
        current_brightness: Option<f32>,
        ambient_enabled: Option<bool>,
        threshold: f32,
    ) -> bool {
        let Some(brightness) = current_brightness else {
            return false;
        };
        if brightness < threshold {
            return true;
        }
        ambient_enabled == Some(true)
    }

    pub fn may_engage(is_built_in: bool) -> bool {
        is_built_in
    }
}

/// Rejects stale cached location fixes.
pub struct LocationFreshness;

impl LocationFreshness {
    pub const MAX_AGE: Duration = Duration::seconds(5 * 60);

    pub fn is_fresh(location_timestamp: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        let age = now.signed_duration_since(location_timestamp);
        age >= Duration::seconds(-60) && age <= Self::MAX_AGE
    }
}

/// Preset index for warmth (custom warmth returns None so the UI clears all
/// highlights).
pub struct PresetMatching;

impl PresetMatching {
    pub fn index_for_warmth(warmth: f64) -> Option<usize> {
        EmberPreset::ALL
            .iter()
            .position(|preset| (warmth - preset.warmth()).abs() < 0.01)
    }
}

/// v1 built-in journal records must never resolve onto an unrelated external
/// display whose transient id was reused.
pub struct LegacyResolver;

impl LegacyResolver {
    pub fn may_resolve_to_candidate(
        legacy_is_built_in: bool,
        exact_id_match: bool,
        candidate_is_built_in: bool,
    ) -> bool {
        if exact_id_match {
            return candidate_is_built_in;
        }
        legacy_is_built_in && candidate_is_built_in
    }
}

/// Applies a manual override that expires at the next solar boundary.
pub fn make_override(
    filter_enabled: bool,
    next_boundary: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<AutomationOverride> {
    let expires_at = next_boundary?;
    if expires_at <= now {
        return None;
    }
    Some(AutomationOverride::new(filter_enabled, expires_at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn three_resets_within_minute_crosses_threshold() {
        let now = Utc::now();
        let mut tracker = OverrideTracker::new();
        assert!(!tracker.record_reset("k", now));
        assert!(!tracker.record_reset("k", now + Duration::seconds(10)));
        assert!(tracker.record_reset("k", now + Duration::seconds(20)));
        assert_eq!(tracker.count("k", now + Duration::seconds(20)), 3);
    }

    #[test]
    fn old_resets_expire() {
        let now = Utc::now();
        let mut tracker = OverrideTracker::new();
        assert!(!tracker.record_reset("k", now));
        assert!(!tracker.record_reset("k", now + Duration::seconds(61)));
        assert_eq!(tracker.count("k", now + Duration::seconds(61)), 1);
    }

    #[test]
    fn tray_routing_matches_macos() {
        use RoutingResult::*;
        assert_eq!(
            TrayRouting::route(TrayPrimaryAction::OpenControls, false, false, false, false),
            OpenControls
        );
        assert_eq!(
            TrayRouting::route(TrayPrimaryAction::ToggleEmber, false, false, false, false),
            ToggleOn
        );
        assert_eq!(
            TrayRouting::route(TrayPrimaryAction::ToggleEmber, false, false, true, true),
            ToggleOff
        );
        assert_eq!(
            TrayRouting::route(TrayPrimaryAction::ToggleEmber, true, false, false, false),
            IgnoredBusy
        );
        assert_eq!(
            TrayRouting::route(TrayPrimaryAction::ToggleEmber, false, true, true, true),
            OpenControls
        );
    }

    #[test]
    fn backlight_drift_only_writes_when_needed() {
        assert!(!BacklightDriftCheck::needs_write(
            Some(1.0),
            Some(false),
            0.97
        ));
        assert!(!BacklightDriftCheck::needs_write(
            Some(0.98),
            Some(false),
            0.97
        ));
        assert!(BacklightDriftCheck::needs_write(
            Some(0.5),
            Some(false),
            0.97
        ));
        assert!(BacklightDriftCheck::needs_write(
            Some(1.0),
            Some(true),
            0.97
        ));
        assert!(!BacklightDriftCheck::needs_write(None, Some(true), 0.97));
        assert!(!BacklightDriftCheck::may_engage(false));
        assert!(BacklightDriftCheck::may_engage(true));
    }

    #[test]
    fn location_freshness() {
        let now = Utc::now();
        assert!(!LocationFreshness::is_fresh(
            now - Duration::minutes(10),
            now
        ));
        assert!(LocationFreshness::is_fresh(
            now - Duration::seconds(60),
            now
        ));
        assert!(LocationFreshness::is_fresh(
            now + Duration::seconds(30),
            now
        ));
    }

    #[test]
    fn preset_index() {
        assert_eq!(PresetMatching::index_for_warmth(0.33), None);
        assert_eq!(PresetMatching::index_for_warmth(0.0), Some(0));
        assert_eq!(PresetMatching::index_for_warmth(0.62), Some(1));
        assert_eq!(PresetMatching::index_for_warmth(1.0), Some(2));
    }

    #[test]
    fn legacy_resolver_never_targets_external() {
        assert!(!LegacyResolver::may_resolve_to_candidate(
            true, false, false
        ));
        assert!(LegacyResolver::may_resolve_to_candidate(true, false, true));
        assert!(!LegacyResolver::may_resolve_to_candidate(
            false, false, true
        ));
        assert!(LegacyResolver::may_resolve_to_candidate(false, true, true));
    }

    #[test]
    fn override_expires_at_boundary() {
        let now = Utc.with_ymd_and_hms(2026, 11, 8, 20, 0, 0).unwrap();
        let boundary = now + Duration::hours(10);
        let created = make_override(true, Some(boundary), now).unwrap();
        assert!(created.is_active_at(now));
        assert!(!created.is_active_at(boundary));
        assert!(make_override(true, Some(now), now).is_none());
        assert!(make_override(true, None, now).is_none());
    }
}
