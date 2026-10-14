import importlib.machinery
import importlib.util
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
from _play_tracks import (  # noqa: E402
    ProbeError,
    choose_source,
    pick_track,
    probe_url,
    text_score,
)
from dsv_wrapper.models.play import Presentation, VideoSource  # noqa: E402

loader = importlib.machinery.SourceFileLoader(
    "ssh_gate", str(ROOT / "gpu/slide-ocr/olympus/ssh-gate")
)
spec = importlib.util.spec_from_loader(loader.name, loader)
gate = importlib.util.module_from_spec(spec)
loader.exec_module(gate)


def test_selection_ignores_audio_and_uses_observed_text():
    p = Presentation(
        id="x",
        title="x",
        sources={
            "camera": VideoSource(url_720p="camera", play_audio=False),
            "capture": VideoSource(
                url_720p="slides-low", url_1080p="slides-high", play_audio=True
            ),
        },
    )

    def resolve(p, index):
        name = ["camera", "capture", "capture"][index]
        return SimpleNamespace(source_name=name, index=index)

    with (
        patch("_play_tracks.resolve_track_download", side_effect=resolve),
        patch(
            "_play_tracks.score_source",
            side_effect=lambda d: 8 if d.source_name == "camera" else 90,
        ),
    ):
        assert pick_track(p).index == 2


def test_sources_are_probed_concurrently_and_all_must_succeed():
    import threading

    p = Presentation(
        id="x",
        title="x",
        sources={
            name: VideoSource(url_720p=name, play_audio=False)
            for name in ("left", "right", "main")
        },
    )
    barrier = threading.Barrier(3, timeout=5)

    def score(download):
        barrier.wait()
        if download.source_name == "right":
            raise ProbeError("video frame probe failed")
        return 90

    with (
        patch(
            "_play_tracks.resolve_track_download",
            side_effect=lambda p, index: SimpleNamespace(
                source_name=["left", "right", "main"][index]
            ),
        ),
        patch("_play_tracks.score_source", side_effect=score),
        pytest.raises(ProbeError),
    ):
        pick_track(p)


def test_blank_probes_do_not_guess_camera():
    for scores in ({"a": 0, "b": 0}, {"a": 10, "b": 2}):
        with pytest.raises(ValueError):
            choose_source(scores)
    assert choose_source({"a": 200, "b": 50}) == "a"


def test_ocr_rejects_low_confidence_noise():
    assert text_score(b"text\tconf\nslide\t95\ngarbage\t20\n\t-1\n") == 5


@pytest.mark.parametrize(
    "command",
    [
        "",
        "bash",
        "scp -t x",
        "sftp",
        "minerva-slide-ocr/olympus/slide-ocr-gate cancel 1; id",
        "minerva-slide-ocr/olympus/slide-ocr-gate submit $(id)",
        "minerva-slide-ocr-deploy " + "a" * 40,
    ],
)
def test_scheduler_key_denies_shell_and_deploy(command):
    with patch.object(gate.subprocess, "run") as run:
        with pytest.raises(ValueError):
            gate.dispatch("scheduler", command)
        run.assert_not_called()


def test_scheduler_dispatch_has_no_shell():
    with patch.object(gate.subprocess, "run") as run:
        gate.dispatch(
            "scheduler", "minerva-slide-ocr/olympus/slide-ocr-gate status 1,2"
        )
        assert run.call_args.args[0][-2:] == ["status", "1,2"]
        assert "shell" not in run.call_args.kwargs


def test_deploy_key_cannot_schedule():
    with pytest.raises(ValueError):
        gate.dispatch("deploy", "minerva-slide-ocr/olympus/slide-ocr-gate cancel 1")


