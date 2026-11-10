use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use ember_core::{
    AttentionSeverity, BacklightState, EmberOperationState, EmberPreset, Snapshot,
    TrayPrimaryAction,
};
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;

use crate::diagnostics;
use crate::ipc::{IpcClient, IpcCommand};
use crate::layershell;

const PANEL_WIDTH: i32 = 390;
const PANEL_HEIGHT: i32 = 812;
const PANEL_HEIGHT_WITH_ATTENTION: i32 = 884;

#[derive(Clone)]
pub struct Panel {
    window: gtk::ApplicationWindow,
    client: Option<IpcClient>,
    orb: Orb,
    hero_title: gtk::Label,
    hero_detail: gtk::Label,
    hero_meta: gtk::Label,
    pills: Vec<gtk::Button>,
    warmth_scale: gtk::Scale,
    warmth_value: gtk::Label,
    brightness_scale: gtk::Scale,
    brightness_value: gtk::Label,
    switch_backlight: gtk::Switch,
    switch_schedule: gtk::Switch,
    switch_launch: gtk::Switch,
    location_button: gtk::Button,
    tray_button: gtk::Button,
    attention_box: gtk::Box,
    attention_title: gtk::Label,
    attention_message: gtk::Label,
    retry_button: gtk::Button,
    rendering: Rc<Cell<bool>>,
    last: Rc<RefCell<Snapshot>>,
    online: Rc<Cell<bool>>,
    demo_mode: bool,
}

