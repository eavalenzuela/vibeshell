# vibeshell KVM VM test plan

The Fedora KVM VM is the only place vibewm's `udev` backend actually
runs — the host (Ubuntu/GNOME) lacks `libseat` and can't claim a seat.
Everything that depends on the udev path goes unverified between VM
sessions; this checklist is what to run next time the VM is up.

## Setup

```bash
# In the VM:
cd ~/vibeshell                        # or wherever the repo is
git pull --ff-only
cargo build --workspace --features udev --bin vibewm
just smoke-test-wlroots               # baseline — should be 23/23 green
```

If the workspace build fails: the VM's smithay deps may have drifted;
`cargo update -p smithay --precise 0.7.0` and re-build.

To launch the full session under DRM:

```bash
WM_BACKEND=wlroots VIBEWM_BACKEND=udev ./scripts/start-vibeshell-session
```

`virsh-screenshot vibeshell-vm /tmp/shot.png` from the host captures
evidence; copy out via `virt-copy-out` or shared folder.

## Checklist

Mark each item with one of: ✓ (passed), ✗ (failed, file an issue), ⏸
(blocked, note why). Capture a screenshot for any visual item.

### W1c-20 — xcursor + client-set cursor surfaces

- [ ] **Default cursor renders.** Mouse over the empty desktop —
      should show the system xcursor theme's left_ptr (not the W1c-19
      black-square fallback). Failure mode: black square = theme load
      failed; check logs for `cursor: no glyph in theme`.
- [ ] **Cursor follows pointer.** Move mouse around — cursor tracks
      smoothly, no lag, no doubling. (Tested this in a prior VM run
      but worth re-confirming after the input.rs gesture changes.)
- [ ] **Client-set cursor on hover.** Open `foot` (terminal). Hover
      over the text area — cursor should change to a text-input
      I-beam (foot calls `wl_pointer.set_cursor` with a surface).
      Hover off the terminal back to desktop — cursor reverts to
      left_ptr.
- [ ] **Hidden cursor honored.** Open a fullscreen video player
      (mpv) and press `c` to toggle cursor hide. Cursor disappears.

### W1c-21 — Animated cursor playback

- [ ] **Animated wait cursor.** Trigger a long operation (e.g. `gtk4
      file picker` opening on a slow path, or any GTK app's
      "thinking" state). The wait cursor should animate (rotating
      spinner / hourglass), not freeze on frame 0. Frame cadence
      should match the xcursor file's per-frame delays.
- [ ] **Static cursors don't burn CPU.** Idle the desktop showing
      left_ptr; check `top` for vibewm process — should be near 0%.
      The animation render-kick path only fires for animated cursors.

### W1c-22 — Compositor-side gestures

These require a touchpad. If the VM is running on a host with one
that gets passed through, all of these apply. Otherwise skip the
gesture items and note ⏸.

- [ ] **3-finger right swipe → cycle cluster forward.** Create 2+
      clusters first. 3-finger swipe right on the touchpad; active
      cluster cycles to the next in MRU order. Daemon log shows
      `gesture: dispatched action=CycleClusterForward`.
- [ ] **3-finger left swipe → cycle cluster backward.** Same setup,
      swipe left. Cycles backward.
- [ ] **3-finger vertical swipe ignored.** Vertical 3-finger swipes
      do nothing (reserved for future). Daemon log shows nothing.
- [ ] **Pinch out → zoom in mode.** Pinch fingers apart. Zoom level
      advances Overview→Cluster (or Cluster→Focus). Daemon log:
      `gesture: dispatched action=ZoomInMode`.
- [ ] **Pinch in → zoom out mode.** Pinch fingers together. Zoom
      level retreats. Daemon log: `action=ZoomOutMode`.
- [ ] **Small swipe under threshold ignored.** Twitch the touchpad
      with 3 fingers ~20px. No action fires.

### W1c-24 — Client gesture forwarding

- [ ] **Browser pinch zoom works.** Open Firefox or Chromium. Pinch
      gesture inside the page → page zoom (browser intercepted via
      wp_pointer_gestures_v1). Note that vibewm's compositor binding
      *also* fires (zoom-in-mode); both should activate. Acceptable —
      cluster zoom on desktop, page zoom in browser.
