"""Guess the slide source from a few frames, with a hard network byte budget.

Never use playAudio or source names as slide labels. A loopback proxy caps
all ffprobe/ffmpeg reads (including moov/tail probes) at 24 MiB per source.
Bearer URLs never appear in subprocess arguments or diagnostics.
"""

import csv
import io
import json
import statistics
import subprocess
import threading
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import requests
from dsv_wrapper.parsers.play import enumerate_track_descriptors, resolve_track_download

PROBE_BYTES = 24 * 1024 * 1024


@contextmanager
def probe_url(download):
    remaining = PROBE_BYTES
    budget_lock = threading.Lock()

    class Proxy(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            nonlocal remaining
            if self.path != "/video" or remaining <= 0:
                self.send_error(413)
                return
            self.connection.settimeout(10)
            headers = {
                "Range": self.headers.get("Range", "bytes=0-"),
                "Accept-Encoding": "identity",
            }
            try:
                with requests.get(
                    download.url,
                    headers=headers,
                    stream=True,
                    timeout=(10, 15),
                    allow_redirects=False,
                ) as response:
                    if response.status_code != 206:
                        # No ranges means probing might transfer an entire video.
                        self.send_error(502)
                        return
                    self.send_response(206)
                    for name in (
                        "Content-Length",
                        "Content-Range",
                        "Content-Type",
                        "Accept-Ranges",
                    ):
                        if name in response.headers:
                            self.send_header(name, response.headers[name])
                    self.end_headers()
                    chunks = response.iter_content(64 * 1024)
                    while True:
                        with budget_lock:
                            if remaining < 64 * 1024:
                                return
                            # Reserve before reading so concurrent seeks cannot
                            # exceed the network cap. Short reads count in full.
                            remaining -= 64 * 1024
                        chunk = next(chunks, None)
                        if chunk is None:
                            return
                        self.wfile.write(chunk)
            except (requests.RequestException, OSError):
                return

    server = ThreadingHTTPServer(("127.0.0.1", 0), Proxy)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/video"
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def run(*args, body=None):
    try:
        result = subprocess.run(
            args,
            input=body,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=45,
            check=True,
        )
    except (subprocess.SubprocessError, OSError) as exc:
        raise ValueError("video frame probe failed") from exc
    return result.stdout


def text_score(tsv):
    words = csv.DictReader(io.StringIO(tsv.decode()), delimiter="\t")
    return sum(
        len(w.get("text", "").strip())
        for w in words
        if len(w.get("text", "").strip()) >= 3 and float(w.get("conf", -1)) >= 50
    )


def score_source(download):
    with probe_url(download) as url:
        info = json.loads(
            run(
                "ffprobe",
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "json",
                url,
            )
        )
        duration = float(info["format"]["duration"])
        if not 0 < duration < 24 * 3600:
            raise ValueError("invalid video duration")
        scores = []
        for fraction in (0.2, 0.5, 0.8):
            frame = run(
                "ffmpeg",
                "-v",
                "error",
                "-ss",
                str(duration * fraction),
                "-i",
                url,
                "-frames:v",
                "1",
                "-vf",
                "scale=1280:-1",
                "-f",
                "image2pipe",
                "-vcodec",
                "png",
                "pipe:1",
            )
            scores.append(
                text_score(
                    run(
                        "tesseract", "stdin", "stdout", "--psm", "11", "tsv", body=frame
                    )
                )
            )
        return statistics.median(scores)


def choose_source(scores):
    ranked = sorted(scores, key=scores.get, reverse=True)
    if len(ranked) == 1:
        return ranked[0]
    best, second = ranked[:2]
    if scores[best] < 20 or scores[best] <= scores[second] * 1.1:
        raise ValueError(
            "slide source ambiguous after frame probes; retry with fresh samples"
        )
    return best


def pick_track(presentation):
    descriptors = enumerate_track_descriptors(presentation)
    sources = {}
    for index, (_, name, height) in enumerate(descriptors):
        sources.setdefault(name, []).append((index, height))
    if not sources:
        raise ValueError("presentation has no video track")
    if len(sources) == 1:
        name = next(iter(sources))
    else:
        # All sources must be probed successfully; a failed slide probe
        # cannot silently make the camera the winning source.
        scores = {}
        for name, variants in sources.items():
            index, _ = min(variants, key=lambda variant: variant[1] or 0)
            scores[name] = score_source(resolve_track_download(presentation, index))
        name = choose_source(scores)
    index, _ = min(sources[name], key=lambda variant: abs((variant[1] or 0) - 1080))
    return resolve_track_download(presentation, index)
