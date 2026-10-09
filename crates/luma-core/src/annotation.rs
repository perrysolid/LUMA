//! Typed annotation commands and the resolver that turns model tags into
//! display-space geometry.
//!
//! The model speaks in *image* coordinates and *ids*; the overlay speaks in
//! *display view points*. This module is the only bridge, which keeps the
//! visual layer independent of whatever grounding produced the geometry
//! (vision model today; accessibility frames, OCR snapping or an app plugin
//! tomorrow).

use crate::geometry::{norm_to_view, Display, DisplayRect, NormBox, Point, Rect, SentImage};
use crate::markup::Tag;
use crate::snap::SnapKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapeKind {
    Box,
    Circle,
    Highlight,
    Underline,
}

/// What the overlay renders. All rects are in the target display's view space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Annotation {
    Shape { display: usize, id: String, kind: ShapeKind, rect: Rect, label: Option<String> },
    /// LUMA's pointer flies to the target.
    Point { display: usize, id: String, rect: Rect, label: Option<String> },
    Arrow { display: usize, id: String, from: Rect, to: Rect, label: Option<String> },
    Step { display: usize, id: String, n: u32, rect: Rect, label: Option<String> },
    Spotlight { display: usize, rect: Rect },
    Zoom { display: usize, rect: Rect },
    Focus { display: usize, id: String, rect: Rect },
    /// Free-standing text callout anchored at a target.
    Label { display: usize, id: String, rect: Rect, text: String },
    /// Whiteboard panel for a diagram LUMA draws itself.
    Board { display: usize, id: String, rect: Rect, title: Option<String> },
    /// A box with text in it: one element of LUMA's own diagram.
    Node { display: usize, id: String, rect: Rect, text: String },
    /// A freehand stroke through points (curves, brackets, loops, circling).
    Sketch { display: usize, id: String, points: Vec<Point>, closed: bool, label: Option<String>, color: Option<String> },
    Clear { id: Option<String> },
}

impl Annotation {
    /// The id of a mark that occupies a place on screen.
    pub fn id(&self) -> Option<&str> {
        match self {
            Annotation::Shape { id, .. }
            | Annotation::Point { id, .. }
            | Annotation::Arrow { id, .. }
            | Annotation::Step { id, .. }
            | Annotation::Focus { id, .. }
            | Annotation::Label { id, .. }
            | Annotation::Board { id, .. }
            | Annotation::Node { id, .. }
            | Annotation::Sketch { id, .. } => Some(id),
            _ => None,
        }
    }

    /// The display a mark is drawn on (None for clear).
    pub fn display(&self) -> Option<usize> {
        match self {
            Annotation::Shape { display, .. }
            | Annotation::Point { display, .. }
            | Annotation::Arrow { display, .. }
            | Annotation::Step { display, .. }
            | Annotation::Spotlight { display, .. }
            | Annotation::Zoom { display, .. }
            | Annotation::Focus { display, .. }
            | Annotation::Label { display, .. }
            | Annotation::Board { display, .. }
            | Annotation::Node { display, .. }
            | Annotation::Sketch { display, .. } => Some(*display),
            Annotation::Clear { .. } => None,
        }
    }

    /// The single target rect of a shape, point or step.
    pub fn target(&self) -> Option<(usize, Rect)> {
        match self {
            Annotation::Shape { display, rect, .. }
            | Annotation::Point { display, rect, .. }
            | Annotation::Step { display, rect, .. } => Some((*display, *rect)),
            _ => None,
        }
    }

    /// The same mark with a refined target rect.
    pub fn with_rect(&self, r: Rect) -> Annotation {
        let mut a = self.clone();
        match &mut a {
            Annotation::Shape { rect, .. } | Annotation::Point { rect, .. } | Annotation::Step { rect, .. } => *rect = r,
            _ => {}
        }
        a
    }
}

/// Marker colours a sketch may use (the overlay maps them to a palette).
pub const SKETCH_COLORS: &[&str] = &["green", "blue", "purple", "orange", "pink", "yellow", "white", "red"];

