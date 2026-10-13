"""Visual extraction worker: the whole life of one Slurm job.

Started by slurm-worker.sh with the path of a file holding the worker's
signed URL (the only credential it gets). Loads Unlimited-OCR on vLLM and
the CLIP image encoder once, then pulls one item at a time from Minerva
until the queue is empty or the wall time is close:

- play_lecture: download the staged video, find the slides, OCR their
  frames in one batch, merge segments of the same slide, upload slide
  frames and figure crops.
- pdf: download the PDF, render every page, OCR them in one batch, upload
  figure crops (PDF pages are not stored as images).

Speech alignment happens in Minerva, which holds the transcript. A failed
item is reported and the worker carries on; Minerva decides on retries.

    worker.py grants/<worker-uuid>
    worker.py --dry-run play_lecture|pdf <source> <out-dir>
"""

import io
import json
import os
import shutil
import sys
import tempfile
import time
import traceback
from pathlib import Path

import attr
import requests

import extract
from ocr_vllm import VllmOcr

VISUAL_MODEL = "Qdrant/clip-ViT-B-32-vision"
# Stop taking new items once less than this much wall time is left; the
# longest item (a 150-minute lecture or a 300-page PDF) fits well inside.
WALL_MARGIN_SECONDS = 45 * 60
HTTP_TIMEOUT = 120
# Stored images are what fills Minerva's disk: slide frames are only shown
# next to citations, so 1280 px at quality 80 (about half a 1080p frame at
# 90) is plenty; figure crops keep more detail.
SLIDE_LONG_SIDE = 1280
SLIDE_QUALITY = 80
FIGURE_LONG_SIDE = 1600
FIGURE_QUALITY = 85


@attr.s(auto_attribs=True)
class Engines:
    ocr: VllmOcr
    clip: object

    @classmethod
    def load(cls):
        from fastembed import ImageEmbedding

        return cls(ocr=VllmOcr(), clip=ImageEmbedding(VISUAL_MODEL))


def jpeg(image, long_side, quality):
    image = image.convert("RGB")
    image.thumbnail((long_side, long_side))
    buffer = io.BytesIO()
    image.save(buffer, format="JPEG", quality=quality, optimize=True)
    return buffer.getvalue()


def download(session, url, dest):
    with session.get(url, stream=True, timeout=HTTP_TIMEOUT) as response:
        response.raise_for_status()
        with open(dest, "wb") as out:
            for chunk in response.iter_content(chunk_size=1 << 20):
                out.write(chunk)


def lecture_pages(engines, video, th):
    segments = extract.detect_slides(video, th)
    for page, raw in zip(segments, engines.ocr([s.image for s in segments])):
        extract.read_page(page, raw)
    return extract.merge_same_slide(segments, th.text_overlap)


def pdf_pages(engines, pdf):
    pages = extract.render_pdf(pdf)
    for page, raw in zip(pages, engines.ocr([p.image for p in pages])):
        extract.read_page(page, raw)
    return pages


def build_result(engines, kind, pages):
    """The multipart fields of a result upload: `result` JSON plus one field
    per image, named by the path the JSON refers to it by."""
    files = []
    result_pages = []
    figures = []
    for position, page in enumerate(pages):
        image_name = None
        if kind == "play_lecture":
            image_name = f"slides/{position}.jpg"
            files.append((image_name, jpeg(page.image, SLIDE_LONG_SIDE, SLIDE_QUALITY)))
        result_pages.append(
            {
                "position": position,
                "page_number": page.page_number,
                "start": page.start,
                "end": page.end,
                "image": image_name,
                "blocks": page.blocks,
                "text": page.text,
            }
        )
        figures.extend(extract.figures_on(position, page))

    vectors = list(engines.clip.embed([f.image.convert("RGB") for f in figures])) if figures else []
    result_figures = []
    for number, (figure, vector) in enumerate(zip(figures, vectors)):
        name = f"figures/{number}.jpg"
        files.append((name, jpeg(figure.image, FIGURE_LONG_SIDE, FIGURE_QUALITY)))
        result_figures.append(
            {
                "page_position": figure.page_position,
                "bbox": figure.bbox,
                "image": name,
                "caption": figure.caption,
                "context": figure.context,
                "visual_vector": [float(v) for v in vector],
            }
        )
    result = {"visual_model": VISUAL_MODEL, "pages": result_pages, "figures": result_figures}
    return [("result", ("result.json", json.dumps(result), "application/json"))] + [
        (name, (Path(name).name, data, "image/jpeg")) for name, data in files
    ]


def process(session, engines, item, workdir, th):
    source = workdir / ("source.mp4" if item["kind"] == "play_lecture" else "source.pdf")
    started = time.monotonic()
    download(session, item["source_url"], source)
    if item["kind"] == "play_lecture":
        pages = lecture_pages(engines, source, th)
    else:
        pages = pdf_pages(engines, source)
    fields = build_result(engines, item["kind"], pages)
    response = session.put(item["result_url"], files=fields, timeout=HTTP_TIMEOUT)
    response.raise_for_status()
    print(
        f"{item['kind']} {item['job_id']} attempt {item['attempt']}: "
        f"{len(pages)} pages in {time.monotonic() - started:.0f}s",
        flush=True,
    )


def dry_run(kind, source, out_dir):
    """Write the result fields one item would upload into out_dir instead,
    for testing Minerva's ingest without a queue."""
    engines = Engines.load()
    th = extract.Thresholds()
    pages = lecture_pages(engines, source, th) if kind == "play_lecture" else pdf_pages(engines, source)
    for name, (_, data, _) in build_result(engines, kind, pages):
        path = out_dir / ("result.json" if name == "result" else name)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data.encode() if isinstance(data, str) else data)
    print(f"{len(pages)} pages -> {out_dir}")


def main():
    if sys.argv[1] == "--dry-run":
        dry_run(sys.argv[2], Path(sys.argv[3]), Path(sys.argv[4]))
        return
    grant_file = Path(sys.argv[1])
    worker_url = grant_file.read_text().strip()
    deadline = time.time() + float(os.environ["SLIDE_OCR_WALL_SECONDS"]) - WALL_MARGIN_SECONDS
    workroot = Path(os.environ.get("SLIDE_OCR_WORKDIR", Path(__file__).parent / "work"))
    workroot.mkdir(parents=True, exist_ok=True)
    th = extract.Thresholds()
    session = requests.Session()
    error = None
    try:
        engines = Engines.load()
        while time.time() < deadline:
            response = session.post(f"{worker_url}/next", timeout=HTTP_TIMEOUT)
            if response.status_code == 204:
                break
            response.raise_for_status()
            item = response.json()
            workdir = Path(tempfile.mkdtemp(dir=workroot))
            try:
                process(session, engines, item, workdir, th)
            except Exception:
                message = traceback.format_exc(limit=5)[-2000:]
                print(f"item {item['job_id']} failed:\n{message}", flush=True)
                try:
                    session.post(item["fail_url"], json={"error": message}, timeout=HTTP_TIMEOUT)
                except requests.RequestException as e:
                    print(f"could not report failure: {e}", flush=True)
            finally:
                shutil.rmtree(workdir, ignore_errors=True)
    except Exception:
        error = traceback.format_exc(limit=5)[-2000:]
        print(error, flush=True)
        raise
    finally:
        try:
            session.post(f"{worker_url}/exit", json={"error": error}, timeout=HTTP_TIMEOUT)
        finally:
            grant_file.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
