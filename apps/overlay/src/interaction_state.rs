use common::contracts::ZoomLevel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionState {
    OverviewIdle,
    OverviewSelecting,
    OverviewPanning,
    OverviewDraggingCluster,
    ClusterZoom,
    FocusZoom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionEvent {
    ClickCluster,
    ClickBackground,
    DoubleClickCluster,
    Enter,
    Esc,
    DragStartPan,
    DragStartCluster,
    DragRelease,
    DragCancel,
    ZoomToCluster,
    ZoomToFocus,
    ZoomToOverview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeAction {
    CancelDrag,
    CloseTransientOverlayUi,
    ZoomBack,
    None,
}

#[derive(Debug, Clone)]
pub struct InteractionMachine {
    state: InteractionState,
}

impl Default for InteractionMachine {
    fn default() -> Self {
        Self {
            state: InteractionState::OverviewIdle,
        }
    }
}

impl InteractionMachine {
    pub fn state(&self) -> InteractionState {
        self.state
    }

    pub fn sync_zoom(&mut self, zoom: ZoomLevel) {
        let (event, next) = match zoom {
            ZoomLevel::Overview => (
                InteractionEvent::ZoomToOverview,
                match self.state {
                    InteractionState::ClusterZoom | InteractionState::FocusZoom => {
                        InteractionState::OverviewIdle
                    }
                    existing => existing,
                },
            ),
            ZoomLevel::Cluster(_) => (
                InteractionEvent::ZoomToCluster,
                InteractionState::ClusterZoom,
            ),
            ZoomLevel::Focus(_) => (InteractionEvent::ZoomToFocus, InteractionState::FocusZoom),
        };
        if next != self.state {
            self.transition(event, next);
        }
    }

    pub fn on_event(&mut self, event: InteractionEvent) {
        let next = match (self.state, event) {
            (InteractionState::OverviewIdle, InteractionEvent::ClickCluster) => {
                InteractionState::OverviewSelecting
            }
            (InteractionState::OverviewSelecting, InteractionEvent::ClickBackground) => {
                InteractionState::OverviewIdle
            }
            (InteractionState::OverviewSelecting, InteractionEvent::DragStartCluster) => {
                InteractionState::OverviewDraggingCluster
            }
            (InteractionState::OverviewIdle, InteractionEvent::DragStartPan)
            | (InteractionState::OverviewSelecting, InteractionEvent::DragStartPan) => {
                InteractionState::OverviewPanning
            }
            (InteractionState::OverviewDraggingCluster, InteractionEvent::DragRelease)
            | (InteractionState::OverviewDraggingCluster, InteractionEvent::DragCancel) => {
                InteractionState::OverviewSelecting
            }
            (InteractionState::OverviewPanning, InteractionEvent::DragRelease)
            | (InteractionState::OverviewPanning, InteractionEvent::DragCancel) => {
                InteractionState::OverviewIdle
            }
            (InteractionState::OverviewSelecting, InteractionEvent::DoubleClickCluster)
            | (InteractionState::OverviewSelecting, InteractionEvent::Enter)
            | (InteractionState::ClusterZoom, InteractionEvent::ZoomToCluster) => {
                InteractionState::ClusterZoom
            }
            (InteractionState::FocusZoom, InteractionEvent::ZoomToFocus) => {
                InteractionState::FocusZoom
            }
            (InteractionState::ClusterZoom, InteractionEvent::Esc) => {
                InteractionState::OverviewIdle
            }
            (InteractionState::FocusZoom, InteractionEvent::Esc) => InteractionState::ClusterZoom,
            (state, _) => state,
        };

        self.transition(event, next);
    }

    pub fn handle_escape(&mut self, transient_overlay_open: bool) -> EscapeAction {
        let action = match self.state {
            InteractionState::OverviewDraggingCluster | InteractionState::OverviewPanning => {
                self.on_event(InteractionEvent::DragCancel);
                EscapeAction::CancelDrag
            }
            InteractionState::OverviewSelecting if transient_overlay_open => {
                self.on_event(InteractionEvent::ClickBackground);
                EscapeAction::CloseTransientOverlayUi
            }
            InteractionState::OverviewSelecting
            | InteractionState::ClusterZoom
            | InteractionState::FocusZoom => {
                self.on_event(InteractionEvent::Esc);
                EscapeAction::ZoomBack
            }
            InteractionState::OverviewIdle if transient_overlay_open => {
                EscapeAction::CloseTransientOverlayUi
            }
            InteractionState::OverviewIdle => EscapeAction::None,
        };
        tracing::info!(state = ?self.state, ?action, "escape action resolved");
        action
    }

    fn transition(&mut self, event: InteractionEvent, next: InteractionState) {
        let prev = self.state;
        self.state = next;
        tracing::info!(?prev, ?event, next = ?self.state, "interaction transition");
    }
}
