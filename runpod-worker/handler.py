"""RunPod serverless handler for Minerva's OCR + video-indexing pipeline.

Three task types dispatched on `input.task`:

  * ``ocr_pdf``     - download a PDF from Minerva, rasterize with pypdfium2,
                       OCR each page with DeepSeek-OCR-2, return per-page
                       markdown + uploaded figure metadata.
  * ``ocr_image``   - download a single image, OCR, same output shape but
                       with one entry in ``pages``.
  * ``video_index`` - download a frames bundle (tar.zst), OCR each frame,
                       drop near-blank frames, exact-match dedupe, fuse with
                       the supplied VTT cues, return a timeline.

Auth back to Minerva uses the existing service API key
(``Authorization: Bearer <key>``); RunPod credentials are NOT in scope
here, the backend already authenticated when it submitted the job.

Figure crops are not inlined in the response. The handler POSTs each crop
to ``figure_upload_url`` (an absolute URL the backend embedded in the
job input) which writes the PNG and inserts a ``document_figures`` row.
This keeps RunPod responses small and avoids the 10MB-ish payload caps
RunPod enforces on async outputs.

Model weights are baked into the image at build time
(``COPY ./model_weights/`` in the Dockerfile), so cold-start cost is
limited to loading from local disk into VRAM, not pulling from HF.
"""

from __future__ import annotations

import base64
import io
import json
import os
import sys
import tarfile
import tempfile
import uuid
from dataclasses import dataclass
from typing import Any

import requests
import runpod
import zstandard
from PIL import Image
import pypdfium2 as pdfium

# DeepSeek-OCR-2 weights are baked in at /opt/deepseek-ocr by the
# Dockerfile. Switching to a different OCR model is a one-line change
# here plus a Dockerfile edit; the rest of the pipeline doesn't care
# what model produced the markdown / figure metadata.
MODEL_DIR = os.environ.get("MINERVA_OCR_MODEL_PATH", "/opt/deepseek-ocr")
MODEL_NAME = os.environ.get("MINERVA_OCR_MODEL_NAME", "deepseek-ai/DeepSeek-OCR-2")

# PDF rasterization DPI. 200 is a good floor for legible body text; bump
# for math-heavy decks if recall on subscripts is poor.
PDF_RASTER_DPI = int(os.environ.get("MINERVA_OCR_PDF_DPI", "200"))

# Frames whose markdown is shorter than this are dropped (presenter-cam
# noise, transition fades, blank end cards). 30 chars is roughly "no
# bullet, no caption, but maybe a title" - empirically the cutoff below
# which OCR returns junk on lecture frames.
MIN_FRAME_MARKDOWN_CHARS = int(os.environ.get("MINERVA_OCR_MIN_CHARS", "30"))


# --- Lazy model loader --------------------------------------------------
#
# Loaded once per worker spawn. Importing the heavy stack at module
# import time (rather than inside `handle`) means the cold-start latency
# is paid once during RunPod's worker boot, not on every job's first
# call. The trade-off is that an import error doesn't surface until a
# job runs, so we wrap initialization in a try/except that emits a
# diagnostic and re-raises so RunPod marks the worker FAILED clearly.


def _load_model():
    """Import + load DeepSeek-OCR. Imported lazily so the module is
    importable in test contexts without GPU/CUDA installed."""
    from deepseek_ocr import DeepSeekOCR  # type: ignore

    try:
        return DeepSeekOCR.load(MODEL_DIR)
    except Exception as exc:
        print(
            f"[handler] failed to load model from {MODEL_DIR}: {exc}",
            file=sys.stderr,
        )
        raise


_MODEL = None


def _model():
    global _MODEL
    if _MODEL is None:
        _MODEL = _load_model()
    return _MODEL


# --- Minerva service-API client ----------------------------------------
#
# Tiny wrapper around requests so each call carries the bearer token. The
# token is read from the environment at module load to fail fast in
# misconfigured deployments.


