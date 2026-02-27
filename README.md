# vibeshell

`vibeshell` is an experimental Wayland shell environment built on top of **Sway**.

Current workspace apps:

- `panel`: top bar with workspace state, focused window title, and a clock
- `launcher`: app launcher that discovers and launches `.desktop` applications
- `notifd`: a basic `org.freedesktop.Notifications` daemon with on-screen cards

---

## Prerequisites

### System dependencies

At minimum, you need:

- Rust toolchain (stable, currently validated with **1.85.x**)
- `sway`
- GTK 4 + libadwaita development libraries
- layer-shell GTK bindings dependencies
- D-Bus user session

On Debian/Ubuntu-based systems, this is a good starting point:

```bash
sudo apt update
sudo apt install -y \
  build-essential pkg-config \
  libgtk-4-dev libadwaita-1-dev \
  libdbus-1-dev \
  sway
```

> Exact package names may differ across distributions.

---

## Setup

1. Clone the repo and enter it:

   ```bash
   git clone <your-fork-or-this-repo-url>
   cd vibeshell
   ```

2. Ensure Rust is installed:

   ```bash
   rustup --version
   cargo --version
   ```

   This project expects the stable toolchain. To match CI exactly:

   ```bash
   rustup toolchain install stable
   rustup default stable
   rustup component add rustfmt clippy
   ```

3. Build the workspace:

   ```bash
   cargo build
   ```

---

## Running vibeshell

### Option A: Run in a nested Sway session (recommended for development)

This project includes a helper script that runs the full session flow (Sway + `panel` + `launcher` + `notifd`) with process-group-aware shutdown:

```bash
./scripts/run-in-nested-sway
```

or via `just`:

```bash
just run-nested
```

If you hit `compositor doesn't support zwlr_layer_shell_v1` while nesting under another compositor, force wlroots to use the X11 nested backend (this is the most reliable workaround for hosts that do not expose layer-shell to nested Wayland clients):

```bash
VIBESHELL_NESTED_BACKEND=x11 ./scripts/run-in-nested-sway
# or auto-pick x11 when both DISPLAY and WAYLAND_DISPLAY are present
VIBESHELL_NESTED_BACKEND=auto ./scripts/run-in-nested-sway
```

Backend options for `run-in-nested-sway`:

- `VIBESHELL_NESTED_BACKEND=wayland` (default): nested Wayland backend.
- `VIBESHELL_NESTED_BACKEND=x11`: nested X11 backend; recommended workaround for the layer-shell error.
- `VIBESHELL_NESTED_BACKEND=auto`: prefers `x11` when both X11 and Wayland sessions are available, otherwise `wayland`.

For display-manager style startup, use the same orchestration script directly:

```bash
./scripts/start-sway-session
```


### Sway keybinding generation

`crates/sway` includes a small generator (`generate-bindings`) that emits `dev/sway.bindings.generated` from configurable keybinding values. `dev/sway.config` includes that generated file.

Lifecycle:

1. `./scripts/start-sway-session` regenerates `dev/sway.bindings.generated` before launching Sway.
2. Override bindings/commands with `VIBESHELL_*` env vars (for example `VIBESHELL_SCREENSHOT_CMD`, `VIBESHELL_VOLUME_UP_KEY`, `VIBESHELL_SHELL_RESTART_CMD`).
3. If you edit defaults in the generator, rerun generation manually and commit the updated generated file:

```bash
cargo run -p sway --bin generate-bindings -- --output dev/sway.bindings.generated
```


To launch only Sway (without shell components) for debugging:

```bash
VIBESHELL_SWAY_ONLY=1 ./scripts/start-sway-session
# or
just run-session-sway-only
```

### Option B: Run components individually

From one Wayland session, run each component:

```bash
cargo run -p panel
cargo run -p launcher
cargo run -p notifd
```

or with `just` shortcuts:

```bash
just run-panel
just run-launcher
just run-notifd
```

---


## Logging

All apps (`panel`, `launcher`, and `notifd`) use shared logging setup from `crates/common`.

- Default level is `info`.
- Set `VIBESHELL_LOG` to control verbosity (falls back to `RUST_LOG` if unset).
- Logs include the `target` field, so output is tagged with the emitting component/module.

Examples:

```bash
VIBESHELL_LOG=debug ./scripts/run-in-nested-sway
```

or, for very verbose tracing:

```bash
VIBESHELL_LOG=trace just run-nested
```

### Inspecting logs in nested Sway development

The nested launcher script keeps child processes attached to the invoking terminal. During development:

- Start nested Sway from a terminal and watch logs live there.
- Optionally capture logs to a file:

```bash
VIBESHELL_LOG=debug ./scripts/run-in-nested-sway 2>&1 | tee /tmp/vibeshell-nested.log
```

- If you need to inspect after the run, open `/tmp/vibeshell-nested.log`.

### `vibeshellctl` command-line control

A new control binary is available for common shell lifecycle actions:

```bash
cargo run -p vibeshellctl -- <subcommand>
```

Subcommands:

- `reload` — requests `sway reload` and sends `SIGHUP` to `panel`, `launcher`, and `notifd` so they re-read `crates/config` values at runtime when safe.
- `status` — reports whether `sway`, `panel`, `launcher`, and `notifd` are running.
- `restart <component>` — stops and re-launches one component (`panel`, `launcher`, or `notifd`).
- `logs <component>` — filters captured nested-session logs for one component.

Examples for nested Sway development:

```bash
# Start nested Sway and capture logs for later inspection
VIBESHELL_LOG=debug ./scripts/run-in-nested-sway 2>&1 | tee /tmp/vibeshell-nested.log

# Check runtime state
cargo run -p vibeshellctl -- status

# Reload sway config in-place
cargo run -p vibeshellctl -- reload

# Equivalent manual signal-based reload
pkill -HUP -x panel
pkill -HUP -x launcher
pkill -HUP -x notifd

# Restart just the panel while nested sway is running
cargo run -p vibeshellctl -- restart panel

# View launcher-only logs from the captured nested session file
cargo run -p vibeshellctl -- logs launcher
```

Notes:

- `logs` reads from `/tmp/vibeshell-nested.log` by default. Override with `VIBESHELL_LOG_FILE=/path/to/file`.
- `restart` respects `VIBESHELL_PANEL_CMD`, `VIBESHELL_LAUNCHER_CMD`, and `VIBESHELL_NOTIFD_CMD` when set.

## Useful development commands

```bash
just fmt-check
just clippy
just check
just smoke-binaries
just ci
```

---

## Project layout

- `apps/panel` – top panel UI
- `apps/launcher` – app launcher UI
- `apps/notifd` – notifications daemon/UI
- `apps/vibeshellctl` – CLI for reload/status/restart/log inspection
- `crates/sway` – sway IPC/event integration
- `crates/xdg` – desktop entry discovery/parsing
- `crates/config` – configuration crate
- `crates/common` – shared logging and utilities
- `dev/sway.config` – Sway config used in development
- `scripts/run-in-nested-sway` – helper runner script

---

## Notes

- This is an early-stage project and not production-ready.
- If launching from a display manager, create a session wrapper that starts Sway plus the three apps.
