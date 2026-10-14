"""Describe a lecture's Play sources for Minerva, which picks the slide track.

Each source is offered with two bearer URLs from the same envelope: its
lowest-resolution variant, which minerva-fetcher probes for slide text, and
the variant closest to 1080p, which it stages if that source wins. Only
response headers are read here, never media bytes, and the URLs never appear
in diagnostics. Source names are identifiers, never slide labels.
"""

import re
from concurrent.futures import ThreadPoolExecutor

import requests
from dsv_wrapper.parsers.play import enumerate_track_descriptors, resolve_track_download

SIZE_TIMEOUT = (10, 15)


class TrackError(ValueError):
    """A failure whose message is fixed text, safe to log and store."""


def media_size(client, url):
    """Play's announced size of `url`, or None to reserve the full cap.

    HEAD first; some media servers lack it, and a one-byte range reports the
    same total in Content-Range. Neither reads a response body.
    """
    headers = {"Accept-Encoding": "identity"}
    try:
        with client.head(
            url, headers=headers, timeout=SIZE_TIMEOUT, allow_redirects=False
        ) as response:
            if response.status_code == 200 and response.headers.get(
                "Content-Encoding", "identity"
            ) == "identity":
                size = response.headers.get("Content-Length", "")
                if size.isdigit():
                    return int(size)
        with client.get(
            url,
            headers={**headers, "Range": "bytes=0-0"},
            timeout=SIZE_TIMEOUT,
            allow_redirects=False,
            stream=True,
        ) as response:
            # Content-Length is the one-byte body here; reject a range the
            # host ignored or expanded.
            span = re.fullmatch(
                r"bytes 0-0/(\d+)", response.headers.get("Content-Range", "")
            )
            if (
                response.status_code == 206
                and span
                and response.headers.get("Content-Length", "1") == "1"
                and response.headers.get("Content-Encoding", "identity") == "identity"
            ):
                return int(span[1])
    except requests.RequestException:
        pass
    return None


def describe_sources(presentation):
    """Candidates to offer and the envelope's token expiry (Unix seconds)."""
    sources = {}
    for index, (_, name, height) in enumerate(
        enumerate_track_descriptors(presentation)
    ):
        sources.setdefault(name, []).append((index, height))
    if not sources:
        raise TrackError("presentation has no video track")
    tracks = [
        (
            name,
            resolve_track_download(
                presentation, min(variants, key=lambda v: v[1] or 0)[0]
            ),
            resolve_track_download(
                presentation, min(variants, key=lambda v: abs((v[1] or 0) - 1080))[0]
            ),
        )
        for name, variants in sources.items()
    ]
    with requests.Session() as client, ThreadPoolExecutor(len(tracks)) as pool:
        sizes = list(
            pool.map(lambda track: media_size(client, track[2].url), tracks)
        )
    candidates = [
        {
            "source": name,
            "probe_url": probe.url,
            "download_url": download.url,
            "size_bytes": size,
        }
        for (name, probe, download), size in zip(tracks, sizes)
    ]
    return candidates, tracks[0][2].expires_at
