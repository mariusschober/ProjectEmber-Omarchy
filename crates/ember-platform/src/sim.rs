use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use ember_core::{
    BackendKind, DisplayBaseline, DisplayIdentity, EmberError, EmberResult, GammaSource,
    GammaTable, HardwareBaseline, VerificationLevel,
};

use crate::backend::{
    ApplyReport, BackendCapabilities, BackendEvent, DisplayBackend, DisplayInfo, EventSink,
    VerifyReport,
};
use crate::backlight::{BacklightCapability, BacklightControl};

/// In-memory display used by tests, CI, the scenario runner and visual QA.
#[derive(Debug, Clone)]
pub struct SimDisplay {
    pub connector: String,
    pub built_in: bool,
    pub gamma_size: u32,
    pub gamma: GammaTable,
    pub baseline: GammaTable,
    pub mirrored: bool,
    pub failed: bool,
}

impl SimDisplay {
    pub fn new(connector: &str, built_in: bool, gamma_size: u32) -> Self {
        let table = GammaTable::identity(gamma_size as usize);
        Self {
            connector: connector.into(),
            built_in,
            gamma_size,
            gamma: table.clone(),
            baseline: table,
            mirrored: false,
            failed: false,
        }
    }

    pub fn identity(&self) -> DisplayIdentity {
        DisplayIdentity::new(
            Some(format!("sim:{}", self.connector.to_ascii_lowercase())),
            if self.built_in { 0x0610 } else { 0x10AC },
            if self.built_in { 0xA032 } else { 0xA040 },
            if self.built_in { 1 } else { 2 },
            crate::edid::connector_unit(&self.connector),
            self.built_in,
            None,
            self.connector.clone(),
        )
    }
}

pub struct SimulatedBackend {
    displays: Vec<SimDisplay>,
    events: EventSink,
    verification: VerificationLevel,
    fail_next_apply: HashSet<String>,
    fail_restore: HashSet<String>,
    sequence: u64,
}

impl SimulatedBackend {
    pub fn new() -> Self {
        Self::with_displays(vec![
            SimDisplay::new("eDP-1", true, 4096),
            SimDisplay::new("DP-1", false, 4096),
        ])
    }

    pub fn single() -> Self {
        Self::with_displays(vec![SimDisplay::new("eDP-1", true, 4096)])
    }

    pub fn with_displays(displays: Vec<SimDisplay>) -> Self {
        Self {
            displays,
            events: EventSink::new(),
            verification: VerificationLevel::Readback,
            fail_next_apply: HashSet::new(),
            fail_restore: HashSet::new(),
            sequence: 0,
        }
    }

    /// Wayland-style backend: writes are accepted but cannot be read back.
    pub fn without_readback(mut self) -> Self {
        self.verification = VerificationLevel::Applied;
        self
    }

    pub fn set_verification(mut self, verification: VerificationLevel) -> Self {
        self.verification = verification;
        self
    }

    pub fn unplug(&mut self, connector: &str) {
        self.displays
            .retain(|display| display.connector != connector);
        self.sequence += 1;
        let _ = self.events.sender().send(BackendEvent::TopologyChanged);
    }

    pub fn plug(&mut self, display: SimDisplay) {
        if !self
            .displays
            .iter()
            .any(|existing| existing.connector == display.connector)
        {
            self.displays.push(display);
        }
        self.sequence += 1;
        let _ = self.events.sender().send(BackendEvent::TopologyChanged);
    }

    pub fn reset_to_identity(&mut self, connector: &str) {
        if let Some(display) = self
            .displays
            .iter_mut()
            .find(|display| display.connector == connector)
        {
            display.gamma = GammaTable::identity(display.gamma_size as usize);
        }
    }

    pub fn set_mirrored(&mut self, connector: &str, mirrored: bool) {
        if let Some(display) = self
            .displays
            .iter_mut()
            .find(|display| display.connector == connector)
        {
            display.mirrored = mirrored;
        }
    }

