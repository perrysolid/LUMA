//! Preparing screen images for the model. Shared by the app and the eval
//! harness so the eval measures exactly what users get.

use anyhow::Result;
use image::{imageops::FilterType, DynamicImage};
use luma_core::geometry::{view_to_image_px, Capture, Display, Point, SentImage, MODEL_NORM};

pub struct EncodedImage {
    pub sent: SentImage,
    pub jpeg: Vec<u8>,
}

pub struct ModelImages {
    pub images: Vec<EncodedImage>,
    /// Pointer in image-1 model coordinates `(y, x)`.
    pub pointer_norm: Option<(f64, f64)>,
}

/// Full display image (long edge ≤ `max_edge`) plus, when the pointer is on
/// this display, a native-resolution close-up of ~480 points around it.
pub fn prepare(
    full: &DynamicImage,
    display: &Display,
    pointer_view: Option<Point>,
    max_edge: u32,
    closeup: bool,
) -> Result<ModelImages> {
    let capture = Capture { display_index: display.index, width_px: full.width(), height_px: full.height() };
    let mut images = Vec::new();
    let sent_full = SentImage::full(capture, max_edge);
    images.push(EncodedImage { sent: sent_full, jpeg: encode(full, &sent_full)? });

    let pointer_view = pointer_view.filter(|p| display.view_bounds().contains(*p));
    let pointer_norm = pointer_view.and_then(|p| view_to_image_px(p, &sent_full, display)).map(|px| {
        (px.y / sent_full.height_px as f64 * MODEL_NORM, px.x / sent_full.width_px as f64 * MODEL_NORM)
    });

    if closeup {
        if let Some(pv) = pointer_view {
            let (vw, _) = display.view_size();
            let px_per_pt = capture.width_px as f64 / vw;
            let size = (480.0 * px_per_pt).round() as u32;
            let center = Point::new(pv.x * px_per_pt, pv.y * px_per_pt);
            let crop = SentImage::crop_around(capture, center, size, 1024);
            images.push(EncodedImage { sent: crop, jpeg: encode(full, &crop)? });
        }
    }
    Ok(ModelImages { images, pointer_norm })
}

pub fn encode(full: &DynamicImage, s: &SentImage) -> Result<Vec<u8>> {
    let r = s.source_px;
    let cropped = full.crop_imm(r.x as u32, r.y as u32, r.w as u32, r.h as u32);
    let img = if cropped.width() != s.width_px || cropped.height() != s.height_px {
        cropped.resize_exact(s.width_px, s.height_px, FilterType::Triangle)
    } else {
        cropped
    };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&img.to_rgb8())?;
    Ok(out)
}
