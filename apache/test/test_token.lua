-- End-to-end token tests for verify_token. We build tokens in Lua using
-- the same helpers (so the test exercises the parsing/verify path; the
-- HMAC primitive itself is covered by test_hmac.lua against RFC vectors).
--
-- We also exercise interop with the Rust producer via a separately-
-- generated fixture (apache/test/fixture.txt) when present; the CI
-- workflow regenerates that fixture using the Rust mint helper before
-- running this test, so a Lua/Rust format drift fails CI.
--
-- Run: lua apache/test/test_token.lua

local script_dir = arg[0]:match("(.*/)") or "./"
local m = dofile(script_dir .. "../minerva-ext-auth.lua")

local SECRET = "smoketest_secret_value"

local failures = 0
local function check(name, ok, detail)
    if ok then
        print("ok   " .. name)
    else
        print("FAIL " .. name .. (detail and (": " .. detail) or ""))
        failures = failures + 1
    end
end

local function b64url_encode(s)
    local alpha = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    local out = {}
    for i = 1, #s, 3 do
        local b1 = s:byte(i)
        local b2 = s:byte(i + 1) or 0
        local b3 = s:byte(i + 2) or 0
        local n = (b1 << 16) | (b2 << 8) | b3
        out[#out + 1] = alpha:sub(((n >> 18) & 63) + 1, ((n >> 18) & 63) + 1)
        out[#out + 1] = alpha:sub(((n >> 12) & 63) + 1, ((n >> 12) & 63) + 1)
        if i + 1 <= #s then
            out[#out + 1] = alpha:sub(((n >> 6) & 63) + 1, ((n >> 6) & 63) + 1)
        end
        if i + 2 <= #s then
            out[#out + 1] = alpha:sub((n & 63) + 1, (n & 63) + 1)
        end
    end
    return (table.concat(out):gsub("+", "-"):gsub("/", "_"))
end

-- Mint a token in the format the Rust producer emits.
local function mint(secret, jti, eppn, display, exp)
    local eppn_b64 = b64url_encode(eppn)
    local display_b64 = display and b64url_encode(display) or ""
    local payload = jti .. ":" .. eppn_b64 .. ":" .. display_b64 .. ":" .. tostring(exp)
    local sig = m.tohex(m.hmac_sha256(secret, payload))
    local raw = payload .. ":" .. sig
    return b64url_encode(raw)
end

local FUTURE = os.time() + 3600
local PAST = os.time() - 3600

-- ---- Happy path ------------------------------------------------------------

do
    local jti = "11111111-2222-3333-4444-555555555555"
    local tok = mint(SECRET, jti, "ext:alice@example.com", "Alice", FUTURE)
    local claims, err = m.verify_token(SECRET, tok)
    check("valid token parses",
        claims ~= nil and claims.eppn == "ext:alice@example.com"
            and claims.jti == jti and claims.display_name == "Alice",
        err or (claims and (claims.eppn .. " / " .. claims.jti)))
end

do
    local tok = mint(SECRET, "abcd", "ext:bob@example.com", nil, FUTURE)
    local claims = m.verify_token(SECRET, tok)
    check("no-display token parses",
        claims ~= nil and claims.display_name == nil)
end

-- ---- Failure modes ---------------------------------------------------------

do
    local _, err = m.verify_token(SECRET, "")
    check("empty cookie -> malformed", err == "malformed", err)
end

do
    local _, err = m.verify_token(SECRET, "!!!not-base64!!!")
    -- Bytes outside the b64url alphabet get stripped, leaving an empty
    -- decode -> malformed (no fields to split).
    check("garbage cookie -> malformed", err == "malformed", err)
end

do
    -- Valid b64 but wrong shape (only 3 fields).
    local raw = b64url_encode("only:three:fields")
    local _, err = m.verify_token(SECRET, raw)
    check("wrong shape -> malformed", err == "malformed", err)
end

do
    -- Tamper the signature (flip the last byte to guarantee a change).
    local tok = mint(SECRET, "x", "ext:eve@example.com", nil, FUTURE)
    local raw = m.b64url_decode(tok)
    local last_byte = tonumber(raw:sub(-2), 16)
    raw = raw:sub(1, -3) .. string.format("%02x", last_byte ~ 0xff)
    local tampered = b64url_encode(raw)
    local _, err = m.verify_token(SECRET, tampered)
    check("tampered sig -> bad_signature", err == "bad_signature", err)
end

do
    -- Right format, signed with the wrong secret.
    local tok = mint("wrong_secret", "x", "ext:eve@example.com", nil, FUTURE)
    local _, err = m.verify_token(SECRET, tok)
    check("wrong secret -> bad_signature", err == "bad_signature", err)
end

do
    local tok = mint(SECRET, "x", "ext:expired@example.com", nil, PAST)
    local _, err = m.verify_token(SECRET, tok)
    check("expired token -> expired", err == "expired", err)
end

do
    -- no ext: prefix
    local tok = mint(SECRET, "x", "alice@su.se", nil, FUTURE)
    local _, err = m.verify_token(SECRET, tok)
    check("non-ext eppn -> bad_eppn", err == "bad_eppn", err)
end

-- ---- Interop with Rust producer (optional fixture) ------------------------

local fixture_path = script_dir .. "fixture.txt"
local f = io.open(fixture_path, "r")
if f then
    local lines = {}
    for line in f:lines() do lines[#lines + 1] = line end
    f:close()
    local fix_secret  = lines[1]
    local fix_token   = lines[2]
    local fix_eppn    = lines[3]
    local fix_jti     = lines[4]
    local fix_display = lines[5] ~= "" and lines[5] or nil
    local claims, err = m.verify_token(fix_secret, fix_token)
    check("rust-minted fixture verifies",
        claims ~= nil
            and claims.eppn == fix_eppn
            and claims.jti == fix_jti
            and claims.display_name == fix_display,
        err or (claims and (claims.eppn .. " jti=" .. claims.jti)))
else
    print("skip fixture: " .. fixture_path .. " not present (run gen_fixture)")
end

if failures > 0 then
    io.stderr:write(string.format("\n%d test(s) failed\n", failures))
    os.exit(1)
end
print("\nAll token tests passed.")
