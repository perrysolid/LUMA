// The annotation scene: turns Annotation commands into animated SVG/HTML.
// Independent of Tauri so it can be driven by the demo page and tests.

import {
  arrowBetween,
  center,
  inflate,
  minVisible,
  overlapArea,
  placeLabel,
  spotlightPath,
  type Pt,
  type Rect,
} from "./geometry";

export type Annotation =
  | { op: "shape"; display: number; id: string; kind: "box" | "circle" | "highlight" | "underline"; rect: Rect; label?: string | null }
  | { op: "point"; display: number; id: string; rect: Rect; label?: string | null }
  | { op: "arrow"; display: number; id: string; from: Rect; to: Rect; label?: string | null }
  | { op: "step"; display: number; id: string; n: number; rect: Rect; label?: string | null }
  | { op: "spotlight"; display: number; rect: Rect }
  | { op: "zoom"; display: number; rect: Rect }
  | { op: "focus"; display: number; id: string; rect: Rect }
  | { op: "label"; display: number; id: string; rect: Rect; text: string }
  | { op: "clear"; id?: string | null };

const NS = "http://www.w3.org/2000/svg";
const reduceMotion = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

interface Item {
  id: string;
  rect: Rect;
  nodes: Element[];
  labelRect?: Rect;
}

function svg<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number>, parent?: Element) {
  const el = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, String(v));
  parent?.appendChild(el);
  return el;
}

function animate(el: Element, frames: Keyframe[], opts: KeyframeAnimationOptions) {
  if (reduceMotion() || !(el as HTMLElement).animate) return;
  (el as HTMLElement).animate(frames, { fill: "both", ...opts });
}

function drawOn(el: SVGGeometryElement, ms = 420, delay = 0) {
  if (reduceMotion() || !el.getTotalLength) return;
  let len = 0;
  try {
    len = el.getTotalLength();
  } catch {
    return;
  }
  el.style.strokeDasharray = `${len}`;
  animate(el, [{ strokeDashoffset: len }, { strokeDashoffset: 0 }], { duration: ms, delay, easing: "cubic-bezier(.3,.7,.2,1)" });
}

function roundedRectPath(r: Rect, rad = 8): string {
  const k = Math.min(rad, r.w / 2, r.h / 2);
  const { x, y, w, h } = r;
  // start top-left so draw-on traces clockwise from the corner
  return `M${x + k},${y}H${x + w - k}A${k},${k} 0 0 1 ${x + w},${y + k}V${y + h - k}A${k},${k} 0 0 1 ${x + w - k},${y + h}H${x + k}A${k},${k} 0 0 1 ${x},${y + h - k}V${y + k}A${k},${k} 0 0 1 ${x + k},${y}Z`;
}

function ellipsePath(r: Rect): string {
  const c = center(r);
  const rx = r.w / 2;
  const ry = r.h / 2;
  return `M${c.x},${c.y - ry}A${rx},${ry} 0 1 1 ${c.x - 0.01},${c.y - ry}Z`;
}

export class Scene {
  private items = new Map<string, Item>();
  private spot: SVGPathElement | null = null;
  private pointerPos: Pt | null = null;
  private pointerAnim = 0;

  constructor(
    private layers: { spot: SVGGElement; shapes: SVGGElement; arrows: SVGGElement; steps: SVGGElement },
    private labels: HTMLElement,
    private pointer: HTMLElement,
    private viewport: () => Rect = () => ({ x: 0, y: 0, w: window.innerWidth, h: window.innerHeight }),
  ) {}

  /** Ids currently drawn (for tests / debugging). */
  ids(): string[] {
    return [...this.items.keys()];
  }

  apply(a: Annotation) {
    switch (a.op) {
      case "shape":
        return this.shape(a.id, a.kind, a.rect, a.label ?? undefined);
      case "point":
        return this.point(a.id, a.rect, a.label ?? undefined);
      case "arrow":
        return this.arrow(a.id, a.from, a.to, a.label ?? undefined);
      case "step":
        return this.step(a.id, a.n, a.rect, a.label ?? undefined);
      case "spotlight":
        return this.spotlight(a.rect, false);
      case "zoom":
        return this.spotlight(a.rect, true);
      case "focus":
        return this.focus(a.id, a.rect);
      case "label":
        return this.callout(a.id, a.rect, a.text);
      case "clear":
        return a.id ? this.remove(a.id) : this.clear();
    }
  }

