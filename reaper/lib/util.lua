--[[
  util.lua -- shared helpers for the QLabs REAPER MCP bridge.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT

  Pure Lua 5.4. No external packages. This module deliberately keeps every
  filesystem primitive behind `util.fs` so the whole bridge can be driven by an
  in-memory filesystem during host-free testing.
--]]

local M = {}

--------------------------------------------------------------------------------
-- Path helpers
--------------------------------------------------------------------------------

--- Path separator for the host platform. REAPER on Windows accepts '/' in most
--- API calls, but we mirror the platform separator for cosmetic correctness.
M.sep = (package and package.config and package.config:sub(1, 1)) or "/"

--- Returns true when `p` looks like an absolute path on either platform family.
function M.is_abs(p)
  if type(p) ~= "string" or p == "" then return false end
  if p:sub(1, 1) == "/" or p:sub(1, 1) == "\\" then return true end
  if p:match("^%a:[/\\]") then return true end
  return false
end

--- Joins path fragments with the platform separator, collapsing duplicates.
function M.join(...)
  local parts = { ... }
  local out = nil
  for i = 1, #parts do
    local p = parts[i]
    if p ~= nil and p ~= "" then
      p = tostring(p)
      if out == nil then
        out = p
      else
        out = out:gsub("[/\\]+$", "") .. M.sep .. p:gsub("^[/\\]+", "")
      end
    end
  end
  return out or ""
end

--- Returns the directory portion of a path (with trailing separator stripped).
function M.dirname(p)
  local d = tostring(p or ""):match("^(.*)[/\\][^/\\]*$")
  return d or "."
end

--- Returns the final path component.
function M.basename(p)
  local b = tostring(p or ""):match("([^/\\]+)$")
  return b or tostring(p or "")
end

