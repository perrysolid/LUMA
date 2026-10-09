// Pure layout math for the overlay. No DOM access, fully unit tested.

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}
export interface Pt {
  x: number;
  y: number;
}

export const center = (r: Rect): Pt => ({ x: r.x + r.w / 2, y: r.y + r.h / 2 });

export function inflate(r: Rect, d: number): Rect {
  return { x: r.x - d, y: r.y - d, w: r.w + 2 * d, h: r.h + 2 * d };
}

export function overlapArea(a: Rect, b: Rect): number {
  const w = Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x);
  const h = Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y);
  return w > 0 && h > 0 ? w * h : 0;
}

/** Point where the ray from the rect centre towards `toward` leaves the rect. */
export function edgePoint(r: Rect, toward: Pt, pad = 0): Pt {
  const c = center(r);
  const dx = toward.x - c.x;
  const dy = toward.y - c.y;
  if (dx === 0 && dy === 0) return c;
  const hw = r.w / 2 + pad;
  const hh = r.h / 2 + pad;
  const t = Math.min(dx !== 0 ? hw / Math.abs(dx) : Infinity, dy !== 0 ? hh / Math.abs(dy) : Infinity);
  return { x: c.x + dx * t, y: c.y + dy * t };
}

export interface ArrowPath {
  start: Pt;
  end: Pt;
  control: Pt;
  mid: Pt;
  /** angle of the tangent at `end`, radians */
  endAngle: number;
}

/**
 * Gently curved connector between two rects, leaving and entering at their
 * borders. The bend keeps it from sitting exactly on top of a straight
 * connector that is already drawn in the diagram.
 */
export function arrowBetween(from: Rect, to: Rect, gap = 6, bend = 0.18): ArrowPath {
  const cf = center(from);
  const ct = center(to);
  const start = edgePoint(from, ct, gap);
  const end = edgePoint(to, cf, gap);
  const dx = end.x - start.x;
  const dy = end.y - start.y;
  const len = Math.hypot(dx, dy) || 1;
  const nx = -dy / len;
  const ny = dx / len;
  const control = { x: (start.x + end.x) / 2 + nx * len * bend, y: (start.y + end.y) / 2 + ny * len * bend };
  // quadratic bezier midpoint (t = .5)
  const mid = {
    x: 0.25 * start.x + 0.5 * control.x + 0.25 * end.x,
    y: 0.25 * start.y + 0.5 * control.y + 0.25 * end.y,
  };
  const endAngle = Math.atan2(end.y - control.y, end.x - control.x);
  return { start, end, control, mid, endAngle };
}

export type Side = "top" | "bottom" | "right" | "left" | "inside";

export interface Placement {
  rect: Rect;
  side: Side;
}

/**
 * Place a label of size `size` next to `target`, inside `viewport`, avoiding
 * `obstacles` (other labels, other marked shapes). Tries the conventional
 * sides first and picks the lowest-cost candidate.
 */
export function placeLabel(
  target: Rect,
  size: { w: number; h: number },
  viewport: Rect,
  obstacles: Rect[],
  gap = 8,
): Placement {
  const cx = target.x + target.w / 2 - size.w / 2;
  const cy = target.y + target.h / 2 - size.h / 2;
  const candidates: Placement[] = [
    { side: "top", rect: { x: target.x, y: target.y - gap - size.h, ...size } },
    { side: "bottom", rect: { x: target.x, y: target.y + target.h + gap, ...size } },
    { side: "right", rect: { x: target.x + target.w + gap, y: cy, ...size } },
    { side: "left", rect: { x: target.x - gap - size.w, y: cy, ...size } },
    { side: "top", rect: { x: cx, y: target.y - gap - size.h, ...size } },
    { side: "bottom", rect: { x: cx, y: target.y + target.h + gap, ...size } },
  ];
  let best: Placement | null = null;
  let bestCost = Infinity;
  candidates.forEach((c, order) => {
    const shifted = clampInto(c.rect, viewport);
    const shift = Math.abs(shifted.x - c.rect.x) + Math.abs(shifted.y - c.rect.y);
    let cost = order * 0.5 + shift * 2;
    // covering the target defeats the purpose of labelling it
    cost += overlapArea(shifted, target) * 4;
    for (const o of obstacles) cost += overlapArea(shifted, inflate(o, 2)) * 3;
    if (cost < bestCost) {
      bestCost = cost;
      best = { side: c.side, rect: shifted };
    }
  });
  // Nothing outside fits without heavy overlap: put it inside the target's top-left.
  if (best === null || bestCost > size.w * size.h * 2) {
    return { side: "inside", rect: clampInto({ x: target.x + 4, y: target.y + 4, ...size }, viewport) };
  }
  return best;
}

