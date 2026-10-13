#!/bin/bash
# Runs one lecture end to end from the operator's machine:
#   ./submit.sh <uuid>                      save source posters, pick one
#   ./submit.sh <uuid> <source> [options]   fetch, OCR on Olympus, pull results
# Play is fetched here with SU_USERNAME / SU_PASSWORD from the local
# environment; Olympus only receives the video and transcript.

set -euo pipefail
cd "$(dirname "$0")"

REMOTE_HOST=olympus
REMOTE_DIR=minerva-slide-ocr
uuid=$1
shift

source venv/bin/activate
if [ $# -eq 0 ]; then
    python3 fetch.py "$uuid"
    exit
fi
slide_source=$1
shift
python3 fetch.py "$uuid" --source "$slide_source"

ssh "$REMOTE_HOST" "mkdir -p $REMOTE_DIR/runs $REMOTE_DIR/logs"
rsync -a --exclude venv/ --exclude runs/ --exclude logs/ --exclude work/ --exclude grants/ --exclude __pycache__/ \
    ./ "$REMOTE_HOST:$REMOTE_DIR/"
rsync -a --exclude sources/ "runs/$uuid/" "$REMOTE_HOST:$REMOTE_DIR/runs/$uuid/"
status=0
ssh "$REMOTE_HOST" "cd $REMOTE_DIR && sbatch --parsable --wait slurm-run.sh runs/$uuid $*" || status=$?
rsync -a --exclude video.mp4 "$REMOTE_HOST:$REMOTE_DIR/runs/$uuid/" "runs/$uuid/"
ssh "$REMOTE_HOST" "tail -n 20 \$(ls -t $REMOTE_DIR/logs/slide-ocr-[0-9]*.out | head -1)"
# Olympus home quota only fits the model weights plus the lectures in flight,
# so nothing is kept there once the results are back. The OCR cache in
# slides/ came back with them and goes up again on a rerun.
ssh "$REMOTE_HOST" "rm -r $REMOTE_DIR/runs/$uuid"
exit $status
