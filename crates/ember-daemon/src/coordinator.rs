use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use ember_core::{
    AppVersion, AttentionAction, AttentionSeverity, AttentionState, AutomationOverride,
    BackendKind, BacklightDriftCheck, CachedSolarLocation, ColorCurve, DisplayBaseline,
    DisplayCountSummary, DisplayEvent, DisplayIdentity, DisplayRecoveryEntry, DisplayRuntimeState,
    DisplayStateMachine, DisplayTopologySnapshot, EmberError, EmberOperationState, EmberPresenter,
    EmberPreset, EmberResult, EmberSettings, GammaSource, GammaTable, JournalLoadOutcome,
    LocationState, NightlightSnapshot, OverrideTracker, Paths, RecoveryJournal, RecoveryRecord,
    RestoreIntent, RestoreOutcome, RestoreReport, SettingsStore, Snapshot, SolarCalculator,
    SolarCoordinate, SolarDayCondition, SolarEvent, SolarSchedule, TrayPrimaryAction,
    VerificationLevel,
};
use ember_platform::{
    Autostart, BackendCapabilities, BackendEvent, BacklightCapability, BacklightControl,
    BacklightController, DisplayBackend, DisplayInfo, GeoClueProvider, HyprlandProbe,
    LocationError, LocationProvider, NullBackend, SleepWatcher, VerifyReport,
};
use log::{debug, warn};
use tokio::sync::watch;

use crate::ipc::{
    BacklightRuntime, BacklightRuntimeState, SnapshotInput, SolarRuntime, VerificationRuntime,
};
use crate::tray::{self, EmberTray};

const TICK: Duration = Duration::from_millis(100);
const RECONCILE_DEBOUNCE: Duration = Duration::from_millis(300);
const RECONCILE_DEADLINE: Duration = Duration::from_millis(2000);
const POST_VERIFY_DELAYS: [Duration; 2] = [Duration::from_millis(500), Duration::from_millis(2000)];
const HEALTH_INTERVAL: Duration = Duration::from_secs(30);
const GUARD_INTERVAL: Duration = Duration::from_secs(5);
const SOLAR_TICK: Duration = Duration::from_secs(15);
const LOCATION_RETRY: Duration = Duration::from_secs(60);
const NIGHTLIGHT_TICK: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub paths: Paths,
    pub headless: bool,
    pub simulate: bool,
    pub safe_mode: bool,
    /// Test hook: apply the saved preference after startup instead of waiting
    /// for a command. Production uses the saved preference naturally.
    pub start_active: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            paths: Paths::from_env(),
            headless: false,
            simulate: false,
            safe_mode: false,
            start_active: false,
        }
    }
}

/// Commands accepted by the coordinator. D-Bus, the tray and tests all use the
/// same command surface.
pub enum DaemonCommand {
    SetEnabled(bool),
    Toggle,
    SetWarmth(f64),
    SetBrightness(f64),
    ApplyPreset(EmberPreset),
    SetBacklightLock(bool),
    SetSunSchedule(bool),
    SetLaunchAtLogin(bool),
    SetTrayPrimaryAction(TrayPrimaryAction),
    SetLocation(f64, f64),
    ClearLocation,
    TurnOffNightlight,
    Retry,
    Reset,
    RestoreNow,
    Refresh,
    ExportDiagnostics(tokio::sync::oneshot::Sender<String>),
    OpenPanel,
    OpenDiagnostics,
    Sleep(bool),
    LocationResult(Result<CachedSolarLocation, LocationError>),
    Shutdown,
}