/// A target the model has marked during this session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkedItem {
    pub id: String,
    pub label: Option<String>,
    pub at: DisplayRect,
    pub turn: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolveError {
    pub tag: String,
    pub reason: String,
}

/// Refines a resolved rect (e.g. accessibility snapping). Returns the better
/// rect, or None to keep the model's.
pub type Snapper<'a> = Box<dyn FnMut(&DisplayRect, Option<&str>, SnapKind) -> Option<Rect> + Send + 'a>;

/// Resolves tags against the images sent this turn and the ids known so far.
pub struct Resolver<'a> {
    pub displays: &'a [Display],
    /// Images in the order they were attached; the model's `img="1"` is index 0.
    pub images: &'a [SentImage],
    pub items: HashMap<String, MarkedItem>,
    pub turn: u32,
    auto_id: u32,
    snapper: Option<Snapper<'a>>,
    /// Whether the last resolved mark was snapped to an accessibility frame.
    pub last_snapped: bool,
    /// Arrows naming ids that are not marked yet ("browser → server" said
    /// before the server node is drawn); resolved as soon as they exist.
    deferred: Vec<Tag>,
    /// A board moved off busy content: marks the model placed inside its
    /// original area move with it.
    shift: Option<(DisplayRect, Point)>,
}

impl<'a> Resolver<'a> {
    pub fn new(displays: &'a [Display], images: &'a [SentImage], prior: Vec<MarkedItem>, turn: u32) -> Self {
        Self {
            displays,
            images,
            items: prior.into_iter().map(|m| (m.id.clone(), m)).collect(),
            turn,
            auto_id: 0,
            snapper: None,
            last_snapped: false,
            deferred: Vec::new(),
            shift: None,
        }
    }

    pub fn with_snapper(mut self, f: Snapper<'a>) -> Self {
        self.snapper = Some(f);
        self
    }

    /// Model geometry, refined by the snapper when one is set: elements snap
    /// to accessibility frames, highlights and underlines to text lines.
    fn snapped(&mut self, tag: &Tag, at: DisplayRect, label: Option<&str>) -> DisplayRect {
        let kind = match tag.name.as_str() {
            "box" | "circle" | "step" => SnapKind::Area,
            "point" => SnapKind::Point,
            "highlight" | "underline" => SnapKind::Text,
            _ => return at,
        };
        let label = label.or(tag.attr("text"));
        match self.snapper.as_mut().and_then(|f| f(&at, label, kind)) {
            Some(rect) => {
                self.last_snapped = true;
                DisplayRect { display_index: at.display_index, rect }
            }
            None => at,
        }
    }

    /// Replace a remembered item's geometry (after a refine pass).
    pub fn update_item(&mut self, id: &str, at: DisplayRect) {
        if let Some(m) = self.items.get_mut(id) {
            m.at = at;
        }
    }

