# Gap Remediation Plan

Status snapshot as of 2026-04-29. Captures all open work across the roadmap in one view, sequenced by dependency and risk. Companion to `ROADMAP.md` — the roadmap is the canonical "what"; this doc is the "how, in what order, and why."

## Summary of remaining work

| Bucket | Items | Effort | Risk |
|---|---|---|---|
| Phase 5 carryover — scratchpad | 1 | S | Low |
| Phase 6 — reactivated deferrals | 2 | S | Low |
| Phase 8 — wlroots compositor port | 7 | XL | High |
| Phase 9 — DBus + daemon resilience | 4 | M | Med |
| Deferred (tracked, not scheduled) | 1 (GTK upgrade) | M | Low |

Net: 14 active work items across 4 phases. Phase 8 dominates total effort (~70%); everything else is quick relative to it.

---

## Sequencing principle

**Land cheap wins first, then commit to wlroots.** Phase 5 + 6 + 9 are mostly orthogonal to compositor choice — they harden the existing Sway backend, and the same code carries forward to the wlroots backend (state store, IPC, daemon, panel). Doing them first means:

1. The wlroots port lands on a stable, polished base instead of a moving target.
2. Smoke-test coverage grows before the high-risk swap.
3. Phase 8's "soak window" can run against a known-good Sway baseline for parity comparison.

Inverse argument (do wlroots first): the compositor port may invalidate panel/overlay assumptions, making panel DBus work feel premature. Mitigation: panel and DBus migration are compositor-agnostic — both backends speak the same overlay/panel/IPC protocol.

**Recommended order: Phase 5 scratchpad → Phase 6 reactivated → Phase 9 → Phase 8.**

---

## Phase 5 carryover — Scratchpad (1 item, S)

### S1. Scratchpad as `WindowRole::Scratchpad`
- **Why**: Last `[ ]` in must-have desktop-reality features. Sway's scratchpad windows currently leak into clusters or get layout-engine-managed, neither correct.
- **Where**:
  - `crates/common/src/contracts.rs` — add `WindowRole::Scratchpad` variant
  - `crates/sway/src/` — detect scratchpad via `swayipc` node `scratchpad_state` field on ingest
  - `apps/vibeshellctl/src/state_store.rs` — `ingest_sway_facts()` skips scratchpad windows for cluster auto-assignment; `LayoutExclusionReason::Scratchpad` added; `anchor_transient_dialogs()` style exclusion in layout pass
  - `apps/overlay/src/ui/overview_canvas.rs` — scratchpad windows hidden from cluster cards (or shown with badge — TBD)
- **Tests**: unit test in vibeshellctl exercising ingest with a scratchpad node; smoke-test asserting layout engine skips it
- **Exit**: send a window to scratchpad; verify it doesn't appear in any cluster, isn't laid out, and persists across daemon restart

---

## Phase 6 — Reactivated deferrals (2 items, S)

### ~~P6.A. Viewport pan/zoom IPC throttle ≤20 Hz~~ — superseded 2026-04-29
After surveying the dispatch sites in `overview_canvas.rs`: drag-pan already fires only on `drag_end` (one IPC per gesture), inertia/viewport-anim only fire at loop end, keyboard pan is event-driven by user input. The remaining sustained path is scroll-wheel zoom, but `OverviewZoom` takes a sign-only `delta` with fixed `STEP=1.12`; throttling drops zoom steps rather than coalescing them. A correct fix requires changing the protocol to send absolute scale — a larger refactor than the throttle is worth. Phase 8 (wlroots) replaces input dispatch entirely.

### P6.B. Simple overlay animations — done 2026-04-29
- Cluster-dive ease-in: 220 ms ease-out-cubic pan + zoom (1.4× scale gain, clamped to MAX_SCALE) before firing `on_dive`/`on_activate` to flip zoom level. Both dive sites (double-click and Enter) call `start_dive_anim`.
- Generalized `ViewportAnim` to interpolate scale (`start_scale`/`target_scale`) and to fire an `on_complete` callback once at t=1.0 (taken out via `Option::take()` for re-entry safety). `start_recenter_anim` now delegates through a shared `start_viewport_anim`.
- **Deferred to Phase 8**: Overview ↔ Cluster ↔ Focus mode-change animations. Those require the overlay to render non-Overview modes, which is a wlroots-era responsibility (the current overlay only owns Overview rendering).

