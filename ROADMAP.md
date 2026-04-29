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
- [ ] **W1c-15+** — smooth zoom transitions (overlay animations + compositor mode-change blends); live thumbnails (smithay scene graph + buffer-capture); DRM backend (~2000-line port from anvil — multi-session); libinput gesture integration.
- [ ] **W1c-4+** — xdg-shell move/resize grabs wired via the daemon drag flow; smooth zoom transitions; live thumbnails; DRM backend; gesture input.
- [ ] Scene-graph rendering pipeline (replaces Sway's tree → unlocks live thumbnails in Overview)
- [ ] Smooth zoom transitions (Overview ↔ Cluster ↔ Focus) — was a v1 non-goal under Sway, in-scope here
- [ ] Gesture integration via `libinput` events (pinch-to-zoom Overview, swipe between clusters)
- [ ] Session script: `WM_BACKEND=wlroots|sway` switch in `scripts/start-sway-session` (rename to `start-vibeshell-session`)
- [ ] Layer-shell, xdg-shell, xdg-decoration, xwayland protocols
- [ ] Migration path: smoke-test passes against `WlrootsBackend`; both backends stay green until parity reached
- [ ] Retire Sway backend once parity + soak window pass (separate cleanup PR)

**Exit criteria:** vibeshell runs as its own compositor; smooth Overview↔Cluster zoom; live thumbnails visible in Overview; pinch/swipe gestures functional; existing IPC + state persistence unchanged.

---

### [ ] Phase 9 — Robustness & system integration

**Scope decision (2026-04-29):** GTK stack upgrade explicitly **deferred** (already audited as not-exploitable; pure dep hygiene, not blocking).

- [x] **DBus migration for panel network status** (2026-04-29): `nmcli` subprocess polling replaced with `zbus::blocking` reads of `org.freedesktop.NetworkManager` (`Connectivity` u32 + `PrimaryConnection` object path → `Connection.Active.Type`). Connection cached on `NetworkProvider`, reset on read failure for self-healing. ~50–100× lower per-poll CPU than fork+nmcli. Audio (`wpctl`) **descoped** — PipeWire has no clean DBus surface; revisit when Phase 8 needs PipeWire for media-key handling.
- [x] **Daemon resilience — IPC connect retry** (2026-04-29): single 100 ms-delayed retry inside both `try_dispatch_via_socket` (overlay) and the vibeshellctl IPC client when the initial `UnixStream::connect` fails, so user actions firing during a daemon restart aren't silently dropped. Read/write errors are not retried (mutations may have side effects).
- [x] **Typed partial-failure `IpcResponse` errors** — descoped 2026-04-29. Roadmap framing assumed batched mutations (with partial-success states); the protocol has none — every IPC is a single mutation with a single response. Renaming `String` → enum-wrapping-`String` without that use case is ceremony. Revisit if/when batched mutations are introduced.

**Exit criteria (revised):** panel network status now reads via DBus (no `nmcli` fork); a brief daemon restart no longer drops in-flight user actions on a single socket retry. Audio DBus and event-driven push deferred per descopes above.
