use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use common::contracts::{daemon_socket_path, IpcRequest};
use sway::backend::{BackendEvent, DiffThresholds, FramePipeline};
use swayipc::{Connection, EventType};
use tracing::{info, warn};

use crate::state_store::with_state_owner;

enum DaemonEvent {
    SwayChanged,
    ReloadConfig,
}

pub fn run_daemon() -> Result<(), Box<dyn std::error::Error>> {
    info!("starting vibeshell daemon");

    let socket_path = daemon_socket_path();

    // Remove stale socket file if present.
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;
    info!(path = %socket_path.display(), "daemon listening on unix socket");

    // Do an initial sway ingest so the state is populated.
    if let Err(e) = with_state_owner(|owner| owner.ingest_sway_facts()) {
        warn!(?e, "initial sway ingest failed");
    }

    // --- SIGHUP reload listener (background thread) ---
    let (event_tx, event_rx) = mpsc::channel::<DaemonEvent>();
    let reload_event_tx = event_tx.clone();
    let (_reload_handle, reload_rx) = common::spawn_reload_listener();
    thread::spawn(move || {
        while let Ok(reason) = reload_rx.recv() {
            info!(reason = reason.as_str(), "daemon: config reload requested");
            if reload_event_tx.send(DaemonEvent::ReloadConfig).is_err() {
                break;
            }
        }
    });

    // --- Sway event subscriber (background thread) ---
    thread::spawn(move || {
        let connection = match Connection::new() {
            Ok(c) => c,
            Err(e) => {
                warn!(?e, "daemon: failed to connect sway event stream");
                return;
            }
        };
        let mut events = match connection.subscribe([EventType::Workspace, EventType::Window]) {
            Ok(e) => e,
            Err(e) => {
                warn!(?e, "daemon: failed to subscribe to sway events");
                return;
            }
        };
        for event in &mut events {
            if let Err(e) = event {
                warn!(?e, "daemon: sway event stream error");
                break;
            }
            if event_tx.send(DaemonEvent::SwayChanged).is_err() {
                break;
            }
        }
    });

    // --- Main loop: tick the frame pipeline + accept socket connections ---
    let mut pipeline = FramePipeline::new(
        Duration::from_millis(24),
        DiffThresholds {
            position_px: 1,
            size_px: 1,
        },
    );
    let mut sway_conn = Connection::new()?;
    let tick_interval = Duration::from_millis(8);

    loop {
        let tick_start = Instant::now();

        // Drain events.
        let mut had_sway_event = false;
        let mut had_reload = false;
        while let Ok(event) = event_rx.try_recv() {
            match event {
                DaemonEvent::SwayChanged => had_sway_event = true,
                DaemonEvent::ReloadConfig => had_reload = true,
            }
        }

        if had_reload {
            with_state_owner(|owner| owner.reload_config());
        }

        if had_sway_event {
            if let Err(e) = with_state_owner(|owner| owner.ingest_sway_facts()) {
                warn!(?e, "daemon: sway ingest failed after event");
            }

            // Queue a backend event for each cluster that might be affected.
            let cluster_ids = with_state_owner(|owner| {
                owner
                    .state()
                    .clusters
                    .iter()
                    .map(|c| c.id)
                    .collect::<Vec<_>>()
            });
            let now = Instant::now();
            for cluster_id in cluster_ids {
                pipeline.queue_event(BackendEvent::WorkspaceChanged { cluster_id }, now);
            }
        }

        // Try to build and apply a layout frame.
        let (inputs, current_geom, _context) = with_state_owner(|owner| {
            (
                owner.build_cluster_layout_inputs(),
                owner.current_window_geometry(),
                owner.layout_context(),
            )
        });

        if let Some(frame) = pipeline.try_build_frame(Instant::now(), &inputs, &current_geom) {
            if let Some(batch) = &frame.command_batch {
                info!(
                    ops = frame.applied_ops.len(),
                    "daemon: applying layout frame"
                );
                match sway_conn.run_command(batch) {
                    Ok(replies) => {
                        for reply in replies {
                            if let Err(e) = reply {
                                warn!(?e, "daemon: sway rejected layout command");
                            }
                        }
                    }
                    Err(e) => {
                        warn!(?e, "daemon: failed to run layout command batch");
                        // Try to reconnect.
                        if let Ok(new_conn) = Connection::new() {
                            sway_conn = new_conn;
                        }
                    }
                }
                with_state_owner(|owner| owner.update_applied_geometry(&frame.computed_targets));
            }
        }

        // Accept socket connections (non-blocking).
        match listener.accept() {
            Ok((stream, _addr)) => {
                // Handle one request per connection.
                if let Err(e) = handle_socket_connection(stream) {
                    warn!(?e, "daemon: socket connection error");
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No pending connections — this is normal.
            }
            Err(e) => {
                warn!(?e, "daemon: socket accept error");
            }
        }

        // Sleep to maintain tick rate.
        let elapsed = tick_start.elapsed();
        if elapsed < tick_interval {
            thread::sleep(tick_interval - elapsed);
        }
    }
}

fn handle_socket_connection(
    stream: std::os::unix::net::UnixStream,
) -> Result<(), Box<dyn std::error::Error>> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let request: IpcRequest = serde_json::from_str(line.trim())?;
    let response = crate::dispatch_ipc_request(request)?;

    let mut writer = stream.try_clone()?;
    let json = serde_json::to_string(&response)?;
    writeln!(writer, "{json}")?;

    Ok(())
}
