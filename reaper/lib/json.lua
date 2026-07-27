--[[
  json.lua -- minimal, dependency-free JSON encoder/decoder for Lua 5.4.

  Copyright (c) 2026 QLabs

  Permission is hereby granted, free of charge, to any person obtaining a copy
  of this software and associated documentation files (the "Software"), to deal
  in the Software without restriction, including without limitation the rights
  to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
  copies of the Software, and to permit persons to whom the Software is
  furnished to do so, subject to the following conditions:

  The above copyright notice and this permission notice shall be included in all
  copies or substantial portions of the Software.

  THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
  IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
  FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
  AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
  LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
  OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
  SOFTWARE.

  Written from scratch for this project. No third-party code is vendored.

  Design notes
  ------------
  * `json.null` is a unique sentinel so a JSON `null` survives a round trip
    through a Lua table without silently deleting the key.
  * Decoded arrays carry the metatable `json.array_mt` so an empty array
    re-encodes as `[]` rather than `{}`.
  * Object keys are sorted on encode by default; the bridge's output must be
    byte-deterministic for a given value.
  * Both directions enforce a nesting depth limit of 128.
--]]

local M = {}

--- Maximum nesting depth accepted by both the encoder and the decoder.
M.MAX_DEPTH = 128

--- Unique sentinel representing JSON `null`.
M.null = setmetatable({}, { __tostring = function() return "json.null" end })

--- Metatable stamped on tables that must encode as JSON arrays.
M.array_mt = { __jsontype = "array" }

--- Metatable stamped on tables that must encode as JSON objects.
M.object_mt = { __jsontype = "object" }

--- Marks `t` (default a fresh table) as a JSON array.
function M.array(t)
  return setmetatable(t or {}, M.array_mt)
end

--- Marks `t` (default a fresh table) as a JSON object.
function M.object(t)
  return setmetatable(t or {}, M.object_mt)
end

--- True when `v` is the JSON null sentinel.
function M.is_null(v)
  return v == M.null
end

--- Maps the null sentinel to Lua nil, passing everything else through.
function M.denull(v)
  if v == M.null then return nil end
  return v
end

--------------------------------------------------------------------------------
-- Encoding
--------------------------------------------------------------------------------

local ESCAPES = {
  ['"'] = '\\"',
  ["\\"] = "\\\\",
  ["\b"] = "\\b",
  ["\f"] = "\\f",
  ["\n"] = "\\n",
  ["\r"] = "\\r",
  ["\t"] = "\\t",
}

local function escape_char(c)
  local e = ESCAPES[c]
  if e then return e end
  return string.format("\\u%04x", string.byte(c))
end

local function encode_string(s)
  -- Escape the two mandatory characters plus all C0 controls and DEL. Bytes
  -- >= 0x20 other than '"' and '\\' (including raw UTF-8) are emitted verbatim,
  -- which RFC 8259 permits.
  return '"' .. s:gsub('[%c"\\\127]', escape_char) .. '"'
end

local function encode_number(v)
  if v ~= v then error("json: cannot encode NaN", 0) end
  if v == math.huge or v == -math.huge then error("json: cannot encode infinity", 0) end
  if math.type(v) == "integer" then
    return string.format("%d", v)
  end
  -- Shortest representation that round-trips exactly.
  local s = string.format("%.14g", v)
  if tonumber(s) ~= v then
    s = string.format("%.17g", v)
  end
  -- Guarantee the token still reads as a JSON number.
  if not s:match("[%.eE]") and not s:match("^-?%d+$") then
    s = string.format("%.17g", v)
  end
  return s
end

local encode_value

local function table_kind(v)
  local mt = getmetatable(v)
  local hint = mt and rawget(mt, "__jsontype")
  if hint == "array" then return "array" end
  if hint == "object" then return "object" end
  local n = 0
  for k in pairs(v) do
    if type(k) ~= "number" or k % 1 ~= 0 or k < 1 then return "object" end
    if k > n then n = k end
  end
  if n == 0 then return "object" end
  for i = 1, n do
    if rawget(v, i) == nil then return "object" end
  end
  return "array"
end

