use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use common::contracts::{
    CanvasState, Cluster, ClusterId, Viewport, Window, ZoomLevel, ZoomTransition,
};
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

// Phase 4B: snap, inertia, animation
const SNAP_GRID_PX: f64 = 200.0;
const SNAP_THRESHOLD_SCREEN: f64 = 24.0;
const INERTIA_FRICTION: f64 = 0.86;
const INERTIA_MIN_PX: f64 = 0.5;
const RECENTER_DURATION_MS: f64 = 220.0;
const DIVE_DURATION_MS: f64 = 220.0;
const DIVE_ZOOM_GAIN: f64 = 1.4;

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
    },
    Panning {
        viewport_start: (f64, f64),
    },
}

#[derive(Clone, Copy)]
enum MoveMode {
    Keyboard { _cluster_id: ClusterId },
}

/// A world-space guide line shown while a snap is active during cluster drag/move.
struct SnapGuide {
    /// true = vertical line (x snapped), false = horizontal (y snapped)
    vertical: bool,
    /// World-space coordinate of the guide line
    coord: f64,
}

/// Smooth viewport animation (used for R-key recenter and cluster dive).
struct ViewportAnim {
    start_x: f64,
    start_y: f64,
    target_x: f64,
    target_y: f64,
    start_scale: f64,
    target_scale: f64,
    start: Instant,
    duration_ms: f64,
    /// Generation counter so a new animation cancels any previous timeout callback.
    generation: u64,
    /// Fires once when the animation reaches t=1.0. `take()`n out before invoking
    /// so re-entrant cancels (e.g. user mashes Enter twice) don't double-fire.
    on_complete: Option<Box<dyn FnOnce()>>,
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
    last_drag_ipc: Option<std::time::Instant>,
    output_name: Option<String>,
    // Phase 4B
    snap_guides: Vec<SnapGuide>,
    pan_velocity: (f64, f64), // screen px per drag event (EMA-smoothed)
    prev_drag_dx: f64,        // previous cumulative drag dx (for velocity diff)
    prev_drag_dy: f64,
    viewport_anim: Option<ViewportAnim>,
    inertia_active: bool,
    /// `started_at_ms` of the last `ZoomTransition` we acted on. Prevents
    /// the W1c-25-1 undive animation from restarting every poll for the
    /// duration the transition is observable.
    last_handled_transition_at: Option<u64>,
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

        let output_name = std::env::var("VIBESHELL_OUTPUT").ok();

