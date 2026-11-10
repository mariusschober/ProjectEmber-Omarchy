use thiserror::Error;

pub type EmberResult<T> = Result<T, EmberError>;

/// The core error surface. Messages are intentionally user-facing and never
/// include raw EDID bytes, serial numbers, or command output containing secrets.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EmberError {
    #[error("No built-in display is currently available.")]
    NoBuiltInDisplay,

    #[error("No connected display accepts Project Ember's color controls.")]
    NoCompatibleDisplay,

    #[error("The saved display is not currently connected.")]
    DisplayUnavailable,

    #[error("The connected display does not match the saved recovery baseline.")]
    DisplayIdentityMismatch,

    #[error("Display identity is ambiguous; left untouched. {0}")]
    DisplayIdentityAmbiguous(String),

    #[error("The current display color table could not be read (code {0}).")]
    GammaReadFailed(i32),

    #[error("The display color table could not be updated (code {0}).")]
    GammaWriteFailed(i32),

    #[error("The display did not preserve the requested color table.")]
    GammaVerificationFailed,

    #[error("The saved display color table is invalid.")]
    InvalidGammaTable,

    #[error("Hardware brightness control is unavailable on the connected displays.")]
    BacklightUnavailable,

    #[error("The hardware brightness could not be read (code {0}).")]
    BacklightReadFailed(i32),

    #[error("The hardware brightness could not be changed (code {0}).")]
    BacklightWriteFailed(i32),

    #[error("Automatic brightness state could not be read (code {0}).")]
    AmbientLightReadFailed(i32),

    #[error("Automatic brightness state could not be changed (code {0}).")]
    AmbientLightWriteFailed(i32),

    #[error("Display recovery failed: {0}")]
    RecoveryFailed(String),

    #[error("Recovery journal is unreadable and was preserved for inspection: {0}")]
    JournalCorrupt(String),

    #[error("Recovery journal uses unsupported schema v{0}.")]
    JournalUnsupportedSchema(u32),

    #[error("Another display service is repeatedly replacing Ember's color table. {0}")]
    ExternalOverrideDetected(String),

    #[error("No supported display session was found: {0}")]
    BackendUnavailable(String),

    #[error("Display backend failed: {0}")]
    BackendFailure(String),

    #[error("Input is invalid: {0}")]
    InvalidInput(String),

    #[error("File operation failed: {0}")]
    Io(String),

    #[error("Another Project Ember instance is already running.")]
    AlreadyRunning,
}

impl From<std::io::Error> for EmberError {
    fn from(value: std::io::Error) -> Self {
        EmberError::Io(value.to_string())
    }
}
