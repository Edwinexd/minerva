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


def fetch_pending_transcripts(
    client: PlayClient,
    api_url: str,
    headers: dict,
) -> None:
    """Transcript phase: fetch VTT for documents in awaiting_transcript state."""
    resp = requests.get(f"{api_url}/api/service/pending-transcripts", headers=headers)
    resp.raise_for_status()
    pending = resp.json()

    play_docs = [doc for doc in pending if "play.dsv.su.se" in doc.get("url", "")]

    if not play_docs:
        print("No pending play.dsv.su.se transcripts.")
        return

    print(f"Found {len(play_docs)} pending play.dsv.su.se document(s).")

    for doc in play_docs:
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
            continue

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
            continue
        except Exception as e:
            error_msg = str(e)
            print(f"  [{filename}] Failed: {error_msg}")
            resp = requests.post(
                f"{api_url}/api/service/documents/{doc_id}/transcript",
                headers=headers,
                json={"error": error_msg},
            )
            resp.raise_for_status()
            continue

        if not transcript or not transcript.strip():
            print(f"  [{filename}] Empty transcript, marking as failed.")
            resp = requests.post(
                f"{api_url}/api/service/documents/{doc_id}/transcript",
                headers=headers,
                json={"error": "transcript is empty (no subtitles)"},
            )
            resp.raise_for_status()
            continue

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
