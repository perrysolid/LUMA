// The companion cursor: trails the real mouse pointer, shows what LUMA is
// doing (listening / thinking / talking) right where the user is looking,
// and flies off to point at things, then comes back.

import type { Pt } from "./geometry";

export type BuddyState = "idle" | "listening" | "thinking" | "speaking" | "paused" | "error";

const OFFSET = { x: 14, y: 16 }; // sits just below-right of the real cursor
const reduceMotion = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

export class Buddy {
  private pos: Pt | null = null;
  private cursor: Pt | null = null;
  private pinned: { target: Pt; label?: string } | null = null;
  private flight: { from: Pt; to: Pt; mid: Pt; t0: number; dur: number } | null = null;
  private raf = 0;
  private label: HTMLElement;
  private badge: HTMLElement;
  private bars: HTMLElement[];

  constructor(private el: HTMLElement) {
    this.label = el.querySelector(".pointer-label")!;
    this.badge = el.querySelector(".buddy-badge")!;
    this.bars = [...el.querySelectorAll<HTMLElement>(".buddy-badge i")];
    this.loop = this.loop.bind(this);
  }

  /** Real mouse position on this display (view points), or null if elsewhere. */
  setCursor(p: Pt | null) {
    this.cursor = p;
    if (!p && !this.pinned) this.el.hidden = true;
    if (p && !this.pos) this.pos = { x: p.x + OFFSET.x, y: p.y + OFFSET.y };
    this.kick();
  }

  setState(s: BuddyState) {
    this.el.dataset.state = s;
    this.badge.hidden = !(s === "listening" || s === "thinking");
    if (s === "idle" || s === "error" || s === "paused") this.release();
    if (s === "paused") this.el.hidden = true;
  }

  level(v: number) {
    const scaled = Math.min(1, v * 6);
    this.bars.forEach((b, i) => (b.style.height = `${3 + scaled * [0.6, 1, 0.6][i % 3] * 9}px`));
  }

  /** Fly to a target and stay there until released. */
  flyTo(target: Pt, text?: string) {
    this.pinned = { target, label: text };
    this.label.hidden = !text;
    this.label.textContent = text ?? "";
    const vw = window.innerWidth;
    this.label.style.left = target.x > vw - 260 ? "auto" : "30px";
    this.label.style.right = target.x > vw - 260 ? "30px" : "auto";
    const from = this.pos ?? (this.cursor ? { x: this.cursor.x + OFFSET.x, y: this.cursor.y + OFFSET.y } : target);
    this.startFlight(from, target);
  }

  /** Return to the user's cursor. */
  release() {
    if (!this.pinned) return;
    this.pinned = null;
    this.label.hidden = true;
    if (this.pos && this.cursor) this.startFlight(this.pos, { x: this.cursor.x + OFFSET.x, y: this.cursor.y + OFFSET.y });
    this.kick();
  }

  private startFlight(from: Pt, to: Pt) {
    const dist = Math.hypot(to.x - from.x, to.y - from.y);
    if (reduceMotion() || dist < 2) {
      this.flight = null;
      this.pos = to;
    } else {
      const mid = { x: (from.x + to.x) / 2, y: Math.min(from.y, to.y) - dist * 0.25 };
      this.flight = { from, to, mid, t0: performance.now(), dur: Math.min(900, 380 + dist * 0.35) };
    }
    this.el.hidden = false;
    this.kick();
  }

  private kick() {
    if (!this.raf) this.raf = requestAnimationFrame(this.loop);
  }

  private loop(now: number) {
    this.raf = 0;
    let moving = false;
    if (this.flight) {
      const f = this.flight;
      const t = Math.min(1, (now - f.t0) / f.dur);
      const e = t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
      const u = 1 - e;
      this.pos = {
        x: u * u * f.from.x + 2 * u * e * f.mid.x + e * e * f.to.x,
        y: u * u * f.from.y + 2 * u * e * f.mid.y + e * e * f.to.y,
      };
      if (t >= 1) this.flight = null;
      moving = true;
    } else if (!this.pinned && this.cursor) {
      // spring towards the cursor
      const goal = { x: this.cursor.x + OFFSET.x, y: this.cursor.y + OFFSET.y };
      const cur = this.pos ?? goal;
      const k = reduceMotion() ? 1 : 0.3;
      const next = { x: cur.x + (goal.x - cur.x) * k, y: cur.y + (goal.y - cur.y) * k };
      moving = Math.abs(goal.x - next.x) > 0.3 || Math.abs(goal.y - next.y) > 0.3;
      this.pos = moving ? next : goal;
      if (this.el.dataset.state !== "paused") this.el.hidden = false;
    }
    if (this.pos) this.el.style.transform = `translate(${this.pos.x - 4}px, ${this.pos.y - 3}px)`;
    if (moving) this.kick();
  }
}
