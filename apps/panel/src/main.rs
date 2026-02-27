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
use config::{CommandsConfig, Config, PanelConfig};
use gtk::glib;
use gtk4 as gtk;
use gtk4_layer_shell::{self as layer_shell, LayerShell};
use status::{PanelStatus, StatusCollector};

#[derive(Clone)]
struct RuntimePanelConfig {
    clock_format: String,
    status_poll_interval_ms: u64,
    sway_event_debounce_ms: u64,
    audio_toggle_command: String,
    audio_mixer_command: Option<String>,
    network_settings_command: String,
    power_menu_command: String,
}

impl RuntimePanelConfig {
    fn from_sections(panel: &PanelConfig, commands: &CommandsConfig) -> Self {
        Self {
            clock_format: panel.clock_format.clone(),
            status_poll_interval_ms: panel.status_poll_interval_ms,
            sway_event_debounce_ms: panel.sway_event_debounce_ms,
            audio_toggle_command: commands.volume.toggle_mute.clone(),
            audio_mixer_command: commands.volume.mixer.clone(),
            network_settings_command: panel.network_settings_command.clone(),
            power_menu_command: commands.power.menu.clone(),
        }
    }
}
use sway::{PanelState, PanelUpdate, WorkspaceState};

const SWAY_CONNECT_INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const SWAY_CONNECT_MAX_BACKOFF: Duration = Duration::from_secs(10);

fn report_config_load_error(error: &config::ConfigLoadError) {
    tracing::warn!(%error, "failed to load config, using defaults");
    if let Some(issues) = error.validation_issues() {
        for issue in issues {
            tracing::warn!(field = %issue.field, message = %issue.message, "config validation issue");
        }
    }
}

fn main() {
    common::init_logging("panel");
    tracing::info!(app = "panel", "starting up");

    let loaded = Config::load().unwrap_or_else(|error| {
        report_config_load_error(&error);
        Config::default()
    });
    let panel_config = loaded.panel.clone();
    let runtime_panel_config = RuntimePanelConfig::from_sections(&loaded.panel, &loaded.commands);

    let (_, reload_rx) = common::spawn_reload_listener();
    let reload_rx = Rc::new(RefCell::new(Some(reload_rx)));

    let app = adw::Application::builder()
        .application_id("com.vibeshell.panel")
        .build();

    app.connect_activate(move |app| {
        let Some(reload_rx) = reload_rx.borrow_mut().take() else {
            tracing::warn!("activate handler called more than once; panel UI already initialized");
            return;
        };

        build_ui(
            app,
            panel_config.clone(),
            runtime_panel_config.clone(),
            reload_rx,
        )
    });
    app.run();
}

fn build_ui(
    app: &adw::Application,
    panel_config: PanelConfig,
    initial_runtime_config: RuntimePanelConfig,
    reload_rx: mpsc::Receiver<common::ReloadReason>,
) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("vibeshell-panel")
        .default_height(panel_config.height)
        .build();

    window.set_size_request(-1, panel_config.height);

    let runtime_config = Arc::new(Mutex::new(initial_runtime_config));

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

    thread::spawn({
        let runtime_config = Arc::clone(&runtime_config);
        move || {
            let mut backoff = SWAY_CONNECT_INITIAL_BACKOFF;

            loop {
                match sway::SwayClient::connect() {
                    Ok(client) => {
                        let debounce_ms = runtime_config
                            .lock()
                            .expect("runtime config poisoned")
                            .sway_event_debounce_ms
                            .max(20);
                        tracing::info!(debounce_ms, "connected to sway ipc");
                        if let Err(error) =
                            client.run_listener(sender.clone(), Duration::from_millis(debounce_ms))
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

    let last_rendered = Rc::new(RefCell::new(None::<PanelState>));

    glib::timeout_add_local(Duration::from_millis(16), move || {
        while let Ok(update) = receiver.try_recv() {
            let PanelUpdate::Snapshot(next_state) = update;

            if last_rendered
                .borrow()
                .as_ref()
                .is_some_and(|rendered| rendered == &next_state)
            {
                continue;
            }

            apply_state(&workspaces, &title, &next_state);
            last_rendered.replace(Some(next_state));
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
                    let next = RuntimePanelConfig::from_sections(&config.panel, &config.commands);

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
                    if old.sway_event_debounce_ms != next.sway_event_debounce_ms {
                        applied.push(format!(
                            "sway_event_debounce_ms: {} -> {}",
                            old.sway_event_debounce_ms, next.sway_event_debounce_ms
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

                    let applied_summary = if applied.is_empty() {
                        "none".to_owned()
                    } else {
                        applied.join(", ")
                    };
                    let restart_required_summary = if restart_required.is_empty() {
                        "none".to_owned()
                    } else {
                        restart_required.join(", ")
                    };

                    tracing::info!(
                        trigger = reason.as_str(),
                        applied = applied_summary,
                        restart_required = restart_required_summary,
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
