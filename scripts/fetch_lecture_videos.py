"""Authorize direct Play downloads. Only bounded probes and JSON use Actions.

Requires SU_USERNAME, SU_PASSWORD, MINERVA_API_URL, MINERVA_SERVICE_API_KEY.
Minerva stores the signed URL, expiry deadline and cues, then downloads
internally in deadline order. SU login credentials never leave this process.
"""

import os
from datetime import datetime, timezone

from _minerva_api import MinervaSession, minerva_session, su_credentials
from _play import extract_presentation_id
from _play_tracks import pick_track
from dsv_wrapper import PlayClient, TranscriptNotReadyError

API = "/api/service/visual-extraction"


def release(session: MinervaSession, job_id: str, error: str, retry: bool) -> None:
    session.post(
        f"{API}/jobs/{job_id}/release", json={"error": error, "retry": retry}
    ).raise_for_status()


def stage(
    session: MinervaSession, client: PlayClient, slot: dict, downloads: dict
) -> None:
    job_id = slot["job_id"]
    presentation_id = extract_presentation_id(slot["url"])
    if not presentation_id:
        release(session, job_id, "no presentation id", retry=False)
        return
    cached = downloads.get(presentation_id)
    if cached and cached[0].expires_at - datetime.now(timezone.utc).timestamp() < 120:
        cached = None
    if cached:
        download, cues = cached
    else:
        try:
            cues = client.get_transcript(presentation_id)
        except TranscriptNotReadyError:
            release(session, job_id, "transcript not generated yet", retry=True)
            return
        presentation = client.get_presentation(presentation_id)
        if not presentation.is_video_ready:
            release(session, job_id, "presentation has no video track", retry=True)
            return
        download = pick_track(presentation)
        # The same lecture can belong to several courses. Probe it once per
        # run; refresh expired authorizations instead of repeating the frames.
        downloads[presentation_id] = (download, cues)
    deadline = datetime.fromtimestamp(download.expires_at, timezone.utc)
    if (deadline - datetime.now(timezone.utc)).total_seconds() < 120:
        raise ValueError("media token too close to expiry")
    session.post(
        f"{API}/jobs/{job_id}/download",
        json={
            "url": download.url,
            "download_deadline": deadline.isoformat(),
            "cues": [
                {"start": c.start_seconds, "end": c.end_seconds, "text": c.text}
                for c in cues
            ],
        },
    ).raise_for_status()
    print(
        f"  queued {presentation_id} source={download.source_name} "
        f"deadline={deadline.isoformat()}"
    )


def main() -> None:
    session = minerva_session()
    username, password = su_credentials()
    response = session.post(
        f"{API}/fetch-slots",
        json={
            "limit": int(os.environ.get("VISUAL_EXTRACTION_SLOTS", "10")),
        },
    )
    response.raise_for_status()
    slots = response.json()
    print(f"{len(slots)} fetch slot(s) granted")
    if not slots:
        return
    downloads = {}
    with PlayClient(username, password) as client:
        for slot in slots:
            try:
                stage(session, client, slot, downloads)
            except Exception as error:
                # Third-party HTTP errors can include the signed URL; record
                # only the error class, never the credential-bearing message.
                reason = f"Play authorization/probe failed ({type(error).__name__})"
                print(f"  {slot['job_id']}: {reason}")
                try:
                    release(session, slot["job_id"], reason, retry=True)
                except Exception:
                    print(
                        f"  {slot['job_id']}: release failed; reservation will expire"
                    )


if __name__ == "__main__":
    main()
