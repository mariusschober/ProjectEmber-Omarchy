//! Version truth for the Linux port.

/// Marketing version shared with the macOS 0.4.0 feature baseline.
pub const MARKETING: &str = "0.4.1";
/// Build number within the 0.4.x line. Linux-specific releases increment it.
pub const BUILD: u32 = 6;
/// Highest recovery-journal schema this build can understand.
pub const MAX_SUPPORTED_SCHEMA: u32 = 2;
/// Current recovery-journal schema written by this build.
pub const CURRENT_SCHEMA: u32 = 2;
/// Edition label used in the UI and diagnostics.
pub const EDITION: &str = "Omarchy";

pub struct AppVersion;

impl AppVersion {
    pub const MARKETING: &'static str = MARKETING;
    pub const BUILD: u32 = BUILD;
    pub const MAX_SUPPORTED_SCHEMA: u32 = MAX_SUPPORTED_SCHEMA;
    pub const EDITION: &'static str = EDITION;

    /// Full version, e.g. `0.4.1 (build 6, Omarchy)`.
    pub fn full() -> String {
        format!("{MARKETING} (build {BUILD}, {EDITION})")
    }

    /// Short version for the footer.
    pub fn short() -> String {
        format!("v{MARKETING} · Linux")
    }

    /// Artifact / package version.
    pub fn package() -> &'static str {
        MARKETING
    }

    /// Unix user-agent-ish identity for D-Bus properties.
    pub fn client_name() -> String {
        format!("ProjectEmber/{MARKETING}")
    }

    /// True when a journal schema can be read by this build.
    pub fn supports_schema(version: u32) -> bool {
        version <= MAX_SUPPORTED_SCHEMA
    }
}