        let data = Rc::new(RefCell::new(WidgetState {
            canvas_state: CanvasState::default(),
            selected_cluster: None,
            cluster_offsets: HashMap::new(),
            drag_mode: None,
            move_mode: None,
            interaction: InteractionMachine::default(),
            daemon_viewport: Viewport::default(),
            has_local_viewport: false,
            last_drag_ipc: None,
            output_name,
            snap_guides: Vec::new(),
            pan_velocity: (0.0, 0.0),
            prev_drag_dx: 0.0,
            prev_drag_dy: 0.0,
            viewport_anim: None,
            inertia_active: false,
            last_handled_transition_at: None,
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
                        drop(state);
                        start_dive_anim(&area, Rc::clone(&data), cluster_id, Rc::clone(&on_dive));
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
                        reset_pan_tracking(&mut state);
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
                    reset_pan_tracking(&mut state);
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
                let committing_cluster =
                    if let Some(DragMode::DraggingCluster { cluster_id, .. }) = state.drag_mode {
                        Some(cluster_id)
                    } else {
                        None
                    };
                if let Some(cluster_id) = committing_cluster {
                    state.last_drag_ipc = None;
                    state.snap_guides.clear();
                    if let Some(offset) = state.cluster_offsets.remove(&cluster_id) {
                        if let Some(cluster) = state
                            .canvas_state
                            .clusters
                            .iter_mut()
                            .find(|c| c.id == cluster_id)
                        {
                            cluster.x += offset.0;
                            cluster.y += offset.1;
                        }
                    }
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
                        state.drag_mode = Some(DragMode::DraggingCluster { cluster_id });
                        on_mutation(IpcMutation::BeginClusterDrag {
                            cluster: cluster_id,
                            pointer_canvas_x: start_canvas_x,
                            pointer_canvas_y: start_canvas_y,
                        });
                        let (cluster_x, cluster_y) =
                            apply_cluster_drag(&mut state, &area, cluster_id, dx, dy);
                        state.last_drag_ipc = Some(std::time::Instant::now());
                        on_mutation(IpcMutation::UpdateClusterDrag {
                            cluster_x,
                            cluster_y,
                        });
                    }
                    DragMode::DraggingCluster { cluster_id } => {
                        let (cluster_x, cluster_y) =
                            apply_cluster_drag(&mut state, &area, cluster_id, dx, dy);
                        let now = std::time::Instant::now();
                        let should_send = state
                            .last_drag_ipc
                            .is_none_or(|t| now.duration_since(t).as_millis() >= 33);
                        if should_send {
                            state.last_drag_ipc = Some(now);
                            on_mutation(IpcMutation::UpdateClusterDrag {
                                cluster_x,
                                cluster_y,
                            });
                        }
                    }
                    DragMode::Panning { viewport_start } => {
                        let scale = state.canvas_state.viewport.scale.max(MIN_SCALE);

                        // Track pan velocity for inertia (EMA of screen-px delta per event)
                        let delta_x = dx - state.prev_drag_dx;
                        let delta_y = dy - state.prev_drag_dy;
                        state.pan_velocity.0 = state.pan_velocity.0 * 0.5 + delta_x * 0.5;
                        state.pan_velocity.1 = state.pan_velocity.1 * 0.5 + delta_y * 0.5;
                        state.prev_drag_dx = dx;
                        state.prev_drag_dy = dy;

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
            let area = area.clone();
            let on_mutation = Rc::clone(&on_mutation);
            move |_, _, _| {
                let start_inertia_now = {
                    let mut state = data.borrow_mut();
                    let committing_cluster =
                        if let Some(DragMode::DraggingCluster { cluster_id, .. }) = state.drag_mode
                        {
                            Some(cluster_id)
                        } else {
                            None
                        };
                    if let Some(cluster_id) = committing_cluster {
                        state.last_drag_ipc = None;
                        state.snap_guides.clear();
                        if let Some(offset) = state.cluster_offsets.remove(&cluster_id) {
                            if let Some(cluster) = state
                                .canvas_state
                                .clusters
                                .iter_mut()
                                .find(|c| c.id == cluster_id)
                            {
                                cluster.x += offset.0;
                                cluster.y += offset.1;
                            }
                        }
                        on_mutation(IpcMutation::CommitClusterDrag);
                    }

                    let mut launch = false;
                    if let Some(DragMode::Panning { .. }) = &state.drag_mode {
                        let dx = state.canvas_state.viewport.x - state.daemon_viewport.x;
                        let dy = state.canvas_state.viewport.y - state.daemon_viewport.y;
                        if dx.abs() > 0.5 || dy.abs() > 0.5 {
                            dispatch_ipc_mutation_detached(IpcMutation::OverviewPan {
                                dx,
                                dy,
                                output: state.output_name.clone(),
                            });
                            state.daemon_viewport = state.canvas_state.viewport.clone();
                            state.has_local_viewport = true;
                        }

                        let (vx, vy) = state.pan_velocity;
                        let speed = (vx * vx + vy * vy).sqrt();
                        state.prev_drag_dx = 0.0;
                        state.prev_drag_dy = 0.0;
                        if speed > INERTIA_MIN_PX && !state.inertia_active {
                            state.inertia_active = true;
                            launch = true;
                        }
                    }
                    if state.drag_mode.is_some() {
                        state.interaction.on_event(InteractionEvent::DragRelease);
                    }
                    state.drag_mode = None;
                    launch
                };
                if start_inertia_now {
                    start_inertia(&area, Rc::clone(&data));
                }
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
                    output: state.output_name.clone(),
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
                                // Compute new position with snap applied
                                let base_pos = state
                                    .canvas_state
                                    .clusters
                                    .iter()
                                    .find(|c| c.id == cluster_id)
                                    .map(|c| (c.x, c.y));
                                if let Some((cluster_x, cluster_y)) = base_pos {
                                    let scale = state.canvas_state.viewport.scale.max(MIN_SCALE);
                                    let cur = state
                                        .cluster_offsets
                                        .get(&cluster_id)
                                        .copied()
                                        .unwrap_or((0.0, 0.0));
                                    let raw_x = (cluster_x + cur.0 + dx)
                                        .clamp(GLOBAL_CANVAS_MIN, GLOBAL_CANVAS_MAX);
                                    let raw_y = (cluster_y + cur.1 + dy)
                                        .clamp(GLOBAL_CANVAS_MIN, GLOBAL_CANVAS_MAX);
                                    let (snap_x, snap_y, guides) = compute_snap(
                                        raw_x,
                                        raw_y,
                                        cluster_id,
                                        scale,
                                        &state.canvas_state.clusters,
                                    );
                                    let entry = state
                                        .cluster_offsets
                                        .entry(cluster_id)
                                        .or_insert((0.0, 0.0));
                                    entry.0 = snap_x - cluster_x;
                                    entry.1 = snap_y - cluster_y;
                                    state.snap_guides = guides;
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
                            state.snap_guides.clear();
                            update_status(&state, &status_label);
                            area.queue_draw();
                            return glib::Propagation::Stop;
                        }
                        gdk::Key::Escape => {
                            on_mutation(IpcMutation::CancelKeyboardMove);
                            state.move_mode = None;
                            state.cluster_offsets.clear();
                            state.snap_guides.clear();
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
                            output: state.output_name.clone(),
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
                            output: state.output_name.clone(),
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
                        state.viewport_anim = None; // cancel recenter animation
                        dispatch_ipc_mutation_detached(IpcMutation::OverviewPan {
                            dx: 0.0,
                            dy: -delta,
                            output: state.output_name.clone(),
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
                        state.viewport_anim = None;
                        dispatch_ipc_mutation_detached(IpcMutation::OverviewPan {
                            dx: 0.0,
                            dy: delta,
                            output: state.output_name.clone(),
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
                        state.viewport_anim = None;
                        dispatch_ipc_mutation_detached(IpcMutation::OverviewPan {
                            dx: -delta,
                            dy: 0.0,
                            output: state.output_name.clone(),
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
                        state.viewport_anim = None;
                        dispatch_ipc_mutation_detached(IpcMutation::OverviewPan {
                            dx: delta,
                            dy: 0.0,
                            output: state.output_name.clone(),
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
                        let target = compute_recenter_target(&state);
                        drop(state); // release borrow before start_recenter_anim borrows data
                        if let Some((tx, ty)) = target {
                            start_recenter_anim(&area, Rc::clone(&data), tx, ty);
                        }
                        update_status(&data.borrow(), &status_label);
                        glib::Propagation::Stop
                    }
                    gdk::Key::Return => {
                        state.interaction.on_event(InteractionEvent::Enter);
                        if let Some(cluster_id) = state.selected_cluster {
                            drop(state);
                            start_dive_anim(
                                &area,
                                Rc::clone(&data),
                                cluster_id,
                                Rc::clone(&on_activate),
                            );
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
                                    state.last_drag_ipc = None;
                                    state.cluster_offsets.clear();
                                    state.snap_guides.clear();
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
        // W1c-25-1: detect a fresh Cluster→Overview transition before
        // overwriting `canvas_state` so we can trigger the undive animation
        // with the cluster id. We do the actual `start_undive_anim` call
        // *after* dropping the borrow, since the animation needs &mut self
        // via `data.borrow_mut`.
        let undive_target: Option<ClusterId> = pending_undive_target(&self.data, &state);

        let mut data = self.data.borrow_mut();
        data.interaction.sync_zoom(state.zoom.clone());

        // Use output-specific viewport when VIBESHELL_OUTPUT is set
        let effective_viewport = data
            .output_name
            .as_ref()
            .and_then(|name| state.output_viewports.get(name).cloned())
            .unwrap_or_else(|| state.viewport.clone());

        // Always track what the daemon last acknowledged
        data.daemon_viewport = effective_viewport.clone();

        // Preserve local viewport if the user has panned/zoomed since last poll
        let preserve = data.has_local_viewport;
        let local_viewport = data.canvas_state.viewport.clone();
        data.canvas_state = state;
        data.canvas_state.viewport = effective_viewport;
        if preserve {
            data.canvas_state.viewport = local_viewport;
        }
        if let Some(t) = data.canvas_state.transition.as_ref() {
            data.last_handled_transition_at = Some(t.started_at_ms);
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
        drop(data);
        if let Some(cluster_id) = undive_target {
            start_undive_anim(&self.area, Rc::clone(&self.data), cluster_id);
        }
    }
}

/// Inspect the incoming canvas state and decide whether to fire an undive
/// animation: only if the daemon stamped a fresh `Cluster→Overview`
/// transition we haven't already acted on. Returns the source cluster id.
fn pending_undive_target(
    data: &Rc<RefCell<WidgetState>>,
    incoming: &CanvasState,
) -> Option<ClusterId> {
    let transition: &ZoomTransition = incoming.transition.as_ref()?;
    let from_cluster = match transition.from {
        ZoomLevel::Cluster(id) => id,
        _ => return None,
    };
    if !matches!(transition.to, ZoomLevel::Overview) {
        return None;
    }
    let state = data.borrow();
    if state.last_handled_transition_at == Some(transition.started_at_ms) {
        return None;
    }
    // Skip if the user is mid-drag/move — they don't want a viewport jump.
    if state.drag_mode.is_some() || state.move_mode.is_some() {
        return None;
    }
    Some(from_cluster)
}

fn width(area: &gtk::DrawingArea) -> f64 {
    f64::from(area.allocated_width())
}

fn height(area: &gtk::DrawingArea) -> f64 {
    f64::from(area.allocated_height())
}

/// Reset pan velocity/inertia tracking when a new pan gesture begins.
fn reset_pan_tracking(state: &mut WidgetState) {
    state.pan_velocity = (0.0, 0.0);
    state.prev_drag_dx = 0.0;
    state.prev_drag_dy = 0.0;
    state.inertia_active = false; // kill any running inertia callback
    state.viewport_anim = None; // cancel recenter animation
}

/// Apply a pointer drag delta to the given cluster, computing snap and updating offsets.
/// Returns the snapped world position (sent to the daemon via IPC).
fn apply_cluster_drag(
    state: &mut WidgetState,
    area: &gtk::DrawingArea,
    cluster_id: ClusterId,
    dx: f64,
    dy: f64,
) -> (f64, f64) {
    let scale = state.canvas_state.viewport.scale.max(MIN_SCALE);
    let world_dx = dx / scale;
    let world_dy = dy / scale;

    // Extract base position first (immutable borrow ends when map() returns)
    let base = state
        .canvas_state
        .clusters
        .iter()
        .find(|c| c.id == cluster_id)
        .map(|c| (c.x, c.y));

    let Some((base_x, base_y)) = base else {
        area.queue_draw();
        return (0.0, 0.0);
    };

    let raw_x = (base_x + world_dx).clamp(GLOBAL_CANVAS_MIN, GLOBAL_CANVAS_MAX);
    let raw_y = (base_y + world_dy).clamp(GLOBAL_CANVAS_MIN, GLOBAL_CANVAS_MAX);

    // Snap borrows clusters immutably (base_x/base_y already extracted)
    let (snap_x, snap_y, guides) = compute_snap(
        raw_x,
        raw_y,
        cluster_id,
        scale,
        &state.canvas_state.clusters,
    );

    let entry = state
        .cluster_offsets
        .entry(cluster_id)
        .or_insert((0.0, 0.0));
    entry.0 = snap_x - base_x;
    entry.1 = snap_y - base_y;
    state.snap_guides = guides;

    area.queue_draw();
    (snap_x, snap_y)
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

/// Compute the world-space target for the R-key recenter, returning None if no cluster selected.
fn compute_recenter_target(state: &WidgetState) -> Option<(f64, f64)> {
    let cluster_id = state.selected_cluster?;
    let cluster = state
        .canvas_state
        .clusters
        .iter()
        .find(|c| c.id == cluster_id)?;
    let offset = state
        .cluster_offsets
        .get(&cluster_id)
        .copied()
        .unwrap_or((0.0, 0.0));
    Some((cluster.x + offset.0, cluster.y + offset.1))
}

fn draw_canvas(state: &WidgetState, cr: &gtk::cairo::Context, width: f64, height: f64) {
    cr.set_source_rgb(0.09, 0.10, 0.12);
    let _ = cr.paint();

    // Draw snap guide lines before cards so they appear in the background
    draw_snap_guides(
        cr,
        &state.snap_guides,
        &state.canvas_state.viewport,
        width,
        height,
    );

    let windows_by_id: HashMap<_, _> = state
        .canvas_state
        .windows
        .iter()
        .map(|window| (window.id, window))
        .collect();

    for cluster in &state.canvas_state.clusters {
        draw_cluster_card(state, cr, width, height, cluster, &windows_by_id);
    }

    draw_zoom_pill(cr, &state.canvas_state, &windows_by_id);
}

/// Top-left pill showing the current `ZoomLevel`: `Overview`,
/// `Cluster: <name>`, or `Focus: <title>`. Long titles are truncated.
fn draw_zoom_pill(
    cr: &gtk::cairo::Context,
    canvas_state: &CanvasState,
    windows_by_id: &HashMap<u64, &Window>,
) {
    const MAX_TITLE_CHARS: usize = 48;
    const PILL_MARGIN: f64 = 16.0;
    const PILL_PAD_X: f64 = 14.0;
    const PILL_PAD_Y: f64 = 8.0;
    const PILL_FONT: f64 = 13.0;

    let text = match &canvas_state.zoom {
        ZoomLevel::Overview => "Overview".to_owned(),
        ZoomLevel::Cluster(id) => {
            let name = canvas_state
                .clusters
                .iter()
                .find(|c| c.id == *id)
                .map(|c| c.name.as_str())
                .unwrap_or("?");
            format!("Cluster: {name}")
        }
        ZoomLevel::Focus(window_id) => {
            let (title, app_id) = windows_by_id
                .get(window_id)
                .map(|w| {
                    let title = if w.title.trim().is_empty() {
                        "untitled"
                    } else {
                        w.title.as_str()
                    };
                    (title, w.app_id.as_deref().unwrap_or("unknown"))
                })
                .unwrap_or(("?", "?"));
            let truncated = truncate_display(title, MAX_TITLE_CHARS);
            format!("Focus: {truncated} — {app_id}")
        }
    };

    cr.select_font_face(
        "Sans",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Bold,
    );
    cr.set_font_size(PILL_FONT);
    let extents = match cr.text_extents(&text) {
        Ok(e) => e,
        Err(_) => return,
    };

    let pill_w = extents.width() + PILL_PAD_X * 2.0;
    let pill_h = PILL_FONT + PILL_PAD_Y * 2.0;
    let pill_x = PILL_MARGIN;
    let pill_y = PILL_MARGIN;

    cr.set_source_rgba(0.10, 0.11, 0.13, 0.85);
    cr.rectangle(pill_x, pill_y, pill_w, pill_h);
    let _ = cr.fill();

    cr.set_source_rgba(0.35, 0.62, 1.0, 0.55);
    cr.set_line_width(1.0);
    cr.rectangle(pill_x, pill_y, pill_w, pill_h);
    let _ = cr.stroke();

    cr.set_source_rgb(0.92, 0.94, 0.97);
    cr.move_to(pill_x + PILL_PAD_X, pill_y + PILL_PAD_Y + PILL_FONT - 3.0);
    let _ = cr.show_text(&text);
}

fn truncate_display(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
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

/// Draw faint guide lines across the viewport for each active snap constraint.
fn draw_snap_guides(
    cr: &gtk::cairo::Context,
    guides: &[SnapGuide],
    viewport: &Viewport,
    width: f64,
    height: f64,
) {
    if guides.is_empty() {
        return;
    }
    cr.set_source_rgba(0.45, 0.72, 1.0, 0.30);
    cr.set_line_width(1.0);
    let scale = viewport.scale.max(MIN_SCALE);
    for guide in guides {
        if guide.vertical {
            let sx = (guide.coord - viewport.x) * scale + width / 2.0;
            cr.move_to(sx, 0.0);
            cr.line_to(sx, height);
        } else {
            let sy = (guide.coord - viewport.y) * scale + height / 2.0;
            cr.move_to(0.0, sy);
            cr.line_to(width, sy);
        }
        let _ = cr.stroke();
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

/// Compute the nearest snap position for a cluster being dragged to (raw_x, raw_y).
///
/// Snap targets (checked independently per axis):
/// - Grid lines every SNAP_GRID_PX world units
/// - Output center (0, 0)
/// - Other cluster center positions
///
/// A guide line is emitted for each axis that snapped.
fn compute_snap(
    raw_x: f64,
    raw_y: f64,
    dragging_id: ClusterId,
    scale: f64,
    clusters: &[Cluster],
) -> (f64, f64, Vec<SnapGuide>) {
    let threshold = SNAP_THRESHOLD_SCREEN / scale.max(MIN_SCALE);

    // Collect candidates for each axis
    let mut x_cands = vec![
        0.0_f64,                                       // output center
        (raw_x / SNAP_GRID_PX).round() * SNAP_GRID_PX, // nearest grid line
    ];
    let mut y_cands = vec![0.0_f64, (raw_y / SNAP_GRID_PX).round() * SNAP_GRID_PX];
    for c in clusters {
        if c.id != dragging_id {
            x_cands.push(c.x);
            y_cands.push(c.y);
        }
    }

    let nearest_within = |raw: f64, cands: &[f64]| -> f64 {
        cands
            .iter()
            .copied()
            .filter(|&c| (raw - c).abs() <= threshold)
            .min_by(|&a, &b| {
                (raw - a)
                    .abs()
                    .partial_cmp(&(raw - b).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(raw)
    };

    let snap_x = nearest_within(raw_x, &x_cands);
    let snap_y = nearest_within(raw_y, &y_cands);

    let mut guides = Vec::new();
    if (snap_x - raw_x).abs() > 1e-6 {
        guides.push(SnapGuide {
            vertical: true,
            coord: snap_x,
        });
    }
    if (snap_y - raw_y).abs() > 1e-6 {
        guides.push(SnapGuide {
            vertical: false,
            coord: snap_y,
        });
    }

    (snap_x, snap_y, guides)
}

fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

/// Start a smooth animated pan to (target_x, target_y) in world space.
/// Any previous animation is superseded via a generation counter.
fn start_recenter_anim(
    area: &gtk::DrawingArea,
    data: Rc<RefCell<WidgetState>>,
    target_x: f64,
    target_y: f64,
) {
    let current_scale = data.borrow().canvas_state.viewport.scale;
    start_viewport_anim(
        area,
        data,
        target_x,
        target_y,
        current_scale,
        RECENTER_DURATION_MS,
        None,
    );
}

/// Symmetric exit of `start_dive_anim`: when overlay observes a
/// `Cluster(c) → Overview` transition (W1c-25-3), seed the viewport at the
/// cluster's dived-in pose and animate back out to the previous overview
/// pose. This makes the user's exit feel like a continuous zoom-out rather
/// than a hard cut from a tiled cluster to the empty canvas.
///
/// We don't know the *exact* pre-dive viewport (overlay state can be lost
/// across overlay restarts), so we target the daemon-acknowledged viewport
/// — that's whatever the user last panned/zoomed to in Overview. If the
/// cluster doesn't exist anymore (closed mid-flight) we skip the animation.
fn start_undive_anim(
    area: &gtk::DrawingArea,
    data: Rc<RefCell<WidgetState>>,
    cluster_id: ClusterId,
) {
    let seed = {
        let state = data.borrow();
        state
            .canvas_state
            .clusters
            .iter()
            .find(|c| c.id == cluster_id)
            .map(|c| {
                let scale =
                    (state.daemon_viewport.scale * DIVE_ZOOM_GAIN).clamp(MIN_SCALE, MAX_SCALE);
                (c.x, c.y, scale)
            })
    };
    let Some((seed_x, seed_y, seed_scale)) = seed else {
        return;
    };
    // Snap viewport to the dived pose first, so the animation visually
    // departs from where the cluster appeared in fullscreen, not from
    // wherever the daemon-overview viewport happened to be.
    {
        let mut state = data.borrow_mut();
        state.canvas_state.viewport.x = seed_x;
        state.canvas_state.viewport.y = seed_y;
        state.canvas_state.viewport.scale = seed_scale;
        state.has_local_viewport = true;
    }
    let (target_x, target_y, target_scale) = {
        let state = data.borrow();
        (
            state.daemon_viewport.x,
            state.daemon_viewport.y,
            state.daemon_viewport.scale,
        )
    };
    start_viewport_anim(
        area,
        data,
        target_x,
        target_y,
        target_scale,
        DIVE_DURATION_MS,
        None,
    );
}

/// Start a cluster-dive animation: ease viewport toward the cluster's center
/// and zoom in by `DIVE_ZOOM_GAIN`, then invoke `on_dive` to flip zoom level.
/// If the cluster vanishes between trigger and tick (Sway closed it), the
/// animation aborts and `on_dive` does not fire.
fn start_dive_anim(
    area: &gtk::DrawingArea,
    data: Rc<RefCell<WidgetState>>,
    cluster_id: ClusterId,
    on_dive: Rc<dyn Fn(ClusterId)>,
) {
    let target = {
        let state = data.borrow();
        state
            .canvas_state
            .clusters
            .iter()
            .find(|c| c.id == cluster_id)
            .map(|c| {
                (
                    c.x,
                    c.y,
                    (state.canvas_state.viewport.scale * DIVE_ZOOM_GAIN)
                        .clamp(MIN_SCALE, MAX_SCALE),
                )
            })
    };
    let Some((tx, ty, target_scale)) = target else {
        return;
    };

    let on_complete: Box<dyn FnOnce()> = Box::new(move || on_dive(cluster_id));
    start_viewport_anim(
        area,
        data,
        tx,
        ty,
        target_scale,
        DIVE_DURATION_MS,
        Some(on_complete),
    );
}

fn start_viewport_anim(
    area: &gtk::DrawingArea,
    data: Rc<RefCell<WidgetState>>,
    target_x: f64,
    target_y: f64,
    target_scale: f64,
    duration_ms: f64,
    on_complete: Option<Box<dyn FnOnce()>>,
) {
    let generation = {
        let mut state = data.borrow_mut();
        let gen = state.viewport_anim.as_ref().map_or(1, |a| a.generation + 1);
        let start_scale = state.canvas_state.viewport.scale;
        state.viewport_anim = Some(ViewportAnim {
            start_x: state.canvas_state.viewport.x,
            start_y: state.canvas_state.viewport.y,
            target_x,
            target_y,
            start_scale,
            target_scale,
            start: Instant::now(),
            duration_ms,
            generation: gen,
            on_complete,
        });
        state.has_local_viewport = true;
        gen
    };

    let area = area.clone();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let tick = {
            let state = data.borrow();
            if state.viewport_anim.as_ref().map(|a| a.generation) != Some(generation) {
                return glib::ControlFlow::Break;
            }
            let anim = state.viewport_anim.as_ref().unwrap();
            let elapsed_ms = anim.start.elapsed().as_secs_f64() * 1000.0;
            let t = (elapsed_ms / anim.duration_ms).min(1.0);
            let te = ease_out_cubic(t);
            (
                anim.start_x + (anim.target_x - anim.start_x) * te,
                anim.start_y + (anim.target_y - anim.start_y) * te,
                anim.start_scale + (anim.target_scale - anim.start_scale) * te,
                t >= 1.0,
            )
        };
        let (new_x, new_y, new_scale, done) = tick;

        let mut state = data.borrow_mut();
        state.canvas_state.viewport.x = new_x;
        state.canvas_state.viewport.y = new_y;
        state.canvas_state.viewport.scale = new_scale;
        area.queue_draw();

        if done {
            // Take the anim out so on_complete fires exactly once even if the
            // callback re-enters this module (e.g. start_dive_anim → on_dive →
            // overlay hide → next session re-uses data).
            let completed = state.viewport_anim.take();
            let dx = new_x - state.daemon_viewport.x;
            let dy = new_y - state.daemon_viewport.y;
            if dx.abs() > 0.5 || dy.abs() > 0.5 {
                dispatch_ipc_mutation_detached(IpcMutation::OverviewPan {
                    dx,
                    dy,
                    output: state.output_name.clone(),
                });
                state.daemon_viewport.x = new_x;
                state.daemon_viewport.y = new_y;
            }
            drop(state);
            if let Some(mut anim) = completed {
                if let Some(cb) = anim.on_complete.take() {
                    cb();
                }
            }
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

/// Launch the inertial panning loop after a pan gesture ends.
/// Uses `inertia_active` as a guard; the loop stops when velocity drops below threshold.
fn start_inertia(area: &gtk::DrawingArea, data: Rc<RefCell<WidgetState>>) {
    let area = area.clone();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let mut state = data.borrow_mut();

        if !state.inertia_active {
            return glib::ControlFlow::Break;
        }

        let (vx, vy) = state.pan_velocity;
        let speed = (vx * vx + vy * vy).sqrt();

        if speed < INERTIA_MIN_PX {
            state.pan_velocity = (0.0, 0.0);
            state.inertia_active = false;
            let vp_x = state.canvas_state.viewport.x;
            let vp_y = state.canvas_state.viewport.y;
            let dx = vp_x - state.daemon_viewport.x;
            let dy = vp_y - state.daemon_viewport.y;
            if dx.abs() > 0.5 || dy.abs() > 0.5 {
                dispatch_ipc_mutation_detached(IpcMutation::OverviewPan {
                    dx,
                    dy,
                    output: state.output_name.clone(),
                });
                state.daemon_viewport.x = vp_x;
                state.daemon_viewport.y = vp_y;
            }
            area.queue_draw();
            return glib::ControlFlow::Break;
        }

        let scale = state.canvas_state.viewport.scale.max(MIN_SCALE);
        // Inertia continues panning in the direction of the last gesture
        state.canvas_state.viewport.x -= vx / scale;
        state.canvas_state.viewport.y -= vy / scale;
        state.pan_velocity = (vx * INERTIA_FRICTION, vy * INERTIA_FRICTION);
        state.has_local_viewport = true;
        area.queue_draw();
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_display_leaves_short_strings_untouched() {
        assert_eq!(truncate_display("hi", 10), "hi");
        assert_eq!(truncate_display("", 10), "");
    }

    #[test]
    fn truncate_display_replaces_tail_with_ellipsis() {
        let out = truncate_display("abcdefghijklmno", 6);
        assert_eq!(out, "abcde…");
        // max_chars includes the ellipsis, so the final string is exactly max.
        assert_eq!(out.chars().count(), 6);
    }

    #[test]
    fn truncate_display_counts_characters_not_bytes() {
        // Unicode scalar count, not byte length — "é" is 2 bytes but 1 char.
        let input = "ééééééééé"; // 9 chars, 18 bytes
        let out = truncate_display(input, 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_display_max_one_avoids_panic() {
        // max_chars = 1 would call `take(0)` — must not panic, just ellipsis.
        let out = truncate_display("abcdef", 1);
        assert_eq!(out, "…");
    }
}
