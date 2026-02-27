use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use common::contracts::{CanvasState, Cluster, ClusterId, Window};
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

const CARD_WIDTH: f64 = 320.0;
const CARD_HEIGHT: f64 = 140.0;
const DRAG_THRESHOLD_PX: f64 = 8.0;
const SCROLL_ZOOM_STEP: f64 = 1.10;
const MIN_SCALE: f64 = 0.2;
const MAX_SCALE: f64 = 6.0;
const KEY_PAN_STEP: f64 = 48.0;

pub struct OverviewCanvas {
    area: gtk::DrawingArea,
    data: Rc<RefCell<WidgetState>>,
}

#[derive(Clone)]
enum DragMode {
    PendingCluster { cluster_id: ClusterId },
    DraggingCluster { cluster_id: ClusterId },
    Panning { viewport_start: (f64, f64) },
}

struct WidgetState {
    canvas_state: CanvasState,
    selected_cluster: Option<ClusterId>,
    cluster_offsets: HashMap<ClusterId, (f64, f64)>,
    drag_mode: Option<DragMode>,
}

impl OverviewCanvas {
    pub fn new(on_activate: Rc<dyn Fn(ClusterId)>, on_dive: Rc<dyn Fn(ClusterId)>) -> Self {
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
            let data = Rc::clone(&data);
            let on_dive = Rc::clone(&on_dive);
            move |gesture, n_press, x, y| {
                let modifiers = gesture.current_event_state();
                let mut state = data.borrow_mut();
                let hit = hit_test(&state, width(&area), height(&area), x, y);

                if n_press >= 2 {
                    if let Some(cluster_id) = state.selected_cluster {
                        on_dive(cluster_id);
                    }
                    return;
                }

                if let Some(cluster_id) = hit {
                    state.selected_cluster = Some(cluster_id);
                    if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                        state.drag_mode = Some(DragMode::Panning {
                            viewport_start: (
                                state.canvas_state.viewport.x,
                                state.canvas_state.viewport.y,
                            ),
                        });
                    } else {
                        state.drag_mode = Some(DragMode::PendingCluster { cluster_id });
                    }
                } else {
                    state.selected_cluster = None;
                    state.drag_mode = Some(DragMode::Panning {
                        viewport_start: (
                            state.canvas_state.viewport.x,
                            state.canvas_state.viewport.y,
                        ),
                    });
                }

                area.grab_focus();
                area.queue_draw();
            }
        });
        click.connect_released({
            let data = Rc::clone(&data);
            move |_, _, _, _| {
                data.borrow_mut().drag_mode = None;
            }
        });
        area.add_controller(click);

        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        drag.connect_drag_update({
            let area = area.clone();
            let data = Rc::clone(&data);
            move |_, dx, dy| {
                let mut state = data.borrow_mut();
                let Some(mode) = state.drag_mode.clone() else {
                    return;
                };

                match mode {
                    DragMode::PendingCluster { cluster_id } => {
                        let distance = (dx * dx + dy * dy).sqrt();
                        if distance < DRAG_THRESHOLD_PX {
                            return;
                        }
                        state.drag_mode = Some(DragMode::DraggingCluster { cluster_id });
                        apply_cluster_drag(&mut state, &area, cluster_id, dx, dy);
                    }
                    DragMode::DraggingCluster { cluster_id } => {
                        apply_cluster_drag(&mut state, &area, cluster_id, dx, dy);
                    }
                    DragMode::Panning { viewport_start } => {
                        let scale = state.canvas_state.viewport.scale.max(MIN_SCALE);
                        state.canvas_state.viewport.x = viewport_start.0 - (dx / scale);
                        state.canvas_state.viewport.y = viewport_start.1 - (dy / scale);
                        area.queue_draw();
                    }
                }
            }
        });
        drag.connect_drag_end({
            let data = Rc::clone(&data);
            move |_, _, _| {
                data.borrow_mut().drag_mode = None;
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
                area.queue_draw();
                glib::Propagation::Stop
            }
        });
        area.add_controller(scroll);

        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed({
            let area = area.clone();
            let data = Rc::clone(&data);
            let on_activate = Rc::clone(&on_activate);
            move |_, keyval, _, _| {
                let mut state = data.borrow_mut();
                let viewport = &mut state.canvas_state.viewport;
                match keyval {
                    gdk::Key::plus | gdk::Key::equal => {
                        viewport.scale =
                            (viewport.scale * SCROLL_ZOOM_STEP).clamp(MIN_SCALE, MAX_SCALE);
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::minus | gdk::Key::underscore => {
                        viewport.scale =
                            (viewport.scale / SCROLL_ZOOM_STEP).clamp(MIN_SCALE, MAX_SCALE);
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::Up => {
                        viewport.y -= KEY_PAN_STEP / viewport.scale.max(MIN_SCALE);
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::Down => {
                        viewport.y += KEY_PAN_STEP / viewport.scale.max(MIN_SCALE);
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::Left => {
                        viewport.x -= KEY_PAN_STEP / viewport.scale.max(MIN_SCALE);
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::Right => {
                        viewport.x += KEY_PAN_STEP / viewport.scale.max(MIN_SCALE);
                        area.queue_draw();
                        glib::Propagation::Stop
                    }
                    gdk::Key::Return => {
                        if let Some(cluster_id) = state.selected_cluster {
                            on_activate(cluster_id);
                        }
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            }
        });
        area.add_controller(key);

        Self { area, data }
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    pub fn set_canvas_state(&self, state: CanvasState) {
        let mut data = self.data.borrow_mut();
        data.canvas_state = state;

        if !data
            .canvas_state
            .clusters
            .iter()
            .any(|cluster| Some(cluster.id) == data.selected_cluster)
        {
            data.selected_cluster = None;
        }

        data.cluster_offsets.retain(|cluster_id, _| {
            data.canvas_state
                .clusters
                .iter()
                .any(|c| c.id == *cluster_id)
        });

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
    entry.0 = world_dx;
    entry.1 = world_dy;

    area.queue_draw();
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
