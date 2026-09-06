#!/usr/bin/env bash
# Real-browser accessibility pass: the same check CI's `frontend-a11y`
# job runs, in one command, for when you have touched layout, a scroll
# container or a colour and don't want to find out from CI.
#
# It is deliberately not a git hook. The vitest + axe layer in pre-commit
# runs in jsdom, which reports every element as zero-sized, so rules that
# depend on layout (axe's `scrollable-region-focusable`, colour contrast)
# cannot fire there; catching those needs a frontend build, a server
# build and a couple of minutes of Chromium. That is CI's job, not
# something to put in front of every push.
#
# Self-contained and non-destructive to your own data: it builds the SPA,
# builds the server binary, and starts it on a scratch database and a
# free port. The a11y seed wipes whatever database it is pointed at, so
# it must never be pointed at a database you care about.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

PG_URL_BASE="${MINERVA_A11Y_PG:-postgres://minerva:minerva@localhost:5432}"
DB_NAME="minerva_a11y"
LOG="$(mktemp -t minerva-a11y-backend.XXXXXX.log)"
BACKEND_PID=""

psql_root() { psql "${PG_URL_BASE}/postgres" -v ON_ERROR_STOP=1 "$@"; }

cleanup() {
  local status=$?
  if [[ -n "$BACKEND_PID" ]] && kill -0 "$BACKEND_PID" 2>/dev/null; then
    kill "$BACKEND_PID" 2>/dev/null || true
    wait "$BACKEND_PID" 2>/dev/null || true
  fi
  # A pa11y run against a 500 page still "passes" its own rules, so the
  # backend log is where a broken seed or migration actually shows up.
  if [[ $status -ne 0 ]]; then
    echo "--- backend log (tail) ---" >&2
    tail -40 "$LOG" >&2 || true
  fi
  psql_root -c "DROP DATABASE IF EXISTS ${DB_NAME}" >/dev/null 2>&1 || true
  rm -f "$LOG"
  exit $status
}
trap cleanup EXIT

if ! pg_isready -d "${PG_URL_BASE}/postgres" >/dev/null 2>&1; then
  echo "pa11y: postgres not reachable at ${PG_URL_BASE}; starting the compose service" >&2
  docker compose -f docker-compose.yml up -d postgres >/dev/null
  for _ in $(seq 1 60); do
    pg_isready -d "${PG_URL_BASE}/postgres" >/dev/null 2>&1 && break
    sleep 1
  done
fi
pg_isready -d "${PG_URL_BASE}/postgres" >/dev/null 2>&1 || {
  echo "pa11y: postgres still not reachable at ${PG_URL_BASE}" >&2
  exit 1
}

psql_root -c "DROP DATABASE IF EXISTS ${DB_NAME}" >/dev/null
psql_root -c "CREATE DATABASE ${DB_NAME} OWNER minerva" >/dev/null

echo "pa11y: building the SPA and the server binary"
npm --prefix frontend run build >/dev/null
# Plain build, exactly like CI: nothing in an a11y pass embeds or reranks,
# so the model engine feature is not needed and the gRPC channels below
# never have to resolve.
SQLX_OFFLINE=true cargo build --manifest-path backend/Cargo.toml --bin minerva >/dev/null

# A free port, because the developer's own dev server usually holds 3000.
PORT="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"

echo "pa11y: starting the backend on 127.0.0.1:${PORT}"
DATABASE_URL="${PG_URL_BASE}/${DB_NAME}" \
MINERVA_EMBEDDER_URL=http://127.0.0.1:59051 \
MINERVA_RERANKER_URL=http://127.0.0.1:59052 \
QDRANT_URL=http://127.0.0.1:56334 \
MINERVA_RUN_WORKER=false \
MINERVA_DEV_MODE=true \
MINERVA_ADMINS=student \
MINERVA_HMAC_SECRET=pa11y-local-secret \
CEREBRAS_API_KEY=pa11y-local-not-a-real-key \
MINERVA_DOCS_PATH="$(mktemp -d -t minerva-a11y-docs.XXXXXX)" \
MINERVA_STATIC_DIR="$PWD/frontend/dist" \
MINERVA_HOST=127.0.0.1 \
MINERVA_PORT="$PORT" \
MINERVA_METRICS_PORT=0 \
RUST_LOG="${RUST_LOG:-warn,minerva_server=info,minerva_app_core=info}" \
  ./backend/target/debug/minerva >"$LOG" 2>&1 &
BACKEND_PID=$!

for _ in $(seq 1 120); do
  curl -sf "http://127.0.0.1:${PORT}/api/health" >/dev/null 2>&1 && break
  kill -0 "$BACKEND_PID" 2>/dev/null || { echo "pa11y: backend exited during startup" >&2; exit 1; }
  sleep 1
done
curl -sf "http://127.0.0.1:${PORT}/api/health" >/dev/null || {
  echo "pa11y: backend never became healthy" >&2
  exit 1
}

# `MINERVA_ADMINS=student` matches CI: the header's dev user-switcher
# persists its first option to localStorage and `api.ts` then sends it as
# X-Dev-User, overriding the header pa11y sets. Granting admin to that
# same identity makes both paths resolve to one admin, so every page
# renders its full authenticated state deterministically.
MINERVA_BASE_URL="http://127.0.0.1:${PORT}" \
MINERVA_A11Y_ADMIN=student@su.se \
MINERVA_HMAC_SECRET=pa11y-local-secret \
  npm --prefix frontend run pa11y
