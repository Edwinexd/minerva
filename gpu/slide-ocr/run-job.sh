#!/bin/bash
# Usage: run-job.sh runs/<uuid> [slide_ocr.py options]

set -euo pipefail
cd "$(dirname "$0")"

source "$(bash ensure-venv.sh)/bin/activate"
export PYTHONUNBUFFERED=1
python3 slide_ocr.py "$@"
