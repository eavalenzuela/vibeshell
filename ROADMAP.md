# Continuum WM — Roadmap

## v1 Feature Goals

### Must-have interactions
- [x] Zoom navigation — Overview → Cluster → Focus levels
- [x] Bindings: Mod+Wheel (zoom), Mod+Drag / Mod+Arrows (pan), Mod+Enter (dive), Mod+Esc (zoom out)
- [x] Clusters (task neighborhoods) — create, rename, move on canvas
- [x] Assign focused window to cluster
- [x] "Auto-cluster" heuristic v1: by app_id/class (optional toggle) *(Phase 6.5)*
- [x] Fast recall — "Recent clusters" switcher (Alt-Tab equivalent for clusters) *(Phase 6.5)*
- [x] Launcher can search windows and clusters *(Phase 6.5)*
- [x] Persistence — save/restore cluster positions, window→cluster mapping, last viewport/zoom
- [x] Graceful handling when windows are missing (apps closed) — `prune_stale_entries()` removes stale refs on ingest

### Must-have desktop-reality features
- [x] Multi-monitor: per-output viewport (even with one global canvas) *(Phase 6.5)*
- [x] Basic rules: float dialogs, keep transient windows attached to parent — `anchor_transient_dialogs()` + `WindowRole::Dialog`
- [x] Handle special windows: fullscreen, scratchpad, modals, popups — `WindowRole::Scratchpad` + `LayoutExclusionReason::Scratchpad`; ingest detects via `scratchpad_state` and `__i3*` workspace name; cluster_id forced to None; auto-cluster + transient anchor skip scratchpad; layout engine excludes
- [x] Non-jank repositioning: debounce geometry updates, respect manual resize overrides *(Phase 6.5)*

### Explicit non-goals (keep v1 shippable)
- Perfect smooth animation (step transitions first)
- True live thumbnails in overview (icons + titles first)
- Complex auto-layout intelligence (deterministic simple rules first)

---

## Implementation Phases

### [x] Phase 0 — Lock the contract

- [x] Config schema (clusters enabled, zoom step sizes, strip placement)
- [x] Internal IPC message protocol
- [x] Data model structs + serde serialization (`CanvasState`, `Cluster`, `Window`, etc.)
- [x] Logging + `vibeshellctl dump-state`

**Exit criteria:** print a coherent model of windows/clusters and reload it. ✓

---

### [x] Phase 1 — Cluster model on top of Sway

- [x] Cluster CRUD: create / rename / delete / move (pure model)
- [x] Assign window → cluster (manual)
- [x] Track window lifecycle from Sway (new window → auto-assign; close → remove)
- [x] UI overlay (GTK layer-shell): Overview showing clusters + window lists
- [x] Click cluster to activate

**Exit criteria:** manage clusters reliably, even if geometry is unchanged. ✓

---

### [x] Phase 2 — Geometry control in Cluster zoom

- [x] Single Sway workspace as the "continuum workspace"
- [x] Compute target tiling layout for active cluster on enter
- [x] Apply layout via Sway IPC (move/resize commands)
- [x] Debouncing: coalesce new-window + focus + zoom events into one apply pass
- [x] Deterministic layouts: 1 window → full; 2 → split; 3+ → columns/BSP

**Exit criteria:** selecting a cluster consistently rearranges windows into a stable layout. ✓

---

### [x] Phase 3 — Focus zoom + context strip

- [x] Focused-window-dominant layout (~70–80% of area)
- [x] Remaining windows in a "context strip" (small tiles along an edge), ordered by recency
- [x] Recency list maintained per cluster
- [x] Zoom in/out keybindings
- [x] "Cycle in strip" keybind
- [x] Stable window ordering (no shuffle on zoom transition)

**Exit criteria:** focus mode is useful and predictable — no window shuffle, no flicker. ✓

---

### [x] Phase 4A — Overview becomes real (core visual navigation)

- [x] Spatial cluster positions on a canvas (world coordinates)
- [x] Pan overview: keyboard (96 px / 384 px with Shift) + pointer drag
- [x] Zoom overview: Mod+Wheel / Mod+Plus / Mod+Minus (12% steps), clamped to [0.35, 2.50]
- [x] Drag clusters to reposition on canvas
- [x] Cluster creation from Overview (`N` key → `CreateCluster` IPC)
- [x] Dive into cluster: single click + Enter, or double click
- [x] Viewport sync to daemon (`OverviewPan` / `OverviewZoom` IPC dispatch)
- [x] Local viewport state prevents poll from overwriting in-progress interaction
- [x] Enter-with-no-selection emits status hint (no accidental mode switch)
- [x] Deterministic Overview ↔ Cluster ↔ Focus transitions (no regressions)

**Exit criteria (verified):**
- [x] C1: Create clusters, drag to positions, restart daemon — positions restored
- [x] C2: Dive into any visible cluster in ≤2 interactions (click+Enter or double-click)
- [x] C3: Pan/zoom aggressively — selection preserved, cluster rediscoverable
- [x] C4: Keyboard-only: select, move (`M`+arrows), dive (Enter), cancel (Esc)
- [x] C5: 20× Overview↔Cluster↔Focus cycles across 3+ clusters — no Phase 2/3 regressions

---

### [x] Phase 4B — Overview polish

- [x] Snap-to affordances during drag/move (grid lines every 200 world-px, card centerlines, output center)
- [x] Snap ghost preview: faint blue guide lines drawn when within 24 screen-px of a snap target
- [x] Inertial panning behavior (EMA velocity + friction loop via glib::timeout_add_local)
- [x] Animated recenter: R key eases to cluster with ease-out-cubic over 220 ms

**Exit criteria:**
- [x] Drag cluster near grid/card/center → card snaps and guide line appears
- [x] Keyboard move (M+arrows) also applies snap
- [x] Pan and release with velocity → viewport drifts and decelerates naturally
- [x] R key smoothly pans to selected cluster; arrow-key pan cancels animation

---

### [x] Phase 5 — Persistence + robustness

- [x] Persist: cluster positions (boot_persisted snap-back fixed via `update_boot_persisted` after every `persist_immediate`)
- [x] Persist: window→cluster assignment hints (`AssignmentHint` config schema + `apply_assignment_hints()`)
- [x] Persist: last active cluster + viewport/zoom level (`active_cluster` added to `PersistedOverviewState`)
- [x] Fullscreen window handling: preserve zoom level when a fullscreen window is in focus
- [x] Dialogs transient_for parent: `anchor_transient_dialogs()` assigns transient windows to parent cluster
- [x] Multi-output: per-output viewport via `VIBESHELL_OUTPUT` env var *(Phase 6.5)*

**Exit criteria:** restart session and the workspace landscape comes back sensibly. ✓

---

### [x] Phase 6 — Performance polish

- [x] Reduce IPC chatter: `needs_sway_ingest()` predicate — only `GetState`/`CreateCluster` call `ingest_sway_facts()`
- [x] Batch Sway commands: `CreateCluster` issues two commands in one semicolon-joined `run_command` call
- [x] Avoid frequent `get_tree`: mutation-only IPC paths skip `get_tree`/`get_workspaces`/`get_outputs`
- [x] Throttle cluster position writes to ≤30 Hz during drag (`last_drag_ipc` Instant guard); unthrottled commit on release
- [x] Non-blocking IPC dispatch for `UpdateClusterDrag` and `KeyboardMoveBy` (`dispatch_ipc_mutation_detached`)
- [x] Full drag lifecycle: `BeginClusterDrag` / `UpdateClusterDrag` / `CommitClusterDrag` / `CancelClusterDrag` wired CLI → IPC → state
- [x] Drag offset baked into `canvas_state` on commit to prevent double-offset on next daemon poll
- [x] Viewport pan/zoom writes ≤20 Hz — **superseded** (2026-04-29). Audit found drag-pan already fires only on `drag_end` (one IPC per gesture); inertia/viewport-anim only fire at loop end; keyboard pan is event-driven by user input. The only sustained path is scroll-wheel zoom, but `OverviewZoom` takes a sign-only `delta` with fixed `STEP=1.12` — throttling drops zoom steps rather than coalescing them, requiring an absolute-scale protocol change to fix correctly. Phase 8 (wlroots) replaces the input dispatch entirely, making this throwaway work.
- [x] Simple overlay animations — cluster-dive ease-in: 220 ms ease-out-cubic pan + zoom (gain 1.4×) before `on_dive`/`on_activate` flips zoom level. Generalized `ViewportAnim` to interpolate scale and run an `on_complete` callback; `start_recenter_anim` now delegates through a shared `start_viewport_anim`. Overview ↔ Cluster ↔ Focus mode-change animations deferred to Phase 8 (those require the overlay to render non-Overview modes, which is a wlroots-era responsibility).

