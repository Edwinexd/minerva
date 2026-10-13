"""Run the production worker's extraction on one lecture fetched by
fetch.py, and write what it found for viewer.py.

Same detection, OCR (Unlimited-OCR on vLLM), merging and figure crops as
worker.py, but reading runs/<uuid>/ instead of Minerva's queue and aligning
the transcript locally the way Minerva does at ingest.
"""

import argparse
import json
import time
from bisect import bisect_right
from pathlib import Path

import attr

import extract
import worker

MODEL = "unlimited-ocr"


def align(pages, cues):
    starts = [p.start for p in pages]
    spoken = [[] for _ in pages]
    for cue in cues:
        middle = (cue["start"] + cue["end"]) / 2
        spoken[max(bisect_right(starts, middle) - 1, 0)].append(cue)
    return spoken


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("run_dir", type=Path, help="runs/<uuid> as written by fetch.py")
    for field in attr.fields(extract.Thresholds):
        parser.add_argument(f"--{field.name.replace('_', '-')}", type=field.type, default=field.default)
    args = parser.parse_args()
    th = extract.Thresholds(**{f.name: getattr(args, f.name) for f in attr.fields(extract.Thresholds)})
    run_dir = args.run_dir
    meta = json.loads((run_dir / "meta.json").read_text())
    cues = json.loads((run_dir / "transcript.json").read_text())

    engines = worker.Engines.load()
    started = time.monotonic()
    pages = worker.lecture_pages(engines, run_dir / "video.mp4", th)
    print(f"extract: {len(pages)} slides in {time.monotonic() - started:.0f}s")

    slide_dir = run_dir / "slides"
    slide_dir.mkdir(exist_ok=True)
    slides = []
    for position, (page, spoken) in enumerate(zip(pages, align(pages, cues))):
        image = f"slides/{position}.jpg"
        (run_dir / image).write_bytes(worker.jpeg(page.image, worker.SLIDE_LONG_SIDE, worker.SLIDE_QUALITY))
        for number, figure in enumerate(extract.figures_on(position, page)):
            crop = f"slides/{position}.fig{number}.jpg"
            (run_dir / crop).write_bytes(worker.jpeg(figure.image, worker.FIGURE_LONG_SIDE, worker.FIGURE_QUALITY))
            block = next(b for b in page.blocks if b["label"] == extract.FIGURE_LABEL and figure.bbox in b["boxes"])
            block.setdefault("crops", []).append(crop)
        slides.append(
            {
                "start": page.start,
                "end": page.end,
                "image": image,
                "page": page.page_number,
                "blocks": page.blocks,
                "text": page.text,
                "cues": spoken,
            }
        )

    out = run_dir / f"result.{MODEL}.json"
    out.write_text(json.dumps({"title": meta["title"], "model": MODEL, "slides": slides}, ensure_ascii=False, indent=2) + "\n")
    print(f"done: {out}")


if __name__ == "__main__":
    main()