pub struct DaemonHandle {
    command_tx: Sender<DaemonCommand>,
    snapshots: watch::Receiver<Snapshot>,
    finished: watch::Receiver<bool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl DaemonHandle {
    pub fn send(&self, command: DaemonCommand) -> bool {
        self.command_tx.send(command).is_ok()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshots.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.snapshots.clone()
    }

    pub fn command_sender(&self) -> Sender<DaemonCommand> {
        self.command_tx.clone()
    }

    pub fn finished(&self) -> watch::Receiver<bool> {
        self.finished.clone()
    }

    /// Blocking wait used by integration tests and the CLI acceptance script.
    pub fn wait_for_snapshot<F>(&self, timeout: Duration, predicate: F) -> Option<Snapshot>
    where
        F: Fn(&Snapshot) -> bool,
    {
        let deadline = Instant::now() + timeout;
        loop {
            let snapshot = self.snapshot();
            if predicate(&snapshot) {
                return Some(snapshot);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn shutdown(&mut self) {
        let _ = self.command_tx.send(DaemonCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    pub fn export_diagnostics(&self) -> Option<String> {
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        if !self.send(DaemonCommand::ExportDiagnostics(tx)) {
            return None;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match rx.try_recv() {
                Ok(text) => return Some(text),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return None,
            }
        }
    }
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.command_tx.send(DaemonCommand::Shutdown);
    }
}

pub struct DaemonBuilder {
    config: DaemonConfig,
    backend: Option<Box<dyn DisplayBackend>>,
    backlight: Option<Box<dyn BacklightControl>>,
    tray: bool,
}

impl DaemonBuilder {
    pub fn new(config: DaemonConfig) -> Self {
        Self {
            config,
            backend: None,
            backlight: None,
            tray: true,
        }
    }

    pub fn with_backend(mut self, backend: Box<dyn DisplayBackend>) -> Self {
        self.backend = Some(backend);
        self
    }

    pub fn with_backlight(mut self, backlight: Box<dyn BacklightControl>) -> Self {
        self.backlight = Some(backlight);
        self
    }

    pub fn with_tray(mut self, tray: bool) -> Self {
        self.tray = tray;
        self
    }

    pub fn spawn(self) -> EmberResult<DaemonHandle> {
        let (command_tx, command_rx) = mpsc::channel();
        let (watch_tx, watch_rx) = watch::channel(Snapshot::empty());
        let (finished_tx, finished_rx) = watch::channel(false);
        let self_tx = command_tx.clone();
        let config = self.config.clone();
        let headless = config.headless;
        let tray_enabled = self.tray && !headless;

        let backend = match self.backend {
            Some(backend) => backend,
            None => {
                if config.simulate {
                    Box::new(ember_platform::SimulatedBackend::new())
                } else {
                    match ember_platform::select() {
                        Ok(selected) => selected.backend,
                        Err(error) => {
                            warn!("no display backend: {error}");
                            Box::new(NullBackend::new(error.to_string()))
                        }
                    }
                }
            }
        };
        let backlight = self
            .backlight
            .unwrap_or_else(|| Box::new(BacklightController::new()));

        let join = std::thread::Builder::new()
            .name("ember-coordinator".into())
            .spawn(move || {
                let mut coordinator = Coordinator::new(
                    config,
                    backend,
                    backlight,
                    watch_tx,
                    finished_tx,
                    self_tx,
                    tray_enabled,
                );
                coordinator.run(command_rx);
            })
            .map_err(|error| EmberError::Io(error.to_string()))?;

        Ok(DaemonHandle {
            command_tx,
            snapshots: watch_rx,
            finished: finished_rx,
            join: Some(join),
        })
    }
}

struct SolarState {
    location_state: LocationState,
    location_detail: Option<String>,
    refreshing: bool,
    timezone: String,
    schedule: Option<SolarSchedule>,
    next_retry: Option<Instant>,
    last_wall: DateTime<Utc>,
}

impl SolarState {
    fn new() -> Self {
        Self {
            location_state: LocationState::Disabled,
            location_detail: None,
            refreshing: false,
            timezone: SolarCalculator::local_timezone().to_string(),
            schedule: None,
            next_retry: None,
            last_wall: Utc::now(),
        }
    }
}

struct Coordinator {
    config: DaemonConfig,
    backend: Box<dyn DisplayBackend>,
    backlight: Box<dyn BacklightControl>,
    backend_capabilities: BackendCapabilities,
    settings_store: SettingsStore,
    journal: RecoveryJournal,
    diagnostics: ember_core::DiagnosticLog,
    autostart: Autostart,
    settings: EmberSettings,
    machine: DisplayStateMachine,
    entries: Vec<DisplayRecoveryEntry>,
    controlled: HashSet<String>,
    verified: HashSet<String>,
    failed: HashMap<String, String>,
    displays: Vec<DisplayInfo>,
    ambiguity_keys: HashSet<String>,
    last_topology: Option<DisplayTopologySnapshot>,
    backlight_runtime: BacklightRuntime,
    backlight_failures: u32,
    backlight_identity: Option<DisplayIdentity>,
    attention: Option<AttentionState>,
    last_error: Option<String>,
    generation: u64,
    reconcile_due: Option<Instant>,
    reconcile_deadline: Option<Instant>,
    post_verify: Vec<Instant>,
    next_health: Instant,
    next_guard: Instant,
    next_solar: Instant,
    next_nightlight: Instant,
    settings_flush_due: Option<Instant>,
    wake_due: Option<Instant>,
    override_tracker: OverrideTracker,
    reapplies_suspended: bool,
    solar: SolarState,
    nightlight: Option<NightlightSnapshot>,
    sleep_watcher: Option<SleepWatcher>,
    snapshot_tx: watch::Sender<Snapshot>,
    finished_tx: watch::Sender<bool>,
    last_snapshot: Snapshot,
    command_tx: Sender<DaemonCommand>,
    tray: Option<ksni::blocking::Handle<EmberTray>>,
    tray_enabled: bool,
    shutdown_requested: bool,
    started: bool,
    last_publish: Instant,
}

impl Drop for Coordinator {
    fn drop(&mut self) {
        // If the coordinator ever unwinds, the runtime must not keep a dead
        // D-Bus service alive. A normal finish also sends this; duplicates are
        // harmless.
        let _ = self.finished_tx.send(true);
    }
}

impl Coordinator {
    fn new(
        config: DaemonConfig,
        backend: Box<dyn DisplayBackend>,
        backlight: Box<dyn BacklightControl>,
        snapshot_tx: watch::Sender<Snapshot>,
        finished_tx: watch::Sender<bool>,
        command_tx: Sender<DaemonCommand>,
        tray_enabled: bool,
    ) -> Self {
        let backend_capabilities = backend.capabilities();
        let now = Instant::now();
        let verification = VerificationRuntime {
            level: backend_capabilities.verification,
            readback: backend_capabilities.readback,
        };
        let _ = verification;
        Self {
            settings_store: SettingsStore::new(config.paths.settings_file()),
            journal: RecoveryJournal::new(config.paths.journal_file()),
            diagnostics: ember_core::DiagnosticLog::new(
                config.paths.diagnostics_file(),
                config.paths.diagnostics_salt_file(),
            ),
            autostart: Autostart::default_for_app(),
            backend,
            backlight,
            backend_capabilities,
            config,
            settings: EmberSettings::default(),
            machine: DisplayStateMachine::new(),
            entries: Vec::new(),
            controlled: HashSet::new(),
            verified: HashSet::new(),
            failed: HashMap::new(),
            displays: Vec::new(),
            ambiguity_keys: HashSet::new(),
            last_topology: None,
            backlight_runtime: BacklightRuntime {
                capability: BacklightCapability::unavailable("Not probed yet."),
                state: BacklightRuntimeState::Disengaged,
                detail: None,
            },
            backlight_failures: 0,
            backlight_identity: None,
            attention: None,
            last_error: None,
            generation: 0,
            reconcile_due: None,
            reconcile_deadline: None,
            post_verify: Vec::new(),
            next_health: now + HEALTH_INTERVAL,
            next_guard: now + GUARD_INTERVAL,
            next_solar: now + SOLAR_TICK,
            next_nightlight: now + NIGHTLIGHT_TICK,
            settings_flush_due: None,
            wake_due: None,
            override_tracker: OverrideTracker::new(),
            reapplies_suspended: false,
            solar: SolarState::new(),
            nightlight: None,
            sleep_watcher: None,
            snapshot_tx,
            finished_tx,
            last_snapshot: Snapshot::empty(),
            command_tx,
            tray: None,
            tray_enabled,
            shutdown_requested: false,
            started: false,
            last_publish: Instant::now(),
        }
    }

    fn run(&mut self, commands: Receiver<DaemonCommand>) {
        self.startup();
        loop {
            match commands.recv_timeout(TICK) {
                Ok(command) => self.handle_command(command),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => self.shutdown_requested = true,
            }
            self.poll_backend_events();
            self.poll_sleep();
            self.run_due_timers();
            if self.shutdown_requested {
                break;
            }
        }
        self.finish();
    }

    // MARK: - Startup

    fn startup(&mut self) {
        self.diagnostics.info(format!(
            "Project Ember {} started (backend: {})",
            AppVersion::full(),
            self.backend_capabilities.name
        ));
        self.diagnostics.info(format!(
            "Session: {} · Hyprland={} · GeoClue={}",
            std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".into()),
            HyprlandProbe::session_present(),
            GeoClueProvider::available()
        ));

        let now = Utc::now();
        let load = self.settings_store.load(now);
        self.settings = load.settings;
        if let Some(error) = &load.error {
            self.diagnostics.error(error);
            self.attention = Some(AttentionState::new(
                AttentionSeverity::Warning,
                "Settings were reset",
                error,
                vec![],
            ));
        }
        if self.config.start_active {
            self.settings.filter_enabled = true;
        }

        // Reconcile the persisted "launch at login" preference with reality.
        let actual = self.autostart.is_enabled()
            || ember_platform::autostart::systemd_unit_enabled(
                &ember_platform::SystemCommandRunner,
            );
        if self.settings.launch_at_login != actual {
            self.settings.launch_at_login = actual;
            self.save_settings();
        }

        self.recover_if_needed();
        self.refresh_displays();
        self.refresh_backlight_capability();
        self.refresh_nightlight();

        if !self.config.headless {
            self.sleep_watcher = SleepWatcher::start();
            if self.tray_enabled {
                self.tray = tray::spawn(self.command_tx.clone(), self.last_snapshot.clone());
            }
        }

        if self.settings.sun_schedule_enabled && !self.config.safe_mode {
            self.solar.location_state = LocationState::NeedsLocation;
            if self.settings.solar_location.is_some() {
                self.solar.location_state = LocationState::Granted;
            }
            self.ensure_location(false);
            self.recompute_solar(true);
        } else if self.settings.filter_enabled
            && !self.config.safe_mode
            && matches!(
                self.machine.state(),
                DisplayRuntimeState::Off | DisplayRuntimeState::Degraded(_)
            )
        {
            self.activate();
        } else {
            self.publish(true);
        }

        // If recovery left a pending entry, try to complete it now.
        if !self.entries.is_empty() && !self.settings.filter_enabled {
            self.try_complete_pending_restore();
        }

        self.publish(true);
        self.started = true;
    }

    fn recover_if_needed(&mut self) {
        if !self.journal.exists() {
            return;
        }
        self.diagnostics
            .warn("Unclean previous exit detected; starting recovery");
        self.machine = DisplayStateMachine::with_state(DisplayRuntimeState::Restoring(
            RestoreIntent::Recovery,
        ));
        match self.journal.load_outcome() {
            JournalLoadOutcome::NoJournal => {
                self.machine = DisplayStateMachine::new();
            }
            JournalLoadOutcome::Loaded(record) => {
                self.entries = record.displays;
                self.refresh_displays();
                let report = self.restore_entries_verified();
                let remaining = report.remaining;
                let still_pending = !remaining.is_empty();
                self.entries = remaining;
                self.settings.filter_enabled = false;
                self.save_settings();
                if !still_pending {
                    let _ = self.journal.clear();
                    self.machine = DisplayStateMachine::new();
                    self.diagnostics
                        .info("Recovery completed; Ember stayed off for safety");
                } else {
                    self.persist_journal();
                    let message = pending_message(self.entries.len());
                    self.machine = DisplayStateMachine::with_state(DisplayRuntimeState::Degraded(
                        message.clone(),
                    ));
                    self.diagnostics.warn(message.clone());
                    self.last_error = Some(message);
                }
            }
            JournalLoadOutcome::UnsupportedFutureSchema(version) => {
                let message = EmberError::JournalUnsupportedSchema(version).to_string();
                self.diagnostics.error(message.clone());
                self.last_error = Some(message.clone());
                self.attention = Some(AttentionState::new(
                    AttentionSeverity::Error,
                    "Needs attention",
                    message,
                    vec![AttentionAction::ExportDiagnostics],
                ));
                self.machine = DisplayStateMachine::with_state(DisplayRuntimeState::Degraded(
                    "Recovery journal uses a newer schema.".into(),
                ));
            }
            JournalLoadOutcome::Corrupt(reason) => {
                let preserved = self.journal.quarantine_corrupt_journal();
                let _ = self.backend.release_all();
                let message = format!(
                    "The previous display state could not be exactly restored (journal unreadable: {reason}{}). A compositor reset was attempted.",
                    preserved
                        .map(|path| format!(", preserved at {}", path.display()))
                        .unwrap_or_default()
                );
                self.diagnostics.error(message.clone());
                self.last_error = Some(message.clone());
                self.attention = Some(AttentionState::new(
                    AttentionSeverity::Error,
                    "Needs attention",
                    message,
                    vec![AttentionAction::ExportDiagnostics, AttentionAction::Reset],
                ));
                self.machine = DisplayStateMachine::with_state(DisplayRuntimeState::Degraded(
                    "Recovery journal was unreadable.".into(),
                ));
                self.settings.filter_enabled = false;
                self.save_settings();
            }
            JournalLoadOutcome::IoFailure(message) => {
                self.diagnostics
                    .error(format!("Recovery journal could not be read: {message}"));
                self.last_error = Some(message);
                self.machine = DisplayStateMachine::with_state(DisplayRuntimeState::Degraded(
                    "Recovery journal could not be read.".into(),
                ));
            }
        }
    }

    // MARK: - Commands

    fn handle_command(&mut self, command: DaemonCommand) {
        match command {
            DaemonCommand::SetEnabled(enabled) => self.set_enabled(enabled, true),
            DaemonCommand::Toggle => {
                let observed = self.current_presentation().is_observed_active;
                let turning_on = !self.settings.filter_enabled || !observed;
                self.set_enabled(turning_on, true);
            }
            DaemonCommand::SetWarmth(warmth) => {
                self.settings.warmth = warmth.clamp(0.0, 1.0);
                self.settings_changed(true);
            }
            DaemonCommand::SetBrightness(brightness) => {
                self.settings.apparent_brightness = brightness.clamp(0.10, 1.0);
                self.settings_changed(true);
            }
            DaemonCommand::ApplyPreset(preset) => {
                self.settings.warmth = preset.warmth();
                self.settings_changed(true);
                self.diagnostics.info(format!("Preset selected: {preset}"));
            }
            DaemonCommand::SetBacklightLock(enabled) => self.set_backlight_lock(enabled),
            DaemonCommand::SetSunSchedule(enabled) => self.set_sun_schedule(enabled),
            DaemonCommand::SetLaunchAtLogin(enabled) => self.set_launch_at_login(enabled),
            DaemonCommand::SetTrayPrimaryAction(action) => {
                self.settings.tray_primary_action = action;
                self.save_settings();
                self.publish(true);
            }
            DaemonCommand::SetLocation(latitude, longitude) => {
                self.set_manual_location(latitude, longitude)
            }
            DaemonCommand::ClearLocation => self.clear_location(),
            DaemonCommand::TurnOffNightlight => self.turn_off_nightlight(),
            DaemonCommand::Retry => self.retry(),
            DaemonCommand::Reset => self.reset_now(),
            DaemonCommand::RestoreNow => self.restore_now(),
            DaemonCommand::Refresh => {
                self.refresh_displays();
                self.refresh_nightlight();
                self.publish(true);
            }
            DaemonCommand::ExportDiagnostics(reply) => {
                let _ = reply.send(self.diagnostics.export_text());
            }
            DaemonCommand::OpenPanel => self.launch_gui(&["--show"]),
            DaemonCommand::OpenDiagnostics => self.launch_gui(&["--diagnostics"]),
            DaemonCommand::Sleep(preparing) => {
                if preparing {
                    self.sleep();
                } else {
                    self.wake();
                }
            }
            DaemonCommand::LocationResult(result) => self.handle_location_result(result),
            DaemonCommand::Shutdown => {
                self.shutdown_requested = true;
            }
        }
    }

    fn set_enabled(&mut self, enabled: bool, manual: bool) {
        self.attention = None;
        self.last_error = None;
        if enabled {
            if self.settings.sun_schedule_enabled && manual {
                if let Some(event) = self
                    .solar
                    .schedule
                    .as_ref()
                    .and_then(|schedule| schedule.next_event)
                {
                    self.settings.automation_override =
                        Some(AutomationOverride::new(true, event.date));
                    self.save_settings();
                }
            }
            self.settings.filter_enabled = true;
            self.save_settings();
            self.activate();
        } else {
            if self.settings.sun_schedule_enabled && manual {
                if let Some(event) = self
                    .solar
                    .schedule
                    .as_ref()
                    .and_then(|schedule| schedule.next_event)
                {
                    self.settings.automation_override =
                        Some(AutomationOverride::new(false, event.date));
                    self.save_settings();
                }
            }
            if !matches!(self.machine.state(), DisplayRuntimeState::Off) || self.journal.exists() {
                self.restore_display(RestoreIntent::Disable, true);
            } else {
                self.settings.filter_enabled = false;
                self.save_settings();
                self.publish(true);
            }
        }
    }

    fn settings_changed(&mut self, reapply: bool) {
        self.settings.normalize(Utc::now());
        self.settings_flush_due = Some(Instant::now() + Duration::from_millis(50));
        if reapply && matches!(self.machine.state(), DisplayRuntimeState::Active) {
            // Update the snapshot immediately; the coalesced write follows.
            self.publish(true);
        } else {
            self.publish(true);
        }
    }

    fn flush_settings(&mut self) {
        self.settings_flush_due = None;
        self.settings.normalize(Utc::now());
        self.save_settings();
        if matches!(self.machine.state(), DisplayRuntimeState::Active) {
            self.apply_current_transform();
        }
        self.publish(true);
    }

    fn set_backlight_lock(&mut self, enabled: bool) {
        if !enabled {
            self.settings.backlight_lock_enabled = false;
            self.save_settings();
            if let Some(identity) = self.backlight_identity.clone() {
                if let Some(entry) = self
                    .entries
                    .iter_mut()
                    .find(|entry| entry.identity.matches(&identity))
                {
                    if entry.hardware.has_values() {
                        if let Some(info) = self
                            .displays
                            .iter()
                            .find(|info| info.identity.matches(&identity))
                        {
                            match self.backlight.restore(&info.connector, &entry.hardware) {
                                Ok(()) => entry.hardware = Default::default(),
                                Err(error) => {
                                    self.diagnostics.error(format!(
                                        "Backlight preference-off restore failed: {error}"
                                    ));
                                }
                            }
                        }
                    }
                }
                self.persist_journal();
            }
            self.backlight_identity = None;
            self.backlight_runtime.state = BacklightRuntimeState::Disengaged;
            self.backlight_failures = 0;
            self.publish(true);
            return;
        }
        self.settings.backlight_lock_enabled = true;
        self.save_settings();
        if matches!(self.machine.state(), DisplayRuntimeState::Active) {
            self.engage_backlight();
        }
        self.publish(true);
    }

    fn set_sun_schedule(&mut self, enabled: bool) {
        self.settings.sun_schedule_enabled = enabled;
        self.settings.automation_override = None;
        self.attention = None;
        self.last_error = None;
        if enabled {
            match self.autostart.set_enabled(true) {
                Ok(()) => {
                    self.settings.launch_at_login = true;
                    self.diagnostics
                        .info("Sun schedule enabled; Launch at login also enabled");
                }
                Err(error) => {
                    self.settings.launch_at_login = self.autostart.is_enabled();
                    self.diagnostics
                        .warn(format!("Launch at login could not be enabled: {error}"));
                }
            }
            self.solar.location_state = if self.settings.solar_location.is_some() {
                LocationState::Granted
            } else {
                LocationState::NeedsLocation
            };
            self.save_settings();
            self.ensure_location(true);
            self.recompute_solar(true);
        } else {
            self.solar.location_state = LocationState::Disabled;
            self.solar.schedule = None;
            self.solar.next_retry = None;
            self.save_settings();
            self.diagnostics
                .info("Sun schedule disabled; current display state retained");
            self.publish(true);
        }
    }

    fn set_launch_at_login(&mut self, enabled: bool) {
        match self.autostart.set_enabled(enabled) {
            Ok(()) => {
                self.settings.launch_at_login = enabled || self.autostart.is_enabled();
                self.last_error = None;
            }
            Err(error) => {
                self.settings.launch_at_login = self.autostart.is_enabled();
                self.last_error = Some(format!("Launch at login could not be changed: {error}"));
                self.diagnostics.error(self.last_error.clone().unwrap());
            }
        }
        self.save_settings();
        self.publish(true);
    }

    fn retry(&mut self) {
        self.attention = None;
        self.last_error = None;
        self.override_tracker.reset();
        self.reapplies_suspended = false;
        if self.settings.filter_enabled {
            self.generation += 1;
            self.reconcile_due = Some(Instant::now());
            self.reconcile_deadline = Some(Instant::now() + RECONCILE_DEADLINE);
            if let DisplayRuntimeState::Degraded(_) = self.machine.state() {
                self.machine.set_state(DisplayRuntimeState::Activating);
            }
        } else {
            self.refresh_displays();
            self.try_complete_pending_restore();
        }
        self.publish(true);
    }

    fn reset_now(&mut self) {
        self.diagnostics.info("Manual display reset requested");
        self.reconcile_due = None;
        self.reconcile_deadline = None;
        self.post_verify.clear();
        self.refresh_displays();
        let report = self.restore_entries_verified();
        let has_failure = report.has_failure();
        let mut remaining = report.remaining;
        if has_failure {
            let _ = self.backend.release_all();
            self.diagnostics
                .warn("Compositor fallback applied after exact reset failure");
            remaining.retain(|entry| {
                self.displays
                    .iter()
                    .find(|info| info.identity.matches(&entry.identity))
                    .is_none()
            });
        }
        self.entries = remaining;
        self.controlled.clear();
        self.verified.clear();
        self.failed.clear();
        self.settings.filter_enabled = false;
        self.settings.backlight_lock_enabled = false;
        self.settings.automation_override = None;
        self.backlight_identity = None;
        self.backlight_runtime.state = BacklightRuntimeState::Disengaged;
        self.save_settings();
        if self.entries.is_empty() {
            let _ = self.journal.clear();
            self.machine = DisplayStateMachine::new();
            self.attention = None;
            self.last_error = None;
            self.diagnostics.info("Manual display reset completed");
        } else {
            self.persist_journal();
            let message = pending_message(self.entries.len());
            self.machine =
                DisplayStateMachine::with_state(DisplayRuntimeState::Degraded(message.clone()));
            self.attention = Some(AttentionState::new(
                AttentionSeverity::Warning,
                "Needs attention",
                message.clone(),
                vec![AttentionAction::Retry, AttentionAction::Reset],
            ));
            self.last_error = Some(message);
        }
        self.refresh_displays();
        self.refresh_backlight_capability();
        self.publish(true);
    }

    fn restore_now(&mut self) {
        if !self.entries.is_empty() {
            self.refresh_displays();
            let report = self.restore_entries_verified();
            self.entries = report.remaining;
            if self.entries.is_empty() {
                let _ = self.journal.clear();
            } else {
                self.persist_journal();
            }
            self.controlled.clear();
            self.verified.clear();
            self.failed.clear();
        }
        self.publish(true);
    }

    // MARK: - Timers

    fn run_due_timers(&mut self) {
        let now = Instant::now();

        if let Some(due) = self.settings_flush_due {
            if now >= due {
                self.flush_settings();
            }
        }
        if let Some(due) = self.wake_due {
            if now >= due {
                self.wake_due = None;
                if self.settings.filter_enabled && !self.config.safe_mode {
                    self.activate();
                }
            }
        }
        if let Some(due) = self.reconcile_due {
            if now >= due {
                self.reconcile_due = None;
                self.reconcile_settled();
            }
        }
        let mut pending_posts = std::mem::take(&mut self.post_verify);
        let mut keep = Vec::new();
        let mut run_post = false;
        for due in pending_posts.drain(..) {
            if now >= due {
                run_post = true;
            } else {
                keep.push(due);
            }
        }
        self.post_verify = keep;
        if run_post {
            self.verify_controlled("post-event");
        }
        if now >= self.next_health {
            self.next_health = now + HEALTH_INTERVAL;
            if matches!(self.machine.state(), DisplayRuntimeState::Active) {
                self.verify_controlled("health");
                self.refresh_displays();
                self.publish(false);
            }
        }
        if now >= self.next_guard {
            self.next_guard = now + GUARD_INTERVAL;
            self.maintain_backlight();
        }
        if now >= self.next_solar {
            self.next_solar = now + SOLAR_TICK;
            self.solar_tick();
        }
        if now >= self.next_nightlight {
            self.next_nightlight = now + NIGHTLIGHT_TICK;
            self.refresh_nightlight();
            self.publish(false);
        }
        if let Some(retry) = self.solar.next_retry {
            if now >= retry {
                self.solar.next_retry = None;
                self.ensure_location(true);
            }
        }
    }

    fn poll_backend_events(&mut self) {
        let events = self.backend.drain_events();
        for event in events {
            match event {
                BackendEvent::TopologyChanged => self.topology_changed(),
                BackendEvent::ControlLost { connector, reason } => {
                    self.diagnostics
                        .warn(format!("Gamma control lost on {connector}: {reason}"));
                    self.last_error = Some(format!("Display control was lost on {connector}."));
                    self.attention = Some(AttentionState::new(
                        AttentionSeverity::Error,
                        "Needs attention",
                        reason.to_string(),
                        vec![AttentionAction::Retry, AttentionAction::Reset],
                    ));
                    for (key, message) in self.failed.iter_mut() {
                        if key.contains(&connector) {
                            *message = reason.clone();
                        }
                    }
                    self.generation += 1;
                    self.reconcile_due = Some(Instant::now() + RECONCILE_DEBOUNCE);
                    self.reconcile_deadline = Some(Instant::now() + RECONCILE_DEADLINE);
                    self.publish(true);
                }
                BackendEvent::Fatal(message) => {
                    self.diagnostics
                        .error(format!("Backend failure: {message}"));
                    self.last_error = Some(message.clone());
                    self.attention = Some(AttentionState::new(
                        AttentionSeverity::Error,
                        "Needs attention",
                        message,
                        vec![AttentionAction::Retry, AttentionAction::Reset],
                    ));
                    self.machine.set_state(DisplayRuntimeState::Degraded(
                        "Display backend failed.".into(),
                    ));
                    self.publish(true);
                }
            }
        }
    }

    fn poll_sleep(&mut self) {
        let Some(watcher) = &self.sleep_watcher else {
            return;
        };
        for preparing in watcher.drain() {
            if preparing {
                self.sleep();
            } else {
                self.wake();
            }
        }
    }

    fn sleep(&mut self) {
        let should_restore = matches!(
            self.machine.state(),
            DisplayRuntimeState::Active
                | DisplayRuntimeState::Activating
                | DisplayRuntimeState::Reconciling
                | DisplayRuntimeState::Degraded(_)
        );
        if !should_restore {
            return;
        }
        self.diagnostics.info("System sleep detected");
        self.reconcile_due = None;
        self.post_verify.clear();
        self.restore_display(RestoreIntent::Sleep, false);
    }

    fn wake(&mut self) {
        self.diagnostics.info("System wake detected");
        self.wake_due = Some(Instant::now() + Duration::from_secs(1));
    }

    fn finish(&mut self) {
        self.diagnostics.info("Daemon terminating");
        if let Some(due) = self.settings_flush_due.take() {
            let _ = due;
            self.save_settings();
        }
        if !matches!(self.machine.state(), DisplayRuntimeState::Off) || self.journal.exists() {
            self.refresh_displays();
            let report = self.restore_entries_verified();
            self.entries = report.remaining;
            if self.entries.is_empty() {
                let _ = self.journal.clear();
            } else {
                self.persist_journal();
            }
        }
        let _ = self.backend.release_all();
        self.backend.shutdown();
        if let Some(tray) = self.tray.take() {
            tray.shutdown().wait();
        }
        self.publish(true);
        let _ = self.finished_tx.send(true);
    }

    // MARK: - Presentation / snapshot

    fn operation_state(&self) -> EmberOperationState {
        match self.machine.state() {
            DisplayRuntimeState::Off => EmberOperationState::Off,
            DisplayRuntimeState::Activating => EmberOperationState::Activating,
            DisplayRuntimeState::Active => EmberOperationState::Active,
            DisplayRuntimeState::Reconciling => EmberOperationState::Reconciling,
            DisplayRuntimeState::Restoring(_) => EmberOperationState::Restoring,
            DisplayRuntimeState::Suspended => EmberOperationState::Suspended,
            DisplayRuntimeState::Degraded(_) => EmberOperationState::Degraded,
        }
    }

    fn current_presentation(&self) -> ember_core::EmberPresentation {
        let online_keys: HashSet<String> = self
            .displays
            .iter()
            .filter(|info| info.gamma_compatible())
            .map(|info| info.identity.key())
            .collect();
        let verified: HashSet<String> = self
            .verified
            .iter()
            .filter(|key| online_keys.contains(*key))
            .cloned()
            .collect();
        let failed: Vec<String> = self
            .failed
            .values()
            .filter(|value| !value.is_empty())
            .cloned()
            .collect();
        let unsupported = self.displays.iter().filter(|info| !info.is_online).count()
            + self
                .displays
                .iter()
                .filter(|info| info.is_online && !info.gamma_compatible())
                .count();
        let mirrored = self.displays.iter().filter(|info| info.is_mirrored).count();
        let online_intended = self
            .displays
            .iter()
            .filter(|info| info.gamma_compatible())
            .count();
        let pending = self.pending_count();
        let verification = self.backend_capabilities.verification;
        EmberPresenter::make(
            self.settings.filter_enabled,
            self.operation_state(),
            &verified,
            unsupported,
            pending,
            &failed,
            online_intended,
            mirrored,
            verification,
            self.attention.clone(),
        )
    }

    fn pending_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| {
                !self
                    .displays
                    .iter()
                    .any(|info| info.identity.matches(&entry.identity) && info.is_online)
            })
            .count()
    }

    fn publish(&mut self, force: bool) {
        if !force && self.last_publish.elapsed() < Duration::from_millis(750) {
            return;
        }
        self.last_publish = Instant::now();
        let presentation = self.current_presentation();
        let verification = VerificationRuntime {
            level: self.backend_capabilities.verification,
            readback: self.backend_capabilities.readback,
        };
        let pending_keys: Vec<DisplayIdentity> = self
            .entries
            .iter()
            .filter(|entry| {
                !self
                    .displays
                    .iter()
                    .any(|info| info.identity.matches(&entry.identity) && info.is_online)
            })
            .map(|entry| entry.identity.clone())
            .collect();
        let verified: Vec<DisplayIdentity> = self
            .entries
            .iter()
            .filter(|entry| self.verified.contains(&entry.identity.key()))
            .map(|entry| entry.identity.clone())
            .collect();
        let failed: Vec<(DisplayIdentity, String)> = self
            .entries
            .iter()
            .filter_map(|entry| {
                self.failed
                    .get(&entry.identity.key())
                    .map(|message| (entry.identity.clone(), message.clone()))
            })
            .collect();
        let schedule = self.solar.schedule.as_ref();
        let input = SnapshotInput {
            settings: &self.settings,
            operation: self.operation_state(),
            presentation: &presentation,
            displays: &self.displays,
            pending_entries: &pending_keys,
            verified: &verified,
            failed: &failed,
            journaled_offline: &pending_keys,
            topology: self.last_topology.as_ref(),
            backlight: &self.backlight_runtime,
            solar: &SolarRuntime {
                location_state: self.solar.location_state,
                location_detail: self.solar.location_detail.clone(),
                refreshing: self.solar.refreshing,
                timezone: self.solar.timezone.clone(),
            },
            solar_schedule: schedule,
            nightlight: self.nightlight.as_ref(),
            verification: &verification,
            backend_kind: self.backend_capabilities.kind,
            backend_name: self.backend_capabilities.name,
            backend_note: self.backend_capabilities.note.as_deref(),
            busy: matches!(
                self.machine.state(),
                DisplayRuntimeState::Activating
                    | DisplayRuntimeState::Reconciling
                    | DisplayRuntimeState::Restoring(_)
            ),
            last_error: self.last_error.as_deref(),
        };
        let snapshot = input.build();
        if force || snapshot != self.last_snapshot {
            self.last_snapshot = snapshot.clone();
            let _ = self.snapshot_tx.send(snapshot.clone());
            if let Some(tray) = &self.tray {
                tray.update(|tray_state: &mut EmberTray| {
                    tray_state.snapshot = snapshot.clone();
                });
            }
        }
    }

    // MARK: - Helpers

    fn save_settings(&mut self) {
        if let Err(error) = self.settings_store.save(&self.settings) {
            self.diagnostics
                .error(format!("Settings could not be saved: {error}"));
            self.last_error = Some(error.to_string());
        }
    }

    fn persist_journal(&mut self) {
        if self.entries.is_empty() {
            if let Err(error) = self.journal.clear() {
                self.diagnostics
                    .error(format!("Journal clear failed: {error}"));
            }
            return;
        }
        let record = RecoveryRecord::new(
            AppVersion::full(),
            self.entries.clone(),
            self.settings.clone(),
        );
        if let Err(error) = self.journal.save(&record) {
            self.diagnostics
                .error(format!("Journal save failed: {error}"));
            self.last_error = Some(format!("Recovery journal could not be saved: {error}"));
        }
    }

    fn expected_table(&self, baseline: &GammaTable) -> GammaTable {
        baseline.applying(
            ColorCurve::gains_for_warmth(self.settings.warmth),
            self.settings.apparent_brightness,
        )
    }

    fn refresh_displays(&mut self) {
        match self.backend.enumerate() {
            Ok(mut displays) => {
                if let Some(probe) = self.hyprland_probe() {
                    let mirrored = probe.mirrored_connectors();
                    for display in &mut displays {
                        if mirrored.contains(&display.connector) {
                            display.is_mirrored = true;
                        }
                    }
                }
                let mut counts: HashMap<String, usize> = HashMap::new();
                for display in &displays {
                    *counts.entry(display.identity.key()).or_default() += 1;
                }
                self.ambiguity_keys = counts
                    .into_iter()
                    .filter(|(_, count)| *count > 1)
                    .map(|(key, _)| key)
                    .collect();
                self.displays = displays;
                self.last_error = None;
            }
            Err(error) => {
                self.displays.clear();
                self.ambiguity_keys.clear();
                self.diagnostics
                    .warn(format!("Display enumeration failed: {error}"));
                self.last_error = Some(error.to_string());
            }
        }
        self.refresh_backlight_capability();
    }

    fn refresh_backlight_capability(&mut self) {
        let built_in = self
            .displays
            .iter()
            .find(|info| info.built_in && info.gamma_compatible());
        let capability = match built_in {
            Some(info) => self.backlight.capability(&info.connector),
            None => BacklightCapability::unavailable("No built-in display is available."),
        };
        if self.backlight_runtime.capability != capability {
            if !capability.available
                && self.backlight_runtime.state == BacklightRuntimeState::Engaged
            {
                self.backlight_runtime.state = BacklightRuntimeState::Unavailable;
            }
            self.backlight_runtime.capability = capability;
        }
    }

    fn hyprland_probe(&self) -> Option<HyprlandProbe> {
        if HyprlandProbe::session_present() {
            let probe = HyprlandProbe::new();
            if probe.available() {
                return Some(probe);
            }
        }
        None
    }

    fn refresh_nightlight(&mut self) {
        self.nightlight = self
            .hyprland_probe()
            .and_then(|probe| probe.nightlight())
            .filter(|state| state.temperature.is_some())
            .map(|state| NightlightSnapshot {
                active: state.active,
                temperature: state.temperature,
            });
    }

    fn turn_off_nightlight(&mut self) {
        use ember_platform::CommandRunner;
        let runner = ember_platform::SystemCommandRunner;
        match runner.run("hyprctl", &["hyprsunset", "identity"]) {
            Ok(outcome) if outcome.success => {
                self.diagnostics
                    .info("Nightlight turned off at the user's request");
                self.refresh_nightlight();
            }
            Ok(outcome) => {
                self.last_error = Some(format!(
                    "Nightlight could not be turned off: {}",
                    outcome.stderr
                ));
            }
            Err(error) => {
                self.last_error = Some(format!("hyprctl is unavailable: {error}"));
            }
        }
        self.publish(true);
    }

    fn launch_gui(&self, args: &[&str]) {
        let program = std::env::var("EMBER_GUI").unwrap_or_else(|_| "ember".to_string());
        match std::process::Command::new(&program).args(args).spawn() {
            Ok(_) => debug!("launched {program} {args:?}"),
            Err(error) => warn!("could not launch {program}: {error}"),
        }
    }
}

fn pending_message(count: usize) -> String {
    format!(
        "Ember saved the original state for {count} disconnected display{}. It will restore when the display reconnects.",
        if count == 1 { "" } else { "s" }
    )
}

/// Single-instance lock using an advisory flock. The file is 0600 and lives in
/// the runtime directory; a stale file from a crash is harmless.
pub struct InstanceLock {
    _file: std::fs::File,
}

impl InstanceLock {
    pub fn acquire(paths: &Paths) -> EmberResult<Self> {
        ember_core::fsutil::ensure_private_dir(paths.runtime_dir())
            .map_err(|error| EmberError::Io(error.to_string()))?;
        let path = paths.lock_file();
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() {
                return Err(EmberError::Io("refusing symlinked lock file".into()));
            }
        }
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|error| EmberError::Io(error.to_string()))?;
        use std::os::fd::AsRawFd;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            return Err(EmberError::AlreadyRunning);
        }
        Ok(Self { _file: file })
    }
}

