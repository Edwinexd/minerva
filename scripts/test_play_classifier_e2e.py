"""End-to-end smoke test: synthetic mp4 tracks -> ffmpeg sampling ->
full classifier pipeline.

We don't have real DSV recordings to test against; this exercises the
glue between `play_frame_sampler` and `play_classifier` using mp4s
generated on the fly. The synthetic content is unsubtle (a clear slide
track and a clear cam track) so an off-by-one in any seam shows up
loudly. Production tuning still requires the labeled corpus.

Skipped automatically if ffmpeg isn't on PATH, so the rest of the test
suite remains runnable in environments without it.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

import cv2
import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import play_classifier as pc  # noqa: E402
import play_frame_sampler as pfs  # noqa: E402
from test_play_classifier import make_cam_frame, make_slide_frame  # noqa: E402


pytestmark = pytest.mark.skipif(
    shutil.which("ffmpeg") is None or shutil.which("ffprobe") is None,
    reason="ffmpeg/ffprobe not on PATH; e2e tests can't run",
)


# Synthetic videos are short (5 frames at 1 fps = 5s) to keep the test
# loop fast while still producing enough samples for percentile
# distribution to spread out without collapsing.
_VIDEO_DURATION_SECONDS = 5
_VIDEO_FPS = 1


def _write_synthetic_mp4(out_path: Path, frame_generator) -> None:
    """Render frames via the provided generator and mux to mp4 with
    ffmpeg's image2pipe. Avoids cv2.VideoWriter which has codec
    portability issues on minimal Linux setups."""
    out_path.parent.mkdir(parents=True, exist_ok=True)
    proc = subprocess.Popen(
        [
            "ffmpeg",
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "image2pipe",
            "-framerate",
            str(_VIDEO_FPS),
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-r",
            str(_VIDEO_FPS),
            str(out_path),
        ],
        stdin=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    assert proc.stdin is not None
    for i in range(_VIDEO_DURATION_SECONDS * _VIDEO_FPS):
        frame = frame_generator(i)
        ok, buf = cv2.imencode(".png", frame)
        assert ok
        proc.stdin.write(buf.tobytes())
    proc.stdin.close()
    rc = proc.wait()
    if rc != 0:
        stderr = proc.stderr.read().decode() if proc.stderr else ""
        raise RuntimeError(f"ffmpeg mux failed (rc={rc}): {stderr}")


def _slide_track_generator(seed: int):
    return lambda i: make_slide_frame(seed=seed * 100 + i, n_bullets=6)


def _cam_track_generator(seed: int):
    return lambda i: make_cam_frame(seed=seed * 100 + i)


@pytest.fixture(scope="module")
def slide_mp4(tmp_path_factory):
    path = tmp_path_factory.mktemp("synth") / "slide.mp4"
    _write_synthetic_mp4(path, _slide_track_generator(0))
    return path


@pytest.fixture(scope="module")
def cam_mp4(tmp_path_factory):
    path = tmp_path_factory.mktemp("synth") / "cam.mp4"
    _write_synthetic_mp4(path, _cam_track_generator(0))
    return path


def test_probe_duration_recognizes_synthetic_mp4(slide_mp4):
    duration = pfs.probe_duration_seconds(slide_mp4)
    # Allow ±1s slack: ffmpeg's container duration on short mp4s is
    # rarely exactly nominal because of frame timestamp rounding.
    assert abs(duration - _VIDEO_DURATION_SECONDS) < 1.5, duration


def test_sample_frames_returns_decodable_frames(slide_mp4):
    frames = pfs.sample_frames(slide_mp4, n=5)
    assert len(frames) >= 3, f"got too few frames: {len(frames)}"
    for f in frames:
        assert f.ndim == 3 and f.shape[2] == 3
        # Slide frames have a near-white background; mean should be high.
        assert f.mean() > 150, f.mean()


def test_full_pipeline_picks_slide_over_cam(slide_mp4, cam_mp4):
    """The whole hot path: probe each candidate, sample frames, build
    track evidence, classify. Expect the slide track to win."""
    candidates = [cam_mp4, slide_mp4]
    evidence = []
    for path in candidates:
        frames = pfs.sample_frames(path, n=5)
        assert len(frames) >= 3, f"insufficient samples from {path}"
        evidence.append(pc.aggregate_track(frames))

    result = pc.classify_tracks(evidence)
    assert result.selected_track_index == 1, result  # the slide track
    assert result.score is not None and result.score > 0


def test_extract_frames_at_fps_writes_files(slide_mp4, tmp_path):
    out_dir = tmp_path / "frames"
    n = pfs.extract_frames_at_fps(slide_mp4, out_dir, sample_fps="1")
    assert n >= 3, n
    pngs = list(out_dir.glob("f_*.png"))
    assert len(pngs) == n
    # Sanity: each is a decodable PNG with the slide-frame mean.
    for p in pngs[:3]:
        img = cv2.imread(str(p))
        assert img is not None
        assert img.mean() > 150


def test_drop_blank_frames_removes_black_frames(slide_mp4, tmp_path):
    out_dir = tmp_path / "frames"
    pfs.extract_frames_at_fps(slide_mp4, out_dir, sample_fps="1")
    # Inject a synthetic black frame to be dropped.
    blank = np.zeros((720, 1280, 3), dtype=np.uint8)
    cv2.imwrite(str(out_dir / "f_99999.png"), blank)
    before = len(list(out_dir.glob("f_*.png")))
    deleted = pfs.drop_blank_frames(out_dir)
    after = len(list(out_dir.glob("f_*.png")))
    assert deleted >= 1, deleted
    assert before - after == deleted
    # The injected blank frame must be among the deleted ones.
    assert not (out_dir / "f_99999.png").exists()


def test_extract_frames_at_fps_with_crop(slide_mp4, tmp_path):
    out_dir = tmp_path / "frames"
    crop = pfs.CropBBox(x=100, y=80, w=400, h=300)
    n = pfs.extract_frames_at_fps(
        slide_mp4, out_dir, sample_fps="1", crop=crop
    )
    assert n >= 3
    sample = next(iter(out_dir.glob("f_*.png")))
    img = cv2.imread(str(sample))
    assert img is not None
    assert img.shape[:2] == (crop.h, crop.w), img.shape
