//! Interactive move + resize grabs for xdg-toplevels.
//!
//! Adapted from smithay's `smallvil` example with `Smallvil` → `Vibewm`. The
//! compositor installs one of these `PointerGrab` impls when a client issues
//! an xdg_toplevel `move_request` / `resize_request`. While the grab is held,
//! all pointer events route through the grab and update the target window's
//! geometry.
//!
//! These run entirely inside vibewm — the daemon learns about the resulting
//! geometry on its next snapshot poll, and `state_store::ingest_facts` flags
//! windows whose geometry diverges from the layout-engine target as
//! `LayoutExclusionReason::ManualResize` so layout doesn't reflow them.

pub mod move_grab;
pub mod resize_grab;

pub use move_grab::MoveSurfaceGrab;
pub use resize_grab::{handle_commit as handle_resize_commit, ResizeEdge, ResizeSurfaceGrab};
