//! Project Ember daemon: display coordinator, tray and D-Bus service.

pub mod coordinator;
pub mod dbus;
pub mod ipc;
pub mod simulate;
pub mod tray;

pub use coordinator::{DaemonBuilder, DaemonCommand, DaemonConfig, DaemonHandle};
