#!/usr/bin/env bash
set -euo pipefail

cat <<'CHECKLIST'
Overlay interaction checklist
=============================

1) Create cluster
   cargo run -p vibeshellctl -- ipc get-state --pretty
   # In your shell workflow create a new cluster, then rerun get-state and verify cluster appears.

2) Assign window
   # Move a window into that cluster using your existing workflow.
   cargo run -p vibeshellctl -- ipc get-state --pretty
   # Verify cluster.window list contains the moved window id/title/app_id.

3) Click activate
   cargo run -p overlay
   # Click the Activate button on a non-focused cluster card.
   # Verify compositor switches to that cluster/workspace.

4) Rename cluster reflected in overlay
   # Rename cluster via your existing command/workflow.
   cargo run -p vibeshellctl -- ipc get-state --pretty
   # Verify overlay card title updates after debounce/periodic refresh.

Notes:
- Overlay refreshes from periodic polling and debounced sway event triggers.
- Closed/missing windows are rendered as "closed window (<id>)" and do not crash rendering.
CHECKLIST
