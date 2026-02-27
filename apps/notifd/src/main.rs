use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use config::{Config, NotificationsConfig};
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell::{self as layer_shell, LayerShell};
use zbus::blocking::Connection;
use zbus::interface;
use zvariant::OwnedValue;

const CLOSE_REASON_EXPIRED: u32 = 1;
const CLOSE_REASON_DISMISSED: u32 = 2;
const CLOSE_REASON_CLOSED_BY_CALL: u32 = 3;
const DEFAULT_MAX_ON_SCREEN: usize = 4;
const MAX_QUEUE_DEPTH: usize = 32;

#[derive(Clone, Copy)]
enum Urgency {
    Normal,
    Critical,
}

impl Urgency {
    fn from_hints(hints: &HashMap<String, OwnedValue>) -> Self {
        let level = hints
            .get("urgency")
            .and_then(|value| u8::try_from(value).ok())
            .unwrap_or(1);

        if level >= 2 {
            Self::Critical
        } else {
            Self::Normal
        }
    }

    fn css_class(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Critical => "critical",
        }
    }
}

#[derive(Clone)]
struct NotificationAction {
    key: String,
    label: String,
}

#[derive(Clone)]
struct NotifyEvent {
    id: u32,
    summary: String,
    body: String,
    timeout_ms: Option<u64>,
    urgency: Urgency,
    actions: Vec<NotificationAction>,
}

enum UiEvent {
    Notify(NotifyEvent),
    Close { id: u32, reason: u32 },
}

enum DbusEvent {
    NotificationClosed { id: u32, reason: u32 },
    ActionInvoked { id: u32, action_key: String },
}

#[derive(Default)]
struct UiState {
    root: Option<gtk::Box>,
    cards: HashMap<u32, gtk::Box>,
    generations: HashMap<u32, u64>,
    queue: VecDeque<NotifyEvent>,
    max_on_screen: usize,
    dbus_sender: Option<mpsc::Sender<DbusEvent>>,
}

struct NotificationsService {
    next_id: Arc<AtomicU32>,
    sender: mpsc::Sender<UiEvent>,
    default_timeout_ms: Arc<AtomicU64>,
}

#[interface(name = "org.freedesktop.Notifications")]
impl NotificationsService {
    fn notify(
        &self,
        _app_name: &str,
        replaces_id: u32,
        _app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id == 0 {
            self.next_id.fetch_add(1, Ordering::Relaxed)
        } else {
            replaces_id
        };

        let timeout_ms = timeout_from_request(
            expire_timeout,
            self.default_timeout_ms.load(Ordering::Relaxed),
            Urgency::from_hints(&hints),
        );

        let event = NotifyEvent {
            id,
            summary: summary.to_owned(),
            body: body.to_owned(),
            timeout_ms,
            urgency: Urgency::from_hints(&hints),
            actions: parse_actions(actions),
        };

        if let Err(error) = self.sender.send(UiEvent::Notify(event)) {
            tracing::warn!(?error, "failed to deliver notification event to gtk thread");
        }

        id
    }

    fn close_notification(&self, id: u32) {
        if let Err(error) = self.sender.send(UiEvent::Close {
            id,
            reason: CLOSE_REASON_CLOSED_BY_CALL,
        }) {
            tracing::warn!(?error, notification_id = id, "failed to close notification");
        }
    }

    #[zbus(signal)]
    fn notification_closed(
        signal_ctxt: &zbus::SignalContext<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    fn action_invoked(
        signal_ctxt: &zbus::SignalContext<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;

    fn get_capabilities(&self) -> Vec<String> {
        vec![
            "body".to_owned(),
            "body-markup".to_owned(),
            "icon-static".to_owned(),
            "actions".to_owned(),
            "persistence".to_owned(),
        ]
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "vibeshell-notifd".to_owned(),
            "vibeshell".to_owned(),
            env!("CARGO_PKG_VERSION").to_owned(),
            "1.2".to_owned(),
        )
    }
}

fn main() {
    common::init_logging("notifd");
    tracing::info!(app = "notifd", "starting up");

    let notifd_config = Config::load()
        .unwrap_or_else(|error| {
            tracing::warn!(?error, "failed to load config, using defaults");
            Config::default()
        })
        .notifications;

    let (_, reload_rx) = common::spawn_reload_listener();

    let app = gtk::Application::builder()
        .application_id("com.vibeshell.notifd")
        .build();

    app.connect_activate(move |app| build_ui(app, notifd_config.clone(), reload_rx));
    app.run();
}

fn build_ui(
    app: &gtk::Application,
    notifd_config: NotificationsConfig,
    reload_rx: mpsc::Receiver<common::ReloadReason>,
) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("vibeshell-notifd")
        .default_width(notifd_config.width)
        .build();