/// Read-only helper used by `emberctl probe` and the acceptance script.
pub fn probe_backend() -> EmberResult<(String, Vec<DisplayInfo>)> {
    let selected = ember_platform::select()?;
    let mut backend = selected.backend;
    let capabilities = backend.capabilities();
    let displays = backend.enumerate()?;
    backend.shutdown();
    Ok((capabilities.name.to_string(), displays))
}

pub fn version_string() -> String {
    format!(
        "{} · backend module {} · journal schema {}",
        AppVersion::full(),
        env!("CARGO_PKG_VERSION"),
        AppVersion::MAX_SUPPORTED_SCHEMA
    )
}

/// Ensure the state directory layout exists with safe permissions.
pub fn prepare_paths(paths: &Paths) -> EmberResult<()> {
    for dir in [paths.config_dir(), paths.state_dir(), paths.runtime_dir()] {
        ember_core::fsutil::ensure_private_dir(dir)
            .map_err(|error| EmberError::Io(format!("{}: {error}", dir.display())))?;
    }
    Ok(())
}

pub fn config_from(paths: Paths) -> DaemonConfig {
    DaemonConfig {
        paths,
        ..Default::default()
    }
}

pub fn path_is_dir(path: &Path) -> bool {
    path.is_dir()
}

// Keep a few imports used by feature-gated diagnostics paths honest.
#[allow(dead_code)]
fn _type_assertions() {
    let _ = DisplayBaseline::new(0, None, GammaTable::identity(2), Utc::now());
    let _ = DisplayCountSummary::default();
    let _ = DisplayEvent::EnableRequested;
    let _ = GammaSource::Readback;
    let _ = SolarCoordinate::new(0.0, 0.0);
    let _ = SolarDayCondition::SunAlwaysAboveHorizon;
    let _ = SolarEvent::new(ember_core::SolarEventKind::Sunrise, Utc::now());
    let _ = DisplayStateMachine::new();
    let _ = RestoreOutcome::resolve_failed(String::new());
    let _ = RestoreReport {
        outcomes: vec![],
        remaining: vec![],
    };
    let _ = BacklightDriftCheck::may_engage(true);
    let _ = DisplayIdentity::legacy(0);
    let _ = VerificationLevel::Applied;
    let _ = BackendKind::None;
}

