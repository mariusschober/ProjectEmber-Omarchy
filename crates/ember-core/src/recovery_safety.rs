use crate::identity::DisplayIdentity;
use crate::models::DisplayRecoveryEntry;

/// Result of restoring one display during a disable/sleep/termination/reset.
/// Unsuccessful outcomes always keep their journal entry.
#[derive(Debug, Clone, PartialEq)]
pub enum RestoreOutcome {
    RestoredAndVerified(DisplayIdentity),
    DisplayOfflinePending(DisplayIdentity),
    IdentityAmbiguous(String),
    GammaWriteFailed {
        identity: DisplayIdentity,
        message: String,
    },
    GammaReadbackMismatch {
        identity: DisplayIdentity,
        delta: f32,
    },
    HardwareRestoreFailed {
        identity: DisplayIdentity,
        message: String,
    },
    HardwareReadbackMismatch {
        identity: DisplayIdentity,
        message: String,
    },
    ResolveFailed(String),
}

impl RestoreOutcome {
    pub fn restored_and_verified(identity: DisplayIdentity) -> Self {
        Self::RestoredAndVerified(identity)
    }

    pub fn display_offline_pending(identity: DisplayIdentity) -> Self {
        Self::DisplayOfflinePending(identity)
    }

    pub fn identity_ambiguous(reason: String) -> Self {
        Self::IdentityAmbiguous(reason)
    }

    pub fn gamma_write_failed(identity: DisplayIdentity, message: String) -> Self {
        Self::GammaWriteFailed { identity, message }
    }

    pub fn gamma_readback_mismatch(identity: DisplayIdentity, delta: f32) -> Self {
        Self::GammaReadbackMismatch { identity, delta }
    }

    pub fn hardware_restore_failed(identity: DisplayIdentity, message: String) -> Self {
        Self::HardwareRestoreFailed { identity, message }
    }

    pub fn hardware_readback_mismatch(identity: DisplayIdentity, message: String) -> Self {
        Self::HardwareReadbackMismatch { identity, message }
    }

    pub fn resolve_failed(message: String) -> Self {
        Self::ResolveFailed(message)
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::RestoredAndVerified(_))
    }

    pub fn is_pending_offline(&self) -> bool {
        matches!(self, Self::DisplayOfflinePending(_))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RestoreReport {
    pub outcomes: Vec<RestoreOutcome>,
    /// Entries that must remain journaled (every non-verified outcome with an
    /// identity).
    pub remaining: Vec<DisplayRecoveryEntry>,
}

impl RestoreReport {
    pub fn new(outcomes: Vec<RestoreOutcome>, remaining: Vec<DisplayRecoveryEntry>) -> Self {
        Self {
            outcomes,
            remaining,
        }
    }

    pub fn verified_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.is_success())
            .count()
    }

    pub fn has_failure(&self) -> bool {
        self.outcomes
            .iter()
            .any(|outcome| !outcome.is_success() && !outcome.is_pending_offline())
    }
}
