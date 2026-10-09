import { emit, listen } from "@tauri-apps/api/event";
import { Buddy } from "./buddy";
import { Hud, type Phase } from "./hud";
import { Scene, type Annotation } from "./scene";

const DISPLAY = Number(new URLSearchParams(location.search).get("display") ?? 0);

const buddy = new Buddy(document.getElementById("pointer")!);
const scene = new Scene(
  {
    spot: document.getElementById("layer-spot") as unknown as SVGGElement,
    shapes: document.getElementById("layer-shapes") as unknown as SVGGElement,
    arrows: document.getElementById("layer-arrows") as unknown as SVGGElement,
    steps: document.getElementById("layer-steps") as unknown as SVGGElement,
  },
  document.getElementById("labels")!,
  buddy,
);
const hud = new Hud(document.getElementById("hud")!);
let hudHere = DISPLAY === 0;

interface Status {
  phase: Phase;
  message?: string | null;
  display?: number | null;
}

listen<Annotation>("luma://annotate", (e) => {
  const a = e.payload;
  if (a.op === "clear" || a.display === DISPLAY) scene.apply(a);
});
listen("luma://clear", () => scene.clear());
listen<{ display: number; rect: { x: number; y: number; w: number; h: number }; src: string }>("luma://magnify", (e) => {
  if (e.payload.display === DISPLAY) scene.magnify(e.payload.rect, e.payload.src);
});
listen<Status>("luma://status", (e) => {
  const s = e.payload;
  hudHere = (s.display ?? 0) === DISPLAY;
  if (hudHere) hud.setPhase(s.phase, s.message);
  else hud.hide();
  buddy.setState(s.phase);
});
listen<string>("luma://transcript", (e) => hudHere && hud.transcript(e.payload));
listen<string>("luma://caption", (e) => hudHere && hud.caption(e.payload));
listen<string>("luma://notice", (e) => hudHere && hud.notice(e.payload));
listen<number>("luma://level", (e) => {
  if (!hudHere) return;
  hud.level(e.payload);
  buddy.level(e.payload);
});
// Real mouse position, streamed by the app (~60 Hz while it moves).
listen<{ display: number; x: number; y: number } | null>("luma://cursor", (e) => {
  const c = e.payload;
  buddy.setCursor(c && c.display === DISPLAY ? { x: c.x, y: c.y } : null);
});
// Voice-only vs annotate turn (long press).
listen<"annotate" | "voice">("luma://mode", (e) => {
  buddy.setMode(e.payload);
  if (e.payload === "annotate" && hudHere && document.getElementById("hud")!.dataset.phase === "listening") {
    hud.setPhase("listening", "Annotate mode: I'll draw on screen");
  }
});

// Health report for the logs: WebKit can pause animation frames in a window
// it thinks is hidden (transparent overlays). Movement no longer depends on
// them, but this says when it happens. Sent on change only.
let lastHealth = "";
function reportHealth() {
  let fired = false;
  requestAnimationFrame(() => (fired = true));
  setTimeout(() => {
    const h = `visibility=${document.visibilityState} frames=${fired ? "running" : "paused"}`;
    if (h !== lastHealth) {
      lastHealth = h;
      emit("luma://overlay-health", { display: DISPLAY, health: h });
    }
  }, 1000);
}
reportHealth();
setInterval(reportHealth, 15000);
