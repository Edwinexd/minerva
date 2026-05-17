# OCR + Video Indexing Pipeline (issues #34, #36, #46)

Status: design draft, blocked on RunPod license approval.

Revision history: v2 incorporates design-review feedback. Key changes from v1:
- mp4 is no longer archived on Minerva; bundle is the canonical re-processable artifact
- PDF rasterization moves from a GH workflow to the RunPod handler (PDFs are tiny, no round trip needed)
- RunPod fetches bundles via the existing `/api/service/` surface, not a new HMAC-signed blob endpoint
- Polling only, no webhook for v1
- Tiebreaker classifier is CPU OCR on the GH runner, not a RunPod GPU task
- Dropped batched-across-docs RunPod jobs, per-course fps override, runner-up-score plumbing for v1

## Goals

- #34: replace the current pdftotext-style PDF path with DeepSeek-OCR for higher quality text extraction, including figure-aware layout output.
- #36: extract figures (slide images, diagrams, document figures) as first-class chunks linked to their source document, retrievable in chat with thumbnails.
- #46: ingest play.dsv lectures as transcript+slide timelines instead of transcript-only, so retrieval surfaces both spoken content and slide content with timestamp citations.

## Constraints

- No GPU on the prod cluster. GitHub larger/GPU runners require Team plan minimum, repo is personal so unavailable.
- RunPod serverless is the planned GPU host (license pending). All GPU work runs there; everything else runs on free GH Actions runners or the Minerva backend.
- Backend builds with `SQLX_OFFLINE=true` in CI; any new SQL must be `cargo sqlx prepare`d and the `.sqlx/` cache committed.
- Apache trust-boundary rules: any new identity-bearing headers must be unset early in `apache/minerva-app.conf`. No new unauthenticated public surfaces; everything goes through `/api/service/` (service API key) or `/api/admin/` (admin auth).
- play.dsv.su.se returns 3-4 raw mp4 track URLs per presentation with NO labels distinguishing presenter cam vs screen capture vs PIP composite. Track selection is fully visual. Step 1 of the build order verifies this assumption against real data before anything else ships.

## High-level architecture

```
GH Actions (free runner, hourly)
    play-ingest.yml:
        dsv-wrapper auth, fetch presentations
        for each new presentation:
            download all candidate tracks (transient, GH disk only)
            classify visually (CPU features + CPU OCR tiebreak)
            select slide track, detect PIP crop region
            ffmpeg frame sample (output-side seek), blank-frame filter
            bundle frames + metadata
            POST bundle (only) to Minerva
            state -> awaiting_video_index
            mp4 is discarded; play URL is the recoverable source

Minerva backend worker (Rust)
    claim awaiting_ocr / awaiting_video_index docs (one at a time, per-course concurrency cap)
    pre-write runpod_jobs row in 'submitting' state with client_request_id
    submit RunPod async job tagged with client_request_id
    PATCH runpod_jobs row with returned runpod_job_id, state -> 'in_queue'
    set doc state -> processing_ocr / processing_video_index

    poll loop (every 30s):
        for each in-flight runpod_jobs row:
            check RunPod status
            on COMPLETED: persist output, insert figures, doc state -> pending
            on FAILED: bump retry; dead-letter after 3 attempts
            on stuck-in-submitting (no runpod_job_id): reconcile by listing
                recent RunPod jobs and matching by client_request_id

RunPod serverless (GPU, DeepSeek-OCR)
    handle("ocr_pdf"):
        download PDF via service API, pypdfium2 rasterize at 200 DPI,
        DeepSeek-OCR each page, return {pages: [{markdown, figures}]}
    handle("ocr_image"):
        download image via service API, OCR, return {markdown, figures}
    handle("video_index"):
        download bundle via service API, OCR each frame,
        dedupe (exact-match only), fuse VTT cues, return timeline

DeepSeek-OCR weights are baked into the container image at build time
(ghcr image is ~10GB). At cold start RunPod pulls the image only;
no HF download per worker spawn.
```

## State machine additions

