"""Authorize direct Play downloads. Only headers and JSON use Actions.

Requires SU_USERNAME, SU_PASSWORD, MINERVA_API_URL, MINERVA_SERVICE_API_KEY.
Minerva stores each lecture's signed source URLs and sizes, the expiry
deadline and the cues; minerva-fetcher picks the slide source and downloads
it in deadline order. SU login credentials never leave this process.
"""

import time
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone

from _minerva_api import MinervaSession, minerva_session, su_credentials
from _play import extract_presentation_id
from _play_tracks import TrackError, describe_sources
from dsv_wrapper import PlayClient, TranscriptNotReadyError

API = "/api/service/visual-extraction"
FETCH_WAVE_SLOTS = 4
FETCH_RUN_SECONDS = 40 * 60
REFILL_HEADER = "X-Minerva-Staging-Refill"
# Minerva's codes for lectures that can never be staged.
PERMANENT_REJECTIONS = {"visual_extraction.video_too_large"}


class OfferRejected(Exception):
    """Minerva refused an offer; the message is its error code."""


def release(session: MinervaSession, job_id: str, error: str, retry: bool) -> None:
    session.post(
        f"{API}/jobs/{job_id}/release", json={"error": error, "retry": retry}
    ).raise_for_status()


def stage(
    session: MinervaSession, client: PlayClient, slot: dict, downloads: dict
) -> None:
    started = time.monotonic()
    job_id = slot["job_id"]
    presentation_id = extract_presentation_id(slot["url"])
    if not presentation_id:
        release(session, job_id, "no presentation id", retry=False)
        return
    cached = downloads.get(presentation_id)
    if cached and cached[1] - datetime.now(timezone.utc).timestamp() < 120:
        cached = None
    if cached:
        candidates, expires_at, cues = cached
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
        candidates, expires_at = describe_sources(presentation)
        # The same lecture can belong to several courses. Describe it once
        # per run; refresh expired authorizations.
        downloads[presentation_id] = (candidates, expires_at, cues)
    deadline = datetime.fromtimestamp(expires_at, timezone.utc)
    if (deadline - datetime.now(timezone.utc)).total_seconds() < 120:
        raise TrackError("media token too close to expiry")
    response = session.post(
        f"{API}/jobs/{job_id}/download",
        json={
            "candidates": candidates,
            "download_deadline": deadline.isoformat(),
            "cues": [
                {"start": c.start_seconds, "end": c.end_seconds, "text": c.text}
                for c in cues
            ],
        },
    )
    if response.status_code == 400:
        try:
            code = response.json().get("code", "")
        except ValueError:
            code = ""
        raise OfferRejected(code or "HTTP 400")
    response.raise_for_status()
    print(
        f"  queued {presentation_id} sources={len(candidates)} "
        f"deadline={deadline.isoformat()} in {time.monotonic() - started:.1f}s",
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


def attempt(session: MinervaSession, client: PlayClient, slot: dict, downloads: dict):
    try:
        stage(session, client, slot, downloads)
    except Exception as error:
        # Third-party HTTP errors can include the signed URL; record only the
        # error class, never the credential-bearing message. TrackError and
        # OfferRejected messages are fixed text and say why it failed.
        safe = type(error) in (TrackError, OfferRejected)
        detail = str(error) if safe else type(error).__name__
        reason = f"Play authorization failed ({detail})"
        print(f"  {slot['job_id']}: {reason}", flush=True)
        try:
            release(
                session,
                slot["job_id"],
                reason,
                retry=str(error) not in PERMANENT_REJECTIONS,
            )
        except Exception:
            print(f"  {slot['job_id']}: release failed; reservation will expire")


def authorize_window(session: MinervaSession, client: PlayClient) -> int:
    attempted = set()
    downloads = {}
    until = time.monotonic() + FETCH_RUN_SECONDS
    with ThreadPoolExecutor(FETCH_WAVE_SLOTS) as pool:
        while time.monotonic() < until:
            # Once Minerva has the sources' sizes, the reservation shrinks to
            # the largest. Refill those released bytes in this run instead of
            # waiting for the next GitHub schedule. Do not retry a failed
            # source repeatedly just because it released its slot.
            slots, can_refill = fetch_slots(session, attempted)
            print(f"{len(slots)} fetch slot(s) granted", flush=True)
            if not slots:
                break
            if time.monotonic() >= until:
                for unused in slots:
                    release(session, unused["job_id"], "fetch run time budget", retry=True)
                break
            first_wave = not attempted
            for slot in slots:
                if slot["job_id"] in attempted:
                    raise ValueError("duplicate fetch reservation")
                attempted.add(slot["job_id"])
            # Play logs in on first use, so one lecture goes alone; the rest
            # share the session in parallel.
            if first_wave:
                attempt(session, client, slots[0], downloads)
                slots = slots[1:]
            list(pool.map(lambda slot: attempt(session, client, slot, downloads), slots))
            # During CD the new Actions script may reach the old API. Only
            # refill when the server supports sizing and exclusions; otherwise
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
