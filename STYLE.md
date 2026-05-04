# vibeshell visual style guide

This is the canonical reference for the vibeshell color palette and
typography. Use it when adding a new GTK app or theming a third-party
launcher / panel / notification daemon to match the rest of the shell.

The full GTK4 stylesheet lives at `crates/gtk-theme/style.css`; user
overrides go in `~/.config/vibeshell/theme.css` (see
`crates/gtk-theme/src/lib.rs::install_theme`).

## Palette

| Token                       | Value                              | Usage                                                   |
|-----------------------------|------------------------------------|---------------------------------------------------------|
| `vibeshell_bg`              | `rgba(13, 13, 18, 0.94)`           | Panel/window background. Mirrors vibewm's clear color.  |
| `vibeshell_bg_lift`         | `rgba(255, 255, 255, 0.04)`        | Inset surfaces (search field, sunken inputs).           |
| `vibeshell_bg_lift_hover`   | `rgba(255, 255, 255, 0.05)`        | Hover state on rows / buttons.                          |
| `vibeshell_border`          | `rgba(255, 255, 255, 0.08)`        | Hairline borders separating panel from canvas.          |
| `vibeshell_border_subtle`   | `rgba(255, 255, 255, 0.06)`        | Inset input borders, internal dividers.                 |
| `vibeshell_text`            | `#e8e8ee`                          | Primary text / titles / clock.                          |
| `vibeshell_text_dim`        | `rgba(232, 232, 238, 0.55)`        | Subtitles, secondary metadata.                          |
| `vibeshell_text_muted`      | `rgba(232, 232, 238, 0.45)`        | Placeholder text, deemphasized hints.                   |
| `vibeshell_accent`          | `#4dd2c8`                          | Caret, focus ring, key-binding glyphs.                  |
| `vibeshell_accent_soft`     | `rgba(77, 210, 200, 0.18)`         | Selection background / focus glow.                      |
| `vibeshell_accent_glow`     | `rgba(77, 210, 200, 0.85)`         | Section headers, accent text on dark.                   |
| (no token; inline)          | `rgba(255, 122, 89, 0.18)` bg      | Workspace urgent state — translucent coral on neutral.  |
| (no token; inline)          | `#ff7a59` text                     | Workspace urgent state — coral text matching bg above.  |

`@define-color` tokens are declared at the top of
`crates/gtk-theme/style.css`. GTK4 CSS does **not** support custom
properties on arbitrary selectors, so use the tokens via `@token_name`
references in stylesheet rules, not `var(...)` syntax.

### Why this palette

- **Background**: matches vibewm's framebuffer clear color (`[0.05,
  0.05, 0.07]` → `#0d0d12`), so GTK clients visually merge with the
  compositor's empty regions instead of stamping out hard rectangles.
- **Accent**: a desaturated cyan-teal. Far enough from any common
  app-icon color (most apps use blue/green/red logos) to read as
  "shell" not "content," and high-contrast on the dark background.
- **Coral urgent**: the only non-teal accent, reserved for attention
  states (workspace urgent, future error toasts). Don't reuse for
  decorative purposes.

## Geometry

| Token (inline)           | Value      | Usage                                          |
|--------------------------|------------|------------------------------------------------|
| Panel radius             | `16px`     | Floating panels (launcher, cheatsheet).        |
| Inset radius             | `12px`     | Inputs (search field).                         |
| Row radius               | `10px`     | List rows, workspace buttons.                  |
| Panel padding            | `16px`     | Inside floating panels.                        |
| Inset padding            | `12px 16px`| Inside inputs.                                 |
| Row padding              | `4px 8px`  | List rows, workspace buttons.                  |
| Drop shadow              | `0 12px 40px rgba(0,0,0,0.55)` | Lifts floating panels off the canvas. |
| Focus ring               | `0 0 0 2px @vibeshell_accent_soft` | Around focused inputs.       |

Min-widths only; `max-width` isn't supported in GTK4 CSS — cap via the
parent container's allocation instead (`gtk::CenterBox`, fixed window
width, etc.).

## Typography

GTK4 inherits the system font by default. The stylesheet doesn't
override the family, so vibeshell follows the user's system sans-serif
preference — `font-family: ...` only specified for monospace contexts.

