use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use adw::prelude::*;
use config::{Config, LauncherConfig};
use gtk::gdk;
use gtk::glib;
use gtk4 as gtk;
use gtk4_layer_shell::{self as layer_shell, LayerShell};
use xdg::DesktopEntry;

fn report_config_load_error(error: &config::ConfigLoadError) {
    tracing::warn!(%error, "failed to load config, using defaults");
    if let Some(issues) = error.validation_issues() {
        for issue in issues {
            tracing::warn!(field = %issue.field, message = %issue.message, "config validation issue");
        }
    }
}
const MATCH_EXACT_PREFIX: i32 = 4;
const MATCH_WORD_PREFIX: i32 = 3;
const MATCH_SUBSTRING: i32 = 2;
const MATCH_KEYWORD: i32 = 1;

#[derive(Debug, Clone, Copy)]
enum LaunchMode {
    Default,
    ForceTerminal,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct UsageStats {
    launches: HashMap<String, u64>,
    last_launched_unix: HashMap<String, u64>,
}

impl UsageStats {
    fn launch_count(&self, id: &str) -> u64 {
        self.launches.get(id).copied().unwrap_or(0)
    }

    fn last_launched(&self, id: &str) -> u64 {
        self.last_launched_unix.get(id).copied().unwrap_or(0)
    }

    fn record_launch(&mut self, id: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        *self.launches.entry(id.to_owned()).or_insert(0) += 1;
        self.last_launched_unix.insert(id.to_owned(), now);
    }
}

#[derive(Clone)]
struct ScoredEntry {
    score: i32,
    entry: DesktopEntry,
}

#[derive(Clone)]
enum SearchResult {
    App(ScoredEntry),
    Window {
        score: i32,
        window_id: u64,
        title: String,
        app_id: String,
        cluster_name: String,
    },
    Cluster {
        score: i32,
        cluster_id: u64,
        name: String,
        window_count: usize,
    },
}

/// Which category of result a row belongs to. Drives section headers in the
/// launcher list and the Tab-cycled filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultCategory {
    App,
    Window,
    Cluster,
}

impl ResultCategory {
    fn header_label(self) -> &'static str {
        match self {
            Self::App => "Apps",
            Self::Window => "Open Windows",
            Self::Cluster => "Clusters",
        }
    }

    /// Fixed display order of the sections: apps first (most familiar),
    /// then open windows (most common action), then clusters (rarer).
    fn display_order(self) -> u8 {
        match self {
            Self::App => 0,
            Self::Window => 1,
            Self::Cluster => 2,
        }
    }
}

/// Tab-cycled category filter. `All` is the default; the other variants
/// hide everything outside the chosen category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultFilter {
    All,
    Only(ResultCategory),
}

impl ResultFilter {
    fn next(self) -> Self {
        match self {
            Self::All => Self::Only(ResultCategory::App),
            Self::Only(ResultCategory::App) => Self::Only(ResultCategory::Window),
            Self::Only(ResultCategory::Window) => Self::Only(ResultCategory::Cluster),
            Self::Only(ResultCategory::Cluster) => Self::All,
        }
    }

    fn includes(self, category: ResultCategory) -> bool {
        match self {
            Self::All => true,
            Self::Only(cat) => cat == category,
        }
    }

    fn placeholder(self) -> String {
        match self {
            Self::All => "Launch, switch, or activate…".to_owned(),
            Self::Only(cat) => format!("Filter: {} only (Tab to cycle)", cat.header_label()),
        }
    }
}

impl SearchResult {
    fn score(&self) -> i32 {
        match self {
            Self::App(e) => e.score,
            Self::Window { score, .. } => *score,
            Self::Cluster { score, .. } => *score,
        }
    }

    fn category(&self) -> ResultCategory {
        match self {
            Self::App(_) => ResultCategory::App,
            Self::Window { .. } => ResultCategory::Window,
            Self::Cluster { .. } => ResultCategory::Cluster,
        }
    }

    fn display_title(&self) -> String {
        match self {
            Self::App(e) => e.entry.name.clone(),
            Self::Window {
                title,
                app_id,
                cluster_name,
                ..
            } => format!("[W] {title} ({app_id}) — {cluster_name}"),
            Self::Cluster {
                name, window_count, ..
            } => format!("[C] {name} ({window_count} windows)"),
        }
    }

    fn display_subtitle(&self) -> String {
        match self {
            Self::App(e) => {
                if e.entry.keywords.is_empty() {
                    e.entry.exec.clone()
                } else {
                    format!("{} · {}", e.entry.exec, e.entry.keywords.join(", "))
                }
            }
            Self::Window { app_id, .. } => format!("Focus window · {app_id}"),
            Self::Cluster { name, .. } => format!("Activate cluster · {name}"),
        }
    }

    fn icon_name(&self) -> Option<&str> {
        match self {
            Self::App(e) => e.entry.icon.as_deref(),
            Self::Window { .. } => Some("preferences-system-windows-symbolic"),
            Self::Cluster { .. } => Some("view-grid-symbolic"),
        }
    }
}

