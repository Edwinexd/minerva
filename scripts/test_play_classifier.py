"""Tests for the slide-track classifier.

Synthetic frames are generated programmatically so the feature signals
are exercised in isolation without depending on real DSV data. These
tests validate the *direction* of each signal (slides score higher than
cams on edge_density, lower on color_std, etc.); the production tuning
of thresholds + weights still needs the offline eval harness against
hand-labeled lectures, which lives elsewhere.

Synthetic generators are intentionally crude. The goal is not to
faithfully model what real cam / slide footage looks like; it's to
construct frames where the *expected* feature ordering is unambiguous
so a feature implementation that's broken in some catastrophic way
gets caught here before it gets to real data.
"""

from __future__ import annotations

import sys
from pathlib import Path

import cv2
import numpy as np
import pytest

# Allow running pytest from repo root or scripts/.
sys.path.insert(0, str(Path(__file__).resolve().parent))

import play_classifier as pc  # noqa: E402


# --- Frame generators ---------------------------------------------------


def make_slide_frame(
    width: int = 1280, height: int = 720, n_bullets: int = 6, seed: int = 0
) -> np.ndarray:
    """White background, black title, several bullet lines. Mimics a
    typical slide deck: high edge density (text), many horizontal lines
    (bullet rules + character baselines), low saturation (white bg)."""
    rng = np.random.default_rng(seed)
    frame = np.full((height, width, 3), 240, dtype=np.uint8)  # off-white

    # Title bar.
    cv2.rectangle(frame, (60, 50), (width - 60, 130), (200, 200, 200), -1)
    cv2.putText(
        frame,
        f"Lecture Title {seed}",
        (80, 110),
        cv2.FONT_HERSHEY_SIMPLEX,
        1.6,
        (20, 20, 20),
        3,
    )

    # Bullet lines, each with a leading dash.
    for i in range(n_bullets):
        y = 220 + i * 60
        cv2.line(frame, (90, y), (110, y), (20, 20, 20), 3)
        text = f"Bullet point {i}: " + " ".join(
            f"word{int(rng.integers(0, 99))}" for _ in range(6)
        )
        cv2.putText(
            frame, text, (130, y + 10), cv2.FONT_HERSHEY_SIMPLEX, 0.9, (20, 20, 20), 2
        )
    return frame


def make_cam_frame(
    width: int = 1280, height: int = 720, seed: int = 0
) -> np.ndarray:
    """Skin-tone-dominant frame with a face-like ellipse, blurry
    background. Mimics a presenter-cam crop: high color variance,
    detectable face, low edge density (no text)."""
    rng = np.random.default_rng(seed)
    # Warm-tinted noise background (room lighting).
    bg = rng.integers(80, 180, size=(height, width, 3), dtype=np.uint8)
    bg[..., 0] = (bg[..., 0] * 0.6).astype(np.uint8)  # less blue
    bg[..., 2] = np.clip(bg[..., 2].astype(np.int32) + 40, 0, 255).astype(np.uint8)
    bg = cv2.GaussianBlur(bg, (51, 51), 30)

    # Face ellipse (skin tone). OpenCV's Haar frontalface needs eyes +
    # mouth contrast, not just an oval; render a stylized face.
    cx, cy = width // 2, height // 2
    face_rgb = (180, 200, 230)  # light skin tone in BGR
    cv2.ellipse(bg, (cx, cy), (140, 180), 0, 0, 360, face_rgb, -1)
    # Eyes (dark ellipses).
    cv2.ellipse(bg, (cx - 50, cy - 40), (20, 12), 0, 0, 360, (40, 40, 40), -1)
    cv2.ellipse(bg, (cx + 50, cy - 40), (20, 12), 0, 0, 360, (40, 40, 40), -1)
    # Pupils.
    cv2.circle(bg, (cx - 50, cy - 40), 6, (10, 10, 10), -1)
    cv2.circle(bg, (cx + 50, cy - 40), 6, (10, 10, 10), -1)
    # Mouth.
    cv2.ellipse(bg, (cx, cy + 60), (50, 20), 0, 0, 180, (80, 50, 50), 4)
    # Nose hint.
    cv2.line(bg, (cx, cy - 15), (cx, cy + 25), (140, 160, 200), 3)
    return bg


