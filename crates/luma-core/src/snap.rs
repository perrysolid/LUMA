//! Accessibility snapping: replace a model's approximate box with the exact
//! frame of the UI element it meant.
//!
//! Vision models land near a control but rarely on its edges. The OS
//! accessibility tree (AX on macOS, UI Automation on Windows) knows the real
//! frames of native controls. The platform layer hit-tests at the model's box
//! and hands us the element there plus its ancestors; this module decides
//! which of those, if any, is the element the model meant. When nothing fits
//! well, the model's box is kept: a slightly loose box is better than a
//! confident snap to the wrong thing.

use crate::geometry::{Display, Point, Rect};

/// An accessibility element near the target, in OS input space.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub frame: Rect,
    /// Platform role, e.g. "AXButton" or "Button".
    pub role: String,
    /// Title / description / name, when the element has one.
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapKind {
    /// Box, circle, step: the model outlined a whole element.
    Area,
    /// The model gave a point; snap to the smallest control under it.
    Point,
    /// Highlight / underline: a span of text lines (snapped with OCR).
    Text,
}

/// A line of text found by on-device OCR, in display view space.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLine {
    pub frame: Rect,
    pub text: String,
}

/// Snap a text highlight to the OCR lines it covers: exact top and bottom
/// edges from the lines, horizontal ends from the lines when the model's
/// box reaches (nearly) to them, otherwise the model's own ends clipped to
/// the text. None keeps the model's box.
pub fn snap_text(model: &Rect, lines: &[TextLine]) -> Option<Rect> {
    let covered: Vec<&Rect> = lines
        .iter()
        .map(|l| &l.frame)
        .filter(|f| {
            let v = (model.bottom().min(f.bottom()) - model.y.max(f.y)).max(0.0);
            let h = (model.right().min(f.right()) - model.x.max(f.x)).max(0.0);
            v >= 0.5 * f.h && h >= 0.3 * model.w.min(f.w)
        })
        .collect();
    let first = covered.first()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.right(), first.bottom());
    for f in &covered[1..] {
        x0 = x0.min(f.x);
        y0 = y0.min(f.y);
        x1 = x1.max(f.right());
        y1 = y1.max(f.bottom());
    }
    let (h, w) = (y1 - y0, x1 - x0);
    if h < model.h * 0.4 || h > model.h * 2.5 {
        return None; // OCR and model disagree about what this is
    }
    let slack = 0.15 * w;
    let left = if model.x <= x0 + slack { x0 } else { model.x.min(x1) };
    let right = if model.right() >= x1 - slack { x1 } else { model.right().max(x0) };
    (right - left >= 4.0).then(|| Rect::new(left, y0, right - left, h))
}

/// Containers that cover a region of the UI rather than one thing in it.
const CONTAINER_ROLES: &[&str] = &[
    "AXApplication", "AXWindow", "AXSheet", "AXDrawer", "AXGroup", "AXScrollArea", "AXSplitGroup",
    "AXWebArea", "AXLayoutArea", "AXLayoutItem", "AXUnknown", "AXBrowser", "AXOutline", "AXTable",
    "AXList", "AXSplitter",
    "Window", "Pane", "Group", "Document", "Custom", "Table", "List", "Tree",
];

/// Things people point at.
const CONTROL_ROLES: &[&str] = &[
    "AXButton", "AXCheckBox", "AXRadioButton", "AXPopUpButton", "AXMenuButton", "AXMenuItem",
    "AXMenuBarItem", "AXTextField", "AXTextArea", "AXSearchField", "AXComboBox", "AXSlider",
    "AXLink", "AXTab", "AXTabButton", "AXDisclosureTriangle", "AXIncrementor", "AXImage", "AXCell",
    "AXRow", "AXStaticText", "AXHeading", "AXToolbarButton", "AXDockItem", "AXColorWell",
    "Button", "CheckBox", "RadioButton", "ComboBox", "Edit", "Hyperlink", "MenuItem", "TabItem",
    "ListItem", "TreeItem", "SplitButton", "Slider", "Image", "Text", "DataItem", "Spinner",
];