    pub fn resolve(&mut self, tag: &Tag) -> Result<Annotation, ResolveError> {
        self.last_snapped = false;
        let err = |reason: &str| ResolveError { tag: tag.name.clone(), reason: reason.to_string() };
        let label = tag.attr("label").filter(|s| !s.trim().is_empty()).map(str::to_string);
        match tag.name.as_str() {
            "box" | "circle" | "highlight" | "underline" | "point" => {
                let at = self.geometry(tag)?;
                let at = self.snapped(tag, at, label.as_deref());
                let id = self.id_for(tag);
                self.remember(&id, label.clone(), at);
                let (display, rect) = (at.display_index, at.rect);
                Ok(match tag.name.as_str() {
                    "point" => Annotation::Point { display, id, rect, label },
                    k => Annotation::Shape {
                        display,
                        id,
                        rect,
                        label,
                        kind: match k {
                            "box" => ShapeKind::Box,
                            "circle" => ShapeKind::Circle,
                            "highlight" => ShapeKind::Highlight,
                            _ => ShapeKind::Underline,
                        },
                    },
                })
            }
            "arrow" => {
                let waits_for_id = |k: &str| {
                    tag.attr(k).is_some_and(|v| !self.items.contains_key(v) && parse_numbers(v).len() < 2 && !v.trim().is_empty())
                };
                if (waits_for_id("from") || waits_for_id("to")) && self.deferred.len() < 16 {
                    self.deferred.push(tag.clone());
                    return Err(err("waiting for its ids to be marked"));
                }
                let from = self.endpoint(tag, "from")?;
                let to = self.endpoint(tag, "to")?;
                if from.display_index != to.display_index {
                    return Err(err("arrow endpoints are on different displays"));
                }
                let id = self.id_for(tag);
                Ok(Annotation::Arrow { display: from.display_index, id, from: from.rect, to: to.rect, label })
            }
            "step" => {
                let n = tag
                    .attr("n")
                    .and_then(|s| s.trim().parse().ok())
                    .ok_or_else(|| err("step needs n"))?;
                let mut at = self.target_or_geometry(tag)?;
                if tag.attr("target").is_none() {
                    at = self.snapped(tag, at, label.as_deref());
                }
                let id = self.id_for(tag);
                if tag.attr("box").is_some() {
                    self.remember(&id, label.clone(), at);
                }
                Ok(Annotation::Step { display: at.display_index, id, n, rect: at.rect, label })
            }
            "spotlight" | "zoom" => {
                let at = self.target_or_geometry(tag)?;
                Ok(if tag.name == "zoom" {
                    Annotation::Zoom { display: at.display_index, rect: at.rect }
                } else {
                    Annotation::Spotlight { display: at.display_index, rect: at.rect }
                })
            }
            "focus" => {
                let id = tag.attr("target").or(tag.attr("id")).ok_or_else(|| err("focus needs target"))?;
                let m = self.items.get(id).ok_or_else(|| err(&format!("unknown id {id}")))?;
                Ok(Annotation::Focus { display: m.at.display_index, id: id.to_string(), rect: m.at.rect })
            }
            "label" => {
                let text = tag
                    .attr("text")
                    .or(tag.attr("label"))
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| err("label needs text"))?
                    .to_string();
                let at = self.target_or_geometry(tag)?;
                let id = self.id_for(tag);
                Ok(Annotation::Label { display: at.display_index, id, rect: at.rect, text })
            }
            "board" => {
                let at = self.geometry(tag)?;
                let id = self.id_for(tag);
                let title = tag.attr("title").or(tag.attr("label")).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
                self.remember(&id, title.clone(), at);
                Ok(Annotation::Board { display: at.display_index, id, rect: at.rect, title })
            }
            "node" => {
                let at = self.geometry(tag)?;
                let text = tag
                    .attr("text")
                    .or(tag.attr("label"))
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| err("node needs text"))?
                    .to_string();
                let id = self.id_for(tag);
                self.remember(&id, Some(text.clone()), at);
                Ok(Annotation::Node { display: at.display_index, id, rect: at.rect, text })
            }
            "sketch" => {
                let raw = tag.attr("path").or(tag.attr("points")).ok_or_else(|| err("sketch needs path"))?;
                let n = parse_numbers(raw);
                if n.len() < 4 || n.len() % 2 != 0 || n.len() > 400 {
                    return Err(err(&format!("bad path {raw:?}")));
                }
                let (img, d) = self.image(tag)?;
                let points: Vec<Point> = n
                    .chunks(2)
                    .filter_map(|p| NormBox::from_point(p[0], p[1]))
                    .map(|b| norm_to_view(&b, img, d).rect.center())
                    .collect();
                let display = d.index;
                let points: Vec<Point> = match self.shift {
                    Some(_) => {
                        let (x0, y0) = points.iter().fold((f64::MAX, f64::MAX), |(a, b), p| (a.min(p.x), b.min(p.y)));
                        let (x1, y1) = points.iter().fold((f64::MIN, f64::MIN), |(a, b), p| (a.max(p.x), b.max(p.y)));
                        let at = DisplayRect { display_index: display, rect: Rect::new(x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0)) };
                        let moved = self.shifted(at);
                        let (dx, dy) = (moved.rect.x - at.rect.x, moved.rect.y - at.rect.y);
                        points.into_iter().map(|p| Point::new(p.x + dx, p.y + dy)).collect()
                    }
                    None => points,
                };
                let (x0, y0) = points.iter().fold((f64::MAX, f64::MAX), |(a, b), p| (a.min(p.x), b.min(p.y)));
                let (x1, y1) = points.iter().fold((f64::MIN, f64::MIN), |(a, b), p| (a.max(p.x), b.max(p.y)));
                let id = self.id_for(tag);
                self.remember(&id, label.clone(), DisplayRect { display_index: display, rect: Rect::new(x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0)) });
                let closed = tag.attr("closed").is_some_and(|c| c == "true");
                let color = tag.attr("color").map(|c| c.trim().to_lowercase()).filter(|c| SKETCH_COLORS.contains(&c.as_str()));
                Ok(Annotation::Sketch { display, id, points, closed, label, color })
            }
            "clear" => Ok(Annotation::Clear {
                id: tag.attr("target").or(tag.attr("id")).map(str::to_string),
            }),
            _ => Err(err("unknown tag")),
        }
    }

    /// Move everything the model places inside `from` by `by` (a board that
    /// was relocated to empty space, with its diagram).
    pub fn shift_region(&mut self, from: DisplayRect, by: Point) {
        self.shift = Some((from, by));
    }

    fn shifted(&self, at: DisplayRect) -> DisplayRect {
        match self.shift {
            Some((from, by)) if from.display_index == at.display_index => {
                let r = from.rect;
                let grown = Rect::new(r.x - r.w * 0.05, r.y - r.h * 0.05, r.w * 1.1, r.h * 1.1);
                if grown.contains(at.rect.center()) {
                    DisplayRect { display_index: at.display_index, rect: Rect::new(at.rect.x + by.x, at.rect.y + by.y, at.rect.w, at.rect.h) }
                } else {
                    at
                }
            }
            _ => at,
        }
    }

    /// Deferred arrows whose ids now exist. Call after each `resolve`.
    pub fn take_ready(&mut self) -> Vec<Annotation> {
        let waiting = std::mem::take(&mut self.deferred);
        let mut out = Vec::new();
        for t in waiting {
            let known = |k: &str| t.attr(k).is_some_and(|v| self.items.contains_key(v) || parse_numbers(v).len() >= 2);
            if known("from") && known("to") {
                if let Ok(a) = self.resolve(&t) {
                    out.push(a);
                }
            } else {
                self.deferred.push(t);
            }
        }
        out
    }

    fn id_for(&mut self, tag: &Tag) -> String {
        match tag.attr("id").map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => id.to_string(),
            None => {
                self.auto_id += 1;
                format!("_{}_{}", self.turn, self.auto_id)
            }
        }
    }

    fn remember(&mut self, id: &str, label: Option<String>, at: DisplayRect) {
        if id.starts_with('_') {
            return;
        }
        self.items.insert(id.to_string(), MarkedItem { id: id.to_string(), label, at, turn: self.turn });
    }

    fn image(&self, tag: &Tag) -> Result<(&SentImage, &Display), ResolveError> {
        let n: usize = tag.attr("img").and_then(|s| s.trim().parse().ok()).unwrap_or(1);
        let img = self
            .images
            .get(n.wrapping_sub(1))
            .ok_or_else(|| ResolveError { tag: tag.name.clone(), reason: format!("no image {n}") })?;
        let d = self
            .displays
            .iter()
            .find(|d| d.index == img.capture.display_index)
            .ok_or_else(|| ResolveError { tag: tag.name.clone(), reason: "display gone".into() })?;
        Ok((img, d))
    }

    fn geometry(&self, tag: &Tag) -> Result<DisplayRect, ResolveError> {
        let raw = tag
            .attr("box")
            .ok_or_else(|| ResolveError { tag: tag.name.clone(), reason: "missing box".into() })?;
        let nums = parse_numbers(raw);
        let nb = match nums.len() {
            4 => NormBox::sanitized([nums[0], nums[1], nums[2], nums[3]]),
            2 => NormBox::from_point(nums[0], nums[1]),
            _ => None,
        }
        .ok_or_else(|| ResolveError { tag: tag.name.clone(), reason: format!("bad box {raw:?}") })?;
        let (img, d) = self.image(tag)?;
        Ok(self.shifted(norm_to_view(&nb, img, d)))
    }

    fn target_or_geometry(&self, tag: &Tag) -> Result<DisplayRect, ResolveError> {
        if let Some(t) = tag.attr("target") {
            return self
                .items
                .get(t)
                .map(|m| m.at)
                .ok_or_else(|| ResolveError { tag: tag.name.clone(), reason: format!("unknown id {t}") });
        }
        self.geometry(tag)
    }

    fn endpoint(&self, tag: &Tag, key: &str) -> Result<DisplayRect, ResolveError> {
        let v = tag
            .attr(key)
            .ok_or_else(|| ResolveError { tag: tag.name.clone(), reason: format!("arrow needs {key}") })?;
        if let Some(m) = self.items.get(v) {
            return Ok(m.at);
        }
        let mut t = tag.clone();
        t.attrs.insert("box".into(), v.to_string());
        self.geometry(&t)
            .map_err(|_| ResolveError { tag: tag.name.clone(), reason: format!("unknown {key} {v:?}") })
    }
}

