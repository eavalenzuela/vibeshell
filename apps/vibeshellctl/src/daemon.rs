use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use common::contracts::{daemon_socket_path, ClusterId, IpcRequest};
use tracing::{info, warn};
use wm::layout::{BackendEvent, DiffThresholds, FramePipeline};
use wm::{WmBackend, WmSignal};

use crate::state_store::with_state_owner;
use crate::wm_factory::connect_default;

enum DaemonEvent {
    WmChanged,
    ReloadConfig,
    /// vibewm finished remapping a cluster's windows. W1c-25-3 uses this
    /// to advance the in-flight `ZoomTransition` past the
    /// `Started → CompositorSettled` phase so overlay can finish its
    /// dive/undive animation (W1c-25-1).
    ClusterMapped {
        cluster: ClusterId,
    },
}

/// Safety timeout for stale `ZoomTransition`s. If `ClusterMapped` never
/// arrives (sway backend doesn't emit it; vibewm crashed mid-flip; etc.),
/// the transition clears after this many ms.
///
/// 800 ms — enough to outlast a 220 ms `DIVE_DURATION_MS` plus overlay's
/// W1c-25-7 push-channel observation latency (a few ms on a healthy
/// daemon socket). Used to be 1500 ms when overlay only had the 1200 ms
/// poll, but the W1c-25-7 daemon-side subscribe channel made that
/// headroom unnecessary.
const TRANSITION_STALENESS_TIMEOUT: Duration = Duration::from_millis(800);

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

    // Construct the active WM backend (reads `WM_BACKEND` env, defaults to sway).
    let mut backend: Box<dyn WmBackend> = connect_default()?;

    // Do an initial WM ingest so the state is populated.
    match backend.snapshot() {
        Ok(facts) => with_state_owner(|owner| owner.ingest_facts(facts)),
        Err(e) => warn!(?e, "initial WM ingest failed"),
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

    // --- WM event subscriber (background thread, owned by the backend) ---
    let wm_signal_rx = backend.spawn_event_stream()?;
    let wm_event_tx = event_tx;
    thread::spawn(move || {
        while let Ok(signal) = wm_signal_rx.recv() {
            // ClusterMapped does double duty: it's a re-ingest trigger
            // (windows mapped/unmapped, snapshot is stale) AND the signal
            // the `ZoomTransition` sequencer waits for. Send both events
            // so the main loop can advance the transition without blocking
            // on the WmChanged-driven snapshot.
            match signal {
                WmSignal::WorkspaceOrWindow => {
                    if wm_event_tx.send(DaemonEvent::WmChanged).is_err() {
                        return;
                    }
                }
                WmSignal::ClusterMapped { cluster, .. } => {
                    if wm_event_tx
                        .send(DaemonEvent::ClusterMapped { cluster })
                        .is_err()
                    {
                        return;
                    }
                    if wm_event_tx.send(DaemonEvent::WmChanged).is_err() {
                        return;
                    }
                }
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
    let tick_interval = Duration::from_millis(8);

    loop {
        let tick_start = Instant::now();

        // Drain events.
        let mut had_wm_event = false;
        let mut had_reload = false;
        let mut mapped_clusters: Vec<ClusterId> = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            match event {
                DaemonEvent::WmChanged => had_wm_event = true,
                DaemonEvent::ReloadConfig => had_reload = true,
                DaemonEvent::ClusterMapped { cluster } => mapped_clusters.push(cluster),
            }
        }

        if had_reload {
            with_state_owner(|owner| owner.reload_config());
        }

        // Advance any in-flight ZoomTransition. Doing this before the
        // WmChanged-driven snapshot means overlay sees CompositorSettled
        // on its next poll without waiting an extra tick for re-ingest.
        if !mapped_clusters.is_empty() {
            with_state_owner(|owner| {
                for cluster in &mapped_clusters {
                    owner.advance_transition_on_cluster_mapped(*cluster);
                }
            });
        }
        // Always run the staleness sweep — cheap, and catches transitions
        // the compositor never confirmed (sway backend, vibewm crash).
        with_state_owner(|owner| owner.clear_stale_transition(TRANSITION_STALENESS_TIMEOUT));

        if had_wm_event {
            match backend.snapshot() {
                Ok(facts) => with_state_owner(|owner| owner.ingest_facts(facts)),
                Err(e) => warn!(?e, "daemon: WM ingest failed after event"),
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
        let (inputs, current_geom, context) = with_state_owner(|owner| {
            (
                owner.build_cluster_layout_inputs(),
                owner.current_window_geometry(),
                owner.layout_context(),
            )
        });

        if let Some(frame) =
            pipeline.try_build_frame(Instant::now(), &inputs, &current_geom, context)
        {
            if !frame.applied_ops.is_empty() {
                info!(
                    ops = frame.applied_ops.len(),
                    "daemon: applying layout frame"
                );
                if let Err(e) = backend.apply_layout_ops(&frame.applied_ops) {
                    warn!(?e, "daemon: failed to apply layout ops");
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

    // W1c-25-7: `Subscribe` upgrades the connection. Reply once with
    // `Subscribed`, then hand the writer half to the state owner so
    // future mutations push `Event(StateChanged)` lines. Subscribers'
    // read timeout doesn't apply — the writer side is what matters and
    // we never read from this connection again.
    if matches!(request, IpcRequest::Subscribe) {
        let mut writer = stream.try_clone()?;
        let ack = serde_json::to_string(&common::contracts::IpcResponse::Subscribed)?;
        writeln!(writer, "{ack}")?;
        // Subscribers stay open indefinitely; clear the write timeout so
        // a slow client doesn't break the connection mid-push.
        let _ = writer.set_write_timeout(None);
        with_state_owner(|owner| owner.register_event_subscriber(writer));
        return Ok(());
    }

    let response = crate::dispatch_ipc_request(request)?;

    let mut writer = stream.try_clone()?;
    let json = serde_json::to_string(&response)?;
    writeln!(writer, "{json}")?;

    Ok(())
}
