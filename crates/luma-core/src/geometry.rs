//! Coordinate spaces and the transforms between them.
//!
//! LUMA deals with five coordinate spaces. Keeping them explicit is what makes
//! annotations land in the right place on Retina, scaled, rotated and
//! multi-monitor setups:
//!
//! 1. **Model space** – what Gemini returns: `[ymin, xmin, ymax, xmax]`
//!    normalized to 0..=1000 relative to *the image it was shown*.
//! 2. **Sent-image space** – pixels of the (possibly cropped and resized)
//!    JPEG that was uploaded.
//! 3. **Capture space** – physical pixels of the full display screenshot.
//! 4. **Display-local view space** – points relative to a display's top-left
//!    corner. Overlay windows cover exactly one display, so annotations are
//!    drawn in this space (1 unit == 1 CSS px in the overlay webview).
//! 5. **Input space** – the OS-wide space used for cursor position and
//!    synthetic input: global points on macOS, virtual-desktop physical pixels
//!    on Windows (per-monitor DPI aware v2).
//!
//! Everything in this module is pure and unit tested; platform code only has
//! to fill in a [`Display`] honestly.

use serde::{Deserialize, Serialize};

pub const MODEL_NORM: f64 = 1000.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn area(&self) -> f64 {
        self.w.max(0.0) * self.h.max(0.0)
    }
    /// Half-open containment: a point on the right/bottom edge belongs to the
    /// neighbouring display, never to both.
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }
    pub fn intersection(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }
    pub fn iou(&self, o: &Rect) -> f64 {
        let inter = self.intersection(o).map(|r| r.area()).unwrap_or(0.0);
        let union = self.area() + o.area() - inter;
        if union <= 0.0 {
            0.0
        } else {
            inter / union
        }
    }
    /// Distance from a point to the rectangle (0 when inside).
    pub fn distance_to(&self, p: Point) -> f64 {
        let dx = (self.x - p.x).max(0.0).max(p.x - self.right());
        let dy = (self.y - p.y).max(0.0).max(p.y - self.bottom());
        (dx * dx + dy * dy).sqrt()
    }
    pub fn clamp_to(&self, bounds: &Rect) -> Rect {
        let x0 = self.x.clamp(bounds.x, bounds.right());
        let y0 = self.y.clamp(bounds.y, bounds.bottom());
        let x1 = self.right().clamp(bounds.x, bounds.right());
        let y1 = self.bottom().clamp(bounds.y, bounds.bottom());
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

/// One physical display as LUMA sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Display {
    /// Stable index used to address overlay windows (`overlay-{index}`).
    pub index: usize,
    pub name: String,
    /// Display bounds in OS input space.
    pub input_frame: Rect,
    /// Input-space units per view point. 1.0 on macOS (input space is
    /// points); the DPI scale on Windows (input space is physical pixels).
    pub input_per_point: f64,
    /// Physical pixels per view point (2.0 on a Retina display).
    pub scale_factor: f64,
    pub is_primary: bool,
}

impl Display {
    /// Size of the display in view points (the overlay's CSS pixel size).
    pub fn view_size(&self) -> (f64, f64) {
        (
            self.input_frame.w / self.input_per_point,
            self.input_frame.h / self.input_per_point,
        )
    }

    pub fn view_bounds(&self) -> Rect {
        let (w, h) = self.view_size();
        Rect::new(0.0, 0.0, w, h)
    }

    pub fn input_to_view(&self, p: Point) -> Point {
        Point::new(
            (p.x - self.input_frame.x) / self.input_per_point,
            (p.y - self.input_frame.y) / self.input_per_point,
        )
    }

    pub fn view_to_input(&self, p: Point) -> Point {
        Point::new(
            self.input_frame.x + p.x * self.input_per_point,
            self.input_frame.y + p.y * self.input_per_point,
        )
    }
}

/// Which display contains an input-space point. Falls back to the nearest
/// display so a cursor parked exactly on an outer edge still resolves.
pub fn display_at(displays: &[Display], p: Point) -> Option<&Display> {
    displays
        .iter()
        .find(|d| d.input_frame.contains(p))
        .or_else(|| {
            displays.iter().min_by(|a, b| {
                a.input_frame
                    .distance_to(p)
                    .total_cmp(&b.input_frame.distance_to(p))
            })
        })
}