`documents.processing_state` adds:

```
awaiting_ocr            // PDF or image, needs DeepSeek-OCR
processing_ocr          // RunPod job in flight
ocr_failed              // dead-letter, admin retryable

awaiting_video_index    // play video, frames bundle uploaded, ready for OCR
vtt_pending             // frames ready but VTT not yet captioned by play
processing_video_index  // RunPod job in flight
video_index_failed      // dead-letter
```

Existing `awaiting_transcript` stays as fallback for the `slide_track_missing` case (transcript-only ingestion using the unchanged path).

VTT-pending half-state mirrors the existing `PresentationNotReadyError` retry in `fetch_transcripts.py`. The GH workflow uploads the frames bundle as soon as frames are extracted; if play hasn't finished captioning yet, the doc enters `vtt_pending` and the worker re-checks each cron tick. Once VTT is available, doc flips to `awaiting_video_index` and the RunPod submission proceeds.

Routing in the worker classifier:

```
mime application/pdf       -> awaiting_ocr
mime image/*               -> awaiting_ocr
mime text/x-url + play.dsv -> awaiting_video_index (was: awaiting_transcript)
mime text/*                -> pending  (already-text, no OCR)
mime text/x-url + other    -> unsupported
```

## New tables

### `document_figures`

```sql
CREATE TABLE document_figures (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    document_id     UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    -- PDF page or null for video
    page            INT,
    -- video timeline or null for PDF
    t_start_seconds REAL,
    t_end_seconds   REAL,
    -- {x,y,w,h} normalized to OCRed image (post-crop)
    bbox            JSONB,
    caption         TEXT,
    -- /data0/minerva/data/figures/<id>.png
    storage_path    TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ON document_figures(document_id);
```

**bbox coordinate system:** normalized to the OCRed image, post-crop. That is, `bbox` is in the same coordinate space DeepSeek-OCR returned it. To recover original-frame pixel coords, combine with the doc's `crop_bbox` (stored on `documents`). This single rule applies to PDF pages (no crop, so bbox is page-relative) and video frames (cropped to slide region before OCR, so bbox is relative to the cropped frame).

Captions plus surrounding text get their own chunks via the existing chunker, with a nullable `figure_id` FK on the chunks table so retrieval can surface the thumbnail alongside the answer text. Visual embeddings are out of scope for v1.

### `runpod_jobs`

```sql
CREATE TABLE runpod_jobs (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- our idempotency key
    client_request_id   TEXT NOT NULL UNIQUE,
    -- null while in 'submitting' state
    runpod_job_id       TEXT UNIQUE,
    -- ocr_pdf | ocr_image | video_index
    task                TEXT NOT NULL,
    document_id         UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    -- submitting | in_queue | in_progress | completed | failed
    status              TEXT NOT NULL,
    submitted_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at        TIMESTAMPTZ,
    output              JSONB,
    error               TEXT,
    retry_count         INT NOT NULL DEFAULT 0,
    -- from RunPod completion payload
    gpu_seconds         REAL,
    -- gpu_seconds * per-second rate
    estimated_cost_usd  REAL
);
CREATE INDEX ON runpod_jobs(status) WHERE status IN ('submitting', 'in_queue', 'in_progress');
CREATE INDEX ON runpod_jobs(document_id);
```

Single doc per job for v1. Cross-doc batching is deferred until billing data shows the per-doc warmup cost is meaningful; partial-failure semantics make batched jobs not worth the complexity for the volumes we'll see initially.

`client_request_id` is generated by the worker before submitting to RunPod and is passed to RunPod as `input.client_request_id`. If the worker crashes between submit and PATCH, on restart it scans `status='submitting'` rows and reconciles by listing recent RunPod jobs whose `input.client_request_id` matches. No leaked GPU spend.

### Schema additions on `documents`

