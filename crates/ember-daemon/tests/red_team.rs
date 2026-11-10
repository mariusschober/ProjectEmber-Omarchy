//! Adversarial / red-team tests for the daemon boundary.
//!
//! These are intentionally hostile inputs: they assert that safety invariants
//! fail closed, not that the happy path works.

use std::time::Duration;

use ember_core::{
    AppVersion, DisplayBaseline, DisplayRecoveryEntry, GammaTable, Paths, RecoveryJournal,
    RecoveryRecord, SettingsStore,
};
use ember_daemon::coordinator::{DaemonBuilder, DaemonCommand, DaemonConfig, InstanceLock};
use ember_daemon::simulate::{SharedBacklight, SharedSim};
use ember_platform::{SimBacklight, SimDisplay, SimulatedBackend};

fn setup(tag: &str) -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(
        dir.path().join(tag).join("config"),
        dir.path().join(tag).join("state"),
        dir.path().join(tag).join("run"),
    );
    for path in [paths.config_dir(), paths.state_dir(), paths.runtime_dir()] {
        ember_core::fsutil::ensure_private_dir(path).unwrap();
    }
    (dir, paths)
}

fn spawn(paths: &Paths, sim: SharedSim, backlight: SharedBacklight) -> ember_daemon::DaemonHandle {
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
    .unwrap()
}

#[test]
fn single_instance_lock_is_exclusive() {
    let (_dir, paths) = setup("lock");
    let first = InstanceLock::acquire(&paths).expect("first lock");
    let second = InstanceLock::acquire(&paths);
    assert!(matches!(
        second,
        Err(ember_core::EmberError::AlreadyRunning)
    ));
    drop(first);
    let third = InstanceLock::acquire(&paths);
    assert!(third.is_ok(), "lock must be released on drop");
}

#[test]
fn future_schema_is_quarantined_and_no_display_is_mutated() {
    let (_dir, paths) = setup("future");
    let sim = SharedSim::new(SimulatedBackend::single());
    let backlight = SharedBacklight::new(SimBacklight::new());
    // A future journal must not be treated as empty and must not be cleared.
    std::fs::write(
        paths.journal_file(),
        br#"{"schemaVersion":99,"appVersion":"future","createdAt":1.0,"displays":[],"intendedSettings":{}}"#,
    )
    .unwrap();
    let mut daemon = spawn(&paths, sim.clone(), backlight);
    let ready = daemon.wait_for_snapshot(Duration::from_secs(5), |snapshot| {
        snapshot.backend.name == "simulator" && !snapshot.attention.message.is_empty()
    });
    assert!(ready.is_some(), "future schema must surface attention");
    let snapshot = daemon.snapshot();
    assert!(
        snapshot.attention.message.contains("schema")
            || snapshot.attention.message.contains("newer"),
        "unexpected attention: {}",
        snapshot.attention.message
    );
    assert!(paths.journal_file().exists(), "journal must be preserved");
    let current = sim.with(|backend| backend.table("eDP-1")).unwrap();
    let baseline = sim.with(|backend| backend.baseline("eDP-1")).unwrap();
    assert_eq!(
        current.maximum_absolute_difference(&baseline),
        0.0,
        "no display may be mutated when the journal is unreadable"
    );
    daemon.shutdown();
}

#[test]
fn corrupted_journal_is_quarantined_not_deleted() {
    let (_dir, paths) = setup("corrupt");
    std::fs::write(paths.journal_file(), b"\x00\x01not-json").unwrap();
    let sim = SharedSim::new(SimulatedBackend::single());
    let backlight = SharedBacklight::new(SimBacklight::new());
    let mut daemon = spawn(&paths, sim, backlight);
    let ready = daemon.wait_for_snapshot(Duration::from_secs(5), |snapshot| {
        snapshot.backend.name == "simulator" && !snapshot.attention.message.is_empty()
    });
    assert!(ready.is_some());
    assert!(!paths.journal_file().exists(), "journal is moved aside");
    let preserved = std::fs::read_dir(paths.state_dir())
        .unwrap()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().contains("corrupt"));
    assert!(preserved, "corrupt bytes must be preserved for inspection");
    daemon.shutdown();
}

#[test]
fn restore_failure_keeps_journal_and_reports_degraded() {
    let (_dir, paths) = setup("restore");
    let sim = SharedSim::new(SimulatedBackend::single());
    let backlight = SharedBacklight::new(SimBacklight::new());
    let mut daemon = spawn(&paths, sim.clone(), backlight);
    daemon.send(DaemonCommand::SetEnabled(true));
    let active =
        daemon.wait_for_snapshot(Duration::from_secs(5), |snapshot| snapshot.observed_active);
    assert!(active.is_some());

    sim.with(|backend| backend.fail_restore("eDP-1"));
    daemon.send(DaemonCommand::SetEnabled(false));
    let degraded = daemon.wait_for_snapshot(Duration::from_secs(5), |snapshot| {
        snapshot.runtime_state == ember_core::EmberOperationState::Degraded
    });
    assert!(
        degraded.is_some(),
        "restore failure must be degraded, not silently off"
    );
    assert!(
        paths.journal_file().exists(),
        "journal must be retained until restoration is verified"
    );
    daemon.shutdown();
}