def _service_headers() -> dict[str, str]:
    key = os.environ.get("MINERVA_SERVICE_API_KEY")
    if not key:
        raise RuntimeError(
            "MINERVA_SERVICE_API_KEY is unset; the handler cannot fetch sources"
        )
    return {"Authorization": f"Bearer {key}"}


def _fetch_to(path: str, url: str) -> None:
    with requests.get(url, headers=_service_headers(), stream=True, timeout=300) as resp:
        resp.raise_for_status()
        with open(path, "wb") as fh:
            for chunk in resp.iter_content(chunk_size=1 << 20):
                fh.write(chunk)


# --- OCR primitives ----------------------------------------------------


@dataclass
class OcrFigure:
    """One figure crop returned by the model. ``crop_png`` is bytes; we
    upload it out-of-band rather than inlining in the JSON response."""

    bbox: dict[str, float]  # {x,y,w,h} normalized 0..1 within the OCRed image
    caption: str | None
    crop_png: bytes


@dataclass
class OcrResult:
    markdown: str
    figures: list[OcrFigure]


def _ocr_pil(image: Image.Image) -> OcrResult:
    """Run DeepSeek-OCR on a single PIL image and adapt its output to the
    pipeline's wire format."""
    out = _model().run(image)
    figures: list[OcrFigure] = []
    for raw_fig in out.get("figures", []) or []:
        crop = raw_fig.get("crop_png_b64") or raw_fig.get("crop_png")
        if isinstance(crop, str):
            crop_bytes = base64.b64decode(crop)
        elif isinstance(crop, (bytes, bytearray)):
            crop_bytes = bytes(crop)
        else:
            continue
        figures.append(
            OcrFigure(
                bbox=raw_fig.get("bbox") or {},
                caption=raw_fig.get("caption"),
                crop_png=crop_bytes,
            )
        )
    return OcrResult(markdown=out.get("markdown", ""), figures=figures)


def _upload_figure(
    figure_upload_url: str,
    *,
    page: int | None = None,
    t_start_seconds: float | None = None,
    t_end_seconds: float | None = None,
    bbox: dict[str, float] | None = None,
    caption: str | None = None,
    png: bytes,
) -> str:
    """POST a figure crop + metadata to Minerva. Returns the figure id
    that was registered. We pre-mint the UUID here so the markdown body
    can reference it (``minerva-figure:<id>``) before the upload
    completes."""
    figure_id = str(uuid.uuid4())
    metadata = {
        "figure_id": figure_id,
        "page": page,
        "t_start_seconds": t_start_seconds,
        "t_end_seconds": t_end_seconds,
        "bbox": bbox,
        "caption": caption,
    }
    files = {
        "metadata": ("metadata.json", json.dumps(metadata), "application/json"),
        "png": (f"{figure_id}.png", png, "image/png"),
    }
    resp = requests.post(
        figure_upload_url,
        headers=_service_headers(),
        files=files,
        timeout=120,
    )
    resp.raise_for_status()
    return figure_id


# --- VTT parsing -------------------------------------------------------


@dataclass
class VttCue:
    start_seconds: float
    end_seconds: float
    text: str


def _parse_vtt(vtt_text: str) -> list[VttCue]:
    """Minimal WebVTT parser: timing line of the form
    ``HH:MM:SS.mmm --> HH:MM:SS.mmm``, followed by one or more text lines.
    Cue identifiers and styling blocks are tolerated by skipping non-timing
    lines until the next blank-separated entry."""
    cues: list[VttCue] = []
    if not vtt_text:
        return cues

    # Strip BOM and the WEBVTT header line if present.
    blocks = vtt_text.replace("﻿", "").split("\n\n")
    for block in blocks:
        lines = [ln.strip() for ln in block.splitlines() if ln.strip()]
        if not lines:
            continue
        timing_idx = None
        for i, ln in enumerate(lines):
            if "-->" in ln:
                timing_idx = i
                break
        if timing_idx is None:
            continue
        try:
            start_str, end_str = [s.strip() for s in lines[timing_idx].split("-->")]
            start = _vtt_time_to_seconds(start_str.split()[0])
            end = _vtt_time_to_seconds(end_str.split()[0])
        except Exception:
            continue
        text = " ".join(lines[timing_idx + 1 :])
        cues.append(VttCue(start_seconds=start, end_seconds=end, text=text))
    return cues