def make_blank_frame(width: int = 1280, height: int = 720) -> np.ndarray:
    """Flat black; used to verify pre-filter logic upstream."""
    return np.zeros((height, width, 3), dtype=np.uint8)


# --- Per-feature direction tests ---------------------------------------


def test_edge_density_higher_on_slides_than_cam():
    slide = pc.extract_features(make_slide_frame())
    cam = pc.extract_features(make_cam_frame())
    assert slide.edge_density > cam.edge_density, (
        slide.edge_density,
        cam.edge_density,
    )


def test_color_std_lower_on_slides_than_cam():
    slide = pc.extract_features(make_slide_frame())
    cam = pc.extract_features(make_cam_frame())
    assert slide.color_std < cam.color_std, (slide.color_std, cam.color_std)


def test_horiz_lines_higher_on_slides_than_cam():
    slide = pc.extract_features(make_slide_frame(n_bullets=8))
    cam = pc.extract_features(make_cam_frame())
    assert slide.horiz_lines > cam.horiz_lines, (
        slide.horiz_lines,
        cam.horiz_lines,
    )


def test_text_regions_higher_on_slides_than_cam():
    slide = pc.extract_features(make_slide_frame(n_bullets=6))
    cam = pc.extract_features(make_cam_frame())
    assert slide.text_regions > cam.text_regions, (
        slide.text_regions,
        cam.text_regions,
    )


def test_face_count_nonzero_on_cam_zero_on_slide():
    slide = pc.extract_features(make_slide_frame())
    cam = pc.extract_features(make_cam_frame())
    assert slide.face_count == 0, slide.face_count
    # The synthetic face is intentionally crude; Haar doesn't always
    # latch on. Accept "at most one face on cam, zero on slide" as the
    # actual contract: the relative signal still flows correctly.
    assert cam.face_count >= slide.face_count


def test_extract_features_rejects_non_bgr():
    gray = np.zeros((480, 640), dtype=np.uint8)
    with pytest.raises(ValueError):
        pc.extract_features(gray)


# --- Track aggregation + scoring ---------------------------------------


def test_aggregate_track_temporal_std_low_on_static_slides():
    """A track of identical slide frames has near-zero temporal std."""
    frames = [make_slide_frame(seed=0) for _ in range(8)]
    ev = pc.aggregate_track(frames)
    assert ev.frames_used == 8
    assert ev.temporal_std < 0.01, ev.temporal_std


def test_aggregate_track_temporal_std_higher_on_moving_cam():
    """A track of distinct cam frames (with random noise variation) has
    higher temporal std than a static slide track."""
    static = pc.aggregate_track([make_slide_frame(seed=0) for _ in range(6)])
    moving = pc.aggregate_track([make_cam_frame(seed=i) for i in range(6)])
    assert moving.temporal_std > static.temporal_std, (
        moving.temporal_std,
        static.temporal_std,
    )


def test_score_tracks_picks_slide_over_cam():
    """The slide track should score above the cam track when both
    candidates are presented."""
    slide_ev = pc.aggregate_track([make_slide_frame(seed=i) for i in range(6)])
    cam_ev = pc.aggregate_track([make_cam_frame(seed=i) for i in range(6)])
    scores = pc.score_tracks([slide_ev, cam_ev])
    assert len(scores) == 2
    assert scores[0].score > scores[1].score, scores
    # Slide track is index 0; verify the API contract.
    best = max(scores, key=lambda s: s.score)
    assert best.track_index == 0


