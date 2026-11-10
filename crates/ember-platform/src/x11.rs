use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use ember_core::{
    BackendKind, DisplayBaseline, DisplayIdentity, EmberError, EmberResult, GammaSource,
    GammaTable, VerificationLevel,
};
use log::{debug, info};
use x11rb::connection::Connection;
use x11rb::protocol::randr;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as XprotoExt};
use x11rb::rust_connection::RustConnection;

use crate::backend::{
    identity_for_connector, identity_from_edid, ApplyReport, BackendCapabilities, BackendEvent,
    DisplayBackend, DisplayInfo, EventSink, VerifyReport,
};
use crate::drm::DrmSysfs;
use crate::edid::EdidInfo;

const GAMMA_TOLERANCE: f32 = 0.004;

pub struct X11Backend {
    conn: RustConnection,
    root: u32,
    sysfs: DrmSysfs,
    events: EventSink,
    stop: Arc<AtomicBool>,
    watcher: Option<std::thread::JoinHandle<()>>,
}

impl X11Backend {
    pub fn connect() -> EmberResult<Self> {
        Self::with_sysfs(DrmSysfs::new())
    }

    pub fn with_sysfs(sysfs: DrmSysfs) -> EmberResult<Self> {
        let (conn, screen) = RustConnection::connect(None).map_err(|error| {
            EmberError::BackendUnavailable(format!("X11 connect failed: {error}"))
        })?;
        let root = conn.setup().roots[screen].root;
        let version = randr::query_version(&conn, 1, 5)
            .map_err(|error| EmberError::BackendFailure(error.to_string()))?
            .reply()
            .map_err(|error| EmberError::BackendFailure(error.to_string()))?;
        if version.major_version < 1 {
            return Err(EmberError::BackendUnavailable(
                "XRandR is not available on this X server".into(),
            ));
        }

        let events = EventSink::new();
        let stop = Arc::new(AtomicBool::new(false));
        let watcher = spawn_randr_watcher(root, events.sender(), stop.clone())
            .map_err(|error| EmberError::BackendFailure(error.to_string()))?;

        info!(
            "x11: connected to {} (XRandR {}.{})",
            std::env::var("DISPLAY").unwrap_or_else(|_| "default".into()),
            version.major_version,
            version.minor_version
        );
        Ok(Self {
            conn,
            root,
            sysfs,
            events,
            stop,
            watcher: Some(watcher),
        })
    }

    fn outputs_snapshot(&self) -> EmberResult<Vec<X11Output>> {
        let resources = randr::get_screen_resources(&self.conn, self.root)
            .map_err(|error| EmberError::BackendFailure(error.to_string()))?
            .reply()
            .map_err(|error| EmberError::BackendFailure(error.to_string()))?;
        let timestamp = resources.config_timestamp;

        let edid_atom = self
            .conn
            .intern_atom(false, b"EDID")
            .map_err(|error| EmberError::BackendFailure(error.to_string()))?
            .reply()
            .map_err(|error| EmberError::BackendFailure(error.to_string()))?
            .atom;

        // Count how many outputs share each CRTC (mirroring).
        let mut crtc_usage: HashMap<u32, usize> = HashMap::new();
        let mut raw = Vec::new();
        for output in &resources.outputs {
            let info = match randr::get_output_info(&self.conn, *output, timestamp)
                .map_err(|error| EmberError::BackendFailure(error.to_string()))?
                .reply()
            {
                Ok(reply) => reply,
                Err(_) => continue,
            };
            *crtc_usage.entry(info.crtc).or_default() += 1;
            raw.push((*output, info));
        }

        let mut out = Vec::new();
        for (output, info) in raw {
            if info.connection != randr::Connection::CONNECTED || info.crtc == 0 {
                continue;
            }
            // x11rb 0.14 exposes the output name as the raw bytes already.
            let connector = String::from_utf8_lossy(&info.name).to_string();
            if connector.is_empty() {
                continue;
            }
            let _crtc_info = randr::get_crtc_info(&self.conn, info.crtc, timestamp)
                .map_err(|error| EmberError::BackendFailure(error.to_string()))?
                .reply()
                .map_err(|error| EmberError::BackendFailure(error.to_string()))?;
            let gamma_size = randr::get_crtc_gamma_size(&self.conn, info.crtc)
                .map_err(|error| EmberError::BackendFailure(error.to_string()))?
                .reply()
                .map_err(|error| EmberError::BackendFailure(error.to_string()))?
                .size;
            let is_mirrored =
                !info.clones.is_empty() || crtc_usage.get(&info.crtc).copied().unwrap_or(0) > 1;

            let edid = self
                .read_edid_property(output, edid_atom)
                .and_then(|bytes| EdidInfo::parse(&bytes));
            let built_in = DisplayIdentity::is_built_in_connector(&connector);
            let (identity, edid, unsupported_reason) = match edid {
                Some(edid) => {
                    let unit = crate::edid::connector_unit(&connector);
                    (
                        identity_from_edid(&connector, &edid, unit, built_in),
                        Some(edid),
                        None,
                    )
                }
                None => {
                    let (identity, edid, reason) =
                        identity_for_connector(&self.sysfs, &connector, output);
                    (identity, edid, reason)
                }
            };

            out.push(X11Output {
                output,
                crtc: info.crtc,
                connector,
                identity,
                edid,
                gamma_size: u32::from(gamma_size),
                is_mirrored,
                built_in,
                unsupported_reason,
            });
        }
        out.sort_by(|a, b| a.connector.cmp(&b.connector));
        Ok(out)
    }