impl Panel {
    pub fn new(
        app: &gtk::Application,
        client: Option<IpcClient>,
        initial: Option<Snapshot>,
    ) -> Self {
        let demo_mode = client.is_none();
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Project Ember")
            .default_width(PANEL_WIDTH)
            .default_height(PANEL_HEIGHT)
            .resizable(false)
            .decorated(false)
            .build();
        window.add_css_class("ember-window");

        let rendering = Rc::new(Cell::new(false));
        let client_for_panel = client.clone();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("ember-root");

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        panel.add_css_class("ember-panel");
        panel.set_size_request(PANEL_WIDTH - 16, -1);
        panel.set_vexpand(true);

        let wash = gtk::Box::new(gtk::Orientation::Vertical, 0);
        wash.add_css_class("ember-wash");
        wash.set_vexpand(true);
        wash.append(&panel);

        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.set_vexpand(true);
        outer.set_margin_top(8);
        outer.set_margin_bottom(8);
        outer.set_margin_start(8);
        outer.set_margin_end(8);
        outer.append(&wash);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_overlay_scrolling(true);
        scroller.set_min_content_height(0);
        scroller.set_propagate_natural_height(false);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&outer));
        root.append(&scroller);
        window.set_child(Some(&root));

        // Header
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 9);
        header.set_margin_top(14);
        header.set_margin_start(16);
        header.set_margin_end(16);
        let mark = icon_area(IconKind::Ember, 18, "#ff5a38");
        header.append(&mark);
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 1);
        let title = gtk::Label::new(Some("PROJECT EMBER"));
        title.add_css_class("ember-header-title");
        title.set_xalign(0.0);
        let subtitle = gtk::Label::new(Some("CIRCADIAN DISPLAY · OMARCHY"));
        subtitle.add_css_class("ember-header-subtitle");
        subtitle.set_xalign(0.0);
        titles.append(&title);
        titles.append(&subtitle);
        header.append(&titles);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        header.append(&spacer);
        let version = gtk::Label::new(Some(&format!("v{}", ember_core::AppVersion::MARKETING)));
        version.add_css_class("ember-version");
        version.set_valign(gtk::Align::Center);
        header.append(&version);
        panel.append(&header);

        // Hero
        let hero = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        hero.add_css_class("ember-hero");
        hero.set_margin_top(10);
        hero.set_margin_start(16);
        hero.set_margin_end(16);
        hero.set_height_request(122);
        let orb = Orb::new(client_for_panel.clone(), demo_mode);
        hero.append(&orb.widget());
        let hero_text = gtk::Box::new(gtk::Orientation::Vertical, 3);
        hero_text.set_valign(gtk::Align::Center);
        hero_text.set_hexpand(true);
        hero_text.set_margin_end(8);
        let hero_title = gtk::Label::new(Some("Ready"));
        hero_title.add_css_class("ember-hero-title");
        hero_title.set_xalign(0.0);
        hero_title.set_wrap(true);
        hero_title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        hero_title.set_max_width_chars(22);
        let hero_detail = gtk::Label::new(Some("Your original display state is untouched."));
        hero_detail.add_css_class("ember-hero-detail");
        hero_detail.set_xalign(0.0);
        hero_detail.set_wrap(true);
        hero_detail.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        hero_detail.set_max_width_chars(26);
        let hero_meta = gtk::Label::new(Some("2 compatible displays ready"));
        hero_meta.add_css_class("ember-hero-meta");
        hero_meta.set_xalign(0.0);
        hero_meta.set_wrap(true);
        hero_meta.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        hero_meta.set_max_width_chars(28);
        hero_text.append(&hero_title);
        hero_text.append(&hero_detail);
        hero_text.append(&hero_meta);
        hero.append(&hero_text);
        panel.append(&hero);

        // Attention (hidden unless needed)
        let attention_box = gtk::Box::new(gtk::Orientation::Vertical, 3);
        attention_box.add_css_class("ember-attention");
        attention_box.set_margin_top(8);
        attention_box.set_margin_start(16);
        attention_box.set_margin_end(16);
        let attention_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let attention_title = gtk::Label::new(Some("Needs attention"));
        attention_title.add_css_class("ember-attention-title");
        attention_title.set_xalign(0.0);
        attention_title.set_hexpand(true);
        let retry_button = gtk::Button::with_label("Retry");
        retry_button.add_css_class("ember-row-button");
        attention_header.append(&attention_title);
        attention_header.append(&retry_button);
        let attention_message = gtk::Label::new(Some(""));
        attention_message.add_css_class("ember-attention-message");
        attention_message.set_xalign(0.0);
        attention_message.set_wrap(true);
        attention_message.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        attention_box.append(&attention_header);
        attention_box.append(&attention_message);
        attention_box.set_visible(false);
        panel.append(&attention_box);

        // Section + presets
        let section = gtk::Label::new(Some("COLOR MODE"));
        section.add_css_class("ember-section");
        section.set_xalign(0.0);
        section.set_margin_start(18);
        section.set_margin_top(12);
        section.set_margin_bottom(5);
        panel.append(&section);

        let pill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pill.add_css_class("ember-pill");
        pill.set_homogeneous(true);
        pill.set_margin_start(16);
        pill.set_margin_end(16);
        let mut pills = Vec::new();
        for preset in EmberPreset::ALL {
            let button = gtk::Button::with_label(preset.label());
            button.set_hexpand(true);
            button.add_css_class("ember-pill-option");
            if let Some(client) = &client_for_panel {
                let client = client.clone();
                button.connect_clicked(move |_| {
                    client.send(IpcCommand::ApplyPreset(preset.as_str().to_string()));
                });
            }
            pill.append(&button);
            pills.push(button);
        }
        panel.append(&pill);

        // Sliders
        let (warmth_block, warmth_scale, warmth_value) = slider_block(
            "WARMTH",
            IconKind::Sun,
            0.0,
            100.0,
            62.0,
            "warmth",
            client_for_panel.clone(),
            true,
            rendering.clone(),
        );
        panel.append(&warmth_block);
        let (brightness_block, brightness_scale, brightness_value) = slider_block(
            "SOFTWARE BRIGHTNESS",
            IconKind::Dim,
            10.0,
            100.0,
            75.0,
            "brightness",
            client_for_panel.clone(),
            false,
            rendering.clone(),
        );
        panel.append(&brightness_block);

        // Divider
        let divider = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        divider.add_css_class("ember-separator");
        divider.set_margin_start(16);
        divider.set_margin_end(16);
        divider.set_margin_top(6);
        divider.set_margin_bottom(8);
        panel.append(&divider);

        // Settings card
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("ember-card");
        card.set_margin_start(16);
        card.set_margin_end(16);

        let (backlight_row, switch_backlight) = toggle_row(
            IconKind::Shield,
            "Backlight Lock",
            "Hardware at 100%, dimmed in software.",
            client_for_panel.clone(),
            ToggleKind::Backlight,
            rendering.clone(),
        );
        card.append(&backlight_row);
        card.append(&row_separator());

        let (schedule_row, switch_schedule, location_button) =
            schedule_row(client_for_panel.clone(), rendering.clone());
        card.append(&schedule_row);
        card.append(&row_separator());

        let (launch_row, switch_launch) = toggle_row(
            IconKind::Rocket,
            "Launch at login",
            "Start Ember after sign-in.",
            client_for_panel.clone(),
            ToggleKind::Launch,
            rendering.clone(),
        );
        card.append(&launch_row);
        card.append(&row_separator());

        let (behavior_row, tray_button) = behavior_row(client_for_panel.clone());
        card.append(&behavior_row);
        panel.append(&card);

        // Footer actions
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.set_margin_start(16);
        actions.set_margin_end(16);
        actions.set_margin_top(11);
        let diagnostics_button = gtk::Button::with_label("Diagnostics");
        diagnostics_button.add_css_class("ember-action");
        diagnostics_button.set_hexpand(true);
        let quit_button = gtk::Button::with_label("Quit");
        quit_button.add_css_class("ember-action");
        actions.append(&diagnostics_button);
        actions.append(&quit_button);
        panel.append(&actions);

        // Footer bar
        let footer = gtk::Box::new(gtk::Orientation::Vertical, 2);
        footer.set_vexpand(true);
        footer.set_valign(gtk::Align::End);
        footer.set_margin_top(12);
        footer.set_margin_bottom(12);
        footer.set_margin_start(16);
        footer.set_margin_end(16);
        let claim = gtk::Label::new(Some(
            "Cuts short-wavelength light · no network · no accounts",
        ));
        claim.add_css_class("ember-footer");
        claim.set_xalign(0.0);
        claim.set_wrap(true);
        claim.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let link = gtk::Label::new(None);
        link.set_markup("<span foreground='#f0a68d'>mariusschober.com</span>");
        link.add_css_class("ember-link");
        link.set_tooltip_text(Some("https://mariusschober.com/"));
        link.set_cursor_from_name(Some("pointer"));
        link.set_accessible_role(gtk::AccessibleRole::Link);
        let link_gesture = gtk::GestureClick::new();
        link_gesture.connect_released(move |_, _, _, _| {
            if let Err(error) = gtk::gio::AppInfo::launch_default_for_uri(
                "https://mariusschober.com/",
                None::<&gtk::gio::AppLaunchContext>,
            ) {
                eprintln!("Could not open author link: {error}");
            }
        });
        link.add_controller(link_gesture);
        let filler = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        filler.set_hexpand(true);
        let badge = gtk::Label::new(Some("LINUX BETA"));
        badge.add_css_class("ember-badge");
        bottom.append(&link);
        bottom.append(&filler);
        bottom.append(&badge);
        footer.append(&claim);
        footer.append(&bottom);
        panel.append(&footer);

        let retry_for_button = retry_button.clone();
        let client_for_retry = client_for_panel.clone();
        retry_for_button.set_visible(false);
        retry_button.connect_clicked(move |_| {
            if let Some(client) = &client_for_retry {
                client.send(IpcCommand::Retry);
            }
        });

        let client_for_diag = client_for_panel.clone();
        let window_for_diag = window.clone();
        let demo_for_diag = demo_mode;
        diagnostics_button.connect_clicked(move |_| {
            if let Some(app) = window_for_diag.application() {
                diagnostics::open(
                    &app,
                    Some(&window_for_diag),
                    client_for_diag.clone(),
                    demo_for_diag,
                );
            }
        });
        let app_for_quit = app.clone();
        let client_for_quit = client_for_panel.clone();
        quit_button.connect_clicked(move |_| {
            if let Some(client) = &client_for_quit {
                client.send(IpcCommand::Quit);
            }
            app_for_quit.quit();
        });

        let client_for_location = client_for_panel.clone();
        let window_for_location = window.clone();
        location_button.connect_clicked(move |_| {
            location_dialog(&window_for_location, client_for_location.clone());
        });

        let escape = gtk::EventControllerKey::new();
        let window_for_escape = window.clone();
        escape.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk::gdk::Key::Escape {
                window_for_escape.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(escape);

        // Layer-shell panel (Omarchy/Hyprland), with a normal-window fallback.
        let _layer_shell = layershell::configure(window.upcast_ref::<gtk::Window>());

        let panel = Self {
            window,
            client,
            orb,
            hero_title,
            hero_detail,
            hero_meta,
            pills,
            warmth_scale,
            warmth_value,
            brightness_scale,
            brightness_value,
            switch_backlight,
            switch_schedule,
            switch_launch,
            location_button,
            tray_button,
            attention_box,
            attention_title,
            attention_message,
            retry_button,
            rendering,
            last: Rc::new(RefCell::new(Snapshot::empty())),
            online: Rc::new(Cell::new(false)),
            demo_mode,
        };

        panel.render(initial.unwrap_or_else(Snapshot::empty));

        // Poll snapshots at a calm cadence; only re-render on change.
        if let Some(client) = &panel.client {
            let client = client.clone();
            let this = panel.clone();
            glib::timeout_add_local(Duration::from_millis(220), move || {
                let online = client.online();
                if online != this.online.get() {
                    this.online.set(online);
                    if !online {
                        this.render(Snapshot::empty());
                    }
                }
                if online {
                    if let Some(snapshot) = client.snapshot() {
                        if *this.last.borrow() != snapshot {
                            this.render(snapshot);
                        }
                    }
                }
                glib::ControlFlow::Continue
            });
        }

        panel
    }

    pub fn window(&self) -> gtk::ApplicationWindow {
        self.window.clone()
    }

    pub fn open_diagnostics(&self) {
        if let Some(app) = self.window.application() {
            diagnostics::open(
                &app,
                Some(&self.window),
                self.client.clone(),
                self.demo_mode,
            );
        }
    }

    fn render(&self, snapshot: Snapshot) {
        *self.last.borrow_mut() = snapshot.clone();
        self.rendering.set(true);

        let active = snapshot.observed_active;
        self.orb.set_active(active);
        self.orb.set_enabled(
            !snapshot.busy
                && snapshot.backend.verification != ember_core::VerificationLevel::Unavailable,
        );

        // Hero copy — mechanism language, no health absolutes.
        let (title, detail, meta) = hero_copy(&snapshot);
        self.hero_title.set_label(&title);
        self.hero_detail.set_label(&detail);
        self.hero_meta.set_label(&meta);

        let preset = snapshot.settings.preset();
        for (index, preset_value) in EmberPreset::ALL.iter().enumerate() {
            let selected = preset == Some(*preset_value);
            if selected {
                self.pills[index].add_css_class("selected");
            } else {
                self.pills[index].remove_css_class("selected");
            }
        }

        let warmth = snapshot.settings.warmth * 100.0;
        if (self.warmth_scale.value() - warmth).abs() > 0.05 {
            self.warmth_scale.set_value(warmth);
        }
        self.warmth_value
            .set_label(&ember_core::ColorCurve::warmth_description(
                snapshot.settings.warmth,
            ));
        let brightness = snapshot.settings.apparent_brightness * 100.0;
        if (self.brightness_scale.value() - brightness).abs() > 0.05 {
            self.brightness_scale.set_value(brightness);
        }
        self.brightness_value
            .set_label(&format!("{}%", brightness.round() as i64));

        set_switch(
            &self.switch_backlight,
            snapshot.settings.backlight_lock_enabled,
        );
        set_switch(
            &self.switch_schedule,
            snapshot.settings.sun_schedule_enabled,
        );
        set_switch(&self.switch_launch, snapshot.settings.launch_at_login);

        let needs_location = snapshot.solar.enabled
            && matches!(
                snapshot.solar.location_state,
                ember_core::LocationState::NeedsLocation
                    | ember_core::LocationState::Denied
                    | ember_core::LocationState::Unavailable
            );
        self.location_button.set_visible(needs_location);

        self.tray_button
            .set_label(snapshot.settings.tray_primary_action.label());

        let attention = &snapshot.attention;
        let show_attention = attention.severity >= AttentionSeverity::Warning;
        self.attention_box.set_visible(show_attention);
        if show_attention {
            self.attention_title.set_label(&attention.title);
            self.attention_message.set_label(&attention.message);
            self.retry_button.set_visible(
                attention
                    .recovery_actions
                    .iter()
                    .any(|action| matches!(action, ember_core::AttentionAction::Retry)),
            );
        }
        let desired_height = if show_attention {
            PANEL_HEIGHT_WITH_ATTENTION
        } else {
            PANEL_HEIGHT
        };
        // Leave room for the Omarchy bar and a small gap. If the screen is
        // shorter, the content scrolls instead of extending off-screen.
        let screen_height = gtk::gdk::Display::default()
            .and_then(|display| display.monitors().item(0))
            .and_then(|object| object.downcast::<gtk::gdk::Monitor>().ok())
            .map(|monitor| monitor.geometry().height())
            .unwrap_or(desired_height);
        let available = (screen_height - 56).max(480);
        self.window
            .set_default_size(PANEL_WIDTH, desired_height.min(available));

        // Backlight caption reflects truthful engagement.
        if let Some(backlight_row) = self.switch_backlight.parent().and_then(|parent| {
            parent
                .downcast::<gtk::Box>()
                .ok()
                .and_then(|row| row.last_child())
                .and_then(|child| child.downcast::<gtk::Box>().ok())
        }) {
            if let Some(detail) = backlight_row
                .last_child()
                .and_then(|c| c.downcast::<gtk::Label>().ok())
            {
                let text = match snapshot.backlight.state {
                    BacklightState::Engaged => "Hardware brightness locked at 100%.",
                    BacklightState::Unavailable => {
                        "Unavailable on this display; preference retained."
                    }
                    BacklightState::Failed => "Stopped after repeated write failures.",
                    BacklightState::Disengaged => "Hardware at 100%, dimmed in software.",
                };
                if detail.label() != text {
                    detail.set_label(text);
                }
            }
        }

        self.rendering.set(false);
        self.window.queue_draw();
    }
}