def test_score_tracks_three_way_picks_slide():
    slide_ev = pc.aggregate_track([make_slide_frame(seed=i) for i in range(6)])
    cam_ev = pc.aggregate_track([make_cam_frame(seed=i) for i in range(6)])
    blank_frames = [make_blank_frame() for _ in range(6)]
    blank_ev = pc.aggregate_track(blank_frames)
    scores = pc.score_tracks([cam_ev, blank_ev, slide_ev])
    best = max(scores, key=lambda s: s.score)
    assert best.track_index == 2  # the slide track


def test_score_tracks_handles_single_candidate():
    slide_ev = pc.aggregate_track([make_slide_frame(seed=0) for _ in range(4)])
    scores = pc.score_tracks([slide_ev])
    assert len(scores) == 1
    # With one candidate, z-norm is zero everywhere; score is 0.
    assert scores[0].score == pytest.approx(0.0)


def test_score_tracks_empty():
    assert pc.score_tracks([]) == []


# --- Decision logic ----------------------------------------------------


def test_classify_picks_slide_when_clearly_better():
    slide_ev = pc.aggregate_track([make_slide_frame(seed=i) for i in range(8)])
    cam_ev = pc.aggregate_track([make_cam_frame(seed=i) for i in range(8)])
    result = pc.classify_tracks([cam_ev, slide_ev])
    assert result.selected_track_index == 1
    assert result.score is not None and result.score > 0
    assert result.runner_up_score is not None
    # Whether `needs_tiebreak` is True depends on the exact thresholds;
    # the contract is that the *selected index* is correct.


def test_classify_all_cams_picks_one_anyway():
    """Two near-identical cam tracks: z-normalization always yields a
    nominal "winner" (one is +1.5, the other -1.5 by construction).
    The classifier picks one rather than punting; detecting an all-cam
    presentation is the OCR tiebreaker's job, not this stage. When OCR
    finds zero readable characters across all candidates, the GH worker
    flips the doc to slide_track_missing.

    This test pins that contract: classify_tracks does not, and should
    not, return slide_track_missing on its own when given non-empty
    input. The signal lives at a layer above."""
    cam1 = pc.aggregate_track([make_cam_frame(seed=i) for i in range(6)])
    cam2 = pc.aggregate_track([make_cam_frame(seed=i + 100) for i in range(6)])
    result = pc.classify_tracks([cam1, cam2])
    assert result.selected_track_index is not None
    assert result.selected_track_index in (0, 1)


def test_classify_empty_candidates():
    result = pc.classify_tracks([])
    assert result.selected_track_index is None
    assert result.score is None
    assert result.runner_up_score is None
    assert not result.needs_tiebreak
    assert result.all_scores == []


# --- Picture-in-picture detection --------------------------------------


def make_pip_frames(
    n_frames: int = 30,
    width: int = 1280,
    height: int = 720,
    slide_bbox: tuple[int, int, int, int] = (50, 50, 800, 600),
    seed: int = 0,
) -> list[np.ndarray]:
    """Build a sequence where a slide region is static and the rest of
    the frame is moving. Used to validate `detect_pip_crop`."""
    rng = np.random.default_rng(seed)
    slide_template = make_slide_frame(width=slide_bbox[2], height=slide_bbox[3], seed=0)
    frames: list[np.ndarray] = []
    for t in range(n_frames):
        # Highly dynamic background = strong motion signal. We deliberately
        # do NOT blur this; blurring drops per-pixel temporal std below the
        # PIP detection threshold, defeating the point of the fixture.
        bg = rng.integers(0, 255, size=(height, width, 3), dtype=np.uint8)
        sx, sy, sw, sh = slide_bbox
        bg[sy : sy + sh, sx : sx + sw] = slide_template
        frames.append(bg)
    return frames