    pub fn fail_next_apply(&mut self, connector: &str) {
        self.fail_next_apply.insert(connector.to_string());
    }

    pub fn fail_restore(&mut self, connector: &str) {
        self.fail_restore.insert(connector.to_string());
    }

    pub fn table(&self, connector: &str) -> Option<GammaTable> {
        self.displays
            .iter()
            .find(|display| display.connector == connector)
            .map(|display| display.gamma.clone())
    }

    pub fn baseline(&self, connector: &str) -> Option<GammaTable> {
        self.displays
            .iter()
            .find(|display| display.connector == connector)
            .map(|display| display.baseline.clone())
    }

    pub fn restore_all(&mut self) {
        for display in &mut self.displays {
            display.gamma = display.baseline.clone();
        }
    }

    fn index(&self, identity: &DisplayIdentity) -> EmberResult<usize> {
        let matches: Vec<usize> = self
            .displays
            .iter()
            .enumerate()
            .filter(|(_, display)| display.identity().matches(identity))
            .map(|(index, _)| index)
            .collect();
        match matches.len() {
            0 => Err(EmberError::DisplayUnavailable),
            1 => Ok(matches[0]),
            _ => Err(EmberError::DisplayIdentityAmbiguous(
                "Multiple simulated displays match the identity.".into(),
            )),
        }
    }
}

impl Default for SimulatedBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayBackend for SimulatedBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            kind: BackendKind::Simulated,
            name: "simulator",
            verification: self.verification,
            readback: self.verification == VerificationLevel::Readback,
            exclusive: false,
            note: Some("Deterministic in-memory display backend.".into()),
        }
    }

    fn enumerate(&mut self) -> EmberResult<Vec<DisplayInfo>> {
        Ok(self
            .displays
            .iter()
            .map(|display| DisplayInfo {
                display_id: crate::edid::connector_unit(&display.connector),
                identity: display.identity(),
                connector: display.connector.clone(),
                gamma_size: display.gamma_size,
                is_online: true,
                is_active: true,
                is_mirrored: display.mirrored,
                built_in: display.built_in,
                edid: None,
                unsupported_reason: display
                    .failed
                    .then(|| "Simulated control failure (another client holds the output).".into()),
            })
            .collect())
    }

    fn capture_baseline(&mut self, identity: &DisplayIdentity) -> EmberResult<DisplayBaseline> {
        let index = self.index(identity)?;
        let display = &self.displays[index];
        Ok(DisplayBaseline::new(
            crate::edid::connector_unit(&display.connector),
            Some(display.identity()),
            display.baseline.clone(),
            chrono::Utc::now(),
        )
        .with_source(GammaSource::Simulated))
    }

    fn apply(
        &mut self,
        identity: &DisplayIdentity,
        table: &GammaTable,
    ) -> EmberResult<ApplyReport> {
        let index = self.index(identity)?;
        let connector = self.displays[index].connector.clone();
        if self.fail_next_apply.remove(&connector) {
            return Err(EmberError::GammaWriteFailed(-99));
        }
        if self.displays[index].failed {
            return Err(EmberError::GammaWriteFailed(-98));
        }
        self.displays[index].gamma = table.clone();
        if self.verification == VerificationLevel::Readback {
            let delta = self.displays[index]
                .gamma
                .maximum_absolute_difference(table);
            if delta >= 0.004 {
                return Err(EmberError::GammaVerificationFailed);
            }
            Ok(ApplyReport {
                accepted: true,
                verified: true,
            })
        } else {
            Ok(ApplyReport {
                accepted: true,
                verified: false,
            })
        }
    }

    fn verify(&mut self, identity: &DisplayIdentity, expected: &GammaTable) -> VerifyReport {
        match self.index(identity) {
            Ok(index) if self.displays[index].failed => {
                VerifyReport::Failed("simulated control failure".into())
            }
            Ok(index) => {
                if self.verification == VerificationLevel::Readback {
                    let delta = self.displays[index]
                        .gamma
                        .maximum_absolute_difference(expected);
                    if delta < 0.004 {
                        VerifyReport::Verified
                    } else {
                        VerifyReport::Mismatch {
                            delta_bits: (delta * 65535.0) as u32,
                        }
                    }
                } else {
                    VerifyReport::AcceptedUnverified
                }
            }
            Err(error) => VerifyReport::Failed(error.to_string()),
        }
    }

    fn restore(
        &mut self,
        identity: &DisplayIdentity,
        baseline: &DisplayBaseline,
    ) -> EmberResult<()> {
        let index = self.index(identity)?;
        let connector = self.displays[index].connector.clone();
        if self.fail_restore.remove(&connector) {
            return Err(EmberError::GammaWriteFailed(-97));
        }
        self.displays[index].gamma = baseline.gamma_table.clone();
        Ok(())
    }

    fn release_all(&mut self) -> EmberResult<()> {
        self.restore_all();
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<BackendEvent> {
        self.events.drain()
    }

    fn shutdown(&mut self) {}
}