export function clampInto(r: Rect, vp: Rect, margin = 6): Rect {
  const x = Math.min(Math.max(r.x, vp.x + margin), vp.x + vp.w - r.w - margin);
  const y = Math.min(Math.max(r.y, vp.y + margin), vp.y + vp.h - r.h - margin);
  return { x: Math.max(x, vp.x), y: Math.max(y, vp.y), w: r.w, h: r.h };
}

/** Rounded rect path with the hole for a spotlight (even-odd fill). */
export function spotlightPath(vp: Rect, hole: Rect, radius = 10): string {
  const r = Math.min(radius, hole.w / 2, hole.h / 2);
  const { x, y, w, h } = hole;
  return (
    `M${vp.x},${vp.y}H${vp.x + vp.w}V${vp.y + vp.h}H${vp.x}Z ` +
    `M${x + r},${y}H${x + w - r}A${r},${r} 0 0 1 ${x + w},${y + r}V${y + h - r}` +
    `A${r},${r} 0 0 1 ${x + w - r},${y + h}H${x + r}A${r},${r} 0 0 1 ${x},${y + h - r}` +
    `V${y + r}A${r},${r} 0 0 1 ${x + r},${y}Z`
  );
}

/** Small targets get a minimum visual size so a box is never a speck. */
export function minVisible(r: Rect, min = 18): Rect {
  const w = Math.max(r.w, min);
  const h = Math.max(r.h, min);
  return { x: r.x - (w - r.w) / 2, y: r.y - (h - r.h) / 2, w, h };
}

/**
 * A smooth SVG path through `pts` (Catmull-Rom → cubic Béziers), for
 * freehand sketch strokes. `closed` joins the end back to the start.
 */
export function smoothPath(pts: Pt[], closed = false): string {
  if (pts.length === 0) return "";
  if (pts.length === 1) return `M${pts[0].x},${pts[0].y}`;
  if (pts.length === 2 && !closed) return `M${pts[0].x},${pts[0].y}L${pts[1].x},${pts[1].y}`;
  const n = pts.length;
  const at = (i: number) => (closed ? pts[(i + n) % n] : pts[Math.max(0, Math.min(n - 1, i))]);
  const r = (v: number) => Math.round(v * 10) / 10;
  let d = `M${r(pts[0].x)},${r(pts[0].y)}`;
  const segs = closed ? n : n - 1;
  for (let i = 0; i < segs; i++) {
    const p0 = at(i - 1), p1 = at(i), p2 = at(i + 1), p3 = at(i + 2);
    const c1 = { x: p1.x + (p2.x - p0.x) / 6, y: p1.y + (p2.y - p0.y) / 6 };
    const c2 = { x: p2.x - (p3.x - p1.x) / 6, y: p2.y - (p3.y - p1.y) / 6 };
    d += `C${r(c1.x)},${r(c1.y)} ${r(c2.x)},${r(c2.y)} ${r(p2.x)},${r(p2.y)}`;
  }
  return closed ? d + "Z" : d;
}

/** Bounding box of points. */
export function boundsOf(pts: Pt[]): Rect {
  const xs = pts.map((p) => p.x);
  const ys = pts.map((p) => p.y);
  const x = Math.min(...xs), y = Math.min(...ys);
  return { x, y, w: Math.max(1, Math.max(...xs) - x), h: Math.max(1, Math.max(...ys) - y) };
}