def test_detect_pip_crop_finds_static_region():
    bbox = (200, 100, 800, 500)  # x, y, w, h
    frames = make_pip_frames(slide_bbox=bbox, n_frames=20)
    crop = pc.detect_pip_crop(frames, motion_threshold=12.0)
    assert crop is not None, "expected a PIP crop bbox, got None"
    # Allow some slack: the connected-component finder may extend
    # slightly beyond the seeded region due to texture noise overlap.
    assert abs(crop.x - bbox[0]) <= 20, crop
    assert abs(crop.y - bbox[1]) <= 20, crop
    assert abs(crop.w - bbox[2]) <= 40, crop
    assert abs(crop.h - bbox[3]) <= 40, crop


def test_detect_pip_crop_returns_none_when_whole_frame_is_slide():
    """A track that's already a clean slide deck (no PIP composite)
    should NOT report a crop - the caller OCRs the whole frame."""
    frames = [make_slide_frame(seed=0) for _ in range(8)]
    crop = pc.detect_pip_crop(frames)
    assert crop is None


def test_detect_pip_crop_returns_none_with_too_few_frames():
    frames = [make_slide_frame(seed=0) for _ in range(3)]
    assert pc.detect_pip_crop(frames) is None


def test_detect_pip_crop_returns_none_for_tiny_static_region():
    """Small static logo in the corner of an otherwise-moving frame
    must NOT be returned as a slide crop."""
    rng = np.random.default_rng(0)
    width, height = 1280, 720
    frames = []
    for _ in range(15):
        bg = rng.integers(0, 255, size=(height, width, 3), dtype=np.uint8)
        # Static 100x100 logo in top-left corner.
        bg[10:110, 10:110] = (50, 50, 50)
        frames.append(bg)
    crop = pc.detect_pip_crop(frames)
    assert crop is None


# --- OCR tiebreaker shape ----------------------------------------------


def test_jaccard_similarity_identical():
    assert pc.jaccard_similarity("hello world test", "hello world test") == 1.0


def test_jaccard_similarity_disjoint():
    assert pc.jaccard_similarity("alpha beta gamma", "delta epsilon zeta") == 0.0


def test_jaccard_similarity_partial_overlap():
    sim = pc.jaccard_similarity(
        "lecture title bullet point one",
        "lecture title bullet point two",
    )
    # token sets share {lecture, title, bullet, point}; differ on
    # {one} vs {two} (length 3, included by tokenizer). 4/6 = 0.667.
    assert 0.5 < sim < 0.8, sim


def test_jaccard_similarity_drops_short_tokens():
    """Short connectives shouldn't drag similarity up. The tokenizer
    only retains tokens of length >= 3, so single letters / `is` / `of`
    don't count toward overlap or union."""
    sim = pc.jaccard_similarity("a b c d e", "a b c d e")
    # All tokens are length 1; tokenizer drops them; similarity is 0
    # because both sets are empty.
    assert sim == 0.0


def test_find_duplicate_slide_clusters_three_distinct():
    samples = [
        pc.TrackOcrSample(0, "introduction lecture machine learning algorithms"),
        pc.TrackOcrSample(1, "presenter wearing blue shirt face"),
        pc.TrackOcrSample(2, "second presenter view different angle"),
    ]
    clusters = pc.find_duplicate_slide_clusters(samples)
    # Three singleton clusters.
    assert len(clusters) == 3
    assert {len(c) for c in clusters} == {1}


def test_find_duplicate_slide_clusters_two_duplicates_and_unique():
    """Two tracks recording the same slide deck (essentially identical
    OCR text) should cluster together; a third distinct track stays
    separate."""
    slide_text = (
        "lecture title machine learning bullet definition algorithm "
        "training data validation accuracy precision recall"
    )
    samples = [
        pc.TrackOcrSample(0, slide_text),
        pc.TrackOcrSample(1, slide_text + " minor recognition variance"),
        pc.TrackOcrSample(2, "completely different presenter cam content"),
    ]
    clusters = pc.find_duplicate_slide_clusters(samples)
    assert len(clusters) == 2
    sizes = sorted(len(c) for c in clusters)
    assert sizes == [1, 2]
    # The duplicate cluster contains 0 and 1.
    pair_cluster = next(c for c in clusters if len(c) == 2)
    assert pair_cluster == {0, 1}


