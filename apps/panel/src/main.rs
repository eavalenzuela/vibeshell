use std::cell::RefCell;
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

mod status;

use adw::prelude::*;
use chrono::Local;
use config::{Config, PanelConfig};
use gtk::glib;
use gtk4 as gtk;
use gtk4_layer_shell::{self as layer_shell, LayerShell};
use status::{PanelStatus, StatusCollector};

#[derive(Clone)]
struct RuntimePanelConfig {
    clock_format: String,
    status_poll_interval_ms: u64,
    audio_toggle_command: String,
    audio_mixer_command: Option<String>,
    network_settings_command: String,
    power_menu_command: String,
}

impl From<&PanelConfig> for RuntimePanelConfig {
    fn from(value: &PanelConfig) -> Self {
        Self {
            clock_format: value.clock_format.clone(),
            status_poll_interval_ms: value.status_poll_interval_ms,
            audio_toggle_command: value.audio_toggle_command.clone(),
            audio_mixer_command: value.audio_mixer_command.clone(),
            network_settings_command: value.network_settings_command.clone(),
            power_menu_command: value.power_menu_command.clone(),
        }
    }
}
use sway::{PanelState, PanelUpdate, WorkspaceState};

const RENDER_DEBOUNCE: Duration = Duration::from_millis(50);
const SWAY_CONNECT_INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const SWAY_CONNECT_MAX_BACKOFF: Duration = Duration::from_secs(10);

fn main() {
    common::init_logging("panel");
    tracing::info!(app = "panel", "starting up");

    let panel_config = Config::load().map(|cfg| cfg.panel).unwrap_or_else(|error| {
        tracing::warn!(?error, "failed to load config, using defaults");
        PanelConfig::default()
    });

    let (_, reload_rx) = common::spawn_reload_listener();

    let app = adw::Application::builder()
        .application_id("com.vibeshell.panel")
        .build();

    app.connect_activate(move |app| build_ui(app, panel_config.clone(), reload_rx));
    app.run();
}