// MARK: - Activation, restore and reconciliation
//
// These methods are intentionally kept in this module: they are the only place
// that mutates display state, and keeping them adjacent to the state fields
// makes the journal-before-mutation ordering auditable.

impl Coordinator {
    fn activate(&mut self) {
        if self.config.safe_mode || !self.settings.filter_enabled {
            return;
        }
        if matches!(self.machine.state(), DisplayRuntimeState::Active) {
            self.apply_current_transform();
            return;
        }
        if matches!(
            self.machine.state(),
            DisplayRuntimeState::Activating | DisplayRuntimeState::Reconciling
        ) {
            return;
        }

        let event = if matches!(self.machine.state(), DisplayRuntimeState::Suspended) {
            DisplayEvent::WakeRequested
        } else {
            DisplayEvent::EnableRequested
        };
        self.machine.transition(event);
        self.publish(true);
        self.refresh_displays();

        // Refuse to continue if the journal exists but cannot be read: silently
        // treating it as empty could overwrite an unverified baseline.
        if self.entries.is_empty() && self.journal.exists() {
            match self.journal.load_outcome() {
                JournalLoadOutcome::NoJournal => {}
                JournalLoadOutcome::Loaded(record) => self.entries = record.displays,
                JournalLoadOutcome::UnsupportedFutureSchema(version) => {
                    let message = EmberError::JournalUnsupportedSchema(version).to_string();
                    self.fail_activation(message);
                    return;
                }
                JournalLoadOutcome::Corrupt(reason) => {
                    let _ = self.journal.quarantine_corrupt_journal();
                    self.fail_activation(EmberError::JournalCorrupt(reason).to_string());
                    return;
                }
                JournalLoadOutcome::IoFailure(message) => {
                    self.fail_activation(format!("Recovery journal could not be read: {message}"));
                    return;
                }
            }
        }

        // Complete any pending restores for displays that returned.
        let report = self.restore_entries_verified();
        self.entries = report.remaining;

        // Capture fresh baselines for displays that have no journal entry.
        let mut fresh: Vec<DisplayRecoveryEntry> = Vec::new();
        let mut capture_failures = 0usize;
        for info in self.displays.clone() {
            if !info.gamma_compatible() {
                continue;
            }
            let key = info.identity.key();
            if self.ambiguity_keys.contains(&key) {
                self.diagnostics.warn(format!(
                    "Skipped ambiguous display {}; identity matches multiple displays",
                    info.connector
                ));
                continue;
            }
            if self
                .entries
                .iter()
                .any(|entry| entry.identity.matches(&info.identity))
            {
                continue;
            }
            match self.backend.capture_baseline(&info.identity) {
                Ok(baseline) => fresh.push(DisplayRecoveryEntry::without_hardware(
                    info.identity.clone(),
                    baseline,
                )),
                Err(error) => {
                    capture_failures += 1;
                    self.diagnostics.warn(format!(
                        "Display {} baseline capture failed: {error}",
                        info.connector
                    ));
                }
            }
        }

        // Capture the built-in hardware baseline only when Backlight Lock is
        // enabled and the display exposes a usable device.
        self.backlight_identity = None;
        if self.settings.backlight_lock_enabled {
            if let Some(info) = self
                .displays
                .iter()
                .find(|info| info.built_in && info.gamma_compatible())
                .cloned()
            {
                let capability = self.backlight.capability(&info.connector);
                self.backlight_runtime.capability = capability.clone();
                if capability.available {
                    match self.backlight.capture(&info.connector) {
                        Ok(hardware) => {
                            if let Some(entry) = fresh
                                .iter_mut()
                                .find(|entry| entry.identity.matches(&info.identity))
                            {
                                entry.hardware = hardware;
                            } else if let Some(entry) = self
                                .entries
                                .iter_mut()
                                .find(|entry| entry.identity.matches(&info.identity))
                            {
                                if !entry.hardware.has_values() {
                                    entry.hardware = hardware;
                                }
                            }
                            self.backlight_identity = Some(info.identity.clone());
                        }
                        Err(error) => {
                            self.diagnostics.warn(format!(
                                "Backlight baseline capture failed; continuing with software dimming: {error}"
                            ));
                        }
                    }
                } else {
                    self.backlight_runtime.state = BacklightRuntimeState::Unavailable;
                    self.backlight_runtime.detail = capability.detail.clone();
                    self.diagnostics
                        .warn("Backlight Lock target unavailable; preference retained");
                }
            }
        }

        let has_online_entry = !fresh.is_empty()
            || self.entries.iter().any(|entry| {
                self.displays
                    .iter()
                    .any(|info| info.identity.matches(&entry.identity) && info.is_online)
            });
        if !has_online_entry {
            if self.entries.is_empty() {
                self.fail_activation(EmberError::NoCompatibleDisplay.to_string());
            } else {
                // Pending-only: stay calm and truthful rather than alarming.
                let message = pending_message(self.entries.len());
                self.machine =
                    DisplayStateMachine::with_state(DisplayRuntimeState::Degraded(message.clone()));
                self.attention = None;
                self.last_error = None;
                self.persist_journal();
                self.publish(true);
            }
            return;
        }

        // Journal before mutation (invariant 1).
        let previous_len = self.entries.len();
        self.entries.extend(fresh.iter().cloned());
        let record = RecoveryRecord::new(
            AppVersion::full(),
            self.entries.clone(),
            self.settings.clone(),
        );
        if let Err(error) = self.journal.save(&record) {
            self.entries.truncate(previous_len);
            self.fail_activation(format!(
                "Recovery journal could not be saved before changing displays: {error}"
            ));
            return;
        }

        // Apply from immutable baselines, including pending entries that are
        // online again. One failing display never blocks the others.
        let targets: Vec<DisplayRecoveryEntry> = self
            .entries
            .iter()
            .filter(|entry| {
                self.displays
                    .iter()
                    .any(|info| info.identity.matches(&entry.identity) && info.gamma_compatible())
            })
            .cloned()
            .collect();
        let mut successes = 0usize;
        for entry in &targets {
            let key = entry.identity.key();
            let expected = self.expected_table(&entry.display.gamma_table);
            match self.backend.apply(&entry.identity, &expected) {
                Ok(_) => {
                    self.controlled.insert(key.clone());
                    self.verified.insert(key.clone());
                    self.failed.remove(&key);
                    successes += 1;
                }
                Err(error) => {
                    self.failed.insert(key.clone(), error.to_string());
                    self.verified.remove(&key);
                    // Best-effort rollback; the journal entry is retained
                    // either way so a later pass can prove restoration.
                    let _ = self.backend.restore(&entry.identity, &entry.display);
                    self.diagnostics.error(format!(
                        "Display {} apply failed: {error}",
                        entry.identity.label()
                    ));
                }
            }
        }

        if successes == 0 {
            let message = self
                .failed
                .values()
                .next()
                .cloned()
                .unwrap_or_else(|| EmberError::NoCompatibleDisplay.to_string());
            self.fail_activation(message);
            return;
        }

        if self.settings.backlight_lock_enabled {
            self.engage_backlight();
        }

        self.machine.transition(DisplayEvent::ActivationSucceeded);
        self.next_guard = Instant::now() + GUARD_INTERVAL;
        self.next_health = Instant::now() + HEALTH_INTERVAL;
        self.reapplies_suspended = false;
        self.override_tracker.reset();
        self.schedule_post_verify();
        self.persist_journal();
        if self.failed.is_empty() {
            self.last_error = None;
            self.attention = None;
        } else {
            let message = self.failed.values().cloned().collect::<Vec<_>>().join(" ");
            self.last_error = Some(message.clone());
            self.attention = Some(AttentionState::new(
                AttentionSeverity::Warning,
                "Partially active",
                message,
                vec![AttentionAction::Retry, AttentionAction::Reset],
            ));
        }
        self.diagnostics.info(format!(
            "Display transform active on {successes} display(s){}",
            if capture_failures > 0 {
                format!("; {capture_failures} capture failure(s)")
            } else {
                String::new()
            }
        ));
        self.publish(true);
    }

