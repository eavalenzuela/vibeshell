//! `WmBackend` impl that talks to a running vibewm compositor over its
//! control socket.
//!
//! Each WmBackend method opens a fresh unix-socket connection, sends one
//! `VibewmRequest` line, and reads one `VibewmResponse` line back. (Same
//! shape as `vibeshellctl`'s daemon-IPC client today.) The exception is
//! `spawn_event_stream`, which keeps a single connection open and pushes
//! events as they arrive.
//!
//! Per-call connections trade a small per-op cost for crash isolation: if
//! vibewm dies, we get an `Unavailable` error rather than a stuck Mutex
//! holding a dead Connection. Mirrors what `swayipc::Connection::new()` does.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use common::contracts::{ClusterId, WindowId};

use crate::backend::{BackendError, WmBackend, WmSignal};
use crate::facts::WmFacts;
use crate::layout::LayoutOp;
use crate::vibewm_ipc::{vibewm_socket_path, VibewmRequest, VibewmResponse};

const READ_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct WlrootsBackend {
    socket_path: PathBuf,
}

impl WlrootsBackend {
    pub fn connect() -> Result<Self, BackendError> {
        let socket_path = vibewm_socket_path();
        // Liveness probe — fail fast if vibewm isn't running so the daemon
        // surfaces a clear error at startup rather than on first dispatch.
        let mut backend = Self { socket_path };
        if !backend.is_alive() {
            return Err(BackendError::Unavailable(format!(
                "vibewm control socket not reachable at {}",
                backend.socket_path.display()
            )));
        }
        Ok(backend)
    }

    fn open(&self) -> Result<UnixStream, BackendError> {
        let stream = UnixStream::connect(&self.socket_path).map_err(|e| {
            BackendError::Unavailable(format!(
                "vibewm socket connect at {}: {e}",
                self.socket_path.display()
            ))
        })?;
        stream
            .set_read_timeout(Some(READ_TIMEOUT))
            .map_err(|e| BackendError::Other(format!("set_read_timeout: {e}")))?;
        stream
            .set_write_timeout(Some(WRITE_TIMEOUT))
            .map_err(|e| BackendError::Other(format!("set_write_timeout: {e}")))?;
        Ok(stream)
    }

    fn dispatch(&self, request: VibewmRequest) -> Result<VibewmResponse, BackendError> {
        let stream = self.open()?;
        let mut writer = stream
            .try_clone()
            .map_err(|e| BackendError::Other(format!("stream clone: {e}")))?;
        let request_line = serde_json::to_string(&request)
            .map_err(|e| BackendError::Other(format!("serialize: {e}")))?;
        writeln!(writer, "{request_line}")
            .map_err(|e| BackendError::Other(format!("write request: {e}")))?;
        writer
            .flush()
            .map_err(|e| BackendError::Other(format!("flush: {e}")))?;

        let mut reader = BufReader::new(stream);
        let mut response_line = String::new();
        reader
            .read_line(&mut response_line)
            .map_err(|e| BackendError::Other(format!("read response: {e}")))?;
        if response_line.is_empty() {
            return Err(BackendError::Other("empty response from vibewm".into()));
        }
        let response: VibewmResponse = serde_json::from_str(response_line.trim()).map_err(|e| {
            BackendError::Other(format!("parse response `{}`: {e}", response_line.trim()))
        })?;
        Ok(response)
    }

