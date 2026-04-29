//! DRM/KMS backend (real-hardware compositor mode).
//!
//! Phase 8 W1c-DRM. Replaces the winit backend's in-a-window dev path with
//! a session-compositor path: vibewm claims `seat0` via libseat/logind,
//! discovers DRM devices via udev, opens the first connected device,
//! enumerates connected connectors, picks the first one's preferred mode,
//! and renders into it via smithay's `DrmCompositor` driving a GBM-backed
//! GLES surface.
//!
//! Selected at runtime via `VIBEWM_BACKEND=udev` (default `winit`). Compiled
//! out unless the `udev` Cargo feature is enabled.
//!
//! Minimum-viable scope: single GPU, single output, no hot-plug, no DRM
//! lease, no dmabuf protocol, no multi-GPU. We pick the lowest-common-denom
//! pixel format (Argb8888) and let the renderer handle scaling. Cursor is
//! not drawn yet (pointer events still fire; visual cursor is W1c-DRM-2).

use std::collections::HashMap;
use std::time::Duration;

use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::compositor::DrmCompositor;
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmNode};
use smithay::backend::egl::context::ContextPriority;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{UdevBackend, UdevEvent};
use smithay::output::{Mode as WlMode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::EventLoop;
// `control::Device` is the trait that exposes `get_connector`, `get_encoder`,
// `resource_handles` etc. on a DrmDevice. The top-level `drm::Device` is only
// for raw fd accessors and isn't what we want here.
use smithay::reexports::drm::control::Device as _;
use smithay::reexports::drm::control::{connector, crtc, ModeTypeFlags};
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::{DeviceFd, Transform};
use tracing::{info, warn};

use crate::state::Vibewm;

/// Color format we negotiate with the DRM driver. Argb8888 is the lowest
/// common denominator and works on virtio-gpu, Intel, AMD, NVIDIA. Could be
/// upgraded to 10-bit on capable hardware in a follow-up.
const COLOR_FORMAT: Fourcc = Fourcc::Argb8888;

/// Per-DRM-device state. Today vibewm tracks at most one of these (single
/// GPU); refactored to a HashMap so multi-GPU is a follow-up addition,
/// not a rewrite.
struct DeviceData {
    /// Render path for this device. None until we've successfully created
    /// the DrmCompositor (i.e. EGL/GBM init succeeded).
    drm_compositor: Option<
        DrmCompositor<
            GbmAllocator<DrmDeviceFd>,
            GbmFramebufferExporter<DrmDeviceFd>,
            (),
            DrmDeviceFd,
        >,
    >,
    /// The wayland Output that wraps this connector.
    output: Output,
    /// Render loop driver: GLES context against the GBM surface.
    renderer: GlesRenderer,
}

/// Top-level DRM backend state. Sits inside `Vibewm` (or alongside, mutably
/// borrowed) for the lifetime of the udev session.
pub struct UdevState {
    pub session: LibSeatSession,
    devices: HashMap<DrmNode, DeviceData>,
}

/// Entry point: wires session, udev, libinput, and the first available DRM
/// device into vibewm's calloop. Doesn't return until the compositor exits;
/// shares the same `EventLoop<Vibewm>` as the wayland-server / IPC sources
/// already registered by `Vibewm::new`.
///
/// Errors here are fatal — there's no winit fallback once we've opted into
/// `VIBEWM_BACKEND=udev`. The caller (main) should let them propagate.
pub fn run_udev(
    event_loop: &mut EventLoop<'static, Vibewm>,
    state: &mut Vibewm,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("vibewm: starting DRM backend");

    // 1. Claim the seat. logind hands us the active session via libseat-rs;
    //    requires us to be on a TTY logged in (Active=yes Type=tty).
    let (session, session_notifier) = LibSeatSession::new()?;
    info!(seat = %session.seat(), "vibewm: claimed seat");

    // 2. Stand up libinput against the same session. The
    //    `LibinputSessionInterface` is what lets libinput open device files
    //    via the seat without us needing root.
    let mut libinput_context =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput_context
        .udev_assign_seat(&session.seat())
        .map_err(|_| "libinput: failed to assign seat — is logind active for this session?")?;
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    // 3. Discover DRM devices via udev.
    let udev_backend = UdevBackend::new(session.seat())?;

    // Move state into the right shape: `Vibewm` doesn't currently hold
    // udev_state, so we attach it here on the heap. Future cleanup will
    // make this a real field once we wire multi-output / hot-plug.
    state.udev = Some(UdevState {
        session,
        devices: HashMap::new(),
    });

    // 4. Initialize whichever DRM device udev tells us about first. This
    //    is the *boot-time* enumeration; hot-plug (UdevEvent::Added at
    //    runtime) is W1c-DRM-2.
    let initial_devices: Vec<_> = udev_backend.device_list().collect();
    if initial_devices.is_empty() {
        return Err("udev: no DRM devices found — is virtio-gpu / amdgpu / i915 loaded?".into());
    }
    for (device_id, path) in initial_devices {
        // `device_list()` yields `&Path` — copy to PathBuf so the device
        // entry can outlive the udev iterator borrow.
        if let Err(e) = open_drm_device(state, device_id, path.to_path_buf()) {
            warn!(?e, %device_id, "udev: failed to bring up DRM device; trying next");
        }
    }
    if state
        .udev
        .as_ref()
        .map(|u| u.devices.is_empty())
        .unwrap_or(true)
    {
        return Err("udev: no DRM device successfully initialized".into());
    }

    // 5. Insert calloop sources for udev hot-plug, libinput events, and
    //    session pause/resume.
    event_loop
        .handle()
        .insert_source(udev_backend, move |event, _, _state| match event {
            UdevEvent::Added { device_id, path } => {
                info!(
                    ?device_id,
                    ?path,
                    "udev: device added (hot-plug not handled yet)"
                );
            }
            UdevEvent::Changed { device_id } => {
                info!(?device_id, "udev: device changed (ignored for now)");
            }
            UdevEvent::Removed { device_id } => {
                info!(?device_id, "udev: device removed (ignored for now)");
            }
        })?;

    event_loop
        .handle()
        .insert_source(libinput_backend, |event, _, state| {
            // libinput events route into the same generic dispatcher the winit
            // backend uses — `process_input_event` is generic over `InputBackend`.
            state.process_input_event(event);
        })?;

    event_loop
        .handle()
        .insert_source(session_notifier, move |event, _, state| match event {
            SessionEvent::PauseSession => {
                info!("vibewm: session paused (TTY switched away?)");
                if let Some(udev) = state.udev.as_mut() {
                    for device in udev.devices.values_mut() {
                        if let Some(comp) = device.drm_compositor.as_mut() {
                            let _ = comp.reset_state();
                        }
                    }
                }
            }
            SessionEvent::ActivateSession => {
                info!("vibewm: session activated");
                // Nudge a frame on each device so the screen lights back up.
                if let Some(udev) = state.udev.as_mut() {
                    for device in udev.devices.values_mut() {
                        if let Some(comp) = device.drm_compositor.as_mut() {
                            let _ = comp.queue_frame(());
                        }
                    }
                }
            }
        })?;

    info!(
        devices = state.udev.as_ref().map(|u| u.devices.len()).unwrap_or(0),
        "vibewm: DRM backend ready",
    );

    Ok(())
}

/// Open a single DRM device, set up GBM/EGL/GLES, find the first connected
/// connector, create a `DrmCompositor` for it, and stash everything in
/// `state.udev.devices`.
fn open_drm_device(
    state: &mut Vibewm,
    device_id: u64,
    path: std::path::PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(%device_id, ?path, "udev: opening DRM device");

    // Open via the session so logind hands us a privileged FD without us
    // being root. `Session::open` returns an `OwnedFd` directly in smithay
    // 0.7 — no need for the FromRawFd dance.
    let udev = state.udev.as_mut().ok_or("udev state missing")?;
    let owned_fd = udev.session.open(
        &path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
    )?;
    let drm_fd = DrmDeviceFd::new(DeviceFd::from(owned_fd));
    let drm_node = DrmNode::from_path(&path)?;

    // Atomic modesetting + connector restore. The notifier is the source we
    // insert into calloop to drive the per-frame VBlank loop.
    let (mut drm_device, drm_notifier) = DrmDevice::new(drm_fd.clone(), true)?;

    // GBM allocator + framebuffer exporter on top of the same FD.
    let gbm = GbmDevice::new(drm_fd.clone())?;
    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(gbm.clone(), drm_node.into());

    // EGL display + GLES renderer.
    let egl_display = egl_from_gbm(&gbm)?;
    let egl_context =
        EGLContext::new_with_priority(&egl_display, ContextPriority::High).or_else(|_| {
            // Some drivers (incl. virtio-gpu) reject non-Medium priority.
            EGLContext::new(&egl_display)
        })?;
    let renderer_formats: FormatSet = egl_context.dmabuf_render_formats().clone();
    // SAFETY: GlesRenderer::new requires a current context; smithay handles
    // make-current internally on first use.
    #[allow(unsafe_code)]
    let renderer = unsafe { GlesRenderer::new(egl_context)? };

    // Find first connected connector with a preferred mode.
    let res = drm_device
        .resource_handles()
        .map_err(|e| format!("DRM resource_handles: {e}"))?;
    let mut chosen: Option<(
        connector::Info,
        crtc::Handle,
        smithay::reexports::drm::control::Mode,
    )> = None;
    for &handle in res.connectors() {
        let info = drm_device
            .get_connector(handle, false)
            .map_err(|e| format!("get_connector: {e}"))?;
        if info.state() != connector::State::Connected {
            continue;
        }
        let preferred_mode = info
            .modes()
            .iter()
            .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| info.modes().first())
            .copied();
        let Some(mode) = preferred_mode else {
            continue;
        };
        // Pick a CRTC compatible with this connector's encoders.
        let mut chosen_crtc: Option<crtc::Handle> = None;
        for &enc_handle in info.encoders() {
            let enc = drm_device
                .get_encoder(enc_handle)
                .map_err(|e| format!("get_encoder: {e}"))?;
            for crtc in res.filter_crtcs(enc.possible_crtcs()) {
                chosen_crtc = Some(crtc);
                break;
            }
            if chosen_crtc.is_some() {
                break;
            }
        }
        let Some(crtc) = chosen_crtc else {
            continue;
        };
        chosen = Some((info, crtc, mode));
        break;
    }
    let (connector_info, crtc, drm_mode) =
        chosen.ok_or("udev: no connected connector with a usable mode")?;

    info!(
        connector = %format!("{:?}-{}", connector_info.interface(), connector_info.interface_id()),
        mode = %format!("{}x{}@{}", drm_mode.size().0, drm_mode.size().1, drm_mode.vrefresh()),
        "udev: selected output"
    );

    // Create the DRM surface for this CRTC + connector.
    let drm_surface = drm_device
        .create_surface(crtc, drm_mode, &[connector_info.handle()])
        .map_err(|e| format!("create_surface: {e}"))?;

    // Build the `Output` and seed it with the picked mode.
    let mode_size = drm_mode.size();
    let wl_mode = WlMode {
        size: (mode_size.0 as i32, mode_size.1 as i32).into(),
        refresh: drm_mode.vrefresh() as i32 * 1000,
    };
    let phys = PhysicalProperties {
        size: (
            connector_info.size().map(|s| s.0 as i32).unwrap_or(0),
            connector_info.size().map(|s| s.1 as i32).unwrap_or(0),
        )
            .into(),
        subpixel: Subpixel::Unknown,
        make: "vibeshell".into(),
        model: format!(
            "{:?}-{}",
            connector_info.interface(),
            connector_info.interface_id()
        ),
    };
    let output = Output::new(phys.model.clone(), phys);
    let _global = output.create_global::<Vibewm>(&state.display_handle);
    output.change_current_state(
        Some(wl_mode),
        Some(Transform::Normal),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(wl_mode);
    state.space.map_output(&output, (0, 0));

    // Stand up the DrmCompositor. It owns the surface + GBM-backed
    // framebuffer + render-element pipeline; we feed it elements per VBlank.
    let drm_compositor = DrmCompositor::new(
        // OutputModeSource is the public re-export under smithay::output;
        // backend::drm::compositor:: is the private internal path.
        smithay::output::OutputModeSource::Static {
            size: (mode_size.0 as i32, mode_size.1 as i32).into(),
            scale: smithay::utils::Scale::from(1.0),
            transform: Transform::Normal,
        },
        drm_surface,
        None, // planes — let smithay pick (primary + cursor)
        allocator,
        exporter,
        [COLOR_FORMAT],
        renderer_formats.iter().copied(),
        smithay::utils::Size::from((64, 64)), // cursor plane size hint
        Some(gbm),
    )
    .map_err(|e| format!("DrmCompositor::new: {e}"))?;

    // Register the DRM event source — fires VBlanks that we use to drive
    // rendering.
    use smithay::reexports::calloop::LoopHandle;
    let loop_handle: LoopHandle<'static, Vibewm> = state.loop_handle.clone();
    let drm_node_for_handler = drm_node;
    loop_handle.insert_source(drm_notifier, move |event, _, state| match event {
        DrmEvent::VBlank(_crtc) => {
            render_node(state, drm_node_for_handler);
        }
        DrmEvent::Error(e) => warn!(?e, "udev: DRM error event"),
    })?;

    // Stash everything.
    state
        .udev
        .as_mut()
        .ok_or("udev state missing")?
        .devices
        .insert(
            drm_node,
            DeviceData {
                drm_compositor: Some(drm_compositor),
                output: output.clone(),
                renderer,
            },
        );

    // Kick off the first frame so we see something on screen.
    schedule_initial_frame(state, drm_node);

    Ok(())
}

/// `EGLDisplay::new` is `unsafe` in smithay 0.7 because it doesn't validate
/// the GBM device pointer at runtime — caller promises it's a live native
/// display. Wrapping the unsafe block in this helper keeps it localized.
fn egl_from_gbm(
    gbm: &GbmDevice<DrmDeviceFd>,
) -> Result<EGLDisplay, Box<dyn std::error::Error>> {
    // SAFETY: `gbm` is a live GbmDevice we just created from an open DRM fd
    // owned by the session; it's valid for the lifetime of the resulting
    // EGLDisplay (which we own and don't drop until session teardown).
    #[allow(unsafe_code)]
    let display = unsafe { EGLDisplay::new(gbm.clone())? };
    Ok(display)
}

/// Render one frame for `drm_node` and queue it on the DRM compositor.
/// Called from the VBlank handler — when this returns, the compositor will
/// kick the buffer out and another VBlank will fire when it's done.
fn render_node(state: &mut Vibewm, drm_node: DrmNode) {
    let Some(udev) = state.udev.as_mut() else {
        return;
    };
    let Some(device) = udev.devices.get_mut(&drm_node) else {
        return;
    };
    let Some(comp) = device.drm_compositor.as_mut() else {
        return;
    };

    // Acknowledge the previous frame submission so smithay can recycle the
    // buffer.
    if let Err(e) = comp.frame_submitted() {
        warn!(?e, "udev: frame_submitted failed");
    }

    // Build render elements from the smithay Space. Three args: renderer,
    // output, alpha (1.0 = fully opaque).
    let elements = state
        .space
        .render_elements_for_output(&mut device.renderer, &device.output, 1.0)
        .unwrap_or_default();

    // Color32F::from([f32; 4]) is the standard way to convert RGBA literals.
    use smithay::backend::renderer::Color32F;
    let res = comp.render_frame::<_, _>(
        &mut device.renderer,
        &elements,
        Color32F::from([0.05, 0.05, 0.07, 1.0]),
        smithay::backend::drm::compositor::FrameFlags::DEFAULT,
    );
    match res {
        Ok(_render_output) => {
            if let Err(e) = comp.queue_frame(()) {
                warn!(?e, "udev: queue_frame failed");
            }
        }
        Err(e) => warn!(?e, "udev: render_frame failed"),
    }

    // Tell every mapped window we've shown a frame, so wayland clients
    // know to release their buffers and start the next one.
    let now = state.start_time.elapsed();
    state.space.elements().for_each(|w| {
        w.send_frame(&device.output, now, Some(Duration::ZERO), |_, _| {
            Some(device.output.clone())
        });
    });
}

/// Schedule the first frame after device init. We can't render directly from
/// `open_drm_device` because the DRM compositor isn't ready until after its
/// first commit; queue a calloop timer to kick it on the next loop tick.
fn schedule_initial_frame(state: &mut Vibewm, drm_node: DrmNode) {
    let timer = Timer::from_duration(Duration::from_millis(16));
    let _ = state.loop_handle.insert_source(timer, move |_, _, state| {
        render_node(state, drm_node);
        TimeoutAction::Drop
    });
}
