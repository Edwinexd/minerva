import importlib.machinery
import importlib.util
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
from _play_tracks import choose_source, pick_track, probe_url, text_score  # noqa: E402
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


def test_blank_or_tied_probes_do_not_guess_camera():
    for scores in ({"a": 0, "b": 0}, {"a": 100, "b": 99}, {"a": 10, "b": 2}):
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
        headers = {
            "Content-Length": str(32 * 1024 * 1024),
            "Content-Range": "bytes 0-33554431/33554432",
        }

        def __enter__(self):
            return self

        def __exit__(self, *args):
            pass

        def iter_content(self, chunk_size):
            for _ in range(512):
                read_chunks.append(chunk_size)
                yield b"x" * chunk_size

    with patch("_play_tracks.requests.get", return_value=Response()):
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