**Exit criteria:** no "constant resizing" feeling; CPU stays sane. ✓

---

### [x] Phase 6.5 — Remaining v1 feature gaps

- [x] "Auto-cluster" heuristic v1: `auto_cluster_by_app_id()` — when `auto_cluster = true`, unassigned windows with matching `app_id` auto-route to existing cluster
- [x] Fast recall — `CycleCluster { direction }` IPC + `cluster_history` MRU tracking in `StateOwner`; `Mod+Tab` / `Mod+Shift+Tab` keybindings
- [x] Launcher: search windows (by title/app_id) and clusters (by name); `SearchResult` enum merges with app results; activate focuses window or switches cluster
- [x] Non-jank repositioning: `ManualResize` added to `LayoutExclusionReason`; geometry tracking (`last_applied_geometry`) in `StateOwner`; tiled windows diverging >10px marked `manual_position_override`
- [x] Multi-monitor: `VIBESHELL_OUTPUT` env var → `output_name` in `WidgetState`; overlay uses `output_viewports.get(output_name)` for rendering; pan/zoom IPC passes `output` parameter

**Exit criteria:** daily-driver usable across multi-monitor setups with no surprise window shuffling. ✓

---

### [x] Phase 7 — SelectCluster + keyboard move operations

- [x] `SelectCluster`: CLI `select-cluster <id>` → `IpcRequest::SelectCluster` → updates `selected_cluster_id` (no zoom change)
- [x] `EnterKeyboardMoveMode`: CLI `enter-keyboard-move-mode <id>` → records `keyboard_move_origin` in `StateOwner`
- [x] `KeyboardMoveBy`: CLI `keyboard-move-by <dx> <dy>` → adds delta to cluster position
- [x] `CommitKeyboardMove`: CLI `commit-keyboard-move` → `persist_immediate` + clear origin
- [x] `CancelKeyboardMove`: CLI `cancel-keyboard-move` → restore cluster to origin coords
- [x] All 5 `MutationType` variants wired end-to-end (CLI → IpcRequest → state_store)

**Exit criteria:** keyboard-only cluster selection and repositioning works without entering Overview drag mode. ✓

---

### [x] Wiring up — Unwired features audit

Features and code paths that exist but are not fully wired into the running system.

#### [x] Fixed (simple wiring)

- [x] **Cluster MRU history not persisted**: `cluster_history` field existed in `StateOwner` but was never saved to or restored from `PersistedOverviewState` → added `cluster_history: Vec<ClusterId>` to `PersistedOverviewState`, populated in `update_boot_persisted()`, restored on boot
- [x] **Overlay not launched by session script**: `scripts/start-sway-session` started panel/launcher/notifd but not overlay → added `OVERLAY_CMD` and `start_component "overlay"` to session startup
- [x] **Cycle-cluster bindings not passed through session script**: `generate-bindings` defaults work, but session script had no env var override passthrough → added `CYCLE_CLUSTER_FORWARD_KEY/CMD` and `CYCLE_CLUSTER_BACKWARD_KEY/CMD` env vars and `--cycle-cluster-*` flags to the generate-bindings invocation
- [x] **`IpcRequest::Pan` unhandled**: legacy `Pan { dx, dy }` variant fell through to `unsupported` catch-all → now forwarded to `overview_pan()`
- [x] **`IpcRequest::MoveWindowToCluster` unhandled**: defined in contracts with tests but no dispatch handler → implemented `move_window_to_cluster()` in `StateOwner`, wired CLI subcommand `move-window-to-cluster`
- [x] **`IpcRequest::RenameCluster` unhandled**: defined in contracts with tests but no dispatch handler → implemented `rename_cluster()` in `StateOwner`, wired CLI subcommand `rename-cluster`

#### [x] Remaining (previously needed design or significant work)

- [x] **FramePipeline / LayoutEngine dead code** (`crates/sway/src/backend.rs`): Wired via `vibeshellctl daemon` — a persistent daemon mode that subscribes to sway Window/Workspace events, feeds them into `FramePipeline`, and applies computed `LayoutOp`s via sway IPC. IPC clients (overlay, keybindings) connect via Unix socket (`$XDG_RUNTIME_DIR/vibeshell-daemon.sock`) with subprocess fallback. Type aliases unified (`i64` → `u64` via `common::contracts`). Session script launches daemon first.

- [x] **Non-jank geometry tracking incomplete** (`apps/vibeshellctl/src/state_store.rs`): Fixed by adding `layout_engine_active` flag. When the daemon applies layouts via `update_applied_geometry()`, `last_applied_geometry` holds layout engine targets. `ingest_sway_facts()` now compares sway's current geometry against layout intent (not just inter-poll drift), preserving recorded targets rather than overwriting them. New windows get sway-reported geometry as initial baseline.

- [x] **StateOwner config not reloaded on SIGHUP**: Fixed by adding `reload_config()` to `StateOwner` (re-reads `auto_cluster` and `assignment_hints` from config). The daemon subscribes to SIGHUP via `common::spawn_reload_listener()` and calls `reload_config()` on signal. `vibeshellctl reload` now also sends SIGHUP to the daemon process (`pkill -HUP -x vibeshellctl`).

- [x] **Multi-monitor per-output overlay instances**: Session script now enumerates outputs via `swaymsg -t get_outputs` and spawns one overlay per output with `VIBESHELL_OUTPUT=<name>`. Falls back to a single instance if output enumeration fails.

- [x] **Keyboard move bindings not generated**: Added `EnterKeyboardMoveModeSelected` IPC variant (uses currently selected cluster) + 8 new bindings in `generate-bindings`: `$mod+Shift+m` (enter move mode), `$mod+Shift+{Up,Down,Left,Right}` (move by 96px), `$mod+Shift+Return` (commit), `$mod+Shift+Escape` (cancel). All overridable via env vars in session script.

---

### [ ] Phase 8 — wlroots compositor port

**Decision (2026-04-29):** committed. Sway-as-backend is the dominant cause of perceived incompleteness — smooth transitions, real thumbnails, and gesture integration are blocked behind it. State model, layout engine, IPC protocol, and overlay UX carry over unchanged; compositor is a new backend, not a rewrite.

**Sequencing (2026-04-29):** W1 broke down further — there was no `WmBackend` trait yet, so step one is defining it. W1a = trait + Sway impl behind it (no behavior change). W1b = minimal smithay compositor. W1c+ = parity (scene graph, gesture input, smooth zoom).