def _vtt_time_to_seconds(stamp: str) -> float:
    # HH:MM:SS.mmm or MM:SS.mmm
    parts = stamp.split(":")
    if len(parts) == 2:
        m, s = parts
        return int(m) * 60 + float(s.replace(",", "."))
    if len(parts) == 3:
        h, m, s = parts
        return int(h) * 3600 + int(m) * 60 + float(s.replace(",", "."))
    raise ValueError(f"unrecognized VTT timestamp: {stamp!r}")


# --- Tasks --------------------------------------------------------------


def _handle_ocr_pdf(inp: dict[str, Any]) -> dict[str, Any]:
    figure_upload_url = inp["figure_upload_url"]
    pages_out: list[dict[str, Any]] = []
    with tempfile.TemporaryDirectory() as tmp:
        pdf_path = os.path.join(tmp, "input.pdf")
        _fetch_to(pdf_path, inp["source_url"])
        pdf = pdfium.PdfDocument(pdf_path)
        try:
            for page_idx in range(len(pdf)):
                page = pdf[page_idx]
                pil = page.render(scale=PDF_RASTER_DPI / 72.0).to_pil()
                result = _ocr_pil(pil)
                figure_ids: list[str] = []
                for fig in result.figures:
                    fid = _upload_figure(
                        figure_upload_url,
                        page=page_idx + 1,
                        bbox=fig.bbox,
                        caption=fig.caption,
                        png=fig.crop_png,
                    )
                    figure_ids.append(fid)
                pages_out.append(
                    {
                        "markdown": result.markdown,
                        "figure_ids": figure_ids,
                    }
                )
        finally:
            pdf.close()
    return {"pages": pages_out}


def _handle_ocr_image(inp: dict[str, Any]) -> dict[str, Any]:
    figure_upload_url = inp["figure_upload_url"]
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "input.bin")
        _fetch_to(path, inp["source_url"])
        with Image.open(path) as pil:
            pil.load()
            result = _ocr_pil(pil)
    figure_ids: list[str] = []
    for fig in result.figures:
        fid = _upload_figure(
            figure_upload_url,
            bbox=fig.bbox,
            caption=fig.caption,
            png=fig.crop_png,
        )
        figure_ids.append(fid)
    return {"pages": [{"markdown": result.markdown, "figure_ids": figure_ids}]}


def _parse_fps_fraction(fps: str) -> float:
    if "/" not in fps:
        return float(fps)
    num, den = fps.split("/", 1)
    return float(num) / float(den)


def _normalize_markdown(md: str) -> str:
    """Whitespace-normalize for exact-match dedupe. Bias is toward over-
    segmenting: build-by-build slides where one bullet is added produce
    different OCR output and stay as separate spans. Retrieval can merge
    later; we cannot recover lost timestamps."""
    return " ".join(md.split())


