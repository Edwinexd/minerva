#!/usr/bin/env bash
#
# Fetches the eureka-2 release artifact pinned in .eureka-version and
# extracts it into backend/vendor/eureka-2/ so the workspace can resolve
# the path dependency declared by backend/crates/minerva-eureka.
#
# Run by:
#   - Developers, once after cloning Minerva (and again whenever
#     .eureka-version changes).
#   - The CI workflows (ci.yml, docker.yml) before any cargo invocation
#     that touches the workspace.
#
# Authentication: requires a GitHub token with read access to
# Edwinexd/eureka-2 (a fine-grained PAT scoped to that repo with
# Contents: Read is sufficient). Looked up in this order:
#   1. $EUREKA_RELEASE_TOKEN (the canonical CI variable name)
#   2. $GH_TOKEN
#   3. $GITHUB_TOKEN
#   4. `gh auth token` if the gh CLI is authenticated locally
#
# Idempotent: if backend/vendor/eureka-2/ already contains the pinned
# version, this is a no-op.

set -euo pipefail

REPO="Edwinexd/eureka-2"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION_FILE="$REPO_ROOT/.eureka-version"
VENDOR_DIR="$REPO_ROOT/backend/vendor/eureka-2"
STAMP_FILE="$VENDOR_DIR/.eureka-version"

if [[ ! -f "$VERSION_FILE" ]]; then
  echo "fetch-eureka: $VERSION_FILE not found" >&2
  exit 1
fi

VERSION="$(tr -d '[:space:]' < "$VERSION_FILE")"
if [[ -z "$VERSION" ]]; then
  echo "fetch-eureka: .eureka-version is empty" >&2
  exit 1
fi

# Strip leading "v" for the .crate filename (cargo packages as N-V.V.V.crate).
SEMVER="${VERSION#v}"
CRATE_NAME="eureka-2-${SEMVER}.crate"
SHA_NAME="${CRATE_NAME}.sha256"

# Idempotency: if the stamp file matches, we're done.
if [[ -f "$STAMP_FILE" ]] && [[ "$(cat "$STAMP_FILE")" == "$VERSION" ]] \
   && [[ -f "$VENDOR_DIR/Cargo.toml" ]]; then
  echo "fetch-eureka: $VERSION already vendored at $VENDOR_DIR"
  exit 0
fi

# Resolve a GitHub token.
TOKEN="${EUREKA_RELEASE_TOKEN:-${GH_TOKEN:-${GITHUB_TOKEN:-}}}"
if [[ -z "$TOKEN" ]] && command -v gh >/dev/null 2>&1; then
  if TOKEN="$(gh auth token 2>/dev/null)" && [[ -n "$TOKEN" ]]; then
    :
  else
    TOKEN=""
  fi
fi
if [[ -z "$TOKEN" ]]; then
  echo "fetch-eureka: no token found. Set EUREKA_RELEASE_TOKEN, GH_TOKEN, or run 'gh auth login'." >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "fetch-eureka: downloading $REPO release $VERSION"

# Use gh CLI if available (handles private-release-asset auth cleanly).
if command -v gh >/dev/null 2>&1; then
  GH_TOKEN="$TOKEN" gh release download "$VERSION" \
    --repo "$REPO" \
    --pattern "$CRATE_NAME" \
    --pattern "$SHA_NAME" \
    --dir "$WORK"
else
  # Fallback: REST API. Resolve the asset id, then download via the
  # asset's API URL with Accept: application/octet-stream.
  api_base="https://api.github.com/repos/$REPO/releases/tags/$VERSION"
  release_json="$(curl -fsSL \
    -H "Authorization: Bearer $TOKEN" \
    -H "Accept: application/vnd.github+json" \
    "$api_base")"
  for name in "$CRATE_NAME" "$SHA_NAME"; do
    asset_id="$(echo "$release_json" \
      | python3 -c "import json,sys;d=json.load(sys.stdin);n=sys.argv[1];print(next(a['id'] for a in d['assets'] if a['name']==n))" \
        "$name")"
    curl -fsSL \
      -H "Authorization: Bearer $TOKEN" \
      -H "Accept: application/octet-stream" \
      -o "$WORK/$name" \
      "https://api.github.com/repos/$REPO/releases/assets/$asset_id"
  done
fi

# Verify checksum.
echo "fetch-eureka: verifying sha256"
( cd "$WORK" && sha256sum -c "$SHA_NAME" )

# Extract to a clean vendor dir.
echo "fetch-eureka: extracting to $VENDOR_DIR"
rm -rf "$VENDOR_DIR"
mkdir -p "$VENDOR_DIR"
tar -xzf "$WORK/$CRATE_NAME" -C "$VENDOR_DIR" --strip-components=1

# Stamp the vendored version so future runs are no-ops.
echo "$VERSION" > "$STAMP_FILE"

echo "fetch-eureka: done. eureka-2 $VERSION vendored at $VENDOR_DIR"
