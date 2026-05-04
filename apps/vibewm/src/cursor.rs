//! Cursor rendering: xcursor theme images + client-set cursor surfaces.
//!
//! Phase 8 W1c-20/21. Replaces W1c-19's hardcoded black-square cursor with:
//! - Real xcursor theme images (default: system theme, configurable via
//!   `XCURSOR_THEME` / `XCURSOR_SIZE`).
//! - Animated multi-frame cursors (e.g. the wait/hourglass spinner): each
//!   xcursor frame's `delay` advances the active frame; the udev render
//!   loop reschedules itself in time for the next frame.
//! - Client-set cursor surfaces (`wl_pointer.set_cursor` with a wl_surface).
//! - The `Hidden` state when a client requests no cursor.
//!
//! The hardcoded black-square stays as a final fallback when no xcursor theme
//! is available (e.g. minimal containers without `xcursor-themes`).

use std::collections::HashMap;
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Color32F, ImportAll, ImportMem, Renderer};
use smithay::input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Physical, Point, Rectangle, Transform};
use smithay::wayland::compositor::with_states;
use tracing::{debug, warn};

/// One decoded xcursor frame uploaded to a `MemoryRenderBuffer`.
struct CachedFrame {
    buffer: MemoryRenderBuffer,
    /// Hotspot in buffer pixels (i.e. the cursor's "tip" inside the image).
    hotspot: (i32, i32),
    /// How long this frame should be shown before advancing. Zero for static
    /// cursors (single-frame); positive for animated ones (e.g. the wait
    /// spinner). xcursor's units are milliseconds.
    delay: Duration,
}

/// All frames at the chosen size for one cursor icon. A single-frame cursor
/// (the common case) holds one entry; animated cursors hold N.
struct CachedImage {
    frames: Vec<CachedFrame>,
    /// Sum of all frame delays. Zero when the cursor is static.
    total_duration: Duration,
}

/// Per-process xcursor cache. Created once at startup; lookups walk the
/// CursorIcon's name + alt names against the system theme inheritance chain
/// (handled by the `xcursor` crate).
pub struct CursorTheme {
    theme_name: String,
    base_size: u32,
    /// Cache keyed by CursorIcon::name(). `None` means we tried and the theme
    /// has no glyph for this icon — we won't retry every frame.
    cache: HashMap<&'static str, Option<CachedImage>>,
}

impl CursorTheme {
    pub fn new() -> Self {
        let theme_name = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".to_string());
        let base_size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(24);
        Self {
            theme_name,
            base_size,
            cache: HashMap::new(),
        }
    }

    fn load(&mut self, icon: CursorIcon) -> Option<&CachedImage> {
        let key = icon.name();
        if !self.cache.contains_key(key) {
            let loaded = self.try_load(icon);
            if loaded.is_none() {
                debug!(icon = key, theme = %self.theme_name, "cursor: no glyph in theme");
            }
            self.cache.insert(key, loaded);
        }
        self.cache.get(key).and_then(|o| o.as_ref())
    }

    fn try_load(&self, icon: CursorIcon) -> Option<CachedImage> {
        let theme = xcursor::CursorTheme::load(&self.theme_name);
        // Try the canonical name first, then the legacy x11 alt names.
        let candidates = std::iter::once(icon.name()).chain(icon.alt_names().iter().copied());
        for name in candidates {
            let Some(path) = theme.load_icon(name) else {
                continue;
            };
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    warn!(?e, path = %path.display(), "cursor: read failed");
                    continue;
                }
            };
            let images = xcursor::parser::parse_xcursor(&bytes)?;
            if images.is_empty() {
                continue;
            }
            // xcursor groups frames by nominal `size`. Pick the size closest
            // to base_size, then keep every frame at that size — those are
            // the animation's frames (one entry for static cursors).
            let target = self.base_size as i64;
            let best_size = images
                .iter()
                .map(|i| i.size)
                .min_by_key(|s| (*s as i64 - target).abs())?;
            let frames: Vec<CachedFrame> = images
                .into_iter()
                .filter(|i| i.size == best_size)
                .map(|img| {
                    let buffer = MemoryRenderBuffer::from_slice(
                        &img.pixels_argb,
                        Fourcc::Argb8888,
                        (img.width as i32, img.height as i32),
                        1,
                        Transform::Normal,
                        None,
                    );
                    CachedFrame {
                        buffer,
                        hotspot: (img.xhot as i32, img.yhot as i32),
                        delay: Duration::from_millis(img.delay as u64),
                    }
                })
                .collect();
            if frames.is_empty() {
                continue;
            }
            let total_duration: Duration = frames.iter().map(|f| f.delay).sum();
            return Some(CachedImage {
                frames,
                total_duration,
            });
        }
        None
    }
}