---

## Phase 9 — DBus + daemon resilience (4 items, M)

### P9.1. Panel network status via DBus (NetworkManager)
- **Why**: Current `nmcli` subprocess poll is wasteful and laggy. NetworkManager exposes `org.freedesktop.NetworkManager` on the system bus with PropertiesChanged signals.
- **Where**: `apps/panel/src/status/network.rs` (or equivalent — verify path before editing). Use `zbus` crate (sync API to match GTK main loop, or `tokio` + glib mainloop bridge).
- **Risk**: zbus brings async runtime concerns into panel. Decide between blocking `zbus::blocking` (simpler) vs `zbus` async + glib integration (more work, lower CPU).
- **Tests**: unit test for parsing NM device state enum; manual: toggle wifi, verify panel updates within ~100 ms
- **Exit**: zero `nmcli` subprocess invocations during normal panel operation

### ~~P9.2. Panel audio status via DBus~~ — descoped 2026-04-29
PipeWire has no clean DBus surface; the `pipewire` Rust crate carries C build deps (libpipewire-dev, clang) and is pre-1.0. `wpctl` polling stays as-is. Revisit when Phase 8 needs PipeWire for media-key handling — natural to do native libpipewire integration there.

### P9.3. IPC connect retry — done 2026-04-29 (smaller than originally planned)
Audit found existing behavior was already mostly self-healing: `try_dispatch_via_socket` opens a fresh `UnixStream` on every call (no caching), and there's already a subprocess fallback. Daemon restart heals on the next 1200ms poll automatically. The only real gap was that user *interactions* firing during the brief restart window were dropped silently. Fixed with a single 100 ms-delayed retry inside both `try_dispatch_via_socket` (overlay) and the vibeshellctl IPC client. Read/write errors are not retried (mutations may have side effects). UI daemon-alive hint not added — not needed at this scope.

### ~~P9.4. Typed `IpcResponse` errors~~ — descoped 2026-04-29
The roadmap framing was "surface which mutation succeeded vs. failed in batched requests." The protocol has no batched mutations — every IPC is a single mutation with a single response. Without that use case, typed errors are renaming `String` to an enum-wrapping-`String`: pure ceremony. Revisit if/when batched mutations are designed.

---

## Phase 8 — wlroots compositor port (7 items, XL)

### Sequencing within Phase 8

The port is large but decomposable. Land in this order:

1. **W1**: `WlrootsBackend` skeleton behind feature flag (parity stub)
2. **W2**: Protocol coverage (xdg-shell, layer-shell, xdg-decoration, xwayland)
3. **W3**: Scene-graph rendering pipeline
4. **W4**: Input + libinput gestures
5. **W5**: Smooth zoom transitions (uses Phase 6 animation API)
6. **W6**: Live thumbnails in Overview
7. **W7**: Sway backend retirement

W1–W2 establish parity. W3–W6 are the value-adds that justify the port. W7 is cleanup.

### W1. WlrootsBackend skeleton
- New crate: `crates/wlroots/` paralleling `crates/sway/`
- Implements existing `WmBackend` trait (must verify the trait actually exists and is shaped right — memory says it does, but check `crates/sway/src/backend.rs`)
- Dependency: `smithay` crate (not raw wlroots-rs — smithay is the canonical Rust wlroots binding stack)
- Session script: `WM_BACKEND=wlroots|sway` switch in `scripts/start-sway-session` (rename to `start-vibeshell-session`)
- Exit: empty wlroots compositor boots, panel/overlay/launcher start (rendering may be wrong, but processes survive)

### W2. Protocol coverage
- `xdg-shell` (toplevels, popups), `xdg-decoration` (server-side decorations), `wlr-layer-shell` (panel/overlay), `xwayland` (legacy apps)
- Each is a smithay handler trait impl
- Smoke-test: existing `just smoke-test` passes against `WM_BACKEND=wlroots`
- Exit: GTK apps render; panel/overlay layer-surface positioning works; legacy X11 apps via Xwayland