- [x] **W1a — Define `WmBackend` trait + port Sway behind it** (2026-04-29): new `crates/wm` (trait, `WmFacts`, layout/frame engine moved over from `crates/sway/src/backend.rs`); `crates/sway` now hosts `SwayBackend impl wm::WmBackend` plus `sway_snapshot` + `collect_windows_from_tree` lifted out of `apps/vibeshellctl/src/state_store.rs`; daemon + state_store + ipc dispatcher route through `&mut dyn WmBackend`; `WM_BACKEND` env var (default `sway`, returns `NotImplemented` for `wlroots`). Panel/launcher/overlay deliberately not refactored — they're wayland clients, not control-plane callers, and will switch to vibeshell IPC when wlroots lands. All 23 unit tests + 26 smoke checks green. Single Sway-specific holdout: `ingest_sway_event_metadata` (dump-state debug probe, TODO(W1c)).
- [x] **W1b — Minimal smithay compositor `apps/vibewm`** (2026-04-29): boots in a winit window on the host compositor, advertises `wl_compositor` + `wl_shm` + `wl_seat` + `wl_output` + `xdg_shell` + `wlr_layer_shell` + `wl_data_device`. Smoke verified: vibewm starts cleanly with EGL/GLES on AMD Radeon, listens on `wayland-1`, `vibeshell-panel` connects through Gdk's wayland init successfully (Vulkan→GL fallback warnings are Gdk-internal and harmless). Move/resize grabs and popup grabs are stubbed (TODO(W1c)). DRM backend, scene-graph effects, gesture input, and daemon control-plane bridge are W1c. The crate-level `unsafe_code = "deny"` lint allows two `#[allow(unsafe_code)]` blocks: smithay's `Generic::get_mut()` and pre-event-loop `std::env::set_var`.
- [x] **W1c-1 — Daemon ↔ vibewm seam** (2026-04-29): `WlrootsBackend impl WmBackend` ships in `crates/wm/src/wlroots_backend.rs`, talking to vibewm over a JSON-line unix socket (`$XDG_RUNTIME_DIR/vibewm-control.sock`, overridable via `VIBEWM_SOCKET`). Wire protocol in `crates/wm/src/vibewm_ipc.rs` covers all 10 `WmBackend` methods + a `Subscribe` long-poll for the event stream. Vibewm's IPC server is a calloop source in `apps/vibewm/src/ipc.rs`. Smoke verified: `WM_BACKEND=wlroots vibeshellctl status` round-trips through Ping/Pong, `... dump-state` round-trips through Snapshot. Snapshot returned an empty `WmFacts` until W1c-2.
- [x] **W1c-2 — Workspace + window-id model** (2026-04-29): `apps/vibewm/src/model.rs` (`VibewmModel`) registers each xdg-toplevel with a stable `u64`, tracks clusters (default cluster "1" mirrors sway's boot convention), and exposes `activate_cluster` / `back_and_forth` / `find_cluster_by_name`. `apps/vibewm/src/ipc.rs::snapshot_facts` now walks the model + smithay state to return real `WmFacts`: clusters with `windows: Vec<WindowId>`, real per-window `title`/`app_id` pulled from `XdgToplevelSurfaceData`, geometry from `Space::element_geometry`, and the winit output's actual mode/scale. `FocusedWindow` maps the seat's keyboard focus through `model.window_id_for_surface`; `FocusWindow` looks up the registered toplevel and sets seat focus. `CreateNamedWorkspace` creates+activates (sway-compat); `ActivateCluster`/`BackAndForthWorkspace` route through the model. Live smoke: launching `foot` against `WAYLAND_DISPLAY=wayland-1` produces a real `dump-state` showing `id=1, title="foot", app_id="foot", cluster_id=1`. 6 new model unit tests; all 26 smoke checks green. Layer-shell surfaces (panel) deliberately stay outside the model — they're not "windows" in the cluster sense.
- [x] **W1c-3 — Layout apply, cluster visibility, event push** (2026-04-29): `Vibewm::apply_layout_ops` repositions windows via `Space::map_element` + sends an xdg_toplevel configure with the target size; `last_known_position` cache survives unmap so reactivation restores in place. `Vibewm::sync_cluster_visibility` unmaps inactive cluster windows from the space and re-maps active ones — invoked from `ActivateCluster`/`BackAndForthWorkspace`/`CreateNamedWorkspace`. `Vibewm::broadcast_workspace_or_window` pushes `VibewmEvent::WorkspaceOrWindow` to all subscribed clients; wired into `new_toplevel`, `toplevel_destroyed`, `SeatHandler::focus_changed`, and the cluster IPC handlers. Live verified: spawning foot under vibewm + nc-Subscribe to the control socket pushed Subscribed + 3 events (toplevel map, focus change, cluster activate); ApplyLayoutOps round-trip logs `applied=1` against a real foot window. All 26 smoke checks green.
- [x] **W1c-4 — Dogfood-ready vibewm session** (2026-04-29): xdg-decoration wired (vibewm forces ServerSide so Gtk/foot stop drawing CSDs), `XdgShellHandler::new_toplevel` now sends an initial configure size based on the active output, vibewm writes its socket name to `$XDG_RUNTIME_DIR/vibewm.wayland-display` so external launchers don't have to guess among stale wayland-N sockets, and `scripts/start-vibeshell-session` (+ `just run-vibeshell-session`) spawns the full session — vibewm + daemon (`WM_BACKEND=wlroots`) + panel + launcher + notifd. Verified end-to-end: session script picks the right WAYLAND_DISPLAY, daemon connects to vibewm-control, panel/launcher/notifd attach as wayland clients.
- [x] **W1c-5 — Panel + launcher backend-neutral** (2026-04-29): `PanelState`/`WorkspaceState`/`PanelUpdate` relocated from `crates/sway` to `crates/common/src/panel.rs` (re-exported from sway for back-compat with overlay's `sway::*` imports). Panel's `sway::SwayClient::connect()` listener thread replaced with `apps/panel/src/daemon_source.rs` polling `IpcRequest::GetState` and projecting `CanvasState` → `PanelState`. Workspace switch wired to `vibeshellctl ipc activate-cluster --cluster <id>` rather than `swaymsg workspace <name>` (works under both backends). Right-click move-focused-to-workspace temporarily no-ops pending a `MoveFocusedWindowToCluster` IPC (W1c-6). Launcher's vestigial `spawn_sway_dependency_probe` removed entirely. `sway` dep dropped from both `apps/panel` and `apps/launcher` Cargo.tomls. Live verified: full vibeshell session under `WM_BACKEND=wlroots` now boots without any "sway IPC unavailable" retry warnings; daemon log shows panel's poll loop hitting `GetState`. 11 panel-crate unit tests (was 8; +3 from `daemon_source::canvas_to_panel_state`). All 26 sway-mode smoke checks still green.
- [x] **W1c-6 — Panel right-click + layer-surface logging** (2026-04-29): new `IpcRequest::MoveFocusedWindowToCluster { cluster }` wired CLI → daemon dispatch (resolves focus via `WmBackend::focused_window()`, then `state_store.move_window_to_cluster`). Panel right-click on workspace button now spawns `vibeshellctl ipc move-focused-window-to-cluster --cluster <id>` so it works under both backends. `apps/vibewm/src/handlers.rs::WlrLayerShellHandler` gained `tracing::info!` lines on layer-surface map and destroy (with namespace + output + Layer kind). Live verified end-to-end: vibewm logs `layer surface mapped namespace=gtk4-layer-shell output=winit layer=Top` for the panel's layer surface and clean `layer surface destroyed` on shutdown; the new IPC returns a structured `{"type":"error","message":"no window currently focused"}` when there's no focused toplevel.
- [x] **W1c-7 — xdg-shell move/resize grabs** (2026-04-29): `apps/vibewm/src/grabs/` ships `MoveSurfaceGrab` + `ResizeSurfaceGrab` (smithay `PointerGrab` impls, ported from smallvil). `XdgShellHandler::move_request` and `resize_request` install the grabs via the seat's pointer after validating the grab via `check_pointer_grab` (pointer must currently grab the focus surface and the surface must belong to the requesting client). `ResizeSurfaceState` rides on the surface's `data_map` so TOP/LEFT-edge resizes adjust window position correctly during commit. Daemon's existing geometry-divergence detection (`LayoutExclusionReason::ManualResize` in W1c-5/state_store ingest) covers post-drag handling automatically — windows the user moves are flagged manual_position_override on the next snapshot poll, so the layout engine leaves them alone. `bitflags = "2"` added as a dep. CI green; 26/26 sway-mode smoke checks still pass; vibewm + foot live boot still works.
- [x] **W1c-8 — Overlay event subscribe under wlroots** (2026-04-29): apps/overlay was the third GTK client still using `sway::spawn_event_stream` directly; under WM_BACKEND=wlroots this silently failed and overlay fell back to its 1.2 s poll. Added a parallel `wm::WlrootsBackend::spawn_event_stream` thread that pushes refresh signals into the same channel — whichever backend is reachable wins. Also logs `vibewm-control: client subscribed to events` server-side for visibility. Closes the panel/launcher/overlay backend-neutral arc started in W1c-5.
- [x] **W1c-9 — XWayland integration** (2026-04-29): `apps/vibewm` gained an `xwayland` Cargo feature (default-on; `--no-default-features` opts out) that pulls in smithay's xwayland subsystem. New `apps/vibewm/src/xwayland.rs` calls `XWayland::spawn` on startup and registers a calloop source that, on `XWaylandEvent::Ready`, attaches an `X11Wm` and stashes `xdisplay`/`xwm` on the state. New `XwmHandler` impl in `apps/vibewm/src/handlers.rs` bridges X11 surface events into vibewm's model + space: `new_window` / `map_window_request` register X11 surfaces in `VibewmModel` via `Window::new_x11_window` (X11 windows fit straight into the existing model alongside xdg-toplevels), `unmapped_window` / `destroyed_window` prune them, `configure_request` honors client geometry asks, override-redirect surfaces (X tooltips/popups) bypass the model. `client_compositor_state` now checks for `XWaylandClientData` before falling back to `ClientState`. Live verified: vibewm spawns Xwayland, attaches `X11Wm` (DISPLAY=:2), and `xwininfo -root -tree` connects + lists the X tree including the "Smithay X WM" presence window. X11 move/resize grabs are stubbed pending a synthesized `PointerGrabStartData` adapter (W1c-10+).
- [x] **W1c-10 — X11 move/resize grab adapter** (2026-04-29): `XwmHandler::move_request` / `resize_request` were W1c-9 stubs because X11 doesn't carry a wayland `PointerGrabStartData`. Synthesize one from the seat's pointer location + the X11 surface's associated `wl_surface` (via `X11Surface::wl_surface()`), then install the existing wayland-side `MoveSurfaceGrab` / `ResizeSurfaceGrab`. `xwm::ResizeEdge` translates to `grabs::ResizeEdge` by name. Buttons hard-coded to `BTN_LEFT` (0x110) for release detection — `MoveSurfaceGrab` only checks `current_pressed.contains` for unset, so the synthesized button doesn't need to match the real X11 click. X11 clients now drag inside vibewm exactly like xdg-toplevels.
- [x] **W1c-11 — Launcher fixes + wlroots-mode log polish** (2026-04-29): two real bugs in launcher's window/cluster activation paths: (a) window focus called `swaymsg [con_id=X] focus` directly, silently broken under `WM_BACKEND=wlroots`; (b) cluster activation called `vibeshellctl ipc activate-cluster <id>` (positional) but the CLI takes `--cluster <id>` (named clap arg). Added `IpcRequest::FocusWindow { window }` + `IpcCommands::FocusWindow` so launcher dispatches `vibeshellctl ipc focus-window --window <id>` (works under both backends). Fixed the cluster-activate shape. Also downgraded `crates/sway/src/lib.rs::spawn_event_stream` connection failures from `tracing::warn!` to `tracing::debug!` when `WM_BACKEND=wlroots` — the absence of sway IPC is expected, not a misconfig. Live verified: full `WM_BACKEND=wlroots` session now boots with zero WARN/ERROR lines in panel.log / launcher.log.
- [x] **W1c-12 — Vibewm WM keybindings** (2026-04-29): under `WM_BACKEND=wlroots`, vibewm previously forwarded every keystroke to the focused client — Mod+Space, Mod+Tab, zoom keys etc. all leaked through to apps instead of triggering the daemon. (Sway-mode reads bindings generated by `crates/sway/src/bin/generate-bindings.rs`; vibewm has no equivalent reader.) New `apps/vibewm/src/keybindings.rs` holds a hardcoded `BINDINGS` table (17 entries) covering the must-haves: Mod+Space → launcher, Mod+/ → cheatsheet, Mod+Tab/Shift+Tab → cycle-cluster, Mod+= / Mod+- → zoom-in/out-mode, Mod+. / Mod+, → cycle-strip, Mod+Shift+E → logout, Mod+Shift+R → reload, Mod+Shift+{M, Up/Down/Left/Right, Enter, Esc} → keyboard-move flow. Vibewm's keyboard input filter (in `apps/vibewm/src/input.rs`) checks `modifiers.logo` (Super) and the keysym against `BINDINGS` on key press, returning `FilterResult::Intercept(())` when matched and forwarding otherwise. Spawn helper distinguishes `spawn <bin>` (direct binary launch for launcher/cheatsheet) from anything else (forwards to `vibeshellctl <argv>`). Future enhancement: load from `~/.config/vibeshell/keybinds.toml` rather than hardcoded.
- [x] **W1c-13 — Panel event-driven refresh** (2026-04-29): panel was the last GTK client still pure-poll (every 500 ms). Mirrored W1c-8's overlay pattern: spawned a parallel `wm::WlrootsBackend::spawn_event_stream` thread that pulses a wakeup channel each `WorkspaceOrWindow` event; the existing poll thread switched its inter-tick sleep from `thread::sleep(poll_interval)` to `wakeup_rx.recv_timeout(poll_interval)` so a wakeup short-circuits the wait. Cluster switches and new toplevels now propagate to panel within event RTT instead of up to 500 ms later. Sway-mode behavior unchanged (the subscribe thread silently no-ops if vibewm isn't running). `wm` dep added to `apps/panel/Cargo.toml`. Live verified: vibewm logs `client subscribed to events subscribers=1` from the panel.
- [x] **W1c-14 — Configurable vibewm keybindings** (2026-04-29): W1c-12's hardcoded `BINDINGS` table moved behind a TOML loader. User config at `$XDG_CONFIG_HOME/vibeshell/keybinds.toml` (default `~/.config/vibeshell/keybinds.toml`; `VIBESHELL_KEYBINDS` env var overrides). Schema: `[[bindings]] keysym="space" modifiers=["super"] action="spawn launcher"`. Modifier names: super/logo/mod4/win, shift, ctrl/control, alt/mod1. Keysyms parsed via `xkb::keysym_from_name` with case-insensitive flag. Action split on whitespace; first token `"spawn"` runs the named binary directly, otherwise forwarded to `vibeshellctl`. Falls back to hardcoded defaults (17 entries unchanged from W1c-12) if the file is missing, malformed, or empty. `common::spawn_reload_listener` wired so `vibeshellctl reload` (or SIGHUP to vibewm) re-reads the config without restarting. 6 unit tests for the parser. `serde` + `toml` deps added.
- [x] **W1c-15 — Example keybinds.toml** (2026-04-29): `dev/keybinds.toml.example` ships a copy-and-edit starting point exercising every modifier + keysym name in the W1c-14 parser. Live-verified by booting vibewm with `VIBESHELL_KEYBINDS=dev/keybinds.toml.example` — logs `loaded keybindings from config count=17`.
- [x] **W1c-16 — DRM backend, working** (2026-04-29): `apps/vibewm/src/udev.rs` is a single-GPU, single-output DRM compositor against smithay 0.7's `LibSeatSession` + `UdevBackend` + `LibinputInputBackend` + `DrmDevice` + `DrmCompositor` + GBM/EGL/GLES. New `udev` Cargo feature (`smithay/backend_libinput,backend_udev,backend_drm,backend_gbm,backend_session_libseat`); selected at runtime by `VIBEWM_BACKEND=udev`. **Live verified in a Fedora 44 KVM/virt-manager VM:** vibewm boots with seatd, claims seat0, opens `/dev/dri/card0`, picks the connected `Virtual-1` connector at 1280x800@75, initializes EGL/llvmpipe, brings up DrmCompositor, and successfully queues frames that hit the display. virsh-screenshot of the VM display while vibewm runs idle shows the dark-gray clear color (RGB ~13,13,18 = `[0.05, 0.05, 0.07]`) — confirmed by an earlier diagnostic where switching the clear color to bright red lit up the entire screen red. Iteration to green took 5 fix rounds: `drm::control::Device` trait import, `EGLDisplay::new` unsafe block, `PathBuf::to_path_buf` for udev path arg, `DeviceFd::from(OwnedFd)` (smithay 0.7 `Session::open` returns `OwnedFd` directly), `OutputModeSource` public path under `smithay::output`. Plus runtime fixes: don't double-call `frame_submitted` (only ack via VBlank handler), treat `EmptyFrame` as benign-and-reschedule (1 s idle poll), drive renders from wayland surface commits via `udev::schedule_render` called from `CompositorHandler::commit`. Still-stubbed: hot-plug, multi-output, multi-GPU, DRM lease, dmabuf protocol, drm-syncobj, smithay-drm-extras EDID, visual cursor, damage-tracked partial redraw.
- [x] **W1c-17 — DRM backend renders xdg_shell clients** (2026-04-29): `foot` connects to `vibewm` under `VIBEWM_BACKEND=udev` and renders end-to-end. Required two fixes after the empty-DRM render path was working: (1) `state.display_handle.flush_clients()` at the end of `render_node` — without it, registry/configure/frame-callback responses sat in the server-side output buffer indefinitely and clients blocked forever waiting for replies (the winit backend already flushes after each redraw at `winit.rs:118`); (2) verify `space.map_element` runs from the xdg-shell `new_toplevel` handler (it does, at `handlers.rs:202`). Verified in the Fedora KVM VM via virsh-screenshot: foot terminal visible at (0,0) sized 1216×704 with the green prompt and cursor, vibewm dark-gray clear color filling the right (64 px) and bottom (96 px) gutters.
- [x] **W1c-18 — DRM backend renders the full vibeshell session** (2026-04-29): panel + launcher + notifd all render under `WM_BACKEND=wlroots VIBEWM_BACKEND=udev` end-to-end. Required fix: `new_layer_surface` must call `surface.send_configure()` after `map.map_layer(...)` to send the *initial* configure. `LayerMap::arrange` only calls `send_pending_configure` when `initial_configure_sent` is already true (the spec mandates the initial configure follows the client's initial commit, which fires our `new_layer_surface` callback) — without the explicit send, GTK4 panels never received a size and the WlSurface never got a buffer attached, so `Space::render_elements_for_output` filtered them out at the bbox-vs-output_geo overlap check. VM virsh-screenshot shows the launcher modal (full app list with icons + search bar — Audio Player, Boxes, Calculator, Calendar, Camera, Characters, Clocks, Color Profile Viewer, Connections, Contacts) and the panel top strip ("y N/A 17:27" workspace + clock + status). Two GTK env vars required for llvmpipe-only VMs: `GSK_RENDERER=cairo` and `LIBGL_ALWAYS_SOFTWARE=1` to bypass the failing Vulkan/Zink path; bare-metal hardware should work on the GL renderer without these.
- [x] **W1c-19 — Visible cursor on DRM backend** (2026-04-29): a 12×12 black square inside a 16×16 white border, drawn at the pointer's `current_location()` via two `SolidColorRenderElement`s prepended to the `OutputRenderElements` list (a new `render_elements!`-defined wrapper enum that lets `SpaceRenderElements` and the cursor share the same `DrmCompositor::render_frame` element type). `Kind::Unspecified` instead of `Kind::Cursor` so the cursor renders through the GLES primary plane (the DRM cursor plane is unreliable on virtio-gpu). New `InputEvent::PointerMotion` handler in `apps/vibewm/src/input.rs` integrates libinput's relative deltas (clamped to output geometry) since bare hardware emits relative motion, not absolute. Render kicks via `udev::schedule_render(self)` after every motion/button event so the cursor follows the pointer without waiting for a wayland surface commit. Verified in the Fedora KVM VM via virsh-screenshot at cursor location (11, 228). Real xcursor / client-set cursor surfaces is a follow-up.
- [x] **W1c-20 — xcursor + client-set cursor surfaces** (2026-05-03): new `apps/vibewm/src/cursor.rs` (gated behind `feature = "udev"`) holds a `CursorTheme` cache that lazy-loads xcursor glyphs from the system theme (configurable via `XCURSOR_THEME` / `XCURSOR_SIZE`, default `default`/24) using the `xcursor` crate. `build_cursor_elements` dispatches on `CursorImageStatus`: `Hidden` → no element; `Surface(WlSurface)` → walks the surface tree via `render_elements_from_surface_tree` and honors the client's hotspot from `CursorImageSurfaceData`; `Named(CursorIcon)` → uploads the matching xcursor frame to a `MemoryRenderBuffer` and renders via `MemoryRenderBufferRenderElement`. Falls back to W1c-19's black-square placeholder when the theme has no glyph at all. `OutputRenderElements` in `udev.rs` gained `CursorSurface` / `CursorImage` / `CursorFallback` variants (was just `Cursor=SolidColorRenderElement`). `SeatHandler::cursor_image` (was a stub) now stores the requested status on `Vibewm.cursor_status` and kicks a render so swaps are visible without waiting on the next surface commit. Cursor surfaces aren't tracked in `Space`, so `render_node` sends frame callbacks on the cursor surface tree separately — without this, animated client-set cursors would never advance past frame 1. Animated multi-frame xcursor playback, per-icon surface-tree rendering optimization, and `Kind::Cursor`-via-DRM-cursor-plane (vs forced GLES primary plane) are follow-ups. `xcursor = "0.3"` added as a dep.
- [x] **W1c-21 — Animated cursor playback** (2026-05-04): `CachedImage` was a single-frame holder; now wraps `Vec<CachedFrame>` (each carrying its own `delay`) plus the precomputed `total_duration`. xcursor groups frames by nominal `size`, so the loader picks the size closest to `XCURSOR_SIZE` and keeps every frame at that size — single-frame cursors get one entry, animated cursors (e.g. the wait spinner) get N. New `CachedImage::frame_at(elapsed)` walks the frame list and returns `(current_frame, time_until_next)`. `build_cursor_elements` returns `CursorRender { elements, next_frame_in: Option<Duration> }`; `udev::render_node` calls `schedule_render_after` with that delay so the next frame paints on time without waiting for any other event. Static cursors return `next_frame_in: None` and the existing event-driven render cadence handles them — no extra wakeups for the common case. Two unit tests lock the wraparound math (gated under `feature = "udev"` since the cursor module itself is).
- [x] **W1c-22 — libinput gesture integration** (2026-05-04): new `apps/vibewm/src/gestures.rs` accumulates pinch + 3-finger swipe state across libinput's Begin/Update/End events and turns them into daemon mutations on End. **3-finger horizontal swipe** (60 px threshold; horizontal-dominant only — vertical-dominant 3-finger swipes are reserved for future workspace-switcher / expose) cycles clusters via `vibeshellctl ipc cycle-cluster --direction {forward|backward}`. **Pinch** maps absolute scale-relative-to-begin to mode-zoom: `>= 1.20` (spread) → `zoom-in-mode`, `<= 0.83` (close) → `zoom-out-mode`; `(0.83, 1.20)` deadband ignores stray jitter. Cancelled gestures emit nothing. Dispatched via `Command::new("vibeshellctl").args(...).spawn()` to mirror the keybindings.rs pattern (same envelope, same audit trail). New `InputEvent::Gesture{Swipe,Pinch}{Begin,Update,End}` arms in `process_input_event` route through `state.gestures: GestureState`. Active under both backends — but only libinput (the udev backend) emits gesture events, so the winit dev path stays inert. Client-side gesture forwarding via `pointer-gestures-unstable-v1` is a follow-up. 9 unit tests cover swipe direction, threshold, vertical-dominance rejection, finger-count gating, cancellation, and pinch deadband / direction.
- [x] **W1c-24 — Client gesture forwarding via `wp_pointer_gestures_v1`** (2026-05-04): vibewm now exposes the `wp_pointer_gestures_v1` global (`PointerGesturesState::new::<Self>(&dh)` in `state.rs`, `delegate_pointer_gestures!(Vibewm)` in `handlers.rs`). Every libinput gesture event arm in `input.rs` does **both** consumer steps: (1) accumulates into `state.gestures` for the W1c-22 compositor-side bindings (cluster cycle, zoom-mode); (2) calls `pointer.gesture_swipe_*` / `gesture_pinch_*` / `gesture_hold_*` to forward to the focused client through smithay's protocol-routing. Both fire unconditionally — overlap is fine in practice since cluster-cycling is typically initiated over the desktop, not a content area. Smithay handles per-client subscription state and only forwards to clients that bound the global. Full per-event mapping: swipe carries serial/time/fingers/delta; pinch carries the same plus `scale` (absolute, vs begin) and `rotation` (degrees, relative); hold carries serial/time/fingers and a cancelled flag on End. Hold gestures are forward-only — no compositor binding today (reserved for future "show overview" or similar).
- [x] **W1c-25-2 — `VibewmEvent::ClusterMapped` event** (2026-05-04): foundation for the smooth-zoom seam fix. New `VibewmEvent::ClusterMapped { cluster, window_count }` variant fired by vibewm at the end of `sync_cluster_visibility` after each `ActivateCluster`/`CreateNamedWorkspace`/`BackAndForthWorkspace`. New `WmSignal::ClusterMapped` mirrors it on the daemon-side channel; `VibewmEvent::to_signal()` translates between the two so the wire stays additive. `WlrootsBackend::spawn_event_stream` now routes via `to_signal()` instead of pattern-matching the wire enum directly. Daemon's WM-event subscriber thread (`apps/vibeshellctl/src/daemon.rs:60`) was a `while let Ok(WorkspaceOrWindow) = recv()` that would have silently terminated on the first new variant — switched to an exhaustive match that funnels both signals into `DaemonEvent::WmChanged` for now (a dedicated `DaemonEvent::ClusterMapped` arrives in W1c-25-3 with the `ZoomTransition` sequencer). `mock-vibewm-control` mirrors the broadcast, so `just smoke-test-wlroots` exercises the new event on every cluster activation. 3 new unit tests in `crates/wm/src/vibewm_ipc.rs` lock the JSON wire shape and the event→signal mapping.
- [x] **W1c-25-1 — Symmetric Cluster→Overview exit animation** (2026-05-04): new `start_undive_anim` in `apps/overlay/src/ui/overview_canvas.rs` is the visual inverse of `start_dive_anim` — seeds the viewport at the cluster's dived-in pose (cluster center, daemon-viewport-scale × `DIVE_ZOOM_GAIN`), then eases-out-cubic over `DIVE_DURATION_MS` (220ms) back to the daemon-acknowledged pre-dive viewport. Triggered from `OverviewCanvas::set_canvas_state` via `pending_undive_target` which inspects the incoming W1c-25-3 `ZoomTransition`: fires only when `from = Cluster(c)` AND `to = Overview` AND we haven't already acted on this transition (deduped via `last_handled_transition_at: Option<u64>` on `WidgetState`, keyed by `started_at_ms`) AND the user isn't mid-drag/move (don't yank the viewport during interaction). The trigger runs *after* the borrow on `data` is dropped so the animation's `borrow_mut` doesn't panic. Pre-existing dive-on-Enter (`start_dive_anim`) untouched — this is purely the missing exit half.
- [x] **W1c-25-3 — Daemon-orchestrated transition sequencer** (2026-05-04): new `ZoomTransition { from, to, phase, started_at_ms }` + `TransitionPhase { Started, CompositorSettled }` types in `crates/common/src/contracts.rs`. `CanvasState.transition: Option<ZoomTransition>` populated in one place — `StateOwner::persist_after_mutation` detects any zoom change and stamps a fresh transition, so every existing zoom mutation site (zoom-in/out-mode, activate-cluster, select-cluster-for-zoom, drag-into-focus, etc.) opts in automatically. `StateOwner::advance_transition_on_cluster_mapped` flips phase to `CompositorSettled` when vibewm reports the matching cluster mapped (target derived from `to: ZoomLevel`); `clear_stale_transition(max_age)` runs every daemon tick and drops transitions older than 800ms (safety net for the sway backend, which never emits `ClusterMapped`, and for vibewm crash-mid-flip). New `DaemonEvent::ClusterMapped { cluster }` flows from `WmSignal::ClusterMapped` through the WM event subscriber thread; the daemon main loop drains it before re-ingesting so overlay sees the advance on its next poll. Smoke-test-wlroots gained 2 new assertions (transition present after `zoom-in-mode`; transition cleared after staleness timeout) — 19/19 passing. One incidental fix: clippy flagged `IpcResponse::State(CanvasState)` as oversized after the new field; allowed at the enum site since boxing would force every `GetState` (the hot path) through indirection to save bytes only on `Ack`/`Error`.
- [x] **W1c-25-7 — Daemon-side `Subscribe` channel** (2026-05-04): closes the seam where overlay only learned about daemon-side state mutations on its 1200 ms baseline poll. New `IpcRequest::Subscribe` upgrades a daemon socket connection to long-lived; daemon replies once with `IpcResponse::Subscribed` then pushes `IpcResponse::Event(DaemonEventKind::StateChanged)` after every successful `bump_revision` (skipping `MutationType::GetState` to avoid storms with subscribed pollers). Connection-stateful subscriber list lives on `StateOwner.event_subscribers: Vec<UnixStream>`; `register_event_subscriber` appends, `broadcast_state_changed` pushes to all and prunes dead ones. Daemon socket handler intercepts `Subscribe` before `dispatch_ipc_request`, clears the write timeout so slow clients don't break mid-push, and hands the stream off to the state owner. Overlay gained a third subscribe thread (alongside its sway and vibewm-control subscribers) that opens the daemon socket, sends `Subscribe`, and pulses overlay's existing refresh channel on every line. With this, the W1c-25-3 staleness timeout could be reverted from 1500 ms to 800 ms (overlay observes mutations within socket-RTT, no need for poll-cadence headroom). 3 new round-trip tests in `crates/common/src/contracts.rs` lock the wire shape (`subscribe_request_round_trips`, `event_response_round_trips`, `subscribed_response_serializes_with_expected_tag`). Smoke-test-wlroots gained 2 new assertions (Subscribed reply, StateChanged push after mutation) — 21/21 passing.
- [x] **W1c-25-4 — Vibewm window position interpolation** (2026-05-04): new `apps/vibewm/src/anim.rs` is the smithay-side counterpart to overlay's `start_undive_anim`. `WindowAnim { from, to, start, duration }` per `WindowId` lives on `Vibewm.window_anims`; `Vibewm::apply_layout_ops` now stages animations (rather than calling `Space::map_element` immediately) for every position-changing layout op, captured from the current `space.element_location` so an op landing mid-animation restarts smoothly from the interpolated position. Sizes still configure immediately — animating `xdg_toplevel.configure(size)` would fight the client's redraw cadence and produce flicker. Render loop (`udev::render_node`) calls `tick_window_anims(now)` before building elements; while any anim is active the loop schedules another render ~16ms out. Default duration `220ms` matches overlay's `DIVE_DURATION_MS` so Cluster↔Focus and Cluster↔Overview transitions feel coherent end-to-end. `sync_cluster_visibility` deliberately bypasses anims (cluster switches want pop-in, not slide-in). 5 unit tests cover ease-out math, midpoint behavior, completion semantics, no-op stage on already-at-target, and restart-from-interpolated-position. vibewm tests: 22 → 27.
- [x] **W1c-25-5a — Custom-IPC thumbnail pipeline (placeholder image)** (2026-05-04): chose option (b) from the deferred analysis — custom IPC instead of `wlr-screencopy-v1`. New `ClusterThumbnail { width, height, rgba_base64 }` type in `crates/common/src/contracts.rs`. New `IpcRequest::GetClusterThumbnail` + `IpcResponse::Thumbnail` / `ThumbnailMissing` (split because Serde's internally-tagged enum can't carry `Option<T>` newtype variants). New `WmBackend::capture_cluster_thumbnail(cluster, max_w, max_h)` trait method with default `Ok(None)` impl; sway backend inherits the default; `WlrootsBackend` proxies via new `VibewmRequest::CaptureClusterThumbnail`. Vibewm responds with a procedural placeholder (hue-shifted vertical gradient + per-window strip rectangles, keyed by cluster id) — real GlesRenderer offscreen capture is W1c-25-5b. Daemon caches per-cluster on the `ClusterMapped` event; new `StateOwner.cluster_thumbnails: BTreeMap<ClusterId, ClusterThumbnail>` with `cluster_thumbnail()`/`set_cluster_thumbnail()`/`prune_thumbnails()`. Overlay fetches via daemon socket (`fetch_thumbnail_surface`), decodes RGBA→Cairo ARgb32 (premultiplied BGRA byte swap), caches as `cairo::ImageSurface` per cluster, paints under the cluster card in `draw_cluster_card`. Mock-vibewm-control returns the same procedural pattern for smoke-test exercise; smoke-test-wlroots gained 2 assertions (thumbnail-typed response, non-empty rgba_base64 payload) — 23/23 passing. New CLI `vibeshellctl ipc get-cluster-thumbnail <cluster>` for ad-hoc inspection.
- [x] **W1c-25-5b — Real offscreen GlesRenderer capture** (2026-05-04): `Vibewm::capture_cluster_thumbnail` now tries a real smithay offscreen render before falling back to W1c-25-5a's placeholder. Path (gated under `feature = "udev"`): grab the first DRM device's `(GlesRenderer, Output)` via new `UdevState::first_renderer_and_output`; collect space elements at native scale via `space.render_elements_for_output`; create an `Offscreen<GlesTexture>` of the thumbnail size preserving output aspect; `Bind` it; `Renderer::render` opens a frame at thumbnail-size physical; clear with vibewm's clear color; `draw_render_elements` at `thumb_w / out_w` scale; `Frame::finish`; `ExportMem::copy_framebuffer` + `map_texture` for bytes; in-place BGRA→RGBA channel swap; base64 + return. Active cluster only — inactive clusters fall through to the placeholder (their windows aren't currently mapped in the space, so we have nothing to render). winit backend doesn't get the path (renderer lives inside the calloop event source, not on `Vibewm`); dev-mode keeps the placeholder. Build verified default-features (the cfg gate keeps the smithay code compiling); real behavior verified in the Fedora KVM VM where the udev backend actually runs.
- [x] **W1c-25-6 — Focus mode context strip animation + try_build_frame focus-context bug fix** (2026-05-04): cycle-strip / Cluster→Focus animations are now subsumed by W1c-25-4 — the daemon's layout engine emits new `LayoutOp`s for every window in the cluster on focus change, and `Vibewm::apply_layout_ops` animates them all via `window_anims`. **Pre-existing bug fixed en route**: `FramePipeline::try_build_frame` (`crates/wm/src/layout.rs:521`) hard-coded `LayoutComputeContext::default()` (Cluster mode), so the daemon's `layout_context()` method computed the right `Focus` context but it was silently dropped by the pipeline — Focus mode never actually fired. New signature `try_build_frame(now, clusters, current_geometry, context)`; daemon passes the value it computes. New regression test `try_build_frame_propagates_focus_context` pins the dominant=750/strip=125 split a Focus context produces. wm crate tests: 15 → 16.
- [x] Session script — kept as separate scripts (`scripts/start-sway-session` for the sway backend, `scripts/start-vibeshell-session` for vibewm/wlroots, shipped in W1c-4) rather than a single switch-on-`WM_BACKEND` script. The two boot paths diverge enough (sway-IPC waits, swaymsg keybinding generation, output enumeration via swaymsg vs vibewm registry) that one branched script would be more conditionals than shared logic.
- [x] Layer-shell, xdg-shell, xdg-decoration, xwayland protocols — all live: xdg-shell + wlr-layer-shell in W1b; xdg-decoration in W1c-4; xwayland in W1c-9 with X11 move/resize-grab adapter in W1c-10.
- [x] **Migration path — wlroots smoke test in CI** (2026-05-04): new `crates/wm/src/bin/mock-vibewm-control.rs` is a tiny in-memory implementation of the `VibewmRequest`/`VibewmResponse` JSON-line protocol that satisfies the daemon's `WlrootsBackend` without smithay/GPU/wayland. New `scripts/smoke-test-wlroots` boots `mock-vibewm-control`, starts `vibeshellctl daemon` under `WM_BACKEND=wlroots`, and runs the same kinds of checks as the sway smoke test (get-state, create-cluster, select+keyboard-move, zoom-in-mode, cycle-cluster, persistence + crash recovery) — 17/17 passing. Promoted to `just ci` (the sway-headless `smoke-test` recipe stays separate since it needs sway and a GUI-ish env). Both backends now stay green on every CI run; regressions in the daemon's wlroots wire path will fail before merge instead of in the VM.
- [ ] Retire Sway backend once parity + soak window pass (separate cleanup PR)