impl CachedImage {
    /// For a given elapsed time since vibewm started, return the frame to
    /// show right now plus the duration until the next frame (None for
    /// static cursors — no future render needed).
    fn frame_at(&self, elapsed: Duration) -> (&CachedFrame, Option<Duration>) {
        if self.frames.len() == 1 || self.total_duration.is_zero() {
            return (&self.frames[0], None);
        }
        // Wrap elapsed into one animation cycle, then walk frames until we
        // find the one whose accumulated end-time is past `t`.
        let t = Duration::from_nanos((elapsed.as_nanos() % self.total_duration.as_nanos()) as u64);
        let mut acc = Duration::ZERO;
        for frame in &self.frames {
            let next = acc + frame.delay;
            if t < next {
                return (frame, Some(next - t));
            }
            acc = next;
        }
        // Numerical edge: t == total_duration. Fall through to last frame.
        (
            self.frames.last().expect("non-empty by construction"),
            Some(self.frames[0].delay),
        )
    }
}

impl Default for CursorTheme {
    fn default() -> Self {
        Self::new()
    }
}

/// Render-element variants the cursor pipeline produces. The udev backend
/// wraps these into its `OutputRenderElements` enum.
pub enum CursorElement {
    /// One of zero-or-more wayland surface elements produced by walking a
    /// client-set cursor surface tree.
    Surface(WaylandSurfaceRenderElement<GlesRenderer>),
    /// An xcursor theme image uploaded via `MemoryRenderBuffer`.
    Image(MemoryRenderBufferRenderElement<GlesRenderer>),
    /// Last-resort solid fallback when the theme has no glyph at all.
    Fallback(SolidColorRenderElement),
}

/// Result of one cursor-render pass: the elements to draw plus, for
/// animated cursors, the delay until the next frame should appear (so the
/// caller can schedule another render to advance the animation).
pub struct CursorRender {
    pub elements: Vec<CursorElement>,
    /// `Some(d)` if the caller should schedule another `render_node` in `d`
    /// to advance an animated cursor. `None` for static cursors / hidden /
    /// surface-set cursors (the wayland client drives those via commits).
    pub next_frame_in: Option<Duration>,
}

/// Build the cursor render elements for the current pointer location.
///
/// `scale` is the output's fractional scale (logical→physical); `pointer_loc`
/// is in logical coordinates; `elapsed` is the wall-clock duration since
/// vibewm started (used to phase animated cursors).
pub fn build_cursor_elements(
    theme: &mut CursorTheme,
    renderer: &mut GlesRenderer,
    status: &CursorImageStatus,
    pointer_loc: Point<f64, Logical>,
    scale: f64,
    elapsed: Duration,
) -> CursorRender {
    match status {
        CursorImageStatus::Hidden => CursorRender {
            elements: Vec::new(),
            next_frame_in: None,
        },
        CursorImageStatus::Surface(surface) if surface.is_alive() => {
            let hotspot = with_states(surface, |states| {
                states
                    .data_map
                    .get::<CursorImageSurfaceData>()
                    .map(|m| m.lock().unwrap().hotspot)
                    .unwrap_or_default()
            });
            let pos: Point<i32, Physical> = (
                ((pointer_loc.x - hotspot.x as f64) * scale).round() as i32,
                ((pointer_loc.y - hotspot.y as f64) * scale).round() as i32,
            )
                .into();
            let elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    surface,
                    pos,
                    scale,
                    1.0,
                    Kind::Unspecified,
                );
            CursorRender {
                elements: elems.into_iter().map(CursorElement::Surface).collect(),
                next_frame_in: None,
            }
        }
        CursorImageStatus::Surface(_) => {
            // Surface was destroyed mid-frame; draw the default cursor
            // until the seat handler resets us back to Named.
            named_cursor_elements(
                theme,
                renderer,
                CursorIcon::Default,
                pointer_loc,
                scale,
                elapsed,
            )
        }
        CursorImageStatus::Named(icon) => {
            named_cursor_elements(theme, renderer, *icon, pointer_loc, scale, elapsed)
        }
    }
}

