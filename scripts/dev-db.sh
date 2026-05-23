#!/usr/bin/env bash
# Per-branch postgres database for Minerva dev.
#
# Every git branch gets its own database carved off `minerva` via
# `CREATE DATABASE foo TEMPLATE minerva` (instant page-level copy).
# This isolates schema changes: a migration on branch A can't corrupt
# branch B's state, so switching branches no longer leaves a half-
# migrated DB behind (the failure mode that triggered this script).
#
# `master` (or `main`) keeps using the existing `minerva` DB, so the
# default workflow is unchanged when you don't branch.
#
# Subcommands:
#   url      Print DATABASE_URL for the current branch. Pure string
#            computation, no DB contact, always succeeds.
#   ensure   Create the branch DB from `minerva` template if missing.
#            Briefly disconnects clients of `minerva` so postgres lets
#            us TEMPLATE-copy it; sqlx pools auto-reconnect.
#   use      `ensure` then print `export DATABASE_URL=...`. Designed
#            for `eval "$(scripts/dev-db.sh use)"` or sourcing from
#            .envrc.
#   list     List all minerva* databases.
#   refresh  Drop + recreate the current branch DB from `minerva`.
#            Useful after pulling master changes you want propagated
#            into a long-running branch DB.
#   prune    Drop branch DBs whose local git branch no longer exists.
#
# Talks to postgres through `docker compose exec` so we don't depend
# on a host psql client. If the postgres container is down `ensure`
# and `refresh` will fail loudly; `url` keeps working.

set -euo pipefail

POSTGRES_USER="${POSTGRES_USER:-minerva}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-minerva}"
POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
MASTER_DB="${MASTER_DB:-minerva}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

current_branch() {
    git -C "$REPO_ROOT" symbolic-ref --short -q HEAD || echo "detached"
}

# Postgres identifier rules: <=63 chars, [a-z_][a-z0-9_]*. Lowercase,
# non-alnum -> _, collapse runs, trim edges, truncate. Branches that
# collide post-sanitization (rare; would need e.g. `feat/x` and
# `feat-x`) share a DB; rename to disambiguate.
slugify() {
    local raw="$1"
    local slug
    slug=$(printf '%s' "$raw" \
        | tr '[:upper:]' '[:lower:]' \
        | tr -c 'a-z0-9' '_' \
        | tr -s '_' \
        | sed -e 's/^_//' -e 's/_$//')
    # 8 chars for "minerva_" prefix leaves 55 for the slug within
    # postgres' 63-char identifier limit.
    printf '%s' "${slug:0:55}"
}

db_name_for_branch() {
    local branch="$1"
    case "$branch" in
        master|main) printf '%s' "$MASTER_DB" ;;
        *) printf '%s_%s' "$MASTER_DB" "$(slugify "$branch")" ;;
    esac
}

# Wrap psql so command callers don't repeat connection args. Talks to
# the `postgres` database (not $MASTER_DB) so the same connection can
# CREATE/DROP arbitrary DBs.
pg() {
    docker compose -f "$REPO_ROOT/docker-compose.yml" exec -T postgres \
        psql -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d postgres "$@"
}

db_exists() {
    local db="$1"
    local out
    out=$(pg -tAc "SELECT 1 FROM pg_database WHERE datname='${db}'" 2>/dev/null || true)
    [ "$out" = "1" ]
}

clone_master() {
    local db="$1"
    # postgres refuses TEMPLATE-copy while the source has live
    # connections; kick everyone off briefly. sqlx connection pools
    # auto-reconnect within a request or two.
    pg <<SQL >/dev/null
SELECT pg_terminate_backend(pid) FROM pg_stat_activity
 WHERE datname = '${MASTER_DB}' AND pid <> pg_backend_pid();
CREATE DATABASE "${db}" TEMPLATE "${MASTER_DB}" OWNER "${POSTGRES_USER}";
SQL
}

