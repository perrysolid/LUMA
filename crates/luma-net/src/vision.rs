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
    /// Pointer in image-2 (close-up) model coordinates `(y, x)`.
    pub pointer_norm_closeup: Option<(f64, f64)>,
}

/// Screenshots never include the mouse cursor, and coordinates alone are a
/// weak cue for "this". A hollow ring (white halo + magenta stroke) marks the
/// pointer without covering what is underneath it.
pub const POINTER_RING_RADIUS_PT: f64 = 15.0;

pub fn draw_pointer_ring(img: &mut image::RgbaImage, center: Point, px_per_pt: f64) {
    let r = POINTER_RING_RADIUS_PT * px_per_pt;
    let stroke = 2.5 * px_per_pt;
    let halo = 1.5 * px_per_pt;
    let outer = r + stroke / 2.0 + halo;
    let x0 = (center.x - outer).floor().max(0.0) as u32;
    let y0 = (center.y - outer).floor().max(0.0) as u32;
    let x1 = ((center.x + outer).ceil() as u32).min(img.width().saturating_sub(1));
    let y1 = ((center.y + outer).ceil() as u32).min(img.height().saturating_sub(1));
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = ((x as f64 + 0.5 - center.x).powi(2) + (y as f64 + 0.5 - center.y).powi(2)).sqrt();
            let off = (d - r).abs();
            let color = if off <= stroke / 2.0 {
                [236u8, 32, 160]
            } else if off <= stroke / 2.0 + halo {
                [255, 255, 255]
            } else {
                continue;
            };
            let p = img.get_pixel_mut(x, y);
            p.0 = [color[0], color[1], color[2], 255];
        }
    }
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
    let pointer_view = pointer_view.filter(|p| display.view_bounds().contains(*p));
    let (vw, _) = display.view_size();
    let px_per_pt = capture.width_px as f64 / vw;

    // Mark the pointer on a copy of the capture; geometry is unaffected.
    let marked;
    let full = match pointer_view {
        Some(pv) => {
            let mut rgba = full.to_rgba8();
            draw_pointer_ring(&mut rgba, Point::new(pv.x * px_per_pt, pv.y * px_per_pt), px_per_pt);
            marked = DynamicImage::ImageRgba8(rgba);
            &marked
        }
        None => full,
    };

    let mut images = Vec::new();
    let sent_full = SentImage::full(capture, max_edge);
    images.push(EncodedImage { sent: sent_full, jpeg: encode(full, &sent_full)? });

    let pointer_norm = pointer_view.and_then(|p| view_to_image_px(p, &sent_full, display)).map(|px| {
        (px.y / sent_full.height_px as f64 * MODEL_NORM, px.x / sent_full.width_px as f64 * MODEL_NORM)
    });

    let mut pointer_norm_closeup = None;
    if closeup {
        if let Some(pv) = pointer_view {
            let size = (480.0 * px_per_pt).round() as u32;
            let center = Point::new(pv.x * px_per_pt, pv.y * px_per_pt);
            let crop = SentImage::crop_around(capture, center, size, 1024);
            pointer_norm_closeup = view_to_image_px(pv, &crop, display).map(|px| {
                (px.y / crop.height_px as f64 * MODEL_NORM, px.x / crop.width_px as f64 * MODEL_NORM)
            });
            images.push(EncodedImage { sent: crop, jpeg: encode(full, &crop)? });
        }
    }
    Ok(ModelImages { images, pointer_norm, pointer_norm_closeup })
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

#[cfg(test)]
mod tests {
    use super::*;
    use luma_core::geometry::Rect;

    #[test]
    fn ring_marks_pointer_without_covering_centre() {
        let mut img = image::RgbaImage::from_pixel(200, 200, image::Rgba([10, 10, 10, 255]));
        draw_pointer_ring(&mut img, Point::new(100.0, 100.0), 2.0);
        assert_eq!(img.get_pixel(100, 100).0, [10, 10, 10, 255], "centre stays visible");
        assert_eq!(img.get_pixel(130, 100).0, [236, 32, 160, 255], "ring at r = 15pt * 2");
        // ring near an edge must not panic
        draw_pointer_ring(&mut img, Point::new(1.0, 199.0), 2.0);
    }

    #[test]
    fn closeup_pointer_is_reported_in_its_own_frame() {
        let d = Display {
            index: 0,
            name: "t".into(),
            input_frame: Rect::new(0.0, 0.0, 1440.0, 900.0),
            input_per_point: 1.0,
            scale_factor: 2.0,
            is_primary: true,
        };
        let full = DynamicImage::ImageRgba8(image::RgbaImage::new(2880, 1800));
        let m = prepare(&full, &d, Some(Point::new(720.0, 450.0)), 1920, true).unwrap();
        assert_eq!(m.images.len(), 2);
        let (y, x) = m.pointer_norm_closeup.unwrap();
        assert!((y - 500.0).abs() < 2.0 && (x - 500.0).abs() < 2.0, "{y} {x}");
        // near a corner the crop shifts, so the pointer is off-centre in it
        let m = prepare(&full, &d, Some(Point::new(20.0, 20.0)), 1920, true).unwrap();
        let (y, x) = m.pointer_norm_closeup.unwrap();
        assert!(y < 100.0 && x < 100.0);
    }
}
