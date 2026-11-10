mod demo;
mod diagnostics;
mod ipc;
mod layershell;
mod theme;
mod window;

use std::path::PathBuf;

use clap::Parser;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::prelude::*;

#[derive(Parser, Debug)]
#[command(
    name = "ember",
    version,
    about = "Project Ember control panel for Omarchy",
    disable_version_flag = true
)]
struct Args {
    /// Show the control panel (default).
    #[arg(long)]
    show: bool,

    /// Open the diagnostics window.
    #[arg(long)]
    diagnostics: bool,

    /// Render the panel to a PNG and exit (visual QA).
    #[arg(long, value_name = "PATH")]
    screenshot: Option<PathBuf>,

    /// Demo state for screenshots: active, off or degraded.
    #[arg(long, default_value = "active")]
    state: String,

    /// Use demo data instead of the running daemon.
    #[arg(long)]
    demo: bool,

    /// Print version and exit.
    #[arg(long)]
    version: bool,
}

fn main() {
    let args = Args::parse();
    if args.version {
        println!(
            "Project Ember {} · GTK {}.{}",
            ember_core::AppVersion::full(),
            gtk::major_version(),
            gtk::minor_version()
        );
        return;
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let mut flags = gio::ApplicationFlags::HANDLES_COMMAND_LINE;
    if args.screenshot.is_some() || args.diagnostics {
        // Visual QA and the tray's diagnostics action must not activate an
        // existing panel; they are intentionally separate windows.
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let application = gtk::Application::builder()
        .application_id("org.mariusschober.ProjectEmber.Panel")
        .flags(flags)
        .build();

    let screenshot = args.screenshot.clone();
    let state = args.state.clone();
    let demo_mode = args.demo;
    let open_diagnostics = args.diagnostics;

    application.connect_command_line(|app, _command_line| {
        app.activate();
        0.into()
    });

    application.connect_startup(move |app| {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(theme::CSS);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
        let settings = gtk::Settings::default();
        if let Some(settings) = settings {
            settings.set_gtk_application_prefer_dark_theme(true);
        }
        let _ = app;
    });

    let startup_state = state.clone();
    application.connect_activate(move |app| {
        if let Some(existing) = app.active_window() {
            existing.present();
            return;
        }
        if open_diagnostics && !demo_mode {
            let client = ipc::IpcClient::start();
            diagnostics::open(app, None, Some(client), false);
            return;
        }
        let client = if demo_mode {
            None
        } else {
            Some(ipc::IpcClient::start())
        };
        let initial = if demo_mode {
            Some(demo::snapshot(parse_state(&startup_state)))
        } else {
            client.as_ref().and_then(|client| client.snapshot())
        };
        let panel = window::Panel::new(app, client, initial);
        let root = panel.window();
        if let Some(path) = &screenshot {
            window::schedule_screenshot(&root, path.clone(), app.clone());
        } else {
            root.present();
        }
        if open_diagnostics {
            panel.open_diagnostics();
        }
    });

    let exit = application.run();
    std::process::exit(exit.into());
}

fn parse_state(value: &str) -> demo::DemoState {
    match value {
        "off" => demo::DemoState::Off,
        "degraded" => demo::DemoState::Degraded,
        _ => demo::DemoState::Active,
    }
}
