// The companion cursor: trails the real mouse pointer, shows what LUMA is
// doing (listening / thinking / talking) right where the user is looking,
// and flies off to point at things, then comes back.
//
// Movement never depends on requestAnimationFrame: WebKit pauses rAF in
// windows it considers hidden, and a transparent, click-through overlay can
// be judged hidden by macOS. Following sets the position directly on each
// mouse event (a short CSS transition smooths it); flights are Web
// Animations, which run on the compositor like the rest of the overlay.

import type { Pt } from "./geometry";

export type BuddyState = "idle" | "listening" | "thinking" | "speaking" | "acting" | "waiting" | "teaching" | "paused" | "error";

const OFFSET = { x: 14, y: 16 }; // sits just below-right of the real cursor
const reduceMotion = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

/** Points along the curved flight path (quadratic Bézier, eased in-out). */
export function flightPath(from: Pt, to: Pt, steps = 16): Pt[] {
  const dist = Math.hypot(to.x - from.x, to.y - from.y);
  const mid = { x: (from.x + to.x) / 2, y: Math.min(from.y, to.y) - dist * 0.25 };
  const out: Pt[] = [];
  for (let i = 0; i <= steps; i++) {
    const t = i / steps;
    const e = t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
    const u = 1 - e;
    out.push({ x: u * u * from.x + 2 * u * e * mid.x + e * e * to.x, y: u * u * from.y + 2 * u * e * mid.y + e * e * to.y });
  }
  return out;
}

const tf = (p: Pt) => `translate(${p.x - 4}px, ${p.y - 3}px)`;

export class Buddy {
  private pos: Pt | null = null;
  private cursor: Pt | null = null;
  private pinned: { target: Pt; label?: string } | null = null;
  private flight: Animation | null = null;
  private label: HTMLElement;
  private badge: HTMLElement;
  private bars: HTMLElement[];

  constructor(private el: HTMLElement) {
    this.label = el.querySelector(".pointer-label")!;
    this.badge = el.querySelector(".buddy-badge")!;
    this.bars = [...el.querySelectorAll<HTMLElement>(".buddy-badge i")];
  }

  /** Where the companion is (for tests and debugging). */
  position(): Pt | null {
    return this.pos;
  }

  /** Real mouse position on this display (view points), or null if elsewhere. */
  setCursor(p: Pt | null) {
    this.cursor = p;
    if (!p) {
      if (!this.pinned) this.el.hidden = true;
      return;
    }
    if (this.pinned || this.flight) return; // pointing somewhere: stay
    this.place({ x: p.x + OFFSET.x, y: p.y + OFFSET.y }, true);
  }

  /** Annotate-mode cue: a ring around the companion while held long enough. */
  setMode(mode: "annotate" | "voice") {
    this.el.classList.toggle("annotate", mode === "annotate");
  }

  setState(s: BuddyState) {
    this.el.dataset.state = s;
    if (s === "idle" || s === "listening") this.el.classList.remove("annotate");
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
    this.fly(from, target);
  }

  /** Return to the user's cursor. */
  release() {
    if (!this.pinned) return;
    this.pinned = null;
    this.label.hidden = true;
    if (!this.cursor) {
      this.el.hidden = true;
      return;
    }
    const home = { x: this.cursor.x + OFFSET.x, y: this.cursor.y + OFFSET.y };
    if (this.pos) this.fly(this.pos, home);
    else this.place(home, false);
  }

  /** Set the position now; `follow` smooths small cursor moves. */
  private place(p: Pt, follow: boolean) {
    this.pos = p;
    this.el.style.transition = follow && !reduceMotion() ? "transform 90ms linear" : "none";
    this.el.style.transform = tf(p);
    if (this.el.dataset.state !== "paused") this.el.hidden = false;
  }

  private fly(from: Pt, to: Pt) {
    this.flight?.cancel();
    this.flight = null;
    const dist = Math.hypot(to.x - from.x, to.y - from.y);
    this.place(to, false); // the resting position, whatever happens to the animation
    if (reduceMotion() || dist < 2 || !this.el.animate) return;
    const anim = this.el.animate(
      flightPath(from, to).map((p) => ({ transform: tf(p) })),
      { duration: Math.min(900, 380 + dist * 0.35), easing: "linear" },
    );
    this.flight = anim;
    const done = () => {
      if (this.flight !== anim) return;
      this.flight = null;
      // back home and the user kept moving: catch up
      if (!this.pinned && this.cursor) this.place({ x: this.cursor.x + OFFSET.x, y: this.cursor.y + OFFSET.y }, true);
    };
    anim.onfinish = done;
    anim.oncancel = () => {
      if (this.flight === anim) this.flight = null;
    };
  }
}