--- Directory of the Lua chunk `level` frames up the stack (1 == caller's chunk).
--- Uses `debug.getinfo(...,"S").source`, never a hardcoded install path.
function M.chunk_dir(level)
  local info = debug.getinfo(level or 2, "S")
  local src = info and info.source or ""
  if src:sub(1, 1) == "@" then src = src:sub(2) end
  local dir = src:match("^(.*)[/\\][^/\\]*$")
  if not dir or dir == "" then return "." end
  return dir
end

--------------------------------------------------------------------------------
-- Identifier / string validation
--------------------------------------------------------------------------------

--- Characters permitted in a request id or transaction id used to build a path.
--- Deliberately excludes '/', '\\', ':' and '.' runs so no IPC-supplied value can
--- escape the configured IPC directory.
M.ID_PATTERN = "^[A-Za-z0-9][A-Za-z0-9._%-]*$"

--- True when `s` is a safe filesystem-facing identifier of bounded length.
function M.is_safe_id(s, maxlen)
  if type(s) ~= "string" then return false end
  if #s < 1 or #s > (maxlen or 128) then return false end
  if not s:match(M.ID_PATTERN) then return false end
  if s:find("..", 1, true) then return false end
  return true
end

--- Trims ASCII whitespace from both ends.
function M.trim(s)
  return (tostring(s or ""):gsub("^%s+", ""):gsub("%s+$", ""))
end

--- Truncates a string to `n` bytes, appending an ellipsis marker when cut.
function M.truncate(s, n)
  s = tostring(s or "")
  if #s <= n then return s end
  return s:sub(1, n - 3) .. "..."
end

--------------------------------------------------------------------------------
-- Numbers
--------------------------------------------------------------------------------

--- True when x is a finite number.
function M.is_finite(x)
  return type(x) == "number" and x == x and x ~= math.huge and x ~= -math.huge
end

--- Canonical fixed-point rendering used by every hash canonical string.
--- Exactly six fractional digits, negative zero normalised to "0.000000".
--- Values outside +/-1e12 or non-finite values are rejected (returns nil).
function M.fmt6(x)
  if not M.is_finite(x) then return nil end
  if x == 0 then x = 0.0 end
  if x > 1e12 or x < -1e12 then return nil end
  return string.format("%.6f", x + 0.0)
end

--- Canonical integer rendering used by hash canonical strings.
function M.fmtint(x)
  if type(x) ~= "number" or x ~= x then return nil end
  local i = math.tointeger(x)
  if i == nil then
    if not M.is_finite(x) then return nil end
    i = math.tointeger(math.floor(x + 0.5))
    if i == nil then return nil end
  end
  return string.format("%d", i)
end

--- Clamps `x` into [lo, hi].
function M.clamp(x, lo, hi)
  if x < lo then return lo end
  if x > hi then return hi end
  return x
end

--- Rounds half away from zero to an integer.
function M.round(x)
  if x >= 0 then return math.floor(x + 0.5) end
  return -math.floor(-x + 0.5)
end

--------------------------------------------------------------------------------
-- Time
--------------------------------------------------------------------------------

--- Current wall-clock time in whole seconds since the Unix epoch.
function M.now()
  return os.time()
end

--- Formats a Unix timestamp as ISO-8601 UTC with second precision.
function M.iso8601(t)
  return os.date("!%Y-%m-%dT%H:%M:%SZ", math.floor(tonumber(t) or os.time()))
end

local function days_from_civil(y, m, d)
  y = y - (m <= 2 and 1 or 0)
  local era = math.floor(y / 400)
  local yoe = y - era * 400
  local mp = (m + 9) % 12
  local doy = math.floor((153 * mp + 2) / 5) + d - 1
  local doe = yoe * 365 + math.floor(yoe / 4) - math.floor(yoe / 100) + doy
  return era * 146097 + doe - 719468
end

--- Parses the ISO-8601 subset the protocol permits and returns Unix seconds.
--- Accepted: `YYYY-MM-DDTHH:MM:SS` with an optional fractional part and an
--- optional zone of `Z`, `+HH:MM`, `-HH:MM`, `+HHMM` or `-HHMM`. A missing zone
--- is treated as UTC. Returns nil on anything else.
function M.parse_iso8601(s)
  if type(s) ~= "string" then return nil end
  s = M.trim(s)
  local y, mo, d, h, mi, sec, rest =
    s:match("^(%d%d%d%d)%-(%d%d)%-(%d%d)[Tt ](%d%d):(%d%d):(%d%d)(.*)$")
  if not y then return nil end
  y, mo, d = tonumber(y), tonumber(mo), tonumber(d)
  h, mi, sec = tonumber(h), tonumber(mi), tonumber(sec)
  if mo < 1 or mo > 12 or d < 1 or d > 31 then return nil end
  if h > 23 or mi > 59 or sec > 60 then return nil end
  rest = rest or ""
  local frac = rest:match("^%.%d+")
  if frac then rest = rest:sub(#frac + 1) end
  local offset = 0
  if rest == "" or rest == "Z" or rest == "z" then
    offset = 0
  else
    local sign, oh, om = rest:match("^([+%-])(%d%d):?(%d%d)$")
    if not sign then return nil end
    offset = (tonumber(oh) * 3600 + tonumber(om) * 60) * (sign == "-" and -1 or 1)
  end
  local days = days_from_civil(y, mo, d)
  return days * 86400 + h * 3600 + mi * 60 + sec - offset
end

--------------------------------------------------------------------------------
-- FNV-1a 64-bit hashing
--------------------------------------------------------------------------------

local FNV_OFFSET_BASIS = 0xcbf29ce484222325 -- wraps to a negative Lua integer
local FNV_PRIME = 0x100000001b3

--- FNV-1a 64-bit hash of a byte string. Lua 5.4 integer arithmetic wraps on
--- overflow (two's complement), which is exactly the modulo-2^64 arithmetic the
--- algorithm requires.
function M.fnv1a64(s)
  local h = FNV_OFFSET_BASIS
  local n = #s
  local i = 1
  local byte = string.byte
  while i <= n do
    local j = i + 63
    if j > n then j = n end
    local b = { byte(s, i, j) }
    for k = 1, #b do
      h = (h ~ b[k]) * FNV_PRIME
    end
    i = j + 1
  end
  return h
end

--- Prefix stamped on every hash string the bridge emits.
M.HASH_PREFIX = "fnv1a64:"

--- Hash of a canonical string, rendered as `fnv1a64:` + 16 lowercase hex digits.
function M.hash_hex(s)
  return M.HASH_PREFIX .. string.format("%016x", M.fnv1a64(s))
end

--------------------------------------------------------------------------------
-- Filesystem backend (swappable)
--------------------------------------------------------------------------------

local realfs = {}

--- Creates `path` and every missing parent. Uses REAPER's recursive create when
--- the host API is present, otherwise falls back to a no-op probe.
function realfs.mkdirp(path)
  if reaper and reaper.RecursiveCreateDirectory then
    reaper.RecursiveCreateDirectory(path, 0)
    return true
  end
  return false
end

function realfs.exists(path)
  if reaper and reaper.file_exists and reaper.file_exists(path) then return true end
  local f = io.open(path, "rb")
  if f then f:close() return true end
  return false
end

function realfs.read(path)
  local f, err = io.open(path, "rb")
  if not f then return nil, err end
  local data = f:read("a")
  f:close()
  return data
end

function realfs.write(path, data)
  local f, err = io.open(path, "wb")
  if not f then return false, err end
  local ok, werr = f:write(data)
  if ok then f:flush() end
  f:close()
  if not ok then return false, werr end
  return true
end

function realfs.append(path, data)
  local f, err = io.open(path, "ab")
  if not f then return false, err end
  local ok, werr = f:write(data)
  if ok then f:flush() end
  f:close()
  if not ok then return false, werr end
  return true
end

function realfs.rename(from, to)
  -- os.rename fails on Windows when the destination exists; remove first.
  if realfs.exists(to) then os.remove(to) end
  local ok, err = os.rename(from, to)
  return ok and true or false, err
end

function realfs.remove(path)
  local ok = os.remove(path)
  return ok and true or false
end

function realfs.listdir(dir)
  local out = {}
  if reaper and reaper.EnumerateFiles then
    local i = 0
    while true do
      local name = reaper.EnumerateFiles(dir, i)
      if name == nil or name == "" then break end
      out[#out + 1] = name
      i = i + 1
      if i > 20000 then break end
    end
  end
  return out
end

function realfs.size(path)
  local f = io.open(path, "rb")
  if not f then return nil end
  local n = f:seek("end")
  f:close()
  return n
end

--- Modification time in Unix seconds, or nil when the host cannot report it.
--- Plain Lua has no stat(); we opportunistically use js_ReaScriptAPI if present.
function realfs.mtime(path)
  if reaper and reaper.JS_File_Stat then
    local ok, _, mtime = pcall(reaper.JS_File_Stat, path)
    if ok and type(mtime) == "number" and mtime > 0 then return mtime end
  end
  return nil
end

M.realfs = realfs

--- The active filesystem backend. Tests replace this wholesale.
M.fs = realfs

--- Writes `data` to `path` atomically: write a sibling temp file, flush, close,
--- then rename over the destination. `tmp_suffix` lets callers honour a wire
--- format that mandates a specific temp name.
function M.atomic_write(path, data, tmp_suffix)
  local tmp = path .. (tmp_suffix or ".tmpwrite")
  local ok, err = M.fs.write(tmp, data)
  if not ok then return false, err end
  local rok, rerr = M.fs.rename(tmp, path)
  if not rok then
    M.fs.remove(tmp)
    return false, rerr
  end
  return true
end

--- Atomic write where the temp file lives at `tmp_path` and the final file at
--- `final_path`, matching the wire protocol's `<id>.result.tmp` -> `.result.json`.
function M.atomic_write_pair(tmp_path, final_path, data)
  local ok, err = M.fs.write(tmp_path, data)
  if not ok then return false, err end
  local rok, rerr = M.fs.rename(tmp_path, final_path)
  if not rok then
    M.fs.remove(tmp_path)
    return false, rerr
  end
  return true
end

--------------------------------------------------------------------------------
-- Table helpers
--------------------------------------------------------------------------------

--- Returns the table's keys as a sorted array of strings.
function M.sorted_keys(t)
  local ks = {}
  for k in pairs(t) do ks[#ks + 1] = tostring(k) end
  table.sort(ks)
  return ks
end

--- Shallow copy.
function M.copy(t)
  local o = {}
  for k, v in pairs(t) do o[k] = v end
  return o
end

--- True when `v` must be treated as a JSON array. The rule matches json.lua's
--- encoder exactly: an explicit `__jsontype` metatable hint wins, otherwise a
--- non-empty table whose keys are exactly 1..n is an array. An EMPTY plain table
--- is an object, which is what makes `payload = {}` a valid JSON object.
function M.is_array(v)
  if type(v) ~= "table" then return false end
  local mt = getmetatable(v)
  local hint = mt and rawget(mt, "__jsontype")
  if hint == "array" then return true end
  if hint == "object" then return false end
  local n = 0
  for k in pairs(v) do
    if type(k) ~= "number" then return false end
    if k < 1 or k % 1 ~= 0 then return false end
    if k > n then n = k end
  end
  if n == 0 then return false end
  for i = 1, n do
    if v[i] == nil then return false end
  end
  return true
end

--- True when `v` is an array-shaped value OR an empty table. Used where the
--- protocol expects a possibly-empty list that a caller may have serialised as
--- `[]` (metatable-tagged) or built as a bare Lua table.
function M.is_list(v)
  if type(v) ~= "table" then return false end
  if M.is_array(v) then return true end
  return next(v) == nil
end

--- Bounded FIFO set, used for the request-id replay guard.
local Ring = {}
Ring.__index = Ring

--- Creates a bounded set holding at most `capacity` recently seen keys.
function M.ring_set(capacity)
  return setmetatable({ cap = capacity or 512, order = {}, seen = {}, head = 0 }, Ring)
end

--- True when `key` is already present.
function Ring:contains(key)
  return self.seen[key] == true
end

--- Records `key`, evicting the oldest entry when the capacity is exceeded.
--- Returns false when the key was already present (a duplicate).
function Ring:add(key)
  if self.seen[key] then return false end
  self.seen[key] = true
  self.order[#self.order + 1] = key
  while #self.order > self.cap do
    local old = table.remove(self.order, 1)
    self.seen[old] = nil
  end
  return true
end

--- Number of tracked keys.
function Ring:size()
  return #self.order
end

--------------------------------------------------------------------------------
-- Logging with size-bounded rotation
--------------------------------------------------------------------------------

local Logger = {}
Logger.__index = Logger

local LEVELS = { debug = 10, info = 20, warn = 30, error = 40, off = 100 }

--- Creates a logger writing to `<dir>/<name>`, rotating when the file exceeds
--- `max_bytes` and keeping `keep` historical generations.
function M.logger(dir, opts)
  opts = opts or {}
  return setmetatable({
    dir = dir,
    name = opts.name or "bridge.log",
    max_bytes = opts.max_bytes or 1048576,
    keep = opts.keep or 3,
    level = LEVELS[opts.level or "info"] or LEVELS.info,
    console = opts.console or false,
    bytes = 0,
    buffer = {},
  }, Logger)
end

--- Sets the minimum level that is written ("debug"|"info"|"warn"|"error"|"off").
function Logger:set_level(name)
  self.level = LEVELS[name] or self.level
end

function Logger:_path()
  return M.join(self.dir, self.name)
end

function Logger:_rotate_if_needed()
  local path = self:_path()
  local size = M.fs.size(path)
  if size == nil or size < self.max_bytes then return end
  for i = self.keep - 1, 1, -1 do
    local from = path .. "." .. i
    local to = path .. "." .. (i + 1)
    if M.fs.exists(from) then M.fs.rename(from, to) end
  end
  M.fs.rename(path, path .. ".1")
end

--- Appends one line. Failures are swallowed: logging must never break the loop.
function Logger:log(level, msg, ...)
  local lv = LEVELS[level] or LEVELS.info
  if lv < self.level then return end
  local text = msg
  if select("#", ...) > 0 then
    local ok, formatted = pcall(string.format, msg, ...)
    text = ok and formatted or msg
  end
  local line = string.format("%s [%s] %s\n", M.iso8601(M.now()), level:upper(), M.truncate(text, 4000))
  if self.console and reaper and reaper.ShowConsoleMsg then
    reaper.ShowConsoleMsg(line)
  end
  local path = self:_path()
  pcall(function()
    self:_rotate_if_needed()
    if M.fs.append then
      M.fs.append(path, line)
    else
      M.fs.write(path, (M.fs.read(path) or "") .. line)
    end
  end)
end

--- Convenience level helpers.
function Logger:debug(...) self:log("debug", ...) end
function Logger:info(...) self:log("info", ...) end
function Logger:warn(...) self:log("warn", ...) end
function Logger:error(...) self:log("error", ...) end

return M
