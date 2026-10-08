"""Throughput of Unlimited-OCR on vLLM over a directory of slide frames, and
how close its text is to the HF `model.infer` output cached next to them.

    python3 bench_vllm.py runs/<uuid>/slides
"""

import argparse
import json
import re
import time
from collections import Counter
from pathlib import Path

from PIL import Image

from ocr_vllm import VllmOcr


def words(text):
    return Counter(re.findall(r"\w+", re.sub(r"<\|.*?\|>", " ", text).lower()))


def overlap(a, b):
    total = sum(a.values()) + sum(b.values())
    return 2 * sum((a & b).values()) / total if total else 1.0


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("slides", type=Path)
    parser.add_argument("--gpu-memory-utilization", type=float, default=0.6)
    args = parser.parse_args()

    paths = sorted(p for p in args.slides.glob("*.jpg") if ".fig" not in p.name)
    images = [Image.open(p) for p in paths]

    started = time.monotonic()
    ocr = VllmOcr(gpu_memory_utilization=args.gpu_memory_utilization)
    load = time.monotonic() - started

    started = time.monotonic()
    texts = ocr(images)
    wall = time.monotonic() - started

    tokens = sum(len(ocr.llm.get_tokenizer().encode(t)) for t in texts)
    similarity = []
    for path, text in zip(paths, texts):
        reference = path.with_name(f"{path.stem}.unlimited-ocr.txt")
        if reference.exists():
            similarity.append(overlap(words(text), words(reference.read_text())))
        path.with_name(f"{path.stem}.unlimited-ocr-vllm.txt").write_text(text + "\n")

    print(json.dumps({
        "slides": len(images),
        "load_s": round(load, 1),
        "wall_s": round(wall, 1),
        "s_per_slide": round(wall / len(images), 3),
        "tokens_per_s": round(tokens / wall, 1),
        "mean_tokens_per_slide": round(tokens / len(images)),
        "word_overlap_vs_hf_mean": round(sum(similarity) / len(similarity), 3) if similarity else None,
        "word_overlap_vs_hf_min": round(min(similarity), 3) if similarity else None,
    }))


if __name__ == "__main__":
    main()
