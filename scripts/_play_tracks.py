"""Guess the slide source from a few frames, with a hard network byte budget.

Never use playAudio or source names as slide labels. A loopback proxy caps
all ffprobe/ffmpeg reads (including moov/tail probes) at 24 MiB per source.
Bearer URLs never appear in subprocess arguments or diagnostics.

Sources and each source's frame samples are probed concurrently: almost all
of a probe is round trips to Play and tesseract, not local CPU.
"""

import csv
import io
import json
import os
import re
import statistics
import subprocess
import threading
from concurrent.futures import Future, ThreadPoolExecutor
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import requests
from dsv_wrapper.parsers.play import enumerate_track_descriptors, resolve_track_download

PROBE_BYTES = 24 * 1024 * 1024
BLOCK_BYTES = 256 * 1024
FRAME_FRACTIONS = (0.2, 0.5, 0.8)
SOURCE_PROBE_WORKERS = 3


class ProbeError(ValueError):
    """A probe failure whose message is fixed text, safe to log and store."""


@contextmanager
def probe_url(download):
    # ffprobe and each ffmpeg process reread the MP4's index. Cache aligned
    # ranges across all samples so large moov atoms count only once against
    # the 24 MiB network budget. Each upstream request is bounded as well.
    remaining = PROBE_BYTES
    blocks = {}
    total_bytes = None
    stopped = False
    lock = threading.Lock()
    client = requests.Session()

    def fetch(index):
        start = index * BLOCK_BYTES
        end = start + BLOCK_BYTES - 1
        headers = {"Range": f"bytes={start}-{end}", "Accept-Encoding": "identity"}
        with client.get(
            download.url,
            headers=headers,
            stream=True,
            timeout=(10, 15),
            allow_redirects=False,
        ) as response:
            span = re.fullmatch(
                r"bytes (\d+)-(\d+)/(\d+)",
                response.headers.get("Content-Range", ""),
            )
            if response.status_code != 206 or not span:
                raise ProbeError("media host did not return a byte range")
            first, last, size = map(int, span.groups())
            if first != start or last != min(end, size - 1) or size <= last:
                raise ProbeError("invalid media byte range")
            data = bytearray()
            for chunk in response.iter_content(BLOCK_BYTES):
                if len(data) + len(chunk) > last - first + 1:
                    raise ProbeError("oversized media byte range")
                data.extend(chunk)
            if len(data) != last - first + 1:
                raise ProbeError("truncated media byte range")
        return size, bytes(data)

    def block_at(index):
        # Concurrent readers fetch different blocks in parallel; a block
        # already in flight is awaited, never fetched or charged twice.
        nonlocal remaining, total_bytes
        with lock:
            if stopped:
                raise ProbeError("probe closed")
            pending = blocks.get(index)
            owner = pending is None
            if owner:
                if remaining < BLOCK_BYTES:
                    raise ProbeError("probe byte budget exhausted")
                remaining -= BLOCK_BYTES
                pending = blocks[index] = Future()
        if owner:
            try:
                size, data = fetch(index)
                with lock:
                    if total_bytes is not None and total_bytes != size:
                        raise ProbeError("media changed during probe")
                    total_bytes = size
            except BaseException as exc:
                # Readers already waiting see the failure; later ones retry,
                # charged against the budget again.
                with lock:
                    blocks.pop(index, None)
                pending.set_exception(exc)
                raise
            pending.set_result(data)
        return pending.result()

    class Proxy(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            self.connection.settimeout(10)
            span = re.fullmatch(
                r"bytes=(\d+)-(\d*)", self.headers.get("Range", "bytes=0-")
            )
            if self.path != "/video" or not span:
                self.send_error(400)
                return
            try:
                start = int(span[1])
                end = min(int(span[2]) if span[2] else total_bytes - 1, total_bytes - 1)
                if start > end:
                    self.send_error(416)
                    return
                # Check the initial block before sending a successful status.
                block_at(start // BLOCK_BYTES)
                self.send_response(206)
                self.send_header("Content-Length", str(end - start + 1))
                self.send_header("Content-Range", f"bytes {start}-{end}/{total_bytes}")
                self.send_header("Content-Type", "video/mp4")
                self.send_header("Accept-Ranges", "bytes")
                self.end_headers()
                while start <= end:
                    data = block_at(start // BLOCK_BYTES)
                    offset = start % BLOCK_BYTES
                    data = data[
                        offset : offset + min(len(data) - offset, end - start + 1)
                    ]
                    self.wfile.write(data)
                    start += len(data)
            except (requests.RequestException, OSError, ValueError):
                # Closing a partial response makes ffmpeg fail safely; neither
                # errors nor subprocess arguments contain the signed URL.
                return

    server = None
    thread = None
    try:
        block_at(0)
        server = ThreadingHTTPServer(("127.0.0.1", 0), Proxy)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        yield f"http://127.0.0.1:{server.server_port}/video"
    finally:
        stopped = True
        if server is not None:
            server.shutdown()
            server.server_close()
            thread.join()
        client.close()


def run(*args, body=None, env=None):
    try:
        result = subprocess.run(
            args,
            input=body,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=45,
            check=True,
        )
    except (subprocess.SubprocessError, OSError) as exc:
        raise ProbeError("video frame probe failed") from exc
    return result.stdout


def text_score(tsv):
    rows = list(csv.DictReader(io.StringIO(tsv.decode()), delimiter="\t"))
    image_height = next(
        (float(w["height"]) for w in rows if w.get("level") == "1" and w.get("height")),
        0,
    )
    # Prefer a full-screen slide over the same slide rendered smaller inside
    # a composite: characters alone would give both streams the same score.
    return sum(
        len(w.get("text", "").strip())
        * (100 * float(w.get("height", 0)) / image_height if image_height else 1)
        for w in rows
        if len(w.get("text", "").strip()) >= 3 and float(w.get("conf", -1)) >= 50
    )


def frame_score(url, seconds):
    frame = run(
        "ffmpeg",
        "-v",
        "error",
        "-ss",
        str(seconds),
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
    # Frames are OCR'd side by side; tesseract's own OpenMP threads would
    # only oversubscribe the runner's cores.
    return text_score(
        run(
            "tesseract",
            "stdin",
            "stdout",
            "--psm",
            "11",
            "tsv",
            body=frame,
            env={**os.environ, "OMP_THREAD_LIMIT": "1"},
        )
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
            raise ProbeError("invalid video duration")
        with ThreadPoolExecutor(len(FRAME_FRACTIONS)) as pool:
            scores = list(
                pool.map(
                    lambda fraction: frame_score(url, duration * fraction),
                    FRAME_FRACTIONS,
                )
            )
        return statistics.median(scores)


def choose_source(scores):
    ranked = sorted(scores, key=scores.get, reverse=True)
    if len(ranked) == 1:
        return ranked[0]
    best = ranked[0]
    if scores[best] < 20:
        raise ProbeError("no readable slide source found after frame probes")
    return best


def pick_track(presentation):
    descriptors = enumerate_track_descriptors(presentation)
    sources = {}
    for index, (_, name, height) in enumerate(descriptors):
        sources.setdefault(name, []).append((index, height))
    if not sources:
        raise ProbeError("presentation has no video track")
    if len(sources) == 1:
        name = next(iter(sources))
    else:
        # All sources must be probed successfully; a failed slide probe
        # cannot silently make the camera the winning source.
        with ThreadPoolExecutor(SOURCE_PROBE_WORKERS) as pool:
            probes = {
                name: pool.submit(
                    score_source,
                    resolve_track_download(
                        presentation,
                        min(variants, key=lambda variant: variant[1] or 0)[0],
                    ),
                )
                for name, variants in sources.items()
            }
            scores = {name: probe.result() for name, probe in probes.items()}
        name = choose_source(scores)
    index, _ = min(sources[name], key=lambda variant: abs((variant[1] or 0) - 1080))
    return resolve_track_download(presentation, index)
