//! Private-session-bus integration test for the daemon's public API.
//!
//! It starts a real `dbus-daemon`, a real coordinator with the simulated
//! backend, and drives state through the same D-Bus methods the GUI and CLI use.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ember_core::{Paths, Snapshot};
use ember_daemon::coordinator::{DaemonBuilder, DaemonCommand, DaemonConfig};
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

#[test]
fn dbus_control_round_trip() {
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

    let sim = SharedSim::new(SimulatedBackend::new());
    let backlight = SharedBacklight::new(SimBacklight::new());
    let mut daemon = DaemonBuilder::new(DaemonConfig {
        paths: paths.clone(),
        headless: true,
        simulate: true,
        safe_mode: false,
        start_active: false,
    })
    .with_backend(Box::new(sim.clone()))
    .with_backlight(Box::new(backlight))
    .with_tray(false)
    .spawn()
    .unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let connection = runtime.block_on(async {
        let command_sender = daemon.command_sender();
        let snapshots = daemon.subscribe();
        let _service = ember_daemon::dbus::serve(command_sender, snapshots)
            .await
            .expect("serve D-Bus");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(connection) = Connection::session().await {
                let proxy = zbus::Proxy::new(
                    &connection,
                    "org.mariusschober.ProjectEmber",
                    "/org/mariusschober/ProjectEmber",
                    "org.mariusschober.ProjectEmber",
                )
                .await;
                if let Ok(proxy) = proxy {
                    if proxy.call::<_, _, String>("GetVersion", &()).await.is_ok() {
                        break connection;
                    }
                }
            }
            if Instant::now() >= deadline {
                panic!("daemon D-Bus name never appeared");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    let proxy = runtime
        .block_on(async {
            zbus::Proxy::new(
                &connection,
                "org.mariusschober.ProjectEmber",
                "/org/mariusschober/ProjectEmber",
                "org.mariusschober.ProjectEmber",
            )
            .await
        })
        .unwrap();

    let snapshot = |runtime: &tokio::runtime::Runtime, proxy: &zbus::Proxy<'_>| {
        let text: String = runtime
            .block_on(proxy.call("GetSnapshot", &()))
            .expect("snapshot");
        Snapshot::from_json(&text).expect("snapshot json")
    };

    // Wait for the coordinator to finish startup before asserting on state.
    let ready_deadline = Instant::now() + Duration::from_secs(5);
    let initial = loop {
        let state = snapshot(&runtime, &proxy);
        if state.backend.name == "simulator" && !state.displays.is_empty() {
            break state;
        }
        assert!(
            Instant::now() < ready_deadline,
            "daemon did not finish startup"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(!initial.settings.filter_enabled);

    runtime.block_on(async {
        proxy
            .call::<_, _, ()>("SetEnabled", &(true,))
            .await
            .unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = snapshot(&runtime, &proxy);
        if state.observed_active {
            assert_eq!(state.counts.verified, 2);
            break;
        }
        assert!(Instant::now() < deadline, "activation timed out");
        std::thread::sleep(Duration::from_millis(100));
    }

    runtime.block_on(async {
        proxy
            .call::<_, _, ()>("SetWarmth", &(0.81_f64))
            .await
            .unwrap();
        proxy
            .call::<_, _, ()>("SetBrightness", &(0.55_f64))
            .await
            .unwrap();
        proxy
            .call::<_, _, ()>("ApplyPreset", &("pureRed",))
            .await
            .unwrap();
        let error = proxy.call::<_, _, ()>("SetWarmth", &(f64::NAN,)).await;
        assert!(error.is_err(), "NaN warmth must be rejected");
        let error = proxy.call::<_, _, ()>("SetBrightness", &(0.02_f64,)).await;
        assert!(error.is_err(), "out-of-range brightness must be rejected");
        let error = proxy
            .call::<_, _, ()>("ApplyPreset", &("not-a-preset",))
            .await;
        assert!(error.is_err(), "unknown preset must be rejected");
        let diagnostics: String = proxy.call("ExportDiagnostics", &()).await.unwrap();
        assert!(diagnostics.contains("Project Ember diagnostics"));
        assert!(!diagnostics.contains("secret"));
    });

    let state = snapshot(&runtime, &proxy);
    assert!((state.settings.warmth - 1.0).abs() < 0.001);
    assert!((state.settings.apparent_brightness - 0.55).abs() < 0.001);

    runtime.block_on(async {
        proxy
            .call::<_, _, ()>("SetEnabled", &(false,))
            .await
            .unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = snapshot(&runtime, &proxy);
        if !state.settings.filter_enabled && state.pending_restore_count == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "disable timed out");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!paths.journal_file().exists(), "journal must be cleared");

    // A second daemon cannot take the same name.
    let duplicate = DaemonBuilder::new(DaemonConfig {
        paths: paths.clone(),
        headless: true,
        simulate: true,
        safe_mode: true,
        start_active: false,
    })
    .with_backend(Box::new(SharedSim::new(SimulatedBackend::single())))
    .with_backlight(Box::new(SharedBacklight::new(SimBacklight::new())))
    .with_tray(false)
    .spawn();
    if let Ok(mut duplicate) = duplicate {
        duplicate.send(DaemonCommand::Shutdown);
        duplicate.shutdown();
    }

    daemon.shutdown();
    drop(connection);
}
