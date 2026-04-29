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

- [ ] Stand up `WlrootsBackend` implementing the existing `WmBackend` trait (parallel to current `SwayBackend`, gated by env/config)
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
