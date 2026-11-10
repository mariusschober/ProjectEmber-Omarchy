/// What a restore is for. Determines the terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreIntent {
    Disable,
    Sleep,
    Terminate,
    Recovery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayRuntimeState {
    Off,
    Activating,
    Active,
    Reconciling,
    Restoring(RestoreIntent),
    Suspended,
    Degraded(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayEvent {
    EnableRequested,
    ActivationSucceeded,
    ActivationFailed(String),
    DisableRequested,
    SleepRequested,
    WakeRequested,
    TerminationRequested,
    RestoreSucceeded,
    RestoreFailed(String),
    SettingsChanged,
    DisplayChanged,
    ReconciliationStarted,
    ReconciliationSucceeded,
    ReconciliationFailed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayAction {
    CaptureBaseline,
    SaveRecoveryRecord,
    ApplyTransform,
    RestoreBaseline,
    ClearRecoveryRecord,
    StartBacklightGuard,
    StopBacklightGuard,
    SynchronizeBacklight,
    VerifyObservedState,
    PublishPresentation,
}

/// State machine ported from macOS EmberCore. The coordinator executes the
/// returned actions; transitions never perform I/O themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayStateMachine {
    state: DisplayRuntimeState,
}

impl Default for DisplayStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayStateMachine {
    pub fn new() -> Self {
        Self {
            state: DisplayRuntimeState::Off,
        }
    }

    pub fn with_state(state: DisplayRuntimeState) -> Self {
        Self { state }
    }

    pub fn state(&self) -> &DisplayRuntimeState {
        &self.state
    }

    pub fn set_state(&mut self, state: DisplayRuntimeState) {
        self.state = state;
    }

    pub fn transition(&mut self, event: DisplayEvent) -> Vec<DisplayAction> {
        use DisplayAction::*;
        use DisplayEvent as E;
        use DisplayRuntimeState as S;
        match (&self.state, event) {
            (S::Off, E::EnableRequested) | (S::Degraded(_), E::EnableRequested) => {
                self.state = S::Activating;
                vec![CaptureBaseline, SaveRecoveryRecord, ApplyTransform]
            }
            (S::Activating, E::DisplayChanged) => {
                self.state = S::Reconciling;
                vec![VerifyObservedState]
            }
            (S::Restoring(_), E::DisplayChanged) => vec![VerifyObservedState],
            (S::Activating, E::ActivationSucceeded) => {
                self.state = S::Active;
                vec![StartBacklightGuard, PublishPresentation]
            }
            (S::Activating, E::ActivationFailed(message)) => {
                self.state = S::Degraded(message);
                vec![StopBacklightGuard, RestoreBaseline, PublishPresentation]
            }
            (S::Active, E::DisableRequested)
            | (S::Activating, E::DisableRequested)
            | (S::Degraded(_), E::DisableRequested)
            | (S::Reconciling, E::DisableRequested)
            | (S::Suspended, E::DisableRequested) => {
                self.state = S::Restoring(RestoreIntent::Disable);
                vec![StopBacklightGuard, RestoreBaseline]
            }
            (S::Active, E::SleepRequested)
            | (S::Degraded(_), E::SleepRequested)
            | (S::Reconciling, E::SleepRequested) => {
                self.state = S::Restoring(RestoreIntent::Sleep);
                vec![StopBacklightGuard, RestoreBaseline]
            }
            (S::Suspended, E::SleepRequested) => vec![],
            (S::Active, E::TerminationRequested)
            | (S::Activating, E::TerminationRequested)
            | (S::Degraded(_), E::TerminationRequested)
            | (S::Reconciling, E::TerminationRequested)
            | (S::Suspended, E::TerminationRequested) => {
                self.state = S::Restoring(RestoreIntent::Terminate);
                vec![StopBacklightGuard, RestoreBaseline]
            }
            (S::Restoring(intent), E::RestoreSucceeded) => {
                self.state = if *intent == RestoreIntent::Sleep {
                    S::Suspended
                } else {
                    S::Off
                };
                vec![ClearRecoveryRecord, PublishPresentation]
            }
            (S::Restoring(_), E::RestoreFailed(message)) => {
                self.state = S::Degraded(message);
                vec![PublishPresentation]
            }
            (S::Active, E::SettingsChanged) => vec![ApplyTransform, SynchronizeBacklight],
            (S::Active, E::DisplayChanged) => {
                self.state = S::Reconciling;
                vec![VerifyObservedState]
            }
            (S::Reconciling, E::ReconciliationSucceeded) => {
                self.state = S::Active;
                vec![PublishPresentation]
            }
            (S::Reconciling, E::ReconciliationFailed(message)) => {
                self.state = S::Degraded(message);
                vec![PublishPresentation]
            }
            (S::Reconciling, E::SettingsChanged) => vec![ApplyTransform],
            (S::Degraded(_), E::DisplayChanged)
            | (S::Off, E::DisplayChanged)
            | (S::Suspended, E::DisplayChanged) => vec![VerifyObservedState],
            (S::Suspended, E::WakeRequested) => {
                self.state = S::Activating;
                vec![CaptureBaseline, SaveRecoveryRecord, ApplyTransform]
            }
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_journals_before_apply() {
        let mut machine = DisplayStateMachine::new();
        assert_eq!(
            machine.transition(DisplayEvent::EnableRequested),
            vec![
                DisplayAction::CaptureBaseline,
                DisplayAction::SaveRecoveryRecord,
                DisplayAction::ApplyTransform
            ]
        );
        assert_eq!(machine.state(), &DisplayRuntimeState::Activating);
    }

    #[test]
    fn activation_then_disable_restores_before_clear() {
        let mut machine = DisplayStateMachine::new();
        machine.transition(DisplayEvent::EnableRequested);
        let actions = machine.transition(DisplayEvent::ActivationSucceeded);
        assert!(actions.contains(&DisplayAction::StartBacklightGuard));
        assert_eq!(machine.state(), &DisplayRuntimeState::Active);
        assert_eq!(
            machine.transition(DisplayEvent::DisableRequested),
            vec![
                DisplayAction::StopBacklightGuard,
                DisplayAction::RestoreBaseline
            ]
        );
        assert_eq!(
            machine.state(),
            &DisplayRuntimeState::Restoring(RestoreIntent::Disable)
        );
        let restored = machine.transition(DisplayEvent::RestoreSucceeded);
        assert!(restored.contains(&DisplayAction::ClearRecoveryRecord));
        assert_eq!(machine.state(), &DisplayRuntimeState::Off);
    }

    #[test]
    fn degraded_sleep_still_restores_and_reaches_suspended() {
        let mut machine =
            DisplayStateMachine::with_state(DisplayRuntimeState::Degraded("boom".into()));
        let actions = machine.transition(DisplayEvent::SleepRequested);
        assert!(actions.contains(&DisplayAction::RestoreBaseline));
        assert_eq!(
            machine.state(),
            &DisplayRuntimeState::Restoring(RestoreIntent::Sleep)
        );
        machine.transition(DisplayEvent::RestoreSucceeded);
        assert_eq!(machine.state(), &DisplayRuntimeState::Suspended);
    }

    #[test]
    fn topology_during_activation_moves_to_reconciling() {
        let mut machine = DisplayStateMachine::with_state(DisplayRuntimeState::Activating);
        assert_eq!(
            machine.transition(DisplayEvent::DisplayChanged),
            vec![DisplayAction::VerifyObservedState]
        );
        assert_eq!(machine.state(), &DisplayRuntimeState::Reconciling);
        machine.transition(DisplayEvent::ReconciliationSucceeded);
        assert_eq!(machine.state(), &DisplayRuntimeState::Active);
    }

    #[test]
    fn topology_during_restore_keeps_restoring() {
        let mut machine =
            DisplayStateMachine::with_state(DisplayRuntimeState::Restoring(RestoreIntent::Disable));
        assert_eq!(
            machine.transition(DisplayEvent::DisplayChanged),
            vec![DisplayAction::VerifyObservedState]
        );
        assert_eq!(
            machine.state(),
            &DisplayRuntimeState::Restoring(RestoreIntent::Disable)
        );
    }

    #[test]
    fn exhaustive_lifecycle() {
        let mut machine = DisplayStateMachine::new();
        machine.transition(DisplayEvent::EnableRequested);
        machine.transition(DisplayEvent::DisplayChanged);
        machine.transition(DisplayEvent::ReconciliationSucceeded);
        machine.transition(DisplayEvent::SleepRequested);
        machine.transition(DisplayEvent::RestoreSucceeded);
        assert_eq!(machine.state(), &DisplayRuntimeState::Suspended);
        machine.transition(DisplayEvent::WakeRequested);
        assert_eq!(machine.state(), &DisplayRuntimeState::Activating);
        machine.transition(DisplayEvent::ActivationSucceeded);
        machine.transition(DisplayEvent::TerminationRequested);
        machine.transition(DisplayEvent::RestoreSucceeded);
        assert_eq!(machine.state(), &DisplayRuntimeState::Off);
    }

    #[test]
    fn restore_failure_is_degraded_not_off() {
        let mut machine = DisplayStateMachine::new();
        machine.transition(DisplayEvent::EnableRequested);
        machine.transition(DisplayEvent::ActivationSucceeded);
        machine.transition(DisplayEvent::DisableRequested);
        machine.transition(DisplayEvent::RestoreFailed("nope".into()));
        assert_eq!(
            machine.state(),
            &DisplayRuntimeState::Degraded("nope".into())
        );
    }

    #[test]
    fn settings_change_only_reapplies_when_active() {
        let mut machine = DisplayStateMachine::new();
        assert!(machine.transition(DisplayEvent::SettingsChanged).is_empty());
        machine.transition(DisplayEvent::EnableRequested);
        machine.transition(DisplayEvent::ActivationSucceeded);
        assert_eq!(
            machine.transition(DisplayEvent::SettingsChanged),
            vec![
                DisplayAction::ApplyTransform,
                DisplayAction::SynchronizeBacklight
            ]
        );
    }

    #[test]
    fn duplicate_sleep_is_idempotent() {
        let mut machine = DisplayStateMachine::with_state(DisplayRuntimeState::Suspended);
        assert!(machine.transition(DisplayEvent::SleepRequested).is_empty());
    }
}