    fn fail_activation(&mut self, message: String) {
        self.diagnostics
            .error(format!("Activation failed: {message}"));
        let report = self.restore_entries_verified();
        self.entries = report.remaining;
        self.controlled.clear();
        self.verified.clear();
        self.failed.clear();
        if self.entries.is_empty() {
            let _ = self.journal.clear();
        } else {
            self.persist_journal();
        }
        self.machine
            .transition(DisplayEvent::ActivationFailed(message.clone()));
        self.settings.filter_enabled = false;
        self.save_settings();
        self.last_error = Some(message.clone());
        self.attention = Some(AttentionState::new(
            AttentionSeverity::Error,
            "Needs attention",
            message,
            vec![AttentionAction::Retry, AttentionAction::Reset],
        ));
        self.publish(true);
    }

    fn restore_display(&mut self, intent: RestoreIntent, persist_disabled_state: bool) {
        let event = match intent {
            RestoreIntent::Disable => DisplayEvent::DisableRequested,
            RestoreIntent::Sleep => DisplayEvent::SleepRequested,
            RestoreIntent::Terminate => DisplayEvent::TerminationRequested,
            RestoreIntent::Recovery => {
                self.machine
                    .set_state(DisplayRuntimeState::Restoring(RestoreIntent::Recovery));
                DisplayEvent::RestoreSucceeded
            }
        };
        if intent != RestoreIntent::Recovery {
            self.machine.transition(event);
        }
        self.reconcile_due = None;
        self.post_verify.clear();
        self.publish(true);

        self.refresh_displays();
        let report = self.restore_entries_verified();
        let has_failure = report.has_failure();
        self.entries = report.remaining;
        self.controlled.clear();
        self.verified.clear();
        self.failed.clear();
        self.backlight_identity = None;
        self.backlight_failures = 0;
        if self.backlight_runtime.state == BacklightRuntimeState::Engaged {
            self.backlight_runtime.state = BacklightRuntimeState::Disengaged;
        }
        if persist_disabled_state {
            self.settings.filter_enabled = false;
        }
        self.save_settings();

        if self.entries.is_empty() {
            self.machine.transition(DisplayEvent::RestoreSucceeded);
            let _ = self.journal.clear();
            self.last_error = None;
            self.attention = None;
            self.diagnostics
                .info("Saved state restored for every available display");
        } else {
            let message = pending_message(self.entries.len());
            self.persist_journal();
            self.machine
                .transition(DisplayEvent::RestoreFailed(message.clone()));
            let has_online = self.entries.iter().any(|entry| {
                self.displays
                    .iter()
                    .any(|info| info.identity.matches(&entry.identity) && info.is_online)
            });
            if has_online || has_failure {
                self.last_error = Some(message.clone());
                self.attention = Some(AttentionState::new(
                    AttentionSeverity::Warning,
                    "Needs attention",
                    message.clone(),
                    vec![AttentionAction::Retry, AttentionAction::Reset],
                ));
            } else {
                // Pending-only disconnected: calm but truthful.
                self.last_error = Some(message.clone());
                self.attention = None;
            }
            self.diagnostics.warn(message);
        }
        self.refresh_backlight_capability();
        self.publish(true);
    }

