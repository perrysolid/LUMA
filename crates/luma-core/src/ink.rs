//! Ink snapping: put LUMA's freehand strokes exactly on the lines that are
//! already drawn on screen (a triangle in a video, a curve on a chart, a
//! connector on a whiteboard).
//!
//! The model's points land near a line, not on it. For each point we look at
//! a small window of the screenshot, estimate the local background colour,
//! find the nearest "ink" pixel (clearly different from that background) and
//! move the point to the centre of the stroke there. Gaps between points are
//! filled in and snapped too, so a trace follows the real line instead of
//! cutting straight across. Points with no ink nearby stay where they are.

/// An RGBA image, row-major.
pub struct Rgba<'a> {
    pub width: u32,
    pub height: u32,
    pub data: &'a [u8],
}

impl Rgba<'_> {
    fn at(&self, x: i64, y: i64) -> Option<[u8; 3]> {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return None;
        }
        let i = ((y as u64 * self.width as u64 + x as u64) * 4) as usize;
        Some([self.data[i], self.data[i + 1], self.data[i + 2]])
    }
}

/// Colour distance at which a pixel counts as ink against the background.
const INK_CONTRAST: i32 = 70;

fn median(v: &mut [u8]) -> u8 {
    if v.is_empty() {
        return 0;
    }
    let mid = v.len() / 2;
    *v.select_nth_unstable(mid).1
}

/// Snap one point (pixels) onto the nearest stroke within `radius`.
pub fn snap_point(img: &Rgba, p: (f64, f64), radius: f64) -> Option<(f64, f64)> {
    let r = radius.ceil() as i64;
    let (cx, cy) = (p.0.round() as i64, p.1.round() as i64);
    // background: per-channel median over a wider window (most of it is background)
    let br = r * 2;
    let (mut rs, mut gs, mut bs) = (Vec::new(), Vec::new(), Vec::new());
    let mut y = cy - br;
    while y <= cy + br {
        let mut x = cx - br;
        while x <= cx + br {
            if let Some(c) = img.at(x, y) {
                rs.push(c[0]);
                gs.push(c[1]);
                bs.push(c[2]);
            }
            x += 3;
        }
        y += 3;
    }
    let bg = [median(&mut rs) as i32, median(&mut gs) as i32, median(&mut bs) as i32];
    let ink = |x: i64, y: i64| {
        img.at(x, y).is_some_and(|c| (c[0] as i32 - bg[0]).abs() + (c[1] as i32 - bg[1]).abs() + (c[2] as i32 - bg[2]).abs() > INK_CONTRAST)
    };
    // nearest ink pixel
    let mut best: Option<(i64, i64, i64)> = None;
    for y in cy - r..=cy + r {
        for x in cx - r..=cx + r {
            let d2 = (x - cx).pow(2) + (y - cy).pow(2);
            if d2 > r * r || !ink(x, y) {
                continue;
            }
            if best.is_none_or(|(_, _, b)| d2 < b) {
                best = Some((x, y, d2));
            }
        }
    }
    let (bx, by, _) = best?;
    // centre of the stroke around it
    let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
    for y in by - 3..=by + 3 {
        for x in bx - 3..=bx + 3 {
            if ink(x, y) {
                sx += x as f64;
                sy += y as f64;
                n += 1.0;
            }
        }
    }
    Some((sx / n, sy / n))
}

/// A segment is a trace of an existing line when at least this share of
/// the points along it find ink close by.
const TRACE_SHARE: f64 = 0.7;