fn is_container(role: &str) -> bool {
    CONTAINER_ROLES.contains(&role)
}

fn is_control(role: &str) -> bool {
    CONTROL_ROLES.contains(&role)
}

/// Lowercase alphanumeric words.
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// 0..1: how well an element's name matches the model's label.
pub fn name_match(label: &str, name: &str) -> f64 {
    let (l, n) = (words(label), words(name));
    if l.is_empty() || n.is_empty() {
        return 0.0;
    }
    if l == n {
        return 1.0;
    }
    let shared = l.iter().filter(|w| n.contains(w)).count() as f64;
    shared / l.len().max(n.len()) as f64
}

/// The element frame to use instead of `model` (both in the display's view
/// space), or None to keep the model's box.
pub fn snap(model: &Rect, label: Option<&str>, kind: SnapKind, candidates: &[Element], display: &Display) -> Option<Rect> {
    if kind == SnapKind::Text {
        return None; // text spans snap to OCR lines (`snap_text`), not elements
    }
    let bounds = display.view_bounds();
    let screen_area = bounds.area();
    let center = model.center();
    let mut best: Option<(f64, Rect)> = None;
    for e in candidates {
        let tl = display.input_to_view(Point::new(e.frame.x, e.frame.y));
        let r = Rect::new(tl.x, tl.y, e.frame.w / display.input_per_point, e.frame.h / display.input_per_point);
        if r.w < 4.0 || r.h < 4.0 || r.intersection(&bounds).is_none() {
            continue;
        }
        // Whole panes and windows are never what someone points at.
        if r.area() > screen_area * 0.2 || (is_container(&e.role) && r.area() > screen_area * 0.02) {
            continue;
        }
        let named = label.map(|l| name_match(l, &e.name)).unwrap_or(0.0);
        let score = match kind {
            SnapKind::Area => {
                let iou = model.iou(&r);
                let ok = iou >= 0.5 || (iou >= 0.25 && named >= 0.5);
                if !ok {
                    continue;
                }
                iou + 0.3 * named + if is_control(&e.role) { 0.05 } else { 0.0 }
            }
            SnapKind::Point => {
                // The smallest real control under the point.
                if !r.contains(center) || !is_control(&e.role) || r.area() > screen_area * 0.01 {
                    continue;
                }
                1.0 - (r.area() / (screen_area * 0.01)) * 0.5 + 0.3 * named
            }
            SnapKind::Text => unreachable!(),
        };
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((score, r));
        }
    }
    best.map(|(_, r)| r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(x: f64, y: f64, w: f64, h: f64) -> TextLine {
        TextLine { frame: Rect::new(x, y, w, h), text: String::new() }
    }

    #[test]
    fn text_highlight_snaps_to_ocr_lines() {
        let lines = [line(100.0, 100.0, 400.0, 18.0), line(100.0, 122.0, 380.0, 18.0), line(100.0, 144.0, 200.0, 18.0)];
        // Sloppy box over the first two lines, slightly short on both ends.
        let got = snap_text(&Rect::new(108.0, 96.0, 385.0, 40.0), &lines).unwrap();
        assert_eq!(got, Rect::new(100.0, 100.0, 400.0, 40.0));
        // A phrase in the middle of a line keeps its own horizontal ends.
        let got = snap_text(&Rect::new(220.0, 103.0, 90.0, 14.0), &lines).unwrap();
        assert_eq!(got, Rect::new(220.0, 100.0, 90.0, 18.0));
        // Nothing under the box, or a wildly different height: keep the model's.
        assert!(snap_text(&Rect::new(700.0, 100.0, 50.0, 18.0), &lines).is_none());
        assert!(snap_text(&Rect::new(100.0, 100.0, 400.0, 6.0), &[line(100.0, 60.0, 400.0, 100.0)]).is_none());
    }

    fn display() -> Display {
        // Retina-style macOS display at a negative origin (secondary, left).
        Display {
            index: 1,
            name: "d".into(),
            input_frame: Rect::new(-1440.0, 0.0, 1440.0, 900.0),
            input_per_point: 1.0,
            scale_factor: 2.0,
            is_primary: false,
        }
    }

    fn el(role: &str, name: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
        Element { frame: Rect::new(x, y, w, h), role: role.into(), name: name.into() }
    }

    #[test]
    fn loose_model_box_snaps_to_the_button_frame() {
        // Button at view (100, 50, 80, 24) → input (-1340, 50).
        let button = el("AXButton", "Save", -1340.0, 50.0, 80.0, 24.0);
        let window = el("AXWindow", "Doc", -1440.0, 0.0, 1440.0, 900.0);
        let text = el("AXStaticText", "Save", -1320.0, 54.0, 40.0, 16.0);
        let model = Rect::new(96.0, 46.0, 90.0, 30.0);
        let got = snap(&model, Some("Save button"), SnapKind::Area, &[text, button, window], &display());
        assert_eq!(got, Some(Rect::new(100.0, 50.0, 80.0, 24.0)));
    }

    #[test]
    fn far_off_box_keeps_the_model_geometry() {
        let button = el("AXButton", "Save", -1340.0, 50.0, 80.0, 24.0);
        let model = Rect::new(400.0, 400.0, 90.0, 30.0);
        assert_eq!(snap(&model, Some("Save"), SnapKind::Area, &[button], &display()), None);
    }

    #[test]
    fn big_containers_are_never_chosen() {
        // The model outlined a whole sidebar; the only frame matching it is a group.
        let group = el("AXGroup", "", -1440.0, 0.0, 300.0, 900.0);
        let model = Rect::new(0.0, 0.0, 300.0, 900.0);
        assert_eq!(snap(&model, None, SnapKind::Area, &[group], &display()), None);
    }

    #[test]
    fn a_matching_name_rescues_a_moderate_overlap() {
        let link = el("AXLink", "Pricing", -1340.0, 50.0, 60.0, 20.0);
        // IoU ≈ 0.3 on geometry alone.
        let model = Rect::new(90.0, 40.0, 90.0, 40.0);
        assert!(snap(&model, None, SnapKind::Area, &[link.clone()], &display()).is_none());
        assert_eq!(
            snap(&model, Some("Pricing"), SnapKind::Area, &[link], &display()),
            Some(Rect::new(100.0, 50.0, 60.0, 20.0))
        );
    }

    #[test]
    fn point_snaps_to_the_smallest_control_under_it() {
        let toolbar = el("AXToolbar", "", -1440.0, 0.0, 1440.0, 40.0);
        let button = el("AXButton", "Share", -1000.0, 8.0, 28.0, 24.0);
        let model = Rect::new(445.0, 18.0, 4.0, 4.0);
        assert_eq!(
            snap(&model, None, SnapKind::Point, &[toolbar, button], &display()),
            Some(Rect::new(440.0, 8.0, 28.0, 24.0))
        );
    }

    #[test]
    fn windows_input_space_is_scaled_to_view_points() {
        let d = Display {
            index: 0,
            name: "w".into(),
            input_frame: Rect::new(0.0, 0.0, 3000.0, 2000.0),
            input_per_point: 1.5,
            scale_factor: 1.5,
            is_primary: true,
        };
        let button = el("Button", "OK", 300.0, 150.0, 120.0, 45.0);
        let model = Rect::new(198.0, 98.0, 84.0, 34.0);
        assert_eq!(snap(&model, Some("OK"), SnapKind::Area, &[button], &d), Some(Rect::new(200.0, 100.0, 80.0, 30.0)));
    }

    #[test]
    fn name_match_scores() {
        assert_eq!(name_match("Save", "save"), 1.0);
        assert_eq!(name_match("Save button", "Save"), 0.5);
        assert_eq!(name_match("", "Save"), 0.0);
        assert_eq!(name_match("Export", "Save"), 0.0);
    }
}