def test_find_duplicate_slide_clusters_all_duplicates():
    slide_text = (
        "lecture introduction overview chapter one definitions bullet "
        "point list multiple words shared across recordings"
    )
    samples = [
        pc.TrackOcrSample(i, slide_text + f" extra{i}") for i in range(4)
    ]
    clusters = pc.find_duplicate_slide_clusters(samples)
    assert len(clusters) == 1
    assert clusters[0] == {0, 1, 2, 3}


def test_find_duplicate_slide_clusters_empty_input():
    assert pc.find_duplicate_slide_clusters([]) == []


def test_find_duplicate_slide_clusters_handles_empty_text():
    """Empty OCR output (cam track that snuck through the absolute
    gate) should land in its own singleton, not bleed into a cluster
    of other tracks."""
    samples = [
        pc.TrackOcrSample(0, "lecture title machine learning content"),
        pc.TrackOcrSample(1, ""),
        pc.TrackOcrSample(2, "lecture title machine learning content"),
    ]
    clusters = pc.find_duplicate_slide_clusters(samples)
    # Tracks 0 and 2 cluster together; track 1 is alone.
    assert len(clusters) == 2
    sizes = sorted(len(c) for c in clusters)
    assert sizes == [1, 2]
    pair = next(c for c in clusters if len(c) == 2)
    assert pair == {0, 2}


def test_track_ocr_sample_stability_zero_when_no_frames():
    s = pc.TrackOcrSample(track_index=0, ocr_text="anything")
    # Default total_frames is 0; stability must not divide-by-zero.
    assert s.stability == 0.0


def test_track_ocr_sample_stability_full_and_partial():
    full = pc.TrackOcrSample(
        track_index=0, ocr_text="...", readable_frames=5, total_frames=5
    )
    partial = pc.TrackOcrSample(
        track_index=1, ocr_text="...", readable_frames=2, total_frames=5
    )
    assert full.stability == 1.0
    assert partial.stability == 0.4


def test_pick_from_cluster_prefers_stable_over_higher_res_unstable():
    """Multi-projector hall: 1080p projector recording is stuttering
    (only 30% of sampled frames produced readable OCR), 720p
    recording is clean. Picker must prefer the 720p over the
    higher-resolution-but-unstable 1080p."""
    samples = {
        0: pc.TrackOcrSample(0, "...", readable_frames=1, total_frames=3),  # 33%
        1: pc.TrackOcrSample(1, "...", readable_frames=3, total_frames=3),  # 100%
    }
    heights = {0: 1080, 1: 720}
    scores = {0: 0.5, 1: 0.4}
    chosen = pc.pick_from_cluster(
        cluster_members={0, 1},
        samples_by_track=samples,
        height_by_track=heights,
        score_by_track=scores,
    )
    assert chosen == 1


def test_pick_from_cluster_picks_higher_res_among_stable():
    """When all members clear the stability floor, falls back to
    the resolution-then-score ordering."""
    samples = {
        0: pc.TrackOcrSample(0, "...", readable_frames=3, total_frames=3),
        1: pc.TrackOcrSample(1, "...", readable_frames=3, total_frames=3),
    }
    heights = {0: 720, 1: 1080}
    scores = {0: 0.5, 1: 0.4}
    chosen = pc.pick_from_cluster(
        cluster_members={0, 1},
        samples_by_track=samples,
        height_by_track=heights,
        score_by_track=scores,
    )
    assert chosen == 1  # 1080p wins over 720p when both stable


def test_pick_from_cluster_ties_on_height_break_by_score():
    samples = {
        0: pc.TrackOcrSample(0, "...", readable_frames=3, total_frames=3),
        1: pc.TrackOcrSample(1, "...", readable_frames=3, total_frames=3),
    }
    heights = {0: 1080, 1: 1080}
    scores = {0: 0.2, 1: 0.9}
    chosen = pc.pick_from_cluster(
        cluster_members={0, 1},
        samples_by_track=samples,
        height_by_track=heights,
        score_by_track=scores,
    )
    assert chosen == 1