**Exit criteria:** vibeshell runs as its own compositor; smooth Overview↔Cluster zoom; live thumbnails visible in Overview; pinch/swipe gestures functional; existing IPC + state persistence unchanged.

---

### [ ] Phase 9 — Robustness & system integration

**Scope decision (2026-04-29):** GTK stack upgrade explicitly **deferred** (already audited as not-exploitable; pure dep hygiene, not blocking).

- [x] **DBus migration for panel network status** (2026-04-29): `nmcli` subprocess polling replaced with `zbus::blocking` reads of `org.freedesktop.NetworkManager` (`Connectivity` u32 + `PrimaryConnection` object path → `Connection.Active.Type`). Connection cached on `NetworkProvider`, reset on read failure for self-healing. ~50–100× lower per-poll CPU than fork+nmcli. Audio (`wpctl`) **descoped** — PipeWire has no clean DBus surface; revisit when Phase 8 needs PipeWire for media-key handling.
- [x] **Daemon resilience — IPC connect retry** (2026-04-29): single 100 ms-delayed retry inside both `try_dispatch_via_socket` (overlay) and the vibeshellctl IPC client when the initial `UnixStream::connect` fails, so user actions firing during a daemon restart aren't silently dropped. Read/write errors are not retried (mutations may have side effects).
- [x] **Typed partial-failure `IpcResponse` errors** — descoped 2026-04-29. Roadmap framing assumed batched mutations (with partial-success states); the protocol has none — every IPC is a single mutation with a single response. Renaming `String` → enum-wrapping-`String` without that use case is ceremony. Revisit if/when batched mutations are introduced.

