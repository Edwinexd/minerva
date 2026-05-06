"""Slide-track classifier for play.dsv multi-mp4 presentations.

play.dsv presentations carry 3-4 mp4 tracks per recording with NO labels
distinguishing presenter cam vs screen capture vs PIP composite. This
module ranks the candidates visually and returns the index of the most
slide-like track plus a recommended crop region (for picture-in-picture
recordings where slides occupy a sub-rectangle of the frame).

Pure CPU. No GPU dependencies, no network. Designed to run on a free
GitHub Actions runner under `python3 -m scripts.play_classifier`.

The decision pipeline is:

  1. For each candidate track, sample N frames at percentile timestamps
     and compute a per-frame feature vector. Aggregate to a per-track
     score by z-scoring across the candidate set (so scores compare
     within this presentation, not across lectures).
  2. If the top track wins by a clear margin, return it. Otherwise punt
     to a CPU-OCR tiebreaker: whichever candidate has the most readable
     text wins. (The OCR tiebreaker isn't implemented in this module to
     keep deps light - call it from the GH worker script.)
  3. On the chosen track, run picture-in-picture detection: per-pixel
     temporal std-dev across a short sample window finds the static
     region. If that region is a sane size and aspect ratio, return it
     as the crop. Otherwise OCR the full frame.

Tests use synthetic numpy arrays so the feature signals are validated
deterministically before this ever runs on real DSV data. The
production tuning (threshold values + feature weights) is finalized by
running the offline eval harness (`play_classifier_eval.py`) against a
hand-labeled corpus, NOT by trusting the defaults here.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Sequence

import cv2
import numpy as np


# --- Feature extraction --------------------------------------------------


@dataclass
class FrameFeatures:
    """Per-frame raw features. Higher = more slide-like for the positive
    features (edges, hlines, text), lower = more slide-like for the
    negative ones (color_std, faces). All values normalized so they're
    roughly comparable across frame resolutions; the per-track aggregator
    z-scores them anyway."""

    edge_density: float
    horiz_lines: float
    text_regions: float
    face_count: float
    color_std: float


# Cached face detector. OpenCV's bundled Haar cascade is good enough for
# "is there a face here at all?" - we don't care about identity or
# precise localization, just a boolean signal. YuNet ONNX would be a
# nicer signal but adds a model download dep; revisit if false negatives
# matter in practice.
_FACE_CASCADE: cv2.CascadeClassifier | None = None


def _face_cascade() -> cv2.CascadeClassifier:
    global _FACE_CASCADE
    if _FACE_CASCADE is None:
        path = cv2.data.haarcascades + "haarcascade_frontalface_default.xml"
        _FACE_CASCADE = cv2.CascadeClassifier(path)
    return _FACE_CASCADE


def extract_features(frame_bgr: np.ndarray) -> FrameFeatures:
    """Compute the five features used for slide-likeness scoring.

    `frame_bgr` is a BGR image as returned by `cv2.imread` / `cv2.VideoCapture`.
    Resolution-independent: each feature normalizes to [0, ~few].
    """
    if frame_bgr.ndim != 3 or frame_bgr.shape[2] != 3:
        raise ValueError(f"expected HxWx3 BGR frame, got shape {frame_bgr.shape}")

    h, w = frame_bgr.shape[:2]
    gray = cv2.cvtColor(frame_bgr, cv2.COLOR_BGR2GRAY)

    # Edge density: Canny mean over [0, 255] -> [0, 1]. Slides have
    # crisp text outlines; cams have softer face contours.
    edges = cv2.Canny(gray, 50, 150)
    edge_density = float(edges.mean()) / 255.0

    # Horizontal lines: HoughLinesP filtered for near-horizontal angles,
    # normalized to image height. Slides have many bullet rules / code
    # underlines / table borders; cams have ~none.
    line_segs = cv2.HoughLinesP(
        edges,
        rho=1,
        theta=np.pi / 180,
        threshold=80,
        minLineLength=int(w * 0.1),
        maxLineGap=10,
    )
    horiz_count = 0
    if line_segs is not None:
        for seg in line_segs[:, 0, :]:
            x1, y1, x2, y2 = seg
            dx = abs(int(x2) - int(x1))
            dy = abs(int(y2) - int(y1))
            # Within ~6 degrees of horizontal.
            if dy * 10 <= dx:
                horiz_count += 1
    horiz_lines = horiz_count / max(h, 1)

    # Text regions: MSER blob count, normalized by image area in
    # megapixels so the feature is roughly resolution-independent.
    # MSER finds maximally stable regions - characters and bullet
    # markers light up reliably; flat skin tones mostly don't.
    mser = cv2.MSER.create()
    regions, _ = mser.detectRegions(gray)
    megapixels = (h * w) / 1_000_000.0
    text_regions = len(regions) / max(megapixels, 0.01)

    # Face count: Haar cascade detection. Downsample to 480p before
    # running so detection time stays bounded on 4K source frames.
    target_w = 640
    if w > target_w:
        scale = target_w / w
        small = cv2.resize(gray, (target_w, int(h * scale)))
    else:
        small = gray
    faces = _face_cascade().detectMultiScale(
        small, scaleFactor=1.2, minNeighbors=5, minSize=(30, 30)
    )
    face_count = float(len(faces))

    # Color std: HSV saturation channel std-dev, normalized to [0, 1].
    # Slides have flat backgrounds with low saturation variance; cams
    # have skin tones + lighting variance which spreads saturation.
    hsv = cv2.cvtColor(frame_bgr, cv2.COLOR_BGR2HSV)
    color_std = float(hsv[..., 1].std()) / 255.0

    return FrameFeatures(
        edge_density=edge_density,
        horiz_lines=horiz_lines,
        text_regions=text_regions,
        face_count=face_count,
        color_std=color_std,
    )


# --- Per-track aggregation + scoring -------------------------------------


@dataclass
class TrackEvidence:
    """Aggregated features over the sampled frames of one candidate track,
    plus the temporal-std signal computed across the same frames."""

    edge_density_mean: float
    horiz_lines_mean: float
    text_regions_mean: float
    face_count_mean: float
    color_std_mean: float
    temporal_std: float
    frames_used: int


def aggregate_track(frames_bgr: Sequence[np.ndarray]) -> TrackEvidence:
    """Compute per-track evidence from a sequence of sampled frames.
    Frames must be the same shape (resampled by the caller if needed).
    Caller is responsible for picking diverse timestamps - 8-10 frames
    spread across the recording works well in practice; fewer skews
    temporal_std by chance."""
    if not frames_bgr:
        raise ValueError("frames_bgr is empty")

    feats = [extract_features(f) for f in frames_bgr]
    # Per-pixel std over time, averaged across spatial dims and channels.
    # High = constant motion (cam); low = mostly static (slides).
    stack = np.stack(frames_bgr).astype(np.float32)  # (T, H, W, 3)
    temporal_std = float(stack.std(axis=0).mean()) / 255.0

    return TrackEvidence(
        edge_density_mean=float(np.mean([f.edge_density for f in feats])),
        horiz_lines_mean=float(np.mean([f.horiz_lines for f in feats])),
        text_regions_mean=float(np.mean([f.text_regions for f in feats])),
        face_count_mean=float(np.mean([f.face_count for f in feats])),
        color_std_mean=float(np.mean([f.color_std for f in feats])),
        temporal_std=temporal_std,
        frames_used=len(feats),
    )


# Default weights. Positive = higher feature -> more slide-like.
# Tune via the offline eval harness; these are starting points based
# on the design plan in docs/plans/ocr-video-pipeline.md.
DEFAULT_WEIGHTS = {
    "edge_density": 1.0,
    "horiz_lines": 0.5,
    "text_regions": 1.0,
    "color_std": -1.0,
    "face_count": -2.0,
    "temporal_std": -0.5,
}


@dataclass
class TrackScore:
    track_index: int
    score: float
    evidence: TrackEvidence


def score_tracks(
    track_evidence: Sequence[TrackEvidence],
    weights: dict[str, float] | None = None,
) -> list[TrackScore]:
    """Return per-track scores, z-normalized within the candidate set.

    Z-normalization within the set (not against any global baseline) is
    deliberate: lecture conditions vary wildly (chalkboard vs slide deck
    vs document camera), but WITHIN a single recording the slide track
    stands out from the cam track on these features in a relative sense
    even when absolute values shift. Cross-presentation scores are NOT
    comparable; do not threshold on absolute score values across lectures.
    """
    if not track_evidence:
        return []
    weights = weights or DEFAULT_WEIGHTS

    def vals(name: str) -> np.ndarray:
        return np.array([getattr(e, f"{name}_mean" if name != "temporal_std" else name)
                         for e in track_evidence], dtype=np.float64)

    def z(arr: np.ndarray) -> np.ndarray:
        s = float(arr.std())
        if s < 1e-9:
            # All tracks identical on this feature; contributes nothing.
            return np.zeros_like(arr)
        return (arr - float(arr.mean())) / s

    components = {
        "edge_density": z(vals("edge_density")),
        "horiz_lines": z(vals("horiz_lines")),
        "text_regions": z(vals("text_regions")),
        "color_std": z(vals("color_std")),
        "face_count": z(vals("face_count")),
        "temporal_std": z(vals("temporal_std")),
    }

    scored: list[TrackScore] = []
    for i, ev in enumerate(track_evidence):
        s = sum(weights[name] * components[name][i] for name in weights)
        scored.append(TrackScore(track_index=i, score=float(s), evidence=ev))
    return scored


# --- Decision logic ------------------------------------------------------


@dataclass
class ClassificationResult:
    """Outcome of classifying a presentation's candidate tracks."""

    selected_track_index: int | None
    """None means slide_track_missing (every candidate failed)."""

    score: float | None
    """Aggregate slide_score for the chosen track, None if missing."""

    runner_up_score: float | None
    """Score of the second-best track; None if there was no runner-up."""

    needs_tiebreak: bool
    """True when the top score didn't beat the runner-up by MARGIN.
    The caller should run a CPU-OCR tiebreak (see `tiebreak_by_ocr`
    skeleton at the bottom of this module) and override
    `selected_track_index` based on that."""

    all_scores: list[TrackScore] = field(default_factory=list)