```sql
-- 'high' (DeepSeek) | 'fallback' (pdftotext)
ALTER TABLE documents ADD COLUMN ocr_quality TEXT;
-- which mp4 from play
ALTER TABLE documents ADD COLUMN selected_track_index INT;
-- for retrospective tuning
ALTER TABLE documents ADD COLUMN slide_track_score REAL;
ALTER TABLE documents ADD COLUMN slide_track_missing BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE documents ADD COLUMN slide_track_user_corrected BOOLEAN NOT NULL DEFAULT FALSE;
-- {x,y,w,h} or null, frame-pixel coords
ALTER TABLE documents ADD COLUMN crop_bbox JSONB;
-- e.g. '1/5', for re-processing parity
ALTER TABLE documents ADD COLUMN sample_fps TEXT;
-- cumulative across re-processings
ALTER TABLE documents ADD COLUMN ocr_gpu_seconds REAL DEFAULT 0;
```

Per-course `video_sample_fps` override is dropped from v1 (subagent feedback: nobody tunes this). Sample fps is hardcoded to `1/5` as an env-configurable constant; revisit if a course actually needs a different rate.

`slide_track_score_runner_up` is dropped from v1 (subagent feedback: useless until ground-truth feedback exists). The user-correction button is enough; if patterns emerge in corrections we add the runner-up column then.

## New service API endpoints

All under `MINERVA_SERVICE_API_KEY` (same pattern as existing `/api/service/`). No new public surfaces and no new identity-bearing headers; the only Apache change is allowing larger request bodies on the bundle-upload endpoint.

```
POST /api/service/documents/{id}/video-bundle
    multipart: bundle.tar.zst, metadata_json
    metadata_json: {selected_track_index, slide_track_score, crop_bbox, sample_fps}
    sets state = awaiting_video_index (or vtt_pending if no VTT yet)

GET  /api/service/documents/{id}/video-bundle
    streams the stored bundle. Auth via service API key (RunPod uses this).

GET  /api/service/documents/{id}/source
    streams the original PDF/image blob. Auth via service API key (RunPod uses this).

GET  /api/service/pending-ocr?limit=N
    returns [{id, course_id, mime, source_url, retry_count}]
    source_url is the absolute URL of /api/service/documents/{id}/source

GET  /api/service/pending-video-index?limit=N
    returns [{id, course_id, bundle_url, vtt_text, sample_fps, retry_count}]
    bundle_url is the absolute URL of /api/service/documents/{id}/video-bundle

POST /api/service/figure-uploads/{document_id}
    multipart: figure_id, png file
    auth via service API key (RunPod uses this for figure crops too large to inline)
```

Track-correction endpoint (teacher-facing, not service):

```
POST /api/courses/{id}/documents/{doc_id}/correct-track
    body: {selected_track_index: N}
    re-queues the document with the new track choice, marks slide_track_user_corrected = true
```

Re-processing endpoint (admin):

```
POST /api/admin/courses/{id}/reocr
    body: {scope: "all" | "video" | "pdf"}
    flips matching docs in the course back to awaiting_ocr / awaiting_video_index
    rate-limited: enqueues with a per-course concurrency cap so a 100-lecture course
    doesn't dump 100 jobs into RunPod at once
```

## Track classification (visual, no labels)

Runs entirely on the GH worker before any frames are uploaded to Minerva.

### Probe

For each of the 3-4 mp4 URLs:

1. Try byte-range fetch of the first 2 MB to check if `moov` atom is at start. If yes, byte-range seeks work and probing is cheap. If no (`moov` at end), fall back to full download for that track. GH free runners have plenty of bandwidth and this is a per-presentation cost (a few times per week per course), not per-cron-tick.
2. Sample 10 frames at percentile timestamps `[5, 15, 25, 40, 55, 70, 80, 90, 95]` plus one random in `[0, 100]`.
3. Use ffmpeg `-ss <t> -i <url> -frames:v 1` (input-side seek). Inaccurate keyframe-aligned seeks are fine for classification probes.

### Per-frame features

