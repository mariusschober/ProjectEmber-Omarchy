use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::identity::DisplayIdentity;
use crate::models::epoch_seconds;

/// Stable per-display topology entry. Identity — not a transient display id —
/// is the reconciliation key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayTopologyEntry {
    pub identity: DisplayIdentity,
    pub display_id: u32,
    pub is_online: bool,
    pub is_active: bool,
    pub is_mirrored: bool,
    pub gamma_capacity: u32,
    pub is_built_in: bool,
}

impl DisplayTopologyEntry {
    pub fn new(identity: DisplayIdentity, display_id: u32) -> Self {
        Self {
            is_built_in: identity.is_built_in,
            identity,
            display_id,
            is_online: true,
            is_active: true,
            is_mirrored: false,
            gamma_capacity: 256,
        }
    }

    pub fn with_state(
        mut self,
        online: bool,
        active: bool,
        mirrored: bool,
        gamma_capacity: u32,
    ) -> Self {
        self.is_online = online;
        self.is_active = active;
        self.is_mirrored = mirrored;
        self.gamma_capacity = gamma_capacity;
        self
    }

    pub fn is_gamma_compatible(&self) -> bool {
        self.gamma_capacity >= 2 && self.is_online && self.is_active && !self.is_mirrored
    }

    pub fn is_backlight_candidate(&self) -> bool {
        self.is_built_in && self.is_gamma_compatible()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayTopologySnapshot {
    pub generation: u64,
    #[serde(with = "epoch_seconds")]
    pub captured_at: DateTime<Utc>,
    pub entries: Vec<DisplayTopologyEntry>,
}

impl DisplayTopologySnapshot {
    pub fn new(
        generation: u64,
        captured_at: DateTime<Utc>,
        entries: Vec<DisplayTopologyEntry>,
    ) -> Self {
        Self {
            generation,
            captured_at,
            entries,
        }
    }

    pub fn identity_keys(&self) -> HashSet<String> {
        self.entries
            .iter()
            .map(|entry| Self::key(&entry.identity))
            .collect()
    }

    pub fn key(identity: &DisplayIdentity) -> String {
        identity.key()
    }

    /// Two snapshots match when they expose the same identity set with the same
    /// compatibility/built-in/mirror flags. Transient display ids are ignored.
    pub fn matches_topology(&self, other: &DisplayTopologySnapshot) -> bool {
        if self.entries.len() != other.entries.len() {
            return false;
        }
        let mut a: Vec<&DisplayTopologyEntry> = self.entries.iter().collect();
        let mut b: Vec<&DisplayTopologyEntry> = other.entries.iter().collect();
        a.sort_by_key(|entry| Self::key(&entry.identity));
        b.sort_by_key(|entry| Self::key(&entry.identity));
        for (x, y) in a.into_iter().zip(b) {
            if Self::key(&x.identity) != Self::key(&y.identity)
                || x.is_online != y.is_online
                || x.is_active != y.is_active
                || x.is_mirrored != y.is_mirrored
                || x.is_gamma_compatible() != y.is_gamma_compatible()
                || x.is_built_in != y.is_built_in
            {
                return false;
            }
        }
        true
    }

    pub fn entry_for(&self, identity: &DisplayIdentity) -> Option<&DisplayTopologyEntry> {
        self.entries
            .iter()
            .find(|entry| entry.identity.matches(identity))
    }

    pub fn online_keys(&self) -> HashSet<String> {
        self.entries
            .iter()
            .filter(|entry| entry.is_online)
            .map(|entry| Self::key(&entry.identity))
            .collect()
    }

    /// Identity keys that occur more than once in this snapshot. Such displays
    /// are never mutated.
    pub fn duplicate_keys(&self) -> HashSet<String> {
        let mut counts = std::collections::HashMap::<String, usize>::new();
        for entry in &self.entries {
            *counts.entry(Self::key(&entry.identity)).or_default() += 1;
        }
        counts
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(key, _)| key)
            .collect()
    }
}

impl Default for DisplayTopologySnapshot {
    fn default() -> Self {
        Self {
            generation: 0,
            captured_at: Utc::now(),
            entries: Vec::new(),
        }
    }
}

/// Display-reconfiguration event with all flags preserved. Linux backends use
/// the same bit names as CoreGraphics where they map cleanly and define new
/// bits for compositor-specific events; raw flags are never discarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayReconfigurationEvent {
    pub display_id: u32,
    pub flags: u32,
    pub is_begin_transaction: bool,
    pub generation: u64,
    #[serde(with = "epoch_seconds")]
    pub timestamp: DateTime<Utc>,
}

impl DisplayReconfigurationEvent {
    pub const FLAG_BEGIN: u32 = 0x0000_0001;
    pub const FLAG_MOVED: u32 = 0x0000_0010;
    pub const FLAG_ENABLED: u32 = 0x0000_0020;
    pub const FLAG_DISABLED: u32 = 0x0000_0040;
    pub const FLAG_MIRROR: u32 = 0x0000_0080;
    pub const FLAG_UNMIRROR: u32 = 0x0000_0100;
    pub const FLAG_DESKTOP_SHAPE_CHANGED: u32 = 0x0000_0200;
    pub const FLAG_ADD: u32 = 0x0000_0400;
    pub const FLAG_REMOVE: u32 = 0x0000_0800;
    pub const FLAG_MAIN_CHANGED: u32 = 0x0001_0000;
    pub const FLAG_MIRROR_CHANGED: u32 = 0x0002_0000;