#[test]
fn pending_display_is_restored_on_reconnect_without_blanking_survivor() {
    let (_dir, paths) = setup("reconnect");
    let sim = SharedSim::new(SimulatedBackend::new());
    let backlight = SharedBacklight::new(SimBacklight::new());
    let mut daemon = spawn(&paths, sim.clone(), backlight);
    daemon.send(DaemonCommand::SetEnabled(true));
    assert!(daemon
        .wait_for_snapshot(Duration::from_secs(5), |snapshot| {
            snapshot.observed_active
        })
        .is_some());

    // Keep a copy of the surviving display's warm table, then unplug the other.
    let survivor_warm = sim.with(|backend| backend.table("eDP-1")).unwrap();
    sim.with(|backend| backend.unplug("DP-1"));
    assert!(daemon
        .wait_for_snapshot(Duration::from_secs(5), |snapshot| {
            snapshot.counts.pending == 1 && snapshot.counts.verified == 1
        })
        .is_some());
    let survivor_after = sim.with(|backend| backend.table("eDP-1")).unwrap();
    assert_eq!(
        survivor_after.maximum_absolute_difference(&survivor_warm),
        0.0,
        "unplugging another display must not restore/blank the survivor"
    );

    sim.with(|backend| backend.plug(SimDisplay::new("DP-1", false, 4096)));
    assert!(daemon
        .wait_for_snapshot(Duration::from_secs(6), |snapshot| {
            snapshot.counts.pending == 0 && snapshot.counts.verified == 2
        })
        .is_some());
    daemon.send(DaemonCommand::SetEnabled(false));
    let _ = daemon.wait_for_snapshot(Duration::from_secs(5), |snapshot| {
        !snapshot.settings.filter_enabled
    });
    daemon.shutdown();
}

#[test]
fn oversized_journal_is_rejected_without_allocation() {
    let (_dir, paths) = setup("oversize");
    let mut file = std::fs::File::create(paths.journal_file()).unwrap();
    use std::io::Write;
    file.write_all(&vec![b'x'; 2 * 1024 * 1024]).unwrap();
    drop(file);
    let journal = RecoveryJournal::new(paths.journal_file());
    assert!(matches!(
        journal.load_outcome(),
        ember_core::JournalLoadOutcome::Corrupt(_)
    ));
}

#[test]
fn journal_never_resolves_to_a_different_display() {
    let (_dir, paths) = setup("identity");
    let sim = SharedSim::new(SimulatedBackend::single());
    let backlight = SharedBacklight::new(SimBacklight::new());
    // Journal an identity that is not the simulated panel (same connector,
    // different serial would be a different physical display).
    let other = ember_core::DisplayIdentity::new(
        Some("edid:dead:beef:1".into()),
        0xDEAD,
        0xBEEF,
        1,
        0,
        true,
        None,
        "eDP-1".into(),
    );
    let entry = DisplayRecoveryEntry::without_hardware(
        other.clone(),
        DisplayBaseline::new(
            1,
            Some(other),
            GammaTable::identity(4096),
            chrono::Utc::now(),
        ),
    );
    let settings = SettingsStore::new(paths.settings_file());
    let value = ember_core::EmberSettings {
        filter_enabled: true,
        ..Default::default()
    };
    settings.save(&value).unwrap();
    RecoveryJournal::new(paths.journal_file())
        .save(&RecoveryRecord::new(AppVersion::full(), vec![entry], value))
        .unwrap();
    let mut daemon = spawn(&paths, sim.clone(), backlight);
    let ready = daemon.wait_for_snapshot(Duration::from_secs(5), |snapshot| {
        snapshot.backend.name == "simulator"
    });
    assert!(ready.is_some());
    // The journaled identity must remain pending, never applied to eDP-1.
    let snapshot = daemon.snapshot();
    assert!(
        snapshot.pending_restore_count >= 1,
        "foreign identity must stay pending"
    );
    let current = sim.with(|backend| backend.table("eDP-1")).unwrap();
    let baseline = sim.with(|backend| backend.baseline("eDP-1")).unwrap();
    assert_eq!(
        current.maximum_absolute_difference(&baseline),
        0.0,
        "baseline must not be redirected to a different physical display"
    );
    daemon.shutdown();
}
