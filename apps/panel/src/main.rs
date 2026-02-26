use std::time::{SystemTime, UNIX_EPOCH};

use adw::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell as layer_shell;

const PANEL_HEIGHT: i32 = 32;

fn main() {
    common::init_logging();
    tracing::info!(app = "panel", "starting up");

    let app = adw::Application::builder()
        .application_id("com.vibeshell.panel")
        .build();

    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("vibeshell-panel")
        .default_height(PANEL_HEIGHT)
        .build();

    window.set_decorated(false);
    window.set_resizable(false);
    window.set_size_request(-1, PANEL_HEIGHT);

    layer_shell::init_for_window(&window);
    layer_shell::set_layer(&window, layer_shell::Layer::Top);
    layer_shell::set_anchor(&window, layer_shell::Edge::Top, true);
    layer_shell::set_anchor(&window, layer_shell::Edge::Left, true);
    layer_shell::set_anchor(&window, layer_shell::Edge::Right, true);
    layer_shell::set_exclusive_zone(&window, PANEL_HEIGHT);

    let workspaces = gtk::Label::new(Some("workspaces"));
    workspaces.set_halign(gtk::Align::Start);
    workspaces.set_margin_start(12);

    let title = gtk::Label::new(Some("title"));
    title.set_halign(gtk::Align::Center);

    let startup_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let clock = gtk::Label::new(Some(&format!("clock: {startup_seconds}")));
    clock.set_halign(gtk::Align::End);
    clock.set_margin_end(12);

    let content = gtk::CenterBox::builder()
        .orientation(gtk::Orientation::Horizontal)
        .hexpand(true)
        .build();
    content.set_start_widget(Some(&workspaces));
    content.set_center_widget(Some(&title));
    content.set_end_widget(Some(&clock));

    window.set_content(Some(&content));
    window.present();
}
