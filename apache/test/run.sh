#!/usr/bin/env bash
# Run all Apache/Lua tests. Invoked locally and from CI.
set -euo pipefail
cd "$(dirname "$0")"

LUA=${LUA:-lua}
"$LUA" -v >/dev/null

echo "=== syntax check ==="
"$LUA" -e 'dofile(arg[1])' ../minerva-ext-auth.lua

echo
echo "=== HMAC / SHA-256 vectors ==="
"$LUA" test_hmac.lua

echo
echo "=== Token verify ==="
"$LUA" test_token.lua

echo
echo "All Apache/Lua tests passed."
