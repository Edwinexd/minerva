#!/usr/bin/env bash
# Trigger the dev-mode seed feature against the running dev server.
#
# Same code path as the "Reseed" button in /admin/dev-tools - both
# end up at POST /admin/dev/seed, which gates on MINERVA_DEV_MODE and
# admin role before wiping and re-creating the fixture set. The
# script is the "fresh clone" path; the button is the "already-
# logged-in mid-session, want to reset" path.
#
# Prerequisites:
#   * `docker compose up -d` (backend listening on :3000 with
#     MINERVA_DEV_MODE=true)
#   * the operator is in MINERVA_ADMINS so the dev-auth fallback
#     resolves them as admin
#
# Usage (from anywhere):
#
#     scripts/seed-dev.sh                   # uses first MINERVA_ADMINS entry
#     scripts/seed-dev.sh --as edsu8469     # impersonate a specific admin
#
# The endpoint returns JSON; we feed it through `jq` (if present) for
# a friendlier read. If `jq` isn't installed the raw JSON is printed
# unchanged so the script still works on a stock box.

set -euo pipefail

API_URL="${MINERVA_API_URL:-http://localhost:3000/api}"

# Pick the admin to run as. CLI flag wins over MINERVA_ADMINS.
admin_eppn=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --as)
      admin_eppn="$2"
      shift 2
      ;;
    --as=*)
      admin_eppn="${1#--as=}"
      shift
      ;;
    -h|--help)
      sed -n '2,/^set -/p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$admin_eppn" ]]; then
  # Match how the backend's dev-auth fallback picks an eppn: first
  # MINERVA_ADMINS entry + @su.se. Keep this in sync with
  # backend/crates/minerva-server/src/auth.rs.
  first_admin="$(printf '%s\n' "${MINERVA_ADMINS:-}" | cut -d',' -f1 | tr -d '[:space:]')"
  if [[ -z "$first_admin" ]]; then
    echo "MINERVA_ADMINS is empty and no --as flag given. Pass --as <eppn> or set MINERVA_ADMINS." >&2
    exit 2
  fi
  admin_eppn="${first_admin}@su.se"
fi

echo "seed-dev: POST ${API_URL}/admin/dev/seed  (as ${admin_eppn})"
echo

# `--fail-with-body` makes curl exit non-zero on 4xx/5xx but still
# print the response body, so a friendly server-side error (e.g.
# "admin eppn unknown - log in once first") makes it to the user.
response="$(curl --silent --fail-with-body \
  -X POST \
  -H "X-Dev-User: ${admin_eppn}" \
  -H "Content-Type: application/json" \
  "${API_URL}/admin/dev/seed")" || {
  status=$?
  echo "$response" >&2
  exit "$status"
}

if command -v jq >/dev/null 2>&1; then
  echo "$response" | jq .
else
  echo "$response"
fi

echo
echo "Documents are queued for embedding via the local fastembed"
echo "provider; the background worker will mark them 'ready' over"
echo "the next ~10-30s (longer on the first run if the ONNX model"
echo "is still cold-loading)."