    pub fn new(
        display_id: u32,
        flags: u32,
        is_begin_transaction: bool,
        generation: u64,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            display_id,
            flags,
            is_begin_transaction,
            generation,
            timestamp,
        }
    }

    pub fn flag_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.flags & Self::FLAG_BEGIN != 0 {
            names.push("BeginConfiguration");
        }
        if self.flags & Self::FLAG_MOVED != 0 {
            names.push("Moved");
        }
        if self.flags & Self::FLAG_ENABLED != 0 {
            names.push("Enabled");
        }
        if self.flags & Self::FLAG_DISABLED != 0 {
            names.push("Disabled");
        }
        if self.flags & Self::FLAG_MIRROR != 0 {
            names.push("Mirror");
        }
        if self.flags & Self::FLAG_UNMIRROR != 0 {
            names.push("UnMirror");
        }
        if self.flags & Self::FLAG_DESKTOP_SHAPE_CHANGED != 0 {
            names.push("DesktopShapeChanged");
        }
        if self.flags & Self::FLAG_ADD != 0 {
            names.push("Add");
        }
        if self.flags & Self::FLAG_REMOVE != 0 {
            names.push("Remove");
        }
        if self.flags & Self::FLAG_MAIN_CHANGED != 0 {
            names.push("MainDisplayChanged");
        }
        if self.flags & Self::FLAG_MIRROR_CHANGED != 0 {
            names.push("MirrorChanged");
        }
        names
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayVerificationResult {
    pub identity_key: String,
    pub outcome: VerificationOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerificationOutcome {
    Verified,
    ReappliedAndVerified,
    ResetDetectedReapplyFailed(String),
    ApplyFailed(String),
    RestoreFailed(String),
    PendingOffline,
    Unsupported(String),
    Ambiguous(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::DisplayIdentity;
    use chrono::Utc;

    fn identity(uuid: &str, serial: u32) -> DisplayIdentity {
        DisplayIdentity::new(
            Some(uuid.into()),
            100,
            200,
            serial,
            1,
            false,
            None,
            "DP-1".into(),
        )
    }

    #[test]
    fn transient_display_ids_are_ignored() {
        let a = identity("A", 1);
        let s1 = DisplayTopologySnapshot::new(
            1,
            Utc::now(),
            vec![DisplayTopologyEntry::new(a.clone(), 5)],
        );
        let s2 = DisplayTopologySnapshot::new(2, Utc::now(), vec![DisplayTopologyEntry::new(a, 9)]);
        assert!(s1.matches_topology(&s2));
    }

    #[test]
    fn added_display_breaks_match() {
        let a = identity("A", 1);
        let b = identity("B", 2);
        let s1 = DisplayTopologySnapshot::new(
            1,
            Utc::now(),
            vec![DisplayTopologyEntry::new(a.clone(), 1)],
        );
        let s2 = DisplayTopologySnapshot::new(
            2,
            Utc::now(),
            vec![
                DisplayTopologyEntry::new(a, 1),
                DisplayTopologyEntry::new(b, 2),
            ],
        );
        assert!(!s1.matches_topology(&s2));
    }

    #[test]
    fn mirrored_display_is_not_gamma_compatible() {
        let entry =
            DisplayTopologyEntry::new(identity("A", 1), 1).with_state(true, true, true, 256);
        assert!(!entry.is_gamma_compatible());
        assert!(!entry.is_backlight_candidate());
    }

    #[test]
    fn backend_capability_requires_two_samples() {
        let entry = DisplayTopologyEntry::new(identity("A", 1), 1).with_state(true, true, false, 1);
        assert!(!entry.is_gamma_compatible());
        let entry = entry.with_state(true, true, false, 2);
        assert!(entry.is_gamma_compatible());
    }

    #[test]
    fn duplicate_keys_detected() {
        let a = DisplayIdentity::new(None, 1, 2, 0, 0, false, None, "DP-1".into());
        let snapshot = DisplayTopologySnapshot::new(
            1,
            Utc::now(),
            vec![
                DisplayTopologyEntry::new(a.clone(), 1),
                DisplayTopologyEntry::new(a, 2),
            ],
        );
        assert_eq!(snapshot.duplicate_keys().len(), 1);
    }

    #[test]
    fn event_flags_decode() {
        let event = DisplayReconfigurationEvent::new(
            1,
            DisplayReconfigurationEvent::FLAG_ADD | DisplayReconfigurationEvent::FLAG_ENABLED,
            false,
            3,
            Utc::now(),
        );
        let names = event.flag_names();
        assert!(names.contains(&"Add"));
        assert!(names.contains(&"Enabled"));
        assert_eq!(event.generation, 3);
    }

    #[test]
    fn built_in_entry_is_backlight_candidate() {
        let id = DisplayIdentity::new(Some("B".into()), 1, 2, 3, 0, true, None, "eDP-1".into());
        let entry = DisplayTopologyEntry::new(id, 1);
        assert!(entry.is_backlight_candidate());
    }
}