fn set_switch(switch: &gtk::Switch, value: bool) {
    if switch.is_active() != value {
        switch.set_active(value);
    }
}

fn hero_copy(snapshot: &Snapshot) -> (String, String, String) {
    let settings = &snapshot.settings;
    let active = snapshot.observed_active;
    let attention = &snapshot.attention;
    if attention.severity >= AttentionSeverity::Warning {
        return (
            attention.title.clone(),
            attention.message.clone(),
            "Open Diagnostics for details.".into(),
        );
    }
    match snapshot.runtime_state {
        EmberOperationState::Active if active => {
            let title = match EmberPreset::from_warmth(settings.warmth) {
                Some(EmberPreset::PureRed) => "Pure Red is on".to_string(),
                Some(EmberPreset::Evening) => "Evening light is on".to_string(),
                Some(EmberPreset::Neutral) => "Neutral is on".to_string(),
                None => format!(
                    "{} is on",
                    ember_core::ColorCurve::warmth_description(settings.warmth)
                ),
            };
            let detail = match EmberPreset::from_warmth(settings.warmth) {
                Some(EmberPreset::PureRed) => {
                    "A red-channel-only display mode for low-light evenings."
                }
                Some(EmberPreset::Evening) => {
                    "Reduces short-wavelength display output for evening use."
                }
                Some(EmberPreset::Neutral) => "Software dimming with original color preserved.",
                None => {
                    "Lower melanopic output can be less disruptive at night; sensitivity varies."
                }
            };
            let mut meta = format!(
                "Active on {} {}",
                snapshot.counts.verified,
                if snapshot.counts.verified == 1 {
                    "display"
                } else {
                    "displays"
                }
            );
            if snapshot.pending_restore_count > 0 {
                meta.push_str(&format!(" · {} pending", snapshot.pending_restore_count));
            }
            if let Some(event) = snapshot.solar.next_event {
                if settings.sun_schedule_enabled {
                    let local = event.date.with_timezone(&chrono::Local);
                    meta = format!("Next: {} at {}", event.kind.label(), local.format("%H:%M"));
                }
            }
            (title, detail.to_string(), meta)
        }
        EmberOperationState::Activating | EmberOperationState::Reconciling => (
            "Applying…".into(),
            "Saving every display state first…".into(),
            "Verifying every display".into(),
        ),
        EmberOperationState::Restoring => (
            "Restoring…".into(),
            "Returning every display to its saved state…".into(),
            "Verifying the original table".into(),
        ),
        EmberOperationState::Suspended => (
            "Paused for sleep".into(),
            "Ember will safely re-evaluate after wake.".into(),
            "Sleep".into(),
        ),
        EmberOperationState::Degraded => (
            "Needs attention".into(),
            snapshot
                .last_error
                .clone()
                .unwrap_or_else(|| snapshot.status_detail.clone()),
            "Open Diagnostics".into(),
        ),
        _ => {
            if snapshot.counts.online_intended() == 0
                && snapshot.displays.is_empty()
                && snapshot.status_detail == "Display control is not running."
            {
                (
                    "Ember is not running".into(),
                    "Start the display controller, then Ember can warm or dim this screen.".into(),
                    "Display control is not running".into(),
                )
            } else if snapshot.counts.online_intended() == 0 && snapshot.displays.is_empty() {
                (
                    "No compatible display".into(),
                    "Connect a display that supports gamma control (Hyprland, Sway, or X11)."
                        .into(),
                    snapshot.backend.name.clone(),
                )
            } else if snapshot.pending_restore_count > 0 {
                (
                    "Waiting for display".into(),
                    format!(
                        "Your original colors are saved for {} disconnected display(s).",
                        snapshot.pending_restore_count
                    ),
                    "Restores automatically on reconnect".into(),
                )
            } else {
                (
                    "Ready".into(),
                    "Your original display state is untouched.".into(),
                    format!(
                        "{} compatible {} ready",
                        snapshot
                            .displays
                            .iter()
                            .filter(|display| display.gamma_compatible)
                            .count(),
                        if snapshot
                            .displays
                            .iter()
                            .filter(|display| display.gamma_compatible)
                            .count()
                            == 1
                        {
                            "display"
                        } else {
                            "displays"
                        }
                    ),
                )
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn slider_block(
    title: &str,
    icon: IconKind,
    minimum: f64,
    maximum: f64,
    initial: f64,
    css: &str,
    client: Option<IpcClient>,
    warmth: bool,
    rendering: Rc<Cell<bool>>,
) -> (gtk::Box, gtk::Scale, gtk::Label) {
    let block = gtk::Box::new(gtk::Orientation::Vertical, 2);
    block.set_margin_start(16);
    block.set_margin_end(16);
    block.set_margin_top(7);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    let icon = icon_area(icon, 15, "#c8b8b1");
    header.append(&icon);
    let label = gtk::Label::new(Some(title));
    label.add_css_class("ember-slider-title");
    label.set_xalign(0.0);
    label.set_hexpand(true);
    let value = gtk::Label::new(Some(""));
    value.add_css_class("ember-slider-value");
    value.set_xalign(1.0);
    header.append(&label);
    header.append(&value);
    block.append(&header);

    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, minimum, maximum, 1.0);
    scale.set_draw_value(false);
    scale.set_value(initial);
    scale.set_hexpand(true);
    scale.add_css_class(css);
    scale.set_margin_top(2);
    block.append(&scale);

    let pending = Rc::new(Cell::new(initial));
    let scheduled = Rc::new(Cell::new(false));
    let rendering_for_scale = rendering.clone();
    let client_for_scale = client;
    scale.connect_value_changed(move |scale| {
        if rendering_for_scale.get() {
            return;
        }
        pending.set(scale.value());
        if scheduled.get() {
            return;
        }
        scheduled.set(true);
        let pending = pending.clone();
        let scheduled = scheduled.clone();
        let client = client_for_scale.clone();
        glib::timeout_add_local_once(Duration::from_millis(55), move || {
            scheduled.set(false);
            if let Some(client) = &client {
                let value = pending.get() / 100.0;
                client.send(if warmth {
                    IpcCommand::SetWarmth(value)
                } else {
                    IpcCommand::SetBrightness(value)
                });
            }
        });
    });

    (block, scale, value)
}

#[derive(Clone, Copy)]
enum ToggleKind {
    Backlight,
    Launch,
}

fn toggle_row(
    icon: IconKind,
    title: &str,
    detail: &str,
    client: Option<IpcClient>,
    kind: ToggleKind,
    rendering: Rc<Cell<bool>>,
) -> (gtk::Box, gtk::Switch) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("ember-row");
    row.append(&icon_area(icon, 18, "#cdbfb8"));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    let title_label = gtk::Label::new(Some(title));
    title_label.add_css_class("ember-row-title");
    title_label.set_xalign(0.0);
    let detail_label = gtk::Label::new(Some(detail));
    detail_label.add_css_class("ember-row-detail");
    detail_label.set_xalign(0.0);
    detail_label.set_wrap(true);
    detail_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    detail_label.set_max_width_chars(34);
    text.append(&title_label);
    text.append(&detail_label);
    row.append(&text);
    let switch = gtk::Switch::new();
    switch.add_css_class("ember-switch");
    switch.set_valign(gtk::Align::Center);
    let client_for_switch = client;
    switch.connect_state_set(move |_, state| {
        if rendering.get() {
            return glib::Propagation::Proceed;
        }
        if let Some(client) = &client_for_switch {
            client.send(match kind {
                ToggleKind::Backlight => IpcCommand::SetBacklightLock(state),
                ToggleKind::Launch => IpcCommand::SetLaunchAtLogin(state),
            });
        }
        glib::Propagation::Proceed
    });
    row.append(&switch);
    (row, switch)
}

fn schedule_row(
    client: Option<IpcClient>,
    rendering: Rc<Cell<bool>>,
) -> (gtk::Box, gtk::Switch, gtk::Button) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("ember-row");
    row.append(&icon_area(IconKind::Clock, 18, "#cdbfb8"));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    let title = gtk::Label::new(Some("Sun schedule"));
    title.add_css_class("ember-row-title");
    title.set_xalign(0.0);
    let detail = gtk::Label::new(Some("On at sunset, restored at sunrise."));
    detail.add_css_class("ember-row-detail");
    detail.set_xalign(0.0);
    detail.set_wrap(true);
    detail.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    detail.set_max_width_chars(34);
    let caption = gtk::Label::new(Some("Uses approximate location on-device."));
    caption.add_css_class("ember-row-caption");
    caption.set_xalign(0.0);
    let location_button = gtk::Button::with_label("Set Location…");
    location_button.add_css_class("ember-row-button");
    location_button.set_halign(gtk::Align::Start);
    location_button.set_visible(false);
    text.append(&title);
    text.append(&detail);
    text.append(&caption);
    text.append(&location_button);
    row.append(&text);
    let switch = gtk::Switch::new();
    switch.add_css_class("ember-switch");
    switch.set_valign(gtk::Align::Center);
    let client_for_switch = client;
    switch.connect_state_set(move |_, state| {
        if rendering.get() {
            return glib::Propagation::Proceed;
        }
        if let Some(client) = &client_for_switch {
            client.send(IpcCommand::SetSunSchedule(state));
        }
        glib::Propagation::Proceed
    });
    row.append(&switch);
    (row, switch, location_button)
}

fn behavior_row(client: Option<IpcClient>) -> (gtk::Box, gtk::Button) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("ember-row");
    row.append(&icon_area(IconKind::Cursor, 18, "#cdbfb8"));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    let title = gtk::Label::new(Some("Tray click"));
    title.add_css_class("ember-row-title");
    title.set_xalign(0.0);
    let detail = gtk::Label::new(Some("What a left click does."));
    detail.add_css_class("ember-row-detail");
    detail.set_xalign(0.0);
    text.append(&title);
    text.append(&detail);
    row.append(&text);
    let button = gtk::Button::with_label(TrayPrimaryAction::OpenControls.label());
    button.add_css_class("ember-action");
    button.set_valign(gtk::Align::Center);
    let client_for_button = client;
    button.connect_clicked(move |button| {
        let next = if button.label().as_deref() == Some("Open Controls") {
            TrayPrimaryAction::ToggleEmber
        } else {
            TrayPrimaryAction::OpenControls
        };
        button.set_label(next.label());
        if let Some(client) = &client_for_button {
            client.send(IpcCommand::SetTrayPrimaryAction(next.as_str().to_string()));
        }
    });
    row.append(&button);
    (row, button)
}