    window.set_resizable(false);

    if layer_shell::is_supported() {
        window.set_decorated(false);
        window.init_layer_shell();
        window.set_layer(layer_shell::Layer::Overlay);
        window.set_anchor(layer_shell::Edge::Top, true);
        window.set_anchor(layer_shell::Edge::Right, true);
        window.set_margin(layer_shell::Edge::Top, notifd_config.margin_top);
        window.set_margin(layer_shell::Edge::Right, notifd_config.margin_right);
    } else {
        tracing::warn!("layer shell protocol unavailable; falling back to a regular GTK window");
        eprintln!(
            "notifd: compositor does not support zwlr_layer_shell_v1; using regular window mode."
        );
    }

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .build();

    window.set_child(Some(&root));
    install_css();
    window.present();

    let default_timeout_ms = Arc::new(AtomicU64::new(notifd_config.default_timeout_ms));
    let (sender, receiver) = mpsc::channel::<UiEvent>();
    let dbus_sender = spawn_dbus_service(sender, Arc::clone(&default_timeout_ms));

    let state = Rc::new(std::cell::RefCell::new(UiState::default()));
    {
        let mut state = state.borrow_mut();
        state.root = Some(root.clone());
        state.max_on_screen = DEFAULT_MAX_ON_SCREEN;
        state.dbus_sender = Some(dbus_sender);
    }

    let state_for_events = Rc::clone(&state);
    glib::timeout_add_local(Duration::from_millis(16), move || {
        for event in receiver.try_iter() {
            match event {
                UiEvent::Notify(event) => enqueue_or_show(&state_for_events, event),
                UiEvent::Close { id, reason } => close_card(&state_for_events, id, reason),
            }
        }

        glib::ControlFlow::Continue
    });