/// Accepts "1 2 3 4", "[1, 2, 3, 4]", "1,2,3,4", and paths like "1 2; 3 4".
fn parse_numbers(s: &str) -> Vec<f64> {
    s.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '[' | ']' | '(' | ')'))
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Capture;
    use crate::markup::{MarkupParser, Segment};

    fn display() -> Display {
        Display {
            index: 0,
            name: "d".into(),
            input_frame: Rect::new(0.0, 0.0, 1000.0, 500.0),
            input_per_point: 1.0,
            scale_factor: 2.0,
            is_primary: true,
        }
    }

    fn tags(s: &str) -> Vec<Tag> {
        let mut p = MarkupParser::new();
        p.push(s)
            .into_iter()
            .chain(p.finish())
            .filter_map(|s| match s {
                Segment::Tag(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn walkthrough_resolves_ids_across_tags() {
        let ds = [display()];
        let cap = Capture { display_index: 0, width_px: 2000, height_px: 1000 };
        let imgs = [SentImage::full(cap, 1000)];
        let mut r = Resolver::new(&ds, &imgs, vec![], 1);
        let out: Vec<_> = tags(
            r#"<box id="a" box="0 0 100 100" label="A"/><circle id="b" box="[500, 500, 600, 600]"/>
               <arrow from="a" to="b" label="calls"/><step n="1" target="a"/><focus target="b"/>
               <spotlight target="a"/><clear/>"#,
        )
        .iter()
        .map(|t| r.resolve(t).unwrap())
        .collect();
        assert_eq!(
            out[0],
            Annotation::Shape {
                display: 0,
                id: "a".into(),
                kind: ShapeKind::Box,
                rect: Rect::new(0.0, 0.0, 100.0, 50.0),
                label: Some("A".into())
            }
        );
        match &out[2] {
            Annotation::Arrow { from, to, label, .. } => {
                assert_eq!(*from, Rect::new(0.0, 0.0, 100.0, 50.0));
                assert_eq!(*to, Rect::new(500.0, 250.0, 100.0, 50.0));
                assert_eq!(label.as_deref(), Some("calls"));
            }
            o => panic!("{o:?}"),
        }
        assert!(matches!(out[3], Annotation::Step { n: 1, .. }));
        assert!(matches!(&out[4], Annotation::Focus { id, .. } if id == "b"));
        assert_eq!(out[6], Annotation::Clear { id: None });
        assert_eq!(r.items.len(), 2);
    }

    #[test]
    fn second_image_crop_is_addressable() {
        let ds = [display()];
        let cap = Capture { display_index: 0, width_px: 2000, height_px: 1000 };
        let crop = SentImage::crop_around(cap, crate::geometry::Point::new(1000.0, 500.0), 400, 400);
        let imgs = [SentImage::full(cap, 1000), crop];
        let mut r = Resolver::new(&ds, &imgs, vec![], 1);
        let a = r.resolve(&tags(r#"<box id="x" img="2" box="0 0 1000 1000"/>"#)[0]).unwrap();
        assert!(matches!(a, Annotation::Shape { rect, .. } if rect == Rect::new(400.0, 150.0, 200.0, 200.0)));
    }

    #[test]
    fn prior_items_from_earlier_turns_resolve() {
        let ds = [display()];
        let prior = vec![MarkedItem {
            id: "db".into(),
            label: Some("Database".into()),
            at: DisplayRect { display_index: 0, rect: Rect::new(10.0, 10.0, 20.0, 20.0) },
            turn: 1,
        }];
        let mut r = Resolver::new(&ds, &[], prior, 2);
        let a = r.resolve(&tags(r#"<focus target="db"/>"#)[0]).unwrap();
        assert!(matches!(a, Annotation::Focus { .. }));
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        let ds = [display()];
        let cap = Capture { display_index: 0, width_px: 2000, height_px: 1000 };
        let imgs = [SentImage::full(cap, 1000)];
        let mut r = Resolver::new(&ds, &imgs, vec![], 1);
        for t in tags(
            r#"<box box="1 2 3"/><box box="5 5 5 5"/><box img="3" box="0 0 10 10"/>
               <focus target="nope"/><arrow from="a"/><step target="x"/><label box="0 0 1 1"/>"#,
        ) {
            assert!(r.resolve(&t).is_err(), "{t:?}");
        }
    }

    #[test]
    fn snapper_sees_elements_and_text_spans_but_not_known_ids() {
        let ds = [display()];
        let cap = Capture { display_index: 0, width_px: 2000, height_px: 1000 };
        let imgs = [SentImage::full(cap, 1000)];
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = calls.clone();
        let mut r = Resolver::new(&ds, &imgs, vec![], 1).with_snapper(Box::new(move |_, label, kind| {
            seen.lock().unwrap().push((label.map(str::to_string), kind));
            (kind != SnapKind::Text).then(|| Rect::new(1.0, 2.0, 3.0, 4.0))
        }));
        let out: Vec<_> = tags(
            r#"<box id="a" box="0 0 100 100" label="Save"/><highlight box="0 0 10 10"/>
               <step n="1" target="a"/><point box="5 5"/>"#,
        )
        .iter()
        .map(|t| r.resolve(t).unwrap())
        .collect();
        assert!(matches!(&out[0], Annotation::Shape { rect, .. } if *rect == Rect::new(1.0, 2.0, 3.0, 4.0)));
        assert!(matches!(&out[1], Annotation::Shape { rect, .. } if *rect != Rect::new(1.0, 2.0, 3.0, 4.0)));
        assert_eq!(r.items["a"].at.rect, Rect::new(1.0, 2.0, 3.0, 4.0));
        assert_eq!(
            *calls.lock().unwrap(),
            vec![(Some("Save".to_string()), SnapKind::Area), (None, SnapKind::Text), (None, SnapKind::Point)]
        );
    }

    #[test]
    fn own_diagrams_resolve_boards_nodes_arrows_and_sketches() {
        let ds = [display()];
        let cap = Capture { display_index: 0, width_px: 2000, height_px: 1000 };
        let imgs = [SentImage::full(cap, 1000)];
        let mut r = Resolver::new(&ds, &imgs, vec![], 1);
        let out: Vec<_> = tags(
            r#"<board id="b" box="100 500 900 950" title="How DNS works"/>
               <node id="browser" box="200 550 300 700" text="Browser"/>
               <node id="dns" box="200 780 300 930" text="DNS resolver"/>
               <arrow from="browser" to="dns" label="asks"/>
               <sketch id="loop" path="400 600; 450 650; 400 700" closed="true" label="cache"/>"#,
        )
        .iter()
        .map(|t| r.resolve(t).unwrap())
        .collect();
        assert!(matches!(&out[0], Annotation::Board { title: Some(t), .. } if t == "How DNS works"));
        assert!(matches!(&out[1], Annotation::Node { text, rect, .. } if text == "Browser" && *rect == Rect::new(550.0, 100.0, 150.0, 50.0)));
        assert!(matches!(&out[3], Annotation::Arrow { .. }), "arrows connect nodes by id");
        match &out[4] {
            Annotation::Sketch { points, closed, .. } => {
                assert_eq!(points.len(), 3);
                assert!(*closed);
                assert!((points[1].x - 650.0).abs() < 1.5 && (points[1].y - 225.0).abs() < 1.5, "{:?}", points[1]);
            }
            o => panic!("{o:?}"),
        }
        for bad in [r#"<node box="1 2 3 4"/>"#, r#"<sketch path="1 2 3"/>"#] {
            assert!(r.resolve(&tags(bad)[0]).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_moved_board_takes_its_diagram_with_it() {
        let ds = [display()];
        let cap = Capture { display_index: 0, width_px: 2000, height_px: 1000 };
        let imgs = [SentImage::full(cap, 1000)];
        let mut r = Resolver::new(&ds, &imgs, vec![], 1);
        let board = match r.resolve(&tags(r#"<board id="b" box="0 0 500 500" title="T"/>"#)[0]).unwrap() {
            Annotation::Board { rect, .. } => rect,
            o => panic!("{o:?}"),
        };
        // the app moved it 500 points right
        r.shift_region(DisplayRect { display_index: 0, rect: board }, Point::new(500.0, 0.0));
        let ts = tags(r#"<node id="n" box="100 100 200 300" text="A"/><sketch path="300 100; 400 200"/><box box="700 700 800 800"/>"#);
        assert!(matches!(r.resolve(&ts[0]).unwrap(), Annotation::Node { rect, .. } if rect.x == 600.0));
        assert!(matches!(r.resolve(&ts[1]).unwrap(), Annotation::Sketch { points, .. } if (points[0].x - 600.0).abs() < 1.5));
        assert!(matches!(r.resolve(&ts[2]).unwrap(), Annotation::Shape { rect, .. } if rect.x == 700.0), "outside the board: unmoved");
    }

    #[test]
    fn arrows_to_nodes_not_drawn_yet_wait_for_them() {
        let ds = [display()];
        let cap = Capture { display_index: 0, width_px: 2000, height_px: 1000 };
        let imgs = [SentImage::full(cap, 1000)];
        let mut r = Resolver::new(&ds, &imgs, vec![], 1);
        let ts = tags(r#"<node id="a" box="100 100 200 300" text="A"/><arrow from="a" to="b" label="x"/><node id="b" box="100 600 200 800" text="B"/>"#);
        assert!(r.resolve(&ts[0]).is_ok());
        assert!(r.take_ready().is_empty());
        assert!(r.resolve(&ts[1]).is_err(), "b does not exist yet");
        assert!(r.take_ready().is_empty());
        assert!(r.resolve(&ts[2]).is_ok());
        let ready = r.take_ready();
        assert!(matches!(ready.as_slice(), [Annotation::Arrow { label: Some(l), .. }] if l == "x"));
        assert!(r.take_ready().is_empty(), "emitted once");
    }

    #[test]
    fn point_tag_accepts_two_numbers() {
        let ds = [display()];
        let cap = Capture { display_index: 0, width_px: 2000, height_px: 1000 };
        let imgs = [SentImage::full(cap, 1000)];
        let mut r = Resolver::new(&ds, &imgs, vec![], 1);
        let a = r.resolve(&tags(r#"<point box="500 500" label="here"/>"#)[0]).unwrap();
        match a {
            Annotation::Point { rect, .. } => {
                let c = rect.center();
                assert!((c.x - 500.0).abs() < 1e-9 && (c.y - 250.0).abs() < 1e-9);
            }
            o => panic!("{o:?}"),
        }
    }
}
