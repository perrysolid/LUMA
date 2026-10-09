// Dev-only: LUMA drawing its own diagram ("draw how DNS works") over a busy
// page, through the real overlay Scene: board, nodes, arrows, a freehand
// loop, the flying pointer, an on-screen arrow and a magnifier inset.
// Open with `npx vite` → /dev/sketch-demo.html (add ?static to skip animation timing).
import { Buddy } from "../src/overlay/buddy";
import { Scene, type Annotation } from "../src/overlay/scene";

const page = document.getElementById("page")!;
page.innerHTML = Array.from({ length: 60 }, (_, i) => `<div><span class="ln">${i + 1}</span>const resolver = await lookup(host, { cache: true, ttl: ${300 + i} }); // ${"x".repeat(i % 40)}</div>`).join("");

const buddy = new Buddy(document.getElementById("pointer")!);
buddy.setCursor({ x: 300, y: 600 });
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

const d = 0;
const B = { x: 700, y: 120, w: 680, h: 520 };
const node = (id: string, x: number, y: number, text: string): Annotation => ({ op: "node", display: d, id, rect: { x: B.x + x, y: B.y + y, w: 170, h: 64 }, text });
const share = document.getElementById("share")!.getBoundingClientRect();
const beats: Annotation[] = [
  { op: "board", display: d, id: "board", rect: B, title: "How DNS works" },
  node("browser", 40, 80, "Browser"),
  node("resolver", 470, 80, "DNS resolver"),
  { op: "arrow", display: d, id: "a1", from: { x: B.x + 40, y: B.y + 80, w: 170, h: 64 }, to: { x: B.x + 470, y: B.y + 80, w: 170, h: 64 }, label: "where is example.com?" },
  node("root", 470, 250, "Root server"),
  { op: "arrow", display: d, id: "a2", from: { x: B.x + 470, y: B.y + 80, w: 170, h: 64 }, to: { x: B.x + 470, y: B.y + 250, w: 170, h: 64 }, label: "asks" },
  node("auth", 255, 400, "example.com NS"),
  {
    op: "sketch", display: d, id: "loop", closed: true, label: "cached next time",
    points: [{ x: B.x + 450, y: B.y + 66 }, { x: B.x + 560, y: B.y + 50 }, { x: B.x + 660, y: B.y + 74 }, { x: B.x + 665, y: B.y + 150 }, { x: B.x + 560, y: B.y + 168 }, { x: B.x + 455, y: B.y + 152 }],
  },
  { op: "point", display: d, id: "p", rect: { x: B.x + 255, y: B.y + 400, w: 170, h: 64 }, label: "the final answer" },
  { op: "shape", display: d, id: "share", kind: "box", rect: { x: share.left, y: share.top, w: share.width, h: share.height }, label: "Share" },
];
const fast = new URLSearchParams(location.search).has("static");
beats.forEach((a, i) => setTimeout(() => scene.apply(a), fast ? 0 : 300 + i * 450));