# Tunable thresholds. The plan recommends finalizing these via the
# offline eval harness; values here are reasonable starting points.
HIGH_CONFIDENCE_SCORE = 1.5
"""If the top z-normalized score is at least this, we trust it directly
without a tiebreak (assuming the margin is also met)."""

CONFIDENCE_MARGIN = 1.0
"""Top score must beat runner-up by at least this many z-units to skip
the tiebreak."""

MIN_SCORE_TO_USE = -0.5
"""If even the best track is below this z-score, treat the lecture as
slide_track_missing rather than picking a hopeless candidate."""


def classify_tracks(track_evidence: Sequence[TrackEvidence]) -> ClassificationResult:
    """Run the full decision pipeline. Caller is responsible for
    invoking the OCR tiebreak when `needs_tiebreak` is True."""
    if not track_evidence:
        return ClassificationResult(
            selected_track_index=None,
            score=None,
            runner_up_score=None,
            needs_tiebreak=False,
            all_scores=[],
        )

    scores = score_tracks(track_evidence)
    scores_sorted = sorted(scores, key=lambda s: s.score, reverse=True)
    best = scores_sorted[0]
    runner_up = scores_sorted[1] if len(scores_sorted) > 1 else None

    if best.score < MIN_SCORE_TO_USE:
        return ClassificationResult(
            selected_track_index=None,
            score=best.score,
            runner_up_score=runner_up.score if runner_up else None,
            needs_tiebreak=False,
            all_scores=scores,
        )

    margin_ok = (
        runner_up is None or (best.score - runner_up.score) >= CONFIDENCE_MARGIN
    )
    confident = best.score >= HIGH_CONFIDENCE_SCORE and margin_ok

    return ClassificationResult(
        selected_track_index=best.track_index,
        score=best.score,
        runner_up_score=runner_up.score if runner_up else None,
        needs_tiebreak=not confident,
        all_scores=scores,
    )