**Exit criteria (revised):** panel network status now reads via DBus (no `nmcli` fork); a brief daemon restart no longer drops in-flight user actions on a single socket retry. Audio DBus and event-driven push deferred per descopes above.

---

### [ ] Phase 10 — Daily-driver readiness

**Theme:** what does it take for vibeshell to replace mutter on a real Ubuntu install? Each tier represents a hard blocker for the tier below it. Don't start Tier 2 until Tier 1 is done — order matters because users hit them in this exact sequence.

Status as of 2026-05-04: vibewm boots and runs the shell (Phase 8 ✓), but daily-driver replacement is gated on the protocol surface below. Estimated ordering, not commitments — each protocol has edge cases that surface only under real-world use.

#### Tier 0 — Boot from gdm

Without these, you can't even get into vibeshell from a normal login screen.

- [ ] **`vibeshell.desktop` session entry** — `/usr/share/wayland-sessions/vibeshell.desktop` invoking `start-vibeshell-session` with proper `Type=Application`, `Exec=`, `DesktopNames=vibeshell`. Install via package or `make install` target.
- [ ] **logind session leader claim** — vibewm already calls `LibSeatSession::new()` (Phase 8 W1c-16); verify it cleanly takes over from gdm-launched session and surrenders on logout. Probably needs minor tweaks once tested in-anger.
- [ ] **Logout exits cleanly to login screen** — vibewm needs to receive SIGTERM gracefully, drop the seat, signal the session bus that the session is over. Currently vibewm exits but doesn't notify systemd-logind.

