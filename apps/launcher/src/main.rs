use std::cell::RefCell;
use std::process::Command;
use std::rc::Rc;
use std::thread;
use std::time::Duration;

use adw::prelude::*;
use config::{Config, LauncherConfig};
use gtk::gdk;
use gtk::glib;
use gtk4 as gtk;
use gtk4_layer_shell::{self as layer_shell, LayerShell};
use xdg::DesktopEntry;

const SWAY_CONNECT_INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const SWAY_CONNECT_MAX_BACKOFF: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct ScoredEntry {
    score: i32,
    entry: DesktopEntry,
}

fn main() {
    common::init_logging("launcher");
    tracing::info!(app = "launcher", "starting up");

    let launcher_config = Config::load()
        .map(|cfg| cfg.launcher)
        .unwrap_or_else(|error| {
            tracing::warn!(?error, "failed to load config, using defaults");
            LauncherConfig::default()
        });

    let apps = xdg::discover_applications().unwrap_or_else(|error| {
        tracing::warn!(?error, "failed to read desktop entries");
        Vec::new()
    });

    spawn_sway_dependency_probe();

    let app = adw::Application::builder()
        .application_id("com.vibeshell.launcher")
        .build();

    app.connect_activate(move |app| build_ui(app, apps.clone(), launcher_config.clone()));
    app.run();
}

fn build_ui(app: &adw::Application, apps: Vec<DesktopEntry>, launcher_config: LauncherConfig) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("vibeshell-launcher")
        .default_width(launcher_config.window_width)
        .default_height(launcher_config.window_height)
        .build();

    window.set_decorated(false);
    window.set_resizable(false);

    window.init_layer_shell();
    window.set_layer(layer_shell::Layer::Overlay);
    window.set_keyboard_mode(layer_shell::KeyboardMode::Exclusive);
    window.set_anchor(layer_shell::Edge::Top, true);
    window.set_anchor(layer_shell::Edge::Bottom, true);
    window.set_anchor(layer_shell::Edge::Left, true);
    window.set_anchor(layer_shell::Edge::Right, true);

    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(20)
        .margin_end(20)
        .build();

    let input = gtk::Entry::builder()
        .placeholder_text("Launch an app...")
        .build();

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::Single);

    panel.append(&input);
    panel.append(&list);

    let container = gtk::CenterBox::new();
    container.set_center_widget(Some(&panel));
    window.set_content(Some(&container));

    let state = Rc::new(RefCell::new(Vec::<ScoredEntry>::new()));

    {
        let apps = apps.clone();
        let list = list.clone();
        let state = state.clone();
        let max_results = launcher_config.max_results;
        input.connect_changed(move |entry| {
            let query = entry.text().to_string();
            let ranked = rank_entries(&apps, &query, max_results);
            populate_results(&list, &state, &ranked);
        });
    }

    {
        let window = window.clone();
        let input = input.clone();
        let state = state.clone();
        let terminal_command = launcher_config.terminal_command.clone();
        input.connect_activate(move |_| {
            if launch_selected(&state.borrow(), 0, &terminal_command).is_ok() {
                window.close();
            }
        });
    }

    {
        let window = window.clone();
        let list = list.clone();
        let state = state.clone();
        let terminal_command = launcher_config.terminal_command.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Escape => {
                window.close();
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
            gdk::Key::Return => {
                let index = selected_index(&list).unwrap_or(0);
                if launch_selected(&state.borrow(), index, &terminal_command).is_ok() {
                    window.close();
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        window.add_controller(key_controller);
    }

    {
        let window = window.clone();
        let state = state.clone();
        let terminal_command = launcher_config.terminal_command.clone();
        list.connect_row_activated(move |_, row| {
            let index = row.index() as usize;
            if launch_selected(&state.borrow(), index, &terminal_command).is_ok() {
                window.close();
            }
        });
    }

    populate_results(
        &list,
        &state,
        &rank_entries(&apps, "", launcher_config.max_results),
    );

    window.present();
    input.grab_focus();
}

fn rank_entries(entries: &[DesktopEntry], query: &str, max_results: usize) -> Vec<ScoredEntry> {
    let normalized_query = query.trim().to_lowercase();

    let mut ranked: Vec<_> = entries
        .iter()
        .filter_map(|entry| {
            let score = score_entry(entry, &normalized_query)?;
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

fn score_entry(entry: &DesktopEntry, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(1);
    }

    let name = entry.name.to_lowercase();
    let exec = entry.exec.to_lowercase();
    let keywords = entry.keywords.join(" ").to_lowercase();

    if name.starts_with(query) {
        return Some(120 - query.len() as i32);
    }
    if name.contains(query) {
        return Some(90 - query.len() as i32);
    }
    if keywords.starts_with(query) {
        return Some(70 - query.len() as i32);
    }
    if keywords.contains(query) {
        return Some(60 - query.len() as i32);
    }
    if exec.starts_with(query) {
        return Some(40 - query.len() as i32);
    }
    if exec.contains(query) {
        return Some(20 - query.len() as i32);
    }

    None
}

fn populate_results(
    list: &gtk::ListBox,
    state: &Rc<RefCell<Vec<ScoredEntry>>>,
    ranked: &[ScoredEntry],
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    {
        let mut guard = state.borrow_mut();
        guard.clear();
        guard.extend_from_slice(ranked);
    }

    for scored in ranked {
        let subtitle = if scored.entry.keywords.is_empty() {
            scored.entry.exec.clone()
        } else {
            format!(
                "{} · {}",
                scored.entry.exec,
                scored.entry.keywords.join(", ")
            )
        };

        let row = adw::ActionRow::builder()
            .title(&scored.entry.name)
            .subtitle(&subtitle)
            .build();
        row.set_activatable(true);
        if let Some(icon_name) = scored.entry.icon.as_deref() {
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

fn launch_selected(
    entries: &[ScoredEntry],
    index: usize,
    terminal_command: &str,
) -> Result<(), String> {
    let Some(selected) = entries.get(index) else {
        return Err("no selection".to_owned());
    };

    let cmd = build_exec_command(&selected.entry).map_err(|error| error.to_string())?;
    tracing::info!(entry = selected.entry.name, ?cmd, "launching desktop entry");

    if selected.entry.terminal {
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

    Ok(())
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

fn spawn_sway_dependency_probe() {
    thread::spawn(|| {
        let mut backoff = SWAY_CONNECT_INITIAL_BACKOFF;

        loop {
            match sway::SwayClient::connect() {
                Ok(_) => {
                    tracing::info!("launcher connected to sway ipc");
                    return;
                }
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        retry_ms = backoff.as_millis(),
                        "launcher could not connect to sway ipc; ensure sway is running and SWAYSOCK is set"
                    );
                    eprintln!(
                        "launcher: sway IPC unavailable. Start sway first (or export SWAYSOCK), retrying in {} ms.",
                        backoff.as_millis()
                    );
                }
            }

            thread::sleep(backoff);
            backoff = (backoff * 2).min(SWAY_CONNECT_MAX_BACKOFF);
        }
    });
}
