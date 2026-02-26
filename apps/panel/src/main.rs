use std::cell::RefCell;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use adw::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell::{self as layer_shell, LayerShell};
use sway::{PanelState, PanelUpdate, WorkspaceState};

const PANEL_HEIGHT: i32 = 32;
const RENDER_DEBOUNCE: Duration = Duration::from_millis(50);
const SWAY_CONNECT_INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const SWAY_CONNECT_MAX_BACKOFF: Duration = Duration::from_secs(10);

fn main() {
    common::init_logging("panel");
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

    window.init_layer_shell();
    window.set_layer(layer_shell::Layer::Top);
    window.set_anchor(layer_shell::Edge::Top, true);
    window.set_anchor(layer_shell::Edge::Left, true);
    window.set_anchor(layer_shell::Edge::Right, true);
    window.set_exclusive_zone(PANEL_HEIGHT);

    let workspaces = gtk::Label::new(Some(""));
    workspaces.set_halign(gtk::Align::Start);
    workspaces.set_margin_start(12);

    let title = gtk::Label::new(Some(""));
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

    let (sender, receiver) = glib::MainContext::channel::<PanelUpdate>(glib::Priority::DEFAULT);

    thread::spawn(move || {
        let mut backoff = SWAY_CONNECT_INITIAL_BACKOFF;

        loop {
            match sway::SwayClient::connect() {
                Ok(client) => {
                    tracing::info!("connected to sway ipc");
                    if let Err(error) =
                        client.run_listener(sender.clone(), Duration::from_millis(80))
                    {
                        tracing::warn!(?error, "sway listener exited; retrying connection");
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        retry_ms = backoff.as_millis(),
                        "unable to connect to sway ipc; ensure sway is running and SWAYSOCK is set"
                    );
                    eprintln!(
                        "panel: sway IPC unavailable. Start sway first (or export SWAYSOCK), retrying in {} ms.",
                        backoff.as_millis()
                    );
                }
            }

            thread::sleep(backoff);
            backoff = (backoff * 2).min(SWAY_CONNECT_MAX_BACKOFF);
        }
    });

    let latest_state = Rc::new(RefCell::new(None::<PanelState>));
    let scheduled_render = Rc::new(RefCell::new(None::<glib::SourceId>));
    let last_rendered = Rc::new(RefCell::new(None::<PanelState>));

    receiver.attach(None, move |update| {
        let PanelUpdate::Snapshot(state) = update;
        latest_state.replace(Some(state));

        if scheduled_render.borrow().is_some() {
            return glib::ControlFlow::Continue;
        }

        let latest_state = Rc::clone(&latest_state);
        let scheduled_render = Rc::clone(&scheduled_render);
        let last_rendered = Rc::clone(&last_rendered);
        let workspaces = workspaces.clone();
        let title = title.clone();

        let source_id = glib::timeout_add_local_once(RENDER_DEBOUNCE, move || {
            scheduled_render.borrow_mut().take();

            let Some(next_state) = latest_state.borrow_mut().take() else {
                return;
            };

            if last_rendered
                .borrow()
                .as_ref()
                .is_some_and(|rendered| rendered == &next_state)
            {
                return;
            }

            apply_state(&workspaces, &title, &next_state);
            last_rendered.replace(Some(next_state));
        });

        scheduled_render.replace(Some(source_id));
        glib::ControlFlow::Continue
    });
}

fn apply_state(workspaces_label: &gtk::Label, title_label: &gtk::Label, state: &PanelState) {
    workspaces_label.set_text(&format_workspaces(&state.workspaces));
    title_label.set_text(state.focused_title.as_deref().unwrap_or(""));
}

fn format_workspaces(workspaces: &[WorkspaceState]) -> String {
    if workspaces.is_empty() {
        return "no workspaces".to_owned();
    }

    workspaces
        .iter()
        .map(|workspace| {
            let display_name = workspace
                .num
                .filter(|num| *num > 0)
                .map(|num| num.to_string())
                .unwrap_or_else(|| workspace.name.clone());

            let status = if workspace.focused {
                '●'
            } else if workspace.visible {
                '◉'
            } else {
                '○'
            };

            let urgent = if workspace.urgent { "!" } else { "" };
            format!("{status}{display_name}{urgent}")
        })
        .collect::<Vec<_>>()
        .join("  ")
}
