//! Typed annotation commands and the resolver that turns model tags into
//! display-space geometry.
//!
//! The model speaks in *image* coordinates and *ids*; the overlay speaks in
//! *display view points*. This module is the only bridge, which keeps the
//! visual layer independent of whatever grounding produced the geometry
//! (vision model today; accessibility frames, OCR snapping or an app plugin
//! tomorrow).

use crate::geometry::{norm_to_view, Display, DisplayRect, NormBox, Rect, SentImage};
use crate::markup::Tag;
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
    Clear { id: Option<String> },
}

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

/// Resolves tags against the images sent this turn and the ids known so far.
pub struct Resolver<'a> {
    pub displays: &'a [Display],
    /// Images in the order they were attached; the model's `img="1"` is index 0.
    pub images: &'a [SentImage],
    pub items: HashMap<String, MarkedItem>,
    pub turn: u32,
    auto_id: u32,
}

impl<'a> Resolver<'a> {
    pub fn new(displays: &'a [Display], images: &'a [SentImage], prior: Vec<MarkedItem>, turn: u32) -> Self {
        Self {
            displays,
            images,
            items: prior.into_iter().map(|m| (m.id.clone(), m)).collect(),
            turn,
            auto_id: 0,
        }
    }

    pub fn resolve(&mut self, tag: &Tag) -> Result<Annotation, ResolveError> {
        let err = |reason: &str| ResolveError { tag: tag.name.clone(), reason: reason.to_string() };
        let label = tag.attr("label").filter(|s| !s.trim().is_empty()).map(str::to_string);
        match tag.name.as_str() {
            "box" | "circle" | "highlight" | "underline" | "point" => {
                let at = self.geometry(tag)?;
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
                let at = self.target_or_geometry(tag)?;
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
            "clear" => Ok(Annotation::Clear {
                id: tag.attr("target").or(tag.attr("id")).map(str::to_string),
            }),
            _ => Err(err("unknown tag")),
        }
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
        Ok(norm_to_view(&nb, img, d))
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

/// Accepts "1 2 3 4", "[1, 2, 3, 4]", "1,2,3,4".
fn parse_numbers(s: &str) -> Vec<f64> {
    s.split(|c: char| c.is_whitespace() || c == ',' || c == '[' || c == ']')
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
