use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ember_core::{
    AppVersion, CachedSolarLocation, DisplayBaseline, DisplayIdentity, DisplayRecoveryEntry,
    EmberSettings, GammaTable, Paths, RecoveryJournal, RecoveryRecord, SettingsStore, Snapshot,
    SolarCoordinate,
};
use ember_platform::{
    BacklightCapability, BacklightControl, DisplayBackend, DisplayInfo, SimBacklight, SimDisplay,
    SimulatedBackend,
};
use serde::{Deserialize, Serialize};

use crate::coordinator::{DaemonBuilder, DaemonCommand, DaemonConfig, DaemonHandle};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioReport {
    pub scenario: String,
    pub passed: bool,
    pub steps: Vec<String>,
}

/// A shared simulated backend so scenario steps can mutate hardware while the
/// coordinator owns the trait object.
#[derive(Clone)]
pub struct SharedSim(Arc<Mutex<SimulatedBackend>>);

impl SharedSim {
    pub fn new(backend: SimulatedBackend) -> Self {
        Self(Arc::new(Mutex::new(backend)))
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut SimulatedBackend) -> R) -> R {
        let mut guard = self.0.lock().unwrap();
        f(&mut guard)
    }
}

impl DisplayBackend for SharedSim {
    fn capabilities(&self) -> ember_platform::BackendCapabilities {
        self.0.lock().unwrap().capabilities()
    }
    fn enumerate(&mut self) -> ember_core::EmberResult<Vec<DisplayInfo>> {
        self.0.lock().unwrap().enumerate()
    }
    fn capture_baseline(
        &mut self,
        identity: &DisplayIdentity,
    ) -> ember_core::EmberResult<DisplayBaseline> {
        self.0.lock().unwrap().capture_baseline(identity)
    }
    fn apply(
        &mut self,
        identity: &DisplayIdentity,
        table: &GammaTable,
    ) -> ember_core::EmberResult<ember_platform::ApplyReport> {
        self.0.lock().unwrap().apply(identity, table)
    }
    fn verify(
        &mut self,
        identity: &DisplayIdentity,
        expected: &GammaTable,
    ) -> ember_platform::VerifyReport {
        self.0.lock().unwrap().verify(identity, expected)
    }
    fn restore(
        &mut self,
        identity: &DisplayIdentity,
        baseline: &DisplayBaseline,
    ) -> ember_core::EmberResult<()> {
        self.0.lock().unwrap().restore(identity, baseline)
    }
    fn release_all(&mut self) -> ember_core::EmberResult<()> {
        self.0.lock().unwrap().release_all()
    }
    fn drain_events(&mut self) -> Vec<ember_platform::BackendEvent> {
        self.0.lock().unwrap().drain_events()
    }
    fn shutdown(&mut self) {
        self.0.lock().unwrap().shutdown()
    }
}

#[derive(Clone)]
pub struct SharedBacklight(Arc<SimBacklight>);

impl SharedBacklight {
    pub fn new(backlight: SimBacklight) -> Self {
        Self(Arc::new(backlight))
    }

    pub fn brightness(&self, connector: &str) -> Option<f32> {
        self.0.brightness(connector)
    }
}

