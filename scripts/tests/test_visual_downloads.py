import importlib.machinery
import importlib.util
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
from _play_tracks import TrackError, describe_sources, media_size  # noqa: E402
from dsv_wrapper.models.play import Presentation, VideoSource  # noqa: E402

loader = importlib.machinery.SourceFileLoader(
    "ssh_gate", str(ROOT / "gpu/slide-ocr/olympus/ssh-gate")
)
spec = importlib.util.spec_from_loader(loader.name, loader)
gate = importlib.util.module_from_spec(spec)
loader.exec_module(gate)


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


def test_shared_presentations_are_described_once_per_batch():
    import time
    from unittest.mock import Mock

    import fetch_lecture_videos

    client = Mock()
    client.get_transcript.return_value = []
    client.get_presentation.return_value = SimpleNamespace(is_video_ready=True)
    session = Mock()
    downloads = {}
    candidates = [{"source": "slides", "download_url": "https://example.invalid/v.mp4"}]
    described = (candidates, int(time.time()) + 3600)
    with patch(
        "fetch_lecture_videos.describe_sources", return_value=described
    ) as pick:
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


@pytest.mark.parametrize("refill,expected", [(True, 37), (False, 10)])
def test_authorization_fills_bytes_beyond_old_url_count(refill, expected):
    from unittest.mock import Mock

    import fetch_lecture_videos as fetch

    import threading

    # Ten GiB available. Each unknown source initially reserves one GiB;
    # the offered sizes shrink it to 256 MiB. The final 768 MiB cannot
    # safely reserve another unknown source. Without the refill header (an
    # old API) only the first grant is used.
    remaining = 10 * 1024
    next_job = 0
    lock = threading.Lock()

    def reserve(url, json):
        nonlocal remaining, next_job
        with lock:
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
        with lock:
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
        session, "bad", "Play authorization failed (ValueError)", retry=True
    )
    assert session.post.call_args.kwargs["json"]["exclude_job_ids"] == ["bad", "good"]


def test_permanent_rejections_park_the_lecture():
    from unittest.mock import Mock

    import fetch_lecture_videos as fetch

    with (
        patch.object(
            fetch,
            "stage",
            side_effect=fetch.OfferRejected("visual_extraction.video_too_large"),
        ),
        patch.object(fetch, "release") as release,
    ):
        fetch.attempt(Mock(), Mock(), {"job_id": "a"}, {})
    release.assert_called_once()
    assert release.call_args.args[2] == (
        "Play authorization failed (visual_extraction.video_too_large)"
    )
    assert release.call_args.kwargs["retry"] is False


def test_lectures_after_login_run_in_parallel():
    import threading
    from unittest.mock import Mock

    import fetch_lecture_videos as fetch

    response = Mock()
    response.headers = {}
    response.json.return_value = [{"job_id": j} for j in "abcde"]
    session = Mock()
    session.post.return_value = response
    # a logs in alone; b-e only finish if they run at the same time.
    barrier = threading.Barrier(4, timeout=5)

    def stage(session, client, slot, downloads):
        if slot["job_id"] != "a":
            barrier.wait()

    with patch.object(fetch, "stage", side_effect=stage) as staged:
        assert fetch.authorize_window(session, Mock()) == 5
    assert staged.call_args_list[0].args[2]["job_id"] == "a"


def test_track_failures_record_their_fixed_reason():
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
            side_effect=TrackError("presentation has no video track"),
        ),
        patch.object(fetch, "release") as release,
    ):
        fetch.authorize_window(session, Mock())
    release.assert_called_once_with(
        session,
        "a",
        "Play authorization failed (presentation has no video track)",
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
    clock = [0]

    def stage(session, client, slot, downloads):
        # The budget runs out while the first lecture is described.
        clock[0] = fetch.FETCH_RUN_SECONDS + 1

    with (
        patch.object(fetch.time, "monotonic", side_effect=lambda: clock[0]),
        patch.object(fetch, "stage", side_effect=stage) as staged,
        patch.object(fetch, "release") as release,
    ):
        assert fetch.authorize_window(session, Mock()) == 2
    staged.assert_called_once()
    release.assert_called_once_with(session, "b", "fetch run time budget", retry=True)
    # No further slots are requested once the budget is spent.
    assert session.post.call_count == 1


def test_sources_offer_low_res_probe_and_1080p_download():
    p = Presentation(
        id="x",
        title="x",
        token="h.eyJleHAiOjQxMDI0NDQ4MDB9.s",
        sources={
            "camera": VideoSource(url_720p="https://m/cam720.mp4", play_audio=True),
            "slides": VideoSource(
                url_720p="https://m/sl720.mp4",
                url_1080p="https://m/sl1080.mp4",
                play_audio=False,
            ),
        },
    )
    sized = []

    def size(client, url):
        sized.append(url)
        return 1000 if "cam" in url else None

    with patch("_play_tracks.media_size", side_effect=size):
        candidates, expires_at = describe_sources(p)
    assert expires_at == 4102444800
    assert [c["source"] for c in candidates] == ["camera", "slides"]
    assert "sl720" in candidates[1]["probe_url"]
    assert "sl1080" in candidates[1]["download_url"]
    assert [c["size_bytes"] for c in candidates] == [1000, None]
    # Only the download variants are sized.
    assert sorted(u.split("?")[0] for u in sized) == [
        "https://m/cam720.mp4",
        "https://m/sl1080.mp4",
    ]


def test_presentation_without_video_is_a_fixed_reason():
    with pytest.raises(TrackError, match="no video track"):
        describe_sources(Presentation(id="x", title="x", sources={}))


class _Response:
    def __init__(self, status, headers):
        self.status_code = status
        self.headers = headers

    def __enter__(self):
        return self

    def __exit__(self, *args):
        pass

    def iter_content(self, *args):
        raise AssertionError("media bytes must not be read")


class _Client:
    def __init__(self, head, ranged):
        self.head_response, self.range_response = head, ranged
        self.requests = []

    def head(self, url, **kwargs):
        self.requests.append(("HEAD", kwargs["headers"]))
        return self.head_response

    def get(self, url, **kwargs):
        self.requests.append(("GET", kwargs["headers"]))
        assert kwargs["stream"]
        return self.range_response


@pytest.mark.parametrize(
    "head, ranged, expected",
    [
        (_Response(200, {"Content-Length": "482474067"}), None, 482474067),
        (
            _Response(405, {}),
            _Response(
                206, {"Content-Length": "1", "Content-Range": "bytes 0-0/482474067"}
            ),
            482474067,
        ),
        (
            _Response(200, {"Content-Length": "512", "Content-Encoding": "gzip"}),
            _Response(206, {"Content-Range": "bytes 0-0/*"}),
            None,
        ),
        (
            _Response(405, {}),
            _Response(206, {"Content-Length": "10", "Content-Range": "bytes 0-0/512"}),
            None,
        ),
        (_Response(405, {}), _Response(200, {"Content-Length": "512"}), None),
    ],
)
def test_sizes_come_from_headers_only(head, ranged, expected):
    client = _Client(head, ranged)
    assert media_size(client, "https://m/v.mp4") == expected
    assert all(h["Accept-Encoding"] == "identity" for _, h in client.requests)
