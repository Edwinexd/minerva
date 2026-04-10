"""Fetch transcripts from play.dsv.su.se for pending URL documents in Minerva.

Called by the GitHub Actions transcript workflow on an hourly schedule.
Requires: SU_USERNAME, SU_PASSWORD, MINERVA_API_URL, MINERVA_SERVICE_API_KEY
"""

import json
import os
import sys
from urllib.parse import parse_qs, urlparse

import requests
from dsv_wrapper import PlayClient


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

    # Check query parameter 'p' first (multiplayer URLs).
    query_params = parse_qs(parsed.query)
    if "p" in query_params:
        return query_params["p"][0]

    # Fall back to last path segment.
    path = parsed.path.strip("/")
    if not path:
        return None
    parts = path.split("/")
    return parts[-1] if parts[-1] else None


def main() -> None:
    api_url = get_env("MINERVA_API_URL").rstrip("/")
    api_key = get_env("MINERVA_SERVICE_API_KEY")
    su_username = get_env("SU_USERNAME")
    su_password = get_env("SU_PASSWORD")

    headers = {"Authorization": f"Bearer {api_key}"}

    # 1. Get pending URL documents from Minerva.
    resp = requests.get(f"{api_url}/api/service/pending-transcripts", headers=headers)
    resp.raise_for_status()
    pending = resp.json()

    # Filter for play.dsv.su.se URLs.
    play_docs = [
        doc for doc in pending if "play.dsv.su.se" in doc.get("url", "")
    ]

    if not play_docs:
        print("No pending play.dsv.su.se transcripts.")
        return

    print(f"Found {len(play_docs)} pending play.dsv.su.se document(s).")

    # 2. Fetch transcripts using dsv-wrapper.
    with PlayClient(username=su_username, password=su_password) as client:
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

            # 3. Submit transcript to Minerva.
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

    print("Done.")


if __name__ == "__main__":
    main()
