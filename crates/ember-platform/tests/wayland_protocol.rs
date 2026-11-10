//! End-to-end test of the Wayland backend against a minimal in-process
//! compositor. It validates the `zwlr_gamma_control_v1` object lifetime and the
//! exact interleaved u16 ramp bytes, not just our internal state.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ember_core::{ColorCurve, DisplayIdentity, GammaTable, VerificationLevel};
use ember_platform::backend::DisplayBackend;
use ember_platform::drm::DrmSysfs;
use ember_platform::wayland::WaylandBackend;
use wayland_protocols_wlr::gamma_control::v1::server::zwlr_gamma_control_manager_v1::{
    Request as ManagerRequest, ZwlrGammaControlManagerV1,
};
use wayland_protocols_wlr::gamma_control::v1::server::zwlr_gamma_control_v1::{
    Request as ControlRequest, ZwlrGammaControlV1,
};
use wayland_server::backend::ClientData;
use wayland_server::protocol::wl_output;
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New,
    Resource,
};

const GAMMA_SIZE: u32 = 256;

struct MockClientData;
impl ClientData for MockClientData {}

#[derive(Default)]
struct Tables {
    applied: Option<Vec<u16>>,
    released: bool,
}

struct ServerState {
    fail_gamma: bool,
    tables: Arc<Mutex<Tables>>,
}

struct MockServer {
    _dir: tempfile::TempDir,
    socket: std::path::PathBuf,
    stop: Arc<AtomicBool>,
    tables: Arc<Mutex<Tables>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MockServer {
    fn start(fail_gamma: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("wayland-ember-test");
        let listener = ListeningSocket::bind_absolute(socket.clone()).unwrap();
        let tables = Arc::new(Mutex::new(Tables::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_tables = tables.clone();
        let thread_stop = stop.clone();
        let handle = std::thread::Builder::new()
            .name("mock-wayland".into())
            .spawn(move || {
                let mut display: Display<ServerState> = Display::new().unwrap();
                let display_handle = display.handle();
                display_handle.create_global::<ServerState, wl_output::WlOutput, ()>(4, ());
                display_handle.create_global::<ServerState, ZwlrGammaControlManagerV1, ()>(1, ());
                let mut state = ServerState {
                    fail_gamma,
                    tables: thread_tables,
                };
                while !thread_stop.load(Ordering::SeqCst) {
                    let _ = display.dispatch_clients(&mut state);
                    let _ = display.flush_clients();
                    if let Ok(Some(stream)) = listener.accept() {
                        let mut handle = display.handle();
                        let _ = handle.insert_client(stream, Arc::new(MockClientData));
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
            .unwrap();
        Self {
            _dir: dir,
            socket,
            stop,
            tables,
            handle: Some(handle),
        }
    }

    fn connect(&self) -> WaylandBackend {
        std::env::set_var("WAYLAND_DISPLAY", &self.socket);
        std::env::remove_var("WAYLAND_SOCKET");
        WaylandBackend::with_sysfs(DrmSysfs::with_root("/nonexistent/ember-sysfs"))
            .expect("connect to mock compositor")
    }

    fn applied(&self) -> Option<Vec<u16>> {
        self.tables.lock().unwrap().applied.clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl GlobalDispatch<wl_output::WlOutput, ()> for ServerState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<wl_output::WlOutput>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let output = data_init.init(resource, ());
        output.geometry(
            0,
            0,
            300,
            200,
            wl_output::Subpixel::Unknown,
            "EmberMock".into(),
            "Panel".into(),
            wl_output::Transform::Normal,
        );
        output.scale(1);
        output.name("eDP-1".into());
        output.description("Mock built-in panel".into());
        output.done();
    }
}

impl Dispatch<wl_output::WlOutput, ()> for ServerState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wl_output::WlOutput,
        _request: <wl_output::WlOutput as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<ZwlrGammaControlManagerV1, ()> for ServerState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrGammaControlManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for ServerState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrGammaControlManagerV1,
        request: ManagerRequest,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let ManagerRequest::GetGammaControl { id, .. } = request {
            if state.fail_gamma {
                let control = data_init.init(id, ());
                control.failed();
            } else {
                let control = data_init.init(id, ());
                control.gamma_size(GAMMA_SIZE);
            }
        }
    }
}

impl Dispatch<ZwlrGammaControlV1, ()> for ServerState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrGammaControlV1,
        request: ControlRequest,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ControlRequest::SetGamma { fd } => {
                let mut file = std::fs::File::from(fd);
                let mut bytes = Vec::new();
                let _ = file.read_to_end(&mut bytes);
                let table: Vec<u16> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|chunk| u16::from_ne_bytes([chunk[0], chunk[1]]))
                    .collect();
                state.tables.lock().unwrap().applied = Some(table);
            }
            ControlRequest::Destroy => {
                let mut tables = state.tables.lock().unwrap();
                tables.released = true;
                tables.applied = None;
            }
            _ => {}
        }
    }
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    predicate()
}

#[test]
fn wayland_gamma_control_end_to_end() {
    let server = MockServer::start(false);
    let mut backend = server.connect();
    assert_eq!(
        backend.capabilities().verification,
        VerificationLevel::Applied
    );
    assert!(!backend.capabilities().readback);

    let displays = backend.enumerate().expect("enumerate");
    assert_eq!(displays.len(), 1, "one mock output");
    let display = &displays[0];
    assert_eq!(display.connector, "eDP-1");
    assert_eq!(display.gamma_size, GAMMA_SIZE);
    assert!(display.built_in);
    let identity: DisplayIdentity = display.identity.clone();

    let baseline = backend.capture_baseline(&identity).expect("baseline");
    assert_eq!(
        baseline.gamma_source,
        ember_core::GammaSource::CompositorDefault
    );

    let table = GammaTable::identity(GAMMA_SIZE as usize)
        .applying(ColorCurve::gains_for_warmth(0.62), 0.75);
    let report = backend.apply(&identity, &table).expect("apply");
    assert!(report.accepted);

    let expected: Vec<u8> = table
        .to_u16_blocks(GAMMA_SIZE as usize)
        .iter()
        .flat_map(|value| value.to_ne_bytes())
        .collect();
    let expected_u16: Vec<u16> = expected
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| u16::from_ne_bytes([chunk[0], chunk[1]]))
        .collect();
    assert!(
        wait_until(Duration::from_secs(2), || server.applied().is_some()),
        "compositor never received set_gamma"
    );
    let applied = server.applied().unwrap();
    assert_eq!(applied.len(), GAMMA_SIZE as usize * 3);
    assert_eq!(applied, expected_u16, "ramp bytes must match exactly");

    assert_eq!(
        backend.verify(&identity, &table),
        ember_platform::VerifyReport::AcceptedUnverified
    );

    backend.restore(&identity, &baseline).expect("restore");
    assert!(
        wait_until(Duration::from_secs(2), || {
            server.tables.lock().unwrap().released
        }),
        "destroying the control must restore the compositor table"
    );
    backend.shutdown();

    // Failure path (failed event).
    let failing = MockServer::start(true);
    let mut backend = failing.connect();
    let displays = backend.enumerate().expect("enumerate failing");
    assert_eq!(displays.len(), 1);
    assert_eq!(
        displays[0].gamma_size, 0,
        "failed control has no gamma size"
    );
    assert!(displays[0].unsupported_reason.is_some());
    assert!(backend.capture_baseline(&displays[0].identity).is_err());
    assert!(backend.apply(&displays[0].identity, &table).is_err());
    backend.shutdown();
}
