use std::sync::mpsc::{self, Receiver};

use ember_core::{
    BackendKind, DisplayBaseline, DisplayIdentity, DisplayTopologyEntry, EmberError, EmberResult,
    GammaSource, GammaTable, VerificationLevel,
};

use crate::drm::DrmSysfs;
use crate::edid::{connector_unit, edid_for_connector, EdidInfo};

/// A display as seen by a backend at one instant.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    /// Transient backend id (registry name, CRTC, simulator id).
    pub display_id: u32,
    pub identity: DisplayIdentity,
    pub connector: String,
    pub gamma_size: u32,
    pub is_online: bool,
    pub is_active: bool,
    pub is_mirrored: bool,
    pub built_in: bool,
    pub edid: Option<EdidInfo>,
    pub unsupported_reason: Option<String>,
}

impl DisplayInfo {
    pub fn gamma_compatible(&self) -> bool {
        self.is_online
            && self.is_active
            && !self.is_mirrored
            && self.gamma_size >= 2
            && self.unsupported_reason.is_none()
    }

    pub fn topology_entry(&self) -> DisplayTopologyEntry {
        DisplayTopologyEntry {
            identity: self.identity.clone(),
            display_id: self.display_id,
            is_online: self.is_online,
            is_active: self.is_active,
            is_mirrored: self.is_mirrored,
            gamma_capacity: self.gamma_size,
            is_built_in: self.built_in,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendCapabilities {
    pub kind: BackendKind,
    pub name: &'static str,
    pub verification: VerificationLevel,
    pub readback: bool,
    /// True when only one client may hold color control per output. This is
    /// what makes a competing color service detectable.
    pub exclusive: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    /// The backend accepted the write.
    pub accepted: bool,
    /// The backend could and did read the table back.
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyReport {
    Verified,
    Mismatch {
        delta_bits: u32,
    },
    /// Wayland: the control is alive and no protocol error was received, but
    /// there is no readback in the protocol.
    AcceptedUnverified,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendEvent {
    /// An output appeared, disappeared or changed. The daemon increments its
    /// topology generation and debounces.
    TopologyChanged,
    /// A gamma-control object became invalid (another client, output removed,
    /// compositor failure).
    ControlLost { connector: String, reason: String },
    /// The backend itself failed and needs a restart.
    Fatal(String),
}

/// Display color-control backend. Methods are synchronous and may block for a
/// few milliseconds; the daemon calls them from a blocking context.
pub trait DisplayBackend: Send {
    fn capabilities(&self) -> BackendCapabilities;

    fn enumerate(&mut self) -> EmberResult<Vec<DisplayInfo>>;

    fn capture_baseline(&mut self, identity: &DisplayIdentity) -> EmberResult<DisplayBaseline>;

    fn apply(&mut self, identity: &DisplayIdentity, table: &GammaTable)
        -> EmberResult<ApplyReport>;

    fn verify(&mut self, identity: &DisplayIdentity, expected: &GammaTable) -> VerifyReport;

    fn restore(
        &mut self,
        identity: &DisplayIdentity,
        baseline: &DisplayBaseline,
    ) -> EmberResult<()>;

    /// Emergency teardown: release every gamma control / restore every ramp.
    fn release_all(&mut self) -> EmberResult<()>;

    fn drain_events(&mut self) -> Vec<BackendEvent>;

    fn shutdown(&mut self);
}

/// Helper for backends that receive events on a channel.
pub struct EventSink {
    sender: mpsc::Sender<BackendEvent>,
    receiver: Receiver<BackendEvent>,
}

impl EventSink {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self { sender, receiver }
    }

    pub fn sender(&self) -> mpsc::Sender<BackendEvent> {
        self.sender.clone()
    }

    pub fn push(&self, event: BackendEvent) {
        let _ = self.sender.send(event);
    }

    pub fn drain(&self) -> Vec<BackendEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            events.push(event);
        }
        events
    }
}

impl Default for EventSink {
    fn default() -> Self {
        Self::new()
    }
}

/// Builds a stable identity from EDID and connector. Displays without EDID and
/// without a built-in connector are deliberately ambiguous so callers never
/// mutate a display they cannot identify.
pub fn identity_for_connector(
    sysfs: &DrmSysfs,
    connector: &str,
    display_id: u32,
) -> (DisplayIdentity, Option<EdidInfo>, Option<String>) {
    let built_in = DisplayIdentity::is_built_in_connector(connector);
    let unit = connector_unit(connector);
    match edid_for_connector(sysfs, connector) {
        Some(edid) => {
            let identity = identity_from_edid(connector, &edid, unit, built_in);
            (identity, Some(edid), None)
        }
        None if built_in => {
            // Internal panels have stable connector names; a laptop panel
            // without a readable EDID is still unambiguously the built-in one.
            let identity = DisplayIdentity::new(
                Some(format!("builtin:{}", connector.to_ascii_lowercase())),
                0,
                0,
                0,
                unit,
                true,
                None,
                connector.to_string(),
            );
            (identity, None, None)
        }
        None => {
            let identity = DisplayIdentity::new(
                None,
                0,
                0,
                0,
                display_id | (unit << 8),
                false,
                None,
                connector.to_string(),
            );
            (
                identity,
                None,
                Some(
                    "Display did not provide an EDID identity; left untouched to avoid \
                     restoring a different monitor's colors."
                        .into(),
                ),
            )
        }
    }
}

/// Builds an identity from parsed EDID. Serial numbers are preferred; when the
/// monitor does not expose one, a hash of the EDID bytes keeps identity stable
/// without pretending the display has a serial.
pub fn identity_from_edid(
    connector: &str,
    edid: &EdidInfo,
    unit: u32,
    built_in: bool,
) -> DisplayIdentity {
    let uuid = if edid.serial_number != 0 {
        format!(
            "edid:{:04x}:{:04x}:{}",
            edid.manufacturer_id, edid.product_code, edid.serial_number
        )
    } else {
        format!("edid-hash:{}", edid.sha256)
    };
    DisplayIdentity::new(
        Some(uuid),
        u32::from(edid.manufacturer_id),
        u32::from(edid.product_code),
        edid.serial_number,
        unit,
        built_in,
        None,
        connector.to_string(),
    )
}

/// Baseline representation for a backend without gamma readback. Wayland's
/// protocol has no getter; releasing the gamma control restores the compositor
/// default. The saved table is therefore an identity ramp, explicitly marked.
pub fn compositor_default_baseline(identity: &DisplayIdentity, gamma_size: u32) -> DisplayBaseline {
    DisplayBaseline::new(
        hash_display_id(identity),
        Some(identity.clone()),
        GammaTable::identity(gamma_size.clamp(2, 65_536) as usize),
        chrono::Utc::now(),
    )
    .with_source(GammaSource::CompositorDefault)
}

pub fn hash_display_id(identity: &DisplayIdentity) -> u32 {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(identity.key().as_bytes());
    let digest = hasher.finalize();
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

pub fn require_display<'a>(
    displays: &'a [DisplayInfo],
    identity: &DisplayIdentity,
) -> EmberResult<&'a DisplayInfo> {
    let matches: Vec<&DisplayInfo> = displays
        .iter()
        .filter(|info| info.identity.matches(identity))
        .collect();
    match matches.len() {
        0 => Err(EmberError::DisplayUnavailable),
        1 => Ok(matches[0]),
        _ => Err(EmberError::DisplayIdentityAmbiguous(format!(
            "{} online displays match the saved identity.",
            matches.len()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sysfs_with_edid(serial: u32) -> (tempfile::TempDir, DrmSysfs) {
        let dir = tempfile::tempdir().unwrap();
        let connector_dir = dir.path().join("card0-DP-1");
        std::fs::create_dir_all(&connector_dir).unwrap();
        let mut bytes = vec![0u8; 256];
        bytes[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        bytes[8] = 0x10;
        bytes[9] = 0xAC;
        bytes[10] = 0x40;
        bytes[11] = 0xA0;
        bytes[12..16].copy_from_slice(&serial.to_le_bytes());
        std::fs::write(connector_dir.join("edid"), bytes).unwrap();
        let sysfs = DrmSysfs::with_root(dir.path());
        (dir, sysfs)
    }

    #[test]
    fn edid_identity_is_stable_across_ids() {
        let (_dir, sysfs) = sysfs_with_edid(42);
        let (a, edid_a, reason_a) = identity_for_connector(&sysfs, "DP-1", 7);
        let (b, _, _) = identity_for_connector(&sysfs, "DP-1", 99);
        assert_eq!(a, b);
        assert!(reason_a.is_none());
        assert!(edid_a.is_some());
        assert_eq!(a.serial_number, 42);
        assert!(!a.is_ambiguous_without_uuid());
    }

    #[test]
    fn built_in_without_edid_gets_stable_connector_identity() {
        let dir = tempfile::tempdir().unwrap();
        let sysfs = DrmSysfs::with_root(dir.path());
        let (identity, edid, reason) = identity_for_connector(&sysfs, "eDP-1", 1);
        assert!(identity.is_built_in);
        assert!(identity.uuid.is_some());
        assert!(edid.is_none());
        assert!(reason.is_none());
        assert!(!identity.is_ambiguous_without_uuid());
    }

    #[test]
    fn external_without_edid_is_ambiguous_and_not_mutable() {
        let dir = tempfile::tempdir().unwrap();
        let sysfs = DrmSysfs::with_root(dir.path());
        let (identity, _, reason) = identity_for_connector(&sysfs, "DP-9", 9);
        assert!(!identity.is_built_in);
        assert!(identity.is_ambiguous_without_uuid());
        assert!(reason.is_some());
    }

    #[test]
    fn duplicate_online_displays_are_ambiguous() {
        let (_dir, sysfs) = sysfs_with_edid(42);
        let (identity, _, _) = identity_for_connector(&sysfs, "DP-1", 1);
        let info = DisplayInfo {
            display_id: 1,
            identity: identity.clone(),
            connector: "DP-1".into(),
            gamma_size: 256,
            is_online: true,
            is_active: true,
            is_mirrored: false,
            built_in: false,
            edid: None,
            unsupported_reason: None,
        };
        let displays = vec![info.clone(), info];
        assert!(matches!(
            require_display(&displays, &identity),
            Err(EmberError::DisplayIdentityAmbiguous(_))
        ));
    }

    #[test]
    fn compositor_default_baseline_is_marked() {
        let identity =
            DisplayIdentity::new(Some("x".into()), 1, 2, 3, 0, false, None, "DP-1".into());
        let baseline = compositor_default_baseline(&identity, 4096);
        assert_eq!(baseline.gamma_source, GammaSource::CompositorDefault);
        assert_eq!(baseline.gamma_table.sample_count(), 4096);
        assert!(baseline.gamma_table.is_valid());
    }
}
