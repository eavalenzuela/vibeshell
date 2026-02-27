use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use adw::prelude::*;
use chrono::Local;
use config::{Config, PanelConfig};
use gtk::glib;
use gtk4 as gtk;
use gtk4_layer_shell::{self as layer_shell, LayerShell};
use sway::{PanelState, PanelUpdate, WorkspaceState};

const RENDER_DEBOUNCE: Duration = Duration::from_millis(50);
const STATUS_POLL_INTERVAL: Duration = Duration::from_secs(5);
const SWAY_CONNECT_INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const SWAY_CONNECT_MAX_BACKOFF: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
struct PanelStatus {
    volume: String,
    wifi: String,
    battery: String,
}

impl Default for PanelStatus {
    fn default() -> Self {
        Self {
            volume: "vol N/A".to_owned(),
            wifi: "wifi N/A".to_owned(),
            battery: "bat N/A".to_owned(),
        }
    }
}

fn main() {
    common::init_logging("panel");
    tracing::info!(app = "panel", "starting up");

    let panel_config = Config::load().map(|cfg| cfg.panel).unwrap_or_else(|error| {
        tracing::warn!(?error, "failed to load config, using defaults");
        PanelConfig::default()
    });

    let app = adw::Application::builder()
        .application_id("com.vibeshell.panel")
        .build();

    app.connect_activate(move |app| build_ui(app, panel_config.clone()));
    app.run();
}

fn build_ui(app: &adw::Application, panel_config: PanelConfig) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("vibeshell-panel")
        .default_height(panel_config.height)
        .build();

    window.set_decorated(false);
    window.set_resizable(false);
    window.set_size_request(-1, panel_config.height);

    window.init_layer_shell();
    window.set_layer(layer_shell::Layer::Top);
    window.set_anchor(layer_shell::Edge::Top, true);
    window.set_anchor(layer_shell::Edge::Left, true);
    window.set_anchor(layer_shell::Edge::Right, true);
    window.set_exclusive_zone(panel_config.height);

    let workspaces = gtk::Label::new(Some(""));
    workspaces.set_halign(gtk::Align::Start);
    workspaces.set_margin_start(panel_config.margin_start);

    let title = gtk::Label::new(Some(""));
    title.set_halign(gtk::Align::Center);

    let clock = gtk::Label::new(Some(
        &Local::now().format(&panel_config.clock_format).to_string(),
    ));
    clock.set_halign(gtk::Align::End);

    let volume = gtk::Label::new(Some("🔊 vol 45%"));
    let wifi = gtk::Label::new(Some("📶 wifi connected"));
    let battery = gtk::Label::new(Some("🔋 bat 82%"));

    let right_section = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .halign(gtk::Align::End)
        .build();
    right_section.set_margin_end(panel_config.margin_end);
    right_section.append(&volume);
    right_section.append(&wifi);
    right_section.append(&battery);
    right_section.append(&clock);

    let content = gtk::CenterBox::builder()
        .orientation(gtk::Orientation::Horizontal)
        .hexpand(true)
        .build();
    content.set_start_widget(Some(&workspaces));
    content.set_center_widget(Some(&title));
    content.set_end_widget(Some(&right_section));

    window.set_content(Some(&content));
    window.present();

    let clock_format = panel_config.clock_format.clone();
    glib::timeout_add_seconds_local(1, move || {
        clock.set_text(&Local::now().format(&clock_format).to_string());
        glib::ControlFlow::Continue
    });

    let (sender, receiver) = mpsc::channel::<PanelUpdate>();
    let (status_sender, status_receiver) = mpsc::channel::<PanelStatus>();

    thread::spawn(move || {
        let mut backoff = SWAY_CONNECT_INITIAL_BACKOFF;

        loop {
            match sway::SwayClient::connect() {
                Ok(client) => {
                    tracing::info!("connected to sway ipc");
                    if let Err(error) =
                        client.run_listener(sender.clone(), Duration::from_millis(80))
                    {
                        tracing::warn!(?error, "sway listener exited; retrying connection");
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        retry_ms = backoff.as_millis(),
                        "unable to connect to sway ipc; ensure sway is running and SWAYSOCK is set"
                    );
                    eprintln!(
                        "panel: sway IPC unavailable. Start sway first (or export SWAYSOCK), retrying in {} ms.",
                        backoff.as_millis()
                    );
                }
            }

            thread::sleep(backoff);
            backoff = (backoff * 2).min(SWAY_CONNECT_MAX_BACKOFF);
        }
    });

    thread::spawn(move || {
        let mut last_status = PanelStatus::default();

        if status_sender.send(last_status.clone()).is_err() {
            return;
        }

        loop {
            let next_status = collect_status();
            if next_status.volume != last_status.volume
                || next_status.wifi != last_status.wifi
                || next_status.battery != last_status.battery
            {
                if status_sender.send(next_status.clone()).is_err() {
                    break;
                }
                last_status = next_status;
            }

            thread::sleep(STATUS_POLL_INTERVAL);
        }
    });

    let latest_state = Rc::new(RefCell::new(None::<PanelState>));
    let scheduled_render = Rc::new(RefCell::new(None::<glib::SourceId>));
    let last_rendered = Rc::new(RefCell::new(None::<PanelState>));

    glib::timeout_add_local(Duration::from_millis(16), move || {
        while let Ok(update) = receiver.try_recv() {
            let PanelUpdate::Snapshot(state) = update;
            latest_state.replace(Some(state));

            if scheduled_render.borrow().is_some() {
                continue;
            }

            let latest_state = Rc::clone(&latest_state);
            let scheduled_render = Rc::clone(&scheduled_render);
            let last_rendered = Rc::clone(&last_rendered);
            let workspaces = workspaces.clone();
            let title = title.clone();

            let source_id = glib::timeout_add_local_once(RENDER_DEBOUNCE, move || {
                scheduled_render.borrow_mut().take();

                let Some(next_state) = latest_state.borrow_mut().take() else {
                    return;
                };

                if last_rendered
                    .borrow()
                    .as_ref()
                    .is_some_and(|rendered| rendered == &next_state)
                {
                    return;
                }

                apply_state(&workspaces, &title, &next_state);
                last_rendered.replace(Some(next_state));
            });

            scheduled_render.replace(Some(source_id));
        }

        glib::ControlFlow::Continue
    });

    glib::timeout_add_local(Duration::from_millis(250), move || {
        while let Ok(status) = status_receiver.try_recv() {
            volume.set_text(&format!("🔊 {}", status.volume));
            wifi.set_text(&format!("📶 {}", status.wifi));
            battery.set_text(&format!("🔋 {}", status.battery));
        }

        glib::ControlFlow::Continue
    });
}