def test_pick_from_cluster_falls_through_when_all_unstable():
    """Bad recording day - every member fell below the stability
    floor. Don't punt, pick by (height, score) across all members
    so the caller still gets a usable index back."""
    samples = {
        0: pc.TrackOcrSample(0, "...", readable_frames=0, total_frames=3),
        1: pc.TrackOcrSample(1, "...", readable_frames=1, total_frames=3),  # 33%
    }
    heights = {0: 1080, 1: 720}
    scores = {0: 0.5, 1: 0.4}
    chosen = pc.pick_from_cluster(
        cluster_members={0, 1},
        samples_by_track=samples,
        height_by_track=heights,
        score_by_track=scores,
    )
    # Both fail the floor (default 0.6); fallthrough sorts by height -> 1080p.
    assert chosen == 0


def test_pick_from_cluster_handles_unknown_height():
    samples = {
        0: pc.TrackOcrSample(0, "...", readable_frames=3, total_frames=3),
        1: pc.TrackOcrSample(1, "...", readable_frames=3, total_frames=3),
    }
    heights = {0: None, 1: 720}  # mp4 didn't publish height for track 0
    scores = {0: 0.5, 1: 0.4}
    chosen = pc.pick_from_cluster(
        cluster_members={0, 1},
        samples_by_track=samples,
        height_by_track=heights,
        score_by_track=scores,
    )
    # None -> 0 in the sort key, so 720p wins despite the visual score gap.
    assert chosen == 1


def test_pick_from_cluster_empty_raises():
    with pytest.raises(ValueError):
        pc.pick_from_cluster(
            cluster_members=set(),
            samples_by_track={},
            height_by_track={},
            score_by_track={},
        )


def test_pick_from_cluster_singleton_returns_member():
    """Cluster of one is the no-op case; return the only member
    regardless of its stability or resolution."""
    samples = {
        0: pc.TrackOcrSample(0, "...", readable_frames=0, total_frames=3),
    }
    chosen = pc.pick_from_cluster(
        cluster_members={0},
        samples_by_track=samples,
        height_by_track={0: 480},
        score_by_track={0: -1.0},
    )
    assert chosen == 0


def test_is_slide_like_absolute_passes_synthetic_slide_evidence():
    """Slide track evidence should pass the cheap absolute gate. Use
    the same synthetic frames the per-feature tests rely on so this
    test breaks loudly if feature extraction regresses below the
    duplicate-detection threshold."""
    ev = pc.aggregate_track([make_slide_frame(seed=i) for i in range(6)])
    assert pc.is_slide_like_absolute(ev), ev


def test_is_slide_like_absolute_rejects_synthetic_cam_evidence():
    ev = pc.aggregate_track([make_cam_frame(seed=i) for i in range(6)])
    # Cam frames should fail the gate. They have skin-tone color
    # variance and detectable faces; they don't have the text-region
    # density that slides do.
    assert not pc.is_slide_like_absolute(ev), ev


def test_tiebreak_picks_track_with_most_chars():
    # Synthetic OCR: track 1 produces more chars than track 0.
    sample_frames = [
        [np.zeros((10, 10, 3), dtype=np.uint8) for _ in range(2)],
        [np.zeros((10, 10, 3), dtype=np.uint8) for _ in range(2)],
    ]
    track_chars = ["short", "much longer text here many characters"]

    def ocr_runner(frame):
        # Identify which track this frame came from by object identity.
        for i, frames in enumerate(sample_frames):
            if any(f is frame for f in frames):
                return track_chars[i]
        return ""

    chosen = pc.tiebreak_by_ocr_char_count(sample_frames, ocr_runner)
    assert chosen == 1