fn build_ui(
    app: &adw::Application,
    panel_config: PanelConfig,
    reload_rx: mpsc::Receiver<common::ReloadReason>,
) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("vibeshell-panel")
        .default_height(panel_config.height)
        .build();

    window.set_size_request(-1, panel_config.height);

    let runtime_config = Arc::new(Mutex::new(RuntimePanelConfig::from(&panel_config)));

    if layer_shell::is_supported() {
        window.set_decorated(false);
        window.set_resizable(false);
        window.init_layer_shell();
        window.set_layer(layer_shell::Layer::Top);
        window.set_anchor(layer_shell::Edge::Top, true);
        window.set_anchor(layer_shell::Edge::Left, true);
        window.set_anchor(layer_shell::Edge::Right, true);
        window.set_exclusive_zone(panel_config.height);
    } else {
        tracing::warn!("layer shell protocol unavailable; falling back to a regular GTK window");
        eprintln!(
            "panel: compositor does not support zwlr_layer_shell_v1; using regular window mode."
        );
    }

    let workspaces = gtk::Label::new(Some(""));
    workspaces.set_halign(gtk::Align::Start);
    workspaces.set_margin_start(panel_config.margin_start);

    let title = gtk::Label::new(Some(""));
    title.set_halign(gtk::Align::Center);

    let clock = gtk::Label::new(Some(
        &Local::now()
            .format(
                &runtime_config
                    .lock()
                    .expect("runtime config poisoned")
                    .clock_format,
            )
            .to_string(),
    ));
    clock.set_halign(gtk::Align::End);

    let audio = gtk::Label::new(Some("🔇 audio N/A"));
    let network = gtk::Label::new(Some("📶 network N/A"));
    let battery = gtk::Label::new(Some("🔋 battery N/A"));
    let power = gtk::Label::new(Some("⏻"));

    let right_section = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .halign(gtk::Align::End)
        .build();
    right_section.set_margin_end(panel_config.margin_end);
    right_section.append(&audio);
    right_section.append(&network);
    right_section.append(&battery);
    right_section.append(&clock);
    right_section.append(&power);

    let content = gtk::CenterBox::builder()
        .orientation(gtk::Orientation::Horizontal)
        .hexpand(true)
        .build();
    content.set_start_widget(Some(&workspaces));
    content.set_center_widget(Some(&title));
    content.set_end_widget(Some(&right_section));

    window.set_content(Some(&content));

    let calendar = gtk::Calendar::new();
    let calendar_popover = gtk::Popover::builder()
        .autohide(true)
        .has_arrow(true)
        .build();
    calendar_popover.set_child(Some(&calendar));
    calendar_popover.set_parent(&clock);

    let clock_click = gtk::GestureClick::new();
    {
        let calendar_popover = calendar_popover.clone();
        clock_click.connect_pressed(move |_, _, _, _| {
            calendar_popover.popup();
        });
    }
    clock.add_controller(clock_click);

    let audio_click = gtk::GestureClick::new();
    {
        let runtime_config = Arc::clone(&runtime_config);
        audio_click.connect_pressed(move |_, _, _, _| {
            let config = runtime_config
                .lock()
                .expect("runtime config poisoned")
                .clone();
            if let Some(command) = config.audio_mixer_command.as_deref() {
                run_configured_command(command, "audio mixer");
            } else {
                run_configured_command(&config.audio_toggle_command, "audio toggle");
            }
        });
    }
    audio.add_controller(audio_click);

    let network_click = gtk::GestureClick::new();
    {
        let runtime_config = Arc::clone(&runtime_config);
        network_click.connect_pressed(move |_, _, _, _| {
            let config = runtime_config
                .lock()
                .expect("runtime config poisoned")
                .clone();
            run_configured_command(&config.network_settings_command, "network settings");
        });
    }
    network.add_controller(network_click);

    let power_click = gtk::GestureClick::new();
    {
        let runtime_config = Arc::clone(&runtime_config);
        power_click.connect_pressed(move |_, _, _, _| {
            let config = runtime_config
                .lock()
                .expect("runtime config poisoned")
                .clone();
            run_configured_command(&config.power_menu_command, "power menu");
        });
    }
    power.add_controller(power_click);

    window.present();

    let runtime_clock_config = Arc::clone(&runtime_config);
    glib::timeout_add_seconds_local(1, move || {
        let config = runtime_clock_config
            .lock()
            .expect("runtime config poisoned")
            .clone();
        clock.set_text(&Local::now().format(&config.clock_format).to_string());
        glib::ControlFlow::Continue
    });

    let (sender, receiver): (mpsc::Sender<PanelUpdate>, mpsc::Receiver<PanelUpdate>) =
        mpsc::channel();
    let (status_sender, status_receiver): (mpsc::Sender<PanelStatus>, mpsc::Receiver<PanelStatus>) =
        mpsc::channel();

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

    thread::spawn({
        let runtime_config = Arc::clone(&runtime_config);
        move || {
            let collector = StatusCollector::new();
            let mut last_status = PanelStatus::default();

            if status_sender.send(last_status.clone()).is_err() {
                return;
            }

            loop {
                let next_status = collector.collect();
                if next_status.audio != last_status.audio
                    || next_status.network != last_status.network
                    || next_status.battery != last_status.battery
                {
                    if status_sender.send(next_status.clone()).is_err() {
                        break;
                    }
                    last_status = next_status;
                }

                let poll_interval_ms = runtime_config
                    .lock()
                    .expect("runtime config poisoned")
                    .status_poll_interval_ms
                    .max(100);
                thread::sleep(Duration::from_millis(poll_interval_ms));
            }
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
            let scheduled_render_for_timeout = Rc::clone(&scheduled_render);

            let source_id = glib::timeout_add_local_once(RENDER_DEBOUNCE, move || {
                scheduled_render_for_timeout.borrow_mut().take();

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
            audio.set_text(&status.audio);
            network.set_text(&status.network);
            battery.set_text(&status.battery);
        }

        glib::ControlFlow::Continue
    });

    glib::timeout_add_local(Duration::from_millis(200), move || {
        while let Ok(reason) = reload_rx.try_recv() {
            match Config::load() {
                Ok(config) => {
                    let old = runtime_config
                        .lock()
                        .expect("runtime config poisoned")
                        .clone();
                    let next = RuntimePanelConfig::from(&config.panel);

                    let mut applied = Vec::new();
                    let mut restart_required = Vec::new();

                    if old.clock_format != next.clock_format {
                        applied.push(format!(
                            "clock_format: {} -> {}",
                            old.clock_format, next.clock_format
                        ));
                    }
                    if old.status_poll_interval_ms != next.status_poll_interval_ms {
                        applied.push(format!(
                            "status_poll_interval_ms: {} -> {}",
                            old.status_poll_interval_ms, next.status_poll_interval_ms
                        ));
                    }
                    if old.audio_toggle_command != next.audio_toggle_command {
                        applied.push("audio_toggle_command updated".to_owned());
                    }
                    if old.audio_mixer_command != next.audio_mixer_command {
                        applied.push("audio_mixer_command updated".to_owned());
                    }
                    if old.network_settings_command != next.network_settings_command {
                        applied.push("network_settings_command updated".to_owned());
                    }
                    if old.power_menu_command != next.power_menu_command {
                        applied.push("power_menu_command updated".to_owned());
                    }

                    if panel_config.height != config.panel.height {
                        restart_required.push("height".to_owned());
                    }
                    if panel_config.margin_start != config.panel.margin_start {
                        restart_required.push("margin_start".to_owned());
                    }
                    if panel_config.margin_end != config.panel.margin_end {
                        restart_required.push("margin_end".to_owned());
                    }

                    *runtime_config.lock().expect("runtime config poisoned") = next;

                    tracing::info!(
                        trigger = reason.as_str(),
                        applied = if applied.is_empty() {
                            "none"
                        } else {
                            &applied.join(", ")
                        },
                        restart_required = if restart_required.is_empty() {
                            "none"
                        } else {
                            &restart_required.join(", ")
                        },
                        "panel config reload processed"
                    );
                }
                Err(error) => tracing::warn!(
                    ?error,
                    trigger = reason.as_str(),
                    "panel reload ignored due to config load error"
                ),
            }
        }

        glib::ControlFlow::Continue
    });
}

fn run_configured_command(command: &str, action: &str) {
    if command.trim().is_empty() {
        tracing::warn!(action, "configured command is empty; ignoring click action");
        return;
    }

    if let Err(error) = Command::new("sh").args(["-c", command]).spawn() {
        tracing::warn!(
            ?error,
            action,
            command,
            "failed to launch configured command"
        );
    }
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
