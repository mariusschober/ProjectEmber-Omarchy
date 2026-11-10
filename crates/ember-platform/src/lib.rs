//! Linux platform backends for Project Ember.
//!
//! The daemon depends only on [`backend::DisplayBackend`]; concrete backends are
//! selected at runtime:
//!  * Wayland `wlr-gamma-control` (Hyprland, Sway, other wlroots compositors)
//!  * X11 XRandR
//!  * a deterministic simulator for tests, CI and visual QA
//!
//! Backlight control (built-in panels only) uses logind, brightnessctl or sysfs,
//! in that order. No code in this crate opens a network connection.

pub mod autostart;
pub mod backend;
pub mod backlight;
pub mod command;
pub mod detect;
pub mod drm;
pub mod edid;
pub mod geoclue;
pub mod hyprland;
pub mod logind;
pub mod null;
pub mod sim;
pub mod wayland;
pub mod x11;

pub use autostart::Autostart;
pub use backend::{
    ApplyReport, BackendCapabilities, BackendEvent, DisplayBackend, DisplayInfo, EventSink,
    VerifyReport,
};
pub use backlight::{BacklightCapability, BacklightControl, BacklightController, BacklightDevice};
pub use command::{CommandOutcome, CommandRunner, RecordingRunner, SystemCommandRunner};
pub use detect::{select, SelectedBackend};
pub use drm::DrmSysfs;
pub use edid::EdidInfo;
pub use geoclue::{GeoClueProvider, LocationError, LocationProvider};
pub use hyprland::{HyprlandMonitor, HyprlandProbe, NightlightState};
pub use logind::SleepWatcher;
pub use null::NullBackend;
pub use sim::{SimBacklight, SimDisplay, SimulatedBackend};