- [ ] **Hold gesture forwarded.** Tap and hold 4 fingers. No
      compositor action fires (we don't bind hold), but if a client
      that handles hold (some touchpad-aware app) responds, the
      protocol round-trip worked.

### W1c-25 — Smooth zoom transitions

- [ ] **Mod++ (Overview→Cluster) animates.** From Overview with a
      cluster selected, press `Mod+=`. Overlay should ease the
      cluster card growing toward fullscreen over ~220ms before
      vibewm's tiled windows appear. Capture a screenshot mid-
      animation if possible (slow motion: comment out the `on_dive`
      call temporarily).
- [ ] **Mod+- (Cluster→Overview) animates.** From Cluster mode,
      press `Mod+-`. Overlay should appear with the viewport
      pre-seeded at the cluster's dived pose, then ease back to
      Overview. The W1c-25-7 daemon Subscribe channel means this
      should fire within socket-RTT, not 1.2s later.
- [ ] **Cluster→Focus window slides smoothly.** From Cluster mode,
      press `Mod+=` again to enter Focus on the focused window. The
      other windows should *slide* into the context strip on the
      side, not snap. (W1c-25-4 + the try_build_frame focus-context
      bug fix together.)
- [ ] **Focus→Cluster animates back.** `Mod+-` from Focus. Strip
      windows slide back to tile.
- [ ] **`Mod+.` / `Mod+,` cycles strip with animation.** In Focus
      mode with 3+ windows, cycle the strip — the new dominant
      window grows from the strip while the previous one shrinks.

### W1c-25-5b — Real thumbnail capture

- [ ] **Active cluster card shows real screen content.** From Cluster
      mode, zoom out to Overview. The active cluster's card should
      show a downscaled snapshot of the actual windows you just had
      tiled, not a procedural HSV gradient. Capture screenshot.
- [ ] **Inactive cluster cards show placeholder.** Other cluster
      cards (not currently active) should show the W1c-25-5a
      gradient with a strip of rectangles for the windows. Different
      colors per cluster id.
- [ ] **Thumbnail refreshes on cluster activation.** Switch to a
      different cluster (Mod+Tab). Switch back. The previously-
      active cluster's card should now show *its* real snapshot
      (because it was the active cluster on its way out). The newly-
      active cluster's card refreshes too.
- [ ] **Capture failure falls through gracefully.** If the GLES path
      errors (unlikely but possible on virtio-gpu), the daemon log
      shows `daemon: thumbnail capture failed; cache unchanged` and
      the card falls back to the placeholder rather than going blank.

### GTK theming (W1c-25 session)

- [ ] **Launcher matches palette.** `Mod+Space`. Launcher panel:
      dark slate background `#0d0d12`, rounded 16px corners, drop
      shadow visible against the desktop, teal accent `#4dd2c8` on
      the focused search ring + selection highlight, uppercase
      teal section header "APPS". Compare to STYLE.md.
- [ ] **Cheatsheet matches palette.** `Mod+/`. Same dark panel,
      rounded corners, teal monospace key glyphs, accent uppercase
      section headers per category.
- [ ] **Panel matches palette.** Panel at top: dark `#0d0d12`
      background, hairline border below, dimmed status text on the
      right (network/battery/clock). Workspace buttons: rounded
      8px, hover lifts to lighter bg, active workspace shows the
      teal `accent_soft` background.
- [ ] **Notifd matches palette.** Trigger a notification (`notify-
      send "test" "body"`). Toast appears with the dark vibeshell
      palette, not host Adwaita defaults.
- [ ] **User override loads.** Drop `~/.config/vibeshell/theme.css`
      with `box.vibeshell-launcher-panel { background: #001122; }`,
      restart launcher. Background should be navy. Removes override
      cleanly when file is deleted + relaunch.

### Regression checks

- [ ] **`just smoke-test-wlroots` 23/23.** Same as host; the udev
      path doesn't change daemon-side logic.
- [ ] **`cargo test --workspace --features udev`.** New udev-side
      tests (if any) pass. At minimum, no compile errors.
- [ ] **No new warnings under udev clippy.** `cargo clippy
      --workspace --all-targets --features udev -- -D warnings`.
- [ ] **Session boots clean to login screen.** No errors in the
      daemon log on boot. `journalctl --user -u vibeshell` (if
      systemd-managed) or the captured session log otherwise.
- [ ] **Hot-loop CPU sane.** Idle desktop, vibewm process: <2% CPU
      sustained. Check with `top` over a 30s window.
- [ ] **Daemon survives 10+ cluster activations.** Mod+Tab between
      clusters 10+ times. No daemon crashes, no memory growth >10MB
      over the session, no thumbnail cache leaks (BTreeMap should
      stay bounded by cluster count).

## After the run

1. Tally pass/fail counts. Update ROADMAP.md's W1c-25 entries with
   ✓ verified-on-VM markers if everything passes.
2. If anything failed, file as a separate W1c-25-fixupN item in
   ROADMAP.md with the symptom + the line in the daemon log that
   pointed at it.
3. If the smooth-zoom story passes end-to-end, the soak phase begins.
   Use the session as your daily driver for ~a week. Note any
   unexpected behavior in `docs/soak-notes.md` (create if missing).
4. Once soak passes, the last unchecked Phase 8 item — "Retire Sway
   backend" — becomes actionable. Plan that as a separate cleanup
   PR (delete `crates/sway`, drop `WM_BACKEND` env handling, drop
   `scripts/start-sway-session`).

## What's NOT in this plan

- **wlr-screencopy-v1**: still deferred. The W1c-25-5b custom-IPC
  capture covers the same use case for our own overlay. External
  screencap tools (grim, wf-recorder) still won't work against
  vibewm until a real screencopy-v1 implementation lands. Plan that
  separately when needed.
- **Multi-monitor**: vibewm currently captures the *first* output
  for thumbnails. Multi-output VMs won't show this gracefully —
  test only on single-output for now.
- **HDR / 10-bit color**: `Fourcc::Argb8888` everywhere. Anything
  HDR-related is unsupported; don't attempt.
