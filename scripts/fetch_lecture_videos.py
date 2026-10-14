"""Authorize direct Play downloads. Only headers and JSON use Actions.

Requires SU_USERNAME, SU_PASSWORD, MINERVA_API_URL, MINERVA_SERVICE_API_KEY.
Minerva stores each lecture's signed source URLs and sizes, the expiry
deadline and the cues; minerva-fetcher picks the slide source and downloads
it in deadline order. SU login credentials never leave this process.
"""

import time
from collections import deque
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from datetime import datetime, timezone

from _minerva_api import MinervaSession, minerva_session, su_credentials
from _play import extract_presentation_id
from _play_tracks import TrackError, describe_sources
from dsv_wrapper import PlayClient, TranscriptNotReadyError

API = "/api/service/visual-extraction"
# Lectures being described at once: each is about three Play page calls,
# so this is the load on Play. Granted slots beyond those wait in a queue so
# a finished lecture is replaced at once, without a round trip to Minerva.
FETCH_CONCURRENCY = 8
FETCH_PREFETCH = 8
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


def fetch_slots(
    session: MinervaSession, attempted: set[str], limit: int
) -> tuple[list, bool]:
    response = session.post(
        f"{API}/fetch-slots",
        json={
            "limit": limit,
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
    queued = deque()
    running = set()
    can_refill = True
    until = time.monotonic() + FETCH_RUN_SECONDS

    def run(slot):
        return pool.submit(attempt, session, client, slot, downloads)

    with ThreadPoolExecutor(FETCH_CONCURRENCY) as pool:
        while True:
            out_of_time = time.monotonic() >= until
            # Top the queue up. An offer shrinks its reservation from the
            # 1 GiB placeholder to the sources' size, so space Minerva could
            # not grant a moment ago may be free once a lecture finishes.
            if can_refill and not out_of_time and len(queued) < FETCH_PREFETCH:
                slots, can_refill = fetch_slots(
                    session,
                    attempted,
                    FETCH_CONCURRENCY + FETCH_PREFETCH - len(queued) - len(running),
                )
                print(f"{len(slots)} fetch slot(s) granted", flush=True)
                for slot in slots:
                    if slot["job_id"] in attempted:
                        raise ValueError("duplicate fetch reservation")
                    attempted.add(slot["job_id"])
                    queued.append(slot)
                # During CD the new Actions script may reach the old API.
                # Only refill when the server supports sizing and
                # exclusions; otherwise retries could reclaim the same failed
                # source in this run.
                if not slots and not running:
                    can_refill = False
            # Play logs in on first use, so the first lecture goes alone.
            if queued and len(attempted) == len(queued) and not running:
                attempt(session, client, queued.popleft(), downloads)
            out_of_time = time.monotonic() >= until
            if out_of_time:
                while queued:
                    release(
                        session,
                        queued.popleft()["job_id"],
                        "fetch run time budget",
                        retry=True,
                    )
            while queued and len(running) < FETCH_CONCURRENCY:
                running.add(run(queued.popleft()))
            if not running:
                if queued or (can_refill and not out_of_time):
                    continue
                break
            _, running = wait(running, return_when=FIRST_COMPLETED)
    return len(attempted)


def main() -> None:
    session = minerva_session()
    username, password = su_credentials()
    with PlayClient(username, password) as client:
        attempted = authorize_window(session, client)
    print(f"{attempted} lecture(s) attempted", flush=True)


if __name__ == "__main__":
    main()