def test_probe_cannot_read_past_byte_budget():
    from urllib.request import urlopen

    import _play_tracks

    read_chunks = []

    class Response:
        status_code = 206

        def __init__(self, start, end):
            self.size = end - start + 1
            self.headers = {
                "Content-Length": str(self.size),
                "Content-Range": f"bytes {start}-{end}/33554432",
            }

        def __enter__(self):
            return self

        def __exit__(self, *args):
            pass

        def iter_content(self, chunk_size):
            read_chunks.append(self.size)
            yield b"x" * self.size

    class Session:
        def get(self, url, headers, **kwargs):
            start, end = map(int, headers["Range"].removeprefix("bytes=").split("-"))
            return Response(start, end)

        def close(self):
            pass

    with patch("_play_tracks.requests.Session", return_value=Session()):
        with probe_url(SimpleNamespace(url="https://example.invalid/video")) as url:
            with urlopen(url) as response:
                while response.read(64 * 1024):
                    pass

    assert sum(read_chunks) <= _play_tracks.PROBE_BYTES


def archive(members):
    import io
    import tarfile

    data = io.BytesIO()
    with tarfile.open(fileobj=data, mode="w:gz") as output:
        for name, kind in members:
            info = tarfile.TarInfo(name)
            if kind == "link":
                info.type = tarfile.SYMTYPE
                info.linkname = "/tmp/escape"
                output.addfile(info)
            else:
                info.size = 4
                output.addfile(info, io.BytesIO(b"code"))
    data.seek(0)
    return SimpleNamespace(buffer=data)


def test_deploy_installs_only_production_files(tmp_path, monkeypatch):
    monkeypatch.setattr(gate, "ROOT", tmp_path / "worker")
    monkeypatch.setattr(gate.sys, "stdin", archive((f, "file") for f in gate.FILES))
    gate.deploy("a" * 40)
    assert (gate.ROOT / "DEPLOYED").read_text() == "a" * 40 + "\n"
    assert not (gate.ROOT / "olympus/ssh-gate").exists()


@pytest.mark.parametrize(
    "bad_name,kind", [("../.ssh/authorized_keys", "file"), ("worker.py", "link")]
)
def test_deploy_rejects_escape_and_links(tmp_path, monkeypatch, bad_name, kind):
    monkeypatch.setattr(gate, "ROOT", tmp_path / "worker")
    members = [(f, "file") for f in gate.FILES if f != "worker.py"] + [(bad_name, kind)]
    monkeypatch.setattr(gate.sys, "stdin", archive(members))
    with pytest.raises(ValueError):
        gate.deploy("a" * 40)
    assert not (gate.ROOT / "DEPLOYED").exists()


@pytest.mark.parametrize(
    "launcher,entry", [("run-worker.sh", "worker.py"), ("run-job.sh", "slide_ocr.py")]
)
def test_worker_launch_uses_interpreter_after_venv_move(tmp_path, launcher, entry):
    import os
    import subprocess

    checkout = tmp_path / "worker checkout"
    checkout.mkdir()
    venv = tmp_path / "moved worker venv"
    (venv / "bin").mkdir(parents=True)
    (venv / "bin/activate").write_text("export PATH=/missing/original-build.tmp/bin\n")
    (checkout / launcher).write_text((ROOT / "gpu/slide-ocr" / launcher).read_text())
    (checkout / "ensure-venv.sh").write_text('printf "%s\\n" "$WORKER_TEST_VENV"\n')
    python = venv / "bin/python"
    python.write_text('#!/bin/bash\nprintf "%s\\n" "$@" > "$WORKER_TEST_TRACE"\n')
    python.chmod(0o700)
    trace = tmp_path / "argv"
    env = os.environ.copy()
    env.update(WORKER_TEST_VENV=str(venv), WORKER_TEST_TRACE=str(trace))
    subprocess.run(
        ["/bin/bash", str(checkout / launcher), "grants/test"], env=env, check=True
    )
    assert trace.read_text().splitlines() == [entry, "grants/test"]


