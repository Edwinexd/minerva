"""Authorize direct Play downloads. Only bounded probes and JSON use Actions.

Requires SU_USERNAME, SU_PASSWORD, MINERVA_API_URL, MINERVA_SERVICE_API_KEY.
Minerva stores the signed URL, expiry deadline and cues, then downloads
internally in deadline order. SU login credentials never leave this process.
"""

import time
from datetime import datetime, timezone

from _minerva_api import MinervaSession, minerva_session, su_credentials
from _play import extract_presentation_id
from _play_tracks import ProbeError, pick_track
from dsv_wrapper import PlayClient, TranscriptNotReadyError

API = "/api/service/visual-extraction"
FETCH_WAVE_SLOTS = 4
FETCH_RUN_SECONDS = 40 * 60
REFILL_HEADER = "X-Minerva-Staging-Refill"


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
        started = time.monotonic()
        download = pick_track(presentation)
        print(
            f"  probed {presentation_id} in {time.monotonic() - started:.1f}s",
            flush=True,
        )
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
        f"deadline={deadline.isoformat()}",
        flush=True,
    )


def fetch_slots(session: MinervaSession, attempted: set[str]) -> tuple[list, bool]:
    response = session.post(
        f"{API}/fetch-slots",
        json={
            "limit": FETCH_WAVE_SLOTS,
            "exclude_job_ids": sorted(attempted),
        },
    )
    response.raise_for_status()
    return response.json(), response.headers.get(REFILL_HEADER) == "1"


def authorize_window(session: MinervaSession, client: PlayClient) -> int:
    attempted = set()
    downloads = {}
    until = time.monotonic() + FETCH_RUN_SECONDS
    while time.monotonic() < until:
        # Once Minerva reads the selected source's headers, its reservation
        # shrinks to the actual size. Refill those released bytes in this run
        # instead of waiting for the next GitHub schedule. Do not retry a
        # failed source repeatedly just because it released its slot.
        slots, can_refill = fetch_slots(session, attempted)
        print(f"{len(slots)} fetch slot(s) granted", flush=True)
        if not slots:
            break
        for index, slot in enumerate(slots):
            if time.monotonic() >= until:
                for unused in slots[index:]:
                    release(session, unused["job_id"], "fetch run time budget", retry=True)
                return len(attempted)
            if slot["job_id"] in attempted:
                raise ValueError("duplicate fetch reservation")
            attempted.add(slot["job_id"])
            try:
                stage(session, client, slot, downloads)
            except Exception as error:
                # Third-party HTTP errors can include the signed URL; record
                # only the error class, never the credential-bearing message.
                # ProbeError messages are fixed text and say why it failed.
                detail = (
                    str(error) if type(error) is ProbeError else type(error).__name__
                )
                reason = f"Play authorization/probe failed ({detail})"
                print(f"  {slot['job_id']}: {reason}", flush=True)
                try:
                    release(session, slot["job_id"], reason, retry=True)
                except Exception:
                    print(
                        f"  {slot['job_id']}: release failed; reservation will expire"
                    )
        # During CD the new Actions script may reach the old API. Only refill
        # when the server supports header sizing and exclusions; otherwise
        # retries could reclaim the same failed source in this run.
        if not can_refill:
            break
    return len(attempted)


def main() -> None:
    session = minerva_session()
    username, password = su_credentials()
    with PlayClient(username, password) as client:
        attempted = authorize_window(session, client)
    print(f"{attempted} lecture(s) attempted", flush=True)


if __name__ == "__main__":
    main()