impl BacklightControl for SharedBacklight {
    fn capability(&self, connector: &str) -> BacklightCapability {
        self.0.capability(connector)
    }
    fn capture(&self, connector: &str) -> ember_core::EmberResult<ember_core::HardwareBaseline> {
        self.0.capture(connector)
    }
    fn engage(&self, connector: &str) -> ember_core::EmberResult<()> {
        self.0.engage(connector)
    }
    fn needs_engagement(&self, connector: &str) -> bool {
        self.0.needs_engagement(connector)
    }
    fn restore(
        &self,
        connector: &str,
        baseline: &ember_core::HardwareBaseline,
    ) -> ember_core::EmberResult<()> {
        self.0.restore(connector, baseline)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioStep {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub sleep_ms: Option<u64>,
    #[serde(default)]
    pub expect: Option<Expect>,
    #[serde(default)]
    pub sim: Option<String>,
    #[serde(default)]
    pub connector: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Expect {
    #[serde(default)]
    pub observed_active: Option<bool>,
    #[serde(default)]
    pub filter_enabled: Option<bool>,
    #[serde(default)]
    pub runtime_state: Option<String>,
    #[serde(default)]
    pub pending: Option<usize>,
    #[serde(default)]
    pub verified: Option<usize>,
    #[serde(default)]
    pub backlight_state: Option<String>,
    #[serde(default)]
    pub solar_enabled: Option<bool>,
    #[serde(default)]
    pub location_state: Option<String>,
}

fn temp_paths(tag: &str) -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join(tag);
    let paths = Paths::for_test(root.join("config"), root.join("state"), root.join("run"));
    let _ = ember_core::fsutil::ensure_private_dir(paths.config_dir());
    let _ = ember_core::fsutil::ensure_private_dir(paths.state_dir());
    let _ = ember_core::fsutil::ensure_private_dir(paths.runtime_dir());
    (dir, paths)
}

fn start(paths: &Paths, sim: SharedSim, backlight: SharedBacklight) -> DaemonHandle {
    DaemonBuilder::new(DaemonConfig {
        paths: paths.clone(),
        headless: true,
        simulate: true,
        safe_mode: false,
        start_active: false,
    })
    .with_backend(Box::new(sim))
    .with_backlight(Box::new(backlight))
    .with_tray(false)
    .spawn()
    .expect("daemon spawn")
}

fn expect_snapshot(snapshot: &Snapshot, expect: &Expect) -> Result<(), String> {
    if let Some(value) = expect.observed_active {
        if snapshot.observed_active != value {
            return Err(format!(
                "observedActive expected {value}, got {}",
                snapshot.observed_active
            ));
        }
    }
    if let Some(value) = expect.filter_enabled {
        if snapshot.settings.filter_enabled != value {
            return Err(format!(
                "filterEnabled expected {value}, got {}",
                snapshot.settings.filter_enabled
            ));
        }
    }
    if let Some(value) = &expect.runtime_state {
        if &snapshot.runtime_label != value && &format!("{:?}", snapshot.runtime_state) != value {
            return Err(format!(
                "runtime expected {value}, got {}",
                snapshot.runtime_label
            ));
        }
    }
    if let Some(value) = expect.pending {
        if snapshot.pending_restore_count != value {
            return Err(format!(
                "pending expected {value}, got {}",
                snapshot.pending_restore_count
            ));
        }
    }
    if let Some(value) = expect.verified {
        if snapshot.counts.verified != value {
            return Err(format!(
                "verified expected {value}, got {}",
                snapshot.counts.verified
            ));
        }
    }
    if let Some(value) = &expect.backlight_state {
        if snapshot.backlight.state.label() != value {
            return Err(format!(
                "backlight expected {value}, got {}",
                snapshot.backlight.state.label()
            ));
        }
    }
    if let Some(value) = expect.solar_enabled {
        if snapshot.solar.enabled != value {
            return Err(format!(
                "solarEnabled expected {value}, got {}",
                snapshot.solar.enabled
            ));
        }
    }
    if let Some(value) = &expect.location_state {
        let actual = format!("{:?}", snapshot.solar.location_state);
        if &actual != value {
            return Err(format!("location state expected {value}, got {actual}"));
        }
    }
    Ok(())
}

fn wait_expect(handle: &DaemonHandle, expect: &Expect) -> Result<(), String> {
    match handle.wait_for_snapshot(Duration::from_secs(5), |snapshot| {
        expect_snapshot(snapshot, expect).is_ok()
    }) {
        Some(_) => Ok(()),
        None => Err(expect_snapshot(&handle.snapshot(), expect)
            .err()
            .unwrap_or_else(|| "timeout".into())),
    }
}

/// Built-in deterministic scenarios. Run with `emberd --simulate <name>`.
pub fn run(name: &str) -> anyhow::Result<ScenarioReport> {
    match name {
        "basic" => basic(),
        "hotplug" => hotplug(),
        "crash-recovery" => crash_recovery(),
        "corrupt-journal" => corrupt_journal(),
        "polar-solar" => polar_solar(),
        "backlight" => backlight(),
        "external-reset" => external_reset(),
        "all" => run_all(),
        other => Err(anyhow::anyhow!(
            "unknown scenario '{other}'; try basic, hotplug, crash-recovery, corrupt-journal, polar-solar, backlight, external-reset or all"
        )),
    }
}

fn run_all() -> anyhow::Result<ScenarioReport> {
    let mut steps = Vec::new();
    let mut passed = true;
    for scenario in [
        "basic",
        "hotplug",
        "crash-recovery",
        "corrupt-journal",
        "polar-solar",
        "backlight",
        "external-reset",
    ] {
        let report = run(scenario)?;
        steps.push(format!(
            "{scenario}: {}",
            if report.passed { "pass" } else { "FAIL" }
        ));
        for step in report.steps {
            steps.push(format!("  {step}"));
        }
        passed &= report.passed;
    }
    Ok(ScenarioReport {
        scenario: "all".into(),
        passed,
        steps,
    })
}

fn basic() -> anyhow::Result<ScenarioReport> {
    let (_dir, paths) = temp_paths("basic");
    let sim = SharedSim::new(SimulatedBackend::new());
    let backlight = SharedBacklight::new(SimBacklight::new());
    let mut handle = start(&paths, sim.clone(), backlight);
    let mut steps = Vec::new();
    let mut passed = true;

    handle.send(DaemonCommand::SetEnabled(true));
    match wait_expect(
        &handle,
        &Expect {
            observed_active: Some(true),
            verified: Some(2),
            ..Default::default()
        },
    ) {
        Ok(()) => steps.push("activated on 2 displays".into()),
        Err(error) => {
            steps.push(format!("activation: {error}"));
            passed = false;
        }
    }

    handle.send(DaemonCommand::SetEnabled(false));
    match wait_expect(
        &handle,
        &Expect {
            observed_active: Some(false),
            filter_enabled: Some(false),
            pending: Some(0),
            ..Default::default()
        },
    ) {
        Ok(()) => steps.push("disabled cleanly".into()),
        Err(error) => {
            steps.push(format!("disable: {error}"));
            passed = false;
        }
    }

    let baseline = sim.with(|backend| backend.baseline("eDP-1")).unwrap();
    let current = sim.with(|backend| backend.table("eDP-1")).unwrap();
    let delta = current.maximum_absolute_difference(&baseline);
    if delta > 0.0001 {
        steps.push(format!(
            "display table did not return to baseline (delta {delta})"
        ));
        passed = false;
    } else {
        steps.push("baseline restored exactly".into());
    }
    if paths.journal_file().exists() {
        steps.push("journal was not cleared after verified restore".into());
        passed = false;
    } else {
        steps.push("journal cleared after verified restore".into());
    }
    handle.shutdown();
    Ok(ScenarioReport {
        scenario: "basic".into(),
        passed,
        steps,
    })
}

fn hotplug() -> anyhow::Result<ScenarioReport> {
    let (_dir, paths) = temp_paths("hotplug");
    let sim = SharedSim::new(SimulatedBackend::new());
    let backlight = SharedBacklight::new(SimBacklight::new());
    let mut handle = start(&paths, sim.clone(), backlight);
    let mut steps = Vec::new();
    let mut passed = true;

    handle.send(DaemonCommand::SetEnabled(true));
    if let Err(error) = wait_expect(
        &handle,
        &Expect {
            observed_active: Some(true),
            verified: Some(2),
            ..Default::default()
        },
    ) {
        steps.push(format!("activation: {error}"));
        handle.shutdown();
        return Ok(ScenarioReport {
            scenario: "hotplug".into(),
            passed: false,
            steps,
        });
    }
    steps.push("activated 2 displays".into());

    sim.with(|backend| backend.unplug("DP-1"));
    match wait_expect(
        &handle,
        &Expect {
            pending: Some(1),
            verified: Some(1),
            ..Default::default()
        },
    ) {
        Ok(()) => steps.push("unplug left 1 active + 1 pending".into()),
        Err(error) => {
            steps.push(format!("unplug: {error}"));
            passed = false;
        }
    }

    sim.with(|backend| backend.plug(SimDisplay::new("DP-1", false, 4096)));
    match wait_expect(
        &handle,
        &Expect {
            pending: Some(0),
            verified: Some(2),
            ..Default::default()
        },
    ) {
        Ok(()) => steps.push("reconnect reapplied the saved baseline".into()),
        Err(error) => {
            steps.push(format!("reconnect: {error}"));
            passed = false;
        }
    }

    handle.send(DaemonCommand::SetEnabled(false));
    if let Err(error) = wait_expect(
        &handle,
        &Expect {
            filter_enabled: Some(false),
            pending: Some(0),
            ..Default::default()
        },
    ) {
        steps.push(format!("disable: {error}"));
        passed = false;
    }
    handle.shutdown();
    Ok(ScenarioReport {
        scenario: "hotplug".into(),
        passed,
        steps,
    })
}

fn crash_recovery() -> anyhow::Result<ScenarioReport> {
    let (_dir, paths) = temp_paths("crash");
    let settings = EmberSettings {
        filter_enabled: true,
        warmth: 0.4,
        ..Default::default()
    };
    SettingsStore::new(paths.settings_file()).save(&settings)?;
    let display = SimDisplay::new("eDP-1", true, 4096);
    let identity = display.identity();
    let baseline = DisplayBaseline::new(
        1,
        Some(identity.clone()),
        GammaTable::identity(4096),
        chrono::Utc::now(),
    );
    let entry = DisplayRecoveryEntry::without_hardware(identity, baseline);
    let journal = RecoveryJournal::new(paths.journal_file());
    journal.save(&RecoveryRecord::new(
        AppVersion::full(),
        vec![entry],
        settings,
    ))?;

    let sim = SharedSim::new(SimulatedBackend::single());
    let backlight = SharedBacklight::new(SimBacklight::new());
    let mut handle = start(&paths, sim, backlight);
    let mut steps = Vec::new();
    let recovered = handle.wait_for_snapshot(Duration::from_secs(5), |snapshot| {
        snapshot.backend.name == "simulator"
            && !snapshot.settings.filter_enabled
            && snapshot.pending_restore_count == 0
    });
    let mut passed = recovered.is_some();
    steps.push(if passed {
        "startup recovery restored and stayed off".into()
    } else {
        "startup recovery did not finish".to_string()
    });
    if paths.journal_file().exists() {
        let diagnostics = handle.export_diagnostics().unwrap_or_default();
        steps.push(format!(
            "journal was not cleared after recovery; diagnostics tail: {}",
            diagnostics
                .lines()
                .rev()
                .take(10)
                .collect::<Vec<_>>()
                .join(" | ")
        ));
        passed = false;
    } else {
        steps.push("journal cleared after verified recovery".into());
    }
    handle.shutdown();
    Ok(ScenarioReport {
        scenario: "crash-recovery".into(),
        passed,
        steps,
    })
}

fn corrupt_journal() -> anyhow::Result<ScenarioReport> {
    let (_dir, paths) = temp_paths("corrupt");
    std::fs::write(paths.journal_file(), b"not-json{{{")?;
    let sim = SharedSim::new(SimulatedBackend::new());
    let backlight = SharedBacklight::new(SimBacklight::new());
    let mut handle = start(&paths, sim, backlight);
    let mut steps = Vec::new();
    let recovered = handle.wait_for_snapshot(Duration::from_secs(5), |snapshot| {
        !snapshot.attention.message.is_empty()
    });
    let mut passed = recovered.is_some();
    steps.push(if passed {
        "corrupt journal surfaced attention".into()
    } else {
        "corrupt journal was not reported".to_string()
    });
    if paths.journal_file().exists() {
        steps.push("corrupt journal was not quarantined".into());
        passed = false;
    } else {
        steps.push("corrupt journal quarantined, never treated as empty".into());
    }
    handle.shutdown();
    Ok(ScenarioReport {
        scenario: "corrupt-journal".into(),
        passed,
        steps,
    })
}

fn polar_solar() -> anyhow::Result<ScenarioReport> {
    let (_dir, paths) = temp_paths("polar");
    let mut settings = EmberSettings {
        sun_schedule_enabled: true,
        ..Default::default()
    };
    settings.solar_location = Some(CachedSolarLocation::new(
        SolarCoordinate::new(78.2, 15.6),
        chrono::Utc::now(),
    ));
    SettingsStore::new(paths.settings_file()).save(&settings)?;
    let sim = SharedSim::new(SimulatedBackend::single());
    let backlight = SharedBacklight::new(SimBacklight::new());
    let mut handle = start(&paths, sim, backlight);
    let mut steps = Vec::new();
    let active = handle.wait_for_snapshot(Duration::from_secs(5), |snapshot| {
        snapshot.solar.enabled && snapshot.observed_active
    });
    let passed = active.is_some();
    steps.push(if passed {
        "polar night activated the schedule".into()
    } else {
        format!("solar state: {:?}", handle.snapshot().solar)
    });
    handle.shutdown();
    Ok(ScenarioReport {
        scenario: "polar-solar".into(),
        passed,
        steps,
    })
}

fn backlight() -> anyhow::Result<ScenarioReport> {
    let (_dir, paths) = temp_paths("backlight");
    let settings = EmberSettings {
        backlight_lock_enabled: true,
        ..Default::default()
    };
    SettingsStore::new(paths.settings_file()).save(&settings)?;
    let sim = SharedSim::new(SimulatedBackend::single());
    let backlight = SharedBacklight::new(SimBacklight::new());
    let mut handle = start(&paths, sim, backlight.clone());
    let mut steps = Vec::new();
    let mut passed = true;

    handle.send(DaemonCommand::SetEnabled(true));
    match wait_expect(
        &handle,
        &Expect {
            observed_active: Some(true),
            backlight_state: Some("Locked".into()),
            ..Default::default()
        },
    ) {
        Ok(()) => steps.push("Backlight Lock engaged".into()),
        Err(error) => {
            steps.push(format!("backlight: {error}"));
            passed = false;
        }
    }
    if backlight.brightness("eDP-1") != Some(1.0) {
        steps.push(format!(
            "hardware brightness not at full: {:?}",
            backlight.brightness("eDP-1")
        ));
        passed = false;
    }

    handle.send(DaemonCommand::SetEnabled(false));
    let _ = handle.wait_for_snapshot(Duration::from_secs(5), |snapshot| {
        !snapshot.settings.filter_enabled
    });
    match backlight.brightness("eDP-1") {
        Some(value) if (value - 0.6).abs() < 0.02 => {
            steps.push("hardware brightness restored".into())
        }
        other => {
            steps.push(format!("hardware brightness not restored: {other:?}"));
            passed = false;
        }
    }
    handle.shutdown();
    Ok(ScenarioReport {
        scenario: "backlight".into(),
        passed,
        steps,
    })
}

fn external_reset() -> anyhow::Result<ScenarioReport> {
    let (_dir, paths) = temp_paths("reset");
    let sim = SharedSim::new(SimulatedBackend::single());
    let backlight = SharedBacklight::new(SimBacklight::new());
    let mut handle = start(&paths, sim.clone(), backlight);
    let mut steps = Vec::new();
    handle.send(DaemonCommand::SetEnabled(true));
    if handle
        .wait_for_snapshot(Duration::from_secs(5), |snapshot| snapshot.observed_active)
        .is_none()
    {
        handle.shutdown();
        return Ok(ScenarioReport {
            scenario: "external-reset".into(),
            passed: false,
            steps: vec!["activation timed out".into()],
        });
    }
    sim.with(|backend| backend.reset_to_identity("eDP-1"));
    std::thread::sleep(Duration::from_millis(1300));
    let snapshot = handle.snapshot();
    let current = sim.with(|backend| backend.table("eDP-1")).unwrap();
    let baseline = sim.with(|backend| backend.baseline("eDP-1")).unwrap();
    let wanted = baseline.applying(
        ember_core::ColorCurve::gains_for_warmth(snapshot.settings.warmth),
        snapshot.settings.apparent_brightness,
    );
    let delta = current.maximum_absolute_difference(&wanted);
    let passed = delta < 0.004;
    steps.push(format!(
        "post-event verification repaired reset: delta={delta:.5}"
    ));
    handle.shutdown();
    Ok(ScenarioReport {
        scenario: "external-reset".into(),
        passed,
        steps,
    })
}

/// Reads a scenario file (reserved for future scripted runs).
pub fn load_scenario(path: &PathBuf) -> anyhow::Result<Vec<ScenarioStep>> {
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}