/// In-memory backlight used by scenario tests and the GUI demo.
pub struct SimBacklight {
    brightness: Mutex<HashMap<String, f32>>,
    sensor_present: bool,
}

impl SimBacklight {
    pub fn new() -> Self {
        let mut brightness = HashMap::new();
        brightness.insert("eDP-1".to_string(), 0.6);
        Self {
            brightness: Mutex::new(brightness),
            sensor_present: false,
        }
    }

    pub fn with_sensor(mut self, present: bool) -> Self {
        self.sensor_present = present;
        self
    }

    pub fn set_brightness(&self, connector: &str, value: f32) {
        self.brightness
            .lock()
            .unwrap()
            .insert(connector.to_string(), value.clamp(0.0, 1.0));
    }

    pub fn brightness(&self, connector: &str) -> Option<f32> {
        self.brightness.lock().unwrap().get(connector).copied()
    }

    fn built_in(&self, connector: &str) -> bool {
        ember_core::DisplayIdentity::is_built_in_connector(connector)
    }
}

impl Default for SimBacklight {
    fn default() -> Self {
        Self::new()
    }
}

impl BacklightControl for SimBacklight {
    fn capability(&self, connector: &str) -> BacklightCapability {
        if !self.built_in(connector) {
            return BacklightCapability::unavailable(
                "Backlight Lock applies to the built-in display only.",
            );
        }
        BacklightCapability {
            available: true,
            device: Some("sim_backlight".into()),
            ambient_control: false,
            sensor_present: self.sensor_present,
            detail: None,
        }
    }

    fn capture(&self, connector: &str) -> EmberResult<HardwareBaseline> {
        if !self.built_in(connector) {
            return Err(EmberError::BacklightUnavailable);
        }
        let value = self.brightness(connector).unwrap_or(1.0);
        Ok(HardwareBaseline {
            brightness: Some(value),
            ambient_light_compensation_enabled: None,
            device: Some("sim_backlight".into()),
        })
    }

    fn engage(&self, connector: &str) -> EmberResult<()> {
        if !self.built_in(connector) {
            return Err(EmberError::BacklightUnavailable);
        }
        self.set_brightness(connector, 1.0);
        Ok(())
    }

    fn needs_engagement(&self, connector: &str) -> bool {
        self.brightness(connector).is_some_and(|value| value < 0.97)
    }

