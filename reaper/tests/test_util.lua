--[[ test_util.lua -- hashing, time parsing, ids, ring set, logging rotation. ]]

local H = require("harness")
local util = require("util")

return {
  { "FNV-1a 64 matches the reference vectors", function()
    -- Reference values for FNV-1a 64-bit (fnvhash spec / test vectors).
    H.eq(string.format("%016x", util.fnv1a64("")), "cbf29ce484222325")
    H.eq(string.format("%016x", util.fnv1a64("a")), "af63dc4c8601ec8c")
    H.eq(string.format("%016x", util.fnv1a64("foobar")), "85944171f73967e8")
  end },

  { "hash_hex is prefixed and 16 hex digits", function()
    local h = util.hash_hex("hello")
    H.eq(h:sub(1, 8), "fnv1a64:")
    H.eq(#h, 8 + 16)
    H.ok(h:sub(9):match("^%x%x%x%x%x%x%x%x%x%x%x%x%x%x%x%x$"))
  end },

  { "hashing is chunk-boundary safe", function()
    -- The implementation batches string.byte in 64-byte groups; verify a long
    -- input matches a byte-at-a-time reference computation.
    local s = string.rep("The quick brown fox. ", 30)
    local h = 0xcbf29ce484222325
    for i = 1, #s do h = (h ~ string.byte(s, i)) * 0x100000001b3 end
    H.eq(util.fnv1a64(s), h)
  end },

  { "fmt6 normalises negative zero and rejects non-finite", function()
    H.eq(util.fmt6(0), "0.000000")
    H.eq(util.fmt6(-0.0), "0.000000")
    H.eq(util.fmt6(1.5), "1.500000")
    H.eq(util.fmt6(-2.25), "-2.250000")
    H.eq(util.fmt6(0 / 0), nil)
    H.eq(util.fmt6(math.huge), nil)
    H.eq(util.fmt6(1e13), nil)
  end },

  { "fmtint renders integers", function()
    H.eq(util.fmtint(7), "7")
    H.eq(util.fmtint(-7), "-7")
    H.eq(util.fmtint(7.0), "7")
  end },

  { "parses the ISO-8601 subset", function()
    H.eq(util.parse_iso8601("1970-01-01T00:00:00Z"), 0)
    H.eq(util.parse_iso8601("2026-07-26T18:51:19Z"), 1785091879)
    H.eq(util.parse_iso8601("2026-07-26T18:51:19.250Z"), 1785091879)
    H.eq(util.parse_iso8601("2026-07-26T18:51:19"), 1785091879)
    H.eq(util.parse_iso8601("2026-07-26T20:51:19+02:00"), 1785091879)
    H.eq(util.parse_iso8601("2026-07-26T16:51:19-0200"), 1785091879)
  end },

  { "rejects malformed timestamps", function()
    local bad = { "", "not a time", "2026-07-26", "2026-13-01T00:00:00Z",
      "2026-07-26T25:00:00Z", "2026-07-26 18:51", "2026/07/26T18:51:19Z" }
    for i = 1, #bad do
      H.eq(util.parse_iso8601(bad[i]), nil, "should reject " .. bad[i])
    end
  end },

  { "iso8601 round-trips through parse", function()
    local t = 1785091879
    H.eq(util.parse_iso8601(util.iso8601(t)), t)
  end },

  { "id validation blocks path escapes", function()
    H.ok(util.is_safe_id("abc-123_x.9"))
    H.ok(util.is_safe_id("a"))
    H.falsy(util.is_safe_id(""))
    H.falsy(util.is_safe_id("../etc/passwd"))
    H.falsy(util.is_safe_id("a/b"))
    H.falsy(util.is_safe_id("a\\b"))
    H.falsy(util.is_safe_id(".hidden"))
    H.falsy(util.is_safe_id("a..b"))
    H.falsy(util.is_safe_id("with space"))
    H.falsy(util.is_safe_id(string.rep("a", 200), 128))
    H.falsy(util.is_safe_id("C:\\Windows"))
  end },

  { "ring set is bounded and detects duplicates", function()
    local r = util.ring_set(3)
    H.ok(r:add("a"))
    H.ok(r:add("b"))
    H.falsy(r:add("a"), "duplicate must be reported")
    H.ok(r:contains("a"))
    r:add("c")
    r:add("d")
    H.eq(r:size(), 3)
    H.falsy(r:contains("a"), "oldest entry must be evicted")
    H.ok(r:contains("d"))
  end },

  { "is_array distinguishes arrays from maps", function()
    H.ok(util.is_array({}))
    H.ok(util.is_array({ 1, 2, 3 }))
    H.falsy(util.is_array({ a = 1 }))
    H.falsy(util.is_array({ [1] = 1, [3] = 3 }))
    H.falsy(util.is_array("x"))
  end },

  { "path joining collapses separators", function()
    H.eq(util.join("a", "b"), "a" .. util.sep .. "b")
    H.eq(util.join("a/", "/b"), "a" .. util.sep .. "b")
    H.eq(util.join("a", nil, "b"), "a" .. util.sep .. "b")
    H.eq(util.basename("/x/y/z.json"), "z.json")
    H.eq(util.dirname("/x/y/z.json"), "/x/y")
  end },

  { "chunk_dir resolves from the running chunk, not a hardcoded path", function()
    local d = util.chunk_dir(1)
    H.ok(type(d) == "string" and #d > 0)
    H.ok(d:find("tests") ~= nil or d == ".", "expected the tests directory, got " .. d)
  end },

  { "logger rotates at the configured size", function()
    local mock_fs = require("mock_fs")
    local fs = mock_fs.new(os.time)
    util.fs = H.fs_adapter(fs)
    fs:mkdirp("/logs")
    local log = util.logger("/logs", { name = "bridge.log", max_bytes = 200, keep = 2, level = "debug" })
    for i = 1, 40 do log:info("entry number %d with padding xxxxxxxxxxxxxxxx", i) end
    H.ok(fs:exists("/logs/bridge.log"), "current log must exist")
    H.ok(fs:exists("/logs/bridge.log.1"), "one rotation must exist")
    H.ok(fs:size("/logs/bridge.log") <= 400, "current log must stay bounded")
    H.falsy(fs:exists("/logs/bridge.log.3"), "rotations beyond keep must be dropped")
    util.fs = util.realfs
  end },

  { "logger honours the level threshold", function()
    local mock_fs = require("mock_fs")
    local fs = mock_fs.new(os.time)
    util.fs = H.fs_adapter(fs)
    fs:mkdirp("/logs")
    local log = util.logger("/logs", { level = "warn" })
    log:debug("nope")
    log:info("nope")
    log:warn("yes")
    local body = fs:read("/logs/bridge.log") or ""
    H.falsy(body:find("nope", 1, true))
    H.contains(body, "yes")
    util.fs = util.realfs
  end },

  { "atomic_write leaves no temp file behind", function()
    local mock_fs = require("mock_fs")
    local fs = mock_fs.new(os.time)
    util.fs = H.fs_adapter(fs)
    fs:mkdirp("/d")
    H.ok(util.atomic_write("/d/f.json", "{}"))
    H.eq(fs:read("/d/f.json"), "{}")
    H.eq(fs:count(), 1, "only the destination may remain")
    util.fs = util.realfs
  end },
}
