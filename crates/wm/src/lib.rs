//! Window manager backend abstraction.
//!
//! `WmBackend` is the trait every concrete WM (Sway, wlroots, …) implements.
//! `WmFacts` is the snapshot the daemon ingests every tick. Layout/frame logic
//! that doesn't depend on a particular WM lives in `layout`.

pub mod backend;
pub mod facts;
pub mod layout;
pub mod vibewm_ipc;
pub mod wlroots_backend;

pub use backend::{BackendError, WmBackend, WmSignal};
pub use facts::{ClusterFact, OutputFact, WindowFact, WmFacts};
pub use layout::{
    diff_targets, BackendEvent, ClusterLayoutInput, DiffThresholds, FramePipeline, FrameResult,
    LayoutComputeContext, LayoutEngine, LayoutExclusionReason, LayoutMode, LayoutOp, Rect,
    WorkspaceMetadata, WorkspaceTransitionController,
};
pub use vibewm_ipc::{vibewm_socket_path, VibewmEvent, VibewmRequest, VibewmResponse};
pub use wlroots_backend::WlrootsBackend;
