// Dev-only: a frame of LUMA doing a task (agent mode) on a GitHub-like page.
import { Buddy } from "../src/overlay/buddy";
import { Hud } from "../src/overlay/hud";
import { Scene } from "../src/overlay/scene";

const frame = document.getElementById("app") as HTMLIFrameElement;
frame.addEventListener("load", () => {
  const doc = frame.contentDocument!;
  const r = (id: string) => {
    const b = doc.querySelector(`[data-truth="${id}"]`)!.getBoundingClientRect();
    return { x: b.left, y: b.top, w: b.width, h: b.height };
  };
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
  const approve = location.hash === "#approve";
  const target = approve ? r("btn_delete") : r("btn_change_username");
  scene.apply({ op: "shape", display: 0, id: "t", kind: "box", rect: target, label: null });
  scene.apply({ op: "step", display: 0, id: "s", n: approve ? 4 : 2, rect: target });
  scene.apply({ op: "point", display: 0, id: "p", rect: target, label: approve ? "Delete your account" : "Change username" });
  if (approve) {
    hud.setPhase("waiting", "Waiting for your OK");
    hud.setSub('Should I go ahead and click "Delete your account"? Say yes or no.');
  } else {
    hud.setPhase("acting", "Working on it: change my GitHub username to perry-solid");
    hud.setSub("Opening the Change username dialog.");
  }
});
