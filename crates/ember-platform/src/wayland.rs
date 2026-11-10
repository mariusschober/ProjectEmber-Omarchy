use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use ember_core::{
    BackendKind, DisplayBaseline, DisplayIdentity, EmberError, EmberResult, GammaTable,
    VerificationLevel,
};
use log::{debug, info, warn};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_callback, wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_wlr::gamma_control::v1::client::zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1;
use wayland_protocols_wlr::gamma_control::v1::client::zwlr_gamma_control_v1::{
    Event as GammaEvent, ZwlrGammaControlV1,
};

use crate::backend::{
    compositor_default_baseline, identity_for_connector, ApplyReport, BackendCapabilities,
    BackendEvent, DisplayBackend, DisplayInfo, EventSink, VerifyReport,
};
use crate::drm::DrmSysfs;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
struct OutputKey(u32);

#[derive(Debug, Clone, Copy)]
struct GammaKey(u32);

struct OutputState {
    output: wl_output::WlOutput,
    connector: String,
    make: String,
    model: String,
    gamma_size: u32,
    failed: bool,
    done: bool,
}

impl OutputState {
    fn new(output: wl_output::WlOutput) -> Self {
        Self {
            output,
            connector: String::new(),
            make: String::new(),
            model: String::new(),
            gamma_size: 0,
            failed: false,
            done: false,
        }
    }
}

pub struct WlState {
    manager: Option<ZwlrGammaControlManagerV1>,
    outputs: HashMap<u32, OutputState>,
    controls: HashMap<u32, ZwlrGammaControlV1>,
    events: Sender<BackendEvent>,
    sysfs: DrmSysfs,
    dirty: bool,
}

impl WlState {
    fn new(sysfs: DrmSysfs, events: Sender<BackendEvent>) -> Self {
        Self {
            manager: None,
            outputs: HashMap::new(),
            controls: HashMap::new(),
            events,
            sysfs,
            dirty: false,
        }
    }

    fn notify_topology(&self) {
        let _ = self.events.send(BackendEvent::TopologyChanged);
    }

    fn ensure_control(&mut self, key: u32, qh: &QueueHandle<WlState>) {
        if self.controls.contains_key(&key) {
            return;
        }
        let Some(manager) = self.manager.clone() else {
            return;
        };
        let Some(output) = self.outputs.get(&key) else {
            return;
        };
        if output.connector.is_empty() || output.failed {
            return;
        }
        let control = manager.get_gamma_control(&output.output, qh, GammaKey(key));
        self.controls.insert(key, control);
    }

    fn ensure_controls(&mut self, qh: &QueueHandle<WlState>) {
        let keys: Vec<u32> = self.outputs.keys().copied().collect();
        for key in keys {
            self.ensure_control(key, qh);
        }
    }

    fn remove_output(&mut self, key: u32) {
        if let Some(control) = self.controls.remove(&key) {
            control.destroy();
        }
        self.outputs.remove(&key);
        self.dirty = true;
    }

    fn connector_for(&self, key: u32) -> String {
        self.outputs
            .get(&key)
            .map(|output| output.connector.clone())
            .unwrap_or_default()
    }

    fn info_for(&self, key: u32) -> Option<DisplayInfo> {
        let output = self.outputs.get(&key)?;
        if output.connector.is_empty() {
            return None;
        }
        let (identity, edid, mut reason) =
            identity_for_connector(&self.sysfs, &output.connector, key);
        if output.failed {
            reason = Some(
                "Another color service holds exclusive gamma control, or this output has no \
                 writable gamma table."
                    .into(),
            );
        }
        Some(DisplayInfo {
            display_id: key,
            identity,
            connector: output.connector.clone(),
            gamma_size: if output.failed { 0 } else { output.gamma_size },
            is_online: true,
            is_active: output.done,
            is_mirrored: false,
            built_in: DisplayIdentity::is_built_in_connector(&output.connector),
            edid,
            unsupported_reason: reason,
        })
    }

    fn infos(&self) -> Vec<DisplayInfo> {
        let mut infos: Vec<DisplayInfo> = self
            .outputs
            .keys()
            .filter_map(|key| self.info_for(*key))
            .collect();
        infos.sort_by(|a, b| a.connector.cmp(&b.connector));
        infos
    }

    fn release_all(&mut self) {
        for (_, control) in self.controls.drain() {
            control.destroy();
        }
    }
}

fn bind_global(
    state: &mut WlState,
    registry: &wl_registry::WlRegistry,
    qh: &QueueHandle<WlState>,
    name: u32,
    interface: &str,
    version: u32,
) {
    match interface {
        "wl_output" => {
            if state.outputs.contains_key(&name) {
                return;
            }
            let output: wl_output::WlOutput =
                registry.bind(name, version.min(4), qh, OutputKey(name));
            state.outputs.insert(name, OutputState::new(output));
            debug!("wayland: bound wl_output {name} v{}", version.min(4));
        }
        "zwlr_gamma_control_manager_v1" if state.manager.is_none() => {
            let manager: ZwlrGammaControlManagerV1 = registry.bind(name, version.min(1), qh, ());
            state.manager = Some(manager);
            info!("wayland: bound wlr-gamma-control manager");
        }
        _ => {}
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WlState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                bind_global(state, registry, qh, name, &interface, version);
                state.notify_topology();
            }
            wl_registry::Event::GlobalRemove { name } => {
                state.remove_output(name);
                state.notify_topology();
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, OutputKey> for WlState {
    fn event(
        state: &mut Self,
        _proxy: &wl_output::WlOutput,
        event: wl_output::Event,
        data: &OutputKey,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let key = data.0;
        let Some(output) = state.outputs.get_mut(&key) else {
            return;
        };
        match event {
            wl_output::Event::Name { name } => output.connector = name,
            wl_output::Event::Description { description } => {
                debug!("wayland: output {key} description {description}");
            }
            wl_output::Event::Geometry { make, model, .. } => {
                output.make = make;
                output.model = model;
            }
            wl_output::Event::Done => {
                output.done = true;
                if output.connector.is_empty() {
                    // Compositors older than wl_output v4 do not expose a
                    // connector name. A synthetic name keeps the display
                    // visible but its missing EDID mapping makes it ambiguous,
                    // so Ember will not mutate it.
                    output.connector = format!("output-{key}");
                }
                state.ensure_control(key, qh);
                state.dirty = true;
                state.notify_topology();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrGammaControlV1, GammaKey> for WlState {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrGammaControlV1,
        event: GammaEvent,
        data: &GammaKey,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let key = data.0;
        match event {
            GammaEvent::GammaSize { size } => {
                if let Some(output) = state.outputs.get_mut(&key) {
                    output.gamma_size = size;
                    debug!("wayland: {} gamma size {size}", output.connector);
                }
            }
            GammaEvent::Failed => {
                let connector = state.connector_for(key);
                if let Some(output) = state.outputs.get_mut(&key) {
                    output.failed = true;
                    output.gamma_size = 0;
                }
                state.controls.remove(&key);
                warn!("wayland: gamma control failed for {connector}");
                let _ = state.events.send(BackendEvent::ControlLost {
                    connector,
                    reason: "Compositor rejected or revoked gamma control (another color service \
                             may hold it, or the output has no gamma table)."
                        .into(),
                });
                state.notify_topology();
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for WlState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_callback::WlCallback,
        _event: wl_callback::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for WlState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrGammaControlManagerV1,
        _event: <ZwlrGammaControlManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

enum CommandReply {
    Enumerate(EmberResult<Vec<DisplayInfo>>),
    Baseline(EmberResult<DisplayBaseline>),
    Apply(EmberResult<ApplyReport>),
    Verify(VerifyReport),
    Unit(EmberResult<()>),
}

enum Command {
    Enumerate(Sender<CommandReply>),
    Baseline(DisplayIdentity, Sender<CommandReply>),
    Apply(DisplayIdentity, GammaTable, Sender<CommandReply>),
    Verify(DisplayIdentity, GammaTable, Sender<CommandReply>),
    Restore(DisplayIdentity, Sender<CommandReply>),
    ReleaseAll(Sender<CommandReply>),
    Shutdown,
}

pub struct WaylandBackend {
    command_tx: Sender<Command>,
    events: EventSink,
    wake_fd: OwnedFd,
    capabilities: BackendCapabilities,
    thread: Option<JoinHandle<()>>,
}

impl WaylandBackend {
    pub fn connect() -> EmberResult<Self> {
        Self::with_sysfs(DrmSysfs::new())
    }

    pub fn with_sysfs(sysfs: DrmSysfs) -> EmberResult<Self> {
        let events = EventSink::new();
        let event_tx = events.sender();
        let (wake_read, wake_write) = make_pipe()?;
        let (command_tx, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<BackendCapabilities, String>>();

        let thread = std::thread::Builder::new()
            .name("ember-wayland".into())
            .spawn(move || {
                if let Err(error) =
                    run_event_loop(sysfs, command_rx, wake_read, event_tx, &ready_tx)
                {
                    let _ = ready_tx.send(Err(error));
                }
            })
            .map_err(|error| EmberError::BackendUnavailable(error.to_string()))?;

        let capabilities = match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(capabilities)) => capabilities,
            Ok(Err(message)) => {
                let _ = thread.join();
                return Err(EmberError::BackendUnavailable(message));
            }
            Err(error) => {
                return Err(EmberError::BackendUnavailable(format!(
                    "Wayland backend did not initialize: {error}"
                )))
            }
        };

        Ok(Self {
            command_tx,
            events,
            wake_fd: wake_write,
            capabilities,
            thread: Some(thread),
        })
    }

    fn wake(&self) {
        let byte = [1u8];
        unsafe {
            libc::write(self.wake_fd.as_raw_fd(), byte.as_ptr().cast(), 1);
        }
    }

    fn request_with_reply(
        &self,
        make: impl FnOnce(Sender<CommandReply>) -> Command,
    ) -> EmberResult<CommandReply> {
        let (tx, rx) = mpsc::channel();
        self.command_tx
            .send(make(tx))
            .map_err(|_| EmberError::BackendFailure("Wayland backend stopped".into()))?;
        self.wake();
        rx.recv_timeout(COMMAND_TIMEOUT)
            .map_err(|_| EmberError::BackendFailure("Wayland backend timed out".into()))
    }
}

impl DisplayBackend for WaylandBackend {
    fn capabilities(&self) -> BackendCapabilities {
        self.capabilities.clone()
    }

    fn enumerate(&mut self) -> EmberResult<Vec<DisplayInfo>> {
        match self.request_with_reply(Command::Enumerate)? {
            CommandReply::Enumerate(result) => result,
            _ => Err(EmberError::BackendFailure("unexpected reply".into())),
        }
    }

    fn capture_baseline(&mut self, identity: &DisplayIdentity) -> EmberResult<DisplayBaseline> {
        match self.request_with_reply(|reply| Command::Baseline(identity.clone(), reply))? {
            CommandReply::Baseline(result) => result,
            _ => Err(EmberError::BackendFailure("unexpected reply".into())),
        }
    }

    fn apply(
        &mut self,
        identity: &DisplayIdentity,
        table: &GammaTable,
    ) -> EmberResult<ApplyReport> {
        match self
            .request_with_reply(|reply| Command::Apply(identity.clone(), table.clone(), reply))?
        {
            CommandReply::Apply(result) => result,
            _ => Err(EmberError::BackendFailure("unexpected reply".into())),
        }
    }

    fn verify(&mut self, identity: &DisplayIdentity, expected: &GammaTable) -> VerifyReport {
        match self
            .request_with_reply(|reply| Command::Verify(identity.clone(), expected.clone(), reply))
        {
            Ok(CommandReply::Verify(result)) => result,
            Ok(_) => VerifyReport::Failed("unexpected reply".into()),
            Err(error) => VerifyReport::Failed(error.to_string()),
        }
    }

    fn restore(
        &mut self,
        identity: &DisplayIdentity,
        _baseline: &DisplayBaseline,
    ) -> EmberResult<()> {
        match self.request_with_reply(|reply| Command::Restore(identity.clone(), reply))? {
            CommandReply::Unit(result) => result,
            _ => Err(EmberError::BackendFailure("unexpected reply".into())),
        }
    }

    fn release_all(&mut self) -> EmberResult<()> {
        match self.request_with_reply(Command::ReleaseAll)? {
            CommandReply::Unit(result) => result,
            _ => Err(EmberError::BackendFailure("unexpected reply".into())),
        }
    }

    fn drain_events(&mut self) -> Vec<BackendEvent> {
        self.events.drain()
    }

    fn shutdown(&mut self) {
        let _ = self.command_tx.send(Command::Shutdown);
        self.wake();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for WaylandBackend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_event_loop(
    sysfs: DrmSysfs,
    command_rx: Receiver<Command>,
    wake_fd: OwnedFd,
    event_tx: Sender<BackendEvent>,
    ready_tx: &Sender<Result<BackendCapabilities, String>>,
) -> Result<(), String> {
    let conn = Connection::connect_to_env()
        .map_err(|error| format!("cannot connect to Wayland: {error}"))?;
    let (globals, mut queue) = registry_queue_init::<WlState>(&conn)
        .map_err(|error| format!("cannot initialize Wayland registry: {error}"))?;
    let qh = queue.handle();
    let mut state = WlState::new(sysfs, event_tx);
    let registry = globals.registry().clone();
    for global in globals.contents().clone_list() {
        bind_global(
            &mut state,
            &registry,
            &qh,
            global.name,
            &global.interface,
            global.version,
        );
    }
    roundtrip(&mut state, &conn, &mut queue)?;
    if state.manager.is_none() {
        return Err(
            "the compositor does not expose zwlr_gamma_control_manager_v1; Ember needs \
             Hyprland, Sway or another wlroots compositor for Wayland color control"
                .into(),
        );
    }
    state.ensure_controls(&qh);
    roundtrip(&mut state, &conn, &mut queue)?;

    let capabilities = BackendCapabilities {
        kind: BackendKind::WaylandWlrGamma,
        name: "wayland-wlr-gamma",
        verification: VerificationLevel::Applied,
        readback: false,
        exclusive: true,
        note: Some(
            "Wayland exposes no color-table readback; Ember verifies that each ramp was accepted. \
             Destroying the gamma control restores the compositor's original table."
                .into(),
        ),
    };
    ready_tx
        .send(Ok(capabilities))
        .map_err(|_| "daemon went away during startup".to_string())?;

    let mut running = true;
    while running {
        if let Err(error) = queue.dispatch_pending(&mut state) {
            let _ = state.events.send(BackendEvent::Fatal(error.to_string()));
            return Err(format!("Wayland dispatch failed: {error}"));
        }
        if let Err(error) = conn.flush() {
            let _ = state.events.send(BackendEvent::Fatal(error.to_string()));
            return Err(format!("Wayland flush failed: {error}"));
        }

        loop {
            match command_rx.try_recv() {
                Ok(Command::Shutdown) => {
                    running = false;
                    break;
                }
                Ok(command) => handle_command(command, &mut state, &conn, &mut queue, &qh),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    running = false;
                    break;
                }
            }
        }
        if !running {
            break;
        }

        if let Err(error) = queue.dispatch_pending(&mut state) {
            let _ = state.events.send(BackendEvent::Fatal(error.to_string()));
            return Err(format!("Wayland dispatch failed: {error}"));
        }

        let Some(guard) = conn.prepare_read() else {
            continue;
        };
        let display_fd = guard.connection_fd().as_raw_fd();
        let mut poll_fds = [
            libc::pollfd {
                fd: display_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake_fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let result = unsafe { libc::poll(poll_fds.as_mut_ptr(), 2, -1) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            let _ = state.events.send(BackendEvent::Fatal(error.to_string()));
            return Err(format!("Wayland poll failed: {error}"));
        }
        if poll_fds[1].revents & libc::POLLIN != 0 {
            drain_fd(wake_fd.as_raw_fd());
        }
        if poll_fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            match guard.read() {
                Ok(_) => {}
                Err(error) => {
                    let _ = state.events.send(BackendEvent::Fatal(error.to_string()));
                    return Err(format!("Wayland read failed: {error}"));
                }
            }
        }
    }

    state.release_all();
    let _ = conn.flush();
    Ok(())
}

fn handle_command(
    command: Command,
    state: &mut WlState,
    conn: &Connection,
    queue: &mut EventQueue<WlState>,
    qh: &QueueHandle<WlState>,
) {
    match command {
        Command::Enumerate(reply) => {
            state.ensure_controls(qh);
            let _ = roundtrip(state, conn, queue);
            let _ = reply.send(CommandReply::Enumerate(Ok(state.infos())));
        }
        Command::Baseline(identity, reply) => {
            let result = (|| {
                let key = find_key(state, &identity)?;
                state.ensure_control(key, qh);
                let _ = roundtrip(state, conn, queue);
                let info = state.info_for(key).ok_or(EmberError::DisplayUnavailable)?;
                if !info.gamma_compatible() {
                    return Err(EmberError::BackendFailure(
                        info.unsupported_reason
                            .unwrap_or_else(|| "Display has no writable gamma table.".into()),
                    ));
                }
                Ok(compositor_default_baseline(&info.identity, info.gamma_size))
            })();
            let _ = reply.send(CommandReply::Baseline(result));
        }
        Command::Apply(identity, table, reply) => {
            let result = (|| {
                let key = find_key(state, &identity)?;
                state.ensure_control(key, qh);
                let _ = roundtrip(state, conn, queue);
                let info = state.info_for(key).ok_or(EmberError::DisplayUnavailable)?;
                if info.gamma_size < 2 || info.unsupported_reason.is_some() {
                    return Err(EmberError::BackendFailure(
                        info.unsupported_reason
                            .unwrap_or_else(|| "Display has no writable gamma table.".into()),
                    ));
                }
                let blocks = table.to_u16_blocks(info.gamma_size as usize);
                let fd = memfd_with_data(&u16_blocks_to_bytes(&blocks))?;
                let control = state
                    .controls
                    .get(&key)
                    .ok_or(EmberError::DisplayUnavailable)?;
                control.set_gamma(fd.as_fd());
                let _ = roundtrip(state, conn, queue);
                if state.outputs.get(&key).is_some_and(|output| output.failed) {
                    return Err(EmberError::BackendFailure(
                        "Compositor rejected the gamma ramp (another color service may hold \
                         control, or the output has no gamma table)."
                            .into(),
                    ));
                }
                Ok(ApplyReport {
                    accepted: true,
                    verified: false,
                })
            })();
            let _ = reply.send(CommandReply::Apply(result));
        }
        Command::Verify(identity, _expected, reply) => {
            let result = match find_key(state, &identity) {
                Ok(key) => match state.outputs.get(&key) {
                    Some(output) if output.failed => {
                        VerifyReport::Failed("Gamma control was revoked by the compositor.".into())
                    }
                    Some(_) if state.controls.contains_key(&key) => {
                        VerifyReport::AcceptedUnverified
                    }
                    _ => VerifyReport::Failed("No gamma control is held for this display.".into()),
                },
                Err(error) => VerifyReport::Failed(error.to_string()),
            };
            let _ = reply.send(CommandReply::Verify(result));
        }
        Command::Restore(identity, reply) => {
            let result = (|| {
                let key = find_key(state, &identity)?;
                if let Some(control) = state.controls.remove(&key) {
                    control.destroy();
                }
                if let Some(output) = state.outputs.get_mut(&key) {
                    output.failed = false;
                }
                let _ = roundtrip(state, conn, queue);
                Ok(())
            })();
            let _ = reply.send(CommandReply::Unit(result));
        }
        Command::ReleaseAll(reply) => {
            state.release_all();
            let _ = conn.flush();
            let _ = roundtrip(state, conn, queue);
            let _ = reply.send(CommandReply::Unit(Ok(())));
        }
        Command::Shutdown => {}
    }
}

fn find_key(state: &WlState, identity: &DisplayIdentity) -> EmberResult<u32> {
    let matches: Vec<u32> = state
        .outputs
        .iter()
        .filter(|(_, output)| !output.connector.is_empty())
        .filter_map(|(key, output)| {
            let (candidate, _, _) = identity_for_connector(&state.sysfs, &output.connector, *key);
            candidate.matches(identity).then_some(*key)
        })
        .collect();
    match matches.len() {
        0 => Err(EmberError::DisplayUnavailable),
        1 => Ok(matches[0]),
        _ => Err(EmberError::DisplayIdentityAmbiguous(
            "Multiple online outputs match the saved display identity.".into(),
        )),
    }
}

fn roundtrip(
    state: &mut WlState,
    conn: &Connection,
    queue: &mut EventQueue<WlState>,
) -> Result<(), String> {
    conn.flush()
        .map_err(|error| format!("Wayland flush failed: {error}"))?;
    queue
        .roundtrip(state)
        .map_err(|error| format!("Wayland roundtrip failed: {error}"))?;
    Ok(())
}

fn u16_blocks_to_bytes(blocks: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(blocks.len() * 2);
    for value in blocks {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

fn memfd_with_data(data: &[u8]) -> EmberResult<OwnedFd> {
    let name = std::ffi::CString::new("project-ember-gamma").unwrap();
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(EmberError::BackendFailure(format!(
            "memfd_create failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut file = std::fs::File::from(owned);
    file.write_all(data)
        .map_err(|error| EmberError::BackendFailure(format!("gamma write failed: {error}")))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| EmberError::BackendFailure(format!("gamma seek failed: {error}")))?;
    Ok(file.into())
}

fn make_pipe() -> EmberResult<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    let result = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if result != 0 {
        return Err(EmberError::BackendFailure(format!(
            "pipe2 failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn drain_fd(fd: std::os::fd::RawFd) {
    let mut buffer = [0u8; 256];
    loop {
        let read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if read <= 0 {
            break;
        }
    }
}

use std::os::fd::AsFd;
