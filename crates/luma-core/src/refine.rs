//! Zoom-and-refine for small targets.
//!
//! On a full-screen image a 16-point icon is a handful of pixels, so the
//! model's box lands near it but rarely on it. When a mark is that small and
//! accessibility snapping found nothing, LUMA asks again on a tight,
//! native-resolution crop around the first guess and replaces the box if
//! the second answer is consistent with the first.

use crate::geometry::{norm_to_view, Capture, Display, NormBox, Point, Rect, SentImage};
use crate::markup::{MarkupParser, Segment};

/// Marks with a side shorter than this (view points) are refined.
pub const SMALL_TARGET_PT: f64 = 24.0;
/// The crop is upscaled so its long edge is at least this many pixels.
const CROP_MIN_EDGE_PX: u32 = 768;

pub const REFINE_TAGS: &[&str] = &["box", "none"];

pub fn needs_refine(r: &Rect) -> bool {
    r.w.min(r.h) < SMALL_TARGET_PT
}

/// A crop of the capture centred on `target` (view points): six times the
/// target, between 120 and 320 points across, upscaled for the model.
pub fn refine_crop(target: &Rect, display: &Display, capture: Capture) -> SentImage {
    let (vw, _) = display.view_size();
    let px_per_pt = capture.width_px as f64 / vw;
    let side_pt = (target.w.max(target.h) * 6.0).clamp(120.0, 320.0);
    let c = target.center();
    let mut s = SentImage::crop_around(capture, Point::new(c.x * px_per_pt, c.y * px_per_pt), (side_pt * px_per_pt).round() as u32, u32::MAX);
    let long = s.width_px.max(s.height_px).max(1);
    if long < CROP_MIN_EDGE_PX {
        let k = CROP_MIN_EDGE_PX as f64 / long as f64;
        s.width_px = (s.width_px as f64 * k).round() as u32;
        s.height_px = (s.height_px as f64 * k).round() as u32;
    }
    s
}

/// The question for the refine call. `what` is the mark's label or id.
pub fn refine_prompt(what: &str) -> String {
    format!(
        "This is a close-up crop of a computer screen. Find exactly one UI element: {what}. It is near the centre of the image.\n\
         Reply with only one tag and nothing else: <box box=\"ymin xmin ymax xmax\"/> tightly around the element's visible edges, \
         integers 0-1000 relative to this image. If the element is not clearly visible, reply <none/>."
    )
}

/// Human description of a mark for the refine prompt.
pub fn describe_target(id: &str, label: Option<&str>) -> String {
    match label.map(str::trim).filter(|l| !l.is_empty()) {
        Some(l) => format!("\"{l}\""),
        None => {
            let words = id.trim_start_matches('_').replace(['_', '-'], " ");
            if words.trim().is_empty() || id.starts_with('_') {
                "the small element at the centre".into()
            } else {
                format!("the {}", words.trim())
            }
        }
    }
}

/// Parse the refine reply into a view-space rect, or None to keep the
/// original. The new box must be plausible: near the first guess, not
/// most of the crop, and not degenerate.
pub fn parse_refined(reply: &str, crop: &SentImage, display: &Display, original: &Rect) -> Option<Rect> {
    let mut p = MarkupParser::with_tags(REFINE_TAGS);
    let tag = p.push(reply).into_iter().chain(p.finish()).find_map(|s| match s {
        Segment::Tag(t) => Some(t),
        _ => None,
    })?;
    if tag.name != "box" {
        return None;
    }
    let n: Vec<f64> = tag
        .attr("box")?
        .split(|c: char| c.is_whitespace() || c == ',' || c == '[' || c == ']')
        .filter_map(|p| p.parse().ok())
        .collect();
    if n.len() != 4 {
        return None;
    }
    let b = NormBox::sanitized([n[0], n[1], n[2], n[3]])?;
    if (b.xmax - b.xmin) > 800.0 || (b.ymax - b.ymin) > 800.0 {
        return None; // "the whole crop" is not an answer
    }
    let r = norm_to_view(&b, crop, display).rect;
    if r.w < 3.0 || r.h < 3.0 {
        return None;
    }
    let oc = original.center();
    let rc = r.center();
    let reach = (original.w.hypot(original.h) * 1.5).max(30.0);
    let moved = (oc.x - rc.x).hypot(oc.y - rc.y);
    (moved <= reach).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retina() -> (Display, Capture) {
        (
            Display {
                index: 0,
                name: "d".into(),
                input_frame: Rect::new(0.0, 0.0, 1440.0, 900.0),
                input_per_point: 1.0,
                scale_factor: 2.0,
                is_primary: true,
            },
            Capture { display_index: 0, width_px: 2880, height_px: 1800 },
        )
    }

    #[test]
    fn only_small_marks_are_refined() {
        assert!(needs_refine(&Rect::new(0.0, 0.0, 16.0, 16.0)));
        assert!(needs_refine(&Rect::new(0.0, 0.0, 200.0, 18.0)), "a thin row is small too");
        assert!(!needs_refine(&Rect::new(0.0, 0.0, 80.0, 30.0)));
    }

    #[test]
    fn crop_is_centred_native_and_upscaled() {
        let (d, cap) = retina();
        let c = refine_crop(&Rect::new(700.0, 440.0, 16.0, 16.0), &d, cap);
        // 120 pt minimum → 240 capture px, upscaled to 768
        assert_eq!((c.source_px.w, c.source_px.h), (240.0, 240.0));
        assert_eq!((c.width_px, c.height_px), (768, 768));
        assert_eq!((c.source_px.x, c.source_px.y), (1296.0, 776.0));
        // near a corner the crop shifts inside the capture
        let c = refine_crop(&Rect::new(2.0, 2.0, 10.0, 10.0), &d, cap);
        assert_eq!((c.source_px.x, c.source_px.y), (0.0, 0.0));
    }

    #[test]
    fn refined_box_maps_back_and_is_sanity_checked() {
        let (d, cap) = retina();
        let original = Rect::new(700.0, 440.0, 16.0, 16.0);
        let crop = refine_crop(&original, &d, cap);
        // crop covers view (648..768, 388..508); centre element 12 pt wide
        let r = parse_refined(r#"<box box="450 450 550 550"/>"#, &crop, &d, &original).unwrap();
        assert!((r.x - 702.0).abs() < 0.01 && (r.w - 12.0).abs() < 0.01, "{r:?}");
        assert!(parse_refined("<none/>", &crop, &d, &original).is_none());
        assert!(parse_refined("I can't see it", &crop, &d, &original).is_none());
        assert!(parse_refined(r#"<box box="0 0 1000 1000"/>"#, &crop, &d, &original).is_none(), "whole crop");
        assert!(parse_refined(r#"<box box="0 0 40 40"/>"#, &crop, &d, &original).is_none(), "too far from the first guess");
    }

    #[test]
    fn targets_are_described_for_the_model() {
        assert_eq!(describe_target("save_btn", Some("Save")), "\"Save\"");
        assert_eq!(describe_target("save_btn", None), "the save btn");
        assert_eq!(describe_target("_3_1", None), "the small element at the centre");
    }
}
