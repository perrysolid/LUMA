// Dev-only: draws marks the real model produced in an eval run over that
// fixture's screenshot, through the real overlay Scene.
//   LUMA_REPLAY=1 npm run eval -- --filter sketch
//   npx vite → /dev/replay.html?case=sketch-web-request
import { Buddy } from "../src/overlay/buddy";
import { Scene, type Annotation } from "../src/overlay/scene";

const id = new URLSearchParams(location.search).get("case") ?? "";
const data: { fixture: string; marks: Annotation[] } = await (await fetch(`/eval/out/replay/${id}.json`)).json();
document.getElementById("shot")!.style.backgroundImage = `url(/eval/out/${data.fixture}.png)`;
const buddy = new Buddy(document.getElementById("pointer")!);
buddy.setCursor({ x: 200, y: 700 });
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
data.marks.forEach((m, i) => setTimeout(() => scene.apply(m), 200 + i * 300));
