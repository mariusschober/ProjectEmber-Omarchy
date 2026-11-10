use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context};
use clap::{Parser, Subcommand};
use ember_core::{EmberPreset, Snapshot, TrayPrimaryAction};
use ember_daemon::dbus::{BUS_NAME, INTERFACE, OBJECT_PATH};
use zbus::Connection;

#[derive(Parser, Debug)]
#[command(
    name = "emberctl",
    version,
    about = "Control Project Ember from Omarchy keybindings, scripts or a terminal",
    disable_version_flag = true
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Print version and exit.
    #[arg(long)]
    version: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Show the current state.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Turn Ember on.
    On,
    /// Turn Ember off (restore original display state).
    Off,
    /// Toggle Ember.
    Toggle,
    /// Set warmth (0–100 %; 100 is Pure Red).
    Warmth { percent: f64 },
    /// Set software brightness (10–100 %).
    Brightness { percent: f64 },
    /// Apply a preset.
    Preset { name: String },
    /// Enable or disable Backlight Lock.
    Backlight { state: String },
    /// Enable or disable the Sun schedule.
    Schedule { state: String },
    /// Enable or disable Launch at login.
    Launch { state: String },
    /// Change what a tray left-click does.
    TrayAction { action: String },
    /// Set or clear the manual Sun schedule location.
    Location {
        #[command(subcommand)]
        command: LocationCommand,
    },
    /// Retry verification/activation.
    Retry,
    /// Restore every display and clear preferences.
    Reset,
    /// Restore saved display state now.
    Restore,
    /// Re-read displays and capabilities.
    Refresh,
    /// Open the control panel.
    Open,
    /// Show diagnostics.
    Diagnostics {
        /// Write the diagnostics text to a file instead of stdout.
        #[arg(long)]
        export: Option<PathBuf>,
    },
    /// Read-only display/backend probe (no daemon required).
    Probe {
        #[arg(long)]
        json: bool,
    },
    /// Run a deterministic simulation scenario.
    Simulate { scenario: String },
    /// Stop the daemon (restores displays first).
    Quit,
    /// Print version information.
    Version,
}

#[derive(Subcommand, Debug)]
enum LocationCommand {
    Set { latitude: f64, longitude: f64 },
    Clear,
}

fn main() -> anyhow::Result<()> {
    // A CLI that is piped into `head` must exit quietly on a closed stdout,
    // not panic with a broken-pipe backtrace.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let args = Args::parse();
    if args.version || matches!(args.command, Some(Command::Version)) {
        println!("{}", ember_daemon::coordinator::version_string());
        return Ok(());
    }
    let command = args.command.unwrap_or(Command::Status { json: false });

    match command {
        Command::Probe { json } => probe(json),
        Command::Simulate { scenario } => simulate(&scenario),
        Command::Version => Ok(()),
        other => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(run(other))
        }
    }
}

