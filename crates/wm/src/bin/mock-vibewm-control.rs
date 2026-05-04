//! Mock vibewm-control listener for headless smoke tests.
//!
//! Phase 8 W1c-23. Implements just enough of the `VibewmRequest`/`VibewmResponse`
//! protocol (see `crates/wm/src/vibewm_ipc.rs`) to satisfy the daemon's
//! `WlrootsBackend`. Mirrors the sway-headless smoke test, but for the
//! wlroots backend path — no GPU, no real wayland, no smithay.
//!
//! What it does:
//! - Listens on `$VIBEWM_SOCKET` (or `$XDG_RUNTIME_DIR/vibewm-control.sock`).
//! - Maintains a tiny in-memory model: clusters keyed by name, focused window.
//! - Replies to every `VibewmRequest` variant. Mutations apply to the model;
//!   queries serve from it.
//! - On `Subscribe`, holds the connection open and pushes a
//!   `VibewmEvent::WorkspaceOrWindow` after every mutating request — same
//!   shape as the real vibewm.
//!
//! What it deliberately doesn't do:
//! - No actual layout. `ApplyLayoutOps` is acknowledged but the geometry is
//!   not propagated back into snapshots (the daemon's layout-engine is the
//!   thing under test, not vibewm's renderer).
//! - No frame timing, no surface tree, no input.
//!
//! Run via `cargo run -p wm --bin mock-vibewm-control` or directly from the
//! smoke script. Exits cleanly on `VibewmRequest::ExitSession`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::thread;

use common::contracts::{Cluster, ClusterId, OutputState, WindowId};
use wm::facts::WmFacts;
use wm::vibewm_ipc::{vibewm_socket_path, VibewmEvent, VibewmRequest, VibewmResponse};

#[derive(Default)]
struct Model {
    /// Clusters keyed by id. Insertion order is preserved separately so the
    /// snapshot matches a sane "most-recent-last" order.
    clusters: BTreeMap<ClusterId, Cluster>,
    /// Stable cluster ordering (insertion order).
    cluster_order: Vec<ClusterId>,
    /// Most recently active cluster ids (front = current).
    active_history: Vec<ClusterId>,
    /// Currently focused window, if any.
    focus: Option<WindowId>,
    /// Subscriber sockets to push events to. Each holds the writer half of
    /// the upgraded stream.
    subscribers: Vec<UnixStream>,
    /// Increment for each cluster create so ids are unique.
    next_cluster_id: ClusterId,
}

impl Model {
    fn new() -> Self {
        // Boot with a single default cluster named "1" — same convention as
        // sway and vibewm itself.
        let mut m = Model {
            next_cluster_id: 2,
            ..Default::default()
        };
        let default = Cluster {
            id: 1,
            name: "1".to_owned(),
            ..Default::default()
        };
        m.clusters.insert(1, default);
        m.cluster_order.push(1);
        m.active_history.push(1);
        m
    }

    fn snapshot(&self) -> WmFacts {
        let clusters: Vec<Cluster> = self
            .cluster_order
            .iter()
            .filter_map(|id| self.clusters.get(id).cloned())
            .map(|mut c| {
                // Mark the current cluster visible; everything else hidden.
                c.enabled = self.active_history.first() == Some(&c.id);
                c
            })
            .collect();
        WmFacts {
            clusters,
            windows: Vec::new(),
            window_geometry: BTreeMap::new(),
            output: OutputState::default(),
            outputs: vec!["mock-output-0".to_owned()],
            primary_output: Some("mock-output-0".to_owned()),
        }
    }

    fn create_named(&mut self, name: &str) -> ClusterId {
        // If the name already exists, just activate it (sway-compat).
        if let Some(c) = self.clusters.values().find(|c| c.name == name) {
            let id = c.id;
            self.activate(id);
            return id;
        }
        let id = self.next_cluster_id;
        self.next_cluster_id += 1;
        let cluster = Cluster {
            id,
            name: name.to_owned(),
            ..Default::default()
        };
        self.clusters.insert(id, cluster);
        self.cluster_order.push(id);
        self.activate(id);
        id
    }