fn row_separator() -> gtk::Box {
    let separator = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    separator.add_css_class("ember-separator");
    separator.set_margin_start(12);
    separator.set_margin_end(12);
    separator
}

fn location_dialog(parent: &gtk::ApplicationWindow, client: Option<IpcClient>) {
    let Some(app) = parent.application() else {
        return;
    };
    let dialog = gtk::ApplicationWindow::builder()
        .application(&app)
        .title("Sun schedule location")
        .transient_for(parent)
        .modal(true)
        .default_width(340)
        .resizable(false)
        .build();
    dialog.add_css_class("ember-window");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.add_css_class("ember-panel");
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);
    let title = gtk::Label::new(Some("Approximate location"));
    title.add_css_class("ember-hero-title");
    title.set_xalign(0.0);
    let detail = gtk::Label::new(Some(
        "Coordinates are rounded to 0.1° and stored only on this computer. You can also let GeoClue provide a fix from the location row.",
    ));
    detail.add_css_class("ember-row-detail");
    detail.set_xalign(0.0);
    detail.set_wrap(true);
    detail.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    let latitude = gtk::Entry::new();
    latitude.set_placeholder_text(Some("Latitude, e.g. 52.5"));
    let longitude = gtk::Entry::new();
    longitude.set_placeholder_text(Some("Longitude, e.g. 13.4"));
    let error = gtk::Label::new(None);
    error.add_css_class("ember-attention-message");
    error.set_xalign(0.0);
    error.set_wrap(true);
    error.set_visible(false);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("ember-action");
    cancel.add_css_class("ember-action");
    buttons.append(&cancel);
    buttons.append(&save);
    root.append(&title);
    root.append(&detail);
    root.append(&latitude);
    root.append(&longitude);
    root.append(&error);
    root.append(&buttons);
    dialog.set_child(Some(&root));
    let dialog_for_cancel = dialog.clone();
    cancel.connect_clicked(move |_| {
        dialog_for_cancel.close();
    });
    let dialog_for_save = dialog.clone();
    let error_for_save = error.clone();
    save.connect_clicked(move |_| {
        let lat = latitude.text().trim().parse::<f64>();
        let lon = longitude.text().trim().parse::<f64>();
        match (lat, lon) {
            (Ok(lat), Ok(lon))
                if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) =>
            {
                if let Some(client) = &client {
                    client.send(IpcCommand::SetLocation(lat, lon));
                }
                dialog_for_save.close();
            }
            _ => {
                error_for_save.set_label(
                    "Enter a latitude between -90 and 90 and a longitude between -180 and 180.",
                );
                error_for_save.set_visible(true);
            }
        }
    });
    dialog.present();
}