def test_probe_reuses_header_ranges_across_clients():
    from urllib.request import Request, urlopen

    reads = []

    class Response:
        status_code = 206
        headers = {"Content-Range": "bytes 0-262143/33554432"}

        def __enter__(self):
            return self

        def __exit__(self, *args):
            pass

        def iter_content(self, chunk_size):
            reads.append(chunk_size)
            yield b"x" * chunk_size

    class Session:
        def get(self, url, **kwargs):
            return Response()

        def close(self):
            pass

    with patch("_play_tracks.requests.Session", return_value=Session()):
        with probe_url(SimpleNamespace(url="https://example.invalid/video")) as url:
            for _ in range(4):
                with urlopen(
                    Request(url, headers={"Range": "bytes=0-1023"})
                ) as response:
                    assert response.read() == b"x" * 1024
    assert reads == [256 * 1024]


def test_probe_fetches_blocks_in_parallel_and_shares_inflight_blocks():
    import threading
    from concurrent.futures import ThreadPoolExecutor
    from urllib.request import Request, urlopen

    fetched = []
    started = threading.Barrier(2, timeout=5)

    class Response:
        status_code = 206

        def __init__(self, start):
            self.start = start
            self.headers = {
                "Content-Range": f"bytes {start}-{start + 262143}/33554432"
            }

        def __enter__(self):
            return self

        def __exit__(self, *args):
            pass

        def iter_content(self, chunk_size):
            fetched.append(self.start)
            if self.start:
                # Blocks 1 and 2 only finish if both are in flight at once.
                started.wait()
            yield b"x" * chunk_size

    class Session:
        def get(self, url, headers, **kwargs):
            return Response(int(headers["Range"].removeprefix("bytes=").split("-")[0]))

        def close(self):
            pass

    def read(block):
        start = block * 256 * 1024
        with urlopen(
            Request(url, headers={"Range": f"bytes={start}-{start + 1023}"})
        ) as response:
            return response.read()

    with patch("_play_tracks.requests.Session", return_value=Session()):
        with probe_url(SimpleNamespace(url="https://example.invalid/video")) as url:
            with ThreadPoolExecutor(6) as pool:
                assert all(r == b"x" * 1024 for r in pool.map(read, [1, 2, 1, 2, 1, 2]))
    assert sorted(fetched) == [0, 256 * 1024, 512 * 1024]


def test_shared_presentations_are_probed_once_per_batch():
    import time
    from unittest.mock import Mock

    import fetch_lecture_videos
    from dsv_wrapper.models.play import TrackDownload

    client = Mock()
    client.get_transcript.return_value = []
    client.get_presentation.return_value = SimpleNamespace(is_video_ready=True)
    session = Mock()
    downloads = {}
    download = TrackDownload(
        url="https://example.invalid/video.mp4",
        expires_at=int(time.time()) + 3600,
        source_name="slides",
    )
    with patch("fetch_lecture_videos.pick_track", return_value=download) as pick:
        for job in ("first-course", "second-course"):
            fetch_lecture_videos.stage(
                session,
                client,
                {"job_id": job, "url": "https://play.dsv.su.se/presentation/shared"},
                downloads,
            )
    pick.assert_called_once()
    client.get_transcript.assert_called_once()
    assert session.post.call_count == 2
    assert (
        session.post.call_args_list[0].args[0].endswith("/jobs/first-course/download")
    )
    assert (
        session.post.call_args_list[1].args[0].endswith("/jobs/second-course/download")
    )


def test_readable_duplicate_slide_sources_do_not_retry_forever():
    assert choose_source({"left": 445, "main": 449, "camera": 0}) == "main"
    assert choose_source({"left": 445, "main": 445, "camera": 0}) == "left"


def test_text_score_prefers_larger_slide_text():
    def tsv(height):
        return (
            f"level\theight\ttext\tconf\n1\t720\t\t-1\n5\t{height}\tslides\t95\n"
        ).encode()

    assert text_score(tsv(30)) == 2 * text_score(tsv(15))