| Context                         | Family                                                                | Weight | Size  |
|---------------------------------|-----------------------------------------------------------------------|--------|-------|
| Window/panel titles             | system sans                                                           | 700    | 18px  |
| Body / row title                | system sans                                                           | 500    | 14px  |
| Subtitle / metadata             | system sans                                                           | 400    | 12px  |
| Search input                    | system sans                                                           | 500    | 16px  |
| Clock                           | system sans, `font-variant-numeric: tabular-nums`                     | 500    | inherit |
| Section header                  | system sans, `text-transform: uppercase`, `letter-spacing: 0.08em`    | 700    | 11px  |
| Status bar info                 | system sans                                                           | 400    | 12px  |
| Cheatsheet key glyphs           | `"JetBrains Mono", "Fira Mono", "DejaVu Sans Mono", monospace`        | 400    | 12px  |

## Motion

| Effect                | Duration | Easing            | Where it fires                          |
|-----------------------|----------|-------------------|-----------------------------------------|
| Hover bg crossfade    | 80ms     | `ease-out`        | List rows, workspace buttons.           |
| Cluster dive zoom     | 220ms    | ease-out-cubic    | Overlay → Cluster transition.           |
| Cluster undive zoom   | 220ms    | ease-out-cubic    | Cluster → Overlay transition.           |
| Window position lerp  | 220ms    | ease-out-cubic    | Vibewm `apply_layout_ops` (smithay).    |
| Recenter pan          | 220ms    | ease-out-cubic    | Overlay R-key recenter.                 |
| Pan inertia           | EMA + friction | `α=0.5`, `friction=0.86` | Overlay drag-pan release.    |

220ms is the canonical "thing changed" duration; reuse it when adding
new transitions so the shell feels coherent. Sub-100ms for hover/touch
feedback only.

## Class naming

| Pattern                                     | When                                  |
|---------------------------------------------|---------------------------------------|
| `.vibeshell-<app>-window`                   | The top-level GTK window per app.     |
| `.vibeshell-<app>-panel`                    | The main content container.           |
| `.vibeshell-<app>-<element>`                | Specific widgets within an app.       |
| `.vibeshell-<app>-section-header`           | Category dividers in a list.          |
| `.workspace-{focused,visible,urgent}` etc.  | State modifiers (panel-only today).   |

Apps add classes via `widget.add_css_class("vibeshell-launcher-panel")`
in Rust, then the stylesheet targets them with `box.vibeshell-launcher-panel
{ ... }`. Reuse Adwaita's built-in classes where they fit
(`.dim-label`, `.heading`, `.title-2`, `.boxed-list`, `.flat`,
`.monospace`) — the stylesheet layers vibeshell rules on top.

## Adding a new GTK app

1. Add `gtk-theme = { path = "../../crates/gtk-theme" }` to your
   `Cargo.toml`.
2. Call `gtk_theme::install_theme(&display)` inside your
   `Application::activate` handler (use `gtk4::gdk::Display::default()`
   to get the display). Must run before any widget is realized.
3. Tag your widgets with `.vibeshell-<app>-<element>` classes that
   correspond to rules in `crates/gtk-theme/style.css`.
4. Extend `crates/gtk-theme/style.css` with your app's section,
   referencing palette tokens via `@vibeshell_*` not raw colors.
5. Use the existing geometry/motion values above unless you have a
   specific reason to deviate.

## Aligning a third-party app

If you're not adding a vibeshell crate but want a tool (rofi, fuzzel,
waybar, dunst, etc.) to look like it belongs:

- Background: `#0d0d12` solid, or `rgba(13, 13, 18, 0.94)` translucent.
- Border: `1px rgba(255, 255, 255, 0.08)`.
- Text primary: `#e8e8ee`. Secondary/dim: `rgba(232, 232, 238, 0.55)`.
- Accent: `#4dd2c8`. Soft accent (selections): `rgba(77, 210, 200, 0.18)`.
- Radius: `16px` for panels, `10px` for rows, `12px` for inputs.
- Drop shadow: `0 12px 40px rgba(0, 0, 0, 0.55)`.
- 220ms ease-out-cubic for state transitions.

For hex everywhere: `#0d0d12 #e8e8ee #4dd2c8 #ff7a59`. That's the
minimum quartet to stay recognizable across third-party tools.
