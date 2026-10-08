"""play.dsv.su.se helpers shared by the scripts that read lectures off Play
(`fetch_transcripts.py`, `fetch_lecture_videos.py`). Imported as a sibling
module, like `_minerva_api`.
"""

from urllib.parse import parse_qs, urlparse


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