fn fetch_canvas_state() -> Option<common::contracts::CanvasState> {
    let output = Command::new("vibeshellctl")
        .args(["ipc", "get-state"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let response: common::contracts::IpcResponse = serde_json::from_slice(&output.stdout).ok()?;
    match response {
        common::contracts::IpcResponse::State(state) => Some(state),
        _ => None,
    }
}

/// Build a list of recent `SearchResult::Window` entries from the canvas
/// state, for the empty-query default view. Ordering:
///
/// 1. If zoomed into a cluster, that cluster's `recency` comes first.
/// 2. Then each other cluster's `recency`, in `canvas.clusters` order.
/// 3. Duplicate window ids are dropped on second encounter.
///
/// Windows are scored high enough to outrank apps so the launcher opens
/// pre-selected on the most-recently-focused window — effectively turning
/// the launcher into a window switcher when invoked without typing.
fn recent_windows(
    canvas: &common::contracts::CanvasState,
    max_results: usize,
) -> Vec<SearchResult> {
    if max_results == 0 {
        return Vec::new();
    }

    // Score floor above anything apps can score via usage ranking.
    const RECENT_BASE_SCORE: i32 = 1_000_000;

    let active_cluster_id = match canvas.zoom {
        common::contracts::ZoomLevel::Cluster(id) => Some(id),
        common::contracts::ZoomLevel::Focus(window_id) => canvas
            .windows
            .iter()
            .find(|w| w.id == window_id)
            .and_then(|w| w.cluster_id),
        common::contracts::ZoomLevel::Overview => None,
    };

    let cluster_names: std::collections::HashMap<u64, &str> = canvas
        .clusters
        .iter()
        .map(|c| (c.id, c.name.as_str()))
        .collect();
    let windows_by_id: std::collections::HashMap<u64, &common::contracts::Window> =
        canvas.windows.iter().map(|w| (w.id, w)).collect();

    // Order clusters: active cluster first (if any), then the rest in list order.
    let mut ordered_clusters: Vec<&common::contracts::Cluster> = Vec::new();
    if let Some(active) = active_cluster_id {
        if let Some(cluster) = canvas.clusters.iter().find(|c| c.id == active) {
            ordered_clusters.push(cluster);
        }
    }
    for cluster in &canvas.clusters {
        if Some(cluster.id) != active_cluster_id {
            ordered_clusters.push(cluster);
        }
    }

    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut results = Vec::new();

    for cluster in ordered_clusters {
        for window_id in &cluster.recency {
            if !seen.insert(*window_id) {
                continue;
            }
            let Some(window) = windows_by_id.get(window_id) else {
                continue;
            };
            let cluster_name = window
                .cluster_id
                .and_then(|id| cluster_names.get(&id).copied())
                .unwrap_or("unassigned")
                .to_owned();
            let score = RECENT_BASE_SCORE - results.len() as i32;
            results.push(SearchResult::Window {
                score,
                window_id: *window_id,
                title: window.title.clone(),
                app_id: window.app_id.clone().unwrap_or_default(),
                cluster_name,
            });
            if results.len() >= max_results {
                return results;
            }
        }
    }

    results
}

fn search_windows_and_clusters(
    canvas: &common::contracts::CanvasState,
    query: &str,
    max_results: usize,
) -> Vec<SearchResult> {
    if query.is_empty() {
        return Vec::new();
    }

    let cluster_names: std::collections::HashMap<u64, &str> = canvas
        .clusters
        .iter()
        .map(|c| (c.id, c.name.as_str()))
        .collect();

    let mut results = Vec::new();

    // Score windows
    for window in &canvas.windows {
        let title_lower = window.title.to_lowercase();
        let app_id_lower = window.app_id.as_deref().unwrap_or_default().to_lowercase();

        let score = if app_id_lower.starts_with(query) {
            MATCH_EXACT_PREFIX * 10_000
        } else if title_lower.contains(query) {
            MATCH_SUBSTRING * 10_000
        } else if app_id_lower.contains(query) {
            MATCH_KEYWORD * 10_000
        } else {
            continue;
        };

        let cluster_name = window
            .cluster_id
            .and_then(|id| cluster_names.get(&id).copied())
            .unwrap_or("unassigned")
            .to_owned();

        results.push(SearchResult::Window {
            score,
            window_id: window.id,
            title: window.title.clone(),
            app_id: window.app_id.clone().unwrap_or_default(),
            cluster_name,
        });
    }

    // Score clusters
    for cluster in &canvas.clusters {
        let name_lower = cluster.name.to_lowercase();
        let score = if name_lower.starts_with(query) {
            MATCH_EXACT_PREFIX * 10_000
        } else if name_lower.contains(query) {
            MATCH_SUBSTRING * 10_000
        } else {
            continue;
        };

        results.push(SearchResult::Cluster {
            score,
            cluster_id: cluster.id,
            name: cluster.name.clone(),
            window_count: cluster.windows.len(),
        });
    }

    results.sort_by_key(|r| std::cmp::Reverse(r.score()));
    results.truncate(max_results);
    results
}

#[derive(Clone)]
struct RuntimeLauncherConfig {
    max_results: usize,
    terminal_command: String,
}

fn main() {
    common::init_logging("launcher");
    tracing::info!(app = "launcher", "starting up");

    let launcher_config = Config::load()
        .unwrap_or_else(|error| {
            report_config_load_error(&error);
            Config::default()
        })
        .launcher;

    let apps = xdg::discover_applications().unwrap_or_else(|error| {
        tracing::warn!(?error, "failed to read desktop entries");
        Vec::new()
    });

    let app = adw::Application::builder()
        .application_id("com.vibeshell.launcher")
        .build();

    app.connect_activate(move |app| {
        let (_, reload_rx) = common::spawn_reload_listener();
        build_ui(app, apps.clone(), launcher_config.clone(), reload_rx)
    });
    app.run();
}

fn build_ui(
    app: &adw::Application,
    apps: Vec<DesktopEntry>,
    launcher_config: LauncherConfig,
    reload_rx: std::sync::mpsc::Receiver<common::ReloadReason>,
) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("vibeshell-launcher")
        .default_width(launcher_config.window_width)
        .default_height(launcher_config.window_height)
        .build();

    window.set_resizable(false);

    let runtime_config = Arc::new(Mutex::new(RuntimeLauncherConfig {
        max_results: launcher_config.max_results,
        terminal_command: launcher_config.terminal_command.clone(),
    }));

    if layer_shell::is_supported() {
        window.set_decorated(false);
        window.init_layer_shell();
        window.set_layer(layer_shell::Layer::Overlay);
        window.set_keyboard_mode(layer_shell::KeyboardMode::Exclusive);
        window.set_anchor(layer_shell::Edge::Top, true);
        window.set_anchor(layer_shell::Edge::Bottom, true);
        window.set_anchor(layer_shell::Edge::Left, true);
        window.set_anchor(layer_shell::Edge::Right, true);
    } else {
        tracing::warn!("layer shell protocol unavailable; falling back to a regular GTK window");
        eprintln!(
            "launcher: compositor does not support zwlr_layer_shell_v1; using regular window mode."
        );
    }

    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(20)
        .margin_end(20)
        .build();

    let input = gtk::Entry::builder()
        .placeholder_text(ResultFilter::All.placeholder())
        .build();

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::Single);

    panel.append(&input);
    panel.append(&list);

    let container = gtk::CenterBox::new();
    container.set_center_widget(Some(&panel));
    window.set_content(Some(&container));

    let state = Rc::new(RefCell::new(Vec::<SearchResult>::new()));
    let usage_path = usage_stats_path();
    let usage_stats = Rc::new(RefCell::new(load_usage_stats(&usage_path)));
    let canvas_state: Rc<Option<common::contracts::CanvasState>> = Rc::new(fetch_canvas_state());
    let filter = Rc::new(RefCell::new(ResultFilter::All));
    let last_query = Rc::new(RefCell::new(String::new()));

    // Section headers: attach a bold label above the first row of each new
    // category. GtkListBox's header func doesn't produce selectable rows, so
    // arrow navigation steps naturally over the groups.
    {
        let state_for_header = Rc::clone(&state);
        list.set_header_func(move |row, before| {
            let state = state_for_header.borrow();
            let row_idx = row.index() as usize;
            let current = state.get(row_idx).map(|r| r.category());
            let before_cat = before
                .map(|r| r.index() as usize)
                .and_then(|idx| state.get(idx).map(|r| r.category()));

            if current != before_cat {
                if let Some(cat) = current {
                    let label = gtk::Label::builder()
                        .label(cat.header_label())
                        .xalign(0.0)
                        .margin_top(8)
                        .margin_bottom(4)
                        .margin_start(8)
                        .build();
                    label.add_css_class("dim-label");
                    label.add_css_class("heading");
                    row.set_header(Some(&label));
                } else {
                    row.set_header(None::<&gtk::Widget>);
                }
            } else {
                row.set_header(None::<&gtk::Widget>);
            }
        });
    }

    let rebuild_results = {
        let apps = apps.clone();
        let list = list.clone();
        let state = Rc::clone(&state);
        let usage_stats = Rc::clone(&usage_stats);
        let runtime_config = Arc::clone(&runtime_config);
        let canvas_state = Rc::clone(&canvas_state);
        let filter = Rc::clone(&filter);
        Rc::new(move |query: &str| {
            let max_results = runtime_config
                .lock()
                .expect("runtime config poisoned")
                .max_results;
            let merged = compose_launcher_results(
                &apps,
                query,
                max_results,
                &usage_stats.borrow(),
                canvas_state.as_ref().as_ref(),
                *filter.borrow(),
            );
            populate_search_results(&list, &state, &merged);
        })
    };

    {
        let rebuild_results = Rc::clone(&rebuild_results);
        let last_query = Rc::clone(&last_query);
        input.connect_changed(move |entry| {
            let query = entry.text().to_string();
            *last_query.borrow_mut() = query.clone();
            rebuild_results(&query);
        });
    }

    {
        let window = window.clone();
        let app = app.clone();
        let list = list.clone();
        let state = state.clone();
        let usage_stats = usage_stats.clone();
        let usage_path = usage_path.clone();
        let runtime_config = Arc::clone(&runtime_config);
        input.connect_activate(move |_| {
            let index = selected_index(&list).unwrap_or(0);
            if activate_search_result(
                &state.borrow(),
                index,
                &runtime_config
                    .lock()
                    .expect("runtime config poisoned")
                    .terminal_command,
                LaunchMode::Default,
                &usage_stats,
                &usage_path,
            )
            .is_ok()
            {
                close_launcher(&window, &app);
            }
        });
    }

    {
        let window = window.clone();
        let app = app.clone();
        let list = list.clone();
        let state = state.clone();
        let usage_stats = usage_stats.clone();
        let usage_path = usage_path.clone();
        let runtime_config = Arc::clone(&runtime_config);
        let filter_for_keys = Rc::clone(&filter);
        let rebuild_for_keys = Rc::clone(&rebuild_results);
        let input_for_keys = input.clone();
        let last_query_for_keys = Rc::clone(&last_query);
        let key_controller = gtk::EventControllerKey::new();
        let controlled_window = window.clone();
        key_controller.connect_key_pressed(move |_, key, _, modifiers| match key {
            gdk::Key::Escape => {
                close_launcher(&controlled_window, &app);
                glib::Propagation::Stop
            }
            gdk::Key::Down => {
                move_selection(&list, 1);
                glib::Propagation::Stop
            }
            gdk::Key::Up => {
                move_selection(&list, -1);
                glib::Propagation::Stop
            }
            gdk::Key::Tab | gdk::Key::ISO_Left_Tab => {
                // Cycle the category filter. Shift+Tab cycles backwards by
                // applying next() three times (four states total).
                let mut current = *filter_for_keys.borrow();
                current = if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                    current.next().next().next()
                } else {
                    current.next()
                };
                *filter_for_keys.borrow_mut() = current;
                input_for_keys.set_placeholder_text(Some(&current.placeholder()));
                let query = last_query_for_keys.borrow().clone();
                rebuild_for_keys(&query);
                glib::Propagation::Stop
            }
            gdk::Key::Return => {
                let index = selected_index(&list).unwrap_or(0);
                let mode = if modifiers
                    .intersects(gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::CONTROL_MASK)
                {
                    LaunchMode::ForceTerminal
                } else {
                    LaunchMode::Default
                };
                if activate_search_result(
                    &state.borrow(),
                    index,
                    &runtime_config
                        .lock()
                        .expect("runtime config poisoned")
                        .terminal_command,
                    mode,
                    &usage_stats,
                    &usage_path,
                )
                .is_ok()
                {
                    close_launcher(&controlled_window, &app);
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        window.add_controller(key_controller);
    }

    {
        let window = window.clone();
        let app = app.clone();
        let state = state.clone();
        let usage_stats = usage_stats.clone();
        let usage_path = usage_path.clone();
        let runtime_config = Arc::clone(&runtime_config);
        list.connect_row_activated(move |_, row| {
            let index = row.index() as usize;
            if activate_search_result(
                &state.borrow(),
                index,
                &runtime_config
                    .lock()
                    .expect("runtime config poisoned")
                    .terminal_command,
                LaunchMode::Default,
                &usage_stats,
                &usage_path,
            )
            .is_ok()
            {
                close_launcher(&window, &app);
            }
        });
    }

    // Initial population — share the same rebuild path the input-changed and
    // Tab handlers use, so category ordering and headers come through.
    rebuild_results("");

    window.present();
    input.grab_focus();

    glib::timeout_add_local(Duration::from_millis(200), move || {
        while let Ok(reason) = reload_rx.try_recv() {
            match Config::load() {
                Ok(config) => {
                    let new_cfg = config.launcher;
                    let mut applied = Vec::new();
                    let mut restart_required = Vec::new();

                    let mut rt = runtime_config.lock().expect("runtime config poisoned");
                    if rt.max_results != new_cfg.max_results {
                        applied.push(format!(
                            "max_results: {} -> {}",
                            rt.max_results, new_cfg.max_results
                        ));
                        rt.max_results = new_cfg.max_results;
                    }
                    if rt.terminal_command != new_cfg.terminal_command {
                        applied.push("terminal_command updated".to_owned());
                        rt.terminal_command = new_cfg.terminal_command.clone();
                    }

                    if launcher_config.window_width != new_cfg.window_width {
                        applied.push(format!(
                            "window_width: {} -> {}",
                            launcher_config.window_width, new_cfg.window_width
                        ));
                        window.set_default_width(new_cfg.window_width);
                    }
                    if launcher_config.window_height != new_cfg.window_height {
                        restart_required.push("window_height".to_owned());
                    }

                    let applied_text = if applied.is_empty() {
                        "none".to_owned()
                    } else {
                        applied.join(", ")
                    };
                    let restart_required_text = if restart_required.is_empty() {
                        "none".to_owned()
                    } else {
                        restart_required.join(", ")
                    };

                    tracing::info!(
                        trigger = reason.as_str(),
                        applied = applied_text,
                        restart_required = restart_required_text,
                        "launcher config reload processed"
                    );
                }
                Err(error) => tracing::warn!(
                    ?error,
                    trigger = reason.as_str(),
                    "launcher reload ignored due to config load error"
                ),
            }
        }

        glib::ControlFlow::Continue
    });
}

fn rank_entries(
    entries: &[DesktopEntry],
    query: &str,
    max_results: usize,
    usage: &UsageStats,
) -> Vec<ScoredEntry> {
    let normalized_query = query.trim().to_lowercase();

    let mut ranked: Vec<_> = entries
        .iter()
        .filter_map(|entry| {
            let score = score_entry(entry, &normalized_query, usage)?;
            Some(ScoredEntry {
                score,
                entry: entry.clone(),
            })
        })
        .collect();

    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.entry.name.cmp(&b.entry.name))
    });
    ranked.truncate(max_results);
    ranked
}