    fn restore_entries_verified(&mut self) -> RestoreReport {
        let mut outcomes: Vec<RestoreOutcome> = Vec::new();
        let mut remaining: Vec<DisplayRecoveryEntry> = Vec::new();
        let entries = std::mem::take(&mut self.entries);
        for entry in entries {
            let key = entry.identity.key();
            if self.ambiguity_keys.contains(&key) {
                outcomes.push(RestoreOutcome::identity_ambiguous(
                    "multiple online displays match the identity".into(),
                ));
                remaining.push(entry);
                continue;
            }
            let Some(info) = self
                .displays
                .iter()
                .find(|info| info.identity.matches(&entry.identity) && info.is_online)
                .cloned()
            else {
                outcomes.push(RestoreOutcome::display_offline_pending(
                    entry.identity.clone(),
                ));
                remaining.push(entry);
                continue;
            };
            match self.backend.restore(&entry.identity, &entry.display) {
                Ok(()) => {
                    if entry.hardware.has_values() {
                        if let Err(error) = self.backlight.restore(&info.connector, &entry.hardware)
                        {
                            outcomes.push(RestoreOutcome::hardware_restore_failed(
                                entry.identity.clone(),
                                error.to_string(),
                            ));
                            remaining.push(entry);
                            self.diagnostics.error(format!(
                                "Hardware restore failed for {}: {error}",
                                info.connector
                            ));
                            continue;
                        }
                    }
                    outcomes.push(RestoreOutcome::restored_and_verified(
                        entry.identity.clone(),
                    ));
                    self.controlled.remove(&key);
                    self.verified.remove(&key);
                    self.failed.remove(&key);
                    self.diagnostics
                        .info(format!("Verified restore for {}", info.connector));
                }
                Err(error) => {
                    let readback = matches!(error, EmberError::GammaVerificationFailed);
                    if readback {
                        outcomes.push(RestoreOutcome::gamma_readback_mismatch(
                            entry.identity.clone(),
                            f32::NAN,
                        ));
                    } else {
                        outcomes.push(RestoreOutcome::gamma_write_failed(
                            entry.identity.clone(),
                            error.to_string(),
                        ));
                    }
                    remaining.push(entry);
                    self.diagnostics
                        .error(format!("Restore failed for {}: {error}", info.connector));
                }
            }
        }
        self.entries = remaining.clone();
        RestoreReport {
            outcomes,
            remaining,
        }
    }

    fn apply_current_transform(&mut self) {
        self.refresh_displays();
        let entries = self.entries.clone();
        let mut failures = 0;
        for entry in entries {
            let key = entry.identity.key();
            if !self.verified.contains(&key) && !self.controlled.contains(&key) {
                continue;
            }
            if !self
                .displays
                .iter()
                .any(|info| info.identity.matches(&entry.identity) && info.gamma_compatible())
            {
                continue;
            }
            let expected = self.expected_table(&entry.display.gamma_table);
            match self.backend.apply(&entry.identity, &expected) {
                Ok(_) => {
                    self.verified.insert(key.clone());
                    self.failed.remove(&key);
                }
                Err(error) => {
                    failures += 1;
                    self.failed.insert(key.clone(), error.to_string());
                    self.verified.remove(&key);
                    self.diagnostics
                        .error(format!("Live update failed: {error}"));
                }
            }
        }
        if failures > 0 {
            let message = self.failed.values().next().cloned().unwrap_or_default();
            self.attention = Some(AttentionState::new(
                AttentionSeverity::Warning,
                "Partially active",
                message.clone(),
                vec![AttentionAction::Retry, AttentionAction::Reset],
            ));
            self.last_error = Some(message);
        }
        self.persist_journal();
        self.publish(true);
    }

    fn try_complete_pending_restore(&mut self) {
        if matches!(
            self.machine.state(),
            DisplayRuntimeState::Active | DisplayRuntimeState::Activating
        ) {
            return;
        }
        self.refresh_displays();
        let report = self.restore_entries_verified();
        self.entries = report.remaining;
        if self.entries.is_empty() {
            let _ = self.journal.clear();
            if matches!(self.machine.state(), DisplayRuntimeState::Degraded(_)) {
                self.machine = DisplayStateMachine::new();
            }
            self.attention = None;
            self.last_error = None;
            self.diagnostics
                .info("A reconnected display completed its pending recovery");
        } else {
            self.persist_journal();
            self.last_error = Some(pending_message(self.entries.len()));
        }
        self.publish(true);
    }

    fn topology_changed(&mut self) {
        self.generation += 1;
        self.diagnostics
            .info(format!("Topology changed (generation {})", self.generation));
        let state = self.machine.state().clone();
        if matches!(
            state,
            DisplayRuntimeState::Active
                | DisplayRuntimeState::Activating
                | DisplayRuntimeState::Reconciling
                | DisplayRuntimeState::Degraded(_)
        ) {
            self.machine.transition(DisplayEvent::DisplayChanged);
        }
        self.reconcile_due = Some(Instant::now() + RECONCILE_DEBOUNCE);
        self.reconcile_deadline = Some(Instant::now() + RECONCILE_DEADLINE);
        self.publish(true);
    }

    fn reconcile_settled(&mut self) {
        let deadline = self.reconcile_deadline.unwrap_or_else(Instant::now);
        let generation = self.generation;
        self.refresh_displays();
        let topology = DisplayTopologySnapshot::new(
            generation,
            Utc::now(),
            self.displays
                .iter()
                .map(|info| info.topology_entry())
                .collect(),
        );
        let settled = self
            .last_topology
            .as_ref()
            .map(|previous| previous.matches_topology(&topology))
            .unwrap_or(false);
        if !settled && Instant::now() < deadline {
            self.last_topology = Some(topology);
            self.reconcile_due = Some(Instant::now() + RECONCILE_DEBOUNCE);
            return;
        }
        let warning = if !settled && Instant::now() >= deadline {
            Some("Display topology did not stabilize within 2s; reconciled the latest safe snapshot.".to_string())
        } else {
            None
        };
        self.last_topology = Some(topology.clone());
        self.reconcile_now(topology, warning);
    }