  clear() {
    const nodes = [...this.items.values()].flatMap((i) => i.nodes);
    if (this.spot) nodes.push(this.spot);
    this.items.clear();
    this.spot = null;
    this.pointer.hidden = true;
    this.pointerPos = null;
    for (const n of nodes) this.fadeRemove(n);
  }

  remove(id: string) {
    const it = this.items.get(id);
    if (!it) return;
    this.items.delete(id);
    it.nodes.forEach((n) => this.fadeRemove(n));
  }

  private fadeRemove(n: Element) {
    if (reduceMotion() || !(n as HTMLElement).animate) return n.remove();
    const anim = (n as HTMLElement).animate([{ opacity: 1 }, { opacity: 0 }], { duration: 180, fill: "forwards" });
    anim.onfinish = () => n.remove();
  }

  private put(id: string, rect: Rect, nodes: Element[], labelRect?: Rect) {
    this.remove(id);
    this.items.set(id, { id, rect, nodes, labelRect });
  }

  private obstacles(exceptId?: string): Rect[] {
    const out: Rect[] = [];
    for (const it of this.items.values()) {
      if (it.id === exceptId) continue;
      out.push(it.rect);
      if (it.labelRect) out.push(it.labelRect);
    }
    return out;
  }

  private label(text: string, target: Rect, ownerId: string, callout = false): { el: HTMLElement; rect: Rect } {
    const el = document.createElement("div");
    el.className = callout ? "label callout" : "label";
    el.textContent = text;
    el.style.visibility = "hidden";
    this.labels.appendChild(el);
    const size = { w: el.offsetWidth || text.length * 8 + 20, h: el.offsetHeight || 26 };
    const p = placeLabel(target, size, this.viewport(), this.obstacles(ownerId));
    el.style.left = `${p.rect.x}px`;
    el.style.top = `${p.rect.y}px`;
    el.style.visibility = "";
    const dy = p.side === "top" ? 6 : p.side === "bottom" ? -6 : 0;
    const dx = p.side === "left" ? 6 : p.side === "right" ? -6 : 0;
    animate(el, [{ opacity: 0, transform: `translate(${dx}px,${dy}px)` }, { opacity: 1, transform: "none" }], {
      duration: 220,
      delay: 160,
      easing: "ease-out",
    });
    return { el, rect: p.rect };
  }

  private shape(id: string, kind: "box" | "circle" | "highlight" | "underline", raw: Rect, text?: string) {
    const r = minVisible(raw);
    const g = svg("g", { "data-id": id }, this.layers.shapes);
    if (kind === "highlight") {
      const m = svg("rect", { class: "mark", x: r.x - 2, y: r.y - 1, width: r.w + 4, height: r.h + 2, rx: 3 }, g);
      animate(m, [{ transform: "scaleX(0)" }, { transform: "scaleX(1)" }], { duration: 320, easing: "ease-out" });
      (m as SVGElement).style.transformOrigin = `${r.x}px ${r.y}px`;
      (m as SVGElement).style.transformBox = "view-box";
    } else if (kind === "underline") {
      const y = r.y + r.h + 3;
      const d = `M${r.x},${y}H${r.x + r.w}`;
      svg("path", { class: "halo", d }, g);
      drawOn(svg("path", { class: "stroke", d }, g), 360);
    } else {
      const outline = kind === "circle" ? inflate(r, Math.max(6, Math.min(r.w, r.h) * 0.12)) : inflate(r, 4);
      const d = kind === "circle" ? ellipsePath(outline) : roundedRectPath(outline);
      svg("path", { class: "fill-soft", d }, g);
      const halo = svg("path", { class: "halo", d }, g);
      const stroke = svg("path", { class: "stroke", d }, g);
      drawOn(halo, 460);
      drawOn(stroke, 460);
    }
    const nodes: Element[] = [g];
    let labelRect: Rect | undefined;
    if (text) {
      const l = this.label(text, inflate(r, 4), id);
      nodes.push(l.el);
      labelRect = l.rect;
    }
    this.put(id, raw, nodes, labelRect);
  }

