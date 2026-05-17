# Agent brief: play.dsv ingestion (slide-track classifier + frame bundler)

Self-contained brief for an agent that builds the GitHub Actions side of
the OCR + video-indexing pipeline. The Minerva backend, RunPod worker
image, and service endpoints are already shipped on
`feat/ocr-pipeline-foundation` (commits `e2b6850`, `02090cf`); your job
is the missing GH workflow + Python script that feeds them.

## What you're building

`.github/workflows/play-ingest.yml` plus `scripts/fetch_play_videos.py`.
The script is what the workflow runs. End-state behavior, hourly:

1. Push the catalog of play.dsv course designations to Minerva
   (existing `PUT /api/service/play-courses`).
2. For each watched designation per course, idempotently register newly
   available presentations as `text/x-url` documents via the existing
   `POST /api/service/courses/{id}/documents/url`.
3. Fetch the list of new `awaiting_video_index` documents from Minerva
   via the new `GET /api/service/pending-video-index`.
4. For each:
   - Use `dsv-wrapper` to enumerate the candidate mp4 tracks + VTT.
   - Visually classify which track is the slide track.
   - Detect picture-in-picture / composite crop region on the chosen
     track. Persist as `crop_bbox`.
   - ffmpeg sample frames at the configured rate, applying the crop;
     drop blank/luma-static frames.
   - tar.zst the bundle (`manifest.json` + `frames/0001.png ...`).
   - Multipart POST the bundle + VTT + metadata to
     `POST /api/service/documents/{id}/video-bundle`.
5. When classification rejects every track (cam-only lecture, no slide
   capture exists): POST `slide_track_missing=true` to `video-bundle`
   without a bundle file, then push the transcript via the existing
   `POST /api/service/documents/{id}/transcript` so the doc still
   becomes searchable, just without the slide path.

The mp4 itself is **discarded after frame extraction** - it's not
archived on Minerva. The play.dsv URL is the recoverable source if a
re-extraction at a different fps is ever needed.

## Step 0 (do this first): inspect dsv-wrapper output for real DSV data

Almost everything else in this brief is conditional on what `dsv-wrapper`
actually returns and what shape the play.dsv mp4 URLs are in. Before
writing any classifier code:

1. Sit down with `dsv-wrapper` and a couple of real DSV course
   designations (any non-archived ones the user has access to).
2. For 5-10 presentations spanning different courses and recording
   years, dump:
   - The full track list - how many tracks per presentation?
   - Track URLs - https vs signed-url? Do they accept `Range:` headers
     for a 2 MB byte-range probe? Does ffmpeg's input-side `-ss` seek
     work, or does it need full download first?
   - Where the `moov` atom is placed (start vs end). HLS-only?
   - VTT availability - immediate, hours-late, or per-presentation
     unpredictable? Existing transcript pipeline handles
     `PresentationNotReadyError` - does the same mechanism apply here?
3. Write up findings as a short notebook or markdown report in
   `docs/plans/` so the rest of the work has a verified basis.

If `dsv-wrapper` doesn't expose a "list all media tracks" method, that's
the first blocker. Existing methods used elsewhere:
`get_courses_by_tag(tag)`, `get_presentations(designation)`,
`get_transcript_text(uuid)`. The wrapper lives at
`gitea.dsv.su.se/edsu8469/dsv-wrapper` (mirrored from the user's
private repo). If you need to extend it, do that as a separate PR
on that repo and pin via `requirements.txt` here.

**Do not start step 1 until step 0 lands.** If your assumptions about
what dsv-wrapper returns are wrong, every subsequent design choice is
wrong.

## Step 1: hand-label a corpus

The classifier has to be visual (no labels in play.dsv tracks per the
user). The only way to know it works is a labeled holdout set.

- 30-50 lectures across courses, recording years, formats. The user
  needs to point you at which courses; if they're hesitant, push back -
  this is the gating step before the rest of the pipeline gets built.
- For each lecture record: which track index is slides; is it cam-only
  (no slide track exists); is the slide track composite/PIP; is it a
  chalkboard / document-camera lecture.
- Persist labels in a CSV in `docs/plans/play-classifier-corpus.csv` so
  the eval harness can rebuild metrics on demand.

## Step 2: offline classifier evaluation harness

A standalone Python script - **not** the production `fetch_play_videos.py`
yet - that takes the labeled corpus and runs the classifier locally.
Output: confusion matrix, per-feature contribution, false-positive /
false-negative listings.

Iterate on thresholds and feature weights until accuracy is >95% on the
holdout. **This is the gate before the workflow ships.** The whole
pipeline silently produces garbage if classification is wrong, so an
eval that you can re-run on demand is worth more than any clever model
choice.

## Step 3: classifier (the hard part)

Per-frame features (CPU, opencv on GH runner):

