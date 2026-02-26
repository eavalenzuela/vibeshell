use std::sync::mpsc::{self, RecvTimeoutError, Sender};
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
                if event.is_err() || event_tx.send(()).is_err() {
                    break;
                }
            }
        });

        loop {
            if event_rx.recv().is_err() {
                break;
            }

            loop {
                match event_rx.recv_timeout(debounce_window) {
                    Ok(()) => continue,
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return Ok(()),
                }
            }

            let state = snapshot_client.snapshot()?;
            if tx.send(PanelUpdate::Snapshot(state)).is_err() {
                break;
            }
        }

        Ok(())
    }
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
