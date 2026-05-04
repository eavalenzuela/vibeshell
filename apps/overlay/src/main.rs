use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use adw::gtk::glib;
use adw::prelude::*;
use common::contracts::{CanvasState, ClusterId, IpcRequest, IpcResponse};
use gtk4_layer_shell::{self as layer_shell, LayerShell};

mod interaction;
mod interaction_state;
mod ui;

const REFRESH_INTERVAL: Duration = Duration::from_millis(1200);
const EVENT_DEBOUNCE: Duration = Duration::from_millis(180);

fn main() {
    common::init_logging("overlay");

    let app = adw::Application::builder()
        .application_id("com.vibeshell.overlay")
        .build();

    app.connect_activate(|app| {
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk_theme::install_theme(&display);
        }
        build_ui(app);
    });
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

    let activate_cluster: Rc<dyn Fn(ClusterId)> = Rc::new(|cluster_id: ClusterId| {
        let status = Command::new("vibeshellctl")
            .args(["ipc", "activate-cluster", &cluster_id.to_string()])
            .status();
        if let Err(error) = status {
            tracing::warn!(?error, cluster_id, "failed to activate cluster via IPC");
        }
    });

    let zoom_back: Rc<dyn Fn()> = Rc::new(|| {
        let status = Command::new("vibeshellctl")
            .args(["ipc", "zoom-out-mode"])
            .status();
        if let Err(error) = status {
            tracing::warn!(?error, "failed to zoom out via IPC");
        }
    });

    let mutation: Rc<dyn Fn(interaction::IpcMutation)> = Rc::new(|mutation| match &mutation {
        interaction::IpcMutation::UpdateClusterDrag { .. }
        | interaction::IpcMutation::KeyboardMoveBy { .. } => {
            interaction::dispatch_ipc_mutation_detached(mutation);
        }
        _ => {
            interaction::dispatch_ipc_mutation(mutation);
        }
    });

    let overview_canvas = ui::OverviewCanvas::new(
        Rc::clone(&activate_cluster),
        Rc::clone(&activate_cluster),
        Rc::clone(&zoom_back),
        Rc::clone(&mutation),
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

    // Sway event stream — usable under WM_BACKEND=sway. Silently no-ops if
    // sway isn't running (e.g. WM_BACKEND=wlroots), in which case the
    // vibewm-control subscribe thread below is the snappy-refresh source.
    {
        let tx = tx.clone();
        thread::spawn(move || {
            let events = sway::spawn_event_stream();
            let normalized = sway::spawn_normalized_stream(events, EVENT_DEBOUNCE);
            while normalized.recv().is_ok() {
                if tx.send(()).is_err() {
                    break;
                }
            }
        });
    }

    // Vibewm-control event subscribe — usable under WM_BACKEND=wlroots. Reuses
    // wm::WlrootsBackend::spawn_event_stream which already returns a
    // Receiver<WmSignal>. Silently no-ops if vibewm isn't running.
    {
        let tx = tx.clone();
        thread::spawn(move || {
            use wm::WmBackend;
            let backend = match wm::WlrootsBackend::connect() {
                Ok(b) => b,
                Err(_) => return,
            };
            let stream = match backend.spawn_event_stream() {
                Ok(s) => s,
                Err(_) => return,
            };
            while stream.recv().is_ok() {
                if tx.send(()).is_err() {
                    break;
                }
            }
        });
    }

    // W1c-25-7: daemon-side state-change subscribe. The daemon pushes
    // `IpcResponse::Event(StateChanged)` for every successful mutation
    // (zoom-in/out-mode, cluster activation, transitions, etc.) — many
    // of which don't ride on a WM event because they're pure state-store
    // mutations. Without this, Cluster→Overview transitions in
    // wlroots-mode would only be observed on the 1200ms baseline poll,
    // missing the W1c-25-1 undive animation trigger window.
    thread::spawn(move || {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;
        let socket_path = common::contracts::daemon_socket_path();
        let stream = match UnixStream::connect(&socket_path) {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut writer = match stream.try_clone() {
            Ok(w) => w,
            Err(_) => return,
        };
        let request = match serde_json::to_string(&IpcRequest::Subscribe) {
            Ok(s) => s,
            Err(_) => return,
        };
        if writeln!(writer, "{request}").is_err() || writer.flush().is_err() {
            return;
        }
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => break,
            }
            // Initial Subscribed reply OR subsequent Event(_) lines —
            // either way it means daemon state changed (or just came up).
            if tx.send(()).is_err() {
                break;
            }
        }
    });

    {
        let last_state = Rc::clone(&last_state);
        let overview_canvas = overview_canvas.clone();
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
    // Try daemon socket first.
    if let Some(response) = interaction::try_dispatch_via_socket(&IpcRequest::GetState) {
        return match response {
            IpcResponse::State(state) => Some(state),
            _ => None,
        };
    }

    // Fall back to subprocess.
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
        | IpcResponse::Error { .. }
        | IpcResponse::Subscribed
        | IpcResponse::Event(_)
        | IpcResponse::Thumbnail(_)
        | IpcResponse::ThumbnailMissing => None,
    }
}