/// A full-display screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    pub display_index: usize,
    pub width_px: u32,
    pub height_px: u32,
}

/// An image actually sent to the model: a crop of a capture, resized.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SentImage {
    pub capture: Capture,
    /// Region of the capture this image shows, in capture pixels.
    pub source_px: Rect,
    pub width_px: u32,
    pub height_px: u32,
}

impl SentImage {
    /// A whole-capture image downscaled so its long edge is at most `max_edge`.
    pub fn full(capture: Capture, max_edge: u32) -> Self {
        let (w, h) = fit_within(capture.width_px, capture.height_px, max_edge);
        Self {
            capture,
            source_px: Rect::new(0.0, 0.0, capture.width_px as f64, capture.height_px as f64),
            width_px: w,
            height_px: h,
        }
    }

    /// A square-ish crop of `size_px` capture pixels centred on `center_px`,
    /// shifted (not shrunk) to stay inside the capture.
    pub fn crop_around(capture: Capture, center_px: Point, size_px: u32, max_edge: u32) -> Self {
        let cw = (size_px.min(capture.width_px)) as f64;
        let ch = (size_px.min(capture.height_px)) as f64;
        let x = (center_px.x - cw / 2.0).clamp(0.0, capture.width_px as f64 - cw);
        let y = (center_px.y - ch / 2.0).clamp(0.0, capture.height_px as f64 - ch);
        let (w, h) = fit_within(cw as u32, ch as u32, max_edge);
        Self {
            capture,
            source_px: Rect::new(x.round(), y.round(), cw, ch),
            width_px: w,
            height_px: h,
        }
    }
}

/// An OS input-space rect in capture pixels, clipped to the capture (None
/// when it does not overlap it). Used to black out regions before upload.
pub fn input_rect_to_capture_px(r: &Rect, display: &Display, capture: &Capture) -> Option<Rect> {
    let (vw, vh) = display.view_size();
    let sx = capture.width_px as f64 / vw;
    let sy = capture.height_px as f64 / vh;
    let tl = display.input_to_view(Point::new(r.x, r.y));
    let px = Rect::new(tl.x * sx, tl.y * sy, r.w / display.input_per_point * sx, r.h / display.input_per_point * sy);
    px.intersection(&Rect::new(0.0, 0.0, capture.width_px as f64, capture.height_px as f64))
}

/// Resize dimensions so the long edge is <= `max_edge`, preserving aspect.
pub fn fit_within(w: u32, h: u32, max_edge: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max_edge || long == 0 {
        return (w, h);
    }
    let s = max_edge as f64 / long as f64;
    (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1))
}

/// A box as emitted by the model: `[ymin, xmin, ymax, xmax]` in 0..=1000.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormBox {
    pub ymin: f64,
    pub xmin: f64,
    pub ymax: f64,
    pub xmax: f64,
}

impl NormBox {
    /// Validates and repairs model output: clamps to range, swaps reversed
    /// edges, and rejects degenerate (zero-area) or non-finite boxes.
    pub fn sanitized(v: [f64; 4]) -> Option<Self> {
        if v.iter().any(|c| !c.is_finite()) {
            return None;
        }
        let c = |n: f64| n.clamp(0.0, MODEL_NORM);
        let (y0, x0, y1, x1) = (c(v[0]), c(v[1]), c(v[2]), c(v[3]));
        let b = Self {
            ymin: y0.min(y1),
            xmin: x0.min(x1),
            ymax: y0.max(y1),
            xmax: x0.max(x1),
        };
        (b.ymax > b.ymin && b.xmax > b.xmin).then_some(b)
    }

    /// A model "point" (`[y, x]`) represented as a tiny box.
    pub fn from_point(y: f64, x: f64) -> Option<Self> {
        Self::sanitized([y - 1.0, x - 1.0, y + 1.0, x + 1.0])
    }
}

/// A rectangle on a specific display, in that display's view space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DisplayRect {
    pub display_index: usize,
    pub rect: Rect,
}

