use ember_core::{
    BackendKind, DisplayBaseline, DisplayIdentity, EmberError, EmberResult, GammaTable,
    VerificationLevel,
};

use crate::backend::{
    ApplyReport, BackendCapabilities, BackendEvent, DisplayBackend, DisplayInfo, VerifyReport,
};

/// A backend for sessions where no display server is reachable. It never
/// mutates anything and lets the daemon/UI report the truth instead of failing
/// to start.
pub struct NullBackend {
    reason: String,
}

impl NullBackend {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl DisplayBackend for NullBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            kind: BackendKind::None,
            name: "none",
            verification: VerificationLevel::Unavailable,
            readback: false,
            exclusive: false,
            note: Some(self.reason.clone()),
        }
    }

    fn enumerate(&mut self) -> EmberResult<Vec<DisplayInfo>> {
        Ok(Vec::new())
    }

    fn capture_baseline(&mut self, _identity: &DisplayIdentity) -> EmberResult<DisplayBaseline> {
        Err(EmberError::NoCompatibleDisplay)
    }

    fn apply(
        &mut self,
        _identity: &DisplayIdentity,
        _table: &GammaTable,
    ) -> EmberResult<ApplyReport> {
        Err(EmberError::NoCompatibleDisplay)
    }

    fn verify(&mut self, _identity: &DisplayIdentity, _expected: &GammaTable) -> VerifyReport {
        VerifyReport::Failed(self.reason.clone())
    }

    fn restore(
        &mut self,
        _identity: &DisplayIdentity,
        _baseline: &DisplayBaseline,
    ) -> EmberResult<()> {
        Ok(())
    }

    fn release_all(&mut self) -> EmberResult<()> {
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<BackendEvent> {
        Vec::new()
    }

    fn shutdown(&mut self) {}
}
