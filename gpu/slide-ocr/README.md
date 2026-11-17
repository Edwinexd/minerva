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
| `olympus/ssh-gate` | Validates and installs a deployment archive, installed outside the worker tree. |
| `slurm-worker.sh`, `run-worker.sh` | What the gate submits. |
| `ensure-venv.sh` | Builds `venv/worker-<requirements hash>/` on the first worker that needs it. |
| `fetch.py`, `submit.sh`, `slide_ocr.py`, `slurm-run.sh`, `run-job.sh`, `viewer.py`, `bench_vllm.py` | Local experiments on one lecture; see below. |

## Rollout order

Publish the `dsv-wrapper` change exposing `resolve_track_download` /
`TrackDownload` before enabling the updated Actions workflow (it installs
that repository from GitHub). Apply the additive Minerva migration and
backend first, then enable the URL handoff workflow. Install the SSH gate
before running the new deploy workflow.

## Olympus setup (`minerva` function account)

Workers run as the function account `minerva@olympus.dsv.su.se` (home
`/home/dsv/minerva`, Slurm account `slurm-staff`, 20 GB quota). One SSH key
reaches it: the scheduler has it in the `minerva-slurm` k8s secret and
`deploy-slide-ocr.yml` as `SLURM_SSH_PRIVATE_KEY`. The account's
`~/.ssh/authorized_keys` is owned by root and the key carries no forced
command, so it is a shell on the account; what limits the blast radius is
that the account is Minerva's alone. The scheduler only ever runs
`olympus/slide-ocr-gate`, whose `status` and `cancel` are limited to the
account's `minerva-ve-*` jobs.

One-time bootstrap:

1. Install `olympus/ssh-gate` at `~/.local/lib/minerva/ssh-gate` (mode
   700). It deliberately lives outside the deployable worker directory, so
   a deployment cannot replace it.
2. If the Slurm account or partition differs from `slurm-staff` / `gpu`,
   put `SLURM_ACCOUNT=...` / `SLURM_PARTITION=...` in
   `~/minerva-slide-ocr/olympus/site.env`. Deployments preserve this file.
3. Set Terraform's `slurm_ssh_target` to `minerva@olympus.dsv.su.se`,
   `slurm_ssh_private_key_path` to the key, and `slurm_known_hosts` to
   Olympus's verified host key. Apply Terraform: the scheduler gets
   `minerva-slurm`, and the workflow gets `SLURM_SSH_PRIVATE_KEY`. Run
   `deploy-slide-ocr.yml` once.

`ssh-gate deploy` accepts only `minerva-slide-ocr-deploy <40-character sha>`
(in `SSH_ORIGINAL_COMMAND`) and a bounded tar archive containing the explicit production file list.
It rejects links, unexpected paths and files; it cannot replace the trusted
SSH gate, personal dotfiles, worker state, or `site.env`. Every push to
`master` touching `gpu/slide-ocr/` deploys through it.

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
