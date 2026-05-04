//! Shared GTK4 theme for vibeshell's GTK apps (panel, launcher, notifd,
//! overlay, cheatsheet).
//!
//! Single entry point: `install_theme(&display)`. Each app calls it once
//! at startup, before constructing widgets. The default stylesheet is
//! bundled via `include_str!` and registered at
//! `STYLE_PROVIDER_PRIORITY_APPLICATION`. If `~/.config/vibeshell/theme.css`
//! exists, it loads next at `STYLE_PROVIDER_PRIORITY_USER` and overrides
//! the bundled defaults rule-by-rule.
//!
//! Why a shared crate (not per-app CSS): without this, each GTK app
//! inherits whatever Adwaita theme the host system runs, so launcher
//! could look light-themed while panel reads dark — the seams between
//! components show through. One stylesheet keeps the shell visually
//! coherent.
//!
//! Class naming convention: `.vibeshell-<app>-<element>` for per-app
//! widgets (e.g. `.vibeshell-launcher-panel`), unprefixed semantic tokens
//! for theme-wide concepts (`.dim-label`, `.heading` already provided by
//! Adwaita are reused as-is). Apps add classes via
//! `widget.add_css_class("vibeshell-launcher-panel")` so the stylesheet
//! can target them precisely.

use gtk4::gdk;
use gtk4::CssProvider;

/// The bundled stylesheet. Edit `crates/gtk-theme/style.css` to retheme.
const BUNDLED_CSS: &str = include_str!("../style.css");

/// Install the bundled theme on `display`, then layer the user override
/// (if present) on top. Idempotent — calling twice just adds two
/// providers, GTK dedupes at lookup time. Each GTK app should call this
/// exactly once at startup, immediately after `Application::activate`.
pub fn install_theme(display: &gdk::Display) {
    install_bundled(display);
    install_user_override(display);
}

fn install_bundled(display: &gdk::Display) {
    let provider = CssProvider::new();
    provider.load_from_data(BUNDLED_CSS);
    gtk4::style_context_add_provider_for_display(
        display,
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    tracing::debug!("gtk-theme: bundled stylesheet installed");
}

fn install_user_override(display: &gdk::Display) {
    let path = match user_theme_path() {
        Some(p) => p,
        None => return,
    };
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!(?e, path = %path.display(), "gtk-theme: read user CSS failed");
            return;
        }
    };
    let provider = CssProvider::new();
    provider.load_from_data(&data);
    gtk4::style_context_add_provider_for_display(
        display,
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_USER,
    );
    tracing::info!(path = %path.display(), "gtk-theme: user override loaded");
}

/// Resolve `~/.config/vibeshell/theme.css` honoring `XDG_CONFIG_HOME`.
/// `VIBESHELL_THEME_CSS` env override wins outright (tests / dev).
fn user_theme_path() -> Option<std::path::PathBuf> {
    if let Ok(custom) = std::env::var("VIBESHELL_THEME_CSS") {
        return Some(std::path::PathBuf::from(custom));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("vibeshell").join("theme.css"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_css_is_non_empty() {
        // If someone empties style.css the apps still load — but we'd
        // silently lose the theme. Catch that here.
        assert!(BUNDLED_CSS.len() > 100);
        // Spot-check a key class so a stray rename doesn't go unnoticed.
        assert!(BUNDLED_CSS.contains(".vibeshell-launcher-panel"));
    }

    #[test]
    fn env_override_takes_precedence() {
        // SAFETY: pre-test, single-threaded.
        std::env::set_var("VIBESHELL_THEME_CSS", "/tmp/vibeshell-test-theme.css");
        assert_eq!(
            user_theme_path(),
            Some(std::path::PathBuf::from("/tmp/vibeshell-test-theme.css"))
        );
        std::env::remove_var("VIBESHELL_THEME_CSS");
    }
}