    fn activate(&mut self, id: ClusterId) {
        if !self.clusters.contains_key(&id) {
            return;
        }
        self.active_history.retain(|&x| x != id);
        self.active_history.insert(0, id);
    }

    fn back_and_forth(&mut self) {
        if self.active_history.len() >= 2 {
            self.active_history.swap(0, 1);
        }
    }
}

fn main() {
    let socket_path = vibewm_socket_path();
    let _ = std::fs::remove_file(&socket_path);
    let listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "mock-vibewm-control: bind {} failed: {e}",
                socket_path.display()
            );
            std::process::exit(1);
        }
    };
    eprintln!(
        "mock-vibewm-control: listening on {}",
        socket_path.display()
    );

    let model: Arc<Mutex<Model>> = Arc::new(Mutex::new(Model::new()));

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("mock-vibewm-control: accept failed: {e}");
                continue;
            }
        };
        let model = model.clone();
        thread::spawn(move || handle_client(stream, model));
    }
}

fn handle_client(stream: UnixStream, model: Arc<Mutex<Model>>) {
    let writer = match stream.try_clone() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("mock-vibewm-control: try_clone failed: {e}");
            return;
        }
    };
    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    if reader.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }
    let request: VibewmRequest = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => {
            send(
                &writer,
                &VibewmResponse::Error {
                    message: format!("parse: {e}"),
                },
            );
            return;
        }
    };

    // Subscribe takes over the connection; everything else is one-shot.
    if matches!(request, VibewmRequest::Subscribe) {
        send(&writer, &VibewmResponse::Subscribed);
        if let Ok(mut m) = model.lock() {
            m.subscribers.push(writer);
        }
        return; // The reader half closes when the client drops; we don't
                // need to drive it further.
    }

    let (response, broadcast) = handle_request(&model, request);
    send(&writer, &response);
    if broadcast {
        broadcast_event(&model, VibewmEvent::WorkspaceOrWindow);
        // Cluster-activating mutations also fire ClusterMapped to mirror
        // the real vibewm's W1c-25-1 sequencing.
        let active = model.lock().ok().and_then(|m| {
            m.active_history.first().copied().map(|id| {
                let count = m
                    .clusters
                    .get(&id)
                    .map(|c| c.windows.len() as u32)
                    .unwrap_or(0);
                (id, count)
            })
        });
        if let Some((cluster, window_count)) = active {
            broadcast_event(
                &model,
                VibewmEvent::ClusterMapped {
                    cluster,
                    window_count,
                },
            );
        }
    }
}

