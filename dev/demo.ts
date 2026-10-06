// Dev-only: replays a scripted "explain this architecture" walkthrough through
// the real overlay Scene, so annotation visuals can be checked in a browser
// without the desktop app or any API keys.
import { Hud } from "../src/overlay/hud";
import { Scene, type Annotation } from "../src/overlay/scene";
import type { Rect } from "../src/overlay/geometry";

const rectOf = (id: string): Rect => {
  const r = document.getElementById(id)!.getBoundingClientRect();
  return { x: r.left, y: r.top, w: r.width, h: r.height };
};

// draw the slide's own connectors
const wires = document.getElementById("wires")!;
const edge = (a: string, b: string) => {
  const A = rectOf(a);
  const B = rectOf(b);
  const p = document.createElementNS("http://www.w3.org/2000/svg", "path");
  const sx = A.x + A.w;
  const sy = A.y + A.h / 2;
  const ex = B.x;
  const ey = B.y + B.h / 2;
  p.setAttribute("d", Math.abs(sy - ey) < 2 ? `M${sx},${sy}H${ex - 2}` : `M${sx},${sy}C${sx + 60},${sy} ${ex - 60},${ey} ${ex - 2},${ey}`);
  wires.appendChild(p);
};
edge("client", "gateway");
edge("gateway", "auth");
edge("gateway", "orders");
edge("orders", "queue");
edge("queue", "email");
{
  const o = rectOf("orders");
  const d = rectOf("db");
  const p = document.createElementNS("http://www.w3.org/2000/svg", "path");
  p.setAttribute("d", `M${o.x + o.w / 2},${o.y + o.h}V${d.y - 2}`);
  wires.appendChild(p);
}

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

type Beat = { say: string; marks: Annotation[] };
const d = 0;
const script = (): Beat[] => [
  {
    say: "This is an order platform, and every request starts at the web client.",
    marks: [
      { op: "clear" },
      { op: "shape", display: d, id: "client", kind: "box", rect: rectOf("client"), label: "Start here" },
      { op: "step", display: d, id: "s1", n: 1, rect: rectOf("client") },
    ],
  },
  {
    say: "It talks to a single front door, the API gateway, which routes each call.",
    marks: [
      { op: "shape", display: d, id: "gateway", kind: "box", rect: rectOf("gateway"), label: "Front door" },
      { op: "step", display: d, id: "s2", n: 2, rect: rectOf("gateway") },
      { op: "arrow", display: d, id: "a1", from: rectOf("client"), to: rectOf("gateway"), label: "HTTPS" },
    ],
  },
  {
    say: "First it checks who you are with the auth service.",
    marks: [
      { op: "shape", display: d, id: "auth", kind: "circle", rect: rectOf("auth"), label: "Identity" },
      { op: "arrow", display: d, id: "a2", from: rectOf("gateway"), to: rectOf("auth"), label: "verify token" },
    ],
  },
  {
    say: "Then the orders service does the real work and saves the order in its own database.",
    marks: [
      { op: "shape", display: d, id: "orders", kind: "box", rect: rectOf("orders"), label: "Business logic" },
      { op: "step", display: d, id: "s3", n: 3, rect: rectOf("orders") },
      { op: "arrow", display: d, id: "a3", from: rectOf("orders"), to: rectOf("db"), label: "writes" },
      { op: "shape", display: d, id: "db", kind: "highlight", rect: rectOf("db") },
    ],
  },
  {
    say: "Finally it drops an event on the queue, so slow work like emails happens in the background.",
    marks: [
      { op: "spotlight", display: d, rect: { ...rectOf("queue"), w: rectOf("email").x + rectOf("email").w - rectOf("queue").x } },
      { op: "arrow", display: d, id: "a4", from: rectOf("queue"), to: rectOf("email"), label: "async" },
      { op: "point", display: d, id: "queue", rect: rectOf("queue"), label: "Event queue" },
    ],
  },
];

let timers: number[] = [];
function reset() {
  timers.forEach(clearTimeout);
  timers = [];
  scene.clear();
  hud.hide();
}

document.getElementById("play")!.addEventListener("click", () => {
  reset();
  hud.setPhase("speaking");
  let t = 0;
  for (const beat of script()) {
    timers.push(
      window.setTimeout(() => {
        hud.setSub(beat.say);
        beat.marks.forEach((m) => scene.apply(m));
      }, t),
    );
    t += 2600;
  }
  timers.push(window.setTimeout(() => hud.setPhase("idle"), t));
});
document.getElementById("final")!.addEventListener("click", () => {
  reset();
  hud.setPhase("speaking");
  const beats = script();
  beats.forEach((b) => b.marks.forEach((m) => scene.apply(m)));
  hud.setSub(beats[beats.length - 1].say);
});
document.getElementById("clear")!.addEventListener("click", reset);

// `#final` renders the last frame immediately (used for headless screenshots).
if (location.hash === "#final") requestAnimationFrame(() => document.getElementById("final")!.click());
