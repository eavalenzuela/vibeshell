# Continuum WM — Roadmap

## v1 Feature Goals

### Must-have interactions
- [x] Zoom navigation — Overview → Cluster → Focus levels
- [x] Bindings: Mod+Wheel (zoom), Mod+Drag / Mod+Arrows (pan), Mod+Enter (dive), Mod+Esc (zoom out)
- [x] Clusters (task neighborhoods) — create, rename, move on canvas
- [x] Assign focused window to cluster
- [ ] "Auto-cluster" heuristic v1: by app_id/class (optional toggle)
- [ ] Fast recall — "Recent clusters" switcher (Alt-Tab equivalent for clusters)
- [ ] Launcher can search windows and clusters
- [ ] Persistence — save/restore cluster positions, window→cluster mapping, last viewport/zoom
- [ ] Graceful handling when windows are missing (apps closed)

### Must-have desktop-reality features
- [ ] Multi-monitor: per-output viewport (even with one global canvas)
- [ ] Basic rules: float dialogs, keep transient windows attached to parent
- [ ] Handle special windows: fullscreen, scratchpad, modals, popups
- [ ] Non-jank repositioning: debounce geometry updates, respect manual resize overrides

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

### [ ] Phase 4B — Overview polish *(depends on Phase 6 frame-time data)*

- [ ] Snap-to affordances during drag/move (grid lines, card centerlines, output center)
- [ ] Snap ghost preview within 24 px of snap target
- [ ] Inertial panning behavior
- [ ] Animation timing/easing tuning for overview transitions and recentering

> **Note:** Complete alongside/after Phase 6 so interaction smoothness work is informed by measured frame-time/input-latency behavior.

---

### [ ] Phase 5 — Persistence + robustness

- [ ] Persist: cluster positions, window→cluster assignment hints (by app_id + title patterns)
- [ ] Persist: last active cluster + viewport/zoom level
- [ ] Fullscreen window handling (temporarily override zoom)
- [ ] Dialogs transient_for parent (keep near parent, don't strip)
- [ ] Multi-output: at least don't break; per-output viewport ideally

**Exit criteria:** restart session and the workspace landscape comes back sensibly.

---

### [ ] Phase 6 — Performance polish

- [ ] Reduce IPC chatter: only apply diffs above thresholds
- [ ] Batch Sway commands (semicolon-separated per frame)
- [ ] Avoid frequent `get_tree` — prefer event-driven state
- [ ] Throttle cluster position writes to ≤30 Hz during drag; final unthrottled commit on release
- [ ] Viewport pan/zoom writes ≤20 Hz during gesture + final commit
- [ ] Optional: simple overlay animations (not moving windows smoothly yet)

**Exit criteria:** no "constant resizing" feeling; CPU stays sane.

---

### [ ] Phase 7 — Compositor decision

- [ ] Evaluate whether wlroots compositor is needed
- [ ] Trigger conditions: smooth transitions, true scene-graph thumbnails, deep gesture integration
- [ ] If proceeding: port to `WlrootsBackend` implementing the existing `WmBackend` trait
- [ ] State model, layout engine, IPC protocol, and UX carry over — compositor is a new backend, not a rewrite