    fn reconcile_now(&mut self, topology: DisplayTopologySnapshot, warning: Option<String>) {
        let generation = self.generation;
        if !self.settings.filter_enabled {
            let report = self.restore_entries_verified();
            self.entries = report.remaining;
            self.controlled.clear();
            self.verified.clear();
            self.failed.clear();
            if self.entries.is_empty() {
                let _ = self.journal.clear();
                if matches!(self.machine.state(), DisplayRuntimeState::Reconciling) {
                    self.machine
                        .transition(DisplayEvent::ReconciliationSucceeded);
                }
                self.last_error = None;
                self.attention = None;
            } else {
                self.persist_journal();
                self.last_error = Some(pending_message(self.entries.len()));
            }
            self.publish(true);
            return;
        }

        let controlled_identities: HashSet<DisplayIdentity> = self
            .entries
            .iter()
            .filter(|entry| self.controlled.contains(&entry.identity.key()))
            .map(|entry| entry.identity.clone())
            .collect();
        let plan = ember_core::DisplayReconciler::plan(
            &topology,
            &self.entries,
            &controlled_identities,
            true,
            &self.ambiguity_keys,
        );
        self.diagnostics.info(format!(
            "Reconciliation generation {}: {} action(s)",
            plan.generation,
            plan.actions.len()
        ));

        for action in plan.actions {
            if self.generation != generation {
                self.diagnostics
                    .warn("Aborting reconciliation: a newer topology generation arrived");
                self.reconcile_due = Some(Instant::now() + RECONCILE_DEBOUNCE);
                return;
            }
            match action {
                ember_core::ReconciliationAction::LeaveVerified { identity } => {
                    self.verified.insert(identity.key());
                    self.failed.remove(&identity.key());
                }
                ember_core::ReconciliationAction::ReapplyFromBaseline { identity } => {
                    let Some(entry) = self
                        .entries
                        .iter()
                        .find(|entry| entry.identity.matches(&identity))
                        .cloned()
                    else {
                        continue;
                    };
                    let expected = self.expected_table(&entry.display.gamma_table);
                    match self.backend.verify(&identity, &expected) {
                        VerifyReport::Verified | VerifyReport::AcceptedUnverified => {
                            self.verified.insert(identity.key());
                            self.failed.remove(&identity.key());
                            self.diagnostics.info(format!(
                                "Verified {} still controlled; left untouched",
                                identity.label()
                            ));
                        }
                        VerifyReport::Mismatch { delta_bits } => {
                            self.diagnostics.warn(format!(
                                "External reset detected on {} (delta {delta_bits}); reapplying from baseline",
                                identity.label()
                            ));
                            self.record_override(&identity, true);
                            match self.backend.apply(&identity, &expected) {
                                Ok(_) => {
                                    self.verified.insert(identity.key());
                                    self.failed.remove(&identity.key());
                                }
                                Err(error) => {
                                    self.failed.insert(identity.key(), error.to_string());
                                    self.verified.remove(&identity.key());
                                }
                            }
                        }
                        VerifyReport::Failed(message) => {
                            self.failed.insert(identity.key(), message.clone());
                            self.verified.remove(&identity.key());
                            self.diagnostics.error(format!(
                                "Verification failed for {}: {message}",
                                identity.label()
                            ));
                        }
                    }
                }
                ember_core::ReconciliationAction::JournalThenApply { identity } => {
                    let Some(info) = self
                        .displays
                        .iter()
                        .find(|info| info.identity.matches(&identity) && info.gamma_compatible())
                        .cloned()
                    else {
                        continue;
                    };
                    match self.backend.capture_baseline(&info.identity) {
                        Ok(baseline) => {
                            let entry = DisplayRecoveryEntry::without_hardware(
                                info.identity.clone(),
                                baseline,
                            );
                            self.entries.push(entry.clone());
                            // Journal before mutation.
                            let record = RecoveryRecord::new(
                                AppVersion::full(),
                                self.entries.clone(),
                                self.settings.clone(),
                            );
                            if let Err(error) = self.journal.save(&record) {
                                self.entries.pop();
                                self.failed.insert(identity.key(), error.to_string());
                                self.diagnostics.error(format!(
                                    "New display journal failed; leaving it untouched: {error}"
                                ));
                                continue;
                            }
                            let expected = self.expected_table(&entry.display.gamma_table);
                            match self.backend.apply(&identity, &expected) {
                                Ok(_) => {
                                    self.controlled.insert(identity.key());
                                    self.verified.insert(identity.key());
                                    self.failed.remove(&identity.key());
                                    self.diagnostics.info(format!(
                                        "Journaled then applied new display {}",
                                        info.connector
                                    ));
                                }
                                Err(error) => {
                                    let _ = self.backend.restore(&identity, &entry.display);
                                    self.failed.insert(identity.key(), error.to_string());
                                    self.verified.remove(&identity.key());
                                }
                            }
                        }
                        Err(error) => {
                            self.failed.insert(identity.key(), error.to_string());
                            self.diagnostics
                                .warn(format!("New display capture failed: {error}"));
                        }
                    }
                }
                ember_core::ReconciliationAction::ApplyPendingBaseline { identity } => {
                    let Some(entry) = self
                        .entries
                        .iter()
                        .find(|entry| entry.identity.matches(&identity))
                        .cloned()
                    else {
                        continue;
                    };
                    let expected = self.expected_table(&entry.display.gamma_table);
                    match self.backend.apply(&identity, &expected) {
                        Ok(_) => {
                            self.controlled.insert(identity.key());
                            self.verified.insert(identity.key());
                            self.failed.remove(&identity.key());
                            self.diagnostics.info(format!(
                                "Applied pending baseline for reconnected display {}",
                                identity.label()
                            ));
                        }
                        Err(error) => {
                            self.failed.insert(identity.key(), error.to_string());
                            self.verified.remove(&identity.key());
                        }
                    }
                }
                ember_core::ReconciliationAction::LeaveUnsupported { identity, reason } => {
                    self.diagnostics.info(format!(
                        "Unsupported display {} left untouched: {reason}",
                        identity.label()
                    ));
                }
                ember_core::ReconciliationAction::LeaveAmbiguous { reason } => {
                    self.diagnostics.warn(reason.clone());
                    self.last_error = Some(reason.clone());
                    self.attention = Some(AttentionState::new(
                        AttentionSeverity::Warning,
                        "Display identity is ambiguous",
                        reason,
                        vec![AttentionAction::Reset],
                    ));
                }
                ember_core::ReconciliationAction::RetainPending { .. } => {}
                ember_core::ReconciliationAction::RestoreSavedBaseline { .. } => {}
            }
        }

        self.persist_journal();
        let has_verified = !self.verified.is_empty();
        if !has_verified {
            let message = self
                .failed
                .values()
                .next()
                .cloned()
                .unwrap_or_else(|| "No verified display after reconciliation.".into());
            self.machine
                .transition(DisplayEvent::ReconciliationFailed(message.clone()));
            self.last_error = Some(message.clone());
            self.attention = Some(AttentionState::new(
                AttentionSeverity::Error,
                "Needs attention",
                message,
                vec![AttentionAction::Retry, AttentionAction::Reset],
            ));
        } else {
            if matches!(self.machine.state(), DisplayRuntimeState::Reconciling) {
                self.machine
                    .transition(DisplayEvent::ReconciliationSucceeded);
            }
            if self.failed.is_empty() {
                self.last_error = None;
                if self
                    .attention
                    .as_ref()
                    .is_some_and(|attention| attention.severity == AttentionSeverity::Warning)
                {
                    self.attention = None;
                }
            } else {
                let message = self.failed.values().cloned().collect::<Vec<_>>().join(" ");
                self.last_error = Some(message.clone());
                self.attention = Some(AttentionState::new(
                    AttentionSeverity::Warning,
                    "Partially active",
                    message,
                    vec![AttentionAction::Retry, AttentionAction::Reset],
                ));
            }
            if let Some(warning) = warning {
                self.diagnostics.warn(warning.clone());
                if self.attention.is_none() {
                    self.attention = Some(AttentionState::new(
                        AttentionSeverity::Warning,
                        "Display topology unstable",
                        warning,
                        vec![AttentionAction::Retry],
                    ));
                }
            }
            if self.settings.backlight_lock_enabled
                && self.backlight_runtime.state != BacklightRuntimeState::Engaged
            {
                self.engage_backlight();
            }
            self.schedule_post_verify();
        }
        self.refresh_backlight_capability();
        self.publish(true);
    }

    fn schedule_post_verify(&mut self) {
        let now = Instant::now();
        self.post_verify = POST_VERIFY_DELAYS
            .iter()
            .map(|delay| now + *delay)
            .collect();
    }

    fn record_override(&mut self, identity: &DisplayIdentity, repaired: bool) {
        let key = identity.key();
        let crossed = self.override_tracker.record_reset(&key, Utc::now());
        if crossed {
            let message = format!(
                "Another display service repeatedly replaced Ember's color table on {}. Automatic reapplication is paused.",
                identity.label()
            );
            self.diagnostics.error(message.clone());
            self.last_error = Some(message.clone());
            self.attention = Some(AttentionState::new(
                AttentionSeverity::Error,
                "Needs attention",
                message,
                vec![AttentionAction::Retry, AttentionAction::Reset],
            ));
            self.reapplies_suspended = true;
            self.machine.set_state(DisplayRuntimeState::Degraded(
                "Repeated external override".into(),
            ));
        }
        let _ = repaired;
    }

    fn verify_controlled(&mut self, kind: &str) {
        if !matches!(
            self.machine.state(),
            DisplayRuntimeState::Active | DisplayRuntimeState::Reconciling
        ) {
            return;
        }
        if !self.settings.filter_enabled || self.reapplies_suspended {
            return;
        }
        let entries = self.entries.clone();
        let mut repaired = 0usize;
        for entry in entries {
            let key = entry.identity.key();
            if !self.verified.contains(&key) && !self.controlled.contains(&key) {
                continue;
            }
            if !self
                .displays
                .iter()
                .any(|info| info.identity.matches(&entry.identity) && info.gamma_compatible())
            {
                continue;
            }
            let expected = self.expected_table(&entry.display.gamma_table);
            match self.backend.verify(&entry.identity, &expected) {
                VerifyReport::Verified | VerifyReport::AcceptedUnverified => {}
                VerifyReport::Mismatch { delta_bits } => {
                    self.diagnostics.warn(format!(
                        "Verification ({kind}) detected a reset on {} (delta {delta_bits}); reapplying once",
                        entry.identity.label()
                    ));
                    self.record_override(&entry.identity, true);
                    match self.backend.apply(&entry.identity, &expected) {
                        Ok(_) => {
                            repaired += 1;
                            self.verified.insert(key.clone());
                            self.failed.remove(&key);
                        }
                        Err(error) => {
                            self.failed.insert(key.clone(), error.to_string());
                            self.verified.remove(&key);
                        }
                    }
                }
                VerifyReport::Failed(message) => {
                    self.diagnostics.warn(format!(
                        "Verification ({kind}) unavailable for {}: {message}",
                        entry.identity.label()
                    ));
                }
            }
        }
        if repaired > 0 {
            self.diagnostics.info(format!(
                "Verification ({kind}) repaired {repaired} display(s)"
            ));
            self.persist_journal();
        }
        self.publish(false);
    }

