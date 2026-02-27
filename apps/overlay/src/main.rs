use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use adw::prelude::*;
use common::contracts::{CanvasState, ClusterId, IpcResponse};
use gtk::glib;
use gtk4_layer_shell::{self as layer_shell, LayerShell};

mod interaction_state;
mod ui;

const REFRESH_INTERVAL: Duration = Duration::from_millis(1200);
const EVENT_DEBOUNCE: Duration = Duration::from_millis(180);

fn main() {
    common::init_logging("overlay");

    let app = adw::Application::builder()
        .application_id("com.vibeshell.overlay")
        .build();

    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("vibeshell-overlay")
        .default_width(560)
        .default_height(720)
        .build();

    if layer_shell::is_supported() {
        window.set_decorated(false);
        window.init_layer_shell();
        window.set_layer(layer_shell::Layer::Overlay);
        window.set_keyboard_mode(layer_shell::KeyboardMode::Exclusive);
        window.set_anchor(layer_shell::Edge::Top, true);
        window.set_anchor(layer_shell::Edge::Bottom, true);
        window.set_anchor(layer_shell::Edge::Left, true);
        window.set_anchor(layer_shell::Edge::Right, true);
    }

    let activate_cluster = Rc::new(|cluster_id: ClusterId| {
        let status = Command::new("vibeshellctl")
            .args(["ipc", "activate-cluster", &cluster_id.to_string()])
            .status();
        if let Err(error) = status {
            tracing::warn!(?error, cluster_id, "failed to activate cluster via IPC");
        }
    });

    let zoom_back = Rc::new(|| {
        let status = Command::new("vibeshellctl")
            .args(["ipc", "zoom-out-mode"])
            .status();
        if let Err(error) = status {
            tracing::warn!(?error, "failed to zoom out via IPC");
        }
    });

    let overview_canvas = ui::OverviewCanvas::new(
        Rc::clone(&activate_cluster),
        Rc::clone(&activate_cluster),
        Rc::clone(&zoom_back),
    );

    window.set_content(Some(overview_canvas.widget()));
    window.present();

    let last_state = Rc::new(std::cell::RefCell::new(None::<CanvasState>));
    let (tx, rx) = mpsc::channel::<()>();

    thread::spawn({
        let tx = tx.clone();
        move || loop {
            thread::sleep(REFRESH_INTERVAL);
            if tx.send(()).is_err() {
                break;
            }
        }
    });

    thread::spawn(move || {
        let events = sway::spawn_event_stream();
        let normalized = sway::spawn_normalized_stream(events, EVENT_DEBOUNCE);
        while normalized.recv().is_ok() {
            if tx.send(()).is_err() {
                break;
            }
        }
    });

    {
        let last_state = Rc::clone(&last_state);
        let overview_canvas = overview_canvas;
        glib::timeout_add_local(Duration::from_millis(120), move || {
            let mut should_refresh = false;
            while rx.try_recv().is_ok() {
                should_refresh = true;
            }

            if should_refresh {
                if let Some(state) = fetch_state_via_ipc() {
                    if last_state.borrow().as_ref() != Some(&state) {
                        overview_canvas.set_canvas_state(state.clone());
                        last_state.replace(Some(state));
                    }
                }
            }

            glib::ControlFlow::Continue
        });
    }

    if let Some(initial) = fetch_state_via_ipc() {
        overview_canvas.set_canvas_state(initial.clone());
        last_state.replace(Some(initial));
    }
}

fn fetch_state_via_ipc() -> Option<CanvasState> {
    let output = Command::new("vibeshellctl")
        .args(["ipc", "get-state"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let response: IpcResponse = serde_json::from_slice(&output.stdout).ok()?;
    match response {
        IpcResponse::State(state) => Some(state),
        IpcResponse::Ack
        | IpcResponse::ClusterDragAck { .. }
        | IpcResponse::Error { .. }
        | IpcResponse::ClusterDragError { .. } => None,
    }
}
