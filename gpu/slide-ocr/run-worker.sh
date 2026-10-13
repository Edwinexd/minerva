#!/bin/bash

set -euo pipefail
cd "$(dirname "$0")"

# The venv is built under .tmp and renamed, and Olympus mounts home under
# different paths on login/GPU nodes. Activation scripts retain the build
# path; invoke the interpreter from the current mount directly.
worker_venv=$(bash ensure-venv.sh)
export PYTHONUNBUFFERED=1
"$worker_venv/bin/python" worker.py "$1"