def _handle_video_index(inp: dict[str, Any]) -> dict[str, Any]:
    figure_upload_url = inp["figure_upload_url"]
    seconds_per_frame = 1.0 / _parse_fps_fraction(inp.get("sample_fps", "1/5"))
    cues = _parse_vtt(inp.get("vtt_text", ""))
    timeline_raw: list[dict[str, Any]] = []

    with tempfile.TemporaryDirectory() as tmp:
        bundle_path = os.path.join(tmp, "bundle.tar.zst")
        _fetch_to(bundle_path, inp["bundle_url"])

        # tar.zst -> directory
        with open(bundle_path, "rb") as fh, zstandard.ZstdDecompressor().stream_reader(fh) as reader:
            with tarfile.open(fileobj=reader, mode="r|") as tar:
                tar.extractall(tmp)

        manifest_path = os.path.join(tmp, "manifest.json")
        if os.path.exists(manifest_path):
            with open(manifest_path) as mf:
                manifest = json.load(mf)
            seconds_per_frame = (
                1.0 / _parse_fps_fraction(manifest.get("sample_fps", inp.get("sample_fps", "1/5")))
            )

        frames_dir = os.path.join(tmp, "frames")
        if not os.path.isdir(frames_dir):
            return {"timeline": []}

        frame_paths = sorted(
            os.path.join(frames_dir, f)
            for f in os.listdir(frames_dir)
            if f.lower().endswith(".png")
        )

        for i, fpath in enumerate(frame_paths):
            with Image.open(fpath) as pil:
                pil.load()
                result = _ocr_pil(pil)
            md = result.markdown.strip()
            if len(md) < MIN_FRAME_MARKDOWN_CHARS:
                continue
            t = i * seconds_per_frame
            timeline_raw.append(
                {
                    "t": t,
                    "markdown": md,
                    "figures": result.figures,
                }
            )

    # Exact-match dedupe (after whitespace normalization). The plan
    # explicitly chose exact-match over fuzzy ratio after the design
    # review flagged that fuzz.token_set_ratio collapses build-by-build
    # slides; the whole point of #46 is keeping per-build granularity.
    deduped: list[dict[str, Any]] = []
    for span in timeline_raw:
        norm = _normalize_markdown(span["markdown"])
        if deduped and deduped[-1]["_norm"] == norm:
            deduped[-1]["t_end"] = span["t"]
            continue
        deduped.append(
            {
                "t_start": span["t"],
                "t_end": span["t"],
                "markdown": span["markdown"],
                "figures": span["figures"],
                "_norm": norm,
            }
        )

    # Upload figures and build response. We only register figures for
    # spans that survived dedupe; intermediate frames' crops are dropped
    # because they reference timestamps we no longer surface.
    out_spans: list[dict[str, Any]] = []
    for span in deduped:
        figure_ids: list[str] = []
        for fig in span["figures"]:
            fid = _upload_figure(
                figure_upload_url,
                t_start_seconds=span["t_start"],
                t_end_seconds=span["t_end"],
                bbox=fig.bbox,
                caption=fig.caption,
                png=fig.crop_png,
            )
            figure_ids.append(fid)

        # Fuse VTT cues that overlap this span.
        vtt_pieces = [
            c.text
            for c in cues
            if c.start_seconds < span["t_end"] + seconds_per_frame
            and c.end_seconds > span["t_start"]
        ]
        out_spans.append(
            {
                "t_start": span["t_start"],
                "t_end": span["t_end"],
                "markdown": span["markdown"],
                "vtt_text": " ".join(vtt_pieces) if vtt_pieces else None,
                "figure_ids": figure_ids,
            }
        )

    return {"timeline": out_spans}


# --- RunPod entrypoint -------------------------------------------------


_TASK_HANDLERS = {
    "ocr_pdf": _handle_ocr_pdf,
    "ocr_image": _handle_ocr_image,
    "video_index": _handle_video_index,
}


def handle(job: dict[str, Any]) -> dict[str, Any]:
    inp = job.get("input") or {}
    task = inp.get("task")
    handler = _TASK_HANDLERS.get(task)
    if handler is None:
        return {"error": f"unknown task: {task!r}"}
    try:
        return handler(inp)
    except requests.HTTPError as e:
        # Surface upstream HTTP errors verbatim so retry/dead-letter
        # decisions on the Minerva side can pattern-match on them.
        return {
            "error": f"service_http_error: {e}",
            "status_code": getattr(e.response, "status_code", None),
        }
    except Exception as e:  # noqa: BLE001 - boundary, want everything
        return {"error": f"handler_exception: {type(e).__name__}: {e}"}


if __name__ == "__main__":
    runpod.serverless.start({"handler": handle})
