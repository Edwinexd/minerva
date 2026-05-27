"""Fetch transcripts from play.dsv.su.se for pending URL documents in Minerva.

Also performs discovery: for each designation configured on a Minerva course,
lists presentations on play.dsv.su.se and creates URL documents for any not
already tracked. The normal transcript flow then picks them up on a subsequent
run (or later in this same run, since discovery runs first).

Called by the GitHub Actions transcript workflow on an hourly schedule.
Requires: SU_USERNAME, SU_PASSWORD, MINERVA_API_URL, MINERVA_SERVICE_API_KEY
"""

import os
import sys
from urllib.parse import parse_qs, urlparse

import requests
from dsv_wrapper import PlayClient, PresentationNotReadyError

PLAY_PRESENTATION_URL = "https://play.dsv.su.se/presentation/{id}"

# Tags used to enumerate the play.dsv.su.se course catalog. Unioning the
# English and Swedish lecture tags yields a near-complete designation list.
CATALOG_TAGS = ["Lecture", "Föreläsning"]


def get_env(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        print(f"error: {name} not set", file=sys.stderr)
        sys.exit(1)
    return value


def extract_presentation_id(url: str) -> str | None:
    """Extract a presentation ID from a play.dsv.su.se URL.

    Handles formats like:
      - https://play.dsv.su.se/multiplayer?p=UUID&l=7620  (ID in query param)
      - https://play.dsv.su.se/media/t/0_abc123            (ID in path)
      - https://play.dsv.su.se/presentation/some-id         (ID in path)
    """
    parsed = urlparse(url)

    query_params = parse_qs(parsed.query)
    if "p" in query_params:
        return query_params["p"][0]

    path = parsed.path.strip("/")
    if not path:
        return None
    parts = path.split("/")
    return parts[-1] if parts[-1] else None


def sanitize_filename(title: str, fallback: str) -> str:
    """Server will also sanitize, but keep things tidy here."""
    cleaned = "".join(c for c in title if c not in ('/', '\\', '\0')).strip()
    if not cleaned:
        cleaned = fallback
    # Avoid absurdly long filenames; server caps at 200 too.
    if len(cleaned) > 180:
        cleaned = cleaned[:180].rstrip()
    return cleaned


def push_catalog(
    client: PlayClient,
    api_url: str,
    headers: dict,
) -> None:
    """Fetch the union of courses across CATALOG_TAGS and push to Minerva."""
    union: dict[str, str] = {}
    for tag in CATALOG_TAGS:
        try:
            for cc in client.get_courses_by_tag(tag):
                union.setdefault(cc.code, cc.name)
        except Exception as e:
            print(f"  catalog: failed tag {tag!r}: {e}")

    if not union:
        print("Catalog push skipped: no courses discovered.")
        return

    entries = [{"code": code, "name": name} for code, name in sorted(union.items())]
    resp = requests.put(
        f"{api_url}/api/service/play-courses",
        headers=headers,
        json=entries,
    )
    resp.raise_for_status()
    body = resp.json()
    print(
        f"Catalog push: {body.get('submitted')} submitted, "
        f"{body.get('upserted')} upserted."
    )


def discover_designations(
    client: PlayClient,
    api_url: str,
    headers: dict,
) -> None:
    """Discovery phase: for each watched designation, create URL docs for any
    presentations that aren't already tracked in the owning course.
    """
    resp = requests.get(f"{api_url}/api/service/play-designations", headers=headers)
    resp.raise_for_status()
    designations = resp.json()

    if not designations:
        print("No watched designations.")
        return

    print(f"Discovering presentations for {len(designations)} designation(s)...")

    for des in designations:
        des_id = des["id"]
        course_id = des["course_id"]
        code = des["designation"]

        try:
            presentations = client.get_presentations(code)
        except Exception as e:
            error_msg = f"failed to list presentations: {e}"
            print(f"  [{code}] {error_msg}")
            requests.post(
                f"{api_url}/api/service/play-designations/{des_id}/mark-synced",
                headers=headers,
                json={"error": error_msg},
            )
            continue

        created = 0
        skipped = 0
        for presentation in presentations:
            url = PLAY_PRESENTATION_URL.format(id=presentation.id)
            filename = sanitize_filename(
                presentation.title or presentation.title_en,
                fallback=f"play-{presentation.id}",
            )

            try:
                resp = requests.post(
                    f"{api_url}/api/service/courses/{course_id}/documents/url",
                    headers=headers,
                    json={"url": url, "filename": filename},
                )
                resp.raise_for_status()
                body = resp.json()
                if body.get("created"):
                    created += 1
                else:
                    skipped += 1
            except Exception as e:
                print(f"  [{code}] Failed to create doc for {presentation.id}: {e}")

        print(
            f"  [{code}] {len(presentations)} presentation(s): "
            f"{created} new, {skipped} already tracked"
        )

        requests.post(
            f"{api_url}/api/service/play-designations/{des_id}/mark-synced",
            headers=headers,
            json={},
        )


# Cursor page size. The backend caps this at 1024; 512 is the
# sweet spot for the script's memory footprint (only one batch's
# worth of doc metadata + transcript text held at a time) without
# the per-page round-trip overhead eating into the hourly window.
# Tunable via `MINERVA_TRANSCRIPTS_PAGE_SIZE` for the occasional
# manual burn-through.
TRANSCRIPTS_PAGE_SIZE = int(
    os.environ.get("MINERVA_TRANSCRIPTS_PAGE_SIZE", "512")
)


def _process_pending_doc(
    client: PlayClient,
    api_url: str,
    headers: dict,
    doc: dict,
) -> None:
    """Fetch the VTT for one pending doc and submit (or mark failed).
    Extracted so the cursor loop in `fetch_pending_transcripts` stays
    readable; behaviour is identical to the previous flat loop."""
    doc_id = doc["id"]
    url = doc["url"]
    filename = doc["filename"]

    presentation_id = extract_presentation_id(url)
    if not presentation_id:
        print(f"  [{filename}] Could not extract presentation ID from: {url}")
        resp = requests.post(
            f"{api_url}/api/service/documents/{doc_id}/transcript",
            headers=headers,
            json={"error": f"could not extract presentation ID from URL: {url}"},
        )
        resp.raise_for_status()
        return

    print(f"  [{filename}] Fetching transcript for {presentation_id}...")

    try:
        transcript = client.get_transcript_text(presentation_id)
    except PresentationNotReadyError as e:
        # Either the recording itself is still being processed (non-dict
        # /presentation/{uuid} envelope) or the video is ready but
        # captions haven't been generated yet. Both are transient; leave
        # the doc in awaiting_transcript so the next hourly run retries
        # once Play finishes processing. PresentationNotReadyError is
        # the parent of TranscriptNotReadyError, so it covers both.
        print(f"  [{filename}] Not ready yet, will retry next run: {e}")
        return
    except Exception as e:
        error_msg = str(e)
        print(f"  [{filename}] Failed: {error_msg}")
        resp = requests.post(
            f"{api_url}/api/service/documents/{doc_id}/transcript",
            headers=headers,
            json={"error": error_msg},
        )
        resp.raise_for_status()
        return

    if not transcript or not transcript.strip():
        print(f"  [{filename}] Empty transcript, marking as failed.")
        resp = requests.post(
            f"{api_url}/api/service/documents/{doc_id}/transcript",
            headers=headers,
            json={"error": "transcript is empty (no subtitles)"},
        )
        resp.raise_for_status()
        return

    resp = requests.post(
        f"{api_url}/api/service/documents/{doc_id}/transcript",
        headers=headers,
        json={"text": transcript},
    )
    resp.raise_for_status()
    result = resp.json()
    print(
        f"  [{filename}] Submitted ({len(transcript)} chars) -> {result.get('status')}"
    )


def fetch_pending_transcripts(
    client: PlayClient,
    api_url: str,
    headers: dict,
) -> None:
    """Transcript phase: drain every `awaiting_transcript` doc via
    cursor-paginated fetches. Memory peak is one page's worth of doc
    metadata + the current item's transcript text; the backlog itself
    is unbounded.

    Cursor design (see `pending_transcripts` route): we order by
    `(created_at, id)` ASC and pass the last item's pair as
    `after_created_at` / `after_id` on the next request. Items that
    stay in `awaiting_transcript` after processing (e.g.
    PresentationNotReadyError leaves the doc unchanged) are NOT
    re-visited within the same run; the cursor moves strictly forward.
    Next hour's cron starts fresh and picks them up if Play has
    finished processing by then.
    """
    after_created_at: str | None = None
    after_id: str | None = None
    total_seen = 0
    total_processed = 0

    while True:
        params: dict[str, str | int] = {"limit": TRANSCRIPTS_PAGE_SIZE}
        if after_created_at is not None and after_id is not None:
            params["after_created_at"] = after_created_at
            params["after_id"] = after_id

        resp = requests.get(
            f"{api_url}/api/service/pending-transcripts",
            headers=headers,
            params=params,
        )
        resp.raise_for_status()
        page = resp.json()
        if not page:
            break

        # Advance cursor past the WHOLE page even if we end up skipping
        # the non-play docs below; that way the next request never
        # rewinds, and items we couldn't handle this iteration get a
        # fresh look on the next cron tick.
        last = page[-1]
        after_created_at = last["created_at"]
        after_id = last["id"]
        total_seen += len(page)

        # Filter to play.dsv.su.se docs only; other URL providers
        # (GitHub PDF, future origins) use different code paths.
        play_docs = [doc for doc in page if "play.dsv.su.se" in doc.get("url", "")]
        if play_docs:
            print(
                f"Page of {len(page)} pending docs "
                f"({len(play_docs)} play.dsv.su.se); processing..."
            )
            for doc in play_docs:
                _process_pending_doc(client, api_url, headers, doc)
                total_processed += 1
        # A page strictly smaller than the requested limit means we've
        # reached the end of the queue; one more empty round-trip would
        # be wasteful.
        if len(page) < TRANSCRIPTS_PAGE_SIZE:
            break

    if total_seen == 0:
        print("No pending play.dsv.su.se transcripts.")
    else:
        print(
            f"Drained {total_processed} play.dsv.su.se transcript(s) "
            f"across {total_seen} pending row(s) total."
        )


def main() -> None:
    api_url = get_env("MINERVA_API_URL").rstrip("/")
    api_key = get_env("MINERVA_SERVICE_API_KEY")
    su_username = get_env("SU_USERNAME")
    su_password = get_env("SU_PASSWORD")

    headers = {"Authorization": f"Bearer {api_key}"}

    with PlayClient(username=su_username, password=su_password) as client:
        # Phase 0: refresh Minerva's catalog of known designations (for
        # teacher-facing autocomplete). Best-effort; failure doesn't abort.
        try:
            push_catalog(client, api_url, headers)
        except Exception as e:
            print(f"Catalog push failed: {e}")

        # Phase 1: discover new presentations for watched designations.
        discover_designations(client, api_url, headers)

        # Phase 2: fetch transcripts for awaiting_transcript docs (including any
        # newly created by phase 1 that have already been triaged).
        fetch_pending_transcripts(client, api_url, headers)

    print("Done.")


if __name__ == "__main__":
    main()
