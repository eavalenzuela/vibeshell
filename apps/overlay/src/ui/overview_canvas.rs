use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use common::contracts::{CanvasState, Cluster, ClusterId, Viewport, Window};
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::interaction::{dispatch_ipc_mutation_detached, IpcMutation};
use crate::interaction_state::{EscapeAction, InteractionEvent, InteractionMachine};

const CARD_WIDTH: f64 = 320.0;
const CARD_HEIGHT: f64 = 140.0;
const DRAG_THRESHOLD_PX: f64 = 8.0;
const SCROLL_ZOOM_STEP: f64 = 1.12;
const MIN_SCALE: f64 = 0.35;
const MAX_SCALE: f64 = 2.50;
const KEY_PAN_STEP: f64 = 96.0;
const KEY_PAN_STEP_LARGE: f64 = 384.0;
const KEY_MOVE_STEP: f64 = 32.0;
const KEY_MOVE_STEP_LARGE: f64 = 128.0;
const GLOBAL_CANVAS_MIN: f64 = -10000.0;
const GLOBAL_CANVAS_MAX: f64 = 10000.0;

#[derive(Clone)]
pub struct OverviewCanvas {
    root: gtk::Box,
    area: gtk::DrawingArea,
    status_label: gtk::Label,
    data: Rc<RefCell<WidgetState>>,
}

#[derive(Clone)]
enum DragMode {
    PendingCluster {
        cluster_id: ClusterId,
        start_canvas_x: f64,
        start_canvas_y: f64,
    },
    DraggingCluster {
        cluster_id: ClusterId,
        start_canvas_x: f64,
        start_canvas_y: f64,
    },
    Panning {
        viewport_start: (f64, f64),
    },
}

#[derive(Clone, Copy)]
enum MoveMode {
    Keyboard { _cluster_id: ClusterId },
}

struct WidgetState {
    canvas_state: CanvasState,
    selected_cluster: Option<ClusterId>,
    cluster_offsets: HashMap<ClusterId, (f64, f64)>,
    drag_mode: Option<DragMode>,
    move_mode: Option<MoveMode>,
    interaction: InteractionMachine,
    daemon_viewport: Viewport,
    has_local_viewport: bool,
}

impl OverviewCanvas {
    pub fn new(
        on_activate: Rc<dyn Fn(ClusterId)>,
        on_dive: Rc<dyn Fn(ClusterId)>,
        on_zoom_back: Rc<dyn Fn()>,
        on_mutation: Rc<dyn Fn(IpcMutation)>,
    ) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .hexpand(true)
            .vexpand(true)
            .build();

        let status_label = gtk::Label::builder()
            .xalign(0.0)
            .margin_start(12)
            .margin_end(12)
            .margin_top(8)
            .margin_bottom(4)
            .build();
        status_label.set_wrap(true);
        status_label.set_accessible_role(gtk::AccessibleRole::Status);

        let area = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .build();
        area.set_focusable(true);

        let data = Rc::new(RefCell::new(WidgetState {
            canvas_state: CanvasState::default(),
            selected_cluster: None,
            cluster_offsets: HashMap::new(),
            drag_mode: None,
            move_mode: None,
            interaction: InteractionMachine::default(),
            daemon_viewport: Viewport::default(),
            has_local_viewport: false,
        }));