fn score_entry(entry: &DesktopEntry, query: &str, usage: &UsageStats) -> Option<i32> {
    if query.is_empty() {
        return Some(usage_boost(entry, usage));
    }

    let name = entry.name.to_lowercase();
    let exec = entry.exec.to_lowercase();
    let keywords: Vec<String> = entry.keywords.iter().map(|k| k.to_lowercase()).collect();

    if name.starts_with(query) {
        return Some(weighted_score(
            MATCH_EXACT_PREFIX,
            query.len(),
            entry,
            usage,
        ));
    }
    if has_word_prefix(&name, query) || has_word_prefix(&exec, query) {
        return Some(weighted_score(MATCH_WORD_PREFIX, query.len(), entry, usage));
    }
    if name.contains(query) || exec.contains(query) {
        return Some(weighted_score(MATCH_SUBSTRING, query.len(), entry, usage));
    }
    if keywords
        .iter()
        .any(|keyword| keyword.starts_with(query) || keyword.contains(query))
    {
        return Some(weighted_score(MATCH_KEYWORD, query.len(), entry, usage));
    }

    None
}

fn weighted_score(class: i32, query_len: usize, entry: &DesktopEntry, usage: &UsageStats) -> i32 {
    class * 10_000 + (100 - query_len as i32).max(0) * 10 + usage_boost(entry, usage)
}

