//! Per-window position interpolation for the udev backend.
//!
//! Phase 8 W1c-25-4. When the daemon's layout engine flips a cluster from
//! tiled to focus mode (or any other layout change), `apply_layout_ops`
//! used to `map_element` windows at their new positions immediately,
//! producing a hard snap. This module replaces that with a 220ms
//! ease-out-cubic position lerp from the current location to the target.
//!
//! Sizing decisions are *not* animated: the xdg_toplevel `configure(size)`
//! still goes immediately so the client redraws once to its new size. The
//! window then visually slides into place at that new size. Trying to
//! animate the configured size would fight the client's render cadence and
//! produce flicker.
//!
//! Active anims are stored in `Vibewm::window_anims`; the udev render loop
//! calls `tick` once per frame, applies the interpolated positions via
//! `Space::map_element`, and asks for another render-soon if any anim is
//! still running. Cluster-visibility re-maps (`sync_cluster_visibility`)
//! deliberately bypass this — those want an instant pop-in, not a slide.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use wm::layout::WindowId;

/// Single per-window animation. Position-only; size goes via configure.
#[derive(Debug, Clone, Copy)]
pub struct WindowAnim {
    pub from: (i32, i32),
    pub to: (i32, i32),
    pub start: Instant,
    pub duration: Duration,
}

impl WindowAnim {
    /// Interpolated `(x, y)` and a `done` flag. Once `done` is true, the
    /// caller should drop this entry from the map.
    pub fn sample(&self, now: Instant) -> ((i32, i32), bool) {
        let elapsed = now.saturating_duration_since(self.start);
        let t = if self.duration.is_zero() {
            1.0
        } else {
            (elapsed.as_secs_f64() / self.duration.as_secs_f64()).clamp(0.0, 1.0)
        };
        let te = ease_out_cubic(t);
        let x = self.from.0 as f64 + (self.to.0 - self.from.0) as f64 * te;
        let y = self.from.1 as f64 + (self.to.1 - self.from.1) as f64 * te;
        ((x.round() as i32, y.round() as i32), t >= 1.0)
    }
}

fn ease_out_cubic(t: f64) -> f64 {
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}

/// Default duration matching the overlay-side `DIVE_DURATION_MS` so the two
/// halves of a Cluster↔Focus transition feel coherent end-to-end.
pub const DEFAULT_DURATION: Duration = Duration::from_millis(220);

/// Stage a fresh position animation for `window_id`. If one is already
/// running, restart from its current interpolated position so the user
/// doesn't see a backwards jump.
pub fn stage(
    anims: &mut HashMap<WindowId, WindowAnim>,
    window_id: WindowId,
    current_position: (i32, i32),
    target: (i32, i32),
    now: Instant,
    duration: Duration,
) {
    if current_position == target {
        anims.remove(&window_id);
        return;
    }
    let from = if let Some(existing) = anims.get(&window_id) {
        existing.sample(now).0
    } else {
        current_position
    };
    anims.insert(
        window_id,
        WindowAnim {
            from,
            to: target,
            start: now,
            duration,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_duration_completes_immediately() {
        let anim = WindowAnim {
            from: (0, 0),
            to: (100, 100),
            start: Instant::now(),
            duration: Duration::ZERO,
        };
        let (pos, done) = anim.sample(Instant::now());
        assert_eq!(pos, (100, 100));
        assert!(done);
    }

    #[test]
    fn midpoint_eases_past_50_percent() {
        let start = Instant::now();
        let anim = WindowAnim {
            from: (0, 0),
            to: (1000, 0),
            start,
            duration: Duration::from_millis(200),
        };
        let (pos, done) = anim.sample(start + Duration::from_millis(100));
        // ease-out-cubic at t=0.5 → 1 - 0.5^3 = 0.875
        assert!(pos.0 > 800 && pos.0 < 900, "got x={}", pos.0);
        assert!(!done);
    }

    #[test]
    fn end_marks_done() {
        let start = Instant::now();
        let anim = WindowAnim {
            from: (0, 0),
            to: (50, 50),
            start,
            duration: Duration::from_millis(100),
        };
        let (pos, done) = anim.sample(start + Duration::from_millis(150));
        assert_eq!(pos, (50, 50));
        assert!(done);
    }

    #[test]
    fn stage_no_op_when_already_at_target() {
        let mut anims: HashMap<WindowId, WindowAnim> = HashMap::new();
        anims.insert(
            42,
            WindowAnim {
                from: (0, 0),
                to: (10, 10),
                start: Instant::now(),
                duration: Duration::from_millis(200),
            },
        );
        stage(
            &mut anims,
            42,
            (10, 10),
            (10, 10),
            Instant::now(),
            DEFAULT_DURATION,
        );
        assert!(!anims.contains_key(&42), "stage should drop a no-op anim");
    }

    #[test]
    fn stage_restart_picks_up_from_current_interpolated_position() {
        let mut anims: HashMap<WindowId, WindowAnim> = HashMap::new();
        let start = Instant::now();
        anims.insert(
            7,
            WindowAnim {
                from: (0, 0),
                to: (1000, 0),
                start,
                duration: Duration::from_millis(200),
            },
        );
        // 100ms in — sample says ~875.
        let mid = start + Duration::from_millis(100);
        stage(&mut anims, 7, (1000, 0), (2000, 0), mid, DEFAULT_DURATION);
        let restarted = anims.get(&7).unwrap();
        assert!(
            restarted.from.0 > 800 && restarted.from.0 < 900,
            "expected restart from interpolated pos, got from={}",
            restarted.from.0
        );
        assert_eq!(restarted.to, (2000, 0));
    }
}
