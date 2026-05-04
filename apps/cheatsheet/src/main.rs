//! Keyboard-shortcut cheat-sheet overlay.
//!
//! Parses `dev/sway.bindings.generated` (or `$VIBESHELL_BINDINGS_FILE`) and
//! renders a modal GTK window listing every binding grouped by inferred
//! category. Meant to be triggered by a single keybinding (`$mod+slash` by
//! default) and dismissed with `Escape`.

use std::env;
use std::fs;
use std::path::PathBuf;

use adw::prelude::*;
use gtk::gdk;
use gtk::glib;
use gtk4 as gtk;
use gtk4_layer_shell::{self as layer_shell, LayerShell};

const APP_ID: &str = "com.vibeshell.cheatsheet";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Category {
    Navigation,
    Move,
    Clusters,
    Shell,
    System,
    Session,
    Other,
}

impl Category {
    fn label(self) -> &'static str {
        match self {
            Self::Navigation => "Navigation",
            Self::Move => "Move windows",
            Self::Clusters => "Clusters",
            Self::Shell => "Shell",
            Self::System => "System",
            Self::Session => "Session",
            Self::Other => "Other",
        }
    }

    /// Display order of the groups.
    fn display_order(self) -> u8 {
        match self {
            Self::Navigation => 0,
            Self::Clusters => 1,
            Self::Move => 2,
            Self::Shell => 3,
            Self::System => 4,
            Self::Session => 5,
            Self::Other => 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    key: String,
    command: String,
    category: Category,
}

fn main() {
    common::init_logging("cheatsheet");
    tracing::info!(app = "cheatsheet", "starting up");

    let bindings = load_bindings();

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| {
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk_theme::install_theme(&display);
        }
        build_ui(app, bindings.clone());
    });
    app.run();
}

fn load_bindings() -> Vec<Binding> {
    let path = resolve_bindings_path();
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(error) => {
            tracing::warn!(
                ?error,
                path = %path.display(),
                "failed to read bindings file; cheatsheet will be empty"
            );
            return Vec::new();
        }
    };
    let mut bindings = parse_bindings(&content);
    bindings.sort_by(|a, b| {
        a.category
            .display_order()
            .cmp(&b.category.display_order())
            .then_with(|| a.key.cmp(&b.key))
    });
    bindings
}

fn resolve_bindings_path() -> PathBuf {
    if let Some(explicit) = env::var_os("VIBESHELL_BINDINGS_FILE") {
        return PathBuf::from(explicit);
    }
    if let Some(home) = env::var_os("HOME") {
        let candidate = PathBuf::from(home).join(".config/vibeshell/sway.bindings.generated");
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from("dev/sway.bindings.generated")
}

/// Extracts `(key, command)` pairs from lines matching `bindsym <key> exec <command>`.
fn parse_bindings(contents: &str) -> Vec<Binding> {
    contents
        .lines()
        .filter_map(parse_binding_line)
        .map(|(key, command)| Binding {
            category: categorize(&command),
            key,
            command,
        })
        .collect()
}

fn parse_binding_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    let after_bindsym = trimmed.strip_prefix("bindsym")?.trim_start();
    let (key, rest) = after_bindsym.split_once(char::is_whitespace)?;
    let command = rest.trim_start().strip_prefix("exec")?.trim_start();
    if key.is_empty() || command.is_empty() {
        return None;
    }
    Some((key.to_owned(), command.to_owned()))
}

/// Heuristic category inference from the command string.
fn categorize(command: &str) -> Category {
    let lower = command.to_lowercase();
    if lower.contains("zoom-") || lower.contains("cycle-") {
        return Category::Navigation;
    }
    if lower.contains("keyboard-move") {
        return Category::Move;
    }
    if lower.contains("cluster") {
        return Category::Clusters;
    }
    if lower.contains("com.vibeshell.launcher") || lower.contains(" launcher") {
        return Category::Shell;
    }
    if lower.contains("wpctl")
        || lower.contains("brightnessctl")
        || lower.contains("grim")
        || lower.contains("wl-copy")
    {
        return Category::System;
    }
    if lower.contains("swaymsg exit") || lower.contains("swaymsg reload") {
        return Category::Session;
    }
    Category::Other
}