### W3. Scene-graph rendering pipeline
- Smithay's `Space` / scene API
- Replaces Sway-as-backend's "we don't draw, Sway does" model — vibeshell now owns rendering
- Damage tracking, multi-output composition
- Exit: per-frame rendering at 60 FPS; outputs composited correctly across multi-monitor

### W4. Input + libinput gestures
- Pointer, keyboard, touch baseline (smithay handlers)
- Pinch-to-zoom Overview (libinput pinch gesture → `OverviewZoom` IPC)
- Three-finger swipe between clusters (libinput swipe → `CycleCluster` IPC)
- Exit: gestures functional on touchpad; keyboard parity with current Sway behavior

### W5. Smooth zoom transitions
- Uses Phase 6 P6.B animation API
- Mode change Overview ↔ Cluster ↔ Focus interpolates the actual rendered scene, not just viewport metadata
- Exit: zoom transitions are smooth at 60 FPS, no stutter

### W6. Live thumbnails in Overview
- Render each cluster's windows into off-screen textures, downsampled
- Damage-driven update (only re-render when window contents change)
- Replaces the icon+title placeholder in `apps/overlay/src/ui/overview_canvas.rs`
- Exit: Overview shows real window contents in cluster cards

### W7. Retire Sway backend
- After parity verified + soak window (suggest 2 weeks daily-driver use)
- Delete `crates/sway/`, remove `WM_BACKEND` switch, rename session script
- Schedule via `/schedule` for the soak deadline
- Exit: single-backend codebase; no Sway dependency

### Phase 8 risks

- **Smithay API churn**: smithay is pre-1.0; expect breaking changes. Pin a known-good version.
- **Rendering correctness**: damage tracking bugs are subtle. Budget extra time for W3.
- **XWayland edge cases**: legacy apps surface obscure protocol bugs. Smoke-test must include at least one X11 app.
- **Smoke-test rewrite**: current smoke-test boots Sway headless. Need a wlroots headless backend (smithay's `winit` or `udev` backend in test mode). Non-trivial.

---

## Cross-cutting work (folded into the above)

- **CI**: `just smoke-test` must support both backends post-W1; matrix it. Currently smoke-test is gated out of `just ci` because it needs Sway headless — add wlroots headless variant.
- **Docs**: `CLAUDE.md` "Architecture" section needs an update once W1 lands (currently says "Wayland shell environment for Sway") — defer the edit until W1 merges, not before.
- **Memory**: project-overview line in `MEMORY.md` (`Wayland shell environment for Sway`) — same deferral.

---

## Deferred (not scheduled)

### D1. GTK stack upgrade (gtk4 0.8→0.11, libadwaita 0.6→0.9, gtk4-layer-shell 0.3→0.8)
- **Why deferred**: RUSTSEC-2024-0429 audited 2026-04-24 as not-exploitable (zero `VariantStrIter` / `variant_iter` / `VariantDict` call sites). Pure dep hygiene.
- **Cost**: ~4 apps of builder/property API churn.
- **Trigger to revisit**: a real CVE in the pinned versions, OR a feature in 0.11 we want, OR if any of the Phase 8 wlroots/smithay deps require glib 0.20+.

---

## Open questions

1. **P9.2 dep choice**: `zbus` portal vs `pipewire` crate for audio. Resolve in implementation review.
2. **W3 framebuffer strategy**: per-output framebuffers vs single composited surface. Resolve when reading smithay docs.
3. **Smoke-test wlroots headless**: smithay `winit` (visible window, dev-friendly) vs `udev` headless (CI-friendly). Pick during W1.
4. **W7 timing**: how long is the soak window? Suggest 2 weeks of daily-driver after W6 lands.

---

## What this plan does *not* commit to

- Specific dates — sequencing is firm, calendar is not.
- Smithay version pin — pick during W1.
- Whether overlay animations get more polish in Phase 8 beyond P6.B baseline.
- Whether to add a dedicated `WindowRole::Floating` distinct from Dialog (current code conflates; could surface during scratchpad work but out of scope here).