```python
def features(frame_bgr):
    gray = cv2.cvtColor(frame_bgr, cv2.COLOR_BGR2GRAY)
    return {
        'edge_density':   cv2.Canny(gray, 50, 150).mean() / 255.0,
        'horiz_lines':    hough_horizontal_count(gray) / frame.shape[0],
        'text_regions':   mser_or_east_count(frame_bgr),
        'face_count':     yunet_detect(frame_bgr),     # ONNX, ~30ms CPU
        'color_std':      cv2.cvtColor(frame_bgr, cv2.COLOR_BGR2HSV)[..., 1].std() / 255.0,
    }
```

`temporal_std` is computed across the 10 samples per track (per-pixel std-dev, then mean).

### Aggregate score

```python
def slide_score(track_features, temporal_std):
    z = standardize_across_tracks  # so scores compare across this presentation's candidates
    return (
        + 1.0 * z('edge_density')
        + 0.5 * z('horiz_lines')
        + 1.0 * z('text_regions')
        - 1.0 * z('color_std')
        - 2.0 * z('face_count')
        - 0.5 * z('temporal_std')
    )
```

Weights are starting points; tune offline (see Build order, step 1-3).

### Decision

```
best, runner_up = top 2 tracks by slide_score
if best.score > HIGH and best - runner_up > MARGIN:
    use best, run PIP detection
else:
    CPU-OCR tiebreak (rapidocr/paddleocr ONNX, runs on GH worker):
        OCR 5 sample frames per candidate, sum char counts
        pick highest-char-count track
        if all char_counts < MIN_CHARS:
            slide_track_missing = true
            ingest transcript-only via existing pipeline
```

CPU OCR tiebreak replaces the v1 plan's `classify_frames` GPU task. Reasoning: paying GPU warmup just to count characters is silly, and rapidocr / paddleocr ONNX runs in seconds on the GH worker per frame. Same oracle (does it OCR well?), no RunPod cost, no extra warmup.

`slide_track_score` is persisted on the doc. Runner-up score is dropped for v1.

### PIP / composite detection

Runs on the chosen track. 60s of frames sampled at 2 fps (120 frames). Per-pixel temporal std-dev:

```python
motion = np.stack(frames).std(axis=0).mean(axis=-1)
static_mask = motion < motion_threshold
bbox = largest_rect_in_mask(static_mask)
if bbox and bbox.area > 0.30 * frame.area and aspect_in([4/3, 16/9, 16/10], tol=0.15):
    crop_bbox = bbox
elif bbox and bbox.area > 0.80 * frame.area:
    crop_bbox = None  # whole frame is the slide, no PIP
else:
    crop_bbox = None  # static region too small, probably logo/UI chrome
```

Persist `crop_bbox` on the doc. Frame extraction applies the crop before bundling.

## Frame extraction

Once the slide track is selected and crop is decided, GH does the real frame extraction:

```bash
ffmpeg -i <track_url> -vf "fps=1/5,crop=W:H:X:Y" frames/f_%05d.png
```

Order is `-i ... -ss` (output-side seek) where seek is needed; for full-video extraction at fixed fps the `-ss` is omitted entirely. Output-side filtering is accurate at frame boundaries, slower than input-side but fine for hourly cron.

Then on-runner pre-filter:

- Drop frames with luma std-dev below threshold (black/blank frames)
- Drop frames whose mean intensity is uniform (test patterns, fade transitions)

Pre-filter cuts ~30-50% of frames before they're bundled, saving GPU cost.

Bundle layout:

```
<bundle>.tar.zst
├── manifest.json   # {doc_id, sample_fps, crop_bbox, frame_count, source_track_url, source_track_sha256}
├── frames/0001.png
├── frames/0002.png
└── ...
```

Multipart-uploaded to Minerva via `POST /api/service/documents/{id}/video-bundle`. Bundle sizes for an hour-long lecture at fps=1/5 with pre-filter are typically 50-150 MB compressed; well within Apache's `LimitRequestBody` after we bump it for this endpoint.

## RunPod handler

Single endpoint, three task types dispatched on input. DeepSeek-OCR weights are baked into the image at build time, not pulled at runtime.