fn usage_boost(entry: &DesktopEntry, usage: &UsageStats) -> i32 {
    let launches = usage.launch_count(&entry.id) as i32;
    let last_launched = usage.last_launched(&entry.id) as i32;
    launches
        .saturating_mul(5)
        .saturating_add(last_launched / 10_000)
}

fn has_word_prefix(text: &str, query: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|segment| !segment.is_empty())
        .any(|word| word.starts_with(query))
}

/// Sort results into category-major, score-descending order. Call this before
/// passing results to `populate_search_results` so the list-box header-func
/// sees contiguous groups per category and emits one header per transition.
fn sort_by_category_and_score(results: &mut [SearchResult]) {
    results.sort_by(|a, b| {
        a.category()
            .display_order()
            .cmp(&b.category().display_order())
            .then_with(|| b.score().cmp(&a.score()))
    });
}

/// Drop results whose category is excluded by `filter`.
fn apply_filter(results: &mut Vec<SearchResult>, filter: ResultFilter) {
    results.retain(|r| filter.includes(r.category()));
}

/// Compose the launcher's result list for `query` under `filter`. Pure logic
/// — no GTK — so it can be unit-tested end-to-end.
fn compose_launcher_results(
    apps: &[DesktopEntry],
    query: &str,
    max_results: usize,
    usage_stats: &UsageStats,
    canvas: Option<&common::contracts::CanvasState>,
    filter: ResultFilter,
) -> Vec<SearchResult> {
    let app_results = rank_entries(apps, query, max_results, usage_stats);
    let mut merged: Vec<SearchResult> = app_results.into_iter().map(SearchResult::App).collect();

    if let Some(canvas) = canvas {
        let normalized = query.trim().to_lowercase();
        if normalized.is_empty() {
            merged.extend(recent_windows(canvas, max_results));
        } else {
            merged.extend(search_windows_and_clusters(
                canvas,
                &normalized,
                max_results,
            ));
        }
    }

    apply_filter(&mut merged, filter);
    sort_by_category_and_score(&mut merged);
    merged.truncate(max_results);
    merged
}