    glib::timeout_add_local(Duration::from_millis(200), move || {
        while let Ok(reason) = reload_rx.try_recv() {
            match Config::load() {
                Ok(config) => {
                    let next = config.notifications;
                    let mut applied = Vec::new();
                    let mut restart_required = Vec::new();

                    let current_timeout = default_timeout_ms.load(Ordering::Relaxed);
                    if current_timeout != next.default_timeout_ms {
                        applied.push(format!(
                            "default_timeout_ms: {} -> {}",
                            current_timeout, next.default_timeout_ms
                        ));
                        default_timeout_ms.store(next.default_timeout_ms, Ordering::Relaxed);
                    }

                    if notifd_config.width != next.width {
                        applied.push(format!("width: {} -> {}", notifd_config.width, next.width));
                        window.set_default_width(next.width);
                    }
                    if notifd_config.margin_top != next.margin_top {
                        applied.push(format!(
                            "margin_top: {} -> {}",
                            notifd_config.margin_top, next.margin_top
                        ));
                        window.set_margin(layer_shell::Edge::Top, next.margin_top);
                    }
                    if notifd_config.margin_right != next.margin_right {
                        applied.push(format!(
                            "margin_right: {} -> {}",
                            notifd_config.margin_right, next.margin_right
                        ));
                        window.set_margin(layer_shell::Edge::Right, next.margin_right);
                    }

                    if !window.is_visible() {
                        restart_required.push("window not visible".to_owned());
                    }

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
                        "notifd config reload processed"
                    );
                }
                Err(error) => tracing::warn!(
                    ?error,
                    trigger = reason.as_str(),
                    "notifd reload ignored due to config load error"
                ),
            }
        }

        glib::ControlFlow::Continue
    });
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        r#"
            .notification-card {
                background: alpha(@theme_bg_color, 0.95);
                border-radius: 12px;
                padding: 12px;
                border: 1px solid alpha(@theme_fg_color, 0.18);
            }

            .notification-card.normal {
                border-color: alpha(@theme_fg_color, 0.18);
            }

            .notification-card.critical {
                border-color: #d64a4a;
                background: alpha(#3d1111, 0.92);
            }

            .notification-summary {
                font-weight: 700;
            }
        "#,
    );

    let Some(display) = gtk::gdk::Display::default() else {
        tracing::error!("no GTK display available; run notifd inside a Wayland session");
        eprintln!("notifd: no display available. Run this inside a graphical Wayland session.");
        return;
    };

    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn spawn_dbus_service(
    sender: mpsc::Sender<UiEvent>,
    default_timeout_ms: Arc<AtomicU64>,
) -> mpsc::Sender<DbusEvent> {
    let (dbus_sender, dbus_receiver) = mpsc::channel::<DbusEvent>();

    thread::spawn(move || {
        let next_id = Arc::new(AtomicU32::new(1));
        let service = NotificationsService {
            next_id,
            sender,
            default_timeout_ms,
        };

        let connection = match Connection::session() {
            Ok(connection) => connection,
            Err(error) => {
                tracing::error!(?error, "failed to connect to session bus");
                return;
            }
        };

        if let Err(error) = connection.request_name("org.freedesktop.Notifications") {
            let details = error.to_string();
            let likely_conflict = details.contains("NameExists")
                || details.contains("NameTaken")
                || details.contains("AlreadyOwner");

            if likely_conflict {
                tracing::error!(
                    ?error,
                    "failed to acquire org.freedesktop.Notifications; another notification daemon may already be running"
                );
                eprintln!(
                    "notifd: org.freedesktop.Notifications is already owned. Stop the other notification daemon (for example mako/dunst) and retry."
                );
            } else {
                tracing::error!(?error, "failed to request bus name");
                eprintln!("notifd: failed to acquire org.freedesktop.Notifications on D-Bus.");
            }
            return;
        }

        if let Err(error) = connection
            .object_server()
            .at("/org/freedesktop/Notifications", service)
        {
            tracing::error!(?error, "failed to register notifications interface");
            return;
        }

        tracing::info!("notification dbus interface ready");

        loop {
            while let Ok(event) = dbus_receiver.try_recv() {
                let signal_ctxt =
                    zbus::SignalContext::new(&connection, "/org/freedesktop/Notifications")
                        .expect("valid signal path");

                match event {
                    DbusEvent::NotificationClosed { id, reason } => {
                        if let Err(error) =
                            NotificationsService::notification_closed(&signal_ctxt, id, reason)
                        {
                            tracing::warn!(
                                ?error,
                                notification_id = id,
                                "failed to emit NotificationClosed"
                            );
                        }
                    }
                    DbusEvent::ActionInvoked { id, action_key } => {
                        if let Err(error) =
                            NotificationsService::action_invoked(&signal_ctxt, id, &action_key)
                        {
                            tracing::warn!(
                                ?error,
                                notification_id = id,
                                action_key,
                                "failed to emit ActionInvoked"
                            );
                        }
                    }
                }
            }

            thread::park_timeout(Duration::from_millis(100));
        }
    });

    dbus_sender
}

fn enqueue_or_show(state: &Rc<std::cell::RefCell<UiState>>, event: NotifyEvent) {
    close_card(state, event.id, CLOSE_REASON_CLOSED_BY_CALL);

    {
        let mut state = state.borrow_mut();
        if let Some(index) = state.queue.iter().position(|queued| queued.id == event.id) {
            state.queue.remove(index);
        }

        if state.cards.len() >= state.max_on_screen {
            state.queue.push_back(event);
            if state.queue.len() > MAX_QUEUE_DEPTH {
                state.queue.pop_front();
            }
            return;
        }
    }

    add_card(state, event);
}

