"""Fetch one DSV Play lecture's slide video and transcript into runs/<uuid>/.

Runs on the operator's machine with SU_USERNAME / SU_PASSWORD in the
environment, so SU credentials never reach the cluster. Without --source it
saves each video source's poster to runs/<uuid>/sources/ to pick from.
"""

import argparse
import json
from pathlib import Path

from dsv_wrapper import PlayClient, TranscriptNotReadyError
from dsv_wrapper.parsers.play import enumerate_track_descriptors


def save_source_posters(client, presentation, out_dir):
    out_dir.mkdir(parents=True, exist_ok=True)
    print(presentation.title)
    for name, source in presentation.sources.items():
        line = f"  {name}: audio={source.play_audio}"
        if source.poster_url:
            response = client._client.get(source.poster_url, params={"token": presentation.token})
            response.raise_for_status()
            poster = out_dir / f"{name}.jpg"
            poster.write_bytes(response.content)
            line += f" poster={poster}"
        print(line)


def download_track(client, uuid, presentation, source, height, dest):
    descriptors = enumerate_track_descriptors(presentation)
    candidates = [(i, h) for i, (_, name, h) in enumerate(descriptors) if name == source]
    if not candidates:
        raise SystemExit(f"no source {source!r}; have {sorted(presentation.sources)}")
    index, _ = min(candidates, key=lambda c: abs((c[1] or 0) - height))
    if dest.exists():
        return
    dest.parent.mkdir(parents=True, exist_ok=True)
    partial = dest.with_suffix(".part")
    client.download_track(uuid, index, partial)
    partial.rename(dest)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("uuid", help="play.dsv.su.se presentation UUID")
    parser.add_argument("--source", help="video source holding the slides")
    parser.add_argument("--height", type=int, default=1080, help="preferred track height")
    parser.add_argument("--runs-dir", type=Path, default=Path("runs"))
    args = parser.parse_args()
    run_dir = args.runs_dir / args.uuid

    with PlayClient() as client:
        presentation = client.get_presentation(args.uuid)
        if not args.source:
            save_source_posters(client, presentation, run_dir / "sources")
            return
        download_track(client, args.uuid, presentation, args.source, args.height, run_dir / "video.mp4")
        try:
            cues = client.get_transcript(args.uuid)
        except TranscriptNotReadyError:
            cues = []

    transcript = [{"start": c.start_seconds, "end": c.end_seconds, "text": c.text} for c in cues]
    (run_dir / "transcript.json").write_text(json.dumps(transcript, ensure_ascii=False) + "\n")
    (run_dir / "meta.json").write_text(
        json.dumps({"title": presentation.title, "source": args.source}, ensure_ascii=False) + "\n"
    )
    print(f"{run_dir}: video.mp4, {len(transcript)} transcript cues")


if __name__ == "__main__":
    main()