    fn engage_backlight(&mut self) {
        if !self.settings.backlight_lock_enabled {
            self.backlight_runtime.state = BacklightRuntimeState::Disengaged;
            return;
        }
        let Some(info) = self
            .displays
            .iter()
            .find(|info| info.built_in && info.gamma_compatible())
            .cloned()
        else {
            self.backlight_runtime.state = BacklightRuntimeState::Unavailable;
            self.backlight_runtime.detail =
                Some("Backlight Lock target is not connected; preference retained.".into());
            return;
        };
        let capability = self.backlight.capability(&info.connector);
        self.backlight_runtime.capability = capability.clone();
        if !capability.available {
            self.backlight_runtime.state = BacklightRuntimeState::Unavailable;
            self.backlight_runtime.detail = capability.detail.clone();
            return;
        }

        // Ensure a hardware baseline is journaled before the first write.
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.identity.matches(&info.identity))
        else {
            self.backlight_runtime.state = BacklightRuntimeState::Unavailable;
            return;
        };
        if !self.entries[index].hardware.has_values() {
            match self.backlight.capture(&info.connector) {
                Ok(hardware) => {
                    self.entries[index].hardware = hardware;
                    let record = RecoveryRecord::new(
                        AppVersion::full(),
                        self.entries.clone(),
                        self.settings.clone(),
                    );
                    if let Err(error) = self.journal.save(&record) {
                        self.entries[index].hardware = Default::default();
                        self.backlight_runtime.state = BacklightRuntimeState::Failed;
                        self.backlight_runtime.detail = Some(error.to_string());
                        self.last_error = Some(format!(
                            "Backlight baseline could not be journaled: {error}"
                        ));
                        return;
                    }
                }
                Err(error) => {
                    self.backlight_runtime.state = BacklightRuntimeState::Unavailable;
                    self.backlight_runtime.detail = Some(error.to_string());
                    return;
                }
            }
        }

        match self.backlight.engage(&info.connector) {
            Ok(()) => {
                self.backlight_identity = Some(info.identity.clone());
                self.backlight_runtime.state = BacklightRuntimeState::Engaged;
                self.backlight_runtime.detail = None;
                self.backlight_failures = 0;
                self.diagnostics
                    .info("Backlight Lock engaged on the built-in display");
            }
            Err(error) => {
                self.backlight_runtime.state = BacklightRuntimeState::Failed;
                self.backlight_runtime.detail = Some(error.to_string());
                self.diagnostics
                    .warn(format!("Backlight engage failed: {error}"));
            }
        }
    }

    fn maintain_backlight(&mut self) {
        if !self.settings.backlight_lock_enabled
            || !matches!(self.machine.state(), DisplayRuntimeState::Active)
        {
            return;
        }
        let Some(info) = self
            .displays
            .iter()
            .find(|info| info.built_in && info.gamma_compatible())
            .cloned()
        else {
            if self.backlight_runtime.state == BacklightRuntimeState::Engaged {
                self.backlight_runtime.state = BacklightRuntimeState::Unavailable;
                self.backlight_runtime.detail =
                    Some("Backlight target unavailable; will retry.".into());
            }
            return;
        };
        if !self.backlight.needs_engagement(&info.connector) {
            self.backlight_failures = 0;
            if self.backlight_runtime.state != BacklightRuntimeState::Engaged
                && self.backlight_runtime.capability.available
            {
                self.backlight_runtime.state = BacklightRuntimeState::Engaged;
            }
            return;
        }
        match self.backlight.engage(&info.connector) {
            Ok(()) => {
                self.backlight_failures = 0;
                self.backlight_runtime.state = BacklightRuntimeState::Engaged;
                self.diagnostics
                    .info("Backlight guard corrected drift with one bounded write + verification");
            }
            Err(error) => {
                self.backlight_failures += 1;
                self.diagnostics.warn(format!(
                    "Backlight guard attempt {} failed: {error}",
                    self.backlight_failures
                ));
                if self.backlight_failures >= 3 {
                    self.backlight_failures = 0;
                    if let Some(index) = self
                        .entries
                        .iter()
                        .position(|entry| entry.identity.matches(&info.identity))
                    {
                        let hardware = self.entries[index].hardware.clone();
                        if self.backlight.restore(&info.connector, &hardware).is_ok() {
                            self.entries[index].hardware = Default::default();
                        }
                    }
                    self.backlight_runtime.state = BacklightRuntimeState::Failed;
                    self.backlight_runtime.detail = Some(
                        "Backlight Lock stopped after three system-level write failures.".into(),
                    );
                    self.last_error =
                        Some("Backlight Lock stopped after three write failures.".into());
                    self.attention = Some(AttentionState::new(
                        AttentionSeverity::Warning,
                        "Backlight Lock stopped",
                        "The built-in display could not keep full hardware brightness. Your saved brightness is retained for restoration.",
                        vec![AttentionAction::Retry],
                    ));
                    self.persist_journal();
                }
            }
        }
        self.publish(false);
    }
}

// MARK: - Solar schedule

impl Coordinator {
    fn ensure_location(&mut self, force: bool) {
        if !self.settings.sun_schedule_enabled || self.solar.refreshing {
            return;
        }
        if !force {
            if let Some(cached) = &self.settings.solar_location {
                let age = Utc::now().signed_duration_since(cached.captured_at);
                if age < chrono::Duration::hours(6) {
                    self.solar.location_state = LocationState::Granted;
                    return;
                }
            }
        }
        self.solar.refreshing = true;
        self.solar.location_detail = None;
        self.publish(true);
        let tx = self.command_tx.clone();
        std::thread::Builder::new()
            .name("ember-geoclue".into())
            .spawn(move || {
                let mut provider = GeoClueProvider::new();
                let result = provider.request();
                let _ = tx.send(DaemonCommand::LocationResult(result));
            })
            .ok();
    }

    fn handle_location_result(&mut self, result: Result<CachedSolarLocation, LocationError>) {
        self.solar.refreshing = false;
        match result {
            Ok(fix) => {
                self.diagnostics.info(format!(
                    "Solar location updated (rounded): {:.1}, {:.1}",
                    fix.coordinate.latitude, fix.coordinate.longitude
                ));
                self.settings.solar_location = Some(fix);
                self.solar.location_state = LocationState::Granted;
                self.solar.location_detail = None;
                self.solar.next_retry = None;
                self.save_settings();
                self.recompute_solar(true);
            }
            Err(error) => {
                self.diagnostics
                    .warn(format!("Location request failed: {error:?}"));
                self.solar.location_detail = Some(error.message());
                if error.is_denied() {
                    self.solar.location_state = LocationState::Denied;
                    if self.settings.solar_location.is_none() {
                        self.settings.sun_schedule_enabled = false;
                        self.settings.automation_override = None;
                        self.solar.schedule = None;
                        self.save_settings();
                        self.attention = Some(AttentionState::new(
                            AttentionSeverity::Warning,
                            "Location unavailable",
                            "Sun schedule was turned off because location access was denied. Use Set Location to enter coordinates manually.",
                            vec![AttentionAction::OpenLocation],
                        ));
                    }
                } else {
                    self.solar.location_state = if self.settings.solar_location.is_some() {
                        LocationState::Granted
                    } else {
                        LocationState::Unavailable
                    };
                    self.solar.next_retry = Some(Instant::now() + LOCATION_RETRY);
                    self.attention = Some(AttentionState::new(
                        AttentionSeverity::Info,
                        "Waiting for location",
                        error.message(),
                        vec![AttentionAction::OpenLocation, AttentionAction::Retry],
                    ));
                }
                self.save_settings();
                self.recompute_solar(false);
            }
        }
        self.publish(true);
    }

    fn set_manual_location(&mut self, latitude: f64, longitude: f64) {
        if !latitude.is_finite() || !longitude.is_finite() {
            self.last_error = Some("Coordinates must be finite numbers.".into());
            self.publish(true);
            return;
        }
        let coordinate = SolarCoordinate::new(latitude, longitude).rounded(1);
        if !coordinate.is_valid() {
            self.last_error = Some("Coordinates are out of range.".into());
            self.publish(true);
            return;
        }
        self.settings.solar_location = Some(CachedSolarLocation::new(coordinate, Utc::now()));
        self.solar.location_state = LocationState::Granted;
        self.solar.location_detail = None;
        self.solar.next_retry = None;
        self.attention = None;
        self.save_settings();
        self.diagnostics
            .info("Manual location set (rounded to 0.1°)");
        self.recompute_solar(true);
        self.publish(true);
    }

    fn clear_location(&mut self) {
        self.settings.solar_location = None;
        self.solar.location_state = if self.settings.sun_schedule_enabled {
            LocationState::NeedsLocation
        } else {
            LocationState::Disabled
        };
        self.solar.schedule = None;
        self.solar.location_detail = Some("No location is stored.".into());
        self.save_settings();
        self.publish(true);
    }

    fn recompute_solar(&mut self, apply: bool) {
        self.solar.timezone = SolarCalculator::local_timezone().to_string();
        let now = Utc::now();
        if let Some(override_value) = &self.settings.automation_override {
            if override_value.expires_at <= now {
                self.settings.automation_override = None;
                self.save_settings();
            }
        }
        if !self.settings.sun_schedule_enabled {
            self.solar.schedule = None;
            self.solar.location_state = LocationState::Disabled;
            self.publish(apply);
            return;
        }
        let Some(location) = self.settings.solar_location else {
            self.solar.schedule = None;
            if !self.solar.refreshing {
                self.solar.location_state = LocationState::NeedsLocation;
            }
            self.publish(apply);
            return;
        };
        let tz = SolarCalculator::local_timezone();
        let schedule = SolarCalculator::schedule(now, location.coordinate, tz);
        self.solar.schedule = Some(schedule.clone());
        self.solar.location_state = LocationState::Granted;
        if !apply || self.solar.refreshing {
            self.publish(apply);
            return;
        }
        if self
            .settings
            .automation_override
            .as_ref()
            .is_some_and(|value| value.is_active_at(now))
        {
            self.publish(false);
            return;
        }
        let desired = schedule.is_night;
        if desired != self.settings.filter_enabled {
            self.diagnostics.info(format!(
                "Solar schedule boundary: {} (is_night={desired})",
                schedule
                    .next_event
                    .map(|event| ember_core::SolarCalculator::format_event(&event, tz))
                    .unwrap_or_else(|| "no event".into())
            ));
            if desired {
                self.settings.filter_enabled = true;
                self.save_settings();
                self.activate();
            } else {
                self.settings.filter_enabled = false;
                self.save_settings();
                self.restore_display(RestoreIntent::Disable, true);
            }
        }
        self.publish(true);
    }

    fn solar_tick(&mut self) {
        if !self.settings.sun_schedule_enabled {
            return;
        }
        let now_wall = Utc::now();
        let jumped = (now_wall - self.solar.last_wall).num_seconds().abs() > 120;
        self.solar.last_wall = now_wall;
        if jumped {
            self.diagnostics
                .info("Wall clock changed; recalculating the solar schedule");
        }
        if let Some(location) = &self.settings.solar_location {
            let age = now_wall.signed_duration_since(location.captured_at);
            if age > chrono::Duration::hours(6) && !self.solar.refreshing {
                self.ensure_location(true);
            }
        } else if !self.solar.refreshing && self.solar.next_retry.is_none() {
            self.ensure_location(true);
        }
        self.recompute_solar(true);
        self.publish(false);
    }
}