encode_value = function(v, out, opts, depth, seen)
  if depth > M.MAX_DEPTH then
    error("json: encode depth limit exceeded", 0)
  end
  local t = type(v)
  if v == M.null or v == nil then
    out[#out + 1] = "null"
  elseif t == "boolean" then
    out[#out + 1] = v and "true" or "false"
  elseif t == "number" then
    out[#out + 1] = encode_number(v)
  elseif t == "string" then
    out[#out + 1] = encode_string(v)
  elseif t == "table" then
    if seen[v] then error("json: cycle detected", 0) end
    seen[v] = true
    local kind = table_kind(v)
    local indent = opts.indent
    local nl, pad, pad2 = "", "", ""
    if indent then
      nl = "\n"
      pad = string.rep(indent, depth)
      pad2 = string.rep(indent, depth - 1)
    end
    if kind == "array" then
      local n = #v
      if n == 0 then
        out[#out + 1] = "[]"
      else
        out[#out + 1] = "[" .. nl
        for i = 1, n do
          if i > 1 then out[#out + 1] = "," .. nl end
          out[#out + 1] = pad
          encode_value(v[i], out, opts, depth + 1, seen)
        end
        out[#out + 1] = nl .. pad2 .. "]"
      end
    else
      local keys = {}
      for k in pairs(v) do
        if type(k) == "string" then
          keys[#keys + 1] = k
        elseif type(k) == "number" then
          keys[#keys + 1] = encode_number(k)
        else
          error("json: unsupported key type " .. type(k), 0)
        end
      end
      if opts.sort_keys ~= false then table.sort(keys) end
      if #keys == 0 then
        out[#out + 1] = "{}"
      else
        out[#out + 1] = "{" .. nl
        for i = 1, #keys do
          if i > 1 then out[#out + 1] = "," .. nl end
          local k = keys[i]
          out[#out + 1] = pad
          out[#out + 1] = encode_string(k)
          out[#out + 1] = opts.indent and ": " or ":"
          local val = v[k]
          if val == nil then val = v[tonumber(k)] end
          encode_value(val, out, opts, depth + 1, seen)
        end
        out[#out + 1] = nl .. pad2 .. "}"
      end
    end
    seen[v] = nil
  else
    error("json: cannot encode value of type " .. t, 0)
  end
end

--- Encodes a Lua value as JSON text.
--- opts.indent  -- indentation string for pretty output (nil == compact)
--- opts.sort_keys -- defaults to true; set false to keep pairs() order
--- Returns the JSON string, or nil plus an error message on failure.
function M.encode(value, opts)
  opts = opts or {}
  local out = {}
  local ok, err = pcall(encode_value, value, out, opts, 1, {})
  if not ok then return nil, tostring(err) end
  return table.concat(out)
end

--- Encodes or raises. Convenience for call sites already inside an xpcall.
function M.encode_or_error(value, opts)
  local s, err = M.encode(value, opts)
  if not s then error(err, 0) end
  return s
end

--------------------------------------------------------------------------------
-- Decoding
--------------------------------------------------------------------------------

local Decoder = {}
Decoder.__index = Decoder

local WHITESPACE = { [' '] = true, ['\t'] = true, ['\n'] = true, ['\r'] = true }

function Decoder:err(msg)
  error(string.format("json: %s at byte %d", msg, self.pos), 0)
end

function Decoder:skip_ws()
  local s, n = self.s, self.n
  local i = self.pos
  while i <= n do
    local c = s:sub(i, i)
    if WHITESPACE[c] then
      i = i + 1
    else
      break
    end
  end
  self.pos = i
end

function Decoder:peek()
  return self.s:sub(self.pos, self.pos)
end

function Decoder:expect(c)
  if self:peek() ~= c then self:err("expected '" .. c .. "'") end
  self.pos = self.pos + 1
end

local function utf8_encode(cp)
  if cp < 0x80 then
    return string.char(cp)
  elseif cp < 0x800 then
    return string.char(0xC0 | (cp >> 6), 0x80 | (cp & 0x3F))
  elseif cp < 0x10000 then
    return string.char(0xE0 | (cp >> 12), 0x80 | ((cp >> 6) & 0x3F), 0x80 | (cp & 0x3F))
  else
    return string.char(0xF0 | (cp >> 18), 0x80 | ((cp >> 12) & 0x3F),
      0x80 | ((cp >> 6) & 0x3F), 0x80 | (cp & 0x3F))
  end
end

function Decoder:read_hex4()
  local h = self.s:sub(self.pos, self.pos + 3)
  if #h < 4 or not h:match("^%x%x%x%x$") then self:err("invalid \\u escape") end
  self.pos = self.pos + 4
  return tonumber(h, 16)
end

function Decoder:parse_string()
  self:expect('"')
  local buf = {}
  local s, n = self.s, self.n
  while true do
    if self.pos > n then self:err("unterminated string") end
    local c = s:sub(self.pos, self.pos)
    if c == '"' then
      self.pos = self.pos + 1
      break
    elseif c == "\\" then
      self.pos = self.pos + 1
      local e = s:sub(self.pos, self.pos)
      self.pos = self.pos + 1
      if e == '"' then buf[#buf + 1] = '"'
      elseif e == "\\" then buf[#buf + 1] = "\\"
      elseif e == "/" then buf[#buf + 1] = "/"
      elseif e == "b" then buf[#buf + 1] = "\b"
      elseif e == "f" then buf[#buf + 1] = "\f"
      elseif e == "n" then buf[#buf + 1] = "\n"
      elseif e == "r" then buf[#buf + 1] = "\r"
      elseif e == "t" then buf[#buf + 1] = "\t"
      elseif e == "u" then
        local cp = self:read_hex4()
        if cp >= 0xD800 and cp <= 0xDBFF then
          -- High surrogate: a low surrogate must follow.
          if s:sub(self.pos, self.pos + 1) ~= "\\u" then self:err("lone high surrogate") end
          self.pos = self.pos + 2
          local lo = self:read_hex4()
          if lo < 0xDC00 or lo > 0xDFFF then self:err("invalid low surrogate") end
          cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00)
        elseif cp >= 0xDC00 and cp <= 0xDFFF then
          self:err("lone low surrogate")
        end
        buf[#buf + 1] = utf8_encode(cp)
      else
        self:err("invalid escape '\\" .. e .. "'")
      end
    elseif c == "" then
      self:err("unterminated string")
    else
      local b = string.byte(c)
      if b < 0x20 then self:err("unescaped control character") end
      -- Consume a run of ordinary characters for speed.
      local j = s:find('[%\\"%c]', self.pos)
      if not j then self:err("unterminated string") end
      buf[#buf + 1] = s:sub(self.pos, j - 1)
      self.pos = j
    end
  end
  return table.concat(buf)
end

function Decoder:parse_number()
  local s = self.s
  local start = self.pos
  local _, e = s:find("^%-?%d+", start)
  if not e then self:err("invalid number") end
  local pos = e + 1
  local is_float = false
  if s:sub(pos, pos) == "." then
    local _, e2 = s:find("^%.%d+", pos)
    if not e2 then self:err("invalid number") end
    pos = e2 + 1
    is_float = true
  end
  local c = s:sub(pos, pos)
  if c == "e" or c == "E" then
    local _, e3 = s:find("^[eE][%+%-]?%d+", pos)
    if not e3 then self:err("invalid number") end
    pos = e3 + 1
    is_float = true
  end
  local text = s:sub(start, pos - 1)
  -- Reject leading zeros as RFC 8259 requires.
  if text:match("^-?0%d") then self:err("invalid number (leading zero)") end
  self.pos = pos
  if not is_float then
    local iv = math.tointeger(tonumber(text))
    if iv ~= nil then return iv end
  end
  local v = tonumber(text)
  if v == nil then self:err("invalid number") end
  return v
end

function Decoder:parse_value(depth)
  if depth > M.MAX_DEPTH then self:err("depth limit exceeded") end
  self:skip_ws()
  local c = self:peek()
  if c == "" then self:err("unexpected end of input") end
  if c == "{" then
    self.pos = self.pos + 1
    local obj = setmetatable({}, M.object_mt)
    self:skip_ws()
    if self:peek() == "}" then
      self.pos = self.pos + 1
      return obj
    end
    while true do
      self:skip_ws()
      if self:peek() ~= '"' then self:err("expected object key") end
      local k = self:parse_string()
      self:skip_ws()
      self:expect(":")
      local v = self:parse_value(depth + 1)
      obj[k] = v
      self:skip_ws()
      local d = self:peek()
      if d == "," then
        self.pos = self.pos + 1
      elseif d == "}" then
        self.pos = self.pos + 1
        break
      else
        self:err("expected ',' or '}'")
      end
    end
    return obj
  elseif c == "[" then
    self.pos = self.pos + 1
    local arr = setmetatable({}, M.array_mt)
    self:skip_ws()
    if self:peek() == "]" then
      self.pos = self.pos + 1
      return arr
    end
    local i = 0
    while true do
      local v = self:parse_value(depth + 1)
      i = i + 1
      arr[i] = v
      self:skip_ws()
      local d = self:peek()
      if d == "," then
        self.pos = self.pos + 1
      elseif d == "]" then
        self.pos = self.pos + 1
        break
      else
        self:err("expected ',' or ']'")
      end
    end
    return arr
  elseif c == '"' then
    return self:parse_string()
  elseif c == "t" then
    if self.s:sub(self.pos, self.pos + 3) ~= "true" then self:err("invalid literal") end
    self.pos = self.pos + 4
    return true
  elseif c == "f" then
    if self.s:sub(self.pos, self.pos + 4) ~= "false" then self:err("invalid literal") end
    self.pos = self.pos + 5
    return false
  elseif c == "n" then
    if self.s:sub(self.pos, self.pos + 3) ~= "null" then self:err("invalid literal") end
    self.pos = self.pos + 4
    return M.null
  elseif c == "-" or c:match("%d") then
    return self:parse_number()
  end
  self:err("unexpected character '" .. c .. "'")
end

--- Decodes JSON text. Returns the value, or nil plus an error message.
--- JSON `null` decodes to `json.null`; arrays and objects carry metatables so
--- the distinction survives re-encoding.
function M.decode(text)
  if type(text) ~= "string" then return nil, "json: input is not a string" end
  local d = setmetatable({ s = text, n = #text, pos = 1 }, Decoder)
  local ok, value = pcall(Decoder.parse_value, d, 1)
  if not ok then return nil, tostring(value) end
  d:skip_ws()
  if d.pos <= d.n then
    return nil, string.format("json: trailing content at byte %d", d.pos)
  end
  return value
end

return M
