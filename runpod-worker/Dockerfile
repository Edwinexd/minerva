# RunPod serverless worker image for Minerva's OCR + video-indexing pipeline.
#
# Build context is `runpod-worker/`. The DeepSeek-OCR-2 weights are
# downloaded at build time so they ship inside the image; cold-start at
# RunPod then only pays for image pull (~10GB) plus weights -> VRAM
# load (~20-25s on T4-class hardware), not a fresh HF download per
# worker spawn. That trade-off matters because RunPod's `min_workers=0`
# scale-to-zero means every quiet hour rebuys the cold start.
#
# Two-stage build keeps the runtime layer slim: stage 1 has git/curl
# and the HF CLI for fetching weights, stage 2 only carries the model
# files + Python deps.

# --- Stage 1: weights download -----------------------------------------
FROM python:3.11-slim AS weights

ARG DEEPSEEK_OCR_REPO=deepseek-ai/DeepSeek-OCR-2
ARG HF_REVISION=main

RUN apt-get update \
 && apt-get install -y --no-install-recommends git git-lfs ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && git lfs install

# Download once, into a known path. We do NOT use `transformers`'
# auto-download at runtime because that would re-fetch on every cold
# start. `huggingface_hub` snapshot is enough; the model loader picks
# up files from this directory by name.
RUN pip install --no-cache-dir "huggingface_hub>=0.24,<1"
RUN python -c "import os; from huggingface_hub import snapshot_download; \
    snapshot_download(repo_id=os.environ['DEEPSEEK_OCR_REPO'], \
                      revision=os.environ['HF_REVISION'], \
                      local_dir='/opt/deepseek-ocr', \
                      local_dir_use_symlinks=False)" \
    -e DEEPSEEK_OCR_REPO=${DEEPSEEK_OCR_REPO} \
    -e HF_REVISION=${HF_REVISION} \
 || python - <<'PY'
import os
from huggingface_hub import snapshot_download
snapshot_download(
    repo_id=os.environ.get("DEEPSEEK_OCR_REPO", "deepseek-ai/DeepSeek-OCR-2"),
    revision=os.environ.get("HF_REVISION", "main"),
    local_dir="/opt/deepseek-ocr",
)
PY

# --- Stage 2: runtime --------------------------------------------------
FROM nvidia/cuda:12.4.0-runtime-ubuntu22.04

ENV DEBIAN_FRONTEND=noninteractive \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PIP_NO_CACHE_DIR=1 \
    MINERVA_OCR_MODEL_PATH=/opt/deepseek-ocr \
    MINERVA_OCR_MODEL_NAME=deepseek-ai/DeepSeek-OCR-2

# Python + minimal native deps. `libgl1` and `libglib2.0-0` are pulled
# in because Pillow/pypdfium2 need them for image decode + Cairo render.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        python3.11 python3.11-venv python3-pip \
        libgl1 libglib2.0-0 ca-certificates \
 && ln -sf /usr/bin/python3.11 /usr/local/bin/python \
 && rm -rf /var/lib/apt/lists/*

# Install Python deps first so changing handler.py doesn't bust the layer.
COPY requirements.txt /tmp/requirements.txt
RUN python -m pip install --upgrade pip \
 && python -m pip install -r /tmp/requirements.txt

# DeepSeek-OCR-2's runtime adapter (the `deepseek_ocr` Python package).
# If it ever ships under a different package name we override here
# rather than touching the handler. The HF repo includes a setup
# stanza in its README; we follow the upstream-recommended install.
RUN python -m pip install "deepseek-ocr>=2.0,<3" || \
    python -m pip install "git+https://github.com/deepseek-ai/DeepSeek-OCR.git@v2"

# Copy weights from stage 1.
COPY --from=weights /opt/deepseek-ocr /opt/deepseek-ocr

# Handler last so source edits don't invalidate the heavy layers.
WORKDIR /app
COPY handler.py /app/handler.py

# RunPod looks for a `handler` callable; entrypoint runs the SDK's
# serverless loop which dispatches to ours.
CMD ["python", "-u", "/app/handler.py"]
