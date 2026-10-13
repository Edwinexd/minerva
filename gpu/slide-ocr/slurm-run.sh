#!/bin/bash
# Local experiments: OCR one fetched lecture with the worker's code.
# Usage: sbatch slurm-run.sh runs/<uuid> [slide_ocr.py options]

#SBATCH --job-name=slide-ocr
#SBATCH --partition=gpu
#SBATCH --account=slurm-staff
#SBATCH --time=02:00:00
#SBATCH --output=logs/%x-%j.out

srun bash run-job.sh "$@"
