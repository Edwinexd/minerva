"""Stage Play lecture videos for the visual extraction pipeline.

Minerva hands out fetch slots only while its staging window has room, so
this never pulls more video than the prod node can hold. For each slot it
downloads the lecture's slide track and timed transcript cues from
play.dsv.su.se, uploads both, and moves on; a slot it cannot fill is
released (for a later run when the problem may pass, or for good).

Called by the GitHub Actions visual-extraction workflow.
Requires: SU_USERNAME, SU_PASSWORD, MINERVA_API_URL, MINERVA_SERVICE_API_KEY
Optional: VISUAL_EXTRACTION_SLOTS (slots to request per run, default 10)
"""

import os
import tempfile
from pathlib import Path

from dsv_wrapper import PlayClient, TranscriptNotReadyError
from dsv_wrapper.parsers.play import enumerate_track_descriptors

from _minerva_api import MinervaSession, minerva_session, su_credentials
from _play import extract_presentation_id

API = "/api/service/visual-extraction"
PREFERRED_HEIGHT = 1080
# Uploads of a few hundred MB over the runner's link; generous on purpose.
UPLOAD_TIMEOUT = 1800


def pick_track(presentation):
    """Index of the track that shows the slides, at the height closest to
    PREFERRED_HEIGHT. Studio recordings have one composite source; lecture
    halls add a screen capture next to the camera, and the capture is the
    source that does not carry the audio."""
    descriptors = enumerate_track_descriptors(presentation)
    silent = {name for name, source in presentation.sources.items() if not source.play_audio}
    candidates = [(i, name, height) for i, (_, name, height) in enumerate(descriptors)]
    if silent:
        candidates = [c for c in candidates if c[1] in silent] or candidates
    if not candidates:
        return None
    return min(candidates, key=lambda c: abs((c[2] or 0) - PREFERRED_HEIGHT))[0]


def release(session: MinervaSession, job_id: str, error: str, retry: bool) -> None:
    session.post(f"{API}/jobs/{job_id}/release", json={"error": error, "retry": retry}).raise_for_status()


def stage(session: MinervaSession, client: PlayClient, slot: dict, workdir: Path) -> None:
    job_id = slot["job_id"]
    presentation_id = extract_presentation_id(slot["url"])
    if not presentation_id:
        release(session, job_id, f"no presentation id in {slot['url']}", retry=False)
        return

    try:
        cues = client.get_transcript(presentation_id)
    except TranscriptNotReadyError:
        release(session, job_id, "transcript not generated yet", retry=True)
        return
    presentation = client.get_presentation(presentation_id)
    track = pick_track(presentation)
    if track is None:
        release(session, job_id, "presentation has no video track", retry=False)
        return

    video = workdir / f"{job_id}.mp4"
    client.download_track(presentation_id, track, video)
    try:
        with open(video, "rb") as body:
            session.put(f"{API}/jobs/{job_id}/video", data=body, timeout=UPLOAD_TIMEOUT).raise_for_status()
    finally:
        video.unlink(missing_ok=True)
    session.post(
        f"{API}/jobs/{job_id}/staged",
        json={"cues": [{"start": c.start_seconds, "end": c.end_seconds, "text": c.text} for c in cues]},
    ).raise_for_status()
    print(f"  staged {presentation_id} ({len(cues)} cues)")


def main() -> None:
    session = minerva_session()
    username, password = su_credentials()
    limit = int(os.environ.get("VISUAL_EXTRACTION_SLOTS", "10"))
    response = session.post(f"{API}/fetch-slots", json={"limit": limit})
    response.raise_for_status()
    slots = response.json()
    print(f"{len(slots)} fetch slot(s) granted")
    if not slots:
        return

    with PlayClient(username, password) as client, tempfile.TemporaryDirectory() as tmp:
        for slot in slots:
            try:
                stage(session, client, slot, Path(tmp))
            except Exception as e:
                print(f"  {slot['job_id']}: {e}")
                try:
                    release(session, slot["job_id"], str(e)[:500], retry=True)
                except Exception as release_error:
                    # The reservation expires on its own after two hours.
                    print(f"  {slot['job_id']}: could not release: {release_error}")


if __name__ == "__main__":
    main()