# --- Picture-in-picture detection ---------------------------------------


@dataclass
class CropBBox:
    """Pixel coordinates of the slide region inside the chosen track's
    frame. None means no PIP detected; OCR the whole frame."""

    x: int
    y: int
    w: int
    h: int


# Aspect ratio classes for slide regions, with tolerance.
_ALLOWED_ASPECTS = (4 / 3, 16 / 9, 16 / 10)
_ASPECT_TOLERANCE = 0.15


def detect_pip_crop(
    frames_bgr: Sequence[np.ndarray],
    motion_threshold: float = 8.0,
) -> CropBBox | None:
    """Find the static region of the frame using per-pixel temporal
    std-dev. Slide content is mostly static within a 30-180s window;
    cam content is constantly moving. The largest connected static
    rectangle of a sane size + aspect ratio is the slide region.

    Returns None when:
      - The largest static region is too small (probably a logo or
        UI chrome, not the slide deck).
      - The largest static region covers >80% of the frame (no PIP -
        the whole frame is the slide track, OCR it directly).
      - The static region's aspect ratio doesn't match a known slide
        aspect (4:3 / 16:9 / 16:10) within tolerance.
    """
    if len(frames_bgr) < 4:
        return None  # not enough samples to estimate motion

    stack = np.stack(frames_bgr).astype(np.float32)  # (T, H, W, 3)
    # Per-pixel std over time, averaged across channels: H x W float map.
    motion = stack.std(axis=0).mean(axis=-1)
    static_mask = (motion < motion_threshold).astype(np.uint8)

    # Largest connected component of the static mask.
    n_labels, labels, stats, _ = cv2.connectedComponentsWithStats(
        static_mask, connectivity=4
    )
    if n_labels <= 1:
        return None  # only background

    # stats columns: x, y, w, h, area. Skip label 0 (background).
    largest_label = 1 + int(np.argmax(stats[1:, cv2.CC_STAT_AREA]))
    x, y, w, h, area = stats[largest_label]

    h_full, w_full = static_mask.shape
    frame_area = h_full * w_full
    if frame_area == 0:
        return None
    coverage = area / frame_area

    if coverage > 0.80:
        # Whole-frame slide (no PIP composite). Return None so the
        # caller OCRs the unchopped frame.
        return None
    if coverage < 0.30:
        # Static region too small to be the slide deck (logo / UI).
        return None
    if h <= 0 or w <= 0:
        return None
    aspect = w / h
    if not any(
        abs(aspect - target) <= _ASPECT_TOLERANCE for target in _ALLOWED_ASPECTS
    ):
        return None

    return CropBBox(x=int(x), y=int(y), w=int(w), h=int(h))


# --- OCR tiebreaker (skeleton for the GH worker to fill in) -------------


def tiebreak_by_ocr_char_count(
    sample_frames_per_track: Sequence[Sequence[np.ndarray]],
    ocr_runner,
) -> int:
    """When `classify_tracks` returns needs_tiebreak=True, the GH worker
    runs CPU OCR (rapidocr-onnxruntime / paddleocr) on a few sample
    frames per candidate and picks the track with the most readable
    characters. This module doesn't ship the OCR engine itself - that
    keeps the dependency surface light - but provides the tiebreak loop
    so the caller only has to pass in an `ocr_runner(frame_bgr) -> str`.

    Returns the track_index whose summed OCR character count is highest.
    Caller should reject the choice if the highest count is still below
    a `MIN_CHARS` threshold (treat as slide_track_missing).
    """
    if not sample_frames_per_track:
        raise ValueError("sample_frames_per_track is empty")

    char_counts = []
    for frames in sample_frames_per_track:
        total = 0
        for f in frames:
            total += len(ocr_runner(f))
        char_counts.append(total)
    return int(np.argmax(np.array(char_counts)))
