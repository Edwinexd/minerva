# Visual extraction worker (Olympus)

The GPU half of the visual extraction pipeline (see "Visual extraction
pipeline" in `docs/ARCHITECTURE.md`): a Slurm job that pulls Play lectures
and PDFs from Minerva one at a time, finds the slides, OCRs them with
`baidu/Unlimited-OCR` on vLLM, crops figures, embeds them with CLIP, and
uploads the result. Minerva's scheduler submits and monitors these jobs over
SSH; the job itself holds nothing but one signed URL.

## Files

| File | Role |
|---|---|
| `worker.py` | The job: `next` -> download -> extract -> upload, until the queue is empty or the wall time is close. `--dry-run` writes one item's result to disk instead. |
| `extract.py` | Model-independent half: slide detection, PDF rendering, parsing grounded OCR output, page numbers, captions, same-slide merging, figure crops. |
| `ocr_vllm.py` | Unlimited-OCR on vLLM, with the two workarounds vLLM 0.31 needs (see its comments). |
| `olympus/slide-ocr-gate` | What the scheduler runs over SSH: `submit`, `status`, `cancel`. |
| `olympus/deploy.sh` | What the deploy workflow runs over SSH to put a new version in place. |
| `slurm-worker.sh`, `run-worker.sh` | What the gate submits. |
| `ensure-venv.sh` | Builds `venv/worker-<requirements hash>/` on the first worker that needs it. |
| `fetch.py`, `submit.sh`, `slide_ocr.py`, `slurm-run.sh`, `run-job.sh`, `viewer.py`, `bench_vllm.py` | Local experiments on one lecture; see below. |

## Olympus setup (service account)

An ordinary user, confined by Slurm (low priority). One SSH key, shared by
`minerva-scheduler` (submitting and watching workers) and the
`deploy-slide-ocr` GitHub workflow (shipping this directory). Once:

1. Put the public key in the account's `~/.ssh/authorized_keys`.
2. Only if the account's Slurm account or partition differ from
   `slurm-staff` / `gpu`: put them in `~/minerva-slide-ocr/olympus/site.env`
   (`SLURM_ACCOUNT=...`, `SLURM_PARTITION=...`).

Then set `slurm_ssh_target`, `slurm_ssh_private_key_path` and
`slurm_known_hosts` in Terraform and apply: that fills the scheduler's
`minerva-slurm` k8s secret and the `prod` environment secrets of the deploy
workflow. Run the workflow once (`gh workflow run deploy-slide-ocr.yml`);
after that every push to `master` touching `gpu/slide-ocr/` deploys itself.

Nothing else is installed by hand. The first worker builds its venv (about
5 minutes, 8.6 GB) and downloads the model weights (6.3 GB); a deploy that
changes the requirements gets a fresh venv on its next worker, which first
removes the old one when no other worker is running (two venvs and the
weights do not fit a 20 GB quota). Storage on Olympus: weights, one venv,
and one source file per running worker under `work/`, about 16 GB; each
submit prunes logs and grants older than 14 days.

## Measured (L40S, Unlimited-OCR, 34-minute lecture)

| | |
|---|---|
| OCR, vLLM, one engine | 0.57 s per slide frame (637 tokens/s); 63 frames in 36 s |
| OCR, HF `model.infer`, best of 1-6 processes | 5.0 s per frame (4 processes; 6 run out of VRAM) |
| Model load | about 100 s, once per worker |
| Slide detection (PyAV, CPU) | about 40 s per lecture |
| Lecture end to end (dry run) | 2 min 39 s including load; 23 slides, 14 figures after filtering |
| 11-page PDF end to end | 65 s including load |

vLLM and HF output agree (mean word overlap 0.95). Figures below 1% of the
frame are dropped: they are logos and slide decoration.

## Local experiments

With `SU_USERNAME` / `SU_PASSWORD` exported locally (never on Olympus):

```
python3 -m venv venv && venv/bin/pip install -r requirements-fetch.txt
./submit.sh <uuid>              # save each video source's poster to runs/<uuid>/sources/
./submit.sh <uuid> <source>     # fetch, run slide_ocr.py on Olympus, pull results back
python3 viewer.py runs/<uuid>   # runs/<uuid>/viewer.html: video, slide, boxes, figures, transcript
```

`slide_ocr.py` runs the worker's own extraction on a fetched lecture, so the
viewer shows what production would ingest. `bench_vllm.py` measures OCR
throughput over a directory of slide frames.

## Cluster notes

- Partition `gpu`: two nodes, L40S 46 GB, 16 cores, 64 GB RAM, whole-node
  allocation. `scrontab` is disabled, which is why Minerva schedules.
- No system ffmpeg; PyAV ships its own. Node `/tmp` is 4.9 GB, so work
  files live under the checkout.
- The venv brings its own torch (vLLM pins 2.13); the system torch is 2.10.
- `dsv-wrapper` needs Python 3.12 and the nodes run 3.11. It is only used by
  GitHub Actions and the local `fetch.py`, never on Olympus.
