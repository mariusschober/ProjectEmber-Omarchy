//! Project Ember — platform-independent core.
//!
//! This crate is a deliberate port of the macOS `EmberCore` module plus the
//! Linux-specific model additions needed by the Omarchy build. It contains no
//! I/O beyond filesystem state (settings/journal) and no display access. Every
//! safety invariant lives here where it can be tested deterministically.
//!
//! Invariants (also asserted in `docs/SYSTEM_DESIGN.md` and the test suite):
//!  1. Journal before mutation.
//!  2. Verified restore before deletion.
//!  3. Immutable original baseline: transforms always derive from the saved
//!     baseline; slider changes never compound.
//!  4. No blanket restore on topology change.
//!  5. Observed truth drives the UI.
//!  6. Disconnected is pending, not forgotten.
//!  7. Ambiguity means no mutation.
//!  8. Backlight Lock is built-in-only and reversible.
//!  9. A stale generation cannot win.
//! 10. No silent fallback: capability and verification limits are surfaced.

pub mod app_version;
pub mod color_curve;
pub mod diagnostics;
pub mod error;
pub mod fsutil;
pub mod gamma;
pub mod identity;
pub mod journal;
pub mod models;
pub mod paths;
pub mod policy;
pub mod presentation;
pub mod reconciler;
pub mod recovery_safety;
pub mod settings;
pub mod snapshot;
pub mod solar;
pub mod state_machine;
pub mod topology;

pub use app_version::AppVersion;
pub use color_curve::ColorCurve;
pub use diagnostics::DiagnosticLog;
pub use error::{EmberError, EmberResult};
pub use gamma::GammaTable;
pub use identity::DisplayIdentity;
pub use journal::{JournalLoadOutcome, RecoveryJournal};
pub use models::{
    AutomationOverride, ColorGains, DisplayBaseline, DisplayRecoveryEntry, EmberPreset,
    EmberSettings, GammaSource, HardwareBaseline, RecoveryRecord, TrayPrimaryAction,
};
pub use paths::Paths;
pub use policy::{
    BacklightDriftCheck, LegacyResolver, LocationFreshness, OverrideTracker, PresetMatching,
    TrayRouting,
};
pub use presentation::{
    AttentionAction, AttentionSeverity, AttentionState, BackendKind, DisplayCountSummary,
    EmberOperationState, EmberPresentation, EmberPresenter, VerificationLevel,
};
pub use reconciler::{DisplayReconciler, ReconciliationAction, ReconciliationPlan};
pub use recovery_safety::{RestoreOutcome, RestoreReport};
pub use settings::{SettingsLoad, SettingsStore};
pub use snapshot::{
    BackendSnapshot, BacklightSnapshot, BacklightState, DisplaySnapshot, DisplayState,
    LocationSnapshot, LocationSource, LocationState, NightlightSnapshot, Snapshot,
    SolarRuntimeSnapshot, IPC_SCHEMA_VERSION,
};
pub use solar::{
    CachedSolarLocation, SolarCalculator, SolarCoordinate, SolarDayCondition, SolarEvent,
    SolarEventKind, SolarSchedule,
};
pub use state_machine::{
    DisplayAction, DisplayEvent, DisplayRuntimeState, DisplayStateMachine, RestoreIntent,
};
pub use topology::{
    DisplayReconfigurationEvent, DisplayTopologyEntry, DisplayTopologySnapshot,
    DisplayVerificationResult,
};
