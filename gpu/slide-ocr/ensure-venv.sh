#!/bin/bash
# Prints the path of the worker venv for the current requirements, building
# it first if this is the first job to need it. The venv is named by a hash
# of the requirements, so a deploy that changes them gets a fresh one on its
# first worker while jobs still running keep theirs. Run on a GPU node: vLLM
# pins its own torch, so the venv is isolated from the cluster's.

set -euo pipefail
cd "$(dirname "$0")"

hash=$(cat requirements-worker.txt requirements-common.txt | sha256sum | cut -c1-12)
venv="venv/worker-$hash"
mkdir -p venv
(
    # Concurrent workers wait for the one building instead of racing it.
    flock 9
    if [ ! -x "$venv/bin/python" ]; then
        # Two venvs do not fit the account's quota beside the model weights,
        # so drop older ones first when no other job of ours can be using
        # them. Otherwise build anyway; if that fails on quota the item is
        # retried once the older job has gone.
        # awk, not grep -v: grep exits 1 on no lines, which pipefail turns
        # into a silent exit whenever this is the only running job.
        others=$(squeue -h -u "$USER" -t RUNNING -o %i | awk -v me="${SLURM_JOB_ID:-none}" '$0 != me' | wc -l)
        if [ "$others" -eq 0 ]; then
            find venv -maxdepth 1 -name 'worker-*' ! -name "worker-$hash" -exec rm -r {} +
        fi
        rm -r "$venv.tmp" venv/.pip-tmp 2>/dev/null || true
        # pip stages every wheel in TMPDIR before installing, several GB of
        # CUDA libraries; the node's /tmp is 4.9 GB and shared.
        mkdir venv/.pip-tmp
        export TMPDIR="$PWD/venv/.pip-tmp"
        python3 -m venv "$venv.tmp"
        "$venv.tmp/bin/pip" install --upgrade pip >&2
        "$venv.tmp/bin/pip" install --no-cache-dir -r requirements-worker.txt >&2
        rm -r venv/.pip-tmp
        mv "$venv.tmp" "$venv"
    fi
) 9> venv/.lock >&2
echo "$PWD/$venv"
