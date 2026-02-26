use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell as layer_shell;
use zbus::blocking::Connection;
use zbus::interface;
use zvariant::OwnedValue;

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const WINDOW_WIDTH: i32 = 360;

#[derive(Clone, Copy)]
enum Urgency {
    Normal,
    Critical,
}

impl Urgency {
    fn from_hints(hints: &HashMap<String, OwnedValue>) -> Self {
        let level = hints
            .get("urgency")
            .and_then(|value| u8::try_from(value.clone()).ok())
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
struct NotifyEvent {
    id: u32,
    summary: String,
    body: String,
    timeout_ms: u64,
    urgency: Urgency,
}

struct NotificationsService {
    next_id: Arc<AtomicU32>,
    sender: glib::Sender<NotifyEvent>,
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
        _actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id == 0 {
            self.next_id.fetch_add(1, Ordering::Relaxed)
        } else {
            replaces_id
        };

        let timeout_ms = if expire_timeout > 0 {
            expire_timeout as u64
        } else {
            DEFAULT_TIMEOUT_MS
        };

        let event = NotifyEvent {
            id,
            summary: summary.to_owned(),
            body: body.to_owned(),
            timeout_ms,
            urgency: Urgency::from_hints(&hints),
        };

        if let Err(error) = self.sender.send(event) {
            tracing::warn!(?error, "failed to deliver notification event to gtk thread");
        }

        id
    }

    fn close_notification(&self, id: u32) {
        tracing::debug!(
            notification_id = id,
            "CloseNotification not implemented in v0"
        );
    }

    fn get_capabilities(&self) -> Vec<String> {
        Vec::new()
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "vibeshell-notifd".to_owned(),
            "vibeshell".to_owned(),
            "0.1.0".to_owned(),
            "1.2".to_owned(),
        )
    }
}

fn main() {
    common::init_logging();
    tracing::info!(app = "notifd", "starting up");

    let app = gtk::Application::builder()
        .application_id("com.vibeshell.notifd")
        .build();

    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("vibeshell-notifd")
        .default_width(WINDOW_WIDTH)
        .build();

    window.set_decorated(false);
    window.set_resizable(false);

    layer_shell::init_for_window(&window);
    layer_shell::set_layer(&window, layer_shell::Layer::Overlay);
    layer_shell::set_anchor(&window, layer_shell::Edge::Top, true);
    layer_shell::set_anchor(&window, layer_shell::Edge::Right, true);
    layer_shell::set_margin(&window, layer_shell::Edge::Top, 12);
    layer_shell::set_margin(&window, layer_shell::Edge::Right, 12);

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

    let (sender, receiver) = glib::MainContext::channel::<NotifyEvent>(glib::Priority::DEFAULT);
    spawn_dbus_service(sender);

    receiver.attach(None, move |event| {
        add_card(&root, event);
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

            .notification-card.critical {
                border-color: #d64a4a;
            }

            .notification-summary {
                font-weight: 700;
            }
        "#,
    );

    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("display should exist"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn spawn_dbus_service(sender: glib::Sender<NotifyEvent>) {
    thread::spawn(move || {
        let next_id = Arc::new(AtomicU32::new(1));
        let service = NotificationsService { next_id, sender };

        let connection = match Connection::session() {
            Ok(connection) => connection,
            Err(error) => {
                tracing::error!(?error, "failed to connect to session bus");
                return;
            }
        };

        if let Err(error) = connection.request_name("org.freedesktop.Notifications") {
            tracing::error!(?error, "failed to request bus name");
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
            if let Err(error) = connection.process(Duration::from_millis(250)) {
                tracing::warn!(?error, "dbus process loop error");
            }
        }
    });
}

fn add_card(root: &gtk::Box, event: NotifyEvent) {
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

    root.prepend(&card);

    let card_for_close = card.clone();
    close_button.connect_clicked(move |_| {
        card_for_close.unparent();
    });

    tracing::debug!(
        notification_id = event.id,
        timeout_ms = event.timeout_ms,
        "rendered notification"
    );

    let card_for_timeout = card.clone();
    glib::timeout_add_local_once(Duration::from_millis(event.timeout_ms), move || {
        card_for_timeout.unparent();
    });
}
