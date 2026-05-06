"""CPU OCR helper for the slide-track classifier's tiebreaker.

When the visual classifier can't pick a clear winner (top score doesn't
beat runner-up by `CONFIDENCE_MARGIN`), the GH worker OCRs a few sample
frames per candidate and picks whichever produces the most readable
text. This module wraps `rapidocr` (ONNX-based, CPU-only) behind a tiny
`OcrRunner` callable so the rest of the classifier doesn't have to know
which engine is in use.

Why rapidocr over alternatives:
  * paddleocr: heavier deps, brittle on newer Python.
  * pytesseract: needs the system tesseract binary, an extra setup step
    on the runner.
  * easyocr: torch-based, drags in a multi-GB CUDA-capable wheel even
    when we're CPU-only.

rapidocr ships ONNX models that auto-download on first use (cached
inside the venv). On the GH runner the cache is rebuilt per workflow
run, adding ~12 MB of download to cold-cache jobs - acceptable.
"""

from __future__ import annotations

from typing import Callable

import numpy as np

OcrRunner = Callable[[np.ndarray], str]


# rapidocr is heavy to import (ONNX runtime warmup, model downloads on
# first call). Cache the engine at module level so a single workflow
# run pays the warmup once.
_ENGINE = None


def _engine():
    global _ENGINE
    if _ENGINE is None:
        # Imported lazily so importing this module in environments
        # without rapidocr (e.g. the unit-test runner if a contributor
        # skipped the optional dep) still works for the type-only import.
        from rapidocr import RapidOCR  # type: ignore

        _ENGINE = RapidOCR()
    return _ENGINE


def ocr_text(frame_bgr: np.ndarray) -> str:
    """Run OCR and return the concatenated recognized text.

    rapidocr returns a `RapidOCROutput` with `.txts` (tuple of strings,
    one per detected text region). For the tiebreaker we just want a
    raw character count, so concatenation with a space separator is
    enough; we don't need bbox / confidence metadata here.
    """
    if frame_bgr.ndim != 3 or frame_bgr.shape[2] != 3:
        raise ValueError(f"expected HxWx3 BGR frame, got shape {frame_bgr.shape}")

    result = _engine()(frame_bgr)
    txts = getattr(result, "txts", None) or ()
    return " ".join(t for t in txts if t)


def make_ocr_runner() -> OcrRunner:
    """Return a callable matching the `OcrRunner` protocol expected by
    `play_classifier.tiebreak_by_ocr_char_count`. Separated from
    `ocr_text` so callers can swap implementations (e.g. paddleocr,
    tesseract) without monkey-patching this module."""
    return ocr_text
