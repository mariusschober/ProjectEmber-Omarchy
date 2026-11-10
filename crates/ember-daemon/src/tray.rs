use std::sync::mpsc::Sender;

use ember_core::{EmberPreset, Snapshot, TrayPrimaryAction};
use ksni::blocking::{Handle as TrayHandle, TrayMethods};
use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::{Icon, MenuItem, ToolTip, Tray};
use log::warn;

use crate::coordinator::DaemonCommand;

/// StatusNotifierItem tray shown by Omarchy's Quickshell bar (or any SNI host).
/// Left click follows the saved preference; the context menu is always the full
/// menu.
pub struct EmberTray {
    pub snapshot: Snapshot,
    command_tx: Sender<DaemonCommand>,
}

impl Tray for EmberTray {
    const MENU_ON_ACTIVATE: bool = false;

    fn id(&self) -> String {
        "project-ember".into()
    }

    fn title(&self) -> String {
        "Project Ember".into()
    }

    fn icon_name(&self) -> String {
        "project-ember-symbolic".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![render_icon(64), render_icon(32)]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: "project-ember-symbolic".into(),
            icon_pixmap: vec![render_icon(32)],
            title: self.snapshot.status_title.clone(),
            description: self.snapshot.tray_tooltip(),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        match self.snapshot.settings.tray_primary_action {
            TrayPrimaryAction::OpenControls => {
                let _ = self.command_tx.send(DaemonCommand::OpenPanel);
            }
            TrayPrimaryAction::ToggleEmber => {
                let _ = self.command_tx.send(DaemonCommand::Toggle);
            }
        }
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        let _ = self.command_tx.send(DaemonCommand::OpenPanel);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let active = self.snapshot.observed_active
            || (self.snapshot.settings.filter_enabled
                && self.snapshot.runtime_state == ember_core::EmberOperationState::Active);
        let preset = self.snapshot.settings.preset();
        let pending = self.snapshot.pending_restore_count;
        vec![
            StandardItem {
                label: "Open Ember".into(),
                icon_name: "project-ember-symbolic".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.command_tx.send(DaemonCommand::OpenPanel);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: if active {
                    "Turn Ember Off".into()
                } else {
                    "Turn Ember On".into()
                },
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.command_tx.send(DaemonCommand::Toggle);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            preset_item(EmberPreset::Neutral, preset, active),
            preset_item(EmberPreset::Evening, preset, active),
            preset_item(EmberPreset::PureRed, preset, active),
            MenuItem::Separator,
            StandardItem {
                label: "Diagnostics…".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.command_tx.send(DaemonCommand::OpenDiagnostics);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: if pending > 0 {
                    format!("Restore Display Now ({pending} pending)")
                } else {
                    "Restore Display Now".into()
                },
                enabled: active || pending > 0,
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.command_tx.send(DaemonCommand::RestoreNow);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit Project Ember".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.command_tx.send(DaemonCommand::Shutdown);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn preset_item(
    value: EmberPreset,
    selected: Option<EmberPreset>,
    active: bool,
) -> MenuItem<EmberTray> {
    CheckmarkItem {
        label: value.label().into(),
        checked: active && selected == Some(value),
        enabled: true,
        activate: Box::new(move |tray: &mut EmberTray| {
            let _ = tray.command_tx.send(DaemonCommand::ApplyPreset(value));
        }),
        ..Default::default()
    }
    .into()
}

/// Spawns the tray. Failure is non-fatal: Ember keeps running headlessly and
/// the GUI/CLI still work.
pub fn spawn(
    command_tx: Sender<DaemonCommand>,
    snapshot: Snapshot,
) -> Option<TrayHandle<EmberTray>> {
    let tray = EmberTray {
        snapshot,
        command_tx,
    };
    match tray.spawn() {
        Ok(handle) => Some(handle),
        Err(error) => {
            warn!("System tray unavailable: {error}");
            None
        }
    }
}

/// Draws the ember mark as ARGB32 (network byte order). Kept tiny so the tray
/// works even without an installed icon theme.
pub fn render_icon(size: i32) -> Icon {
    let size = size.max(8);
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    let centre = (size as f32 - 1.0) * 0.5;
    let radius = size as f32 * 0.40;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - centre;
            let dy = y as f32 - centre + size as f32 * 0.04;
            let distance = (dx * dx + dy * dy).sqrt();
            let (alpha, red, green, blue) = if distance <= radius {
                let t = (distance / radius).clamp(0.0, 1.0);
                (
                    255u8,
                    255u8,
                    (74.0 + (40.0 * (1.0 - t))) as u8,
                    (30.0 + (30.0 * (1.0 - t))) as u8,
                )
            } else if distance <= radius + 3.0 {
                let glow = 1.0 - ((distance - radius) / 3.0).clamp(0.0, 1.0);
                ((90.0 * glow) as u8, 255, 74, 46)
            } else {
                (0, 0, 0, 0)
            };
            data.extend_from_slice(&[alpha, red, green, blue]);
        }
    }
    Icon {
        width: size,
        height: size,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_expected_shape_and_alpha() {
        let icon = render_icon(16);
        assert_eq!(icon.width, 16);
        assert_eq!(icon.height, 16);
        assert_eq!(icon.data.len(), 16 * 16 * 4);
        let centre = ((8 * 16) + 8) * 4;
        assert_eq!(icon.data[centre], 255);
        assert_eq!(icon.data[3], 0);
    }

    #[test]
    fn icon_size_is_sanitized() {
        let icon = render_icon(-4);
        assert_eq!(icon.width, 8);
    }
}