fn build_ui(app: &adw::Application, bindings: Vec<Binding>) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("vibeshell-cheatsheet")
        .default_width(520)
        .default_height(620)
        .build();
    window.add_css_class("vibeshell-cheatsheet-window");

    // Prefer layer-shell overlay so it sits above everything; fall back to a
    // regular floating window if the compositor doesn't support layer-shell.
    if layer_shell::is_supported() {
        window.init_layer_shell();
        window.set_layer(layer_shell::Layer::Overlay);
        window.set_keyboard_mode(layer_shell::KeyboardMode::OnDemand);
        // Centered: anchoring to nothing lets the compositor place it.
    } else {
        window.set_decorated(false);
        window.set_resizable(false);
    }

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(20)
        .margin_end(20)
        .build();
    root.add_css_class("vibeshell-cheatsheet-panel");

    let title = gtk::Label::new(Some("Keyboard shortcuts"));
    title.add_css_class("title-2");
    title.add_css_class("vibeshell-cheatsheet-title");
    title.set_halign(gtk::Align::Start);
    root.append(&title);

    let hint = gtk::Label::new(Some("Esc to close"));
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");
    hint.add_css_class("vibeshell-cheatsheet-hint");
    hint.set_halign(gtk::Align::Start);
    root.append(&hint);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .build();

    let list = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();

    if bindings.is_empty() {
        let empty = gtk::Label::new(Some(
            "No bindings found. Set VIBESHELL_BINDINGS_FILE \
             or generate dev/sway.bindings.generated first.",
        ));
        empty.add_css_class("dim-label");
        empty.set_halign(gtk::Align::Start);
        empty.set_wrap(true);
        list.append(&empty);
    } else {
        render_bindings(&list, &bindings);
    }

    scroller.set_child(Some(&list));
    root.append(&scroller);

    window.set_content(Some(&root));

    let key_controller = gtk::EventControllerKey::new();
    let window_for_keys = window.clone();
    let app_for_keys = app.clone();
    key_controller.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            window_for_keys.close();
            app_for_keys.quit();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key_controller);

    window.present();
}

fn render_bindings(list: &gtk::Box, bindings: &[Binding]) {
    let mut current_category: Option<Category> = None;
    for binding in bindings {
        if current_category != Some(binding.category) {
            current_category = Some(binding.category);
            let header = gtk::Label::new(Some(binding.category.label()));
            header.add_css_class("heading");
            header.add_css_class("vibeshell-cheatsheet-section-header");
            header.set_halign(gtk::Align::Start);
            header.set_margin_top(12);
            list.append(&header);
        }

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .build();
        row.add_css_class("vibeshell-cheatsheet-row");

        let key_label = gtk::Label::new(Some(&humanize_key(&binding.key)));
        key_label.add_css_class("monospace");
        key_label.add_css_class("vibeshell-cheatsheet-key");
        key_label.set_xalign(0.0);
        key_label.set_width_chars(24);

        let desc_label = gtk::Label::new(Some(&describe_command(&binding.command)));
        desc_label.add_css_class("vibeshell-cheatsheet-desc");
        desc_label.set_xalign(0.0);
        desc_label.set_hexpand(true);
        desc_label.set_wrap(true);

        row.append(&key_label);
        row.append(&desc_label);
        list.append(&row);
    }
}

/// Makes `$mod+Shift+Up` a bit friendlier on the eye — purely cosmetic.
fn humanize_key(key: &str) -> String {
    key.replace("$mod", "Super")
}