ensure_db() {
    local branch db
    branch=$(current_branch)
    db=$(db_name_for_branch "$branch")
    if [ "$db" = "$MASTER_DB" ]; then
        return 0
    fi
    if db_exists "$db"; then
        return 0
    fi
    if ! db_exists "$MASTER_DB"; then
        echo "dev-db: master DB '$MASTER_DB' missing; can't TEMPLATE-copy" >&2
        return 1
    fi
    echo "dev-db: creating '$db' from '$MASTER_DB' (branch: $branch)" >&2
    clone_master "$db"
}

url_for_branch() {
    local branch db
    branch=$(current_branch)
    db=$(db_name_for_branch "$branch")
    printf 'postgres://%s:%s@%s:%s/%s\n' \
        "$POSTGRES_USER" "$POSTGRES_PASSWORD" \
        "$POSTGRES_HOST" "$POSTGRES_PORT" "$db"
}

cmd_url() { url_for_branch; }
cmd_ensure() { ensure_db; }
cmd_use() {
    ensure_db
    echo "export DATABASE_URL=$(url_for_branch)"
}

cmd_list() {
    pg -tAc "SELECT datname FROM pg_database
              WHERE datname = '${MASTER_DB}' OR datname LIKE '${MASTER_DB}\\_%' ESCAPE '\\'
              ORDER BY datname"
}

cmd_refresh() {
    local branch db
    branch=$(current_branch)
    db=$(db_name_for_branch "$branch")
    if [ "$db" = "$MASTER_DB" ]; then
        echo "dev-db: refusing to refresh '$MASTER_DB'; recreate via docker compose down -v" >&2
        return 1
    fi
    echo "dev-db: refreshing '$db' from '$MASTER_DB'" >&2
    pg <<SQL >/dev/null
SELECT pg_terminate_backend(pid) FROM pg_stat_activity
 WHERE datname IN ('${db}', '${MASTER_DB}') AND pid <> pg_backend_pid();
DROP DATABASE IF EXISTS "${db}";
SQL
    clone_master "$db"
}

cmd_prune() {
    local live_branches live_dbs existing victim
    live_branches=$(git -C "$REPO_ROOT" for-each-ref --format='%(refname:short)' refs/heads/)
    live_dbs=""
    while IFS= read -r br; do
        [ -z "$br" ] && continue
        live_dbs+="$(db_name_for_branch "$br")"$'\n'
    done <<<"$live_branches"
    existing=$(pg -tAc "SELECT datname FROM pg_database
                         WHERE datname LIKE '${MASTER_DB}\\_%' ESCAPE '\\'
                         ORDER BY datname")
    while IFS= read -r victim; do
        [ -z "$victim" ] && continue
        if ! grep -qxF "$victim" <<<"$live_dbs"; then
            echo "dev-db: dropping orphaned '$victim'" >&2
            pg <<SQL >/dev/null
SELECT pg_terminate_backend(pid) FROM pg_stat_activity
 WHERE datname = '${victim}' AND pid <> pg_backend_pid();
DROP DATABASE IF EXISTS "${victim}";
SQL
        fi
    done <<<"$existing"
}

usage() {
    cat >&2 <<EOF
usage: dev-db.sh {url|ensure|use|list|refresh|prune}

  url       Print DATABASE_URL for the current branch
  ensure    Create branch DB from master template if missing
  use       ensure + print 'export DATABASE_URL=...' (eval me)
  list      List all minerva* databases
  refresh   Drop + recreate current branch DB from master
  prune     Drop branch DBs whose local git branch no longer exists
EOF
}

main() {
    local cmd="${1:-use}"
    case "$cmd" in
        url) cmd_url ;;
        ensure) cmd_ensure ;;
        use) cmd_use ;;
        list) cmd_list ;;
        refresh) cmd_refresh ;;
        prune) cmd_prune ;;
        -h|--help|help) usage ;;
        *) usage; exit 2 ;;
    esac
}

main "$@"
