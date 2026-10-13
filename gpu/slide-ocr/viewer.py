"""Build runs/<uuid>/viewer.html for validating a slide OCR run by eye.

Plays the lecture with the slide on screen beside it, the OCR blocks drawn
over the slide where the model located them, figure crops, and the
transcript of that slide with the current cue highlighted. Every
result.<model>.json in the run directory becomes a switchable model.

Open the file straight from disk; it reads video.mp4 and slides/ next to it.
Append #t=<seconds> to open at a given moment.
"""

import argparse
import html
import json
from pathlib import Path

PAGE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Slide OCR Viewer</title>
<style>
:root {
  --bg: #f6f6f4; --panel: #ffffff; --text: #1d1d1b; --muted: #6b6b66;
  --border: #dcdcd6; --accent: #1f5fbf; --accent-bg: #e3ecfa; --cue: #fff2c2;
  --title: #c2410c; --body: #1f5fbf; --figure: #15803d; --table: #7e22ce; --other: #6b6b66;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --bg: #161615; --panel: #1f1f1d; --text: #ecece8; --muted: #a3a39c;
    --border: #3a3a36; --accent: #7aa7ee; --accent-bg: #23324a; --cue: #4a3f17;
    --title: #fb923c; --body: #7aa7ee; --figure: #4ade80; --table: #c084fc; --other: #a3a39c;
  }
}
:root[data-theme="dark"] {
  --bg: #161615; --panel: #1f1f1d; --text: #ecece8; --muted: #a3a39c;
  --border: #3a3a36; --accent: #7aa7ee; --accent-bg: #23324a; --cue: #4a3f17;
  --title: #fb923c; --body: #7aa7ee; --figure: #4ade80; --table: #c084fc; --other: #a3a39c;
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--text);
  font: 15px/1.45 system-ui, -apple-system, "Segoe UI", sans-serif; }
header { display: flex; flex-wrap: wrap; gap: 8px 24px; align-items: baseline;
  padding: 12px 16px; border-bottom: 1px solid var(--border); background: var(--panel); }