/// Model box on a sent image → rectangle in display view space.
pub fn norm_to_view(b: &NormBox, img: &SentImage, display: &Display) -> DisplayRect {
    // model space -> fraction of the sent image == fraction of its source region
    let fx0 = b.xmin / MODEL_NORM;
    let fy0 = b.ymin / MODEL_NORM;
    let fx1 = b.xmax / MODEL_NORM;
    let fy1 = b.ymax / MODEL_NORM;
    let s = img.source_px;
    // -> capture pixels
    let cx0 = s.x + fx0 * s.w;
    let cy0 = s.y + fy0 * s.h;
    let cx1 = s.x + fx1 * s.w;
    let cy1 = s.y + fy1 * s.h;
    // -> view points. Derive px-per-point from the capture itself rather than
    // trusting the reported scale factor: captures can be downscaled by the
    // OS, and the ratio is what actually matters.
    let (vw, vh) = display.view_size();
    let sx = vw / img.capture.width_px as f64;
    let sy = vh / img.capture.height_px as f64;
    DisplayRect {
        display_index: display.index,
        rect: Rect::new(cx0 * sx, cy0 * sy, (cx1 - cx0) * sx, (cy1 - cy0) * sy)
            .clamp_to(&display.view_bounds()),
    }
}

/// Display view space → model box on a sent image (used to remind the model
/// where things it already marked are). Returns `None` if off-image.
pub fn view_to_norm(r: &Rect, img: &SentImage, display: &Display) -> Option<NormBox> {
    let (vw, vh) = display.view_size();
    let px = img.capture.width_px as f64 / vw;
    let py = img.capture.height_px as f64 / vh;
    let s = img.source_px;
    let f = |v: f64, origin: f64, len: f64| (v - origin) / len * MODEL_NORM;
    let x0 = f(r.x * px, s.x, s.w);
    let x1 = f(r.right() * px, s.x, s.w);
    let y0 = f(r.y * py, s.y, s.h);
    let y1 = f(r.bottom() * py, s.y, s.h);
    if x1 <= 0.0 || y1 <= 0.0 || x0 >= MODEL_NORM || y0 >= MODEL_NORM {
        return None;
    }
    NormBox::sanitized([y0, x0, y1, x1])
}