/// Best-effort description from the command. Many commands are
/// `vibeshellctl ipc <thing>`; strip the prefix.
fn describe_command(command: &str) -> String {
    if let Some(rest) = command.strip_prefix("vibeshellctl ipc ") {
        return format!("vibeshell: {}", rest.replace('-', " "));
    }
    if let Some(rest) = command.strip_prefix("swaymsg ") {
        return format!("sway: {rest}");
    }
    command.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_binding() {
        let line = "bindsym $mod+equal exec vibeshellctl ipc zoom-in-mode";
        let parsed = parse_binding_line(line).expect("should parse");
        assert_eq!(parsed.0, "$mod+equal");
        assert_eq!(parsed.1, "vibeshellctl ipc zoom-in-mode");
    }

    #[test]
    fn parse_binding_preserves_multi_word_command() {
        let line = r#"bindsym Print exec grim -g "$(slurp)" - | wl-copy"#;
        let (key, command) = parse_binding_line(line).expect("should parse");
        assert_eq!(key, "Print");
        assert!(command.contains("grim"));
        assert!(command.contains("wl-copy"));
    }

    #[test]
    fn parse_skips_comments_and_blank_lines() {
        let text = "\
# comment line

bindsym $mod+equal exec vibeshellctl ipc zoom-in-mode
# another comment
bindsym $mod+minus exec vibeshellctl ipc zoom-out-mode
";
        let parsed = parse_bindings(text);
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn parse_rejects_lines_without_exec() {
        assert!(parse_binding_line("bindsym $mod+q focus").is_none());
        assert!(parse_binding_line("random line").is_none());
        assert!(parse_binding_line("").is_none());
    }

    #[test]
    fn categorize_routes_zoom_and_cycle_to_navigation() {
        assert_eq!(
            categorize("vibeshellctl ipc zoom-in-mode"),
            Category::Navigation
        );
        assert_eq!(
            categorize("vibeshellctl ipc cycle-cluster --direction forward"),
            Category::Navigation
        );
    }

    #[test]
    fn categorize_routes_keyboard_move_to_move() {
        assert_eq!(
            categorize("vibeshellctl ipc keyboard-move-by --dx 96 --dy 0"),
            Category::Move
        );
        assert_eq!(
            categorize("vibeshellctl ipc commit-keyboard-move"),
            Category::Move
        );
    }

    #[test]
    fn categorize_routes_media_keys_to_system() {
        assert_eq!(
            categorize("wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+"),
            Category::System
        );
        assert_eq!(categorize("brightnessctl set +10%"), Category::System);
        assert_eq!(
            categorize(r#"grim -g "$(slurp)" - | wl-copy"#),
            Category::System
        );
    }

    #[test]
    fn categorize_routes_swaymsg_exit_to_session() {
        assert_eq!(categorize("swaymsg exit"), Category::Session);
        assert_eq!(categorize("swaymsg reload"), Category::Session);
    }

    #[test]
    fn categorize_falls_back_to_other() {
        assert_eq!(categorize("some custom command"), Category::Other);
    }

    #[test]
    fn humanize_key_replaces_mod_token() {
        assert_eq!(humanize_key("$mod+Shift+Up"), "Super+Shift+Up");
        assert_eq!(humanize_key("XF86AudioMute"), "XF86AudioMute");
    }

    #[test]
    fn describe_command_strips_known_prefixes() {
        assert_eq!(
            describe_command("vibeshellctl ipc zoom-in-mode"),
            "vibeshell: zoom in mode"
        );
        assert_eq!(describe_command("swaymsg exit"), "sway: exit");
        assert_eq!(describe_command("custom"), "custom");
    }

    #[test]
    fn full_bindings_file_parses_and_categorizes() {
        // Mirrors the shape of dev/sway.bindings.generated.
        let text = "\
# header
bindsym $mod+space exec swaymsg '[app_id=\"com.vibeshell.launcher\"] kill' || launcher
bindsym Print exec grim -g \"$(slurp)\" - | wl-copy
bindsym XF86AudioMute exec wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle
bindsym $mod+equal exec vibeshellctl ipc zoom-in-mode
bindsym $mod+Shift+e exec swaymsg exit
bindsym $mod+Shift+Up exec vibeshellctl ipc keyboard-move-by --dx 0 --dy -96
";
        let bindings = parse_bindings(text);
        assert_eq!(bindings.len(), 6);

        let categories: Vec<Category> = bindings.iter().map(|b| b.category).collect();
        assert!(categories.contains(&Category::Shell));
        assert!(categories.contains(&Category::System));
        assert!(categories.contains(&Category::Navigation));
        assert!(categories.contains(&Category::Session));
        assert!(categories.contains(&Category::Move));
    }
}
