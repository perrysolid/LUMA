use luma_core::annotation::Annotation;
use luma_core::geometry::{Point, Rect};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct Viewport {
    pub w: f64,
    pub h: f64,
}

#[derive(Deserialize, Clone)]
pub struct Target {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Human-readable name, kept for result inspection.
    #[serde(default)]
    #[allow(dead_code)]
    pub label: String,
}

impl Target {
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.w, self.h)
    }
}

#[derive(Deserialize)]
pub struct Case {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub pointer: Option<String>,
    #[serde(default)]
    pub expect: Vec<String>,
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub say: Vec<String>,
    /// agent cases: the goal given to the agent
    #[serde(default)]
    pub goal: Option<String>,
    /// route cases: whether the reply should start a task
    #[serde(default)]
    pub task: Option<bool>,
    /// draws cases: annotation ops that must appear ("arrow", "point", ...)
    #[serde(default)]
    pub ops: Vec<String>,
    /// route cases: whether the reply should start a lesson (absent = no)
    #[serde(default)]
    pub lesson: Option<bool>,
    /// agent cases: whether the first action must require approval
    #[serde(default)]
    pub approval: Option<bool>,
    /// agent cases: an <open url> containing this also counts as correct
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Deserialize)]
pub struct Fixture {
    pub name: String,
    pub dpr: f64,
    #[allow(dead_code)]
    pub viewport: Viewport,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub title: String,
    pub targets: BTreeMap<String, Target>,
    /// Ground-truth line segments [x1, y1, x2, y2] (view points), for trace cases.
    #[serde(default)]
    pub lines: BTreeMap<String, [f64; 4]>,
    pub cases: Vec<Case>,
}

/// How well a stroke lies on a segment: (mean distance of its points to the
/// segment, fraction of the segment's length it spans).
pub fn trace_fit(points: &[Point], seg: [f64; 4]) -> (f64, f64) {
    let [x1, y1, x2, y2] = seg;
    let (dx, dy) = (x2 - x1, y2 - y1);
    let len2 = (dx * dx + dy * dy).max(1e-9);
    let mut ts = Vec::new();
    let mut dist = 0.0;
    for p in points {
        let t = (((p.x - x1) * dx + (p.y - y1) * dy) / len2).clamp(0.0, 1.0);
        ts.push(t);
        dist += ((x1 + t * dx - p.x).powi(2) + (y1 + t * dy - p.y).powi(2)).sqrt();
    }
    let n = points.len().max(1) as f64;
    let span = ts.iter().cloned().fold(0.0, f64::max) - ts.iter().cloned().fold(1.0, f64::min);
    (dist / n, span.max(0.0))
}

#[derive(Serialize, Default)]
pub struct CaseResult {
    pub id: String,
    pub kind: String,
    pub run: usize,
    pub pass: bool,
    /// first mark centre inside the expected target
    pub first_hit: Option<bool>,
    pub any_hit: Option<bool>,
    pub best_iou: Option<f64>,
    pub coverage: Option<f64>,
    pub order: Option<f64>,
    pub marks: usize,
    pub dropped_tags: usize,
    pub ttft_ms: u64,
    pub total_ms: u64,
    pub speech: String,
    pub raw: String,
    pub error: Option<String>,
}

impl CaseResult {
    pub fn error(id: &str, kind: &str, run: usize, e: &str) -> Self {
        Self { id: id.into(), kind: kind.into(), run, error: Some(e.into()), ..Default::default() }
    }

    pub fn line(&self) -> String {
        let f = |o: Option<f64>| o.map(|v| format!("{v:.2}")).unwrap_or_else(|| "  - ".into());
        format!(
            "{:<28} {:<9} {}  iou {}  cov {}  ord {}  marks {:>2}  ttft {:>5}ms  total {:>5}ms",
            self.id,
            self.kind,
            if self.pass { "PASS" } else { "FAIL" },
            f(self.best_iou),
            f(self.coverage),
            f(self.order),
            self.marks,
            self.ttft_ms,
            self.total_ms
        )
    }
}

