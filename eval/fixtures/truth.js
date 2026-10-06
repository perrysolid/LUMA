// Collects ground-truth rects for every [data-truth] element after layout and
// writes them, with the fixture's cases, into the DOM for --dump-dom.
window.addEventListener("load", () => {
  const targets = {};
  for (const el of document.querySelectorAll("[data-truth]")) {
    const r = el.getBoundingClientRect();
    targets[el.dataset.truth] = { x: r.left, y: r.top, w: r.width, h: r.height, label: el.dataset.label || el.textContent.trim().slice(0, 40) };
  }
  const cases = JSON.parse(document.getElementById("cases").textContent);
  const out = document.createElement("script");
  out.type = "application/json";
  out.id = "truth-out";
  out.textContent = JSON.stringify({ viewport: { w: innerWidth, h: innerHeight }, app: document.body.dataset.app || "", title: document.title, targets, cases });
  document.body.appendChild(out);
});
