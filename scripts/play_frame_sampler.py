"""ffmpeg-based frame sampling helpers used by the classifier and the
production play-ingest workflow.

Two responsibilities:

  * `sample_frames(path, n)`: pull N frames at percentile timestamps from
    a local video file. Used during classification: cheap, doesn't care
    about exact frame boundaries.

  * `extract_frames_at_fps(path, out_dir, fps, crop=None)`: produce the
    bundle's frame set after track + crop are decided. Output-side seek,
    accurate frame boundaries, optional crop applied via the ffmpeg
    filter graph so we never OCR pixels we don't need.

Both shell out to `ffmpeg` / `ffprobe`; the GH workflow installs them on
the runner. A failed ffmpeg invocation surfaces stderr to the caller so
log triage doesn't require re-running locally.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path

import cv2
import numpy as np


def _require_tool(name: str) -> str:
    path = shutil.which(name)
    if path is None:
        raise RuntimeError(
            f"{name} is required for play_frame_sampler but is not on PATH"
        )
    return path


def probe_duration_seconds(video_path: str | Path) -> float:
    """Use ffprobe to read container duration. Returns 0.0 when unknown
    rather than raising, so a malformed track produces no samples
    instead of taking the whole pipeline down."""
    ffprobe = _require_tool("ffprobe")
    result = subprocess.run(
        [
            ffprobe,
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-print_format",
            "json",
            str(video_path),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        return 0.0
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError:
        return 0.0
    duration = payload.get("format", {}).get("duration")
    try:
        return float(duration) if duration is not None else 0.0
    except (TypeError, ValueError):
        return 0.0


# Percentile timestamps for classification-time frame sampling. Spread
# across the recording so a single intro/outro doesn't dominate the
# evidence; a sprinkling outside the bookends catches mid-lecture state.
_DEFAULT_SAMPLE_PERCENTILES = (5, 15, 25, 40, 55, 70, 80, 90, 95)


def sample_frames(
    video_path: str | Path,
    n: int = 9,
    duration_seconds: float | None = None,
) -> list[np.ndarray]:
    """Pull `n` BGR frames from `video_path` at percentile timestamps.

    `duration_seconds` may be passed in if the caller already probed it
    (avoids a second ffprobe spawn per track). Returns frames in
    timestamp order; skips timestamps that ffmpeg fails to seek to so
    the count may be less than `n`.

    Uses input-side `-ss` for speed; classification doesn't need
    exact-keyframe precision and the speedup is significant on long
    lectures.
    """
    ffmpeg = _require_tool("ffmpeg")
    duration = duration_seconds or probe_duration_seconds(video_path)
    if duration <= 0:
        return []

    if n <= 0:
        return []

    # Pick percentile-distributed timestamps. Use the canonical 9 if n=9
    # exactly; otherwise spread `n` evenly over (5, 95).
    if n == len(_DEFAULT_SAMPLE_PERCENTILES):
        pct = _DEFAULT_SAMPLE_PERCENTILES
    else:
        pct = tuple(np.linspace(5, 95, n).tolist())
    timestamps = [duration * (p / 100.0) for p in pct]

    frames: list[np.ndarray] = []
    for t in timestamps:
        result = subprocess.run(
            [
                ffmpeg,
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                f"{t:.3f}",
                "-i",
                str(video_path),
                "-frames:v",
                "1",
                "-f",
                "image2pipe",
                "-vcodec",
                "png",
                "-",
            ],
            capture_output=True,
            check=False,
        )
        if result.returncode != 0 or not result.stdout:
            continue
        arr = np.frombuffer(result.stdout, dtype=np.uint8)
        decoded = cv2.imdecode(arr, cv2.IMREAD_COLOR)
        if decoded is None:
            continue
        frames.append(decoded)
    return frames


@dataclass
class CropBBox:
    """Same shape as `play_classifier.CropBBox`. Re-declared locally so
    this module doesn't import the classifier (keeps each script
    runnable in isolation when debugging)."""

    x: int
    y: int
    w: int
    h: int


def extract_frames_at_fps(
    video_path: str | Path,
    out_dir: str | Path,
    sample_fps: str = "1/5",
    crop: CropBBox | None = None,
) -> int:
    """Run ffmpeg to extract frames at `sample_fps`, optionally cropping
    to the slide region. Frames written as `f_00001.png`, `f_00002.png`,
    etc. Returns the count of files written.

    Output-side seek (no `-ss` before `-i`): accurate frame boundaries
    matter here because the timestamp -> frame index mapping is what
    the timeline JSON gets keyed on later.
    """
    ffmpeg = _require_tool("ffmpeg")
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    vf_chain = [f"fps={sample_fps}"]
    if crop is not None:
        vf_chain.append(f"crop={crop.w}:{crop.h}:{crop.x}:{crop.y}")

    pattern = str(out_dir / "f_%05d.png")
    result = subprocess.run(
        [
            ffmpeg,
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            str(video_path),
            "-vf",
            ",".join(vf_chain),
            pattern,
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"ffmpeg frame extraction failed: {result.stderr.strip()}"
        )
    return sum(1 for _ in out_dir.glob("f_*.png"))


def drop_blank_frames(
    frames_dir: str | Path,
    luma_std_threshold: float = 5.0,
    intensity_uniformity_threshold: float = 5.0,
) -> int:
    """Walk a frames directory and delete frames that are either nearly
    black/blank (low luma std-dev across the frame) or uniformly toned
    (low std-dev between channels). Returns the number of files deleted.

    This is the pre-filter described in the design plan: cuts ~30-50%
    of frames before they hit the GPU, since lecture intros, end cards,
    and fade transitions OCR to garbage anyway. Done on the GH worker
    rather than RunPod so we don't pay GPU cost per dropped frame.
    """
    frames_dir = Path(frames_dir)
    deleted = 0
    for path in sorted(frames_dir.glob("f_*.png")):
        img = cv2.imread(str(path), cv2.IMREAD_COLOR)
        if img is None:
            path.unlink(missing_ok=True)
            deleted += 1
            continue
        gray = cv2.cvtColor(img, cv2.COLOR_BGR2GRAY)
        if float(gray.std()) < luma_std_threshold:
            path.unlink(missing_ok=True)
            deleted += 1
            continue
        # Channel-uniformity check (test patterns / single-color fade frames).
        per_channel_std = float(np.std(img.mean(axis=(0, 1))))
        if per_channel_std < intensity_uniformity_threshold and float(gray.std()) < 30.0:
            path.unlink(missing_ok=True)
            deleted += 1
    return deleted