fn populate_search_results(
    list: &gtk::ListBox,
    state: &Rc<RefCell<Vec<SearchResult>>>,
    results: &[SearchResult],
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    {
        let mut guard = state.borrow_mut();
        guard.clear();
        guard.extend(results.iter().cloned());
    }

    for result in results {
        let row = adw::ActionRow::builder()
            .title(result.display_title())
            .subtitle(result.display_subtitle())
            .build();
        row.set_activatable(true);
        if let Some(icon_name) = result.icon_name() {
            let image = gtk::Image::from_icon_name(icon_name);
            row.add_prefix(&image);
        }

        list.append(&row);
    }

    if let Some(row) = list.row_at_index(0) {
        list.select_row(Some(&row));
    }
}

fn selected_index(list: &gtk::ListBox) -> Option<usize> {
    list.selected_row().map(|row| row.index() as usize)
}

fn move_selection(list: &gtk::ListBox, direction: i32) {
    let count = list.observe_children().n_items() as i32;
    if count <= 0 {
        return;
    }

    let current = selected_index(list).map(|idx| idx as i32).unwrap_or(0);
    let next = (current + direction).clamp(0, count - 1);
    if let Some(row) = list.row_at_index(next) {
        list.select_row(Some(&row));
    }
}

fn activate_search_result(
    results: &[SearchResult],
    index: usize,
    terminal_command: &str,
    mode: LaunchMode,
    usage_stats: &Rc<RefCell<UsageStats>>,
    usage_path: &PathBuf,
) -> Result<(), String> {
    let Some(selected) = results.get(index) else {
        return Err("no selection".to_owned());
    };

    match selected {
        SearchResult::App(scored) => {
            let cmd = build_exec_command(&scored.entry).map_err(|error| error.to_string())?;
            tracing::info!(entry = scored.entry.name, ?cmd, "launching desktop entry");

            if scored.entry.terminal || matches!(mode, LaunchMode::ForceTerminal) {
                let terminal = parse_terminal_command(terminal_command);
                let mut command = Command::new(&terminal[0]);
                command
                    .args(&terminal[1..])
                    .arg("-e")
                    .arg(&cmd[0])
                    .args(&cmd[1..]);
                command.spawn().map_err(|error| error.to_string())?;
            } else {
                let mut command = Command::new(&cmd[0]);
                command.args(&cmd[1..]);
                command.spawn().map_err(|error| error.to_string())?;
            }

            {
                let mut usage = usage_stats.borrow_mut();
                usage.record_launch(&scored.entry.id);
                if let Err(error) = save_usage_stats(usage_path, &usage) {
                    tracing::warn!(?error, path = ?usage_path, "failed to persist launcher usage stats");
                }
            }
        }
        SearchResult::Window {
            window_id, title, ..
        } => {
            tracing::info!(
                window_id,
                title = title.as_str(),
                "focusing window from launcher"
            );
            Command::new("swaymsg")
                .args([&format!("[con_id={window_id}] focus")])
                .spawn()
                .map_err(|error| error.to_string())?;
        }
        SearchResult::Cluster {
            cluster_id, name, ..
        } => {
            tracing::info!(
                cluster_id,
                name = name.as_str(),
                "activating cluster from launcher"
            );
            Command::new("vibeshellctl")
                .args(["ipc", "activate-cluster", &cluster_id.to_string()])
                .spawn()
                .map_err(|error| error.to_string())?;
        }
    }

    Ok(())
}

