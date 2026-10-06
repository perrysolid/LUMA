import { listen } from "@tauri-apps/api/event";
import { Hud, type Phase } from "./hud";
import { Scene, type Annotation } from "./scene";

const DISPLAY = Number(new URLSearchParams(location.search).get("display") ?? 0);

const scene = new Scene(
  {
    spot: document.getElementById("layer-spot") as unknown as SVGGElement,
    shapes: document.getElementById("layer-shapes") as unknown as SVGGElement,
    arrows: document.getElementById("layer-arrows") as unknown as SVGGElement,
    steps: document.getElementById("layer-steps") as unknown as SVGGElement,
  },
  document.getElementById("labels")!,
  document.getElementById("pointer")!,
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
listen<Status>("luma://status", (e) => {
  const s = e.payload;
  hudHere = (s.display ?? 0) === DISPLAY;
  if (hudHere) hud.setPhase(s.phase, s.message);
  else hud.hide();
});
listen<string>("luma://transcript", (e) => hudHere && hud.transcript(e.payload));
listen<string>("luma://caption", (e) => hudHere && hud.caption(e.payload));
listen<string>("luma://notice", (e) => hudHere && hud.notice(e.payload));
listen<number>("luma://level", (e) => hudHere && hud.level(e.payload));
