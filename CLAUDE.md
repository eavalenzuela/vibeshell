# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

All common development tasks use `just` (see `justfile` for all recipes):

```bash
just fmt           # Format code
just fmt-check     # Check formatting (CI)
just clippy        # Run strict linter
just check         # Cargo check without building
just smoke-binaries # Build all app binaries
just ci            # Full CI suite (fmt-check + clippy + check + smoke-binaries)
just smoke-test    # Headless integration test (starts Sway, daemon, exercises IPC)
```

Running individual apps during development:
```bash
just run-nested         # Recommended: run full session in nested Sway
just run-panel          # Run panel standalone
just run-launcher       # Run launcher standalone
just run-notifd         # Run notifd standalone
just run-ctl [args]     # Run vibeshellctl CLI
```

There are no automated tests in this project. CI validates via `just ci`.

Logging is controlled by `VIBESHELL_LOG=<level>` (falls back to `RUST_LOG`; default: `info`).

## Architecture

**vibeshell** is a Wayland shell environment for Sway, implementing a "Continuum WM" cluster-based window management system. It is a Rust workspace with 5 apps and 4 shared crates.

### Workspace layout

```
apps/
  panel/        # Top bar: workspaces, focused title, clock, status indicators
  launcher/     # App launcher with fuzzy search and usage-ranking
  notifd/       # DBus notifications daemon (org.freedesktop.Notifications)
  overlay/      # Cluster overview & zoom navigation UI
  vibeshellctl/ # CLI binary for lifecycle management and IPC dispatch

crates/
  common/   # Logging init, SIGHUP reload mechanics, IPC contracts (CanvasState, etc.)
  config/   # TOML config schema and loading (~/.config/vibeshell/config.toml)
  sway/     # Sway IPC: workspace/window events, PanelState snapshots
  xdg/      # .desktop file discovery and parsing (stdlib only, no deps)
```

### Key architectural patterns

**GTK4 + Layer Shell**: All apps use `gtk4` + `libadwaita` with `gtk4-layer-shell` for Wayland layer surface positioning. Apps degrade gracefully if layer-shell is unavailable.

**Config reload**: Apps call `common::spawn_reload_listener()` which sets up a SIGHUP handler. `vibeshellctl reload` sends SIGHUP to all components so they re-read config without restarting.

**IPC**: `overlay` fetches state from `vibeshellctl ipc get-state` (JSON subprocess call). The shared types live in `crates/common/src/contracts.rs` (`CanvasState`, `IpcRequest`, `IpcResponse`, etc.).

**Sway integration**: `crates/sway` wraps `swayipc::Connection`. Apps subscribe to Workspace + Window events with debouncing; `PanelState` / `PanelUpdate` are the key snapshot types.

**Session startup**: `scripts/start-sway-session` orchestrates startup — it generates Sway keybindings (via `cargo run -p sway --bin generate-bindings` → `dev/sway.bindings.generated`), launches Sway with `dev/sway.config`, then spawns panel/launcher/notifd in the background. Use `just run-nested` for development; it wraps everything in a nested Sway window.

### Workspace-wide constraints

- `unsafe` code is forbidden across all crates (enforced in root `Cargo.toml`).
- Rust edition 2021 throughout.
- Clippy is run with strict `deny(warnings)` in CI.