  private arrow(id: string, from: Rect, to: Rect, text?: string) {
    const a = arrowBetween(inflate(from, 4), inflate(to, 4));
    const g = svg("g", { "data-id": id }, this.layers.arrows);
    const d = `M${a.start.x},${a.start.y}Q${a.control.x},${a.control.y} ${a.end.x},${a.end.y}`;
    drawOn(svg("path", { class: "halo", d }, g), 480);
    drawOn(svg("path", { class: "stroke", d }, g), 480);
    const s = 13;
    const ang = a.endAngle;
    const p1 = { x: a.end.x - s * Math.cos(ang - 0.45), y: a.end.y - s * Math.sin(ang - 0.45) };
    const p2 = { x: a.end.x - s * Math.cos(ang + 0.45), y: a.end.y - s * Math.sin(ang + 0.45) };
    const head = svg("polygon", { class: "arrowhead", points: `${a.end.x},${a.end.y} ${p1.x},${p1.y} ${p2.x},${p2.y}` }, g);
    animate(head, [{ opacity: 0 }, { opacity: 1 }], { duration: 120, delay: 420 });
    const nodes: Element[] = [g];
    let labelRect: Rect | undefined;
    if (text) {
      const l = this.label(text, { x: a.mid.x - 1, y: a.mid.y - 1, w: 2, h: 2 }, id);
      nodes.push(l.el);
      labelRect = l.rect;
    }
    const bounds = {
      x: Math.min(a.start.x, a.end.x),
      y: Math.min(a.start.y, a.end.y),
      w: Math.abs(a.end.x - a.start.x) || 1,
      h: Math.abs(a.end.y - a.start.y) || 1,
    };
    this.put(id, bounds, nodes, labelRect);
  }

  private step(id: string, n: number, rect: Rect, text?: string) {
    const g = svg("g", { class: "badge", "data-id": id }, this.layers.steps);
    // On the top-left corner: clear of connectors, which usually meet an
    // element mid-edge. If that element's label sits there, slide it right.
    const cx = Math.max(16, rect.x - 2);
    const cy = Math.max(16, rect.y - 2);
    const badge = { x: cx - 14, y: cy - 14, w: 28, h: 28 };
    for (const it of this.items.values()) {
      if (!it.labelRect || !sameRect(it.rect, rect) || overlapArea(it.labelRect, badge) === 0) continue;
      const el = it.nodes.find((n) => n instanceof HTMLElement) as HTMLElement | undefined;
      const dx = badge.x + badge.w + 4 - it.labelRect.x;
      if (el && dx > 0) {
        it.labelRect = { ...it.labelRect, x: it.labelRect.x + dx };
        el.style.left = `${it.labelRect.x}px`;
      }
    }
    svg("circle", { cx, cy, r: 13 }, g);
    const t = svg("text", { x: cx, y: cy + 0.5 }, g);
    t.textContent = String(n);
    (g as SVGElement).style.transformOrigin = `${cx}px ${cy}px`;
    (g as SVGElement).style.transformBox = "view-box";
    animate(g, [{ transform: "scale(0)" }, { transform: "scale(1.15)" }, { transform: "scale(1)" }], {
      duration: 320,
      easing: "ease-out",
    });
    const nodes: Element[] = [g];
    // A step on an element that is not boxed yet gets a light outline too.
    if (![...this.items.values()].some((i) => sameRect(i.rect, rect))) {
      const o = svg("path", { class: "stroke", d: roundedRectPath(inflate(minVisible(rect), 4)), opacity: 0.55 }, this.layers.shapes);
      drawOn(o, 380);
      nodes.push(o);
    }
    let labelRect: Rect | undefined;
    if (text) {
      const l = this.label(text, inflate(rect, 4), id);
      nodes.push(l.el);
      labelRect = l.rect;
    }
    this.put(id, rect, nodes, labelRect);
  }

