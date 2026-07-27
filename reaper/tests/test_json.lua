--[[ test_json.lua -- JSON encode/decode round trips, escapes, unicode, errors. ]]

local H = require("harness")
local json = require("json")

local function roundtrip(text)
  local v, err = json.decode(text)
  H.ok(v ~= nil, "decode failed: " .. tostring(err))
  local out = json.encode(v)
  H.ok(out ~= nil, "encode failed")
  local v2 = json.decode(out)
  H.ok(v2 ~= nil, "re-decode failed")
  return v, out
end

return {
  { "decodes scalars", function()
    H.eq(json.decode("true"), true)
    H.eq(json.decode("false"), false)
    H.eq(json.decode("null"), json.null)
    H.eq(json.decode("0"), 0)
    H.eq(json.decode("-17"), -17)
    H.eq(json.decode("3.5"), 3.5)
    H.eq(json.decode("1e3"), 1000.0)
    H.eq(json.decode('"hi"'), "hi")
  end },

  { "integers stay integers", function()
    H.eq(math.type(json.decode("42")), "integer")
    H.eq(math.type(json.decode("42.0")), "float")
    H.eq(json.encode(42), "42")
    H.eq(json.encode(42.0), "42")
  end },

  { "encodes floats that round-trip", function()
    local v = 0.1 + 0.2
    local s = json.encode(v)
    H.eq(tonumber(s), v, "float must round-trip exactly")
    H.eq(json.encode(1.5), "1.5")
  end },

  { "rejects NaN and infinity", function()
    H.falsy(json.encode(0 / 0))
    H.falsy(json.encode(math.huge))
    H.falsy(json.encode(-math.huge))
  end },

  { "object keys are sorted for determinism", function()
    local s = json.encode({ zeta = 1, alpha = 2, mid = 3 })
    H.eq(s, '{"alpha":2,"mid":3,"zeta":1}')
  end },

  { "empty array and empty object are distinguishable", function()
    H.eq(json.encode(json.array({})), "[]")
    H.eq(json.encode(json.object({})), "{}")
    H.eq(json.encode(json.decode("[]")), "[]")
    H.eq(json.encode(json.decode("{}")), "{}")
  end },

  { "escapes control characters and quotes", function()
    H.eq(json.encode('a"b\\c'), '"a\\"b\\\\c"')
    H.eq(json.encode("line\nbreak"), '"line\\nbreak"')
    H.eq(json.encode("tab\there"), '"tab\\there"')
    H.eq(json.encode("\1"), '"\\u0001"')
    H.eq(json.encode("\127"), '"\\u007f"')
  end },

  { "decodes escape sequences", function()
    H.eq(json.decode('"\\u0041\\u0042"'), "AB")
    H.eq(json.decode('"a\\/b"'), "a/b")
    H.eq(json.decode('"\\b\\f\\n\\r\\t"'), "\b\f\n\r\t")
  end },

  { "decodes non-BMP surrogate pairs into UTF-8", function()
    -- U+1F600 GRINNING FACE
    local s = json.decode('"\\ud83d\\ude00"')
    H.eq(s, "\240\159\152\128")
    H.eq(#s, 4)
  end },

  { "rejects lone surrogates", function()
    H.falsy(json.decode('"\\ud83d"'))
    H.falsy(json.decode('"\\udc00"'))
  end },

  { "passes raw UTF-8 through unchanged", function()
    local s = "caf\195\169 \228\184\173\230\150\135"
    H.eq(json.decode(json.encode(s)), s)
  end },

  { "round-trips a nested document", function()
    local text =
      '{"a":[1,2,{"b":null,"c":[true,false]}],"d":{"e":"f"},"g":-1.25,"h":"\\u00e9"}'
    local v, out = roundtrip(text)
    H.eq(v.a[3].b, json.null)
    H.eq(v.g, -1.25)
    H.eq(v.h, "\195\169")
    H.contains(out, '"g":-1.25')
  end },

  { "rejects malformed input", function()
    local bad = {
      "", "   ", "{", "[", "{\"a\"}", "{\"a\":}", "[1,]", "{,}", "tru", "nul",
      "01", "-", "1.", ".5", "1e", '"unterminated', "{\"a\":1}extra", "[1 2]",
      '"\9"', "{'a':1}", "+1", "1e+", "[1,,2]",
    }
    for i = 1, #bad do
      local v, err = json.decode(bad[i])
      H.ok(v == nil, string.format("input %q should not decode (got %s)", bad[i], tostring(v)))
      H.ok(type(err) == "string" and #err > 0, "an error message is required")
    end
  end },

  { "rejects non-string input", function()
    H.falsy(json.decode(nil))
    H.falsy(json.decode(42))
  end },

  { "enforces a depth limit both ways", function()
    local deep = string.rep("[", 200) .. string.rep("]", 200)
    H.falsy(json.decode(deep))
    local t = {}
    local cur = t
    for _ = 1, 200 do
      local n = {}
      cur[1] = n
      cur = n
    end
    H.falsy(json.encode(json.array(t)))
  end },

  { "detects cycles", function()
    local t = {}
    t.self = t
    H.falsy(json.encode(t))
  end },

  { "pretty printing is parseable", function()
    local doc = { a = 1, b = json.array({ 1, 2 }), c = { d = "x" } }
    local s = json.encode(doc, { indent = "  " })
    H.contains(s, "\n")
    local back = json.decode(s)
    H.eq(back.a, 1)
    H.eq(back.b[2], 2)
    H.eq(back.c.d, "x")
  end },

  { "null helpers behave", function()
    H.ok(json.is_null(json.null))
    H.falsy(json.is_null(false))
    H.eq(json.denull(json.null), nil)
    H.eq(json.denull(7), 7)
  end },

  { "encoding is byte-stable across calls", function()
    local doc = { z = 1, a = { m = json.array({ 3, 1, 2 }) }, k = "v" }
    H.eq(json.encode(doc), json.encode(doc))
  end },
}
