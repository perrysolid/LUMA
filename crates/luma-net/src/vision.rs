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

/// A magnified inset for `<zoom>`: the region around `rect` (view points)
/// at native resolution, as a JPEG data URL the overlay can show. Long edge
/// at most 900 px.
pub fn magnifier_data_url(full: &DynamicImage, display: &Display, rect: &luma_core::geometry::Rect) -> Result<String> {
    use base64::Engine;
    let (vw, vh) = display.view_size();
    let sx = full.width() as f64 / vw;
    let sy = full.height() as f64 / vh;
    let pad = 8.0;
    let r = luma_core::geometry::Rect::new((rect.x - pad) * sx, (rect.y - pad) * sy, (rect.w + 2.0 * pad) * sx, (rect.h + 2.0 * pad) * sy)
        .intersection(&luma_core::geometry::Rect::new(0.0, 0.0, full.width() as f64, full.height() as f64))
        .ok_or_else(|| anyhow::anyhow!("zoom region is off screen"))?;
    let crop = full.crop_imm(r.x as u32, r.y as u32, (r.w as u32).max(1), (r.h as u32).max(1));
    let crop = if crop.width().max(crop.height()) > 900 { crop.resize(900, 900, FilterType::Triangle) } else { crop };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&crop.to_rgb8())?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(out)))
}

/// Put a `<sketch>` stroke exactly on the line it traces (see
/// `luma_core::ink`): points move onto the nearest drawn stroke within ~18 pt
/// and gaps are filled in along it. Other marks are returned unchanged.
pub fn snap_sketch_to_ink(a: &luma_core::annotation::Annotation, full: &DynamicImage, display: &Display) -> luma_core::annotation::Annotation {
    use luma_core::annotation::Annotation;
    let Annotation::Sketch { points, .. } = a else { return a.clone() };
    let (vw, _) = display.view_size();
    let s = full.width() as f64 / vw;
    let owned;
    let rgba = match full.as_rgba8() {
        Some(b) => b,
        None => {
            owned = full.to_rgba8();
            &owned
        }
    };
    let img = luma_core::ink::Rgba { width: rgba.width(), height: rgba.height(), data: rgba.as_raw() };
    let px: Vec<(f64, f64)> = points.iter().map(|p| (p.x * s, p.y * s)).collect();
    let snapped = luma_core::ink::snap_path(&img, &px, 18.0 * s, 14.0 * s);
    let mut out = a.clone();
    if let Annotation::Sketch { points, .. } = &mut out {
        *points = snapped.into_iter().map(|(x, y)| Point::new(x / s, y / s)).collect();
    }
    out
}

/// Keep LUMA's own diagram off the user's content: if a `<board>` landed on a
/// busy part of the screen, move it (and, through the resolver, everything
/// drawn inside it) to the emptiest area of the same size nearby. Other marks,
/// and boards already on empty space, are returned unchanged.
pub fn place_board(
    a: luma_core::annotation::Annotation,
    resolver: &mut luma_core::annotation::Resolver,
    full: &DynamicImage,
    display: &Display,
) -> luma_core::annotation::Annotation {
    use luma_core::annotation::Annotation;
    use luma_core::geometry::{DisplayRect, Rect};
    let Annotation::Board { display: d, id, rect, title } = a else { return a };
    if d != display.index {
        return Annotation::Board { display: d, id, rect, title };
    }
    let (vw, _) = display.view_size();
    let s = full.width() as f64 / vw;
    let owned;
    let rgba = match full.as_rgba8() {
        Some(b) => b,
        None => {
            owned = full.to_rgba8();
            &owned
        }
    };
    let img = luma_core::ink::Rgba { width: rgba.width(), height: rgba.height(), data: rgba.as_raw() };
    let grid = luma_core::ink::BusyGrid::new(&img, (12.0 * s).round().max(4.0) as u32);
    let here = grid.busy(rect.x * s, rect.y * s, rect.w * s, rect.h * s);
    let moved = (here > 0.15)
        .then(|| grid.emptiest(rect.w * s, rect.h * s, (rect.x * s, rect.y * s)))
        .flatten()
        .filter(|&(_, _, b)| b < 0.1 && b < here * 0.5)
        .map(|(x, y, _)| Rect::new(x / s, y / s, rect.w, rect.h));
    match moved {
        Some(new) => {
            resolver.shift_region(DisplayRect { display_index: d, rect }, Point::new(new.x - rect.x, new.y - rect.y));
            resolver.update_item(&id, DisplayRect { display_index: d, rect: new });
            Annotation::Board { display: d, id, rect: new, title }
        }
        None => Annotation::Board { display: d, id, rect, title },
    }
}

/// A 64×40 grayscale fingerprint of a screen, for cheap change detection.
pub const THUMB_W: u32 = 64;
pub const THUMB_H: u32 = 40;

pub fn thumbnail(img: &DynamicImage) -> Vec<u8> {
    img.resize_exact(THUMB_W, THUMB_H, FilterType::Triangle).to_luma8().into_raw()
}

/// Fraction of thumbnail cells that changed noticeably (0..1). Robust to
/// small changes such as a blinking caret or a clock.
pub fn changed_fraction(a: &[u8], b: &[u8]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 1.0;
    }
    let changed = a.iter().zip(b).filter(|(x, y)| x.abs_diff(**y) > 24).count();
    changed as f64 / a.len() as f64
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
    fn change_detection_ignores_tiny_changes_and_catches_scrolls() {
        let base = image::RgbaImage::from_fn(1440, 900, |x, y| {
            let v = if (y / 30) % 2 == 0 { 230 } else { 40 } as u8;
            image::Rgba([v, v, v.wrapping_add((x % 7) as u8), 255])
        });
        let a = thumbnail(&DynamicImage::ImageRgba8(base.clone()));
        // a caret blinking somewhere
        let mut caret = base.clone();
        for y in 400..420 {
            caret.put_pixel(700, y, image::Rgba([0, 0, 0, 255]));
        }
        assert!(changed_fraction(&a, &thumbnail(&DynamicImage::ImageRgba8(caret))) < 0.02);
        // content scrolled by 30px: stripes swap
        let scrolled = image::RgbaImage::from_fn(1440, 900, |x, y| *base.get_pixel(x, (y + 30) % 900));
        assert!(changed_fraction(&a, &thumbnail(&DynamicImage::ImageRgba8(scrolled))) > 0.3);
    }

    #[test]
    fn magnifier_crops_the_region_at_native_resolution() {
        let d = Display {
            index: 0,
            name: "t".into(),
            input_frame: Rect::new(0.0, 0.0, 1440.0, 900.0),
            input_per_point: 1.0,
            scale_factor: 2.0,
            is_primary: true,
        };
        let full = DynamicImage::ImageRgba8(image::RgbaImage::new(2880, 1800));
        let url = magnifier_data_url(&full, &d, &Rect::new(100.0, 100.0, 40.0, 20.0)).unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"));
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&url[23..]).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (112, 72), "(40+16)×2 by (20+16)×2");
        assert!(magnifier_data_url(&full, &d, &Rect::new(5000.0, 0.0, 10.0, 10.0)).is_err());
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
