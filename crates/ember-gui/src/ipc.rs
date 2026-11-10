use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ember_core::Snapshot;
use log::{debug, warn};
use zbus::Connection;

const BUS_NAME: &str = "org.mariusschober.ProjectEmber";
const OBJECT_PATH: &str = "/org/mariusschober/ProjectEmber";
const INTERFACE: &str = "org.mariusschober.ProjectEmber";

pub enum IpcCommand {
    Toggle,
    SetWarmth(f64),
    SetBrightness(f64),
    ApplyPreset(String),
    SetBacklightLock(bool),
    SetSunSchedule(bool),
    SetLaunchAtLogin(bool),
    SetTrayPrimaryAction(String),
    SetLocation(f64, f64),
    Retry,
    Reset,
    RestoreNow,
    Quit,
}

#[derive(Clone)]
pub struct IpcClient {
    commands: Sender<IpcCommand>,
    snapshot: Arc<Mutex<Option<Snapshot>>>,
    online: Arc<AtomicBool>,
}

impl IpcClient {
    pub fn start() -> Self {
        let (commands, receiver) = mpsc::channel();
        let snapshot = Arc::new(Mutex::new(None));
        let online = Arc::new(AtomicBool::new(false));
        let thread_snapshot = snapshot.clone();
        let thread_online = online.clone();
        std::thread::Builder::new()
            .name("ember-gui-ipc".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        warn!("GUI IPC runtime failed: {error}");
                        return;
                    }
                };
                runtime.block_on(run(receiver, thread_snapshot, thread_online));
            })
            .ok();
        Self {
            commands,
            snapshot,
            online,
        }
    }

    pub fn send(&self, command: IpcCommand) -> bool {
        self.commands.send(command).is_ok()
    }

    pub fn snapshot(&self) -> Option<Snapshot> {
        self.snapshot.lock().ok().and_then(|guard| guard.clone())
    }

    pub fn online(&self) -> bool {
        self.online.load(Ordering::Relaxed)
    }
}

async fn run(
    receiver: Receiver<IpcCommand>,
    snapshot: Arc<Mutex<Option<Snapshot>>>,
    #[allow(dead_code)] online: Arc<AtomicBool>,
) {
    let mut connection: Option<Connection> = None;
    let mut spawn_attempts = 0u32;
    let attempted_at = AtomicU64::new(0);
    loop {
        if connection.is_none() {
            match Connection::session().await {
                Ok(value) => {
                    debug!("GUI connected to session bus");
                    connection = Some(value);
                }
                Err(error) => {
                    warn!("GUI cannot reach session bus: {error}");
                    online.store(false, Ordering::Relaxed);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            }
        }
        let conn = connection.as_ref().unwrap();
        let proxy = match zbus::Proxy::new(conn, BUS_NAME, OBJECT_PATH, INTERFACE).await {
            Ok(proxy) => proxy,
            Err(_) => {
                connection = None;
                continue;
            }
        };

        // Poll the snapshot. A signal subscription is unnecessary for a
        // 300 ms UI cadence and makes reconnects trivial.
        match proxy.call::<_, _, String>("GetSnapshot", &()).await {
            Ok(text) => {
                if let Some(value) = Snapshot::from_json(&text) {
                    if let Ok(mut guard) = snapshot.lock() {
                        *guard = Some(value);
                    }
                    online.store(true, Ordering::Relaxed);
                }
            }
            Err(_) => {
                online.store(false, Ordering::Relaxed);
                connection = None;
                if spawn_attempts < 2 && attempted_at.load(Ordering::Relaxed) == 0 {
                    attempted_at.store(1, Ordering::Relaxed);
                    spawn_attempts += 1;
                    if let Err(error) = std::process::Command::new("emberd").spawn() {
                        debug!("could not start emberd: {error}");
                    }
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
                continue;
            }
        }

        loop {
            match receiver.try_recv() {
                Ok(command) => {
                    if dispatch(&proxy, command).await.is_err() {
                        connection = None;
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn dispatch(proxy: &zbus::Proxy<'_>, command: IpcCommand) -> zbus::Result<()> {
    match command {
        IpcCommand::Toggle => proxy.call::<_, _, ()>("Toggle", &()).await,
        IpcCommand::SetWarmth(value) => proxy.call::<_, _, ()>("SetWarmth", &(value,)).await,
        IpcCommand::SetBrightness(value) => {
            proxy.call::<_, _, ()>("SetBrightness", &(value,)).await
        }
        IpcCommand::ApplyPreset(value) => {
            proxy
                .call::<_, _, ()>("ApplyPreset", &(value.as_str(),))
                .await
        }
        IpcCommand::SetBacklightLock(value) => {
            proxy.call::<_, _, ()>("SetBacklightLock", &(value,)).await
        }
        IpcCommand::SetSunSchedule(value) => {
            proxy.call::<_, _, ()>("SetSunSchedule", &(value,)).await
        }
        IpcCommand::SetLaunchAtLogin(value) => {
            proxy.call::<_, _, ()>("SetLaunchAtLogin", &(value,)).await
        }
        IpcCommand::SetTrayPrimaryAction(value) => {
            proxy
                .call::<_, _, ()>("SetTrayPrimaryAction", &(value.as_str(),))
                .await
        }
        IpcCommand::SetLocation(latitude, longitude) => {
            proxy
                .call::<_, _, ()>("SetLocation", &(latitude, longitude))
                .await
        }
        IpcCommand::Retry => proxy.call::<_, _, ()>("Retry", &()).await,
        IpcCommand::Reset => proxy.call::<_, _, ()>("Reset", &()).await,
        IpcCommand::RestoreNow => proxy.call::<_, _, ()>("RestoreNow", &()).await,
        IpcCommand::Quit => proxy.call::<_, _, ()>("Quit", &()).await,
    }
}

/// Synchronous diagnostics fetch for the diagnostics window. Runs on a
/// short-lived runtime so the GTK main loop is untouched.
pub fn fetch_diagnostics() -> String {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return format!("Diagnostics unavailable: {error}"),
    };
    runtime.block_on(async {
        let Ok(connection) = Connection::session().await else {
            return "Diagnostics unavailable: no session bus.".to_string();
        };
        let Ok(proxy) = zbus::Proxy::new(&connection, BUS_NAME, OBJECT_PATH, INTERFACE).await
        else {
            return "Diagnostics unavailable: Project Ember is not running.".to_string();
        };
        match proxy.call::<_, _, String>("ExportDiagnostics", &()).await {
            Ok(text) => text,
            Err(error) => format!("Diagnostics unavailable: {error}"),
        }
    })
}

pub fn fetch_snapshot_sync() -> Option<Snapshot> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    runtime.block_on(async {
        let connection = Connection::session().await.ok()?;
        let proxy = zbus::Proxy::new(&connection, BUS_NAME, OBJECT_PATH, INTERFACE)
            .await
            .ok()?;
        let text: String = proxy.call("GetSnapshot", &()).await.ok()?;
        Snapshot::from_json(&text)
    })
}