    fn read_edid_property(&self, output: randr::Output, property: u32) -> Option<Vec<u8>> {
        let any_atom: u32 = AtomEnum::ANY.into();
        let reply = randr::get_output_property(
            &self.conn, output, property, any_atom, 0, 256, false, false,
        )
        .ok()?
        .reply()
        .ok()?;
        // EDID is exposed as 8-bit data. Other formats are ignored.
        if reply.format != 8 || reply.data.is_empty() {
            return None;
        }
        Some(reply.data)
    }

    fn find(&self, identity: &DisplayIdentity) -> EmberResult<X11Output> {
        let outputs = self.outputs_snapshot()?;
        let matches: Vec<X11Output> = outputs
            .into_iter()
            .filter(|output| output.identity.matches(identity))
            .collect();
        match matches.len() {
            0 => Err(EmberError::DisplayUnavailable),
            1 => Ok(matches.into_iter().next().unwrap()),
            _ => Err(EmberError::DisplayIdentityAmbiguous(
                "Multiple X11 outputs match the saved display identity.".into(),
            )),
        }
    }

    fn read_gamma(&self, output: &X11Output) -> EmberResult<GammaTable> {
        let reply = randr::get_crtc_gamma(&self.conn, output.crtc)
            .map_err(|error| EmberError::BackendFailure(error.to_string()))?
            .reply()
            .map_err(|error| EmberError::BackendFailure(error.to_string()))?;
        GammaTable::from_u16_channels(&reply.red, &reply.green, &reply.blue)
            .ok_or(EmberError::InvalidGammaTable)
    }

    fn write_gamma(&self, output: &X11Output, table: &GammaTable) -> EmberResult<()> {
        if output.gamma_size < 2 {
            return Err(EmberError::BackendFailure(
                "Output does not expose a writable gamma table.".into(),
            ));
        }
        let (red, green, blue) = table.to_u16_channels(output.gamma_size as usize);
        randr::set_crtc_gamma(&self.conn, output.crtc, &red, &green, &blue)
            .map_err(|error| EmberError::BackendFailure(error.to_string()))?
            .check()
            .map_err(|error| EmberError::BackendFailure(format!("set gamma failed: {error}")))?;
        self.conn
            .flush()
            .map_err(|error| EmberError::BackendFailure(error.to_string()))?;
        Ok(())
    }
}

