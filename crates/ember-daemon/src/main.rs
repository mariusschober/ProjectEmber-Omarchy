use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use ember_core::Paths;
use ember_daemon::coordinator::{
    prepare_paths, probe_backend, version_string, DaemonBuilder, DaemonConfig, InstanceLock,
};
use log::{info, warn};

#[derive(Parser, Debug)]
#[command(
    name = "emberd",
    version,
    about = "Project Ember daemon — circadian display control for Omarchy",
    disable_version_flag = true
)]
struct Args {
    /// Run a deterministic simulation scenario and exit.
    #[arg(long, value_name = "SCENARIO", num_args = 0..=1, default_missing_value = "all")]
    simulate: Option<String>,

    /// Do not create a StatusNotifierItem tray icon.
    #[arg(long)]
    no_tray: bool,

    /// Start without applying the saved display preference (recovery still runs).
    #[arg(long)]
    safe_mode: bool,

    /// Print a read-only backend probe as JSON and exit.
    #[arg(long)]
    probe: bool,

    /// Print version information and exit.
    #[arg(long)]
    version: bool,

    /// Internal: used by the XDG autostart entry. Accepted for compatibility.
    #[arg(long, hide = true)]
    autostart: bool,
}

fn main() -> anyhow::Result<()> {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let args = Args::parse();
    let _autostart = args.autostart;
    if args.version {
        println!("{}", version_string());
        return Ok(());
    }
    if args.probe {
        match probe_backend() {
            Ok((backend, displays)) => {
                let value = serde_json::json!({
                    "ok": true,
                    "backend": backend,
                    "displays": displays.iter().map(|info| serde_json::json!({
                        "connector": info.connector,
                        "builtIn": info.built_in,
                        "gammaSize": info.gamma_size,
                        "gammaCompatible": info.gamma_compatible(),
                        "mirrored": info.is_mirrored,
                        "identity": info.identity.key(),
                        "label": info.identity.label(),
                        "unsupportedReason": info.unsupported_reason,
                    })).collect::<Vec<_>>(),
                });
                println!("{}", serde_json::to_string_pretty(&value)?);
                return Ok(());
            }
            Err(error) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "ok": false,
                        "error": error.to_string(),
                    }))?
                );
                std::process::exit(2);
            }
        }
    }

    if let Some(scenario) = args.simulate {
        let report = ember_daemon::simulate::run(&scenario)?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        std::process::exit(if report.passed { 0 } else { 1 });
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let paths = Paths::from_env();
    prepare_paths(&paths).context("prepare state directories")?;

    let _lock = match InstanceLock::acquire(&paths) {
        Ok(lock) => lock,
        Err(ember_core::EmberError::AlreadyRunning) => {
            println!("Project Ember is already running.");
            return Ok(());
        }
        Err(error) => return Err(error).context("acquire single-instance lock"),
    };

    info!("{}", version_string());
    let config = DaemonConfig {
        paths: paths.clone(),
        headless: false,
        simulate: false,
        safe_mode: args.safe_mode,
        start_active: false,
    };
    let mut handle = DaemonBuilder::new(config)
        .with_tray(!args.no_tray)
        .spawn()
        .context("start coordinator")?;

    let snapshots = handle.subscribe();
    let mut finished = handle.finished();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let command_tx = handle.command_sender();
        let connection = match ember_daemon::dbus::serve(command_tx, snapshots).await {
            Ok(connection) => Some(connection),
            Err(error) => {
                warn!("D-Bus service unavailable: {error}");
                None
            }
        };

        let daemon_finished = async {
            while finished.changed().await.is_ok() {
                if *finished.borrow() {
                    return;
                }
            }
        };
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        let terminate = async {
            let mut signal =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            signal.recv().await;
        };
        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => info!("SIGINT received"),
            _ = terminate => info!("SIGTERM received"),
            _ = daemon_finished => info!("daemon requested shutdown"),
        }
        drop(connection);
    });

    std::thread::sleep(Duration::from_millis(50));
    handle.shutdown();
    Ok(())
}