fn handle_request(model: &Arc<Mutex<Model>>, request: VibewmRequest) -> (VibewmResponse, bool) {
    let mut model = match model.lock() {
        Ok(g) => g,
        Err(_) => {
            return (
                VibewmResponse::Error {
                    message: "model lock poisoned".into(),
                },
                false,
            )
        }
    };
    match request {
        VibewmRequest::Ping => (VibewmResponse::Pong, false),
        VibewmRequest::Snapshot => (VibewmResponse::Snapshot(model.snapshot()), false),
        VibewmRequest::ApplyLayoutOps { ops: _ } => (VibewmResponse::Ack, false),
        VibewmRequest::FocusWindow { window } => {
            model.focus = Some(window);
            (VibewmResponse::Ack, true)
        }
        VibewmRequest::ActivateCluster { cluster } => {
            if !model.clusters.contains_key(&cluster) {
                return (
                    VibewmResponse::Error {
                        message: format!("unknown cluster {cluster}"),
                    },
                    false,
                );
            }
            model.activate(cluster);
            (VibewmResponse::Ack, true)
        }
        VibewmRequest::CreateNamedWorkspace { name } => {
            model.create_named(&name);
            (VibewmResponse::Ack, true)
        }
        VibewmRequest::BackAndForthWorkspace => {
            model.back_and_forth();
            (VibewmResponse::Ack, true)
        }
        VibewmRequest::ExitSession => {
            // Send Ack then exit the whole process so the smoke script can
            // detect a clean shutdown.
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(50));
                std::process::exit(0);
            });
            (VibewmResponse::Ack, false)
        }
        VibewmRequest::ReloadWmConfig => (VibewmResponse::Ack, false),
        VibewmRequest::FocusedWindow => (
            VibewmResponse::FocusedWindow {
                window: model.focus,
            },
            false,
        ),
        // Mock returns a procedural placeholder thumbnail (HSV-shifted
        // by cluster id) so the daemon ↔ overlay wire stays exercisable
        // in CI without a real renderer. The W1c-25-5 production path
        // captures from vibewm's GlesRenderer.
        VibewmRequest::CaptureClusterThumbnail {
            cluster,
            max_width,
            max_height,
        } => {
            let w = max_width.clamp(8, 96);
            let h = max_height.clamp(8, 54);
            let hue = ((cluster.wrapping_mul(67) % 360) as u8) as f32;
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for _ in 0..(w * h) {
                // Cheap HSV→RGB at S=0.5 V=0.7. Just enough variety so
                // the test pattern visibly differs per cluster id.
                let h6 = hue / 60.0;
                let c = 0.7_f32 * 0.5;
                let x = c * (1.0 - (h6 % 2.0 - 1.0).abs());
                let m = 0.7_f32 - c;
                let (r, g, b) = match h6 as u8 {
                    0 => (c, x, 0.0),
                    1 => (x, c, 0.0),
                    2 => (0.0, c, x),
                    3 => (0.0, x, c),
                    4 => (x, 0.0, c),
                    _ => (c, 0.0, x),
                };
                rgba.push(((r + m) * 255.0) as u8);
                rgba.push(((g + m) * 255.0) as u8);
                rgba.push(((b + m) * 255.0) as u8);
                rgba.push(255);
            }
            let thumb = common::contracts::ClusterThumbnail {
                width: w,
                height: h,
                rgba_base64: base64_encode(&rgba),
            };
            (VibewmResponse::Thumbnail(thumb), false)
        }
        // Subscribe is intercepted upstream.
        VibewmRequest::Subscribe => unreachable!("Subscribe handled in handle_client"),
    }
}

/// Tiny base64 encoder so the mock binary doesn't pull in a `base64`
/// crate dep. Standard alphabet, no line wrap, '=' padding.
fn base64_encode(input: &[u8]) -> String {
    const ALPH: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPH[(b0 >> 2) as usize] as char);
        out.push(ALPH[((b0 << 4 | b1 >> 4) & 0x3F) as usize] as char);
        if chunk.len() >= 2 {
            out.push(ALPH[((b1 << 2 | b2 >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() >= 3 {
            out.push(ALPH[(b2 & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn send(mut writer: &UnixStream, response: &VibewmResponse) {
    let line = match serde_json::to_string(response) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mock-vibewm-control: serialize response: {e}");
            return;
        }
    };
    let _ = writeln!(writer, "{line}");
    let _ = writer.flush();
}

fn broadcast_event(model: &Arc<Mutex<Model>>, event: VibewmEvent) {
    let line = match serde_json::to_string(&VibewmResponse::Event(event)) {
        Ok(s) => s + "\n",
        Err(_) => return,
    };
    let mut m = match model.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let mut alive: Vec<UnixStream> = Vec::new();
    for mut sub in m.subscribers.drain(..) {
        if sub.write_all(line.as_bytes()).is_ok() && sub.flush().is_ok() {
            alive.push(sub);
        }
    }
    m.subscribers = alive;
}