```python
# runpod-worker/handler.py
import runpod, requests, tempfile, os, glob, base64, io, tarfile, zstandard, json
from deepseek_ocr import DeepSeekOCR
from PIL import Image
import pypdfium2

# Loaded once per worker spawn from a path inside the image.
# COPY ./model_weights/ /opt/deepseek-ocr/ in the Dockerfile.
model = DeepSeekOCR.load("/opt/deepseek-ocr/")  # cold start ~20-25s for weights to memory

SVC_BASE = os.environ["MINERVA_API_BASE"]
SVC_KEY = os.environ["MINERVA_SERVICE_API_KEY"]

def svc_get(path):
    r = requests.get(f"{SVC_BASE}{path}", headers={"X-Service-Key": SVC_KEY},
                     stream=True, timeout=300)
    r.raise_for_status()
    return r

def fetch_to(path, url):
    with svc_get(url) as r:
        with open(path, "wb") as f:
            for c in r.iter_content(1 << 20):
                f.write(c)

def ocr_page(img):
    return model.run(img)  # {markdown, figures: [{bbox, caption, crop_b64}]}

def ocr_pdf(inp):
    with tempfile.TemporaryDirectory() as d:
        pdf_path = f"{d}/in.pdf"
        fetch_to(pdf_path, inp["source_url"])
        pdf = pypdfium2.PdfDocument(pdf_path)
        pages = []
        for i in range(len(pdf)):
            img = pdf[i].render(scale=200/72).to_pil()
            pages.append(ocr_page(img))
        return {"pages": pages}

def ocr_image(inp):
    with tempfile.TemporaryDirectory() as d:
        img_path = f"{d}/in.bin"
        fetch_to(img_path, inp["source_url"])
        return ocr_page(Image.open(img_path))

def video_index(inp):
    with tempfile.TemporaryDirectory() as d:
        bundle_path = f"{d}/bundle.tar.zst"
        fetch_to(bundle_path, inp["bundle_url"])
        with open(bundle_path, "rb") as f:
            with zstandard.ZstdDecompressor().stream_reader(f) as r:
                with tarfile.open(fileobj=r, mode="r|") as tar:
                    tar.extractall(d)
        manifest = json.load(open(f"{d}/manifest.json"))
        seconds_per_frame = parse_fps(manifest["sample_fps"])
        frames = []
        for i, p in enumerate(sorted(glob.glob(f"{d}/frames/*.png"))):
            img = Image.open(p)
            out = ocr_page(img)
            md = out["markdown"].strip()
            if len(md) < 30:
                continue
            frames.append({"t": i * seconds_per_frame, "markdown": md,
                           "figures": out["figures"]})

        # Exact-match dedupe only, after whitespace normalization.
        # Bias toward over-segmenting: build-by-build slides where one bullet is
        # added produce different OCR output and will be kept as separate spans.
        # Retrieval can merge later; we cannot recover lost timestamps.
        deduped = []
        for f in frames:
            norm = " ".join(f["markdown"].split())
            if deduped and deduped[-1]["_norm"] == norm:
                deduped[-1]["t_end"] = f["t"]
            else:
                f["t_start"] = f["t"]; f["t_end"] = f["t"]; f["_norm"] = norm
                deduped.append(f)
        for span in deduped: del span["_norm"]

        cues = parse_vtt(inp["vtt_text"])
        for span in deduped:
            span["vtt_text"] = " ".join(
                c.text for c in cues
                if c.start_seconds < span["t_end"] + seconds_per_frame
                and c.end_seconds > span["t_start"]
            )
        return {"timeline": deduped}

def handle(job):
    t = job["input"]["task"]
    if t == "ocr_pdf":     return ocr_pdf(job["input"])
    if t == "ocr_image":   return ocr_image(job["input"])
    if t == "video_index": return video_index(job["input"])
    return {"error": f"unknown task: {t}"}

runpod.serverless.start({"handler": handle})
```

Container: cuda 12.x base, torch, transformers, deepseek-ocr (weights baked in), pypdfium2, runpod sdk, zstandard, pillow, requests. Pushed to ghcr; RunPod template references the ghcr image.

