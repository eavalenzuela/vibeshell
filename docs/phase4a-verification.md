# Phase 4A Verification Checklist (Developer/QA)

This checklist mirrors roadmap Phase 4A criteria **C1–C5** and is intended for consistent manual verification runs.

## Scope

- Feature scope: Overview interaction loop, cluster manipulation, and deterministic zoom/focus transitions.
- Run after any change that affects cluster creation/movement, overview controls, or focus handoff/zoom transitions.

## C1 — Cluster creation + spatial persistence

- [ ] Create two test clusters in Overview.
- [ ] Drag/reposition both clusters to distinct positions.
- [ ] Restart daemon/session.
- [ ] Verify both clusters restore to their committed positions without manual recovery.

## C2 — Discoverable dive path (<= 2 interactions)

For every visible cluster in the current viewport:

- [ ] Validate path A: click to select + Enter to dive.
- [ ] Validate path B: double-click to dive.
- [ ] If no selection exists, verify UI hint is visible and mode does not change unexpectedly.

## C3 — Pan/zoom selection stability

- [ ] Select a cluster.
- [ ] Perform aggressive pan/zoom in Overview (including near output edges).
- [ ] Verify selected cluster remains selected until explicit selection change or mode exit.
- [ ] If selected cluster moves off-screen, verify rediscovery/recenter behavior works without needing reselection.

## C4 — Keyboard-only parity

Without pointer input:

- [ ] Select a cluster using keyboard traversal/selection controls.
- [ ] Enter move mode (`M`), move with Arrow/Shift+Arrow.
- [ ] Validate commit with Enter and cancel with Esc.
- [ ] Dive/activate with Enter from selected cluster.

## C5 — Phase 2/3 regression guardrails (determinism)

- [ ] Execute 20 consecutive Overview <-> Cluster <-> Focus transition cycles.
- [ ] Run cycles across at least three clusters (or fixtures) covering window counts: **1**, **2**, and **3+**.
- [ ] Confirm no window shuffle/flicker regressions and stable deterministic outcomes.

## Notes template

- Build/commit under test:
- Environment (outputs, compositor/session type):
- Observed pass/fail per criterion (C1..C5):
- Repro details for failures:
