"""Tests for the rapidocr-backed OCR runner.

Skipped automatically when rapidocr isn't installed, so the rest of the
classifier test suite remains runnable without the optional dep.
"""

from __future__ import annotations

import sys
from pathlib import Path

import cv2
import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

rapidocr = pytest.importorskip("rapidocr")

import play_classifier as pc  # noqa: E402
import play_ocr  # noqa: E402


def _slide_with_text(text: str, width: int = 1200, height: int = 400) -> np.ndarray:
    img = np.full((height, width, 3), 240, dtype=np.uint8)
    cv2.putText(
        img, text, (40, height // 2), cv2.FONT_HERSHEY_SIMPLEX, 1.6, (10, 10, 10), 3
    )
    return img


def _blank() -> np.ndarray:
    return np.full((400, 1200, 3), 240, dtype=np.uint8)


def test_ocr_text_recognizes_simple_text():
    img = _slide_with_text("Hello DSV lecture")
    text = play_ocr.ocr_text(img)
    # Don't pin to exact recognition - rapidocr can split words. Just
    # require that a substantial fraction of the rendered characters
    # comes back.
    assert len(text) >= 10, text
    assert "ello" in text or "lecture" in text or "DSV" in text


def test_ocr_text_returns_empty_on_blank_frame():
    text = play_ocr.ocr_text(_blank())
    assert text == "" or len(text) <= 2, repr(text)


def test_ocr_text_rejects_non_bgr():
    gray = np.zeros((400, 1200), dtype=np.uint8)
    with pytest.raises(ValueError):
        play_ocr.ocr_text(gray)


def test_make_ocr_runner_plugs_into_classifier_tiebreak():
    """End-to-end: the classifier's tiebreak loop accepts the runner
    returned by play_ocr.make_ocr_runner and picks the track with
    more readable text."""
    runner = play_ocr.make_ocr_runner()

    # Two synthetic candidate sets: track 0 has lots of slide text,
    # track 1 is mostly blank. Tiebreaker should pick track 0.
    track_0 = [
        _slide_with_text("Lecture title with multiple readable words"),
        _slide_with_text("Bullet point one with decent length text content"),
        _slide_with_text("Definition: this is a longer sentence with content"),
    ]
    track_1 = [_blank(), _blank(), _blank()]

    chosen = pc.tiebreak_by_ocr_char_count([track_0, track_1], runner)
    assert chosen == 0