Endpoint config:
- `min_workers=0`, `max_workers=2` (start)
- Async job timeout: **60 minutes**. A 1hr lecture at 720 frames pre-filtered to ~360, OCR ~2s/frame = ~12 min wallclock; 60 min gives 5x headroom for slower frames or re-tries.
- Spend cap: hard daily limit checked by the backend circuit breaker, not RunPod-side (RunPod has no native cap).

## Backend worker changes

In the existing worker loop (Rust), add task families with idempotency-first submission:

```rust
// pseudocode
async fn submit_runpod_job(doc: &Document, task: &str, input: Value) -> Result<()> {
    let client_request_id = format!("doc-{}-{}", doc.id, Uuid::new_v4());

    // 1. Pre-write 'submitting' row so we never lose track of an in-flight job.
    sqlx::query!(
        "INSERT INTO runpod_jobs (client_request_id, task, document_id, status)
         VALUES ($1, $2, $3, 'submitting')",
        client_request_id, task, doc.id
    ).execute(&db).await?;

    // 2. Submit (with our request id embedded in the input for reconciliation).
    let mut input = input;
    input["client_request_id"] = client_request_id.clone().into();
    let job = runpod::submit(task, input).await?;

    // 3. Patch with the runpod-side id and flip to in_queue.
    sqlx::query!(
        "UPDATE runpod_jobs SET runpod_job_id = $1, status = 'in_queue'
         WHERE client_request_id = $2",
        job.id, client_request_id
    ).execute(&db).await?;

    set_state(doc.id, doc_state_for_task(task)).await?;
    Ok(())
}

async fn reconcile_orphans() -> Result<()> {
    // Run on worker startup and periodically.
    let orphans = sqlx::query!(
        "SELECT client_request_id, document_id FROM runpod_jobs
         WHERE status = 'submitting' AND submitted_at < now() - interval '5 minutes'"
    ).fetch_all(&db).await?;

    for o in orphans {
        // List recent RunPod jobs, match on client_request_id in input metadata.
        if let Some(j) = runpod::find_by_client_id(&o.client_request_id).await? {
            sqlx::query!(
                "UPDATE runpod_jobs SET runpod_job_id = $1, status = $2
                 WHERE client_request_id = $3",
                j.id, j.status, o.client_request_id
            ).execute(&db).await?;
        } else {
            // Submission never reached RunPod, safe to mark failed and retry.
            sqlx::query!(
                "UPDATE runpod_jobs SET status = 'failed',
                                       error = 'submission_orphaned'
                 WHERE client_request_id = $1",
                o.client_request_id
            ).execute(&db).await?;
        }
    }
    Ok(())
}

async fn poll_runpod_jobs() -> Result<()> {
    let in_flight = sqlx::query!(
        "SELECT id, runpod_job_id, document_id, task FROM runpod_jobs
         WHERE status IN ('in_queue', 'in_progress')"
    ).fetch_all(&db).await?;

    for j in in_flight {
        let status = runpod::status(&j.runpod_job_id).await?;
        match status.status.as_str() {
            "COMPLETED" => apply_runpod_output(&j, status).await?,
            "FAILED" => bump_retry_or_dead_letter(&j, &status.error).await?,
            "IN_QUEUE" | "IN_PROGRESS" => update_status(&j.id, &status.status).await?,
            _ => {}
        }
    }
    Ok(())
}
```

Concurrency caps:
- Per-course in-flight cap (configurable, default 3) so a course with many lectures doesn't saturate RunPod budget on one cron tick.
- Global cap = `max_workers` = 2 to start; matches RunPod-side ceiling.

## GitHub Actions workflows

### `play-ingest.yml` (replaces transcript-only path for play sources)