fn apply_state(workspaces_label: &gtk::Label, title_label: &gtk::Label, state: &PanelState) {
    workspaces_label.set_text(&format_workspaces(&state.workspaces));
    title_label.set_text(state.focused_title.as_deref().unwrap_or(""));
}

fn format_workspaces(workspaces: &[WorkspaceState]) -> String {
    if workspaces.is_empty() {
        return "no workspaces".to_owned();
    }

    workspaces
        .iter()
        .map(|workspace| {
            let display_name = workspace
                .num
                .filter(|num| *num > 0)
                .map(|num| num.to_string())
                .unwrap_or_else(|| workspace.name.clone());

            let status = if workspace.focused {
                '●'
            } else if workspace.visible {
                '◉'
            } else {
                '○'
            };

            let urgent = if workspace.urgent { "!" } else { "" };
            format!("{status}{display_name}{urgent}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn collect_status() -> PanelStatus {
    PanelStatus {
        volume: read_volume_status().unwrap_or_else(|| "vol N/A".to_owned()),
        wifi: read_wifi_status().unwrap_or_else(|| "wifi N/A".to_owned()),
        battery: read_battery_status().unwrap_or_else(|| "bat N/A".to_owned()),
    }
}

fn read_volume_status() -> Option<String> {
    let wpctl_output = run_command("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])?.stdout;
    let mut tokens = wpctl_output.split_whitespace();
    let _label = tokens.next()?;
    let value = tokens.next()?.parse::<f32>().ok()?;

    Some(format!("vol {}%", (value * 100.0).round() as i32))
}

fn read_wifi_status() -> Option<String> {
    let wifi_output = run_command("nmcli", &["-t", "-f", "WIFI", "g"])?;
    let wifi_state = wifi_output.stdout.lines().next()?.trim().to_lowercase();

    if wifi_state == "enabled" {
        Some("wifi connected".to_owned())
    } else {
        Some("wifi disconnected".to_owned())
    }
}

fn read_battery_status() -> Option<String> {
    let entries = fs::read_dir("/sys/class/power_supply").ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name()?.to_str()?.to_owned();
        if !name.starts_with("BAT") {
            continue;
        }

        let capacity = read_trimmed(path.join("capacity"))?;
        return Some(format!("bat {capacity}%"));
    }

    None
}

fn read_trimmed(path: impl AsRef<Path>) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    Some(raw.trim().to_owned())
}

fn run_command(binary: &str, args: &[&str]) -> Option<CommandOutput> {
    let output = Command::new(binary).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    Some(CommandOutput {
        stdout: String::from_utf8(output.stdout).ok()?,
    })
}

struct CommandOutput {
    stdout: String,
}
