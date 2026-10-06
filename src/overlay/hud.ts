// Status pill: makes it obvious when LUMA is listening, looking, thinking or
// talking. Shown on the display the user is working on.

export type Phase = "idle" | "listening" | "thinking" | "speaking" | "paused" | "error";

const LABELS: Record<Phase, string> = {
  idle: "",
  listening: "Listening · looking at this screen",
  thinking: "Thinking…",
  speaking: "",
  paused: "Paused — not seeing or listening",
  error: "Something went wrong",
};

export class Hud {
  private hideTimer = 0;
  private phase: Phase = "idle";
  private dot: HTMLElement;
  private text: HTMLElement;
  private sub: HTMLElement;
  private meter: HTMLElement;

  constructor(private root: HTMLElement) {
    this.dot = root.querySelector(".hud-dot")!;
    this.text = root.querySelector(".hud-text")!;
    this.sub = root.querySelector(".hud-sub")!;
    this.meter = root.querySelector(".hud-meter")!;
  }

  setPhase(phase: Phase, message?: string | null) {
    this.phase = phase;
    this.root.dataset.phase = phase;
    clearTimeout(this.hideTimer);
    this.meter.hidden = phase !== "listening";
    if (phase !== "speaking") this.setSub("");
    if (phase === "listening") this.setSub("");
    const text = message ?? LABELS[phase];
    this.text.textContent = text;
    this.text.hidden = !text;
    if (phase === "idle") {
      if (message) {
        this.show();
        this.hideTimer = window.setTimeout(() => this.hide(), 4000);
      } else {
        this.hideTimer = window.setTimeout(() => this.hide(), 600);
      }
    } else if (phase === "error" || phase === "paused") {
      this.show();
      this.hideTimer = window.setTimeout(() => this.hide(), 6000);
    } else {
      this.show();
    }
    void this.dot;
  }

  /** Live transcript while listening, captions while speaking. */
  setSub(text: string) {
    this.sub.textContent = text;
    this.sub.hidden = !text;
    if (text && this.phase === "speaking") this.text.hidden = true;
  }

  transcript(text: string) {
    if (this.phase === "listening") this.setSub(text);
  }

  caption(text: string) {
    if (this.phase === "speaking" || this.phase === "thinking") {
      if (this.phase === "thinking") this.setPhase("speaking");
      this.setSub(text);
    }
  }

  notice(text: string) {
    if (this.phase === "idle") this.setPhase("idle", text);
  }

  level(v: number) {
    const bars = this.meter.querySelectorAll<HTMLElement>("i");
    const scaled = Math.min(1, v * 6);
    bars.forEach((b, i) => {
      const shape = [0.5, 0.8, 1, 0.8, 0.5][i] ?? 1;
      b.style.height = `${4 + scaled * shape * 12}px`;
    });
  }

  show() {
    this.root.hidden = false;
  }

  hide() {
    this.root.hidden = true;
  }
}