fn usage_stats_path() -> PathBuf {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from(".local/share"));
    data_home.join("vibeshell").join("launcher-usage.json")
}

fn load_usage_stats(path: &Path) -> UsageStats {
    let Ok(contents) = fs::read_to_string(path) else {
        return UsageStats::default();
    };

    serde_json::from_str(&contents).unwrap_or_else(|error| {
        tracing::warn!(?error, path = ?path, "failed to parse launcher usage stats");
        UsageStats::default()
    })
}

fn save_usage_stats(path: &Path, usage: &UsageStats) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let payload = serde_json::to_string_pretty(usage).map_err(|error| error.to_string())?;
    fs::write(path, payload).map_err(|error| error.to_string())
}

fn close_launcher(window: &adw::ApplicationWindow, app: &adw::Application) {
    if layer_shell::is_supported() {
        window.set_keyboard_mode(layer_shell::KeyboardMode::OnDemand);
    }
    window.close();
    app.quit();
}

fn build_exec_command(entry: &DesktopEntry) -> Result<Vec<String>, shell_words::ParseError> {
    let mut tokens = shell_words::split(&entry.exec)?;
    for token in &mut tokens {
        *token = expand_exec_token(token, entry);
    }
    tokens.retain(|token| !token.is_empty());
    Ok(tokens)
}

fn expand_exec_token(token: &str, entry: &DesktopEntry) -> String {
    let mut out = String::new();
    let mut chars = token.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }

        match chars.next() {
            Some('%') => out.push('%'),
            Some('c') => out.push_str(&entry.name),
            Some('k') => out.push_str(&entry.path.to_string_lossy()),
            Some('i') => {
                if let Some(icon) = &entry.icon {
                    out.push_str("--icon ");
                    out.push_str(icon);
                }
            }
            Some('f' | 'F' | 'u' | 'U') => {}
            Some(_) | None => {}
        }
    }

    out
}