        area.set_draw_func({
            let data = Rc::clone(&data);
            move |_, cr, width, height| {
                let state = data.borrow();
                draw_canvas(&state, cr, width as f64, height as f64);
            }
        });

        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        click.connect_pressed({
            let area = area.clone();
            let status_label = status_label.clone();
            let data = Rc::clone(&data);
            let on_dive = Rc::clone(&on_dive);
            let on_mutation = Rc::clone(&on_mutation);
            move |gesture, n_press, x, y| {
                let modifiers = gesture.current_event_state();
                let mut state = data.borrow_mut();
                let hit = hit_test(&state, width(&area), height(&area), x, y);

                if n_press >= 2 {
                    state
                        .interaction
                        .on_event(InteractionEvent::DoubleClickCluster);
                    if let Some(cluster_id) = state.selected_cluster {
                        on_dive(cluster_id);
                    }
                    return;
                }

                state.move_mode = None;

                if let Some(cluster_id) = hit {
                    state.selected_cluster = Some(cluster_id);
                    state.interaction.on_event(InteractionEvent::ClickCluster);
                    on_mutation(IpcMutation::SelectCluster {
                        cluster: cluster_id,
                    });
                    if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                        state.interaction.on_event(InteractionEvent::DragStartPan);
                        state.drag_mode = Some(DragMode::Panning {
                            viewport_start: (
                                state.canvas_state.viewport.x,
                                state.canvas_state.viewport.y,
                            ),
                        });
                    } else {
                        state.drag_mode = Some(DragMode::PendingCluster {
                            cluster_id,
                            start_canvas_x: x,
                            start_canvas_y: y,
                        });
                    }
                } else {
                    state.selected_cluster = None;
                    state
                        .interaction
                        .on_event(InteractionEvent::ClickBackground);
                    state.interaction.on_event(InteractionEvent::DragStartPan);
                    state.drag_mode = Some(DragMode::Panning {
                        viewport_start: (
                            state.canvas_state.viewport.x,
                            state.canvas_state.viewport.y,
                        ),
                    });
                }

                update_status(&state, &status_label);
                area.grab_focus();
                area.queue_draw();
            }
        });
        click.connect_released({
            let data = Rc::clone(&data);
            let on_mutation = Rc::clone(&on_mutation);
            move |_, _, _, _| {
                let mut state = data.borrow_mut();
                if matches!(state.drag_mode, Some(DragMode::DraggingCluster { .. })) {
                    on_mutation(IpcMutation::CommitClusterDrag);
                }
                if matches!(
                    state.drag_mode,
                    Some(DragMode::DraggingCluster { .. }) | Some(DragMode::Panning { .. })
                ) {
                    state.interaction.on_event(InteractionEvent::DragRelease);
                }
                state.drag_mode = None;
            }
        });
        area.add_controller(click);

        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        drag.connect_drag_update({
            let area = area.clone();
            let status_label = status_label.clone();
            let data = Rc::clone(&data);
            let on_mutation = Rc::clone(&on_mutation);
            move |_, dx, dy| {
                let mut state = data.borrow_mut();
                let Some(mode) = state.drag_mode.clone() else {
                    return;
                };

                match mode {
                    DragMode::PendingCluster {
                        cluster_id,
                        start_canvas_x,
                        start_canvas_y,
                    } => {
                        let distance = (dx * dx + dy * dy).sqrt();
                        if distance < DRAG_THRESHOLD_PX {
                            return;
                        }
                        state
                            .interaction
                            .on_event(InteractionEvent::DragStartCluster);
                        state.drag_mode = Some(DragMode::DraggingCluster {
                            cluster_id,
                            start_canvas_x,
                            start_canvas_y,
                        });
                        on_mutation(IpcMutation::BeginClusterDrag {
                            cluster: cluster_id,
                            pointer_canvas_x: start_canvas_x,
                            pointer_canvas_y: start_canvas_y,
                            base_revision: state.canvas_state.state_revision,
                        });
                        let pointer_canvas_x = start_canvas_x + dx;
                        let pointer_canvas_y = start_canvas_y + dy;
                        on_mutation(IpcMutation::UpdateClusterDrag {
                            pointer_canvas_x,
                            pointer_canvas_y,
                        });
                        apply_cluster_drag(&mut state, &area, cluster_id, dx, dy);
                    }
                    DragMode::DraggingCluster {
                        cluster_id,
                        start_canvas_x,
                        start_canvas_y,
                    } => {
                        let pointer_canvas_x = start_canvas_x + dx;
                        let pointer_canvas_y = start_canvas_y + dy;
                        on_mutation(IpcMutation::UpdateClusterDrag {
                            pointer_canvas_x,
                            pointer_canvas_y,
                        });
                        apply_cluster_drag(&mut state, &area, cluster_id, dx, dy);
                    }
                    DragMode::Panning { viewport_start } => {
                        let scale = state.canvas_state.viewport.scale.max(MIN_SCALE);
                        state.canvas_state.viewport.x = viewport_start.0 - (dx / scale);
                        state.canvas_state.viewport.y = viewport_start.1 - (dy / scale);
                        area.queue_draw();
                    }
                }

                update_status(&state, &status_label);
            }
        });
        drag.connect_drag_end({
            let data = Rc::clone(&data);
            let on_mutation = Rc::clone(&on_mutation);
            move |_, _, _| {
                let mut state = data.borrow_mut();
                if matches!(state.drag_mode, Some(DragMode::DraggingCluster { .. })) {
                    on_mutation(IpcMutation::CommitClusterDrag);
                }
                if let Some(DragMode::Panning { .. }) = &state.drag_mode {
                    let dx = state.canvas_state.viewport.x - state.daemon_viewport.x;
                    let dy = state.canvas_state.viewport.y - state.daemon_viewport.y;
                    if dx.abs() > 0.5 || dy.abs() > 0.5 {
                        dispatch_ipc_mutation_detached(IpcMutation::OverviewPan { dx, dy });
                        state.daemon_viewport = state.canvas_state.viewport.clone();
                        state.has_local_viewport = true;
                    }
                }
                if state.drag_mode.is_some() {
                    state.interaction.on_event(InteractionEvent::DragRelease);
                }
                state.drag_mode = None;
            }
        });
        area.add_controller(drag);

        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
        );
        scroll.connect_scroll({
            let area = area.clone();
            let data = Rc::clone(&data);
            move |_, _dx, dy| {
                let mut state = data.borrow_mut();
                let factor = if dy < 0.0 {
                    SCROLL_ZOOM_STEP
                } else {
                    1.0 / SCROLL_ZOOM_STEP
                };
                state.canvas_state.viewport.scale =
                    (state.canvas_state.viewport.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
                let delta = if dy < 0.0 { 1.0 } else { -1.0 };
                let anchor_x = state.canvas_state.viewport.x;
                let anchor_y = state.canvas_state.viewport.y;
                dispatch_ipc_mutation_detached(IpcMutation::OverviewZoom {
                    delta,
                    anchor_x,
                    anchor_y,
                });
                state.has_local_viewport = true;
                area.queue_draw();
                glib::Propagation::Stop
            }
        });
        area.add_controller(scroll);

        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed({
            let area = area.clone();
            let status_label = status_label.clone();
            let data = Rc::clone(&data);
            let on_activate = Rc::clone(&on_activate);
            let on_zoom_back = Rc::clone(&on_zoom_back);
            let on_mutation = Rc::clone(&on_mutation);
            move |_, keyval, _, modifiers| {
                let mut state = data.borrow_mut();
                let large_step = modifiers.contains(gdk::ModifierType::SHIFT_MASK);

                if matches!(state.move_mode, Some(MoveMode::Keyboard { .. })) {
                    let step = if large_step {
                        KEY_MOVE_STEP_LARGE
                    } else {
                        KEY_MOVE_STEP
                    };
                    match keyval {
                        gdk::Key::Up | gdk::Key::Down | gdk::Key::Left | gdk::Key::Right => {
                            let (dx, dy) = match keyval {
                                gdk::Key::Up => (0.0, -step),
                                gdk::Key::Down => (0.0, step),
                                gdk::Key::Left => (-step, 0.0),
                                gdk::Key::Right => (step, 0.0),
                                _ => (0.0, 0.0),
                            };
                            if let Some(cluster_id) = state.selected_cluster {
                                on_mutation(IpcMutation::KeyboardMoveBy { dx, dy });
                                if let Some((cluster_x, cluster_y)) = state
                                    .canvas_state
                                    .clusters
                                    .iter()
                                    .find(|c| c.id == cluster_id)
                                    .map(|cluster| (cluster.x, cluster.y))
                                {
                                    let entry = state
                                        .cluster_offsets
                                        .entry(cluster_id)
                                        .or_insert((0.0, 0.0));
                                    let target_x = (cluster_x + entry.0 + dx)
                                        .clamp(GLOBAL_CANVAS_MIN, GLOBAL_CANVAS_MAX);
                                    let target_y = (cluster_y + entry.1 + dy)
                                        .clamp(GLOBAL_CANVAS_MIN, GLOBAL_CANVAS_MAX);
                                    entry.0 = target_x - cluster_x;
                                    entry.1 = target_y - cluster_y;
                                }
                            }
                            update_status(&state, &status_label);
                            area.queue_draw();
                            return glib::Propagation::Stop;
                        }
                        gdk::Key::Return => {
                            on_mutation(IpcMutation::CommitKeyboardMove);
                            state.move_mode = None;
                            state.cluster_offsets.clear();
                            update_status(&state, &status_label);
                            area.queue_draw();
                            return glib::Propagation::Stop;
                        }
                        gdk::Key::Escape => {
                            on_mutation(IpcMutation::CancelKeyboardMove);
                            state.move_mode = None;
                            state.cluster_offsets.clear();
                            update_status(&state, &status_label);
                            area.queue_draw();
                            return glib::Propagation::Stop;
                        }
                        _ => {}
                    }
                }

                match keyval {
                    gdk::Key::Tab => {
                        traverse_selection(&mut state, 1, &on_mutation);
                        update_status(&state, &status_label);
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::ISO_Left_Tab => {
                        traverse_selection(&mut state, -1, &on_mutation);
                        update_status(&state, &status_label);
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::m | gdk::Key::M => {
                        if let Some(cluster_id) = state.selected_cluster {
                            on_mutation(IpcMutation::EnterKeyboardMoveMode {
                                cluster: cluster_id,
                            });
                            state.move_mode = Some(MoveMode::Keyboard {
                                _cluster_id: cluster_id,
                            });
                            update_status(&state, &status_label);
                            area.queue_draw();
                            glib::Propagation::Stop
                        } else {
                            glib::Propagation::Proceed
                        }
                    }
                    gdk::Key::plus | gdk::Key::equal => {
                        state.canvas_state.viewport.scale = (state.canvas_state.viewport.scale
                            * SCROLL_ZOOM_STEP)
                            .clamp(MIN_SCALE, MAX_SCALE);
                        let anchor_x = state.canvas_state.viewport.x;
                        let anchor_y = state.canvas_state.viewport.y;
                        dispatch_ipc_mutation_detached(IpcMutation::OverviewZoom {
                            delta: 1.0,
                            anchor_x,
                            anchor_y,
                        });
                        state.has_local_viewport = true;
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::minus | gdk::Key::underscore => {
                        state.canvas_state.viewport.scale = (state.canvas_state.viewport.scale
                            / SCROLL_ZOOM_STEP)
                            .clamp(MIN_SCALE, MAX_SCALE);
                        let anchor_x = state.canvas_state.viewport.x;
                        let anchor_y = state.canvas_state.viewport.y;
                        dispatch_ipc_mutation_detached(IpcMutation::OverviewZoom {
                            delta: -1.0,
                            anchor_x,
                            anchor_y,
                        });
                        state.has_local_viewport = true;
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::Up => {
                        let step = if large_step {
                            KEY_PAN_STEP_LARGE
                        } else {
                            KEY_PAN_STEP
                        };
                        let scale = state.canvas_state.viewport.scale.max(MIN_SCALE);
                        let delta = step / scale;
                        state.canvas_state.viewport.y -= delta;
                        dispatch_ipc_mutation_detached(IpcMutation::OverviewPan {
                            dx: 0.0,
                            dy: -delta,
                        });
                        state.daemon_viewport = state.canvas_state.viewport.clone();
                        state.has_local_viewport = true;
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::Down => {
                        let step = if large_step {
                            KEY_PAN_STEP_LARGE
                        } else {
                            KEY_PAN_STEP
                        };
                        let scale = state.canvas_state.viewport.scale.max(MIN_SCALE);
                        let delta = step / scale;
                        state.canvas_state.viewport.y += delta;
                        dispatch_ipc_mutation_detached(IpcMutation::OverviewPan {
                            dx: 0.0,
                            dy: delta,
                        });
                        state.daemon_viewport = state.canvas_state.viewport.clone();
                        state.has_local_viewport = true;
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::Left => {
                        let step = if large_step {
                            KEY_PAN_STEP_LARGE
                        } else {
                            KEY_PAN_STEP
                        };
                        let scale = state.canvas_state.viewport.scale.max(MIN_SCALE);
                        let delta = step / scale;
                        state.canvas_state.viewport.x -= delta;
                        dispatch_ipc_mutation_detached(IpcMutation::OverviewPan {
                            dx: -delta,
                            dy: 0.0,
                        });
                        state.daemon_viewport = state.canvas_state.viewport.clone();
                        state.has_local_viewport = true;
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::Right => {
                        let step = if large_step {
                            KEY_PAN_STEP_LARGE
                        } else {
                            KEY_PAN_STEP
                        };
                        let scale = state.canvas_state.viewport.scale.max(MIN_SCALE);
                        let delta = step / scale;
                        state.canvas_state.viewport.x += delta;
                        dispatch_ipc_mutation_detached(IpcMutation::OverviewPan {
                            dx: delta,
                            dy: 0.0,
                        });
                        state.daemon_viewport = state.canvas_state.viewport.clone();
                        state.has_local_viewport = true;
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::n | gdk::Key::N => {
                        let name = format!("Cluster {}", state.canvas_state.clusters.len() + 1);
                        let x = state.canvas_state.viewport.x;
                        let y = state.canvas_state.viewport.y;
                        on_mutation(IpcMutation::CreateCluster { name, x, y });
                        glib::Propagation::Stop
                    }
                    gdk::Key::r | gdk::Key::R => {
                        recenter_selected_cluster(&mut state);
                        update_status(&state, &status_label);
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::Return => {
                        state.interaction.on_event(InteractionEvent::Enter);
                        if let Some(cluster_id) = state.selected_cluster {
                            on_activate(cluster_id);
                        } else {
                            status_label.set_text(
                                "No cluster selected — use Tab to select or N to create one",
                            );
                        }
                        glib::Propagation::Stop
                    }
                    gdk::Key::Escape => {
                        let transient_overlay_open = state.selected_cluster.is_some();
                        match state.interaction.handle_escape(transient_overlay_open) {
                            EscapeAction::CancelDrag => {
                                if matches!(state.drag_mode, Some(DragMode::DraggingCluster { .. }))
                                {
                                    on_mutation(IpcMutation::CancelClusterDrag);
                                }
                                state.drag_mode = None;
                                area.queue_draw();
                            }
                            EscapeAction::CloseTransientOverlayUi => {
                                state.selected_cluster = None;
                                area.queue_draw();
                            }
                            EscapeAction::ZoomBack => {
                                on_zoom_back();
                            }
                            EscapeAction::None => {}
                        }
                        update_status(&state, &status_label);
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            }
        });
        area.add_controller(key);

        root.append(&status_label);
        root.append(&area);

        {
            let state = data.borrow();
            update_status(&state, &status_label);
        }

        Self {
            root,
            area,
            status_label,
            data,
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn set_canvas_state(&self, state: CanvasState) {
        let mut data = self.data.borrow_mut();
        data.interaction.sync_zoom(state.zoom.clone());

        // Always track what the daemon last acknowledged
        data.daemon_viewport = state.viewport.clone();

        // Preserve local viewport if the user has panned/zoomed since last poll
        let preserve = data.has_local_viewport;
        let local_viewport = data.canvas_state.viewport.clone();
        data.canvas_state = state;
        if preserve {
            data.canvas_state.viewport = local_viewport;
        }

        if !data
            .canvas_state
            .clusters
            .iter()
            .any(|cluster| Some(cluster.id) == data.selected_cluster)
        {
            data.selected_cluster = None;
            data.move_mode = None;
        }

        let cluster_ids: std::collections::HashSet<_> = data
            .canvas_state
            .clusters
            .iter()
            .map(|cluster| cluster.id)
            .collect();
        data.cluster_offsets
            .retain(|cluster_id, _| cluster_ids.contains(cluster_id));

        update_status(&data, &self.status_label);
        self.area.queue_draw();
    }
}

fn width(area: &gtk::DrawingArea) -> f64 {
    f64::from(area.allocated_width())
}

fn height(area: &gtk::DrawingArea) -> f64 {
    f64::from(area.allocated_height())
}

fn apply_cluster_drag(
    state: &mut WidgetState,
    area: &gtk::DrawingArea,
    cluster_id: ClusterId,
    dx: f64,
    dy: f64,
) {
    let viewport = &state.canvas_state.viewport;
    let scale = viewport.scale.max(MIN_SCALE);
    let world_dx = dx / scale;
    let world_dy = dy / scale;

    let entry = state
        .cluster_offsets
        .entry(cluster_id)
        .or_insert((0.0, 0.0));
    if let Some(cluster) = state
        .canvas_state
        .clusters
        .iter()
        .find(|c| c.id == cluster_id)
    {
        let target_x = (cluster.x + world_dx).clamp(GLOBAL_CANVAS_MIN, GLOBAL_CANVAS_MAX);
        let target_y = (cluster.y + world_dy).clamp(GLOBAL_CANVAS_MIN, GLOBAL_CANVAS_MAX);
        entry.0 = target_x - cluster.x;
        entry.1 = target_y - cluster.y;
    }

    area.queue_draw();
}

fn update_status(state: &WidgetState, label: &gtk::Label) {
    let mut parts = Vec::new();
    if let Some(cluster_id) = state.selected_cluster {
        if let Some(cluster) = state
            .canvas_state
            .clusters
            .iter()
            .find(|c| c.id == cluster_id)
        {
            let offset = state
                .cluster_offsets
                .get(&cluster_id)
                .copied()
                .unwrap_or((0.0, 0.0));
            let x = cluster.x + offset.0;
            let y = cluster.y + offset.1;
            parts.push(format!(
                "Selected: {} ({}) @ ({x:.0}, {y:.0})",
                cluster.name, cluster.id
            ));
        }
    } else {
        parts.push("Selected: none".to_owned());
    }

    match state.move_mode {
        Some(MoveMode::Keyboard { .. }) => {
            parts.push("Move mode: ON (Arrows move 32px, Shift+Arrow move 128px, Enter commit, Esc cancel)".to_owned());
        }
        None => {
            parts.push("Move mode: OFF (Tab/Shift+Tab traverse, M enters move mode)".to_owned());
        }
    }

    if selected_cluster_offscreen(
        state,
        f64::from(state.canvas_state.output.width),
        f64::from(state.canvas_state.output.height),
    ) {
        parts.push("Selected cluster is off-screen (press R to recenter)".to_owned());
    }

    let text = parts.join("  •  ");
    label.set_text(&text);
    label.set_tooltip_text(Some(&text));
}

fn traverse_selection(
    state: &mut WidgetState,
    direction: isize,
    on_mutation: &Rc<dyn Fn(IpcMutation)>,
) {
    if state.canvas_state.clusters.is_empty() {
        state.selected_cluster = None;
        return;
    }

    let len = state.canvas_state.clusters.len() as isize;
    let current = state
        .selected_cluster
        .and_then(|id| {
            state
                .canvas_state
                .clusters
                .iter()
                .position(|cluster| cluster.id == id)
        })
        .map(|idx| idx as isize)
        .unwrap_or(0);

    let next = (current + direction).rem_euclid(len) as usize;
    let cluster_id = state.canvas_state.clusters[next].id;
    state.selected_cluster = Some(cluster_id);
    state.interaction.on_event(InteractionEvent::ClickCluster);
    on_mutation(IpcMutation::SelectCluster {
        cluster: cluster_id,
    });
}

fn selected_cluster_offscreen(state: &WidgetState, width: f64, height: f64) -> bool {
    let Some(cluster_id) = state.selected_cluster else {
        return false;
    };
    let Some(cluster) = state
        .canvas_state
        .clusters
        .iter()
        .find(|c| c.id == cluster_id)
    else {
        return false;
    };
    let (sx, sy) = project_cluster(state, width.max(1.0), height.max(1.0), cluster);
    let left = sx - CARD_WIDTH / 2.0;
    let right = sx + CARD_WIDTH / 2.0;
    let top = sy - CARD_HEIGHT / 2.0;
    let bottom = sy + CARD_HEIGHT / 2.0;
    right < 0.0 || left > width || bottom < 0.0 || top > height
}

fn recenter_selected_cluster(state: &mut WidgetState) {
    let Some(cluster_id) = state.selected_cluster else {
        return;
    };
    let Some(cluster) = state
        .canvas_state
        .clusters
        .iter()
        .find(|c| c.id == cluster_id)
    else {
        return;
    };
    let offset = state
        .cluster_offsets
        .get(&cluster_id)
        .copied()
        .unwrap_or((0.0, 0.0));
    state.canvas_state.viewport.x = cluster.x + offset.0;
    state.canvas_state.viewport.y = cluster.y + offset.1;
}

fn draw_canvas(state: &WidgetState, cr: &gtk::cairo::Context, width: f64, height: f64) {
    cr.set_source_rgb(0.09, 0.10, 0.12);
    let _ = cr.paint();

    let windows_by_id: HashMap<_, _> = state
        .canvas_state
        .windows
        .iter()
        .map(|window| (window.id, window))
        .collect();

    for cluster in &state.canvas_state.clusters {
        draw_cluster_card(state, cr, width, height, cluster, &windows_by_id);
    }
}

fn draw_cluster_card(
    state: &WidgetState,
    cr: &gtk::cairo::Context,
    width: f64,
    height: f64,
    cluster: &Cluster,
    windows_by_id: &HashMap<u64, &Window>,
) {
    let (sx, sy) = project_cluster(state, width, height, cluster);
    let rect_x = sx - CARD_WIDTH / 2.0;
    let rect_y = sy - CARD_HEIGHT / 2.0;

    let selected = state.selected_cluster == Some(cluster.id);
    if selected {
        cr.set_source_rgba(0.25, 0.54, 0.95, 0.94);
    } else {
        cr.set_source_rgba(0.15, 0.16, 0.18, 0.92);
    }
    cr.rectangle(rect_x, rect_y, CARD_WIDTH, CARD_HEIGHT);
    let _ = cr.fill();

    cr.set_source_rgb(0.85, 0.88, 0.92);
    cr.select_font_face(
        "Sans",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Bold,
    );
    cr.set_font_size(16.0);
    cr.move_to(rect_x + 12.0, rect_y + 24.0);
    let _ = cr.show_text(&format!("{} ({})", cluster.name, cluster.windows.len()));

    cr.select_font_face(
        "Sans",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Normal,
    );
    cr.set_font_size(12.0);
    let mut y = rect_y + 46.0;
    for window_id in cluster.windows.iter().take(4) {
        let text = if let Some(window) = windows_by_id.get(window_id) {
            let title = if window.title.trim().is_empty() {
                "untitled"
            } else {
                window.title.as_str()
            };
            let app_id = window.app_id.as_deref().unwrap_or("unknown-app");
            format!("• {title} — {app_id}")
        } else {
            format!("• closed window ({window_id})")
        };

        cr.move_to(rect_x + 12.0, y);
        let _ = cr.show_text(&text);
        y += 18.0;
    }

    if cluster.windows.len() > 4 {
        cr.move_to(rect_x + 12.0, rect_y + CARD_HEIGHT - 12.0);
        let _ = cr.show_text("…");
    }
}

fn project_cluster(state: &WidgetState, width: f64, height: f64, cluster: &Cluster) -> (f64, f64) {
    let viewport = &state.canvas_state.viewport;
    let scale = viewport.scale.max(MIN_SCALE);
    let offset = state
        .cluster_offsets
        .get(&cluster.id)
        .copied()
        .unwrap_or((0.0, 0.0));
    let world_x = cluster.x + offset.0;
    let world_y = cluster.y + offset.1;
    let sx = (world_x - viewport.x) * scale + (width / 2.0);
    let sy = (world_y - viewport.y) * scale + (height / 2.0);
    (sx, sy)
}

fn hit_test(state: &WidgetState, width: f64, height: f64, x: f64, y: f64) -> Option<ClusterId> {
    for cluster in state.canvas_state.clusters.iter().rev() {
        let (sx, sy) = project_cluster(state, width, height, cluster);
        let left = sx - CARD_WIDTH / 2.0;
        let right = sx + CARD_WIDTH / 2.0;
        let top = sy - CARD_HEIGHT / 2.0;
        let bottom = sy + CARD_HEIGHT / 2.0;
        if x >= left && x <= right && y >= top && y <= bottom {
            return Some(cluster.id);
        }
    }
    None
}