fn add_card(state: &Rc<std::cell::RefCell<UiState>>, event: NotifyEvent) {
    let root = {
        let state_ref = state.borrow();
        state_ref
            .root
            .clone()
            .expect("notification root should be initialized")
    };

    let id = event.id;
    let actions = event.actions;

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    card.add_css_class("notification-card");
    card.add_css_class(event.urgency.css_class());

    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();

    let summary = gtk::Label::new(Some(&event.summary));
    summary.set_halign(gtk::Align::Start);
    summary.set_xalign(0.0);
    summary.set_hexpand(true);
    summary.set_wrap(true);
    summary.add_css_class("notification-summary");

    let close_button = gtk::Button::from_icon_name("window-close-symbolic");
    close_button.add_css_class("flat");
    close_button.set_valign(gtk::Align::Start);

    header.append(&summary);
    header.append(&close_button);
    card.append(&header);

    if !event.body.is_empty() {
        let body = gtk::Label::new(Some(&event.body));
        body.set_halign(gtk::Align::Start);
        body.set_xalign(0.0);
        body.set_wrap(true);
        card.append(&body);
    }

    if !actions.is_empty() {
        let actions_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();

        for action in actions {
            let button = gtk::Button::with_label(&action.label);
            let action_key = action.key;
            let state_for_action = Rc::clone(state);
            button.connect_clicked(move |_| {
                emit_dbus_event(
                    &state_for_action,
                    DbusEvent::ActionInvoked {
                        id,
                        action_key: action_key.clone(),
                    },
                );
                close_card(&state_for_action, id, CLOSE_REASON_DISMISSED);
            });
            actions_row.append(&button);
        }

        card.append(&actions_row);
    }

    root.prepend(&card);

    let generation = {
        let mut state = state.borrow_mut();
        state.cards.insert(id, card.clone());
        let generation = state.generations.entry(id).or_insert(0);
        *generation += 1;
        *generation
    };

    let state_for_close = Rc::clone(state);
    close_button.connect_clicked(move |_| {
        close_card(&state_for_close, id, CLOSE_REASON_DISMISSED);
    });

    if let Some(timeout_ms) = event.timeout_ms {
        let state_for_timeout = Rc::clone(state);
        glib::timeout_add_local_once(Duration::from_millis(timeout_ms), move || {
            let current_generation = state_for_timeout.borrow().generations.get(&id).copied();

            if current_generation == Some(generation) {
                close_card(&state_for_timeout, id, CLOSE_REASON_EXPIRED);
            }
        });
    }
}

fn close_card(state: &Rc<std::cell::RefCell<UiState>>, id: u32, reason: u32) {
    let (card, next, removed_queued) = {
        let mut state = state.borrow_mut();
        state.generations.remove(&id);
        let card = state.cards.remove(&id);

        let removed_queued = if card.is_none() {
            if let Some(index) = state.queue.iter().position(|queued| queued.id == id) {
                state.queue.remove(index);
                true
            } else {
                false
            }
        } else {
            false
        };

        let next = if card.is_some() {
            state.queue.pop_front()
        } else {
            None
        };

        (card, next, removed_queued)
    };

    if let Some(card) = card {
        card.unparent();
        emit_dbus_event(state, DbusEvent::NotificationClosed { id, reason });
    } else if removed_queued {
        emit_dbus_event(state, DbusEvent::NotificationClosed { id, reason });
    }

    if let Some(next) = next {
        add_card(state, next);
    }
}

fn emit_dbus_event(state: &Rc<std::cell::RefCell<UiState>>, event: DbusEvent) {
    if let Some(sender) = state.borrow().dbus_sender.as_ref() {
        if let Err(error) = sender.send(event) {
            tracing::warn!(?error, "failed to send dbus signal event");
        }
    }
}

fn parse_actions(actions: Vec<String>) -> Vec<NotificationAction> {
    actions
        .chunks_exact(2)
        .map(|chunk| NotificationAction {
            key: chunk[0].clone(),
            label: chunk[1].clone(),
        })
        .collect()
}

fn timeout_from_request(
    expire_timeout: i32,
    default_timeout_ms: u64,
    urgency: Urgency,
) -> Option<u64> {
    if expire_timeout > 0 {
        return Some(expire_timeout as u64);
    }

    if expire_timeout == 0 {
        return None;
    }

    match urgency {
        Urgency::Critical => None,
        Urgency::Normal => Some(default_timeout_ms),
    }
}
