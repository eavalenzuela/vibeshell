pub mod contracts;
pub mod model;

use std::sync::mpsc;
use std::sync::Once;
use std::thread;

use signal_hook::consts::signal::SIGHUP;
use signal_hook::iterator::Signals;
use tracing_subscriber::{fmt, EnvFilter};

static LOGGING_INIT: Once = Once::new();

pub fn init_logging(component: &str) {
    LOGGING_INIT.call_once(|| {
        let env_filter = EnvFilter::try_from_env("VIBESHELL_LOG")
            .or_else(|_| EnvFilter::try_from_default_env())
            .unwrap_or_else(|_| EnvFilter::new("info"));

        fmt()
            .with_env_filter(env_filter)
            .with_target(true)
            .compact()
            .init();

        tracing::debug!(component, "logging initialized");
    });
}

#[derive(Debug, Clone, Copy)]
pub enum ReloadReason {
    Signal,
    Command,
}

impl ReloadReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Signal => "SIGHUP",
            Self::Command => "vibeshellctl reload",
        }
    }
}

pub struct ReloadHandle {
    command_tx: mpsc::Sender<()>,
}

impl ReloadHandle {
    pub fn request_reload(&self) {
        if let Err(error) = self.command_tx.send(()) {
            tracing::warn!(?error, "failed to queue reload command");
        }
    }
}

pub fn spawn_reload_listener() -> (ReloadHandle, mpsc::Receiver<ReloadReason>) {
    let (reload_tx, reload_rx) = mpsc::channel::<ReloadReason>();
    let (command_tx, command_rx) = mpsc::channel::<()>();

    let signal_tx = reload_tx.clone();
    thread::spawn(move || {
        let mut signals = match Signals::new([SIGHUP]) {
            Ok(signals) => signals,
            Err(error) => {
                tracing::error!(?error, "failed to subscribe to SIGHUP for reload handling");
                return;
            }
        };

        for _ in signals.forever() {
            if signal_tx.send(ReloadReason::Signal).is_err() {
                return;
            }
        }
    });

    thread::spawn(move || {
        while command_rx.recv().is_ok() {
            if reload_tx.send(ReloadReason::Command).is_err() {
                break;
            }
        }
    });

    (ReloadHandle { command_tx }, reload_rx)
}