@pytest.mark.parametrize("refill,expected", [(True, 37), (False, 4)])
def test_authorization_fills_bytes_beyond_old_url_count(refill, expected):
    from unittest.mock import Mock

    import fetch_lecture_videos as fetch

    # Ten GiB available. Each unknown source initially reserves one GiB;
    # the selected source's headers shrink it to 256 MiB. The final 768 MiB
    # cannot safely reserve another unknown source.
    remaining = 10 * 1024
    next_job = 0

    def reserve(url, json):
        nonlocal remaining, next_job
        count = min(json["limit"], remaining // 1024)
        slots = [{"job_id": str(j)} for j in range(next_job, next_job + count)]
        next_job += count
        remaining -= count * 1024
        return SimpleNamespace(
            raise_for_status=lambda: None,
            json=lambda: slots,
            headers={fetch.REFILL_HEADER: "1"} if refill else {},
        )

    def sized(session, client, slot, downloads):
        nonlocal remaining
        remaining += 1024 - 256

    session = Mock()
    session.post.side_effect = reserve
    with patch.object(fetch, "stage", side_effect=sized):
        assert fetch.authorize_window(session, Mock()) == expected
    if refill:
        assert remaining == 768
        assert len(session.post.call_args.kwargs["json"]["exclude_job_ids"]) == 37


def test_authorization_skips_failed_sources_for_rest_of_run():
    from unittest.mock import Mock

    import fetch_lecture_videos as fetch

    session = Mock()

    def post(url, json):
        if url.endswith("/release"):
            return Mock()
        available = [j for j in ("bad", "good") if j not in json["exclude_job_ids"]]
        return SimpleNamespace(
            raise_for_status=lambda: None,
            json=lambda: [{"job_id": j} for j in available],
            headers={fetch.REFILL_HEADER: "1"},
        )

    def stage(session, client, slot, downloads):
        if slot["job_id"] == "bad":
            raise ValueError("signed URL must not appear in output")

    session.post.side_effect = post
    with (
        patch.object(fetch, "stage", side_effect=stage) as authorization,
        patch.object(fetch, "release") as release,
    ):
        assert fetch.authorize_window(session, Mock()) == 2
    assert authorization.call_count == 2
    release.assert_called_once_with(
        session, "bad", "Play authorization/probe failed (ValueError)", retry=True
    )
    assert session.post.call_args.kwargs["json"]["exclude_job_ids"] == ["bad", "good"]


def test_probe_failures_record_their_fixed_reason():
    from unittest.mock import Mock

    import fetch_lecture_videos as fetch

    response = Mock()
    response.json.return_value = [{"job_id": "a"}]
    response.headers = {}
    session = Mock()
    session.post.return_value = response
    with (
        patch.object(
            fetch,
            "stage",
            side_effect=ProbeError("no readable slide source found after frame probes"),
        ),
        patch.object(fetch, "release") as release,
    ):
        fetch.authorize_window(session, Mock())
    release.assert_called_once_with(
        session,
        "a",
        "Play authorization/probe failed "
        "(no readable slide source found after frame probes)",
        retry=True,
    )


def test_time_budget_releases_unprocessed_reservations():
    from unittest.mock import Mock

    import fetch_lecture_videos as fetch

    response = Mock()
    response.json.return_value = [{"job_id": "a"}, {"job_id": "b"}]
    response.headers = {fetch.REFILL_HEADER: "1"}
    session = Mock()
    session.post.return_value = response
    with (
        patch.object(fetch.time, "monotonic", side_effect=[0, 0, 0, 2401]),
        patch.object(fetch, "stage") as stage,
        patch.object(fetch, "release") as release,
    ):
        assert fetch.authorize_window(session, Mock()) == 1
    stage.assert_called_once()
    release.assert_called_once_with(session, "b", "fetch run time budget", retry=True)