#### Tier 1 — Apps that don't render or break instantly

The complaints any user files within 5 minutes.

- [ ] **`wlr-foreign-toplevel-management-v1`** — every taskbar (waybar, the Ubuntu dock if anyone tries it, even our own panel for window-list integration) needs this to enumerate windows and switch focus. Smithay ships server-side support. ~1 session.
- [ ] **`wlr-data-control-v1`** — clipboard managers (`wl-copy`, `wl-paste`, clipman, copyq) need this. Without it, copy/paste between apps works but clipboard history doesn't. Smithay support exists. ~1 session.
- [ ] **`wlr-screencopy-v1`** — OBS, screenshot tools (grim/slurp), screen recording (wf-recorder), Zoom/Discord screen share. Was deferred in W1c-25 in favor of custom IPC for our own thumbnails; daily-driver use needs the actual protocol. **Multi-session lift** — frame copy semantics, dmabuf negotiation, per-frame state machine. The hardest single Tier 1 item.
- [ ] **`wp-fractional-scale-v1` + `wp-viewporter`** — anyone with a HiDPI laptop (most modern Ubuntu installs) gets either blurry or comically-sized everything without these. Smithay has both. ~1 session.
- [ ] **`xdg-foreign`** — file pickers in Flatpak/sandboxed apps use this to grant access to dirs outside the sandbox. Without it, file dialogs in Firefox-as-snap, Slack, Discord are broken. ~1 session.

