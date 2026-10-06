//! Displays, pointer, active window and screen capture.
//!
//! This is the only place that knows how each OS reports coordinates; it
//! converts everything into `luma_core::geometry` types.

use anyhow::{anyhow, Context, Result};
use image::DynamicImage;
use luma_core::geometry::{display_at, Display, Point, Rect};
pub use luma_net::vision::EncodedImage;
use tauri::{AppHandle, Runtime};

/// Enumerate displays in OS input space.
pub fn displays() -> Result<Vec<Display>> {
    let monitors = xcap::Monitor::all().map_err(|e| anyhow!("listing displays: {e}"))?;
    let mut out = Vec::with_capacity(monitors.len());
    for (index, m) in monitors.iter().enumerate() {
        let scale = m.scale_factor().unwrap_or(1.0) as f64;
        out.push(Display {
            index,
            name: m.friendly_name().or_else(|_| m.name()).unwrap_or_else(|_| format!("Display {}", index + 1)),
            // xcap reports CGDisplayBounds (points) on macOS and monitor rects
            // in physical pixels on per-monitor-DPI-aware Windows processes:
            // exactly our input space on each OS.
            input_frame: Rect::new(
                m.x().unwrap_or(0) as f64,
                m.y().unwrap_or(0) as f64,
                m.width().unwrap_or(1) as f64,
                m.height().unwrap_or(1) as f64,
            ),
            input_per_point: if cfg!(target_os = "macos") { 1.0 } else { scale },
            scale_factor: scale,
            is_primary: m.is_primary().unwrap_or(index == 0),
        });
    }
    if out.is_empty() {
        return Err(anyhow!("no displays found"));
    }
    Ok(out)
}

/// Pointer position in OS input space.
pub fn pointer<R: Runtime>(app: &AppHandle<R>, displays: &[Display]) -> Option<Point> {
    let p = app.cursor_position().ok()?;
    if cfg!(target_os = "macos") {
        // tao converts NSEvent.mouseLocation (global points) to "physical"
        // using the *primary* display's scale; undo exactly that.
        let primary = displays.iter().find(|d| d.is_primary).or(displays.first())?;
        Some(Point::new(p.x / primary.scale_factor, p.y / primary.scale_factor))
    } else {
        Some(Point::new(p.x, p.y))
    }
}

#[derive(Debug, Clone, Default)]
pub struct ActiveWindow {
    pub app: String,
    pub title: String,
}

pub fn active_window() -> Option<ActiveWindow> {
    let w = active_win_pos_rs::get_active_window().ok()?;
    Some(ActiveWindow { app: w.app_name, title: w.title })
}

/// Everything captured at the moment the user invoked LUMA.
pub struct Snapshot {
    pub displays: Vec<Display>,
    pub images: Vec<EncodedImage>,
    pub window: ActiveWindow,
    /// Pointer in image-1 model coordinates `(y, x)`.
    pub pointer_norm: Option<(f64, f64)>,
}

impl Snapshot {
    pub fn context_key(&self) -> String {
        format!("{} — {}", self.window.app, self.window.title)
    }
}

pub struct CaptureOptions {
    pub max_edge: u32,
    pub closeup: bool,
}

/// Capture the display under the pointer plus an optional close-up.
pub fn snapshot(displays: Vec<Display>, pointer: Option<Point>, opts: &CaptureOptions) -> Result<Snapshot> {
    let display = pointer
        .and_then(|p| display_at(&displays, p))
        .or_else(|| displays.iter().find(|d| d.is_primary))
        .unwrap_or(&displays[0])
        .clone();
    let monitors = xcap::Monitor::all().map_err(|e| anyhow!("listing displays: {e}"))?;
    let monitor = monitors.get(display.index).context("display disappeared")?;
    let raw = monitor
        .capture_image()
        .map_err(|e| anyhow!("screen capture failed (is Screen Recording permission granted?): {e}"))?;
    let full = DynamicImage::ImageRgba8(raw);
    let prepared = luma_net::vision::prepare(
        &full,
        &display,
        pointer.map(|p| display.input_to_view(p)),
        opts.max_edge,
        opts.closeup,
    )?;
    Ok(Snapshot {
        displays,
        images: prepared.images,
        window: active_window().unwrap_or_default(),
        pointer_norm: prepared.pointer_norm,
    })
}
