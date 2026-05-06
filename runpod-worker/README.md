# Minerva RunPod worker

GPU handler image for Minerva's OCR + video-indexing pipeline. Runs on
RunPod serverless; built and pushed by
`.github/workflows/runpod-worker-image.yml`.

See `docs/plans/ocr-video-pipeline.md` for the architecture.

## Layout

- `handler.py` - dispatches `ocr_pdf` / `ocr_image` / `video_index` tasks.
- `Dockerfile` - two-stage build that bakes DeepSeek-OCR-2 weights into
  the runtime image so cold starts skip a HuggingFace pull.
- `requirements.txt` - non-torch Python deps. Torch comes in via the
  `deepseek-ocr` install in the Dockerfile.

## Required environment at runtime

The backend embeds these in each job's input payload, so RunPod itself
only needs `MINERVA_SERVICE_API_KEY` set as an endpoint secret:

| Variable | Source |
| --- | --- |
| `MINERVA_SERVICE_API_KEY` | RunPod endpoint secret. Same value as the backend's `MINERVA_SERVICE_API_KEY`. |

Optional tuning knobs:

| Variable | Default | Purpose |
| --- | --- | --- |
| `MINERVA_OCR_MODEL_PATH` | `/opt/deepseek-ocr` | Where the baked weights live. |
| `MINERVA_OCR_MODEL_NAME` | `deepseek-ai/DeepSeek-OCR-2` | Logged for traceability. |
| `MINERVA_OCR_PDF_DPI` | `200` | PDF rasterization DPI. |
| `MINERVA_OCR_MIN_CHARS` | `30` | Drop frames whose OCR markdown is shorter than this. |

## Local smoke test

Without GPU you can import-check the handler:

```sh
cd runpod-worker
python -c "import importlib, handler; print('imports clean')"
```

A real end-to-end test requires GPU + the DeepSeek-OCR-2 weights; the
GHA workflow runs a pull-then-import smoke job after each successful build.
