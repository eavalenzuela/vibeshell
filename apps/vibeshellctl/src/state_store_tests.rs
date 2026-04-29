use super::{exclusion_reason, Window, WindowRole, WindowState};
use wm::layout::LayoutExclusionReason;

fn scratchpad_window() -> Window {
    Window {
        id: 42,
        role: WindowRole::Scratchpad,
        state: WindowState::MinimizedLike,
        manual_position_override: true,
        ..Default::default()
    }
}

#[test]
fn scratchpad_takes_priority_over_other_exclusions() {
    let mut w = scratchpad_window();
    assert_eq!(
        exclusion_reason(&w),
        Some(LayoutExclusionReason::Scratchpad)
    );

    // Even if the window also looks fullscreen / dialog-ish, scratchpad wins —
    // it's the strongest signal that the window is user-managed and out-of-band.
    w.state = WindowState::Fullscreen;
    w.transient_for = Some(7);
    assert_eq!(
        exclusion_reason(&w),
        Some(LayoutExclusionReason::Scratchpad)
    );
}

#[test]
fn non_scratchpad_paths_unchanged() {
    let fullscreen = Window {
        id: 1,
        state: WindowState::Fullscreen,
        ..Default::default()
    };
    assert_eq!(
        exclusion_reason(&fullscreen),
        Some(LayoutExclusionReason::FullscreenTemporaryOverride)
    );

    let dialog = Window {
        id: 2,
        role: WindowRole::Dialog,
        ..Default::default()
    };
    assert_eq!(
        exclusion_reason(&dialog),
        Some(LayoutExclusionReason::TransientDialogAttached)
    );

    let normal = Window {
        id: 3,
        ..Default::default()
    };
    assert_eq!(exclusion_reason(&normal), None);
}
