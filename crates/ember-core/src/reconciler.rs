use std::collections::{HashMap, HashSet};

use crate::identity::DisplayIdentity;
use crate::models::DisplayRecoveryEntry;
use crate::topology::{DisplayTopologyEntry, DisplayTopologySnapshot};

/// Per-identity reconciliation decision. An unchanged display is never
/// blanket-restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconciliationAction {
    /// Leave untouched; the transform is already installed (readback on X11) or
    /// accepted by the compositor (Wayland).
    LeaveVerified { identity: DisplayIdentity },
    /// Reapply the transform derived from the saved baseline (OS reset).
    ReapplyFromBaseline { identity: DisplayIdentity },
    /// New display: journal the baseline first, then apply and verify.
    JournalThenApply { identity: DisplayIdentity },
    /// Reconnected pending display: apply from the canonical saved baseline,
    /// never recapture (avoids an app-induced neutral flash).
    ApplyPendingBaseline { identity: DisplayIdentity },
    /// Restore the saved baseline, then remove the entry only after verified
    /// restoration.
    RestoreSavedBaseline { identity: DisplayIdentity },
    /// Display offline: retain the journal entry unchanged as pending.
    RetainPending { identity: DisplayIdentity },
    /// Leave untouched; count accurately in presentation.
    LeaveUnsupported {
        identity: DisplayIdentity,
        reason: String,
    },
    /// Ambiguous identity: no mutation allowed.
    LeaveAmbiguous { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationPlan {
    pub generation: u64,
    pub actions: Vec<ReconciliationAction>,
    pub diagnostic_warning: Option<String>,
}

impl ReconciliationPlan {
    pub fn new(generation: u64, actions: Vec<ReconciliationAction>) -> Self {
        Self {
            generation,
            actions,
            diagnostic_warning: None,
        }
    }

    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.diagnostic_warning = Some(warning.into());
        self
    }

    pub fn contains<F>(&self, predicate: F) -> bool
    where
        F: Fn(&ReconciliationAction) -> bool,
    {
        self.actions.iter().any(predicate)
    }
}

/// Pure, testable reconciliation planner.
pub struct DisplayReconciler;