// MARK: - Icons and orb

#[derive(Clone, Copy)]
enum IconKind {
    Ember,
    Sun,
    Dim,
    Shield,
    Clock,
    Rocket,
    Cursor,
}

fn icon_area(kind: IconKind, size: i32, color: &'static str) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(size);
    area.set_content_height(size);
    area.set_size_request(size, size);
    area.add_css_class("ember-row-icon");
    let rgba = parse_hex(color);
    area.set_draw_func(move |_, cr, width, height| {
        draw_icon(cr, kind, width as f64, height as f64, rgba);
    });
    area
}

fn parse_hex(value: &str) -> (f64, f64, f64) {
    let value = value.trim_start_matches('#');
    if value.len() < 6 {
        return (0.85, 0.8, 0.77);
    }
    let parse = |offset: usize| {
        u8::from_str_radix(&value[offset..offset + 2], 16).unwrap_or(200) as f64 / 255.0
    };
    (parse(0), parse(2), parse(4))
}

fn draw_icon(
    cr: &gtk::cairo::Context,
    kind: IconKind,
    width: f64,
    height: f64,
    color: (f64, f64, f64),
) {
    let scale = width.min(height);
    let cx = width / 2.0;
    let cy = height / 2.0;
    cr.set_source_rgb(color.0, color.1, color.2);
    cr.set_line_width(1.45);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    match kind {
        IconKind::Ember => {
            let gradient = gtk::cairo::RadialGradient::new(cx, cy, 0.5, cx, cy, scale / 2.0);
            gradient.add_color_stop_rgba(0.0, 1.0, 0.55, 0.35, 1.0);
            gradient.add_color_stop_rgba(1.0, 0.78, 0.18, 0.09, 1.0);
            cr.set_source(&gradient).ok();
            cr.arc(cx, cy, scale / 2.0, 0.0, std::f64::consts::TAU);
            cr.fill().ok();
        }
        IconKind::Sun => {
            cr.arc(cx, cy, scale * 0.16, 0.0, std::f64::consts::TAU);
            cr.stroke().ok();
            for index in 0..8 {
                let angle = index as f64 * std::f64::consts::TAU / 8.0;
                let inner = scale * 0.27;
                let outer = scale * 0.38;
                cr.move_to(cx + angle.cos() * inner, cy + angle.sin() * inner);
                cr.line_to(cx + angle.cos() * outer, cy + angle.sin() * outer);
                cr.stroke().ok();
            }
        }
        IconKind::Dim => {
            cr.arc(cx, cy, scale * 0.20, 0.0, std::f64::consts::TAU);
            cr.stroke().ok();
            for index in 0..4 {
                let angle = index as f64 * std::f64::consts::TAU / 4.0;
                cr.move_to(
                    cx + angle.cos() * scale * 0.28,
                    cy + angle.sin() * scale * 0.28,
                );
                cr.line_to(
                    cx + angle.cos() * scale * 0.40,
                    cy + angle.sin() * scale * 0.40,
                );
                cr.stroke().ok();
            }
        }
        IconKind::Shield => {
            cr.move_to(cx, cy - scale * 0.34);
            cr.line_to(cx + scale * 0.28, cy - scale * 0.20);
            cr.line_to(cx + scale * 0.24, cy + scale * 0.12);
            cr.curve_to(
                cx + scale * 0.18,
                cy + scale * 0.28,
                cx + scale * 0.06,
                cy + scale * 0.36,
                cx,
                cy + scale * 0.38,
            );
            cr.curve_to(
                cx - scale * 0.06,
                cy + scale * 0.36,
                cx - scale * 0.18,
                cy + scale * 0.28,
                cx - scale * 0.24,
                cy + scale * 0.12,
            );
            cr.line_to(cx - scale * 0.28, cy - scale * 0.20);
            cr.close_path();
            cr.stroke().ok();
            cr.move_to(cx - scale * 0.10, cy);
            cr.line_to(cx - scale * 0.02, cy + scale * 0.09);
            cr.line_to(cx + scale * 0.12, cy - scale * 0.08);
            cr.stroke().ok();
        }
        IconKind::Clock => {
            cr.arc(cx, cy, scale * 0.30, 0.0, std::f64::consts::TAU);
            cr.stroke().ok();
            cr.move_to(cx, cy - scale * 0.19);
            cr.line_to(cx, cy);
            cr.line_to(cx + scale * 0.12, cy + scale * 0.07);
            cr.stroke().ok();
        }
        IconKind::Rocket => {
            cr.move_to(cx, cy - scale * 0.34);
            cr.curve_to(
                cx + scale * 0.18,
                cy - scale * 0.18,
                cx + scale * 0.14,
                cy + scale * 0.08,
                cx + scale * 0.05,
                cy + scale * 0.22,
            );
            cr.line_to(cx - scale * 0.05, cy + scale * 0.22);
            cr.curve_to(
                cx - scale * 0.14,
                cy + scale * 0.08,
                cx - scale * 0.18,
                cy - scale * 0.18,
                cx,
                cy - scale * 0.34,
            );
            cr.stroke().ok();
            cr.move_to(cx - scale * 0.12, cy + scale * 0.20);
            cr.line_to(cx - scale * 0.18, cy + scale * 0.34);
            cr.move_to(cx + scale * 0.12, cy + scale * 0.20);
            cr.line_to(cx + scale * 0.18, cy + scale * 0.34);
            cr.stroke().ok();
        }
        IconKind::Cursor => {
            cr.move_to(cx - scale * 0.14, cy - scale * 0.30);
            cr.line_to(cx + scale * 0.26, cy + scale * 0.04);
            cr.line_to(cx + scale * 0.04, cy + scale * 0.08);
            cr.line_to(cx - scale * 0.02, cy + scale * 0.30);
            cr.close_path();
            cr.stroke().ok();
        }
    }
}

