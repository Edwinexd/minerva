"""CLI driver: classify a real play.dsv presentation's tracks.

Pulls every candidate mp4 via dsv-wrapper, samples frames with ffmpeg,
runs the visual classifier, and prints a JSON report of per-track
evidence + the chosen slide track. Use this to validate the classifier
against real DSV data before wiring it into the production GH workflow.

Usage:
    python -m scripts.classify_play_presentation <presentation_uuid> [--keep-frames]

Required environment:
    SU_USERNAME, SU_PASSWORD - Shibboleth credentials for play.dsv.

Required tooling on PATH:
    ffmpeg, ffprobe.

This driver is intentionally NOT what production runs (the production
GH worker will pre-filter, bundle, and POST to Minerva). It exists so a
developer can paste a UUID, see scores, and iterate on classifier
weights without spinning up a workflow.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import tempfile
from dataclasses import asdict
from pathlib import Path

import numpy as np

# Allow running as a script (`python scripts/classify_play_presentation.py`)
# or as a module (`python -m scripts.classify_play_presentation`).
sys.path.insert(0, str(Path(__file__).resolve().parent))

import play_classifier as pc
import play_frame_sampler as pfs
import play_ocr


# Sample frames per candidate used by the OCR tiebreaker. 5 is a
# compromise: enough text for the char-count signal to be meaningful
# even when one frame happens to be a transition, but few enough that
# the tiebreak doesn't dominate runtime when it fires.
_TIEBREAK_FRAMES_PER_TRACK = 5

# A candidate whose total OCR'd character count is below this is
# treated as "no readable slides", and if EVERY candidate is below it
# we report slide_track_missing.
_TIEBREAK_MIN_CHARS = 50


def _classify(presentation_uuid: str, keep_frames: bool) -> dict:
    try:
        from dsv_wrapper import PlayClient  # type: ignore
    except ImportError as e:
        raise SystemExit(
            "dsv-wrapper >= 0.2 is required. "
            "Install with: pip install -r scripts/requirements-play-ingest.txt "
            f"(import error: {e})"
        ) from e

    user = os.environ.get("SU_USERNAME")
    pw = os.environ.get("SU_PASSWORD")
    if not user or not pw:
        raise SystemExit(
            "SU_USERNAME and SU_PASSWORD must be set in the environment."
        )

    workdir = Path(tempfile.mkdtemp(prefix="play-classify-"))
    cleanup = not keep_frames
    try:
        with PlayClient(username=user, password=pw) as client:
            tracks = client.get_media_tracks(presentation_uuid)
            if not tracks:
                raise SystemExit(
                    f"presentation {presentation_uuid} has no media tracks "
                    "(or dsv-wrapper returned an empty list - is the UUID correct?)"
                )

            evidence_per_track = []
            track_meta = []
            for track in tracks:
                idx = track.index
                dest = workdir / f"track_{idx}.mp4"
                print(
                    f"  downloading track {idx} "
                    f"({track.height or '?'}p, "
                    f"{(track.size_bytes or 0) / 1_000_000:.0f} MB)...",
                    file=sys.stderr,
                )
                client.download_track(presentation_uuid, idx, str(dest))

                duration = pfs.probe_duration_seconds(dest)
                frames = pfs.sample_frames(dest, n=9, duration_seconds=duration)
                print(
                    f"    sampled {len(frames)} frames, duration {duration:.1f}s",
                    file=sys.stderr,
                )
                if not frames:
                    print(
                        f"    skipping track {idx}: no decodable frames",
                        file=sys.stderr,
                    )
                    continue
                ev = pc.aggregate_track(frames)
                evidence_per_track.append(ev)
                track_meta.append(
                    {
                        "index": idx,
                        "height": track.height,
                        "size_bytes": track.size_bytes,
                        "mime_type": track.mime_type,
                        "duration_seconds": duration,
                    }
                )

        if not evidence_per_track:
            raise SystemExit("no track produced usable frames")

        result = pc.classify_tracks(evidence_per_track)

        # OCR tiebreak: when the visual classifier isn't confident, OCR
        # a small batch of sample frames per candidate and let
        # whichever track has the most readable text win. If every
        # candidate is below MIN_CHARS, treat as slide_track_missing.
        tiebreak_report: dict | None = None
        if result.needs_tiebreak and len(evidence_per_track) >= 2:
            print(
                "  visual scores ambiguous - running OCR tiebreak...",
                file=sys.stderr,
            )
            ocr_runner = play_ocr.make_ocr_runner()
            sample_frames_per_track = []
            for meta in track_meta:
                track_path = workdir / f"track_{meta['index']}.mp4"
                tb_frames = pfs.sample_frames(
                    track_path,
                    n=_TIEBREAK_FRAMES_PER_TRACK,
                    duration_seconds=meta["duration_seconds"],
                )
                sample_frames_per_track.append(tb_frames)

            char_counts = []
            for frames in sample_frames_per_track:
                total = sum(len(ocr_runner(f)) for f in frames)
                char_counts.append(total)
            print(f"  OCR char counts per track: {char_counts}", file=sys.stderr)

            best_count = max(char_counts) if char_counts else 0
            if best_count < _TIEBREAK_MIN_CHARS:
                # Every candidate is below the readable-slides floor.
                # Override the visual pick with a slide_track_missing
                # signal so the GH worker bounces this presentation to
                # the transcript-only fallback path.
                result = pc.ClassificationResult(
                    selected_track_index=None,
                    score=result.score,
                    runner_up_score=result.runner_up_score,
                    needs_tiebreak=False,
                    all_scores=result.all_scores,
                )
            else:
                # Override selected_track_index with the OCR winner.
                # The structured-output downstream consumers don't see
                # all_scores, just selected_track_index, so this is the
                # right place to let OCR have the final word.
                ocr_winner = int(np.argmax(np.array(char_counts)))
                result = pc.ClassificationResult(
                    selected_track_index=ocr_winner,
                    score=result.score,
                    runner_up_score=result.runner_up_score,
                    needs_tiebreak=False,
                    all_scores=result.all_scores,
                )
            tiebreak_report = {
                "fired": True,
                "char_counts": char_counts,
                "min_chars_threshold": _TIEBREAK_MIN_CHARS,
                "result": (
                    "slide_track_missing"
                    if result.selected_track_index is None
                    else f"ocr_picked_track_{result.selected_track_index}"
                ),
            }

        # PIP detection runs on the chosen track's frames; re-sample
        # densely for the temporal-std signal it depends on.
        crop = None
        if result.selected_track_index is not None:
            chosen_meta = track_meta[result.selected_track_index]
            chosen_path = workdir / f"track_{chosen_meta['index']}.mp4"
            pip_frames = pfs.sample_frames(
                chosen_path, n=20, duration_seconds=chosen_meta["duration_seconds"]
            )
            if len(pip_frames) >= 4:
                crop_bbox = pc.detect_pip_crop(pip_frames)
                if crop_bbox is not None:
                    crop = {
                        "x": crop_bbox.x,
                        "y": crop_bbox.y,
                        "w": crop_bbox.w,
                        "h": crop_bbox.h,
                    }

        return {
            "presentation_uuid": presentation_uuid,
            "tracks": [
                {**meta, "evidence": asdict(ev)}
                for meta, ev in zip(track_meta, evidence_per_track)
            ],
            "scores": [
                {
                    "track_index": s.track_index,
                    "score": s.score,
                }
                for s in result.all_scores
            ],
            "selected_track_index": result.selected_track_index,
            "score": result.score,
            "runner_up_score": result.runner_up_score,
            "needs_tiebreak": result.needs_tiebreak,
            "tiebreak": tiebreak_report,
            "crop_bbox": crop,
            "workdir": str(workdir) if not cleanup else None,
        }
    finally:
        if cleanup:
            shutil.rmtree(workdir, ignore_errors=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "presentation_uuid",
        help="play.dsv presentation UUID (the trailing path segment of a "
        "play.dsv.su.se/presentation/<uuid> URL).",
    )
    parser.add_argument(
        "--keep-frames",
        action="store_true",
        help="Don't delete the temp workdir on exit; useful for inspecting "
        "downloaded mp4s and the sampled frames manually.",
    )
    args = parser.parse_args()

    report = _classify(args.presentation_uuid, keep_frames=args.keep_frames)
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