/// Geometry of a mark that points at something (not arrows/clears).
fn mark_rect(a: &Annotation) -> Option<Rect> {
    match a {
        Annotation::Shape { rect, .. }
        | Annotation::Point { rect, .. }
        | Annotation::Step { rect, .. }
        | Annotation::Label { rect, .. }
        | Annotation::Focus { rect, .. }
        | Annotation::Spotlight { rect, .. }
        | Annotation::Zoom { rect, .. }
        | Annotation::Node { rect, .. } => Some(*rect),
        _ => None,
    }
}

pub fn target_hit(fx: &Fixture, id: &str, r: &Rect) -> bool {
    fx.targets.get(id).is_some_and(|t| hits(r, &t.rect()))
}

/// A mark hits a target if its centre is inside the target (with a few
/// points of tolerance for tight boxes around small icons).
fn hits(mark: &Rect, target: &Rect) -> bool {
    let t = Rect::new(target.x - 4.0, target.y - 4.0, target.w + 8.0, target.h + 8.0);
    t.contains(mark.center()) || (mark.iou(target) > 0.5)
}

#[allow(clippy::too_many_arguments)]
pub fn score_case(
    fx: &Fixture,
    case: &Case,
    speech: &str,
    marks: &[Annotation],
    dropped: usize,
    ttft: u64,
    total: u64,
    raw: String,
) -> CaseResult {
    let rects: Vec<Rect> = marks.iter().filter_map(mark_rect).collect();
    let mut r = CaseResult {
        id: case.id.clone(),
        kind: case.kind.clone(),
        marks: rects.len(),
        dropped_tags: dropped,
        ttft_ms: ttft,
        total_ms: total,
        speech: speech.trim().to_string(),
        raw,
        ..Default::default()
    };
    let target = |id: &str| fx.targets.get(id).map(|t| t.rect());
    match case.kind.as_str() {
        "trace" => {
            // annotate in place: a stroke ON the expected line, no board,
            // and no box swallowing the figure
            let seg = case.expect.first().and_then(|id| fx.lines.get(id)).copied();
            let best = seg.and_then(|seg| {
                marks
                    .iter()
                    .filter_map(|m| match m {
                        Annotation::Sketch { points, .. } => Some(trace_fit(points, seg)),
                        _ => None,
                    })
                    .min_by(|a, b| (a.0 - a.1 * 20.0).total_cmp(&(b.0 - b.1 * 20.0)))
            });
            let board = marks.iter().any(|m| matches!(m, Annotation::Board { .. }));
            // a box that swallows the figure: it encloses a whole traced line
            let encloses = |r: &Rect| {
                let r = Rect::new(r.x - 10.0, r.y - 10.0, r.w + 20.0, r.h + 20.0);
                fx.lines.values().any(|l| r.contains(Point::new(l[0], l[1])) && r.contains(Point::new(l[2], l[3])) && ((l[2] - l[0]).hypot(l[3] - l[1]) > 150.0))
            };
            let huge_box = marks.iter().any(|m| matches!(m, Annotation::Shape { rect, kind: luma_core::annotation::ShapeKind::Box | luma_core::annotation::ShapeKind::Circle, .. } if encloses(rect)));
            // within about one marker-stroke width of the line, along most of it
            let on_line = best.is_some_and(|(d, span)| d <= 8.0 && span >= 0.7);
            r.best_iou = best.map(|(d, _)| d);
            r.coverage = best.map(|(_, s)| s);
            r.pass = on_line && !board && !huge_box && dropped == 0;
            r.raw = format!("trace {best:?} board {board} huge_box {huge_box}. {}", r.raw);
        }
        "sketch" => {
            // LUMA's own diagram: a board, readable nodes inside it, arrows.
            let board = marks.iter().find_map(|m| match m {
                Annotation::Board { rect, .. } => Some(*rect),
                _ => None,
            });
            let nodes: Vec<Rect> = marks.iter().filter_map(|m| match m { Annotation::Node { rect, .. } => Some(*rect), _ => None }).collect();
            let arrows = marks.iter().filter(|m| matches!(m, Annotation::Arrow { .. })).count();
            let inside = board.is_some_and(|b| {
                let b = Rect::new(b.x - 6.0, b.y - 6.0, b.w + 12.0, b.h + 12.0);
                nodes.iter().all(|n| b.contains(Point::new(n.x, n.y)) && b.contains(Point::new(n.right() - 0.01, n.bottom() - 0.01)))
            });
            let overlapping = nodes.iter().enumerate().any(|(i, a)| nodes[i + 1..].iter().any(|b| a.intersection(b).is_some_and(|x| x.area() > 0.1 * a.area().min(b.area()))));
            let readable = nodes.iter().all(|n| n.w >= 60.0 && n.h >= 28.0);
            r.coverage = Some(nodes.len() as f64);
            r.pass = board.is_some() && nodes.len() >= 3 && arrows >= 2 && inside && !overlapping && readable && dropped == 0;
            r.raw = format!(
                "board {} nodes {} arrows {arrows} inside {inside} overlapping {overlapping} readable {readable} dropped {dropped}. {}",
                board.is_some(),
                nodes.len(),
                r.raw
            );
        }
        "draws" => {
            let ops: Vec<String> = marks
                .iter()
                .filter_map(|m| serde_json::to_value(m).ok())
                .filter_map(|v| v.get("op").and_then(|o| o.as_str()).map(str::to_string))
                .collect();
            let all = case.ops.iter().all(|o| ops.contains(o));
            // where an expected target is given, a mark of the first op must hit it
            let on_target = match case.expect.first().and_then(|id| target(id)) {
                Some(t) => marks.iter().filter(|m| serde_json::to_value(m).ok().and_then(|v| v.get("op").and_then(|o| o.as_str()).map(|o| Some(o) == case.ops.first().map(String::as_str))).unwrap_or(false)).filter_map(mark_rect).any(|m| hits(&m, &t)),
                None => true,
            };
            r.first_hit = case.expect.first().map(|_| on_target);
            r.pass = all && on_target && dropped == 0;
            r.raw = format!("ops {ops:?} dropped {dropped}. {}", r.raw);
        }
        "locate" | "refer" => {
            if let Some(t) = case.expect.first().and_then(|id| target(id)) {
                let first = rects.first().map(|m| hits(m, &t));
                let any = rects.iter().any(|m| hits(m, &t));
                r.first_hit = Some(first.unwrap_or(false));
                r.any_hit = Some(any);
                r.best_iou = Some(rects.iter().map(|m| m.iou(&t)).fold(0.0, f64::max));
                r.pass = first.unwrap_or(false);
            }
        }
        "flow" => {
            let expected: Vec<(String, Rect)> =
                case.expect.iter().filter_map(|id| target(id).map(|t| (id.clone(), t))).collect();
            let covered = expected.iter().filter(|(_, t)| rects.iter().any(|m| hits(m, t))).count();
            r.coverage = Some(covered as f64 / expected.len().max(1) as f64);
            // order: first mark index hitting each ordered component
            let idx: Vec<usize> = case
                .order
                .iter()
                .filter_map(|id| target(id))
                .filter_map(|t| rects.iter().position(|m| hits(m, &t)))
                .collect();
            let pairs = idx.windows(2).count();
            let good = idx.windows(2).filter(|w| w[0] < w[1]).count();
            r.order = Some(if pairs == 0 { 0.0 } else { good as f64 / pairs as f64 });
            r.pass = r.coverage.unwrap() >= 0.7 && r.order.unwrap() >= 0.75;
        }
        "ambiguous" => {
            let asks = speech.contains('?');
            let distinct_targets = {
                let mut v: Vec<Point> = Vec::new();
                for m in &rects {
                    let c = m.center();
                    if !v.iter().any(|p| (p.x - c.x).abs() < 40.0 && (p.y - c.y).abs() < 40.0) {
                        v.push(c);
                    }
                }
                v.len()
            };
            r.pass = asks && (distinct_targets == 0 || distinct_targets >= 2);
        }
        "explain" => {
            let s = speech.to_lowercase();
            r.pass = case.say.iter().all(|w| s.contains(&w.to_lowercase()));
        }
        _ => {}
    }
    // Spoken output must stay speakable: no markdown leaking into speech.
    if speech.contains("**") || speech.contains("```") || speech.contains("\n- ") {
        r.pass = false;
    }
    r
}