fn probe(json: bool) -> anyhow::Result<()> {
    let (backend, displays) = ember_daemon::coordinator::probe_backend()?;
    if json {
        let value = serde_json::json!({
            "ok": true,
            "backend": backend,
            "displays": displays.iter().map(|info| serde_json::json!({
                "connector": info.connector,
                "label": info.identity.label(),
                "identity": info.identity.key(),
                "builtIn": info.built_in,
                "gammaSize": info.gamma_size,
                "gammaCompatible": info.gamma_compatible(),
                "mirrored": info.is_mirrored,
                "unsupportedReason": info.unsupported_reason,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("Backend: {backend}");
        for info in displays {
            println!(
                "{}  gamma={}  {}  {}",
                info.connector,
                info.gamma_size,
                if info.built_in {
                    "built-in"
                } else {
                    "external"
                },
                info.unsupported_reason
                    .unwrap_or_else(|| "compatible".into())
            );
        }
    }
    Ok(())
}

fn simulate(scenario: &str) -> anyhow::Result<()> {
    let report = ember_daemon::simulate::run(scenario)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    std::process::exit(if report.passed { 0 } else { 1 });
}

async fn run(command: Command) -> anyhow::Result<()> {
    match command {
        Command::Status { json } => {
            let mut snapshot = fetch_snapshot(true).await?;
            // `emberctl status` may be the call that activates the daemon over
            // D-Bus. Give startup a short, bounded window instead of printing
            // a misleading "not running" snapshot.
            for _ in 0..20 {
                if snapshot.backend.name != "none" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                snapshot = fetch_snapshot(false).await?;
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                print_status(&snapshot);
            }
            Ok(())
        }
        Command::Diagnostics { export } => {
            let connection = connection_with_daemon().await?;
            let proxy = proxy(&connection).await?;
            let text: String = proxy
                .call("ExportDiagnostics", &())
                .await
                .context("export diagnostics")?;
            match export {
                Some(path) => {
                    std::fs::write(&path, text)
                        .with_context(|| format!("write {}", path.display()))?;
                    println!("Wrote diagnostics to {}", path.display());
                }
                None => println!("{text}"),
            }
            Ok(())
        }
        other => {
            let connection = connection_with_daemon().await?;
            let proxy = proxy(&connection).await?;
            match other {
                Command::On => call(&proxy, "SetEnabled", &(true,)).await?,
                Command::Off => call(&proxy, "SetEnabled", &(false,)).await?,
                Command::Toggle => call(&proxy, "Toggle", &()).await?,
                Command::Warmth { percent } => {
                    if !(0.0..=100.0).contains(&percent) {
                        return Err(anyhow!("warmth must be between 0 and 100"));
                    }
                    call(&proxy, "SetWarmth", &(percent / 100.0,)).await?;
                }
                Command::Brightness { percent } => {
                    if !(10.0..=100.0).contains(&percent) {
                        return Err(anyhow!("brightness must be between 10 and 100"));
                    }
                    call(&proxy, "SetBrightness", &(percent / 100.0,)).await?;
                }
                Command::Preset { name } => {
                    let preset: EmberPreset =
                        name.parse().map_err(|error: String| anyhow!(error))?;
                    call(&proxy, "ApplyPreset", &(preset.as_str(),)).await?;
                }
                Command::Backlight { state } => {
                    call(&proxy, "SetBacklightLock", &(parse_on_off(&state)?,)).await?
                }
                Command::Schedule { state } => {
                    call(&proxy, "SetSunSchedule", &(parse_on_off(&state)?,)).await?
                }
                Command::Launch { state } => {
                    call(&proxy, "SetLaunchAtLogin", &(parse_on_off(&state)?,)).await?
                }
                Command::TrayAction { action } => {
                    let action: TrayPrimaryAction =
                        action.parse().map_err(|error: String| anyhow!(error))?;
                    call(&proxy, "SetTrayPrimaryAction", &(action.as_str(),)).await?;
                }
                Command::Location { command } => match command {
                    LocationCommand::Set {
                        latitude,
                        longitude,
                    } => call(&proxy, "SetLocation", &(latitude, longitude)).await?,
                    LocationCommand::Clear => call(&proxy, "ClearLocation", &()).await?,
                },
                Command::Retry => call(&proxy, "Retry", &()).await?,
                Command::Reset => call(&proxy, "Reset", &()).await?,
                Command::Restore => call(&proxy, "RestoreNow", &()).await?,
                Command::Refresh => call(&proxy, "Refresh", &()).await?,
                Command::Open => call(&proxy, "OpenPanel", &()).await?,
                Command::Quit => {
                    call(&proxy, "Quit", &()).await?;
                    println!("Project Ember is shutting down and restoring displays.");
                    return Ok(());
                }
                Command::Status { .. }
                | Command::Diagnostics { .. }
                | Command::Probe { .. }
                | Command::Simulate { .. }
                | Command::Version => unreachable!(),
            }
            // Give the coordinator a moment, then print the resulting status.
            std::thread::sleep(Duration::from_millis(120));
            let snapshot = fetch_snapshot(false).await?;
            print_status(&snapshot);
            Ok(())
        }
    }
}

async fn proxy(connection: &Connection) -> anyhow::Result<zbus::Proxy<'_>> {
    Ok(zbus::Proxy::new(connection, BUS_NAME, OBJECT_PATH, INTERFACE).await?)
}

async fn call(
    proxy: &zbus::Proxy<'_>,
    method: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> anyhow::Result<()> {
    proxy
        .call::<_, _, ()>(method, body)
        .await
        .with_context(|| format!("call {method}"))?;
    Ok(())
}

async fn connection_with_daemon() -> anyhow::Result<Connection> {
    let connection = Connection::session().await.context("session D-Bus")?;
    if version(&connection).await.is_some() {
        return Ok(connection);
    }
    // Ask D-Bus activation first; fall back to launching the daemon binary.
    if let Ok(dbus) = zbus::fdo::DBusProxy::new(&connection).await {
        if let Ok(name) = BUS_NAME.try_into() {
            let _ = dbus.start_service_by_name(name, 0).await;
        }
    }
    for _ in 0..10 {
        if version(&connection).await.is_some() {
            return Ok(connection);
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    if let Ok(mut child) = std::process::Command::new("emberd").spawn() {
        let _ = child.try_wait();
    }
    for _ in 0..30 {
        if version(&connection).await.is_some() {
            return Ok(connection);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    Err(anyhow!(
        "Project Ember daemon is not running. Start it with `systemctl --user start project-ember` or launch Ember."
    ))
}

async fn version(connection: &Connection) -> Option<String> {
    let proxy = proxy(connection).await.ok()?;
    proxy.call::<_, _, String>("GetVersion", &()).await.ok()
}

async fn fetch_snapshot(start_daemon: bool) -> anyhow::Result<Snapshot> {
    let connection = if start_daemon {
        connection_with_daemon().await?
    } else {
        let connection = Connection::session().await?;
        if version(&connection).await.is_none() {
            return Err(anyhow!("daemon stopped"));
        }
        connection
    };
    let proxy = proxy(&connection).await?;
    let text: String = proxy
        .call("GetSnapshot", &())
        .await
        .context("get snapshot")?;
    Snapshot::from_json(&text).ok_or_else(|| anyhow!("invalid snapshot from daemon"))
}

fn parse_on_off(value: &str) -> anyhow::Result<bool> {
    match value {
        "on" | "true" | "enable" | "enabled" | "1" => Ok(true),
        "off" | "false" | "disable" | "disabled" | "0" => Ok(false),
        other => Err(anyhow!("expected on or off, got '{other}'")),
    }
}

fn print_status(snapshot: &Snapshot) {
    println!(
        "Project Ember {} · {}",
        snapshot.version, snapshot.runtime_label
    );
    println!("{}", snapshot.status_title);
    println!("{}", snapshot.status_detail);
    println!(
        "Displays: {} active · {} unsupported · {} pending · {} failed",
        snapshot.counts.verified,
        snapshot.counts.unsupported,
        snapshot.pending_restore_count,
        snapshot.counts.failed
    );
    println!(
        "Backend: {} ({})",
        snapshot.backend.name,
        snapshot.backend.verification.label()
    );
    println!(
        "Backlight: {}",
        match snapshot.backlight.state {
            ember_core::BacklightState::Engaged => "locked",
            ember_core::BacklightState::Disengaged => "off",
            ember_core::BacklightState::Unavailable => "unavailable",
            ember_core::BacklightState::Failed => "stopped",
        }
    );
    if snapshot.solar.enabled {
        let next = snapshot
            .solar
            .next_event
            .map(|event| {
                format!(
                    "{} at {}",
                    event.kind.label(),
                    event.date.with_timezone(&chrono::Local).format("%H:%M")
                )
            })
            .unwrap_or_else(|| "no event".into());
        println!("Sun schedule: on · next {next}");
    } else {
        println!("Sun schedule: off");
    }
    if let Some(nightlight) = &snapshot.nightlight {
        if nightlight.active {
            println!(
                "Nightlight: on ({})",
                nightlight
                    .temperature
                    .map(|value| format!("{value} K"))
                    .unwrap_or_else(|| "active".into())
            );
        }
    }
    if !snapshot.attention.is_none() {
        println!(
            "Attention: {} — {}",
            snapshot.attention.title, snapshot.attention.message
        );
    }
}