#### Tier 2 — Daily friction without

You can boot and use most apps, but life is annoying.

- [ ] **`ext-session-lock-v1`** — swaylock, gtklock, or our own lock screen. Without this, you can't lock the screen. Smithay support exists. ~1 session for the protocol; another for a vibeshell-themed lock screen UI.
- [ ] **`idle-notify-v1` + `idle-inhibit-v1`** — screen lockers to know when to lock; mpv/zoom to keep screen awake during video. Smithay support exists. ~1 session both.
- [ ] **`xdg-desktop-portal` backend** — Flatpak apps (most Ubuntu desktop apps in 2026) use portals for file dialogs, screen share, opening URLs, settings access. Either implement `xdg-desktop-portal-vibeshell` from scratch or bridge to `xdg-desktop-portal-wlr` (the wlroots reference impl) by exposing the protocols it needs (screencopy, foreign-toplevel, data-control). Bridging is faster but constrains us to whatever wlr supports. **Multi-session.**
- [ ] **`wlr-output-management-v1`** — `wlr-randr`, GUIs like `nwg-displays`, system tools that twiddle resolution/refresh. Without it: locked at boot config. Smithay has the protocol. ~1 session.
- [ ] **`wp-cursor-shape-v1`** — newer cursor protocol that Qt 6 apps prefer. Falls back to wp_pointer.set_cursor (W1c-20) if missing, but loses the shape-name semantics. ~half session.
- [ ] **Output hot-plug** — vibewm's UdevEvent::Added handler is currently `info!("hot-plug not handled yet")`. Plugging in an external monitor today does nothing. Wire the existing handler to `open_drm_device` + register the new output with `space.map_output`. ~1 session — risk is in the edge cases (mode change, primary output change, cluster repositioning).

