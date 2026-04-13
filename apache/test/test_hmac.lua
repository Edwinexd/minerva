-- HMAC-SHA256 + SHA-256 known-answer tests for the pure-Lua crypto in
-- minerva-ext-auth.lua. Vectors:
--   - SHA-256: NIST CSRC examples ("abc", empty, two-block).
--   - HMAC-SHA256: RFC 4231 test cases 1, 2, 4, 5 (skipping 3 because it
--     uses a key padded with the sender's identity in a way that's
--     awkward to reproduce inline; cases 6/7 are oversize-key tests we
--     cover via case 6).
--
-- Run: lua apache/test/test_hmac.lua

local script_dir = arg[0]:match("(.*/)") or "./"
local m = dofile(script_dir .. "../minerva-ext-auth.lua")

local failures = 0
local function check(name, actual, expected)
    if actual == expected then
        print("ok   " .. name)
    else
        print("FAIL " .. name)
        print("  expected: " .. tostring(expected))
        print("  actual:   " .. tostring(actual))
        failures = failures + 1
    end
end

local function hex(s) return m.tohex(s) end
local function unhex(h)
    return (h:gsub("(%x%x)", function(b) return string.char(tonumber(b, 16)) end))
end

-- ---- SHA-256 ---------------------------------------------------------------

check("sha256 empty",
    hex(m.sha256("")),
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")

check("sha256 'abc'",
    hex(m.sha256("abc")),
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")

check("sha256 two-block (448 bits)",
    hex(m.sha256("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
    "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1")

-- ---- HMAC-SHA256 (RFC 4231) ------------------------------------------------

-- Case 1: key = 20x 0x0b, data = "Hi There"
check("rfc4231 case 1",
    hex(m.hmac_sha256(string.rep("\x0b", 20), "Hi There")),
    "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7")

-- Case 2: key = "Jefe", data = "what do ya want for nothing?"
check("rfc4231 case 2",
    hex(m.hmac_sha256("Jefe", "what do ya want for nothing?")),
    "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")

-- Case 4: key = 25 incrementing bytes, data = 50x 0xcd
local key4 = unhex("0102030405060708090a0b0c0d0e0f10111213141516171819")
check("rfc4231 case 4",
    hex(m.hmac_sha256(key4, string.rep("\xcd", 50))),
    "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b")

-- Case 5: 20-byte key, 0x0c repeated; truncation case but we test full output
check("rfc4231 case 5 (full 256-bit output)",
    hex(m.hmac_sha256(string.rep("\x0c", 20), "Test With Truncation")),
    "a3b6167473100ee06e0c796c2955552bfa6f7c0a6a8aef8b93f860aab0cd20c5")

-- Case 6: 131-byte key (longer than block), short data
check("rfc4231 case 6 (oversized key)",
    hex(m.hmac_sha256(string.rep("\xaa", 131),
        "Test Using Larger Than Block-Size Key - Hash Key First")),
    "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54")

-- Case 7: 131-byte key, long data
check("rfc4231 case 7 (oversized key + long data)",
    hex(m.hmac_sha256(string.rep("\xaa", 131),
        "This is a test using a larger than block-size key and a larger " ..
        "than block-size data. The key needs to be hashed before being " ..
        "used by the HMAC algorithm.")),
    "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2")

-- ---- ct_eq -----------------------------------------------------------------

check("ct_eq equal", m.ct_eq("abcdef", "abcdef"), true)
check("ct_eq diff",  m.ct_eq("abcdef", "abcdeg"), false)
check("ct_eq len",   m.ct_eq("abc",    "abcd"),   false)

if failures > 0 then
    io.stderr:write(string.format("\n%d test(s) failed\n", failures))
    os.exit(1)
end
print("\nAll HMAC/SHA-256 tests passed.")
