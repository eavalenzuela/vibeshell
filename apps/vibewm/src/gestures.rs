//! Touchpad gesture interpretation: pinch + 3-finger swipe → vibeshell IPC.
//!
//! Phase 8 W1c-22. libinput emits gesture events on touchpads that support
//! them (modern laptops). vibewm intercepts these at the compositor level
//! (no forwarding to the focused client today; client gesture forwarding
//! via `pointer-gestures-unstable-v1` is a follow-up) and turns them into
//! daemon mutations:
//!
//! - **3-finger horizontal swipe** → cycle clusters (left = backward,
//!   right = forward; the same MRU cycle that Mod+Tab triggers).
//! - **Pinch gesture** → zoom level shift in the overlay (spread = zoom in
//!   toward focus, pinch = zoom out toward overview).
//!
//! Decisions are made on the End event (or the first qualifying threshold
//! crossing for swipes), so a brief twitch doesn't fire an action. Only one
//! action per gesture: a 3-finger swipe that crosses the threshold and then
//! reverses still resolves to its accumulated direction.
//!
//! All actions dispatch via `Command::new("vibeshellctl")` to mirror how
//! `keybindings.rs` reaches the daemon — same envelope, same audit trail.

use std::process::Command;

use tracing::warn;

/// Minimum cumulative swipe distance (in libinput's logical pixels at the
/// touchpad's resolution) before a 3-finger swipe registers as a cluster
/// cycle. Tuned to match a deliberate finger drag, not a stray brush.
const SWIPE_THRESHOLD_PX: f64 = 60.0;

/// Pinch scale (relative to the begin event) for "spread / zoom in".
const PINCH_SPREAD_THRESHOLD: f64 = 1.20;
/// Pinch scale (relative to the begin event) for "close / zoom out".
const PINCH_CLOSE_THRESHOLD: f64 = 0.83;

#[derive(Default)]
pub struct GestureState {
    swipe: Option<SwipeAccum>,
    pinch: Option<PinchAccum>,
}

struct SwipeAccum {
    fingers: u32,
    dx: f64,
    dy: f64,
}

struct PinchAccum {
    /// Last `scale()` reported by libinput (absolute scale relative to the
    /// pinch's begin event — not a per-update delta).
    scale: f64,
}

/// Resolved gesture action: a single daemon mutation to fire on End.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureAction {
    CycleClusterForward,
    CycleClusterBackward,
    ZoomInMode,
    ZoomOutMode,
}

impl GestureState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_swipe_begin(&mut self, fingers: u32) {
        self.swipe = Some(SwipeAccum {
            fingers,
            dx: 0.0,
            dy: 0.0,
        });
    }

    pub fn on_swipe_update(&mut self, dx: f64, dy: f64) {
        if let Some(s) = self.swipe.as_mut() {
            s.dx += dx;
            s.dy += dy;
        }
    }

    pub fn on_swipe_end(&mut self, cancelled: bool) -> Option<GestureAction> {
        let s = self.swipe.take()?;
        if cancelled {
            return None;
        }
        // 3-finger horizontal swipes only — vertical 3-finger and other
        // finger counts are reserved for follow-ups (workspace switcher,
        // expose mode, etc.).
        if s.fingers != 3 || s.dx.abs() <= s.dy.abs() || s.dx.abs() < SWIPE_THRESHOLD_PX {
            return None;
        }
        Some(if s.dx > 0.0 {
            GestureAction::CycleClusterForward
        } else {
            GestureAction::CycleClusterBackward
        })
    }

    pub fn on_pinch_begin(&mut self) {
        self.pinch = Some(PinchAccum { scale: 1.0 });
    }

    pub fn on_pinch_update(&mut self, scale: f64) {
        if let Some(p) = self.pinch.as_mut() {
            p.scale = scale;
        }
    }

    pub fn on_pinch_end(&mut self, cancelled: bool) -> Option<GestureAction> {
        let p = self.pinch.take()?;
        if cancelled {
            return None;
        }
        if p.scale >= PINCH_SPREAD_THRESHOLD {
            Some(GestureAction::ZoomInMode)
        } else if p.scale <= PINCH_CLOSE_THRESHOLD {
            Some(GestureAction::ZoomOutMode)
        } else {
            None
        }
    }
}

impl GestureAction {
    pub fn dispatch(self) {
        let argv: Vec<&str> = match self {
            GestureAction::CycleClusterForward => {
                vec!["ipc", "cycle-cluster", "--direction", "forward"]
            }
            GestureAction::CycleClusterBackward => {
                vec!["ipc", "cycle-cluster", "--direction", "backward"]
            }
            GestureAction::ZoomInMode => vec!["ipc", "zoom-in-mode"],
            GestureAction::ZoomOutMode => vec!["ipc", "zoom-out-mode"],
        };
        match Command::new("vibeshellctl").args(&argv).spawn() {
            Ok(_) => tracing::info!(?self, "gesture: dispatched"),
            Err(e) => warn!(?e, ?argv, "gesture: vibeshellctl spawn failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_finger_right_swipe_cycles_forward() {
        let mut g = GestureState::new();
        g.on_swipe_begin(3);
        g.on_swipe_update(80.0, 5.0);
        assert_eq!(
            g.on_swipe_end(false),
            Some(GestureAction::CycleClusterForward)
        );
    }

    #[test]
    fn three_finger_left_swipe_cycles_backward() {
        let mut g = GestureState::new();
        g.on_swipe_begin(3);
        g.on_swipe_update(-80.0, 0.0);
        assert_eq!(
            g.on_swipe_end(false),
            Some(GestureAction::CycleClusterBackward)
        );
    }

    #[test]
    fn vertical_dominated_swipe_ignored() {
        let mut g = GestureState::new();
        g.on_swipe_begin(3);
        g.on_swipe_update(40.0, 200.0);
        assert_eq!(g.on_swipe_end(false), None);
    }

    #[test]
    fn under_threshold_swipe_ignored() {
        let mut g = GestureState::new();
        g.on_swipe_begin(3);
        g.on_swipe_update(20.0, 0.0);
        assert_eq!(g.on_swipe_end(false), None);
    }

    #[test]
    fn cancelled_swipe_emits_nothing() {
        let mut g = GestureState::new();
        g.on_swipe_begin(3);
        g.on_swipe_update(200.0, 0.0);
        assert_eq!(g.on_swipe_end(true), None);
    }

    #[test]
    fn four_finger_swipe_currently_unhandled() {
        let mut g = GestureState::new();
        g.on_swipe_begin(4);
        g.on_swipe_update(200.0, 0.0);
        assert_eq!(g.on_swipe_end(false), None);
    }

    #[test]
    fn pinch_spread_zooms_in() {
        let mut g = GestureState::new();
        g.on_pinch_begin();
        g.on_pinch_update(1.5);
        assert_eq!(g.on_pinch_end(false), Some(GestureAction::ZoomInMode));
    }

    #[test]
    fn pinch_close_zooms_out() {
        let mut g = GestureState::new();
        g.on_pinch_begin();
        g.on_pinch_update(0.7);
        assert_eq!(g.on_pinch_end(false), Some(GestureAction::ZoomOutMode));
    }

    #[test]
    fn pinch_within_deadband_ignored() {
        let mut g = GestureState::new();
        g.on_pinch_begin();
        g.on_pinch_update(1.05);
        assert_eq!(g.on_pinch_end(false), None);
    }
}