#### Tier 3 — Power/laptop behavior

If your machine moves, these matter.

- [ ] **Lid-close suspend** — listen on logind's `PrepareForSleep` D-Bus signal; trigger compositor suspend (turn off outputs, lock if configured). ~1 session.
- [ ] **Brightness keys (XF86MonBrightnessUp/Down)** — wired in keybindings already (spawn `brightnessctl`); verify they actually fire under the udev backend. Likely already works.
- [ ] **Battery indicator + low-battery warnings** — panel reads via UPower D-Bus; toast when <10%. ~half session.
- [ ] **Audio media keys (XF86AudioRaiseVolume etc.)** — currently spawn `wpctl`; verify under udev. Note Phase 9 descope: native PipeWire is bigger work — spawning is fine for now.

#### Tier 4 — Hardware coverage

Long tail. Hit-or-miss for any given user.

- [ ] **Multi-GPU (laptop hybrid graphics)** — vibewm picks the first DRM device. Real laptops have iGPU + dGPU, want render-on-dGPU + scanout-on-iGPU. Smithay has multigpu support. **Multi-session.**
- [ ] **NVIDIA proprietary** — separately fraught; smithay generally works on Nouveau but proprietary needs `wl_drm` + EGL/dmabuf paths that break in subtle ways. Realistically: blocked until you have an NVIDIA box to test on.
- [ ] **DRM lease (VR headsets, VRR monitors)** — `wp-drm-lease-v1`. Niche but the people who care REALLY care.
- [ ] **HDR / 10-bit color** — Wayland's HDR story is still in flux; protocols haven't stabilized. Defer indefinitely.
- [ ] **Touchscreen** — vibewm forwards touch events via smithay; verify pinch-to-zoom and tap-as-click work on a touchscreen-capable VM or device.

#### Tier 5 — Accessibility + i18n

Easy to defer; impossible to ship a real DE without.

- [ ] **IME / input method support** — `text-input-v3` + integration with IBus/fcitx for users typing CJK, dead-keys for European layouts. **Multi-session.**
- [ ] **Accessibility tree** — `at-spi` integration for screen readers (Orca). Smithay doesn't help here; this is GTK-app responsibility for our own apps + the wider compositor a11y story for clients.
- [ ] **High-contrast / large-text themes** — STYLE.md has a single palette. Add a high-contrast variant + size-token system. ~1 session.

#### Tier 6 — Polish + parity with mutter quirks

- [ ] **xdg-activation-v1** — apps requesting focus when launched from a different app (e.g. Slack notification opens browser to a URL). Smithay has it. ~half session.
- [ ] **`wlr-virtual-pointer-v1` / `wlr-virtual-keyboard-v1`** — for `ydotool`, accessibility tools, automation. ~1 session.
- [ ] **Per-app notification preferences** — notifd already routes; add a per-app rules file for "do not disturb", "urgent only", etc. ~1 session.

#### Suggested sequencing

**Path A — Curated workflow** (terminal + browser + editor, single monitor, no Flatpaks): Tier 0 → Tier 1 (skip xdg-foreign + screencopy) → Tier 2 (skip portals). Probably 2-4 weeks of focused work. You're daily-driving for "real coding" use cases.

**Path B — Full Ubuntu replacement**: Tier 0 → Tier 1 → Tier 2 → Tier 3 → Tier 4. Realistic estimate: 6-12 months. The xdg-desktop-portal piece is the boss fight — Flatpak apps are most of the Ubuntu desktop in 2026, and without portal support they're crippled.

**Decision criterion for "ready"**: dual-session for ≥2 weeks of daily use, falling back to GNOME for specific tasks. Track each fallback as a Phase 10 item. When the fallback list goes a week without growing, vibeshell is ready.
