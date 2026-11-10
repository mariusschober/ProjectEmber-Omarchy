//! Regression test for the Omarchy bar race: `emberd` starts from autostart
//! before `omarchy-shell` has exported `org.kde.StatusNotifierWatcher`.
//!
//! The tray must stay alive, register when the watcher appears, and register
//! again when the shell restarts.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ember_core::{Paths, Snapshot};
use ember_daemon::coordinator::{DaemonBuilder, DaemonConfig, DaemonHandle};
use ember_daemon::simulate::{SharedBacklight, SharedSim};
use ember_platform::{SimBacklight, SimulatedBackend};
use zbus::Connection;

struct TestBus {
    child: Child,
    address: String,
}

impl TestBus {
    fn start() -> Self {
        let mut child = Command::new("dbus-daemon")
            .args([
                "--session",
                "--nofork",
                "--print-address=1",
                "--print-pid=1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("dbus-daemon is required for this test");
        let stdout = child.stdout.take().expect("dbus address");
        let mut reader = BufReader::new(stdout);
        let mut address = String::new();
        reader.read_line(&mut address).unwrap();
        let mut pid = String::new();
        reader.read_line(&mut pid).unwrap();
        let address = address.trim().to_string();
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
        Self { child, address }
    }
}

impl Drop for TestBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::env::var("DBUS_SESSION_BUS_ADDRESS").as_deref() == Ok(&self.address) {
            std::env::remove_var("DBUS_SESSION_BUS_ADDRESS");
        }
    }
}

#[derive(Default)]
struct Watcher {
    items: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    fn register_status_notifier_item(&self, service: String) {
        self.items.lock().unwrap().push(service);
    }

    fn register_status_notifier_host(&self, _service: String) {}

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().unwrap().clone()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }
}

async fn start_watcher() -> (Connection, Arc<Mutex<Vec<String>>>) {
    let watcher = Watcher::default();
    let items = watcher.items.clone();
    let connection = zbus::connection::Builder::session()
        .expect("session bus")
        .name("org.kde.StatusNotifierWatcher")
        .expect("watcher name")
        .serve_at("/StatusNotifierWatcher", watcher)
        .expect("serve watcher")
        .build()
        .await
        .expect("build watcher");
    (connection, items)
}

fn wait_for_item(items: &Arc<Mutex<Vec<String>>>, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(item) = items.lock().unwrap().first().cloned() {
            return Some(item);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn spawn_daemon(paths: &Paths) -> DaemonHandle {
    DaemonBuilder::new(DaemonConfig {
        paths: paths.clone(),
        headless: false,
        simulate: true,
        safe_mode: false,
        start_active: false,
    })
    .with_backend(Box::new(SharedSim::new(SimulatedBackend::new())))
    .with_backlight(Box::new(SharedBacklight::new(SimBacklight::new())))
    .with_tray(true)
    .spawn()
    .expect("daemon")
}

#[test]
fn tray_waits_for_late_watcher_and_reregisters_after_shell_restart() {
    let _bus = TestBus::start();
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(
        dir.path().join("config"),
        dir.path().join("state"),
        dir.path().join("run"),
    );
    for path in [paths.config_dir(), paths.state_dir(), paths.runtime_dir()] {
        ember_core::fsutil::ensure_private_dir(path).unwrap();
    }

    let mut daemon = spawn_daemon(&paths);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    // The daemon must already be up while there is no watcher at all.
    std::thread::sleep(Duration::from_millis(300));
    let snapshot: Snapshot = daemon.snapshot();
    assert_eq!(snapshot.backend.name, "simulator");

    // Omarchy's shell starts later and exports the watcher.
    let (watcher, items) = runtime.block_on(start_watcher());
    let first = wait_for_item(&items, Duration::from_secs(5))
        .expect("tray did not register after the watcher appeared");
    assert!(
        first.contains("StatusNotifierItem") || first.contains("project-ember"),
        "unexpected SNI registration string: {first}"
    );

    // Restart the shell: the watcher disappears and a new one appears.
    drop(watcher);
    std::thread::sleep(Duration::from_millis(300));
    let (watcher, items_after) = runtime.block_on(start_watcher());
    let second = wait_for_item(&items_after, Duration::from_secs(5))
        .expect("tray did not re-register after the watcher restarted");
    assert!(
        second.contains("StatusNotifierItem") || second.contains("project-ember"),
        "unexpected re-registration string: {second}"
    );
    drop(watcher);
    daemon.shutdown();
}
