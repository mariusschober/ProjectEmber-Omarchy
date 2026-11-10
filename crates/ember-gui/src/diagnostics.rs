use gtk4 as gtk;
use gtk4::prelude::*;

use crate::ipc::{self, IpcClient, IpcCommand};

pub fn open(
    app: &gtk::Application,
    parent: Option<&gtk::ApplicationWindow>,
    client: Option<IpcClient>,
    demo: bool,
) {
    let mut builder = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Project Ember Diagnostics")
        .default_width(560)
        .default_height(540);
    if let Some(parent) = parent {
        builder = builder.transient_for(parent);
    }
    let window = builder.build();
    window.add_css_class("ember-window");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.add_css_class("ember-panel");
    root.set_margin_top(14);
    root.set_margin_bottom(14);
    root.set_margin_start(14);
    root.set_margin_end(14);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some("Diagnostics"));
    title.add_css_class("ember-hero-title");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    let refresh = gtk::Button::with_label("Refresh");
    refresh.add_css_class("ember-action");
    header.append(&title);
    header.append(&refresh);
    root.append(&header);

    let backend = gtk::Label::new(None);
    backend.add_css_class("ember-row-detail");
    backend.set_xalign(0.0);
    backend.set_wrap(true);
    if let Some(snapshot) = client
        .as_ref()
        .and_then(|client| client.snapshot())
        .or_else(ipc::fetch_snapshot_sync)
    {
        backend.set_label(&format!(
            "{} · {} · {} · {}",
            snapshot.backend.name,
            snapshot.backend.verification.label(),
            snapshot.runtime_label,
            snapshot.footer_version()
        ));
    }
    root.append(&backend);

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_vexpand(true);
    scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    let text_view = gtk::TextView::new();
    text_view.set_editable(false);
    text_view.set_cursor_visible(false);
    text_view.set_wrap_mode(gtk::WrapMode::WordChar);
    text_view.add_css_class("ember-diagnostics");
    let buffer = text_view.buffer();
    let initial = if demo {
        "Project Ember diagnostics · 0.4.1 (build 6, Omarchy)\n\n\
         [INFO] Simulated backend active\n\
         [INFO] Demo data — no real display was changed\n\
         [INFO] Display identity tags are salted; no serial numbers are exported"
            .to_string()
    } else {
        ipc::fetch_diagnostics()
    };
    buffer.set_text(&initial);
    scroller.set_child(Some(&text_view));
    root.append(&scroller);

    let note = gtk::Label::new(Some(
        "Exports stay local. Display identifiers are salted hashes; serial numbers, screenshots and window titles are never included.",
    ));
    note.add_css_class("ember-row-caption");
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    root.append(&note);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let copy = gtk::Button::with_label("Copy");
    let export = gtk::Button::with_label("Export…");
    let retry = gtk::Button::with_label("Retry");
    let reset = gtk::Button::with_label("Reset");
    let restore = gtk::Button::with_label("Restore Display Now");
    for button in [&copy, &export, &retry, &reset, &restore] {
        button.add_css_class("ember-action");
    }
    restore.set_hexpand(true);
    actions.append(&copy);
    actions.append(&export);
    actions.append(&retry);
    actions.append(&reset);
    actions.append(&restore);
    root.append(&actions);

    let status = gtk::Label::new(None);
    status.add_css_class("ember-row-caption");
    status.set_xalign(0.0);
    root.append(&status);

    window.set_child(Some(&root));

    let buffer_for_copy = buffer.clone();
    let status_for_copy = status.clone();
    copy.connect_clicked(move |button| {
        if let Ok(display) = button.display().downcast::<gtk::gdk::Display>() {
            display.clipboard().set_text(&buffer_for_copy.text(
                &buffer_for_copy.start_iter(),
                &buffer_for_copy.end_iter(),
                false,
            ));
            status_for_copy.set_label("Copied to clipboard.");
        }
    });

    let buffer_for_export = buffer.clone();
    let status_for_export = status.clone();
    export.connect_clicked(move |_| {
        let path = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("project-ember-diagnostics.txt");
        let text = buffer_for_export.text(
            &buffer_for_export.start_iter(),
            &buffer_for_export.end_iter(),
            false,
        );
        match std::fs::write(&path, text) {
            Ok(()) => status_for_export.set_label(&format!("Exported to {}", path.display())),
            Err(error) => status_for_export.set_label(&format!("Export failed: {error}")),
        }
    });

    let client_for_retry = client.clone();
    retry.connect_clicked(move |_| {
        if let Some(client) = &client_for_retry {
            client.send(IpcCommand::Retry);
        }
    });
    let client_for_reset = client.clone();
    reset.connect_clicked(move |_| {
        if let Some(client) = &client_for_reset {
            client.send(IpcCommand::Reset);
        }
    });
    let client_for_restore = client.clone();
    restore.connect_clicked(move |_| {
        if let Some(client) = &client_for_restore {
            client.send(IpcCommand::RestoreNow);
        }
    });

    let buffer_for_refresh = buffer.clone();
    let backend_for_refresh = backend.clone();
    let client_for_refresh = client.clone();
    refresh.connect_clicked(move |_| {
        let text = ipc::fetch_diagnostics();
        buffer_for_refresh.set_text(&text);
        if let Some(snapshot) = client_for_refresh
            .as_ref()
            .and_then(|client| client.snapshot())
            .or_else(ipc::fetch_snapshot_sync)
        {
            backend_for_refresh.set_label(&format!(
                "{} · {} · {} · {}",
                snapshot.backend.name,
                snapshot.backend.verification.label(),
                snapshot.runtime_label,
                snapshot.footer_version()
            ));
        }
    });

    window.present();
}