h1 { font-size: 17px; margin: 0; }
fieldset { border: 0; margin: 0; padding: 0; display: flex; gap: 12px; flex-wrap: wrap; }
legend { float: left; margin-right: 8px; color: var(--muted); }
main { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 16px; padding: 16px; }
@media (max-width: 900px) { main { grid-template-columns: minmax(0, 1fr); } }
section { background: var(--panel); border: 1px solid var(--border); border-radius: 8px; padding: 12px; min-width: 0; }
h2 { font-size: 14px; margin: 0 0 8px; color: var(--muted); font-weight: 600; }
video { width: 100%; border-radius: 4px; background: #000; display: block; }
.slide-wrap { position: relative; }
.slide-wrap img { width: 100%; display: block; border-radius: 4px; }
.slide-wrap svg { position: absolute; inset: 0; width: 100%; height: 100%; }
.slide-wrap rect { fill: transparent; stroke-width: 2px; vector-effect: non-scaling-stroke; }
.slide-wrap rect.on { fill: color-mix(in srgb, currentColor 22%, transparent); stroke-width: 3px; }
.meta { color: var(--muted); font-size: 13px; margin: 6px 0 10px; }
.blocks { display: grid; gap: 6px; }
.block { border-left: 4px solid currentColor; padding: 4px 8px; background: var(--bg); border-radius: 0 4px 4px 0; }
.block.on { outline: 2px solid currentColor; }
.block b { font-size: 12px; text-transform: uppercase; letter-spacing: .04em; }
.block pre { margin: 2px 0 0; white-space: pre-wrap; word-break: break-word; font: inherit; color: var(--text); }
.block small { color: var(--muted); font-variant-numeric: tabular-nums; }
.figures { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 6px; }
.figures img { max-height: 120px; max-width: 100%; border: 1px solid var(--border); border-radius: 4px; background: #fff; }
.cues { position: relative; max-height: 340px; overflow-y: auto; display: grid; gap: 2px; }
.cues button, .strip button { font: inherit; color: inherit; text-align: left; background: none;
  border: 1px solid transparent; border-radius: 4px; padding: 3px 6px; cursor: pointer; }
.cues button:hover, .strip button:hover { border-color: var(--border); }
.cues button[aria-current="true"] { background: var(--cue); }
.cues time, .strip time { color: var(--muted); font-variant-numeric: tabular-nums; margin-right: 6px; }
.strip { grid-column: 1 / -1; }
.strip ol { list-style: none; margin: 0; padding: 0; display: grid;
  grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 4px; }
.strip button { width: 100%; }
.strip button[aria-current="true"] { background: var(--accent-bg); border-color: var(--accent); }
label { cursor: pointer; }
.title { color: var(--title); } .text { color: var(--body); } .image { color: var(--figure); }
.table { color: var(--table); } .other { color: var(--other); }
</style>
</head>
<body>
<header>
  <h1>__TITLE__</h1>
  <fieldset id="models"><legend>Model</legend></fieldset>
  <label><input type="checkbox" id="boxes" checked> Show boxes</label>
</header>
<main>
  <section aria-labelledby="h-video">
    <h2 id="h-video">Lecture</h2>
    <video id="video" src="video.mp4" controls preload="metadata"></video>
    <h2 style="margin-top:12px">Spoken during this slide</h2>
    <div class="cues" id="cues" tabindex="0" aria-label="Transcript of the current slide"></div>
  </section>
  <section aria-labelledby="h-slide">
    <h2 id="h-slide">Slide <span id="slide-no"></span></h2>
    <div class="slide-wrap">
      <img id="slide-img" alt="Captured slide frame">
      <svg id="overlay" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"></svg>
    </div>
    <p class="meta" id="slide-meta"></p>
    <div class="blocks" id="blocks"></div>
  </section>
  <section class="strip" aria-labelledby="h-strip">
    <h2 id="h-strip">Slides</h2>
    <ol id="strip"></ol>
  </section>
</main>
<script>
const RUNS = __RUNS__;
const video = document.getElementById("video");
const names = Object.keys(RUNS);
let model = names[0];
let current = -1;
let currentCue = null;

const fmt = s => {
  s = Math.floor(s);
  const h = Math.floor(s / 3600), m = Math.floor(s % 3600 / 60), r = s % 60;
  return (h ? h + ":" + String(m).padStart(2, "0") : m) + ":" + String(r).padStart(2, "0");
};
// Models name the same things differently (DeepSeek: sub_title, figure_title).
const kind = label => label.endsWith("title") ? "title"
  : ["text", "image", "table"].includes(label) ? label : "other";
const el = (tag, props = {}, children = []) => {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children);
  return node;
};
const slides = () => RUNS[model].slides;
const titleOf = slide => {
  const t = slide.blocks.find(b => kind(b.label) === "title" && b.text) || slide.blocks.find(b => b.text);
  return t ? t.text.replace(/^#+\\s*/, "").split("\\n")[0] : "(no text)";
};
const seek = t => { video.currentTime = t + 0.01; video.play(); };

function renderStrip() {
  const strip = document.getElementById("strip");
  strip.replaceChildren(...slides().map((s, i) => el("li", {}, [
    el("button", { type: "button", onclick: () => seek(s.start) },
       [el("time", { textContent: fmt(s.start) }), titleOf(s)])
  ])));
}

function highlight(index, on) {
  document.querySelectorAll(`[data-block="${index}"]`).forEach(n => n.classList.toggle("on", on));
}

function renderSlide(i) {
  const s = slides()[i];
  document.getElementById("slide-no").textContent = `${i + 1} of ${slides().length}`;
  document.getElementById("slide-img").src = s.image;
  document.getElementById("slide-meta").textContent =
    `${fmt(s.start)} to ${fmt(s.end)} · ${s.blocks.length} blocks · ${s.cues.length} cues · ${s.image}`;
  const svg = document.getElementById("overlay");
  svg.replaceChildren();
  const blocks = document.getElementById("blocks");
  blocks.replaceChildren();
  s.blocks.forEach((b, n) => {
    for (const [x0, y0, x1, y1] of b.boxes) {
      const r = document.createElementNS("http://www.w3.org/2000/svg", "rect");
      Object.entries({ x: x0, y: y0, width: x1 - x0, height: y1 - y0 }).forEach(([k, v]) => r.setAttribute(k, v));
      r.setAttribute("class", kind(b.label));
      r.setAttribute("stroke", "currentColor");
      r.dataset.block = n;
      r.addEventListener("mouseenter", () => highlight(n, true));
      r.addEventListener("mouseleave", () => highlight(n, false));
      svg.append(r);
    }
    const coords = b.boxes.map(box => "[" + box.map(v => Math.round(v * 999)).join(", ") + "]").join(" ");
    const card = el("div", { className: `block ${kind(b.label)}`, tabIndex: 0 }, [
      el("b", { textContent: b.label }), " ", el("small", { textContent: coords || "no box" }),
      el("pre", { textContent: b.text || "" }),
    ]);
    if (b.crops && b.crops.length) {
      card.append(el("div", { className: "figures" },
        b.crops.map(src => el("img", { src, alt: `Figure crop from slide ${i + 1}`, loading: "lazy" }))));
    }
    card.dataset.block = n;
    card.addEventListener("mouseenter", () => highlight(n, true));
    card.addEventListener("mouseleave", () => highlight(n, false));
    card.addEventListener("focus", () => highlight(n, true));
    card.addEventListener("blur", () => highlight(n, false));
    blocks.append(card);
  });
  const cues = document.getElementById("cues");
  cues.replaceChildren(...s.cues.map(c => el("button", { type: "button", onclick: () => seek(c.start) },
    [el("time", { textContent: fmt(c.start) }), c.text])));
  cues.scrollTop = 0;
  currentCue = null;
  document.querySelectorAll("#strip button").forEach((b, n) => b.setAttribute("aria-current", n === i));
}

function sync() {
  const t = video.currentTime;
  const list = slides();
  let i = 0;
  while (i + 1 < list.length && list[i + 1].start <= t) i++;
  if (i !== current) { current = i; renderSlide(i); }
  const cueIndex = list[i].cues.findIndex(c => c.start <= t && t < c.end);
  if (cueIndex !== currentCue) {
    currentCue = cueIndex;
    document.querySelectorAll("#cues button").forEach((b, n) => {
      b.setAttribute("aria-current", n === cueIndex);
      // Scroll the transcript panel only; scrollIntoView would move the page.
      if (n === cueIndex) b.parentElement.scrollTop = b.offsetTop - b.parentElement.clientHeight / 3;
    });
  }
}

const fieldset = document.getElementById("models");
names.forEach(name => {
  const input = el("input", { type: "radio", name: "model", value: name, checked: name === model,
    onchange: () => { model = name; current = -1; renderStrip(); sync(); } });
  fieldset.append(el("label", {}, [input, " " + name]));
});
document.getElementById("boxes").addEventListener("change", e => {
  document.getElementById("overlay").style.display = e.target.checked ? "" : "none";
});
video.addEventListener("timeupdate", sync);
video.addEventListener("seeked", sync);
renderStrip();
sync();
// viewer.html#t=754 opens at that second of the lecture.
const start = Number((location.hash.match(/t=([\\d.]+)/) || [])[1]);
if (start) video.addEventListener("loadedmetadata", () => { video.currentTime = start; sync(); }, { once: true });
</script>
</body>
</html>
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("run_dir", type=Path)
    args = parser.parse_args()

    runs = {}
    for path in sorted(args.run_dir.glob("result.*.json")):
        result = json.loads(path.read_text())
        runs[result["model"]] = result
    if not runs:
        raise SystemExit(f"no result.<model>.json in {args.run_dir}")
    title = next(iter(runs.values()))["title"]

    # The JSON sits inside a <script>; escaping "<" keeps OCR text such as
    # "</table>" or "</script>" from closing it.
    payload = json.dumps(runs, ensure_ascii=False).replace("<", "\\u003c")
    page = PAGE.replace("__TITLE__", html.escape(title)).replace("__RUNS__", payload)
    out = args.run_dir / "viewer.html"
    out.write_text(page)
    print(out)


if __name__ == "__main__":
    main()