```yaml
on:
  schedule: [{ cron: "15 * * * *" }]
  workflow_dispatch:
concurrency: { group: play-ingest, cancel-in-progress: false }
jobs:
  ingest:
    runs-on: ubuntu-latest
    timeout-minutes: 60
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with: { python-version: "3.13" }
      - run: pip install -r scripts/requirements-ingest.txt
      - run: python scripts/fetch_play_videos.py
        env:
          MINERVA_API_BASE: https://minerva.dsv.su.se
          MINERVA_SERVICE_API_KEY: ${{ secrets.MINERVA_SERVICE_API_KEY }}
          SU_USERNAME: ${{ secrets.SU_USERNAME }}
          SU_PASSWORD: ${{ secrets.SU_PASSWORD }}
```

`scripts/fetch_play_videos.py` does:
1. Catalog push (existing logic, kept)
2. Discovery (existing logic, kept)
3. For each new `awaiting_video_index` doc: dsv-wrapper download all candidate tracks to GH disk, classify visually (CPU features + CPU OCR tiebreak as needed), pick track + crop, ffmpeg sample, drop blank frames, tar.zst bundle, multipart POST bundle to Minerva. Discard mp4s.
4. For tracks that fail classification entirely: POST transcript-only via existing service endpoint, set `slide_track_missing = true`.

There is **no `pdf-ingest.yml`**. PDFs and images stay on Minerva and get sent to RunPod by the backend worker. RunPod handles rasterization itself in `ocr_pdf`.

## Storage layout

```
/data0/minerva/data/
├── bundles/<doc_id>.tar.zst          # canonical re-processable artifact (frames + manifest + VTT)
├── <doc_id>.md                       # generated text body for chunker
└── figures/<figure_id>.png           # slide thumbnails for citations

# No videos/ directory. mp4s are transient on the GH runner; play.dsv URL
# in the document is the recoverable source if frames need re-extraction
# at a different sample rate.
```

Janitor:
- On `documents` row delete (cascade or explicit), remove `bundles/<doc_id>.tar.zst`, `<doc_id>.md`, and all `figures/<figure_id>.png` for that doc. Implemented as a backend post-delete hook (Rust) rather than a DB trigger so it's testable and observable.
- Bundles are kept indefinitely so re-OCR on model upgrade doesn't require re-extraction. If `/data0` fills, GC oldest bundles by access time; their frames can be regenerated from play.dsv.

## Frontend changes

- Chat citation rendering: when chunk has `figure_id`, render thumbnail link to `/api/figures/{id}/thumbnail` (course-membership auth).
- Video citations: `MM:SS` link that deep-links to play.dsv with `?t=N` query.
- Admin doc detail: show selected track index, slide_track_score, crop bbox; teacher can click "wrong track? select a different one" which calls the correct-track endpoint and re-queues.
- Admin filter: "videos with missing slide track" surfaces docs where transcript-only fallback fired.
- Admin re-OCR button per course (calls `/api/admin/courses/{id}/reocr`).

## Cost / batching strategy

- DeepSeek-OCR cold start: ~20-25s once weights are baked into the image. Per-frame inference: ~1-3s.
- PDFs and images: single doc per RunPod job for v1. Worker submits as soon as a doc is `awaiting_ocr`. Cross-doc batching deferred until billing data justifies the partial-failure complexity.
- Videos: single video per job. 24min worst-case inference dwarfs warmup; per-course concurrency cap of 3 prevents backlog bursts.
- Tiebreak: CPU OCR on the GH runner. Zero RunPod cost.
- Pre-filter: drop blank/luma-static frames on GH before bundling.
- `min_workers=0` (scale to zero). Switch to `min_workers=1` only if daily ingest volume justifies always-on cost.

### Cost accounting

Per-job `gpu_seconds` and `estimated_cost_usd` are recorded on `runpod_jobs` at completion. Per-doc cumulative cost on `documents.ocr_gpu_seconds` lets admin UI show "cost per course / per doc / per teacher". Daily aggregate feeds the circuit breaker:

```
if today's total estimated_cost_usd > MINERVA_RUNPOD_DAILY_BUDGET_USD:
    pause new submissions (state stays awaiting_*; cron resumes next day)
    alert via existing admin notification path
```