pub fn summarize(results: &[CaseResult]) -> String {
    let n = results.len().max(1) as f64;
    let pass = results.iter().filter(|r| r.pass).count();
    let grounding: Vec<&CaseResult> = results.iter().filter(|r| r.first_hit.is_some()).collect();
    let g_hit = grounding.iter().filter(|r| r.first_hit == Some(true)).count();
    let mean_iou = grounding.iter().filter_map(|r| r.best_iou).sum::<f64>() / grounding.len().max(1) as f64;
    let mut ttft: Vec<u64> = results.iter().filter(|r| r.error.is_none()).map(|r| r.ttft_ms).collect();
    ttft.sort();
    let p50 = ttft.get(ttft.len() / 2).copied().unwrap_or(0);
    let p90 = ttft.get(((ttft.len() as f64) * 0.9) as usize).or(ttft.last()).copied().unwrap_or(0);
    let errors = results.iter().filter(|r| r.error.is_some()).count();
    format!(
        "pass {pass}/{} ({:.0}%)  grounding hit {g_hit}/{} ({:.0}%)  mean IoU {mean_iou:.2}  ttft p50 {p50}ms p90 {p90}ms  errors {errors}",
        results.len(),
        100.0 * pass as f64 / n,
        grounding.len(),
        100.0 * g_hit as f64 / grounding.len().max(1) as f64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx() -> Fixture {
        serde_json::from_str(
            r#"{"name":"t","dpr":2,"viewport":{"w":1000,"h":800},
                "targets":{"a":{"x":100,"y":100,"w":50,"h":20},"b":{"x":400,"y":100,"w":50,"h":20},"c":{"x":700,"y":100,"w":50,"h":20}},
                "cases":[]}"#,
        )
        .unwrap()
    }

    fn shape(x: f64, y: f64) -> Annotation {
        Annotation::Shape {
            display: 0,
            id: "x".into(),
            kind: luma_core::annotation::ShapeKind::Box,
            rect: Rect::new(x, y, 50.0, 20.0),
            label: None,
        }
    }

    fn case(kind: &str, expect: &[&str], order: &[&str]) -> Case {
        Case {
            id: "c".into(),
            kind: kind.into(),
            q: "q".into(),
            pointer: None,
            expect: expect.iter().map(|s| s.to_string()).collect(),
            order: order.iter().map(|s| s.to_string()).collect(),
            say: vec![],
            goal: None,
            task: None,
            lesson: None,
            ops: Vec::new(),
            approval: None,
            url: None,
        }
    }

    #[test]
    fn locate_scores_first_mark() {
        let f = fx();
        let r = score_case(&f, &case("locate", &["b"], &[]), "Here.", &[shape(402.0, 101.0)], 0, 1, 2, String::new());
        assert!(r.pass && r.best_iou.unwrap() > 0.8);
        let r = score_case(&f, &case("locate", &["b"], &[]), "Here.", &[shape(100.0, 100.0), shape(400.0, 100.0)], 0, 1, 2, String::new());
        assert!(!r.pass && r.any_hit == Some(true));
    }

    #[test]
    fn flow_scores_coverage_and_order() {
        let f = fx();
        let marks = [shape(100.0, 100.0), shape(400.0, 100.0), shape(700.0, 100.0)];
        let r = score_case(&f, &case("flow", &["a", "b", "c"], &["a", "b", "c"]), "x", &marks, 0, 1, 2, String::new());
        assert_eq!((r.coverage, r.order, r.pass), (Some(1.0), Some(1.0), true));
        let rev = [shape(700.0, 100.0), shape(400.0, 100.0), shape(100.0, 100.0)];
        let r = score_case(&f, &case("flow", &["a", "b", "c"], &["a", "b", "c"]), "x", &rev, 0, 1, 2, String::new());
        assert_eq!(r.order, Some(0.0));
        assert!(!r.pass);
    }

    #[test]
    fn ambiguity_needs_a_question_and_no_single_commitment() {
        let f = fx();
        let c = case("ambiguous", &[], &[]);
        assert!(score_case(&f, &c, "Which one, the first or second?", &[shape(100.0, 100.0), shape(400.0, 100.0)], 0, 1, 1, String::new()).pass);
        assert!(!score_case(&f, &c, "Which one?", &[shape(100.0, 100.0)], 0, 1, 1, String::new()).pass);
        assert!(!score_case(&f, &c, "Moving it now.", &[], 0, 1, 1, String::new()).pass);
    }

    #[test]
    fn markdown_in_speech_fails() {
        let f = fx();
        let r = score_case(&f, &case("locate", &["a"], &[]), "**Bold** here", &[shape(100.0, 100.0)], 0, 1, 1, String::new());
        assert!(!r.pass);
    }
}