    fn restore(&self, connector: &str, baseline: &HardwareBaseline) -> EmberResult<()> {
        if !self.built_in(connector) {
            return Err(EmberError::BacklightUnavailable);
        }
        self.set_brightness(connector, baseline.brightness.unwrap_or(1.0));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ember_core::ColorCurve;

    #[test]
    fn apply_verify_restore_round_trip() {
        let mut backend = SimulatedBackend::single();
        let displays = backend.enumerate().unwrap();
        let identity = displays[0].identity.clone();
        let baseline = backend.capture_baseline(&identity).unwrap();
        let table = baseline
            .gamma_table
            .applying(ColorCurve::gains_for_warmth(0.7), 0.6);
        let report = backend.apply(&identity, &table).unwrap();
        assert!(report.accepted && report.verified);
        assert!(matches!(
            backend.verify(&identity, &table),
            VerifyReport::Verified
        ));
        backend.restore(&identity, &baseline).unwrap();
        assert!(matches!(
            backend.verify(&identity, &baseline.gamma_table),
            VerifyReport::Verified
        ));
    }

    #[test]
    fn os_reset_is_detectable() {
        let mut backend = SimulatedBackend::single();
        let identity = backend.enumerate().unwrap()[0].identity.clone();
        let baseline = backend.capture_baseline(&identity).unwrap();
        let table = baseline
            .gamma_table
            .applying(ColorCurve::gains_for_warmth(0.8), 0.5);
        backend.apply(&identity, &table).unwrap();
        backend.reset_to_identity("eDP-1");
        match backend.verify(&identity, &table) {
            VerifyReport::Mismatch { .. } => {}
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn failed_apply_is_injected_once() {
        let mut backend = SimulatedBackend::single();
        let identity = backend.enumerate().unwrap()[0].identity.clone();
        let table = GammaTable::identity(4096);
        backend.fail_next_apply("eDP-1");
        assert!(backend.apply(&identity, &table).is_err());
        assert!(backend.apply(&identity, &table).is_ok());
    }

    #[test]
    fn unplug_emits_topology_event_and_removes_display() {
        let mut backend = SimulatedBackend::new();
        backend.unplug("DP-1");
        let displays = backend.enumerate().unwrap();
        assert_eq!(displays.len(), 1);
        assert!(backend
            .drain_events()
            .iter()
            .any(|event| matches!(event, BackendEvent::TopologyChanged)));
    }

    #[test]
    fn applied_mode_cannot_read_back() {
        let mut backend = SimulatedBackend::single().without_readback();
        let identity = backend.enumerate().unwrap()[0].identity.clone();
        let baseline = backend.capture_baseline(&identity).unwrap();
        let table = baseline
            .gamma_table
            .applying(ColorCurve::gains_for_warmth(0.5), 0.8);
        let report = backend.apply(&identity, &table).unwrap();
        assert!(report.accepted);
        assert!(!report.verified);
        assert!(matches!(
            backend.verify(&identity, &table),
            VerifyReport::AcceptedUnverified
        ));
        // A reset is invisible to the applied-mode backend.
        backend.reset_to_identity("eDP-1");
        assert!(matches!(
            backend.verify(&identity, &table),
            VerifyReport::AcceptedUnverified
        ));
    }

    #[test]
    fn simulated_backlight_only_controls_built_in() {
        let backlight = SimBacklight::new();
        assert!(backlight.capability("eDP-1").available);
        assert!(!backlight.capability("DP-1").available);
        assert!(backlight.needs_engagement("eDP-1"));
        let baseline = backlight.capture("eDP-1").unwrap();
        backlight.engage("eDP-1").unwrap();
        assert!(!backlight.needs_engagement("eDP-1"));
        backlight.restore("eDP-1", &baseline).unwrap();
        assert!((backlight.brightness("eDP-1").unwrap() - 0.6).abs() < 0.001);
    }

    #[test]
    fn mirroring_marks_display_unsupported() {
        let mut backend = SimulatedBackend::new();
        backend.set_mirrored("DP-1", true);
        let displays = backend.enumerate().unwrap();
        let external = displays
            .iter()
            .find(|display| display.connector == "DP-1")
            .unwrap();
        assert!(external.is_mirrored);
        assert!(!external.gamma_compatible());
    }
}
