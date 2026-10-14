import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
for module in ("attr", "av", "numpy", "PIL"):
    pytest.importorskip(module)
spec = importlib.util.spec_from_file_location(
    "slide_ocr_extract", ROOT / "gpu/slide-ocr/extract.py"
)
extract = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = extract
spec.loader.exec_module(extract)


@pytest.mark.parametrize(
    "box",
    [
        "[{'x': 1}, 2, 3, 4]",
        "[[{'x': 1}, 2, 3, 4]]",
        "{'x1': 1, 'y1': 2}",
        "[[1, 2, 3]]",
        "[[True, 2, 3, 4]]",
        "'not a box'",
        "[[1, 2, 3, 4",
    ],
)
def test_malformed_boxes_are_dropped_and_text_kept(box):
    raw = f"<|ref|>text<|/ref|><|det|>{box}<|/det|>Slide title"
    assert extract.parse_blocks(raw) == [
        {"label": "text", "boxes": [], "text": "Slide title"}
    ]


def test_well_formed_boxes_become_fractions():
    grid = extract.GRID
    raw = (
        f"<|ref|>image<|/ref|><|det|>[[0, 0, {grid}, {grid // 2}], "
        f"[1, 2, 3]]<|/det|>caption"
    )
    [block] = extract.parse_blocks(raw)
    assert block["boxes"] == [[0.0, 0.0, 1.0, round((grid // 2) / grid, 4)]]
