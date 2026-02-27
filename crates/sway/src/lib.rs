pub mod backend;

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use swayipc::{Connection, EventType, Fallible, Node};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceState {
    pub id: i64,
    pub num: Option<i32>,
    pub name: String,
    pub output: String,
    pub focused: bool,
    pub visible: bool,
    pub urgent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelState {
    pub workspaces: Vec<WorkspaceState>,
    pub focused_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelUpdate {
    Snapshot(PanelState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwaySignal {
    WorkspaceOrWindow,
}

pub struct SwayClient {
    connection: Connection,
}

impl SwayClient {
    pub fn connect() -> Fallible<Self> {
        Ok(Self {
            connection: Connection::new()?,
        })
    }

    pub fn get_workspaces(&mut self) -> Fallible<Vec<WorkspaceState>> {
        let mut workspaces: Vec<_> = self
            .connection
            .get_workspaces()?
            .into_iter()
            .map(|workspace| WorkspaceState {
                id: workspace.id,
                num: (workspace.num >= 0).then_some(workspace.num),
                name: workspace.name,
                output: workspace.output,
                focused: workspace.focused,
                visible: workspace.visible,
                urgent: workspace.urgent,
            })
            .collect();

        workspaces
            .sort_by_key(|workspace| (workspace.num.unwrap_or(i32::MAX), workspace.name.clone()));
        Ok(workspaces)
    }

    pub fn get_focused_title(&mut self) -> Fallible<Option<String>> {
        let tree = self.connection.get_tree()?;
        Ok(find_focused_title(&tree))
    }

    pub fn snapshot(&mut self) -> Fallible<PanelState> {
        Ok(PanelState {
            workspaces: self.get_workspaces()?,
            focused_title: self.get_focused_title()?,
        })
    }

    pub fn run_listener(self, tx: Sender<PanelUpdate>, debounce_window: Duration) -> Fallible<()> {
        let mut snapshot_client = self;
        if let Ok(initial) = snapshot_client.snapshot() {
            let _ = tx.send(PanelUpdate::Snapshot(initial));
        }

        let event_rx = spawn_event_stream();
        let normalized_rx = spawn_normalized_stream(event_rx, debounce_window);

        loop {
            if normalized_rx.recv().is_err() {
                break;
            }

            let state = snapshot_client.snapshot()?;
            if tx.send(PanelUpdate::Snapshot(state)).is_err() {
                break;
            }
        }

        Ok(())
    }
}

pub fn spawn_event_stream() -> Receiver<SwaySignal> {
    let (event_tx, event_rx) = mpsc::channel();

    thread::spawn(move || {
        let connection = match Connection::new() {
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!(?error, "failed to connect event stream");
                return;
            }
        };

        let mut events = match connection.subscribe([EventType::Workspace, EventType::Window]) {
            Ok(events) => events,
            Err(error) => {
                tracing::warn!(?error, "failed to subscribe to sway events");
                return;
            }
        };

        for event in &mut events {
            if let Err(error) = event {
                tracing::warn!(?error, "sway event stream ended with error");
                break;
            }

            tracing::debug!(
                stage = "queued_events",
                queued_events = 1,
                "received sway event signal"
            );

            if event_tx.send(SwaySignal::WorkspaceOrWindow).is_err() {
                break;
            }
        }
    });

    event_rx
}

pub fn spawn_normalized_stream(
    event_rx: Receiver<SwaySignal>,
    debounce_window: Duration,
) -> Receiver<SwaySignal> {
    let (normalized_tx, normalized_rx) = mpsc::channel();

    thread::spawn(move || {
        while let Ok(event) = event_rx.recv() {
            let mut latest_event = event;

            loop {
                match event_rx.recv_timeout(debounce_window) {
                    Ok(next_event) => latest_event = next_event,
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        let _ = normalized_tx.send(latest_event);
                        return;
                    }
                }
            }

            if normalized_tx.send(latest_event).is_err() {
                break;
            }
        }
    });

    normalized_rx
}

fn find_focused_title(node: &Node) -> Option<String> {
    if node.focused {
        if let Some(name) = node.name.as_ref() {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }

    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(find_focused_title)
}