/// View point on a display → pixel in a sent image (for marking the cursor).
pub fn view_to_image_px(p: Point, img: &SentImage, display: &Display) -> Option<Point> {
    let (vw, vh) = display.view_size();
    let cx = p.x * img.capture.width_px as f64 / vw;
    let cy = p.y * img.capture.height_px as f64 / vh;
    let s = img.source_px;
    if !s.contains(Point::new(cx, cy)) {
        return None;
    }
    Some(Point::new(
        (cx - s.x) / s.w * img.width_px as f64,
        (cy - s.y) / s.h * img.height_px as f64,
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn input_rects_map_to_capture_pixels() {
        // Windows: input is physical px at 1.5x; capture is the same pixels.
        let d = Display {
            index: 0,
            name: "w".into(),
            input_frame: Rect::new(-3000.0, 0.0, 3000.0, 2000.0),
            input_per_point: 1.5,
            scale_factor: 1.5,
            is_primary: false,
        };
        let cap = Capture { display_index: 0, width_px: 3000, height_px: 2000 };
        let r = input_rect_to_capture_px(&Rect::new(-2700.0, 150.0, 300.0, 60.0), &d, &cap).unwrap();
        assert!((r.x - 300.0).abs() < 1e-6 && (r.y - 150.0).abs() < 1e-6 && (r.w - 300.0).abs() < 1e-6);
        // partly off-display is clipped; fully off is None
        assert_eq!(input_rect_to_capture_px(&Rect::new(-3100.0, 0.0, 200.0, 10.0), &d, &cap).unwrap().w, 100.0);
        assert!(input_rect_to_capture_px(&Rect::new(10.0, 0.0, 50.0, 10.0), &d, &cap).is_none());
    }

    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn rect_approx(a: Rect, b: Rect) {
        assert!(
            approx(a.x, b.x) && approx(a.y, b.y) && approx(a.w, b.w) && approx(a.h, b.h),
            "{a:?} != {b:?}"
        );
    }

    /// MacBook Retina: 1440x900 points, 2880x1800 pixels, primary.
    fn mac_retina() -> Display {
        Display {
            index: 0,
            name: "Built-in".into(),
            input_frame: Rect::new(0.0, 0.0, 1440.0, 900.0),
            input_per_point: 1.0,
            scale_factor: 2.0,
            is_primary: true,
        }
    }

    /// External 1x monitor placed to the LEFT and slightly ABOVE the primary
    /// (negative coordinates are normal on macOS).
    fn mac_external_left() -> Display {
        Display {
            index: 1,
            name: "External".into(),
            input_frame: Rect::new(-1920.0, -180.0, 1920.0, 1080.0),
            input_per_point: 1.0,
            scale_factor: 1.0,
            is_primary: false,
        }
    }

    /// Windows 4K monitor at 150% to the right of a 1080p 100% primary.
    fn win_4k_150() -> Display {
        Display {
            index: 1,
            name: "4K".into(),
            input_frame: Rect::new(1920.0, 0.0, 3840.0, 2160.0),
            input_per_point: 1.5,
            scale_factor: 1.5,
            is_primary: false,
        }
    }

    /// A portrait (rotated) 1080x1920 monitor. Both OSes report rotated
    /// displays with swapped dimensions and capture them upright.
    fn portrait() -> Display {
        Display {
            index: 2,
            name: "Portrait".into(),
            input_frame: Rect::new(1440.0, -500.0, 1080.0, 1920.0),
            input_per_point: 1.0,
            scale_factor: 1.0,
            is_primary: false,
        }
    }

    #[test]
    fn full_image_box_maps_to_retina_points() {
        let d = mac_retina();
        let cap = Capture { display_index: 0, width_px: 2880, height_px: 1800 };
        let img = SentImage::full(cap, 1920);
        assert_eq!((img.width_px, img.height_px), (1920, 1200));
        let b = NormBox::sanitized([100.0, 250.0, 200.0, 500.0]).unwrap();
        let r = norm_to_view(&b, &img, &d);
        assert_eq!(r.display_index, 0);
        rect_approx(r.rect, Rect::new(360.0, 90.0, 360.0, 90.0));
    }

    #[test]
    fn crop_box_maps_back_through_offset() {
        let d = mac_retina();
        let cap = Capture { display_index: 0, width_px: 2880, height_px: 1800 };
        // 800px crop centred at capture pixel (1000, 1000)
        let img = SentImage::crop_around(cap, Point::new(1000.0, 1000.0), 800, 800);
        assert_eq!(img.source_px, Rect::new(600.0, 600.0, 800.0, 800.0));
        // the whole crop
        let b = NormBox::sanitized([0.0, 0.0, 1000.0, 1000.0]).unwrap();
        let r = norm_to_view(&b, &img, &d);
        rect_approx(r.rect, Rect::new(300.0, 300.0, 400.0, 400.0));
    }

    #[test]
    fn crop_is_shifted_inside_capture_at_edges() {
        let cap = Capture { display_index: 0, width_px: 2880, height_px: 1800 };
        let img = SentImage::crop_around(cap, Point::new(2870.0, 5.0), 800, 800);
        assert_eq!(img.source_px, Rect::new(2080.0, 0.0, 800.0, 800.0));
    }

    #[test]
    fn windows_scaled_display_uses_view_points() {
        let d = win_4k_150();
        assert_eq!(d.view_size(), (2560.0, 1440.0));
        let cap = Capture { display_index: 1, width_px: 3840, height_px: 2160 };
        let img = SentImage::full(cap, 1920);
        let b = NormBox::sanitized([500.0, 500.0, 600.0, 600.0]).unwrap();
        let r = norm_to_view(&b, &img, &d);
        rect_approx(r.rect, Rect::new(1280.0, 720.0, 256.0, 144.0));
        // and the centre converts to the right physical input coordinate
        let input = d.view_to_input(r.rect.center());
        assert!(approx(input.x, 1920.0 + 1408.0 * 1.5));
        assert!(approx(input.y, 792.0 * 1.5));
    }

    #[test]
    fn os_downscaled_capture_still_maps_correctly() {
        // Retina display captured at 1x (e.g. capture API returned points)
        let d = mac_retina();
        let cap = Capture { display_index: 0, width_px: 1440, height_px: 900 };
        let img = SentImage::full(cap, 1920);
        let b = NormBox::sanitized([0.0, 0.0, 500.0, 500.0]).unwrap();
        rect_approx(norm_to_view(&b, &img, &d).rect, Rect::new(0.0, 0.0, 720.0, 450.0));
    }

    #[test]
    fn portrait_display() {
        let d = portrait();
        let cap = Capture { display_index: 2, width_px: 1080, height_px: 1920 };
        let img = SentImage::full(cap, 1536);
        assert_eq!((img.width_px, img.height_px), (864, 1536));
        let b = NormBox::sanitized([900.0, 0.0, 1000.0, 1000.0]).unwrap();
        rect_approx(norm_to_view(&b, &img, &d).rect, Rect::new(0.0, 1728.0, 1080.0, 192.0));
    }

    #[test]
    fn display_lookup_with_negative_coordinates() {
        let ds = vec![mac_retina(), mac_external_left(), portrait()];
        assert_eq!(display_at(&ds, Point::new(100.0, 100.0)).unwrap().index, 0);
        assert_eq!(display_at(&ds, Point::new(-10.0, -100.0)).unwrap().index, 1);
        assert_eq!(display_at(&ds, Point::new(1500.0, -400.0)).unwrap().index, 2);
        // shared edge belongs to exactly one display
        assert_eq!(display_at(&ds, Point::new(0.0, 10.0)).unwrap().index, 0);
        assert_eq!(display_at(&ds, Point::new(-0.5, 10.0)).unwrap().index, 1);
        // off every display -> nearest
        assert_eq!(display_at(&ds, Point::new(500.0, 1000.0)).unwrap().index, 0);
        let p = mac_external_left().input_to_view(Point::new(-1920.0, -180.0));
        assert_eq!(p, Point::new(0.0, 0.0));
    }

    #[test]
    fn sanitize_repairs_and_rejects() {
        let b = NormBox::sanitized([600.0, 700.0, 100.0, -5.0]).unwrap();
        assert_eq!(b, NormBox { ymin: 100.0, xmin: 0.0, ymax: 600.0, xmax: 700.0 });
        assert!(NormBox::sanitized([10.0, 10.0, 10.0, 50.0]).is_none());
        assert!(NormBox::sanitized([f64::NAN, 0.0, 10.0, 10.0]).is_none());
        let b = NormBox::sanitized([0.0, 0.0, 2000.0, 1500.0]).unwrap();
        assert_eq!((b.ymax, b.xmax), (1000.0, 1000.0));
    }

    #[test]
    fn round_trip_view_norm_view() {
        let d = win_4k_150();
        let cap = Capture { display_index: 1, width_px: 3840, height_px: 2160 };
        for img in [
            SentImage::full(cap, 1920),
            SentImage::crop_around(cap, Point::new(1900.0, 1100.0), 900, 768),
        ] {
            let r = Rect::new(1200.0, 700.0, 150.0, 60.0);
            let n = view_to_norm(&r, &img, &d).unwrap();
            rect_approx(norm_to_view(&n, &img, &d).rect, r);
        }
        // a rect outside a crop has no representation in it
        let img = SentImage::crop_around(cap, Point::new(500.0, 500.0), 400, 400);
        assert!(view_to_norm(&Rect::new(2000.0, 1000.0, 10.0, 10.0), &img, &d).is_none());
    }

    #[test]
    fn cursor_marker_position_in_image() {
        let d = mac_retina();
        let cap = Capture { display_index: 0, width_px: 2880, height_px: 1800 };
        let img = SentImage::full(cap, 1440);
        let p = view_to_image_px(Point::new(720.0, 450.0), &img, &d).unwrap();
        assert!(approx(p.x, 720.0) && approx(p.y, 450.0));
    }

    #[test]
    fn iou_and_distance() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 0.0, 10.0, 10.0);
        assert!(approx(a.iou(&b), 50.0 / 150.0));
        assert_eq!(a.distance_to(Point::new(13.0, 14.0)), 5.0);
        assert_eq!(a.distance_to(Point::new(3.0, 3.0)), 0.0);
    }
}
