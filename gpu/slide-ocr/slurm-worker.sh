#!/bin/bash
# One visual extraction worker, submitted by olympus/slide-ocr-gate with the
# path of its grant file. Account and partition come from the gate (see
# olympus/site.env); the wall time matches WORKER_WALL_SECS in
# minerva-app-core's visual_extraction.rs.
# Usage: sbatch slurm-worker.sh grants/<worker-uuid>

#SBATCH --time=12:00:00
#SBATCH --output=logs/worker-%j.out

export SLIDE_OCR_WALL_SECONDS=43200
srun bash run-worker.sh "$1"
