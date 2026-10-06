// Renders every fixture with headless Chrome at 2x (Retina) and extracts
// ground-truth rects from the live DOM. Output: eval/out/<name>.png + .json
import { execFileSync } from "node:child_process";
import { mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const chrome =
  process.env.CHROME ??
  (process.platform === "darwin"
    ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
    : process.platform === "win32"
      ? "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe"
      : "google-chrome");
const W = 1440, H = 900, DPR = 2;
const out = join(here, "out");
mkdirSync(out, { recursive: true });
const common = ["--headless=new", "--disable-gpu", "--hide-scrollbars", `--window-size=${W},${H}`, `--force-device-scale-factor=${DPR}`, "--allow-file-access-from-files", "--virtual-time-budget=3000"];

for (const f of readdirSync(join(here, "fixtures")).filter((f) => f.endsWith(".html"))) {
  const name = f.replace(/\.html$/, "");
  const url = pathToFileURL(resolve(here, "fixtures", f)).href;
  execFileSync(chrome, [...common, `--screenshot=${join(out, name + ".png")}`, url], { stdio: "ignore" });
  const dom = execFileSync(chrome, [...common, "--dump-dom", url], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], maxBuffer: 1 << 26 });
  const m = dom.match(/<script type="application\/json" id="truth-out">([\s\S]*?)<\/script>/);
  if (!m) throw new Error(`no truth for ${name}`);
  const truth = JSON.parse(m[1].replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">"));
  writeFileSync(join(out, name + ".json"), JSON.stringify({ name, dpr: DPR, ...truth }, null, 2));
  console.log(`${name}: ${Object.keys(truth.targets).length} targets, ${truth.cases.length} cases`);
}