- `edge_density`: Canny mean. Slides have high edge counts from text
  borders, bullet rules, code blocks; cams have medium (face contours)
  and chalkboards have low.
- `horiz_lines`: Hough transform horizontal-line count, normalized to
  frame height. Slides have many (bullets, underlines, code lines).
- `text_regions`: MSER or EAST detector blob count. Slides have many,
  cams have few.
- `face_count`: YuNet ONNX face detector. ~30 ms CPU, runs locally.
  Cams have 1, slides usually 0.
- `color_std`: HSV saturation channel std-dev. Slides have flat
  backgrounds, cams have skin tones.
- `temporal_std`: per-pixel std-dev across 10 sampled frames per track.
  Slides change every 30-180s, cams move constantly.

Aggregate score: z-score each feature within the candidate set (so
scores compare across this presentation's tracks, not across lectures),
weighted sum:

```
slide_score =
    + 1.0 * z(edge_density)
    + 0.5 * z(horiz_lines)
    + 1.0 * z(text_regions)
    - 1.0 * z(color_std)
    - 2.0 * z(face_count)
    - 0.5 * z(temporal_std)
```

Decision:

- If `best.score > HIGH` AND `best - runner_up > MARGIN` -> use best.
- Else: CPU OCR tiebreak with rapidocr-onnxruntime / paddleocr ONNX on
  5 sample frames per candidate. Pick highest char count. If all char
  counts < `MIN_CHARS`, mark `slide_track_missing = true` and bounce
  to the transcript-only path.

CPU OCR tiebreak runs on the GH worker. **Do not** call RunPod for
classification - that was on an earlier draft of the plan and got
dropped because paying GPU warmup just to count characters is silly.

PIP / composite detection (always runs on the chosen track):

```python
# 60s of frames sampled at 2 fps = 120 frames
motion = np.stack(frames).std(axis=0).mean(axis=-1)   # H,W
static_mask = motion < motion_threshold
bbox = largest_rect_in_mask(static_mask)
if bbox.area > 0.30 * frame.area and aspect_in([4/3, 16/9, 16/10]):
    crop_bbox = bbox       # PIP detected, crop to slide region
elif bbox.area > 0.80 * frame.area:
    crop_bbox = None       # whole frame is the slide
else:
    crop_bbox = None       # static region too small (logo/UI chrome)
```

## Step 4: frame extraction + bundling

Once track and crop are decided:

```bash
ffmpeg -i <track_url> -vf "fps=1/5,crop=W:H:X:Y" frames/f_%05d.png
```

- Use **output-side seek** ordering (`-i ... -vf ...`) for accurate
  frame boundaries. Input-side `-ss` is fine for probing but not for
  the real extraction.
- Sample rate: read from the `MINERVA_VIDEO_SAMPLE_FPS` env var
  (default `1/5`); the backend exposes the same default per request.
- Pre-filter on the runner: drop frames whose luma std-dev is below
  threshold (black/blank), and frames with uniform mean intensity (test
  patterns, fade transitions). Cuts ~30-50% of frames before bundling.

Bundle layout:

```
<bundle>.tar.zst
├── manifest.json    # {doc_id, sample_fps, crop_bbox, frame_count,
                     #  source_track_url, source_track_sha256}
├── frames/0001.png
├── frames/0002.png
└── ...
```

Multipart upload:

```python
files = {
    "metadata": ("metadata.json", json.dumps({
        "selected_track_index": idx,
        "slide_track_score": score,
        "crop_bbox": crop_bbox,        # {x,y,w,h} pixel coords or null
        "sample_fps": "1/5",
        "slide_track_missing": False,
    }), "application/json"),
    "bundle": ("bundle.tar.zst", open(bundle_path, "rb"), "application/zstd"),
    "vtt": ("transcript.vtt", vtt_text, "text/vtt"),
}
requests.post(
    f"{MINERVA_API_BASE}/api/service/documents/{doc_id}/video-bundle",
    headers={"Authorization": f"Bearer {MINERVA_SERVICE_API_KEY}"},
    files=files,
)
```

Bundle size cap on the backend is 500 MB. At fps=1/5 with the pre-filter
an hour-long lecture is typically 50-150 MB compressed; well under.

## Step 5: workflow file

`.github/workflows/play-ingest.yml`. Mirror `transcripts.yml` for shape;
run hourly with `workflow_dispatch` for manual triggers; concurrency
group `play-ingest` so a slow run doesn't double-bill. 60-minute
timeout.

Required secrets (already configured, see the project root
instructions file): `MINERVA_SERVICE_API_KEY`, `SU_USERNAME`,
`SU_PASSWORD`.

The workflow installs `dsv-wrapper`, `opencv-python-headless`,
`numpy`, `requests`, `zstandard`, `rapidocr-onnxruntime` (or
`paddleocr` if benchmarks favor it). ffmpeg ships with the runner.

## Step 6: cutover from `transcripts.yml`

Existing `transcripts.yml` only handles `awaiting_transcript`; the new
flag (`MINERVA_OCR_PIPELINE_ENABLED=true`) routes play URLs to
`awaiting_video_index` instead. Keep `transcripts.yml` running in
parallel - it's the slide_track_missing fallback path.

When a track classifies as missing, `fetch_play_videos.py` should:

1. POST `slide_track_missing=true` to `/api/service/documents/{id}/video-bundle`
   with no bundle file (the backend records the metadata and tells
   the caller to use the transcript path).
2. Fetch the VTT and POST it to `/api/service/documents/{id}/transcript`
   exactly like `fetch_transcripts.py` does today.

## Step 7 (optional, defer): teacher-facing track correction button

When ground truth from the live deployment accumulates (the
`slide_track_user_corrected` column on `documents`), the user will
ask for an admin UI button. Don't build it now - data first, UI later.

## Pointers / existing code to read

| File | Why |
| --- | --- |
| `scripts/fetch_transcripts.py` | Model for dsv-wrapper auth, error handling, multipart upload patterns. |
| `.github/workflows/transcripts.yml` | Model for the workflow shape. |
| `backend/crates/minerva-server/src/routes/service.rs` | `GET /pending-video-index`, `POST /video-bundle`, `POST /figure-uploads/{id}` reference implementations. |
| `backend/crates/minerva-server/src/ocr_worker.rs` | What the backend does with the bundle once you upload it. |
| `runpod-worker/handler.py` | What RunPod does - confirms the bundle layout the handler expects. |
| `docs/plans/ocr-video-pipeline.md` | Full pipeline plan; this brief is the GH-side excerpt. |

## Notes on dsv-wrapper 0.2.0

- mp4 access is via `get_media_tracks(uuid) -> [TrackInfo]` then
  `download_track(uuid, track_index, dest)` or `stream_track(uuid,
  track_index, start_byte=, end_byte=)`. No URLs ever cross the
  library boundary - auth (a JWT) is wrapped internally.
- `stream_track` honours `Range:` (verified end-to-end), so the
  moov-atom probe step in classification works as designed: ~2 MB per
  candidate instead of full downloads.
- `TrackInfo.duration_seconds` and `width` are `None`; the API
  doesn't publish them. Get duration from ffprobe of a downloaded
  track when you need it.
- The play CDN misreports `Content-Type: text/html` on mp4 responses.
  Trust `TrackInfo.mime_type` (derived from the URL extension by the
  wrapper) or ffmpeg's content sniffing, never the response header.

## Constraints to respect

- GitHub free runner: 14 GB disk, 7 GB RAM. Multi-GB lecture downloads
  are fine transiently; clean up between iterations.
- Python 3.13 (matches `transcripts.yml`).
- No emdashes; no space-dash-dash-space (CI greps for both). See
  `.github/workflows/ci.yml` for the exact patterns.
- Auth header is `Authorization: Bearer <key>`, not `X-Service-Key`.
- The bundle multipart endpoint has a 500 MB body limit. Fail loudly
  if you exceed it; don't silently truncate.

## Out of scope for this agent's work

- Frontend changes (track-correction UI, figure thumbnails). Different
  agent, comes after the data is flowing.
- Apache `LimitRequestBody` bump for the bundle endpoint - lives with
  the deploy PR, not this one.
- Cross-doc figure dedup, visual figure embeddings - both deferred
  to v2 per the main plan.
- Backend changes - everything you need exists on
  `feat/ocr-pipeline-foundation`. Don't touch Rust.

## Done definition

1. Step 0 findings document committed.
2. Hand-labeled corpus CSV committed.
3. Offline classifier eval harness committed and passing >95% accuracy.
4. `scripts/fetch_play_videos.py` committed; runs locally against a
   small designation without errors.
5. `.github/workflows/play-ingest.yml` committed and runs successfully
   on `workflow_dispatch` against a staging Minerva instance (or the
   user manually validates against prod with the OCR flag still off).
6. `transcripts.yml` continues to work for the slide_track_missing
   fallback path.

## Questions to ask up front, not after writing code

1. Can you point me at 2-3 designation codes I can use for hand-labeling
   that span different recording formats?
2. Should the workflow target every course's designations on every
   tick, or batch-by-course with a stagger? (Existing
   `transcripts.yml` does the all-at-once approach; check whether
   that survives at the new bundle scale.)
3. Is there a staging Minerva I can hit with the new endpoints, or do
   I validate end-to-end on prod with the OCR flag off (so docs land
   in `awaiting_video_index` but no GPU spend happens)?
4. What's the budget tolerance for an hour of failed extraction during
   classifier tuning? (Per-tick worst case is ~10 lectures × ~3 GB =
   30 GB transient bandwidth, no backend cost; cheap on GHA.)