fn named_cursor_elements(
    theme: &mut CursorTheme,
    renderer: &mut GlesRenderer,
    icon: CursorIcon,
    pointer_loc: Point<f64, Logical>,
    scale: f64,
    elapsed: Duration,
) -> CursorRender {
    if let Some(image) = theme.load(icon) {
        let (frame, next_frame_in) = image.frame_at(elapsed);
        let pos: Point<f64, Physical> = (
            (pointer_loc.x - frame.hotspot.0 as f64) * scale,
            (pointer_loc.y - frame.hotspot.1 as f64) * scale,
        )
            .into();
        match MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            pos,
            &frame.buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        ) {
            Ok(elem) => {
                return CursorRender {
                    elements: vec![CursorElement::Image(elem)],
                    next_frame_in,
                };
            }
            Err(e) => warn!(?e, "cursor: MemoryRenderBufferRenderElement upload failed"),
        }
    }
    // Theme gave us nothing (or upload failed). Fall back to W1c-19's
    // black-square placeholder so the pointer is at least visible.
    CursorRender {
        elements: fallback_square(pointer_loc, scale)
            .into_iter()
            .map(CursorElement::Fallback)
            .collect(),
        next_frame_in: None,
    }
}

const FALLBACK_INNER: i32 = 12;
const FALLBACK_BORDER: i32 = 2;

fn fallback_square(pointer_loc: Point<f64, Logical>, scale: f64) -> [SolidColorRenderElement; 2] {
    let lx = pointer_loc.x as i32;
    let ly = pointer_loc.y as i32;
    let outer = Rectangle::<i32, Logical>::new(
        (lx - FALLBACK_BORDER, ly - FALLBACK_BORDER).into(),
        (
            FALLBACK_INNER + FALLBACK_BORDER * 2,
            FALLBACK_INNER + FALLBACK_BORDER * 2,
        )
            .into(),
    );
    let inner =
        Rectangle::<i32, Logical>::new((lx, ly).into(), (FALLBACK_INNER, FALLBACK_INNER).into());
    [
        SolidColorRenderElement::new(
            Id::new(),
            inner.to_physical_precise_round(scale),
            0,
            Color32F::from([0.0, 0.0, 0.0, 1.0]),
            Kind::Unspecified,
        ),
        SolidColorRenderElement::new(
            Id::new(),
            outer.to_physical_precise_round(scale),
            0,
            Color32F::from([1.0, 1.0, 1.0, 1.0]),
            Kind::Unspecified,
        ),
    ]
}

// `Renderer + ImportAll + ImportMem` is what the cursor render path requires;
// asserting it here keeps the type bounds visible alongside the helper above.
#[allow(dead_code)]
fn _renderer_bounds_assert<R: Renderer + ImportAll + ImportMem>() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_image(delays_ms: &[u64]) -> CachedImage {
        let frames: Vec<CachedFrame> = delays_ms
            .iter()
            .map(|d| CachedFrame {
                buffer: MemoryRenderBuffer::from_slice(
                    &[0u8; 4],
                    Fourcc::Argb8888,
                    (1, 1),
                    1,
                    Transform::Normal,
                    None,
                ),
                hotspot: (0, 0),
                delay: Duration::from_millis(*d),
            })
            .collect();
        let total_duration = frames.iter().map(|f| f.delay).sum();
        CachedImage {
            frames,
            total_duration,
        }
    }

    #[test]
    fn static_cursor_returns_no_next_frame() {
        let img = make_image(&[0]);
        let (_, next) = img.frame_at(Duration::from_secs(99));
        assert!(next.is_none());
    }

    #[test]
    fn animated_cursor_phases_correctly() {
        // Three frames @ 100ms each; total = 300ms.
        let img = make_image(&[100, 100, 100]);

        // t=0 → frame 0, 100ms remaining.
        let (_, next) = img.frame_at(Duration::from_millis(0));
        assert_eq!(next, Some(Duration::from_millis(100)));

        // t=150 → frame 1, 50ms remaining.
        let (_, next) = img.frame_at(Duration::from_millis(150));
        assert_eq!(next, Some(Duration::from_millis(50)));

        // t=250 → frame 2, 50ms remaining.
        let (_, next) = img.frame_at(Duration::from_millis(250));
        assert_eq!(next, Some(Duration::from_millis(50)));

        // t=350 wraps to t=50 → frame 0, 50ms remaining.
        let (_, next) = img.frame_at(Duration::from_millis(350));
        assert_eq!(next, Some(Duration::from_millis(50)));
    }
}
