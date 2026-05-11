#!/usr/bin/env bash
# Reject the commit / PR if any backend/migrations/*.sql file that exists
# at the baseline SHA (recorded in .migrations-immutable-baseline) has
# different content in the working tree (or staged index, in pre-commit
# mode). Net-new migration files are always allowed, since they don't
# exist at the baseline.
#
# Why: sqlx hashes each migration's bytes when it first applies it and
# stores that hash in `_sqlx_migrations`. If the file content changes
# after a deploy has already applied it, the next deploy refuses to
# start with "migration N was previously applied but has been modified".
# The only safe way to amend a migration is to write a new one.
#
# The baseline SHA captures the canonical applied-to-prod content of every
# migration. Updating the baseline SHA is a deliberate signal that the
# migration set is consistent with what's deployed; gaming the rule by
# bumping the baseline is detectable in code review (one-line diff in
# .migrations-immutable-baseline).
#
# Run modes:
#   - pre-commit: diffs the index against baseline.
#   - CI:         diffs HEAD against baseline.
set -euo pipefail

BASELINE_FILE=".migrations-immutable-baseline"
if [[ ! -f "$BASELINE_FILE" ]]; then
    echo "::error::$BASELINE_FILE missing; cannot enforce migration immutability." >&2
    exit 1
fi
baseline=$(tr -d '[:space:]' < "$BASELINE_FILE")
if [[ -z "$baseline" ]]; then
    echo "::error::$BASELINE_FILE is empty." >&2
    exit 1
fi

if ! git rev-parse --quiet --verify "$baseline^{commit}" >/dev/null 2>&1; then
    # Shallow clones may miss the baseline. Try to fetch it.
    git fetch --quiet origin "$baseline" 2>/dev/null || true
    if ! git rev-parse --quiet --verify "$baseline^{commit}" >/dev/null 2>&1; then
        echo "::error::baseline SHA $baseline not reachable in this repo (try unshallow checkout)." >&2
        exit 1
    fi
fi

# The "current" tree to compare. In pre-commit, that's the staged index;
# elsewhere (CI, manual run), that's HEAD.
in_pre_commit=0
if [[ "${PRE_COMMIT:-0}" == "1" || -n "${PRE_COMMIT_HOME:-}" ]]; then
    in_pre_commit=1
fi

violations=()
# Enumerate migration files at the baseline (only existing ones can change).
# Plain `while read` loop instead of `mapfile` for macOS bash 3.2 compatibility.
baseline_files=()
while IFS= read -r line; do
    baseline_files+=("$line")
done < <(git ls-tree -r --name-only "$baseline" "--" backend/migrations \
    | grep -E '\.sql$')

for f in "${baseline_files[@]}"; do
    baseline_blob=$(git rev-parse "$baseline:$f" 2>/dev/null) || continue
    if (( in_pre_commit )); then
        # Compare staged blob (or worktree fallback) against baseline.
        current_blob=$(git ls-files --stage "--" "$f" 2>/dev/null | awk '{print $2}')
        if [[ -z "$current_blob" ]]; then
            # File deleted from index; that's also a violation.
            violations+=("D $f")
            continue
        fi
    else
        # CI / non-pre-commit: compare HEAD blob.
        if ! current_blob=$(git rev-parse "HEAD:$f" 2>/dev/null); then
            violations+=("D $f")
            continue
        fi
    fi
    if [[ "$baseline_blob" != "$current_blob" ]]; then
        violations+=("M $f")
    fi
done

if (( ${#violations[@]} > 0 )); then
    cat >&2 <<EOF
::error::backend/migrations/*.sql files are immutable once committed.

  sqlx checksums every migration into _sqlx_migrations on first apply; any
  byte-level change after that breaks every subsequent deploy with
  "migration N was previously applied but has been modified".

  Add a NEW migration (later timestamp) instead of editing one that's
  already on .migrations-immutable-baseline ($baseline).

Offending migrations (compared against baseline $baseline):
EOF
    for v in "${violations[@]}"; do
        printf '  %s\n' "$v" >&2
    done
    exit 1
fi
