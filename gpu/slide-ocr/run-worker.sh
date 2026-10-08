#!/bin/bash

set -euo pipefail
cd "$(dirname "$0")"

source "$(bash ensure-venv.sh)/bin/activate"
export PYTHONUNBUFFERED=1
python3 worker.py "$1"