  private callout(id: string, rect: Rect, text: string) {
    const l = this.label(text, inflate(rect, 6), id, true);
    this.put(id, rect, [l.el], l.rect);
  }

  private spotlight(rect: Rect, zoom: boolean) {
    if (this.spot) this.fadeRemove(this.spot);
    const hole = inflate(minVisible(rect, 40), zoom ? 14 : 10);
    const p = svg("path", { class: "spot", d: spotlightPath(this.viewport(), hole, 12) }, this.layers.spot) as SVGPathElement;
    animate(p, [{ opacity: 0 }, { opacity: 1 }], { duration: 300, easing: "ease-out" });
    if (zoom) {
      const ring = svg("path", { class: "zoom-ring", d: roundedRectPath(hole, 12) }, this.layers.spot);
      drawOn(ring as SVGGeometryElement, 380);
      const g = svg("g", {}, this.layers.spot);
      g.appendChild(p);
      g.appendChild(ring);
      this.spot = g as unknown as SVGPathElement;
    } else {
      this.spot = p;
    }
  }

  private focus(id: string, rect: Rect) {
    const r = inflate(minVisible(rect), 6);
    const ring = svg("path", { class: "ring", d: roundedRectPath(r, 10) }, this.layers.steps);
    (ring as SVGElement).style.transformOrigin = `${r.x + r.w / 2}px ${r.y + r.h / 2}px`;
    (ring as SVGElement).style.transformBox = "view-box";
    if (reduceMotion() || !(ring as unknown as HTMLElement).animate) {
      setTimeout(() => ring.remove(), 900);
    } else {
      const anim = (ring as unknown as HTMLElement).animate(
        [
          { opacity: 0.95, transform: "scale(1)" },
          { opacity: 0, transform: "scale(1.12)" },
        ],
        { duration: 700, iterations: 2, easing: "ease-out" },
      );
      anim.onfinish = () => ring.remove();
    }
    // bring an existing mark back if it had been cleared
    if (!this.items.has(id)) this.shape(id, "box", rect);
  }

  private point(id: string, rect: Rect, text?: string) {
    const target = center(rect);
    const el = this.pointer;
    const lbl = el.querySelector<HTMLElement>(".pointer-label")!;
    lbl.hidden = !text;
    lbl.textContent = text ?? "";
    const vp = this.viewport();
    const from = this.pointerPos ?? { x: vp.w / 2, y: vp.h - 120 };
    el.hidden = false;
    // keep the bubble on screen
    lbl.style.left = target.x > vp.w - 260 ? "auto" : "30px";
    lbl.style.right = target.x > vp.w - 260 ? "30px" : "auto";
    const place = (p: Pt) => (el.style.transform = `translate(${p.x - 4}px, ${p.y - 3}px)`);
    cancelAnimationFrame(this.pointerAnim);
    if (reduceMotion()) {
      place(target);
    } else {
      // fly along a gentle arc
      const mid = { x: (from.x + target.x) / 2, y: Math.min(from.y, target.y) - Math.hypot(target.x - from.x, target.y - from.y) * 0.25 };
      const t0 = performance.now();
      const dur = Math.min(900, 380 + Math.hypot(target.x - from.x, target.y - from.y) * 0.35);
      const tick = (now: number) => {
        const t = Math.min(1, (now - t0) / dur);
        const e = t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
        const u = 1 - e;
        place({ x: u * u * from.x + 2 * u * e * mid.x + e * e * target.x, y: u * u * from.y + 2 * u * e * mid.y + e * e * target.y });
        if (t < 1) this.pointerAnim = requestAnimationFrame(tick);
      };
      this.pointerAnim = requestAnimationFrame(tick);
    }
    this.pointerPos = target;
    // the pointer is a singleton; remember the target for references only
    this.items.delete(id);
    this.items.set(id, { id, rect, nodes: [] });
  }
}

function sameRect(a: Rect, b: Rect) {
  return Math.abs(a.x - b.x) < 1 && Math.abs(a.y - b.y) < 1 && Math.abs(a.w - b.w) < 1 && Math.abs(a.h - b.h) < 1;
}