impl DisplayReconciler {
    /// - `snapshot`: settled topology snapshot.
    /// - `journaled`: current recovery entries (canonical baselines).
    /// - `controlled`: identities Ember currently considers controlled.
    /// - `desired_on`: whether Ember is desired on.
    /// - `ambiguity_keys`: identity keys matching more than one online display.
    pub fn plan(
        snapshot: &DisplayTopologySnapshot,
        journaled: &[DisplayRecoveryEntry],
        controlled: &HashSet<DisplayIdentity>,
        desired_on: bool,
        ambiguity_keys: &HashSet<String>,
    ) -> ReconciliationPlan {
        let controlled_keys: HashSet<String> = controlled
            .iter()
            .map(DisplayTopologySnapshot::key)
            .collect();
        let mut journal_by_key: HashMap<String, Vec<&DisplayRecoveryEntry>> = HashMap::new();
        for entry in journaled {
            journal_by_key
                .entry(DisplayTopologySnapshot::key(&entry.identity))
                .or_default()
                .push(entry);
        }
        let online_by_key: HashMap<String, &DisplayTopologyEntry> = snapshot
            .entries
            .iter()
            .filter(|entry| entry.is_online)
            .map(|entry| (DisplayTopologySnapshot::key(&entry.identity), entry))
            .collect();

        let mut actions = Vec::new();

        // Ambiguous identities: explicit no-mutation action, never first-match.
        let mut sorted_ambiguity_keys: Vec<&String> = ambiguity_keys.iter().collect();
        sorted_ambiguity_keys.sort();
        for key in &sorted_ambiguity_keys {
            actions.push(ReconciliationAction::LeaveAmbiguous {
                reason: format!("Identity {key} matches multiple online displays; left untouched."),
            });
        }

        if !desired_on {
            // Desired off: restore every online journaled display; retain
            // offline entries as pending.
            for entry in journaled {
                let key = DisplayTopologySnapshot::key(&entry.identity);
                if ambiguity_keys.contains(key.as_str()) {
                    continue;
                }
                if online_by_key.contains_key(&key) {
                    actions.push(ReconciliationAction::RestoreSavedBaseline {
                        identity: entry.identity.clone(),
                    });
                } else {
                    actions.push(ReconciliationAction::RetainPending {
                        identity: entry.identity.clone(),
                    });
                }
            }
            return ReconciliationPlan::new(snapshot.generation, actions);
        }

        // Desired ON.
        for entry in snapshot.entries.iter().filter(|entry| entry.is_online) {
            let key = DisplayTopologySnapshot::key(&entry.identity);
            if ambiguity_keys.contains(&key) {
                continue;
            }
            let journal_entries = journal_by_key.get(&key).cloned().unwrap_or_default();
            if journal_entries.len() > 1 {
                actions.push(ReconciliationAction::LeaveAmbiguous {
                    reason: format!(
                        "Multiple journal entries for {key}; manual resolution required."
                    ),
                });
                continue;
            }
            if !entry.is_gamma_compatible() {
                actions.push(ReconciliationAction::LeaveUnsupported {
                    identity: entry.identity.clone(),
                    reason: if entry.is_mirrored {
                        "Display is mirrored; gamma control is left to the primary output.".into()
                    } else {
                        "Display does not expose a writable gamma table.".into()
                    },
                });
                continue;
            }
            if let Some(journaled) = journal_entries.first() {
                if controlled_keys.contains(&key) {
                    // Existing controlled display still online: verify, never
                    // blanket-restore. The coordinator maps a successful
                    // readback to "leave verified"; otherwise it reapplies.
                    actions.push(ReconciliationAction::ReapplyFromBaseline {
                        identity: journaled.identity.clone(),
                    });
                } else {
                    actions.push(ReconciliationAction::ApplyPendingBaseline {
                        identity: journaled.identity.clone(),
                    });
                }
            } else {
                actions.push(ReconciliationAction::JournalThenApply {
                    identity: entry.identity.clone(),
                });
            }
        }

        // Journaled but now offline → retain pending. Never clear and never
        // turn off remaining displays.
        for entry in journaled {
            let key = DisplayTopologySnapshot::key(&entry.identity);
            if ambiguity_keys.contains(&key) {
                continue;
            }
            if !online_by_key.contains_key(&key) {
                actions.push(ReconciliationAction::RetainPending {
                    identity: entry.identity.clone(),
                });
            }
        }

        ReconciliationPlan::new(snapshot.generation, actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamma::GammaTable;
    use crate::models::{DisplayBaseline, EmberSettings};
    use chrono::Utc;

    fn identity(uuid: &str, serial: u32, unit: u32) -> DisplayIdentity {
        DisplayIdentity::new(
            Some(uuid.into()),
            100,
            200,
            serial,
            unit,
            false,
            None,
            "DP-1".into(),
        )
    }

    fn entry(uuid: &str, serial: u32, unit: u32) -> DisplayRecoveryEntry {
        let id = identity(uuid, serial, unit);
        DisplayRecoveryEntry::without_hardware(
            id.clone(),
            DisplayBaseline::new(unit, Some(id), GammaTable::identity(8), Utc::now()),
        )
    }

    fn snapshot(identities: Vec<DisplayIdentity>) -> DisplayTopologySnapshot {
        DisplayTopologySnapshot::new(
            1,
            Utc::now(),
            identities
                .into_iter()
                .enumerate()
                .map(|(index, id)| DisplayTopologyEntry::new(id, index as u32 + 1))
                .collect(),
        )
    }

    #[test]
    fn unplug_makes_zero_restore_calls_to_survivor() {
        let a = identity("A", 1, 1);
        let b = identity("B", 2, 2);
        let entry_a = entry("A", 1, 1);
        let entry_b = entry("B", 2, 2);
        let plan = DisplayReconciler::plan(
            &snapshot(vec![a.clone()]),
            &[entry_a.clone(), entry_b.clone()],
            &HashSet::from([a.clone(), b.clone()]),
            true,
            &HashSet::new(),
        );
        assert!(!plan.contains(|action| matches!(
            action,
            ReconciliationAction::RestoreSavedBaseline { .. }
        )));
        assert!(plan.contains(|action| matches!(
            action,
            ReconciliationAction::ReapplyFromBaseline { identity } if identity == &a
        )));
        assert!(plan.contains(|action| matches!(
            action,
            ReconciliationAction::RetainPending { identity } if identity == &b
        )));
    }

    #[test]
    fn disconnect_retains_entry() {
        let a = identity("A", 1, 1);
        let entry_a = entry("A", 1, 1);
        let plan = DisplayReconciler::plan(
            &snapshot(vec![]),
            std::slice::from_ref(&entry_a),
            &HashSet::from([a.clone()]),
            true,
            &HashSet::new(),
        );
        assert!(plan.contains(|action| matches!(
            action,
            ReconciliationAction::RetainPending { identity } if identity == &a
        )));
    }

    #[test]
    fn new_display_journals_before_apply() {
        let fresh = identity("NEW", 99, 9);
        let plan = DisplayReconciler::plan(
            &snapshot(vec![fresh.clone()]),
            &[],
            &HashSet::new(),
            true,
            &HashSet::new(),
        );
        assert!(plan.contains(|action| matches!(
            action,
            ReconciliationAction::JournalThenApply { identity } if identity == &fresh
        )));
    }

    #[test]
    fn reconnect_uses_saved_baseline() {
        let a = identity("A", 1, 1);
        let entry_a = entry("A", 1, 1);
        let plan = DisplayReconciler::plan(
            &snapshot(vec![a.clone()]),
            std::slice::from_ref(&entry_a),
            &HashSet::new(),
            true,
            &HashSet::new(),
        );
        assert!(plan.contains(|action| matches!(
            action,
            ReconciliationAction::ApplyPendingBaseline { .. }
        )));
        assert!(!plan
            .contains(|action| matches!(action, ReconciliationAction::JournalThenApply { .. })));
    }

    #[test]
    fn stale_generation_cannot_win() {
        let a = identity("A", 1, 1);
        let plan = DisplayReconciler::plan(
            &snapshot(vec![a]),
            &[],
            &HashSet::new(),
            true,
            &HashSet::new(),
        );
        assert_eq!(plan.generation, 1);
        assert_ne!(plan.generation, 2);
    }

    #[test]
    fn ambiguous_identities_cause_no_mutation() {
        let a = DisplayIdentity::new(None, 100, 200, 0, 1, false, None, "DP-1".into());
        let key = DisplayTopologySnapshot::key(&a);
        let plan = DisplayReconciler::plan(
            &snapshot(vec![a]),
            &[],
            &HashSet::new(),
            true,
            &HashSet::from([key]),
        );
        assert!(
            plan.contains(|action| matches!(action, ReconciliationAction::LeaveAmbiguous { .. }))
        );
        assert!(!plan
            .contains(|action| matches!(action, ReconciliationAction::JournalThenApply { .. })));
    }

    #[test]
    fn desired_off_restores_online_and_keeps_offline_pending() {
        let a = identity("A", 1, 1);
        let b = identity("B", 2, 2);
        let plan = DisplayReconciler::plan(
            &snapshot(vec![a.clone()]),
            &[entry("A", 1, 1), entry("B", 2, 2)],
            &HashSet::new(),
            false,
            &HashSet::new(),
        );
        assert!(plan.contains(|action| matches!(
            action,
            ReconciliationAction::RestoreSavedBaseline { identity } if identity == &a
        )));
        assert!(plan.contains(|action| matches!(
            action,
            ReconciliationAction::RetainPending { identity } if identity == &b
        )));
    }

    #[test]
    fn duplicate_journal_entries_are_ambiguous() {
        let a = identity("A", 1, 1);
        let plan = DisplayReconciler::plan(
            &snapshot(vec![a.clone()]),
            &[entry("A", 1, 1), entry("A", 1, 1)],
            &HashSet::new(),
            true,
            &HashSet::new(),
        );
        assert!(
            plan.contains(|action| matches!(action, ReconciliationAction::LeaveAmbiguous { .. }))
        );
    }

    #[test]
    fn mirrored_or_incapable_display_is_unsupported() {
        let a = identity("A", 1, 1);
        let mut topo = snapshot(vec![a]);
        topo.entries[0].is_mirrored = true;
        let plan = DisplayReconciler::plan(&topo, &[], &HashSet::new(), true, &HashSet::new());
        assert!(
            plan.contains(|action| matches!(action, ReconciliationAction::LeaveUnsupported { .. }))
        );
    }

    #[test]
    fn empty_topology_with_no_journal_is_a_noop() {
        let plan = DisplayReconciler::plan(
            &snapshot(vec![]),
            &[],
            &HashSet::new(),
            true,
            &HashSet::new(),
        );
        assert!(plan.actions.is_empty());
    }

    #[test]
    fn settings_do_not_influence_plan_shape() {
        // Ensures the planner stays pure (no settings side channel).
        let _settings = EmberSettings::default();
        let a = identity("A", 1, 1);
        let plan = DisplayReconciler::plan(
            &snapshot(vec![a]),
            &[],
            &HashSet::new(),
            true,
            &HashSet::new(),
        );
        assert_eq!(plan.actions.len(), 1);
    }
}
