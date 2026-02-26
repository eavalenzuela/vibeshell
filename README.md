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

- Rust toolchain (stable)
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

For display-manager style startup, use the same orchestration script directly:

```bash
./scripts/start-sway-session
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

## Useful development commands

```bash
cargo check
cargo fmt
cargo clippy --workspace --all-targets
```

---

## Project layout

- `apps/panel` – top panel UI
- `apps/launcher` – app launcher UI
- `apps/notifd` – notifications daemon/UI
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
