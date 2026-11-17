import base64
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
from check_secret_removal import filled_keys, load_objects, lost_keys, main  # noqa: E402


def b64(s: str) -> str:
    return base64.b64encode(s.encode()).decode()


def new_manifest(**keys: str) -> list[dict]:
    return [{"kind": "Secret", "metadata": {"name": "minerva-secrets"}, "stringData": keys}]


def live_secret(**keys: str) -> list[dict]:
    return [
        {
            "kind": "Secret",
            "metadata": {"name": "minerva-secrets"},
            "data": {k: b64(v) for k, v in keys.items()},
        },
    ]


def test_emptied_key_is_lost():
    new = filled_keys(new_manifest(OPENAI_API_KEY="sk", MISTRAL_API_KEY=""))
    live = filled_keys(live_secret(OPENAI_API_KEY="sk", MISTRAL_API_KEY="m"))
    assert lost_keys(new, live) == ["minerva-secrets/MISTRAL_API_KEY"]


def test_missing_key_is_lost():
    new = filled_keys(new_manifest(OPENAI_API_KEY="sk"))
    live = filled_keys(live_secret(OPENAI_API_KEY="sk", MISTRAL_API_KEY="m"))
    assert lost_keys(new, live) == ["minerva-secrets/MISTRAL_API_KEY"]


def test_empty_on_cluster_is_not_lost():
    new = filled_keys(new_manifest(MISTRAL_API_KEY=""))
    live = filled_keys(live_secret(MISTRAL_API_KEY=""))
    assert lost_keys(new, live) == []


def test_new_and_rotated_keys_pass():
    new = filled_keys(new_manifest(OPENAI_API_KEY="rotated", GROQ_API_KEY="g"))
    live = filled_keys(live_secret(OPENAI_API_KEY="sk"))
    assert lost_keys(new, live) == []


def test_secret_not_in_manifest_is_ignored():
    new = filled_keys(new_manifest(OPENAI_API_KEY="sk"))
    live = filled_keys(live_secret(OPENAI_API_KEY="sk"))
    live["grafana-admin"] = {"admin-password"}
    assert lost_keys(new, live) == []


def test_main_blocks_unless_allowed(tmp_path, capsys):
    new = tmp_path / "new.json"
    live = tmp_path / "live.json"
    # kubectl's shapes: back-to-back objects for a manifest, a List for `get`.
    new.write_text(
        "\n".join(json.dumps(o, indent=4) for o in new_manifest(MISTRAL_API_KEY="") * 2)
    )
    live.write_text(json.dumps({"kind": "List", "items": live_secret(MISTRAL_API_KEY="secret-value")}))

    assert main([str(new), str(live)]) == 1
    assert main([str(new), str(live), "--allow", "MISTRAL_API_KEY"]) == 0
    assert main([str(new), str(live), "--allow", "minerva-secrets/MISTRAL_API_KEY"]) == 0
    assert "secret-value" not in capsys.readouterr().out


def test_load_objects_reads_concatenated_documents_and_lists():
    text = '{"kind": "Secret", "metadata": {"name": "a"}}\n{"kind": "List", "items": [{"kind": "Secret", "metadata": {"name": "b"}}]}\n'
    assert [o["metadata"]["name"] for o in load_objects(text)] == ["a", "b"]
    assert load_objects("") == []