/// Snap a whole stroke, but only where it traces something already drawn.
/// Each segment between the model's points is tested: if ink runs along it,
/// its end points and filled-in points move onto the line; if not (the model
/// is drawing something new, like a square or a loop around a word), the
/// segment stays a clean straight stroke and its corners are not moved.
/// `radius` is the search distance for the model's own points; filled-in
/// points use a tighter one so they do not jump to nearby text.
pub fn snap_path(img: &Rgba, pts: &[(f64, f64)], radius: f64, step: f64) -> Vec<(f64, f64)> {
    if pts.len() < 2 {
        return pts.to_vec();
    }
    let tight = (radius * 0.45).max(4.0);
    let snapped: Vec<Option<(f64, f64)>> = pts.iter().map(|&p| snap_point(img, p, radius)).collect();
    // per segment: filled-in snapped points if it is a trace
    let mut traced: Vec<Option<Vec<(f64, f64)>>> = Vec::with_capacity(pts.len() - 1);
    for i in 0..pts.len() - 1 {
        let (a, b) = (snapped[i].unwrap_or(pts[i]), snapped[i + 1].unwrap_or(pts[i + 1]));
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let n = ((len / step).floor() as usize).max(2);
        let mut found = Vec::new();
        let mut hits = 0;
        for k in 1..n {
            let t = k as f64 / n as f64;
            let q = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            if let Some(s) = snap_point(img, q, tight) {
                hits += 1;
                found.push(s);
            }
        }
        let is_trace = snapped[i].is_some() && snapped[i + 1].is_some() && hits as f64 >= TRACE_SHARE * (n - 1) as f64;
        traced.push(is_trace.then_some(found));
    }
    let mut out = Vec::with_capacity(pts.len() * 4);
    for i in 0..pts.len() {
        // a point moves onto the ink only if a segment it belongs to is a trace
        let touches_trace = (i > 0 && traced[i - 1].is_some()) || traced.get(i).is_some_and(|t| t.is_some());
        out.push(if touches_trace { snapped[i].unwrap_or(pts[i]) } else { pts[i] });
        if let Some(Some(fill)) = traced.get(i) {
            out.extend(fill.iter().copied());
        }
    }
    out
}

/// Where the screen is busy: a grid of `cell`-pixel squares, each true
/// when it has visible detail (text, lines, edges), false when it is a flat
/// colour (a page margin, an empty video background).
pub struct BusyGrid {
    pub cols: usize,
    pub rows: usize,
    pub cell: u32,
    /// Summed-area table over busy cells, (cols+1) × (rows+1).
    sat: Vec<u32>,
}

impl BusyGrid {
    pub fn new(img: &Rgba, cell: u32) -> Self {
        let cols = (img.width / cell).max(1) as usize;
        let rows = (img.height / cell).max(1) as usize;
        let mut sat = vec![0u32; (cols + 1) * (rows + 1)];
        for r in 0..rows {
            for c in 0..cols {
                let (mut lo, mut hi) = (255u8, 0u8);
                let step = (cell / 6).max(1);
                let mut y = r as u32 * cell;
                while y < (r as u32 + 1) * cell {
                    let mut x = c as u32 * cell;
                    while x < (c as u32 + 1) * cell {
                        if let Some(p) = img.at(x as i64, y as i64) {
                            let l = ((p[0] as u32 * 3 + p[1] as u32 * 6 + p[2] as u32) / 10) as u8;
                            lo = lo.min(l);
                            hi = hi.max(l);
                        }
                        x += step;
                    }
                    y += step;
                }
                let busy = (hi.saturating_sub(lo) > 40) as u32;
                sat[(r + 1) * (cols + 1) + c + 1] =
                    busy + sat[r * (cols + 1) + c + 1] + sat[(r + 1) * (cols + 1) + c] - sat[r * (cols + 1) + c];
            }
        }
        Self { cols, rows, cell, sat }
    }

    /// Fraction of busy cells in a pixel rect.
    pub fn busy(&self, x: f64, y: f64, w: f64, h: f64) -> f64 {
        let c0 = ((x / self.cell as f64).floor().max(0.0) as usize).min(self.cols);
        let r0 = ((y / self.cell as f64).floor().max(0.0) as usize).min(self.rows);
        let c1 = (((x + w) / self.cell as f64).ceil().max(0.0) as usize).min(self.cols);
        let r1 = (((y + h) / self.cell as f64).ceil().max(0.0) as usize).min(self.rows);
        if c1 <= c0 || r1 <= r0 {
            return 0.0;
        }
        let k = self.cols + 1;
        let n = self.sat[r1 * k + c1] + self.sat[r0 * k + c0] - self.sat[r0 * k + c1] - self.sat[r1 * k + c0];
        n as f64 / ((c1 - c0) * (r1 - r0)) as f64
    }

