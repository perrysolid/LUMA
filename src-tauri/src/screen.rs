//! Displays, pointer, active window and screen capture.
//!
//! This is the only place that knows how each OS reports coordinates; it
//! converts everything into `luma_core::geometry` types.

use anyhow::{anyhow, Context, Result};
use image::{imageops::FilterType, DynamicImage, RgbaImage};
use luma_core::geometry::{display_at, view_to_image_px, Capture, Display, Point, Rect, SentImage, MODEL_NORM};
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

pub struct EncodedImage {
    pub sent: SentImage,
    pub jpeg: Vec<u8>,
}

/// Everything captured at the moment the user invoked LUMA.
pub struct Snapshot {
    pub displays: Vec<Display>,
    pub display: Display,
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
    let raw: RgbaImage = monitor
        .capture_image()
        .map_err(|e| anyhow!("screen capture failed (is Screen Recording permission granted?): {e}"))?;
    let capture = Capture { display_index: display.index, width_px: raw.width(), height_px: raw.height() };
    let full = DynamicImage::ImageRgba8(raw);

    let mut images = Vec::new();
    let sent_full = SentImage::full(capture, opts.max_edge);
    images.push(EncodedImage { sent: sent_full, jpeg: encode(&full, &sent_full)? });

    let pointer_view = pointer.map(|p| display.input_to_view(p)).filter(|p| display.view_bounds().contains(*p));
    let pointer_norm = pointer_view.and_then(|p| view_to_image_px(p, &sent_full, &display)).map(|px| {
        (
            px.y / sent_full.height_px as f64 * MODEL_NORM,
            px.x / sent_full.width_px as f64 * MODEL_NORM,
        )
    });

    if opts.closeup {
        if let Some(pv) = pointer_view {
            let (vw, _) = display.view_size();
            let px_per_pt = capture.width_px as f64 / vw;
            // ~480 points of context around the pointer, at native resolution
            let size = (480.0 * px_per_pt).round() as u32;
            let center = Point::new(pv.x * px_per_pt, pv.y * px_per_pt);
            let crop = SentImage::crop_around(capture, center, size, 1024);
            images.push(EncodedImage { sent: crop, jpeg: encode(&full, &crop)? });
        }
    }

    Ok(Snapshot { displays, display, images, window: active_window().unwrap_or_default(), pointer_norm })
}

fn encode(full: &DynamicImage, s: &SentImage) -> Result<Vec<u8>> {
    let r = s.source_px;
    let cropped = full.crop_imm(r.x as u32, r.y as u32, r.w as u32, r.h as u32);
    let img = if cropped.width() != s.width_px || cropped.height() != s.height_px {
        cropped.resize_exact(s.width_px, s.height_px, FilterType::Triangle)
    } else {
        cropped
    };
    let mut out = Vec::new();
    let rgb = img.to_rgb8();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&rgb)?;
    Ok(out)
}