impl DisplayBackend for X11Backend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            kind: BackendKind::X11RandR,
            name: "x11-randr",
            verification: VerificationLevel::Readback,
            readback: true,
            exclusive: false,
            note: Some(
                "XRandR returns the current color table, so every write is read back.".into(),
            ),
        }
    }

    fn enumerate(&mut self) -> EmberResult<Vec<DisplayInfo>> {
        Ok(self
            .outputs_snapshot()?
            .into_iter()
            .map(|output| DisplayInfo {
                display_id: output.output,
                identity: output.identity,
                connector: output.connector,
                gamma_size: output.gamma_size,
                is_online: true,
                is_active: true,
                is_mirrored: output.is_mirrored,
                built_in: output.built_in,
                edid: output.edid,
                unsupported_reason: output.unsupported_reason,
            })
            .collect())
    }

    fn capture_baseline(&mut self, identity: &DisplayIdentity) -> EmberResult<DisplayBaseline> {
        let output = self.find(identity)?;
        let table = self.read_gamma(&output)?;
        Ok(DisplayBaseline::new(
            output.output,
            Some(output.identity),
            table,
            chrono::Utc::now(),
        )
        .with_source(GammaSource::Readback))
    }

    fn apply(
        &mut self,
        identity: &DisplayIdentity,
        table: &GammaTable,
    ) -> EmberResult<ApplyReport> {
        let output = self.find(identity)?;
        self.write_gamma(&output, table)?;
        let readback = self.read_gamma(&output)?;
        let delta = readback.maximum_absolute_difference(table);
        if delta >= GAMMA_TOLERANCE {
            return Err(EmberError::GammaVerificationFailed);
        }
        Ok(ApplyReport {
            accepted: true,
            verified: true,
        })
    }

    fn verify(&mut self, identity: &DisplayIdentity, expected: &GammaTable) -> VerifyReport {
        match self.find(identity) {
            Ok(output) => match self.read_gamma(&output) {
                Ok(current) => {
                    let delta = current.maximum_absolute_difference(expected);
                    if delta < GAMMA_TOLERANCE {
                        VerifyReport::Verified
                    } else {
                        VerifyReport::Mismatch {
                            delta_bits: (delta * 65535.0) as u32,
                        }
                    }
                }
                Err(error) => VerifyReport::Failed(error.to_string()),
            },
            Err(error) => VerifyReport::Failed(error.to_string()),
        }
    }

    fn restore(
        &mut self,
        identity: &DisplayIdentity,
        baseline: &DisplayBaseline,
    ) -> EmberResult<()> {
        let output = self.find(identity)?;
        self.write_gamma(&output, &baseline.gamma_table)?;
        let readback = self.read_gamma(&output)?;
        let delta = readback.maximum_absolute_difference(&baseline.gamma_table);
        if delta >= GAMMA_TOLERANCE {
            return Err(EmberError::GammaVerificationFailed);
        }
        debug!("x11: restored {} within {delta}", output.connector);
        Ok(())
    }

    fn release_all(&mut self) -> EmberResult<()> {
        // X11 gamma is not owned by a handle; the daemon restores each journaled
        // baseline explicitly before this call. Nothing to release here.
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<BackendEvent> {
        self.events.drain()
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}

impl Drop for X11Backend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct X11Output {
    output: u32,
    crtc: u32,
    connector: String,
    identity: DisplayIdentity,
    edid: Option<EdidInfo>,
    gamma_size: u32,
    is_mirrored: bool,
    built_in: bool,
    unsupported_reason: Option<String>,
}

fn spawn_randr_watcher(
    root: u32,
    events: mpsc::Sender<BackendEvent>,
    stop: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>, x11rb::errors::ConnectError> {
    // A dedicated connection is required because the main connection is used
    // synchronously for requests; RandR events must not be stolen from it.
    let (conn, _screen) = RustConnection::connect(None)?;
    let mask = randr::NotifyMask::SCREEN_CHANGE
        | randr::NotifyMask::CRTC_CHANGE
        | randr::NotifyMask::OUTPUT_CHANGE;
    let _ = randr::select_input(&conn, root, mask);
    let _ = conn.flush();

    Ok(std::thread::Builder::new()
        .name("ember-x11-hotplug".into())
        .spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match conn.poll_for_event() {
                    Ok(Some(event)) => {
                        if matches!(
                            event,
                            x11rb::protocol::Event::RandrScreenChangeNotify(_)
                                | x11rb::protocol::Event::RandrNotify(_)
                        ) {
                            let _ = events.send(BackendEvent::TopologyChanged);
                        }
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        })
        .expect("spawn X11 hotplug watcher"))
}
