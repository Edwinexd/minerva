-- Minerva external-auth Apache hook.
--
-- Validates the `minerva_ext` JWT cookie set by the backend's
-- /api/external-auth/callback endpoint. On success, injects `eppn`,
-- `displayName`, and `X-Minerva-Ext-Jti` request headers so the backend's
-- auth middleware sees the request as authenticated (same shape as
-- mod_shib's ShibUseHeaders, plus the JTI for DB revocation lookup).
--
-- Crypto runs entirely in Lua because Apache 2.4's mod_lua on Debian has
-- no HTTP client / subrequest API, no HMAC primitive, and no FFI. SHA-256
-- is implemented inline below using Lua 5.4 native bitops.
--
-- The HMAC secret lives in /etc/apache2/secrets/minerva-hmac and must
-- match `MINERVA_HMAC_SECRET` in the backend's k8s deployment. The file
-- is read once per process and cached (mod_lua reuses Lua VMs across
-- requests inside a worker).
--
-- Defense in depth: this script enforces signature + expiry. The backend
-- additionally checks the JTI against `external_auth_invites.revoked_at`,
-- so an admin can revoke individual invites without rotating the secret.
--
-- The pure-Lua helpers (sha256, hmac_sha256, b64url_decode, parse_token,
-- verify_token) are exposed via the returned `_M` table so the test
-- harness in apache/test/ can exercise them outside Apache. mod_lua
-- discards the return value of the loaded script; the real entry
-- point is `check_ext_auth`, defined as a global below.

-- `apache2` is a pre-injected global in mod_lua; outside Apache (test
-- harness) it's absent, so we tolerate either.
local apache2_mod = rawget(_G, "apache2") or { OK = 0, DECLINED = -1 }

local SECRET_PATH = "/etc/apache2/secrets/minerva-hmac"
local COOKIE_NAME = "minerva_ext"

-- ---------------------------------------------------------------------------
-- SHA-256 + HMAC-SHA256 (FIPS 180-4). Adapted for Lua 5.4 bitops.
-- ---------------------------------------------------------------------------

local K = {
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
}

local function rotr(x, n)
    return ((x >> n) | (x << (32 - n))) & 0xFFFFFFFF
end

local function sha256(msg)
    local H = {
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    }

    local bit_len = #msg * 8
    msg = msg .. "\x80"
    while (#msg % 64) ~= 56 do msg = msg .. "\0" end
    msg = msg .. string.pack(">I8", bit_len)

    for chunk_start = 1, #msg, 64 do
        local W = {}
        for i = 0, 15 do
            local off = chunk_start + i * 4
            W[i + 1] = (msg:byte(off) << 24)
                | (msg:byte(off + 1) << 16)
                | (msg:byte(off + 2) << 8)
                | msg:byte(off + 3)
        end
        for i = 17, 64 do
            local w15 = W[i - 15]
            local w2 = W[i - 2]
            local s0 = rotr(w15, 7) ~ rotr(w15, 18) ~ (w15 >> 3)
            local s1 = rotr(w2, 17) ~ rotr(w2, 19) ~ (w2 >> 10)
            W[i] = (W[i - 16] + s0 + W[i - 7] + s1) & 0xFFFFFFFF
        end

        local a, b, c, d, e, f, g, h =
            H[1], H[2], H[3], H[4], H[5], H[6], H[7], H[8]

        for i = 1, 64 do
            local S1 = rotr(e, 6) ~ rotr(e, 11) ~ rotr(e, 25)
            local ch = (e & f) ~ ((~e & 0xFFFFFFFF) & g)
            local t1 = (h + S1 + ch + K[i] + W[i]) & 0xFFFFFFFF
            local S0 = rotr(a, 2) ~ rotr(a, 13) ~ rotr(a, 22)
            local mj = (a & b) ~ (a & c) ~ (b & c)
            local t2 = (S0 + mj) & 0xFFFFFFFF
            h = g
            g = f
            f = e
            e = (d + t1) & 0xFFFFFFFF
            d = c
            c = b
            b = a
            a = (t1 + t2) & 0xFFFFFFFF
        end

        H[1] = (H[1] + a) & 0xFFFFFFFF
        H[2] = (H[2] + b) & 0xFFFFFFFF
        H[3] = (H[3] + c) & 0xFFFFFFFF
        H[4] = (H[4] + d) & 0xFFFFFFFF
        H[5] = (H[5] + e) & 0xFFFFFFFF
        H[6] = (H[6] + f) & 0xFFFFFFFF
        H[7] = (H[7] + g) & 0xFFFFFFFF
        H[8] = (H[8] + h) & 0xFFFFFFFF
    end

    return string.pack(">I4I4I4I4I4I4I4I4",
        H[1], H[2], H[3], H[4], H[5], H[6], H[7], H[8])
end

local function hmac_sha256(key, msg)
    if #key > 64 then key = sha256(key) end
    if #key < 64 then key = key .. string.rep("\0", 64 - #key) end
    local opad, ipad = {}, {}
    for i = 1, 64 do
        local b = key:byte(i)
        opad[i] = string.char(b ~ 0x5c)
        ipad[i] = string.char(b ~ 0x36)
    end
    return sha256(table.concat(opad) .. sha256(table.concat(ipad) .. msg))
end

local function tohex(s)
    return (s:gsub(".", function(c) return string.format("%02x", c:byte()) end))
end

-- Constant-time string comparison. Both inputs already match in length
-- (they're both hex-encoded SHA-256 output = 64 chars), but we check
-- defensively rather than short-circuiting.
local function ct_eq(a, b)
    if #a ~= #b then return false end
    local diff = 0
    for i = 1, #a do
        diff = diff | (a:byte(i) ~ b:byte(i))
    end
    return diff == 0
end

-- ---------------------------------------------------------------------------
-- Base64 URL-safe decode (no padding).
-- ---------------------------------------------------------------------------

local b64alpha = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
local b64lookup = {}
for i = 1, #b64alpha do b64lookup[b64alpha:sub(i, i)] = i - 1 end

local function b64url_decode(s)
    s = s:gsub("-", "+"):gsub("_", "/"):gsub("[^A-Za-z0-9+/]", "")
    local out = {}
    for i = 1, #s, 4 do
        local c1 = b64lookup[s:sub(i, i)]
        local c2 = b64lookup[s:sub(i + 1, i + 1)]
        local c3 = b64lookup[s:sub(i + 2, i + 2)]
        local c4 = b64lookup[s:sub(i + 3, i + 3)]
        if not c1 or not c2 then return nil end
        local n = (c1 << 18) | (c2 << 12) | ((c3 or 0) << 6) | (c4 or 0)
        out[#out + 1] = string.char((n >> 16) & 0xFF)
        if c3 then out[#out + 1] = string.char((n >> 8) & 0xFF) end
        if c4 then out[#out + 1] = string.char(n & 0xFF) end
    end
    return table.concat(out)
end

-- ---------------------------------------------------------------------------
-- Secret loading (cached per worker process).
-- ---------------------------------------------------------------------------

local cached_secret = nil

local function load_secret()
    if cached_secret then return cached_secret end
    local f = io.open(SECRET_PATH, "r")
    if not f then return nil end
    local s = f:read("*a") or ""
    f:close()
    cached_secret = (s:gsub("[\r\n%s]+$", ""))
    if cached_secret == "" then cached_secret = nil end
    return cached_secret
end

-- ---------------------------------------------------------------------------
-- Cookie extraction.
-- ---------------------------------------------------------------------------

local function get_cookie(r, name)
    local hdr = r.headers_in["Cookie"]
    if not hdr then return nil end
    for part in hdr:gmatch("[^;]+") do
        local k, v = part:match("^%s*([^=]+)=(.*)$")
        if k == name then return v end
    end
    return nil
end

-- ---------------------------------------------------------------------------
-- Pure token verification (no Apache deps). Returns the parsed claims
-- table { jti, eppn, display_name } on success, or `nil, reason` on
-- failure where reason is one of "malformed", "expired", "bad_signature",
-- "bad_eppn". Tests in apache/test/ call this directly.
-- ---------------------------------------------------------------------------

local function verify_token(secret, cookie_value, now)
    if not cookie_value or cookie_value == "" then
        return nil, "malformed"
    end
    local raw = b64url_decode(cookie_value)
    if not raw or raw == "" then return nil, "malformed" end

    -- Token format: jti:eppn_b64:display_b64:exp_ts:hex_sig
    -- (eppn is base64-encoded inside because it contains `:`; see Rust
    --  external_auth::mint_token for the matching producer.)
    local jti, eppn_b64, display_b64, exp_ts, sig =
        raw:match("^([^:]+):([^:]+):([^:]*):([^:]+):([^:]+)$")
    if not jti then return nil, "malformed" end

    local exp_n = tonumber(exp_ts)
    if not exp_n then return nil, "malformed" end
    if (now or os.time()) > exp_n then return nil, "expired" end

    local payload = jti .. ":" .. eppn_b64 .. ":" .. display_b64 .. ":" .. exp_ts
    local expected = tohex(hmac_sha256(secret, payload))
    if not ct_eq(expected, sig) then return nil, "bad_signature" end

    local eppn = b64url_decode(eppn_b64)
    if not eppn or eppn == "" then return nil, "bad_eppn" end
    -- Defensive: the eppn embedded in our own tokens always carries the
    -- ext: prefix. A token without it is either malformed or forged with
    -- a different signing scheme; either way, reject.
    if not eppn:find("^ext:") then return nil, "bad_eppn" end

    local display_name = nil
    if display_b64 ~= "" then
        local d = b64url_decode(display_b64)
        if d and d ~= "" then display_name = d end
    end

    return { jti = jti, eppn = eppn, display_name = display_name }
end

-- ---------------------------------------------------------------------------
-- Access checker (the actual Apache hook). Wraps verify_token with
-- cookie extraction, secret loading, and request-header injection.
--
-- This hook is mounted only on the cookie-bypass `/__ext_proxy__/` path
-- (see minerva-app.conf), reached via mod_rewrite when the cookie is
-- present. So "no cookie" here means something is wrong with the
-- rewrite or the client crafted a request to the bypass path directly:
-- in either case, 401.
--
-- mod_lua on Debian Apache 2.4 doesn't expose `apache2.HTTP_*` constants
-- (only `apache2.OK = 0` and `apache2.DECLINED = -1`); raw integers are
-- the portable choice.
-- ---------------------------------------------------------------------------

function check_ext_auth(r)
    local cookie = get_cookie(r, COOKIE_NAME)
    if not cookie or cookie == "" then return 401 end

    local secret = load_secret()
    if not secret then
        r:err("minerva-ext-auth: secret file unreadable: " .. SECRET_PATH)
        return 500
    end

    local claims, _reason = verify_token(secret, cookie)
    if not claims then return 401 end

    r.headers_in["eppn"] = claims.eppn
    r.headers_in["X-Minerva-Ext-Jti"] = claims.jti
    if claims.display_name then
        r.headers_in["displayName"] = claims.display_name
    end

    return apache2_mod.OK
end

-- ---------------------------------------------------------------------------
-- Module table for tests. mod_lua discards the return value of the
-- script when loading it via `LuaHookAccessChecker`, so this is purely
-- for the test harness in apache/test/.
-- ---------------------------------------------------------------------------

return {
    sha256       = sha256,
    hmac_sha256  = hmac_sha256,
    tohex        = tohex,
    ct_eq        = ct_eq,
    b64url_decode = b64url_decode,
    verify_token = verify_token,
}