    fn dispatch_ack(&self, request: VibewmRequest) -> Result<(), BackendError> {
        match self.dispatch(request)? {
            VibewmResponse::Ack => Ok(()),
            VibewmResponse::Error { message } => Err(BackendError::Other(message)),
            other => Err(BackendError::Other(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }
}

impl WmBackend for WlrootsBackend {
    fn snapshot(&mut self) -> Result<WmFacts, BackendError> {
        match self.dispatch(VibewmRequest::Snapshot)? {
            VibewmResponse::Snapshot(facts) => Ok(facts),
            VibewmResponse::Error { message } => Err(BackendError::Other(message)),
            other => Err(BackendError::Other(format!("Snapshot returned: {other:?}"))),
        }
    }

    fn apply_layout_ops(&mut self, ops: &[LayoutOp]) -> Result<(), BackendError> {
        if ops.is_empty() {
            return Ok(());
        }
        self.dispatch_ack(VibewmRequest::ApplyLayoutOps { ops: ops.to_vec() })
    }

    fn focus_window(&mut self, window: WindowId) -> Result<(), BackendError> {
        self.dispatch_ack(VibewmRequest::FocusWindow { window })
    }

    fn activate_cluster(&mut self, cluster: ClusterId) -> Result<(), BackendError> {
        self.dispatch_ack(VibewmRequest::ActivateCluster { cluster })
    }

    fn create_named_workspace(&mut self, name: &str) -> Result<(), BackendError> {
        self.dispatch_ack(VibewmRequest::CreateNamedWorkspace {
            name: name.to_owned(),
        })
    }

    fn back_and_forth_workspace(&mut self) -> Result<(), BackendError> {
        self.dispatch_ack(VibewmRequest::BackAndForthWorkspace)
    }

    fn exit_session(&mut self) -> Result<(), BackendError> {
        self.dispatch_ack(VibewmRequest::ExitSession)
    }

    fn reload_wm_config(&mut self) -> Result<(), BackendError> {
        self.dispatch_ack(VibewmRequest::ReloadWmConfig)
    }

    fn focused_window(&mut self) -> Result<Option<WindowId>, BackendError> {
        match self.dispatch(VibewmRequest::FocusedWindow)? {
            VibewmResponse::FocusedWindow { window } => Ok(window),
            VibewmResponse::Error { message } => Err(BackendError::Other(message)),
            other => Err(BackendError::Other(format!(
                "FocusedWindow returned: {other:?}"
            ))),
        }
    }

    fn is_alive(&mut self) -> bool {
        // Cheap liveness check: open the socket and round-trip a Ping. Any
        // failure (connect, write, read, deserialize) means vibewm is gone.
        UnixStream::connect(&self.socket_path).is_ok()
            && matches!(self.dispatch(VibewmRequest::Ping), Ok(VibewmResponse::Pong))
    }

    fn capture_cluster_thumbnail(
        &mut self,
        cluster: ClusterId,
        max_width: u32,
        max_height: u32,
    ) -> Result<Option<common::contracts::ClusterThumbnail>, BackendError> {
        match self.dispatch(VibewmRequest::CaptureClusterThumbnail {
            cluster,
            max_width,
            max_height,
        })? {
            VibewmResponse::Thumbnail(thumb) => Ok(Some(thumb)),
            VibewmResponse::ThumbnailMissing => Ok(None),
            VibewmResponse::Error { message } => Err(BackendError::Other(message)),
            other => Err(BackendError::Other(format!(
                "CaptureClusterThumbnail returned: {other:?}"
            ))),
        }
    }

    fn spawn_event_stream(&self) -> Result<Receiver<WmSignal>, BackendError> {
        let socket_path = self.socket_path.clone();
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let stream = match UnixStream::connect(&socket_path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(?e, path = %socket_path.display(), "vibewm event stream: connect failed");
                    return;
                }
            };
            // Subscribe-mode connections are long-lived; no read timeout.
            if let Err(e) = stream.set_read_timeout(None) {
                tracing::warn!(?e, "vibewm event stream: set_read_timeout(None) failed");
                return;
            }
            let mut writer = match stream.try_clone() {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!(?e, "vibewm event stream: clone failed");
                    return;
                }
            };
            let subscribe = match serde_json::to_string(&VibewmRequest::Subscribe) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(?e, "vibewm event stream: serialize Subscribe failed");
                    return;
                }
            };
            if writeln!(writer, "{subscribe}").is_err() || writer.flush().is_err() {
                tracing::warn!("vibewm event stream: failed to send Subscribe");
                return;
            }

            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            // Initial response should be `Subscribed`.
            if reader.read_line(&mut line).is_err() {
                tracing::warn!("vibewm event stream: failed to read Subscribed reply");
                return;
            }
            match serde_json::from_str::<VibewmResponse>(line.trim()) {
                Ok(VibewmResponse::Subscribed) => {}
                Ok(other) => {
                    tracing::warn!(?other, "vibewm event stream: unexpected initial reply");
                    return;
                }
                Err(e) => {
                    tracing::warn!(?e, "vibewm event stream: parse Subscribed failed");
                    return;
                }
            }

            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break, // EOF
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(?e, "vibewm event stream: read error");
                        break;
                    }
                }
                match serde_json::from_str::<VibewmResponse>(line.trim()) {
                    Ok(VibewmResponse::Event(event)) => {
                        if let Some(signal) = event.to_signal() {
                            if tx.send(signal).is_err() {
                                break;
                            }
                        }
                    }
                    Ok(other) => {
                        tracing::debug!(?other, "vibewm event stream: ignoring non-event message");
                    }
                    Err(e) => {
                        tracing::warn!(?e, "vibewm event stream: parse error");
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }
}