This is separate from the existing per-owner LLM-token caps because units are different (GPU seconds vs LLM tokens). The two layers compose: a teacher with no LLM budget still gets their videos OCRed; a teacher who's blown the OCR budget still has chat available.

## Build order

This is the bit to push hard on, given the no-labels classification problem.

1. **Inspect dsv-wrapper output for real DSV lectures.** Confirm we get 3-4 mp4 URLs, check moov-atom placement across tracks and courses, sample frame quality. Check whether URLs are signed / range-fetchable / HLS-only. Check whether VTT is reliably available within an hour of a lecture or hours-to-days late. **Output: a notebook or doc with concrete findings; the rest of the plan is conditional on what's actually there.**
2. **Hand-label a corpus.** 30-50 lectures across courses, recording years, and formats. For each: which track index is slides, is lecture cam-only, is the slide track composite/PIP, is it a chalkboard/document-camera lecture. 2-3h of work.
3. **Build offline classifier evaluation.** Standalone Python script: input = (track URLs, ground-truth label), output = confusion matrix + per-feature contribution + tuning report. Iterate until >95% accuracy on holdout. **This is the gate before any GPU infrastructure ships.**
4a. **Migrations + state machine + tables, behind a feature flag.** Ships first, no endpoints yet. Lets the offline classifier eval write provenance into the real DB while iterating.
4b. **Service API endpoints.** Once migrations are in, add the endpoints behind the same flag. Stub RunPod with a fake handler that returns canned outputs to validate the worker loop end-to-end.
5. **RunPod handler + ghcr image** with weights baked in. Validate end-to-end with a few hand-picked PDFs and one lecture.
6. **`play-ingest.yml` + worker ocr submission.** Ship behind feature flag (`MINERVA_OCR_PIPELINE_ENABLED=true`); default off. Migrate one course manually, validate output quality, then expand.
7. **Frontend: figure thumbnails + track correction UI + admin re-OCR.** Shipped after data is flowing so design is grounded in real outputs.
8. **Feedback loop.** Teacher "wrong slide track" data accumulates. Quarterly: pull as new validation samples, re-tune thresholds. If patterns emerge, add `slide_track_score_runner_up` then.

## Open questions (now mostly unblocked)

- **RunPod payload limits.** Verify async job request body and response size caps. If response is capped (e.g. 10MB), figure crops in `video_index` output won't fit; handler uploads crops to `/api/service/figure-uploads/{document_id}` and returns only metadata. The infrastructure for this is already in the plan, but we need to confirm the threshold to know whether to enable it always or only on overflow.
- **Apache `LimitRequestBody` for bundle uploads.** Bundles are 50-150 MB. Bump on the bundle endpoint specifically; don't change the default for other routes. Stream to disk on the backend, don't buffer in memory.
- **Whiteboard / document-camera lectures.** Visual classifier should let these through (low color_std, lots of text-like edges). OCR quality on handwriting varies. Validate during step 3.
- **Re-processing on model upgrade.** `/admin/courses/{id}/reocr` button ships in step 7; the backend rate-limits via per-course concurrency cap so a 100-lecture course doesn't dump 100 jobs simultaneously.
- **Whether DSV ever issues short-lived signed mp4 URLs.** If yes, reconciling orphans (worker crashed mid-submit) might need a re-fetch step because the URL in the input has expired. Worth checking in step 1.

## Out of scope for v1

- Visual embeddings on figures (CLIP-style). Caption-only retrieval is the v1.
- Cross-document figure dedup (same diagram in multiple lectures).
- Auto-detection of "the teacher uploaded a slide PDF separately, link it to this video". Future enhancement when both ingestion paths are stable.
- LTI / external embed integration with the new figure-aware retrieval. Once retrieval works internally, embed surfaces it for free.
- mp4 archival on Minerva. Re-fetched from play.dsv on demand if a re-extraction at a different fps is ever needed.
- Per-course `video_sample_fps` override.
- Cross-doc batched RunPod jobs.
- `slide_track_score_runner_up` plumbing.
- RunPod webhook callback (polling only).