    /// The least busy place for a `w`×`h` pixel rect, preferring places near
    /// `near`: (x, y, busy fraction).
    pub fn emptiest(&self, w: f64, h: f64, near: (f64, f64)) -> Option<(f64, f64, f64)> {
        let (cw, ch) = (self.cols as f64 * self.cell as f64, self.rows as f64 * self.cell as f64);
        if w > cw || h > ch {
            return None;
        }
        let step = self.cell as f64;
        let mut best: Option<(f64, f64, f64, f64)> = None;
        let mut y = 0.0;
        while y + h <= ch {
            let mut x = 0.0;
            while x + w <= cw {
                let b = self.busy(x, y, w, h);
                let dist = ((x - near.0).powi(2) + (y - near.1).powi(2)).sqrt() / (cw.max(ch));
                let score = b + dist * 0.05;
                if best.is_none_or(|(s, ..)| score < s) {
                    best = Some((score, x, y, b));
                }
                x += step;
            }
            y += step;
        }
        best.map(|(_, x, y, b)| (x, y, b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A black image with an orange line from (20, 180) to (280, 40), 3 px wide,
    /// and some green "text" blobs above it.
    fn picture() -> (u32, u32, Vec<u8>) {
        let (w, h) = (300u32, 220u32);
        let mut d = vec![0u8; (w * h * 4) as usize];
        let mut put = |x: i64, y: i64, c: [u8; 3]| {
            if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                let i = ((y as u32 * w + x as u32) * 4) as usize;
                d[i..i + 3].copy_from_slice(&c);
                d[i + 3] = 255;
            }
        };
        for t in 0..=1000 {
            let t = t as f64 / 1000.0;
            let (x, y) = (20.0 + 260.0 * t, 180.0 - 140.0 * t);
            for o in -1..=1 {
                put(x.round() as i64, y.round() as i64 + o, [230, 90, 60]);
            }
        }
        for x in 60..90 {
            for y in 60..70 {
                put(x, y, [120, 220, 90]);
            }
        }
        (w, h, d)
    }

    fn dist_to_line(p: (f64, f64)) -> f64 {
        // line through (20,180)-(280,40)
        let (x1, y1, x2, y2) = (20.0, 180.0, 280.0, 40.0);
        ((y2 - y1) * p.0 - (x2 - x1) * p.1 + x2 * y1 - y2 * x1).abs() / ((y2 - y1).powi(2) + (x2 - x1).powi(2)).sqrt()
    }

    #[test]
    fn points_near_a_line_land_on_it() {
        let (w, h, d) = picture();
        let img = Rgba { width: w, height: h, data: &d };
        // 9 px off the line, perpendicular-ish
        let s = snap_point(&img, (150.0, 100.0), 14.0).unwrap();
        assert!(dist_to_line(s) < 1.5, "{s:?} is {} px off", dist_to_line(s));
        // nothing within reach: no snap
        assert!(snap_point(&img, (280.0, 200.0), 10.0).is_none());
    }

    #[test]
    fn a_rough_two_point_trace_follows_the_real_line() {
        let (w, h, d) = picture();
        let img = Rgba { width: w, height: h, data: &d };
        let path = snap_path(&img, &[(26.0, 168.0), (270.0, 52.0)], 16.0, 20.0);
        assert!(path.len() > 8, "gaps are filled in");
        for p in &path {
            assert!(dist_to_line(*p) < 2.0, "{p:?}");
        }
    }

    #[test]
    fn finds_the_empty_part_of_the_screen() {
        // 400×300 black; a busy striped block on the left half
        let (w, h) = (400u32, 300u32);
        let mut d = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..200 {
                if (x / 4 + y / 4) % 2 == 0 {
                    let i = ((y * w + x) * 4) as usize;
                    d[i..i + 3].copy_from_slice(&[240, 240, 240]);
                }
            }
        }
        let img = Rgba { width: w, height: h, data: &d };
        let g = BusyGrid::new(&img, 16);
        assert!(g.busy(0.0, 0.0, 190.0, 290.0) > 0.9);
        assert!(g.busy(210.0, 0.0, 180.0, 290.0) < 0.05);
        let (x, _, b) = g.emptiest(160.0, 160.0, (0.0, 0.0)).unwrap();
        assert!(x >= 200.0 && b < 0.05, "x {x} busy {b}");
    }

    #[test]
    fn new_shapes_away_from_ink_are_left_alone() {
        let (w, h, d) = picture();
        let img = Rgba { width: w, height: h, data: &d };
        // a square drawn in empty space near (but not along) the line and the text blob
        let square = [(200.0, 150.0), (260.0, 150.0), (260.0, 210.0), (200.0, 210.0), (200.0, 150.0)];
        assert_eq!(snap_path(&img, &square, 16.0, 20.0), square.to_vec());
    }

    #[test]
    fn works_on_light_backgrounds_too() {
        let (w, h) = (100u32, 100u32);
        let mut d = vec![255u8; (w * h * 4) as usize];
        for y in 0..h {
            let i = ((y * w + 50) * 4) as usize;
            d[i..i + 3].copy_from_slice(&[20, 20, 20]);
        }
        let img = Rgba { width: w, height: h, data: &d };
        let s = snap_point(&img, (44.0, 30.0), 10.0).unwrap();
        assert!((s.0 - 50.0).abs() < 0.5);
    }
}