fn parse_terminal_command(configured: &str) -> Vec<String> {
    let parsed = shell_words::split(configured).unwrap_or_else(|_| vec!["foot".to_owned()]);
    if parsed.is_empty() {
        vec!["foot".to_owned()]
    } else {
        parsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::contracts::{
        CanvasState, Cluster, OutputState, Viewport, Window, WindowRole, WindowState,
    };
    use std::collections::HashMap;

    fn fixture_canvas() -> CanvasState {
        CanvasState {
            state_revision: 1,
            zoom: Default::default(),
            viewport: Viewport::default(),
            output_viewports: HashMap::new(),
            clusters: vec![
                Cluster {
                    id: 1,
                    name: "web".into(),
                    x: 0.0,
                    y: 0.0,
                    enabled: true,
                    windows: vec![101, 102],
                    last_focus: Some(101),
                    recency: vec![101, 102],
                },
                Cluster {
                    id: 2,
                    name: "terminals".into(),
                    x: 200.0,
                    y: 0.0,
                    enabled: true,
                    windows: vec![201],
                    last_focus: Some(201),
                    recency: vec![201],
                },
            ],
            windows: vec![
                Window {
                    id: 101,
                    title: "My personal Firefox window".into(),
                    app_id: Some("firefox".into()),
                    cluster_id: Some(1),
                    role: WindowRole::Normal,
                    state: WindowState::Tiled,
                    ..Default::default()
                },
                Window {
                    id: 102,
                    title: "GitHub — Firefox".into(),
                    app_id: Some("firefox".into()),
                    cluster_id: Some(1),
                    role: WindowRole::Normal,
                    state: WindowState::Tiled,
                    ..Default::default()
                },
                Window {
                    id: 201,
                    title: "zsh".into(),
                    app_id: Some("foot".into()),
                    cluster_id: Some(2),
                    role: WindowRole::Normal,
                    state: WindowState::Tiled,
                    ..Default::default()
                },
                Window {
                    id: 999,
                    title: "orphan pane".into(),
                    app_id: Some("code".into()),
                    cluster_id: None,
                    role: WindowRole::Normal,
                    state: WindowState::Tiled,
                    ..Default::default()
                },
            ],
            output: OutputState::default(),
        }
    }

    #[test]
    fn empty_query_returns_no_results() {
        let canvas = fixture_canvas();
        assert!(search_windows_and_clusters(&canvas, "", 10).is_empty());
    }

    #[test]
    fn app_id_prefix_match_outranks_title_substring() {
        // query "fire" — matches app_id "firefox" as prefix (highest tier)
        // and title "My personal Firefox window" as substring (middle tier).
        // Both rank together; app_id-prefix scores higher.
        let canvas = fixture_canvas();
        let results = search_windows_and_clusters(&canvas, "fire", 10);

        assert!(!results.is_empty());
        // Top result must be a Window scored at MATCH_EXACT_PREFIX tier.
        let top_score = results[0].score();
        assert_eq!(top_score, MATCH_EXACT_PREFIX * 10_000);
    }

    #[test]
    fn title_substring_match_returns_hit() {
        let canvas = fixture_canvas();
        let results = search_windows_and_clusters(&canvas, "github", 10);

        assert!(results.iter().any(|r| matches!(
            r,
            SearchResult::Window { title, .. } if title.contains("GitHub")
        )));
    }

    #[test]
    fn cluster_name_prefix_match_is_returned() {
        let canvas = fixture_canvas();
        let results = search_windows_and_clusters(&canvas, "term", 10);

        assert!(results.iter().any(|r| matches!(
            r,
            SearchResult::Cluster { name, .. } if name == "terminals"
        )));
    }

    #[test]
    fn window_without_cluster_labeled_unassigned() {
        let canvas = fixture_canvas();
        // "orphan" matches the title of window 999, which has cluster_id = None.
        let results = search_windows_and_clusters(&canvas, "orphan", 10);

        let hit = results
            .iter()
            .find(|r| matches!(r, SearchResult::Window { window_id: 999, .. }))
            .expect("orphan window should match");
        match hit {
            SearchResult::Window { cluster_name, .. } => {
                assert_eq!(cluster_name, "unassigned");
            }
            _ => panic!("expected Window variant"),
        }
    }

    #[test]
    fn results_truncated_to_max_results() {
        let canvas = fixture_canvas();
        // query "e" matches broadly (personal, firefox, zsh, orphan, terminals, web).
        let results = search_windows_and_clusters(&canvas, "e", 2);
        assert!(results.len() <= 2);
    }

    #[test]
    fn results_sorted_by_score_descending() {
        let canvas = fixture_canvas();
        let results = search_windows_and_clusters(&canvas, "f", 10);

        for pair in results.windows(2) {
            assert!(
                pair[0].score() >= pair[1].score(),
                "results not sorted descending: {} then {}",
                pair[0].score(),
                pair[1].score()
            );
        }
    }

    #[test]
    fn non_matching_query_returns_empty() {
        let canvas = fixture_canvas();
        let results = search_windows_and_clusters(&canvas, "qzzzzx", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn recent_windows_returns_empty_when_no_clusters() {
        let canvas = CanvasState::default();
        assert!(recent_windows(&canvas, 10).is_empty());
    }

    #[test]
    fn recent_windows_orders_active_cluster_first() {
        // Zoom into cluster 2 ("terminals"). Its recency [201] should precede
        // cluster 1's windows (101, 102).
        let mut canvas = fixture_canvas();
        canvas.zoom = common::contracts::ZoomLevel::Cluster(2);
        canvas.clusters[0].recency = vec![102, 101];
        canvas.clusters[1].recency = vec![201];

        let out = recent_windows(&canvas, 10);
        let ids: Vec<u64> = out
            .iter()
            .filter_map(|r| match r {
                SearchResult::Window { window_id, .. } => Some(*window_id),
                _ => None,
            })
            .collect();

        assert_eq!(ids, vec![201, 102, 101]);
    }

    #[test]
    fn recent_windows_dedupes_across_clusters() {
        let mut canvas = fixture_canvas();
        // Simulate a stale recency entry: cluster 2 still remembers window 101
        // (a bug, since 101 lives in cluster 1 now). It should only appear once,
        // from whichever cluster is visited first.
        canvas.clusters[0].recency = vec![101, 102];
        canvas.clusters[1].recency = vec![101, 201];

        let out = recent_windows(&canvas, 10);
        let ids: Vec<u64> = out
            .iter()
            .filter_map(|r| match r {
                SearchResult::Window { window_id, .. } => Some(*window_id),
                _ => None,
            })
            .collect();

        assert_eq!(ids.iter().filter(|&&id| id == 101).count(), 1);
    }

    #[test]
    fn recent_windows_scores_descend_with_position() {
        let canvas = fixture_canvas();
        let out = recent_windows(&canvas, 10);
        for pair in out.windows(2) {
            assert!(
                pair[0].score() > pair[1].score(),
                "scores must strictly descend: {} then {}",
                pair[0].score(),
                pair[1].score()
            );
        }
    }

    #[test]
    fn recent_windows_respects_max_results() {
        let canvas = fixture_canvas();
        let out = recent_windows(&canvas, 2);
        assert!(out.len() <= 2);
    }

    #[test]
    fn result_filter_cycles_through_all_then_wraps() {
        let seen = [
            ResultFilter::All,
            ResultFilter::All.next(),
            ResultFilter::All.next().next(),
            ResultFilter::All.next().next().next(),
            ResultFilter::All.next().next().next().next(),
        ];
        assert_eq!(seen[0], ResultFilter::All);
        assert_eq!(seen[1], ResultFilter::Only(ResultCategory::App));
        assert_eq!(seen[2], ResultFilter::Only(ResultCategory::Window));
        assert_eq!(seen[3], ResultFilter::Only(ResultCategory::Cluster));
        assert_eq!(seen[4], ResultFilter::All);
    }

    #[test]
    fn result_filter_includes_respects_category() {
        assert!(ResultFilter::All.includes(ResultCategory::App));
        assert!(ResultFilter::All.includes(ResultCategory::Window));
        assert!(ResultFilter::All.includes(ResultCategory::Cluster));
        assert!(ResultFilter::Only(ResultCategory::Window).includes(ResultCategory::Window));
        assert!(!ResultFilter::Only(ResultCategory::Window).includes(ResultCategory::App));
    }

    #[test]
    fn apply_filter_retains_only_matching_category() {
        let canvas = fixture_canvas();
        let mut results = search_windows_and_clusters(&canvas, "fire", 10);
        results.extend(search_windows_and_clusters(&canvas, "web", 10));
        let before_len = results.len();
        assert!(before_len > 0);

        apply_filter(&mut results, ResultFilter::Only(ResultCategory::Cluster));
        assert!(results
            .iter()
            .all(|r| r.category() == ResultCategory::Cluster));
    }

    #[test]
    fn sort_by_category_groups_apps_windows_clusters_in_order() {
        let canvas = fixture_canvas();
        let mut mixed: Vec<SearchResult> = Vec::new();
        mixed.extend(recent_windows(&canvas, 10));
        mixed.extend(search_windows_and_clusters(&canvas, "web", 10));
        // No apps in this fixture (we'd need DesktopEntry fixtures); just
        // check that the category ordering invariant holds for what we have.

        sort_by_category_and_score(&mut mixed);

        let mut prev_order: u8 = 0;
        for r in &mixed {
            let order = r.category().display_order();
            assert!(order >= prev_order, "category out of order in sorted list");
            prev_order = order;
        }
    }

    #[test]
    fn sort_by_category_descends_by_score_within_group() {
        let canvas = fixture_canvas();
        let mut windows = recent_windows(&canvas, 10);
        sort_by_category_and_score(&mut windows);
        for pair in windows.windows(2) {
            assert!(pair[0].score() >= pair[1].score());
        }
    }

    #[test]
    fn recent_windows_skips_ids_not_in_windows_list() {
        let mut canvas = fixture_canvas();
        // Cluster 1 recency mentions a window (555) that's not in canvas.windows
        // — simulates a race where the window closed between Sway events.
        canvas.clusters[0].recency = vec![555, 102];

        let out = recent_windows(&canvas, 10);
        let ids: Vec<u64> = out
            .iter()
            .filter_map(|r| match r {
                SearchResult::Window { window_id, .. } => Some(*window_id),
                _ => None,
            })
            .collect();

        assert!(!ids.contains(&555));
        assert!(ids.contains(&102));
    }
}
