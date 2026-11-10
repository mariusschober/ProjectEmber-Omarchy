use std::sync::mpsc::Sender;
use std::time::Duration;

use ember_core::{EmberPreset, Snapshot, TrayPrimaryAction};
use tokio::sync::watch;
use zbus::fdo;

use crate::coordinator::DaemonCommand;

pub const BUS_NAME: &str = "org.mariusschober.ProjectEmber";
pub const OBJECT_PATH: &str = "/org/mariusschober/ProjectEmber";
pub const INTERFACE: &str = "org.mariusschober.ProjectEmber";

pub struct ProjectEmberInterface {
    command_tx: Sender<DaemonCommand>,
    snapshots: watch::Receiver<Snapshot>,
}

impl ProjectEmberInterface {
    fn send(&self, command: DaemonCommand) -> fdo::Result<()> {
        self.command_tx
            .send(command)
            .map_err(|_| fdo::Error::Failed("Project Ember is shutting down".into()))
    }
}

#[zbus::interface(name = "org.mariusschober.ProjectEmber")]
impl ProjectEmberInterface {
    /// Complete JSON snapshot (see `Snapshot` in ember-core).
    fn get_snapshot(&self) -> String {
        self.snapshots.borrow().to_json()
    }

    fn get_version(&self) -> String {
        crate::coordinator::version_string()
    }

    async fn set_enabled(&self, enabled: bool) -> fdo::Result<()> {
        self.send(DaemonCommand::SetEnabled(enabled))
    }

    async fn toggle(&self) -> fdo::Result<()> {
        self.send(DaemonCommand::Toggle)
    }

    async fn set_warmth(&self, warmth: f64) -> fdo::Result<()> {
        if !warmth.is_finite() || !(0.0..=1.0).contains(&warmth) {
            return Err(fdo::Error::InvalidArgs(
                "warmth must be a finite number between 0 and 1".into(),
            ));
        }
        self.send(DaemonCommand::SetWarmth(warmth))
    }

    async fn set_brightness(&self, brightness: f64) -> fdo::Result<()> {
        if !brightness.is_finite() || !(0.10..=1.0).contains(&brightness) {
            return Err(fdo::Error::InvalidArgs(
                "brightness must be a finite number between 0.10 and 1".into(),
            ));
        }
        self.send(DaemonCommand::SetBrightness(brightness))
    }

    async fn apply_preset(&self, preset: String) -> fdo::Result<()> {
        let value: EmberPreset = preset.parse().map_err(fdo::Error::InvalidArgs)?;
        self.send(DaemonCommand::ApplyPreset(value))
    }

    async fn set_backlight_lock(&self, enabled: bool) -> fdo::Result<()> {
        self.send(DaemonCommand::SetBacklightLock(enabled))
    }

    async fn set_sun_schedule(&self, enabled: bool) -> fdo::Result<()> {
        self.send(DaemonCommand::SetSunSchedule(enabled))
    }

    async fn set_launch_at_login(&self, enabled: bool) -> fdo::Result<()> {
        self.send(DaemonCommand::SetLaunchAtLogin(enabled))
    }

    async fn set_tray_primary_action(&self, action: String) -> fdo::Result<()> {
        let value: TrayPrimaryAction = action.parse().map_err(fdo::Error::InvalidArgs)?;
        self.send(DaemonCommand::SetTrayPrimaryAction(value))
    }

    async fn set_location(&self, latitude: f64, longitude: f64) -> fdo::Result<()> {
        if !latitude.is_finite()
            || !longitude.is_finite()
            || !(-90.0..=90.0).contains(&latitude)
            || !(-180.0..=180.0).contains(&longitude)
        {
            return Err(fdo::Error::InvalidArgs("invalid coordinates".into()));
        }
        self.send(DaemonCommand::SetLocation(latitude, longitude))
    }

    async fn clear_location(&self) -> fdo::Result<()> {
        self.send(DaemonCommand::ClearLocation)
    }

    async fn turn_off_nightlight(&self) -> fdo::Result<()> {
        self.send(DaemonCommand::TurnOffNightlight)
    }

    async fn retry(&self) -> fdo::Result<()> {
        self.send(DaemonCommand::Retry)
    }

    async fn reset(&self) -> fdo::Result<()> {
        self.send(DaemonCommand::Reset)
    }

    async fn restore_now(&self) -> fdo::Result<()> {
        self.send(DaemonCommand::RestoreNow)
    }

    async fn refresh(&self) -> fdo::Result<()> {
        self.send(DaemonCommand::Refresh)
    }

    async fn open_panel(&self) -> fdo::Result<()> {
        self.send(DaemonCommand::OpenPanel)
    }

    async fn open_diagnostics(&self) -> fdo::Result<()> {
        self.send(DaemonCommand::OpenDiagnostics)
    }

    async fn export_diagnostics(&self) -> fdo::Result<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(DaemonCommand::ExportDiagnostics(tx))?;
        match tokio::time::timeout(Duration::from_secs(2), rx).await {
            Ok(Ok(text)) => Ok(text),
            _ => Err(fdo::Error::Failed("diagnostics export timed out".into())),
        }
    }

    async fn quit(&self) -> fdo::Result<()> {
        self.send(DaemonCommand::Shutdown)
    }
}

/// Starts the session-bus service. The returned connection must be kept alive
/// for as long as the service should run.
pub async fn serve(
    command_tx: Sender<DaemonCommand>,
    snapshots: watch::Receiver<Snapshot>,
) -> anyhow::Result<zbus::Connection> {
    let interface = ProjectEmberInterface {
        command_tx,
        snapshots: snapshots.clone(),
    };
    let connection = zbus::connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, interface)?
        .build()
        .await?;

    let signal_connection = connection.clone();
    let mut watch = snapshots;
    tokio::spawn(async move {
        while watch.changed().await.is_ok() {
            let json = watch.borrow().to_json();
            let _ = signal_connection
                .emit_signal(
                    None::<&str>,
                    OBJECT_PATH,
                    INTERFACE,
                    "SnapshotChanged",
                    &(json,),
                )
                .await;
        }
    });

    Ok(connection)
}

/// Creates a proxy to a running daemon (used by the CLI and tests).
pub async fn proxy(connection: &zbus::Connection) -> anyhow::Result<zbus::Proxy<'_>> {
    Ok(zbus::Proxy::new(connection, BUS_NAME, OBJECT_PATH, INTERFACE).await?)
}