#[derive(Clone)]
struct Orb {
    area: gtk::DrawingArea,
    active: Rc<Cell<bool>>,
    enabled: Rc<Cell<bool>>,
}

impl Orb {
    fn new(client: Option<IpcClient>, demo: bool) -> Self {
        let area = gtk::DrawingArea::new();
        area.set_content_width(124);
        area.set_content_height(122);
        area.set_size_request(124, 122);
        area.set_hexpand(false);
        area.set_focusable(true);
        area.set_accessible_role(gtk::AccessibleRole::Button);
        area.update_property(&[gtk::accessible::Property::Label(
            "Turn the Ember display filter on or off",
        )]);
        let active = Rc::new(Cell::new(false));
        let hover = Rc::new(Cell::new(false));
        let enabled = Rc::new(Cell::new(true));
        let active_draw = active.clone();
        let hover_draw = hover.clone();
        let enabled_draw = enabled.clone();
        area.set_draw_func(move |_, cr, width, height| {
            draw_orb(
                cr,
                width as f64,
                height as f64,
                active_draw.get(),
                hover_draw.get(),
                enabled_draw.get(),
            );
        });
        let enabled_click = enabled.clone();
        let gesture = gtk::GestureClick::new();
        let client_for_click = client.clone();
        let area_for_click = area.clone();
        gesture.connect_pressed(move |_, _, _, _| {
            if !enabled_click.get() || demo {
                return;
            }
            if let Some(client) = &client_for_click {
                client.send(IpcCommand::Toggle);
            }
            area_for_click.queue_draw();
        });
        area.add_controller(gesture);
        let motion = gtk::EventControllerMotion::new();
        let area_for_motion = area.clone();
        let hover_enter = hover.clone();
        motion.connect_enter(move |_, _, _| {
            hover_enter.set(true);
            area_for_motion.set_cursor_from_name(Some("pointer"));
            area_for_motion.queue_draw();
        });
        let area_for_leave = area.clone();
        let hover_leave = hover.clone();
        motion.connect_leave(move |_| {
            hover_leave.set(false);
            area_for_leave.queue_draw();
        });
        area.add_controller(motion);
        // Keyboard activation.
        let key = gtk::EventControllerKey::new();
        let client_for_key = client.clone();
        let enabled_key = enabled.clone();
        key.connect_key_pressed(move |_, keyval, _, _| {
            if (keyval == gtk::gdk::Key::space || keyval == gtk::gdk::Key::Return)
                && enabled_key.get()
                && !demo
            {
                if let Some(client) = &client_for_key {
                    client.send(IpcCommand::Toggle);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        area.add_controller(key);
        let focus = gtk::EventControllerFocus::new();
        let area_for_focus = area.clone();
        focus.connect_enter(move |_| area_for_focus.queue_draw());
        area.add_controller(focus);
        Self {
            area,
            active,
            enabled,
        }
    }

    fn widget(&self) -> gtk::DrawingArea {
        self.area.clone()
    }

    fn set_active(&self, value: bool) {
        if self.active.get() != value {
            self.active.set(value);
            self.area.queue_draw();
        }
    }

    fn set_enabled(&self, value: bool) {
        if self.enabled.get() != value {
            self.enabled.set(value);
            self.area.queue_draw();
        }
    }
}

fn draw_orb(
    cr: &gtk::cairo::Context,
    width: f64,
    height: f64,
    active: bool,
    hover: bool,
    enabled: bool,
) {
    let orb_size = 72.0;
    let cx = width / 2.0;
    let cy = (height - orb_size) / 2.0 + 8.0 + orb_size / 2.0;
    let mix = if hover && enabled { 0.4 } else { 0.0 };
    let blend = |a: (f64, f64, f64), b: (f64, f64, f64)| {
        (
            a.0 + (b.0 - a.0) * mix,
            a.1 + (b.1 - a.1) * mix,
            a.2 + (b.2 - a.2) * mix,
        )
    };
    let (mut r0, mut g0, mut b0) = if active {
        (1.0, 0.38, 0.22)
    } else {
        (0.52, 0.52, 0.52)
    };
    let (r1, g1, b1) = if active {
        (1.0, 0.22, 0.12)
    } else {
        (0.42, 0.42, 0.42)
    };
    let (r2, g2, b2) = if active {
        (0.78, 0.14, 0.09)
    } else {
        (0.32, 0.32, 0.32)
    };
    if hover {
        let target = if active {
            (0.60, 0.60, 0.60)
        } else {
            (1.0, 0.30, 0.18)
        };
        let base = blend((r0, g0, b0), target);
        r0 = base.0;
        g0 = base.1;
        b0 = base.2;
    }
    let glow_alpha = if active { 0.30 } else { 0.10 } * if enabled { 1.0 } else { 0.4 };
    cr.set_source_rgba(1.0, 0.28, 0.16, glow_alpha);
    cr.arc(cx, cy, orb_size / 2.0 + 15.0, 0.0, std::f64::consts::TAU);
    cr.fill().ok();

    let gradient = gtk::cairo::RadialGradient::new(
        cx - orb_size * 0.16,
        cy - orb_size * 0.18,
        2.0,
        cx,
        cy,
        orb_size * 0.62,
    );
    gradient.add_color_stop_rgba(0.0, r0, g0, b0, if enabled { 1.0 } else { 0.6 });
    gradient.add_color_stop_rgba(0.55, r1, g1, b1, if enabled { 1.0 } else { 0.6 });
    gradient.add_color_stop_rgba(1.0, r2, g2, b2, if enabled { 1.0 } else { 0.6 });
    cr.set_source(&gradient).ok();
    cr.arc(cx, cy, orb_size / 2.0, 0.0, std::f64::consts::TAU);
    cr.fill().ok();

    // Specular crescent.
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.10);
    cr.save().ok();
    cr.translate(cx + orb_size * 0.05, cy - orb_size * 0.29);
    cr.scale(1.0, 0.42);
    cr.arc(0.0, 0.0, orb_size * 0.24, 0.0, std::f64::consts::TAU);
    cr.fill().ok();
    cr.restore().ok();

    // Power glyph.
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.26);
    cr.set_line_width(2.2);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    let glyph_cy = cy + 8.0;
    cr.arc(cx, glyph_cy - 2.0, 8.5, -2.2, 2.2);
    cr.stroke().ok();
    cr.move_to(cx, glyph_cy - 13.0);
    cr.line_to(cx, glyph_cy - 3.0);
    cr.stroke().ok();

    // Waves.
    for index in 0..3 {
        let y = cy + orb_size / 2.0 + 7.0 + index as f64 * 5.0;
        let wave_width = width * 0.58;
        let start = (width - wave_width) / 2.0;
        cr.set_source_rgba(0.55, 0.45, 0.42, 0.05 - index as f64 * 0.012);
        cr.set_line_width(0.9);
        cr.move_to(start, y);
        cr.curve_to(
            start + wave_width * 0.15,
            y + 4.0,
            start + wave_width * 0.35,
            y - 4.0,
            start + wave_width * 0.5,
            y,
        );
        cr.curve_to(
            start + wave_width * 0.65,
            y + 3.0,
            start + wave_width * 0.85,
            y - 3.0,
            start + wave_width,
            y,
        );
        cr.stroke().ok();
    }
}

// MARK: - Screenshot

pub fn schedule_screenshot(window: &gtk::ApplicationWindow, path: PathBuf, app: gtk::Application) {
    window.present();
    let window = window.clone();
    glib::timeout_add_local_once(Duration::from_millis(900), move || {
        let result = render_to_png(&window, &path);
        match result {
            Ok(()) => println!("Wrote {}", path.display()),
            Err(error) => eprintln!("Screenshot failed: {error}"),
        }
        app.quit();
    });
}

fn render_to_png(window: &gtk::ApplicationWindow, path: &PathBuf) -> Result<(), String> {
    let width = window.width();
    let height = window.height();
    if width <= 1 || height <= 1 {
        return Err(format!("window has no size ({width}x{height})"));
    }
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let node = snapshot
        .to_node()
        .ok_or_else(|| "could not build render node".to_string())?;
    let native = window
        .native()
        .ok_or_else(|| "window is not realized".to_string())?;
    let renderer = native
        .renderer()
        .ok_or_else(|| "window has no renderer".to_string())?;
    let texture = renderer.render_texture(
        &node,
        Some(&gtk::graphene::Rect::new(
            0.0,
            0.0,
            width as f32,
            height as f32,
        )),
    );
    texture.save_to_png(path).map_err(|error| error.to_string())
}
