--[[
  harness.lua -- tiny assertion library and test-environment builder.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT
--]]

local mock_fs = require("mock_fs")
local mock_reaper = require("mock_reaper")

local H = {}

--------------------------------------------------------------------------------
-- Assertions
--------------------------------------------------------------------------------

local function fmt(v)
  if type(v) == "string" then return string.format("%q", v) end
  return tostring(v)
end

--- Asserts truthiness.
function H.ok(v, msg)
  if not v then
    error(string.format("expected truthy value%s", msg and (": " .. msg) or ""), 2)
  end
  return v
end

--- Asserts falsiness.
function H.falsy(v, msg)
  if v then
    error(string.format("expected falsy value, got %s%s", fmt(v), msg and (": " .. msg) or ""), 2)
  end
  return v
end

--- Asserts equality.
function H.eq(actual, expected, msg)
  if actual ~= expected then
    error(string.format("expected %s, got %s%s", fmt(expected), fmt(actual),
      msg and (": " .. msg) or ""), 2)
  end
  return actual
end

--- Asserts inequality.
function H.ne(actual, unexpected, msg)
  if actual == unexpected then
    error(string.format("expected a value different from %s%s", fmt(unexpected),
      msg and (": " .. msg) or ""), 2)
  end
  return actual
end

--- Asserts approximate float equality.
function H.near(actual, expected, tol, msg)
  tol = tol or 1e-9
  if type(actual) ~= "number" or math.abs(actual - expected) > tol then
    error(string.format("expected %s +/- %s, got %s%s", fmt(expected), fmt(tol), fmt(actual),
      msg and (": " .. msg) or ""), 2)
  end
  return actual
end

--- Asserts a structured error table carries `code`.
function H.err_code(err, code, msg)
  if type(err) ~= "table" then
    error(string.format("expected a structured error with code %s, got %s%s",
      fmt(code), fmt(err), msg and (": " .. msg) or ""), 2)
  end
  if err.code ~= code then
    error(string.format("expected error code %s, got %s (%s)%s", fmt(code), fmt(err.code),
      tostring(err.message), msg and (": " .. msg) or ""), 2)
  end
  return err
end

--- Asserts `substr` appears in `s`.
function H.contains(s, substr, msg)
  if type(s) ~= "string" or not s:find(substr, 1, true) then
    error(string.format("expected %s to contain %s%s", fmt(s), fmt(substr),
      msg and (": " .. msg) or ""), 2)
  end
  return s
end

--- Runs `fn` and asserts it raised.
function H.throws(fn, msg)
  local ok, e = pcall(fn)
  if ok then
    error(string.format("expected an error to be raised%s", msg and (": " .. msg) or ""), 2)
  end
  return e
end

--------------------------------------------------------------------------------
-- Environment
--------------------------------------------------------------------------------

local function fs_adapter(fs)
  return {
    mkdirp = function(p) return fs:mkdirp(p) end,
    exists = function(p) return fs:exists(p) end,
    read = function(p) return fs:read(p) end,
    write = function(p, d) return fs:write(p, d) end,
    append = function(p, d) return fs:append(p, d) end,
    rename = function(a, b) return fs:rename(a, b) end,
    remove = function(p) return fs:remove(p) end,
    listdir = function(p) return fs:listdir(p) end,
    size = function(p) return fs:size(p) end,
    mtime = function(p) return fs:mtime(p) end,
  }
end

H.fs_adapter = fs_adapter

--- Builds an isolated test environment: an in-memory filesystem installed as
--- `util.fs`, a mock REAPER host installed as the global `reaper`, and (unless
--- `opts.no_bridge`) a started bridge instance.
---
--- Returns a table with `fs`, `host`, `reaper`, `bridge`, `script_dir`,
--- `ipc_dir`, `token` and helper methods.
function H.env(opts)
  opts = opts or {}
  local util = require("util")
  local bridge_mod = require("bridge")

  local fs = mock_fs.new(os.time)
  util.fs = fs_adapter(fs)

  local host = mock_reaper.new({
    fs = fs,
    app_version = opts.app_version or "7.22/linux-x86_64",
    project_name = opts.project_name or "MockProject",
    project_path = opts.project_path or "/projects/mock.rpp",
  })
  host:install()

  local script_dir = opts.script_dir or "/reaper/Scripts/QLabs-Reaper-MCP"
  fs:mkdirp(script_dir)

  local token = opts.token or "0123456789abcdef0123456789abcdef"
  local config = {
    instance_token = token,
    ipc_dir = opts.ipc_dir or (script_dir .. "/ipc"),
    poll_interval_ms = 50,
    heartbeat_interval_ms = 1000,
    log_level = opts.log_level or "off",
    console_log = false,
  }

  local env = {
    fs = fs,
    host = host,
    reaper = _G.reaper,
    script_dir = script_dir,
    ipc_dir = config.ipc_dir,
    token = token,
    config = config,
  }

  if not opts.no_bridge then
    local b = bridge_mod.new({ script_dir = script_dir, config = config,
      now_fn = function() return host.time end })
    env.bridge = b
    if not opts.no_start then
      local ok, reason = b:start()
      env.start_ok, env.start_reason = ok, reason
    end
  end

  --- Writes a command file using the mandated `.tmp` -> `.command.json` rename.
  function env:submit(request, request_id)
    local json = require("json")
    request_id = request_id or request.request_id
    local body = json.encode(request, { indent = "  " })
    local dir = self.ipc_dir .. "/commands"
    self.fs:write(dir .. "/" .. request_id .. ".tmp", body)
    self.fs:rename(dir .. "/" .. request_id .. ".tmp", dir .. "/" .. request_id .. ".command.json")
    return request_id
  end

  --- Writes a raw command body (bypassing JSON encoding), same rename dance.
  function env:submit_raw(body, request_id)
    local dir = self.ipc_dir .. "/commands"
    self.fs:write(dir .. "/" .. request_id .. ".tmp", body)
    self.fs:rename(dir .. "/" .. request_id .. ".tmp", dir .. "/" .. request_id .. ".command.json")
    return request_id
  end

  --- Reads and decodes a result envelope.
  function env:result(request_id)
    local json = require("json")
    local raw = self.fs:read(self.ipc_dir .. "/results/" .. request_id .. ".result.json")
    if raw == nil then return nil end
    return (json.decode(raw))
  end

  --- Builds a well-formed request envelope with sensible defaults.
  function env:request(command, payload, over)
    local util2 = require("util")
    local protocol = require("protocol")
    local req = {
      protocol_version = protocol.PROTOCOL_VERSION,
      request_id = "req-" .. tostring(os.clock()):gsub("%.", "") .. tostring(math.random(1e6)),
      instance_token = self.token,
      created_at = util2.iso8601(util2.now()),
      expires_at = util2.iso8601(util2.now() + 60),
      command = command,
      payload = payload or {},
    }
    for k, v in pairs(over or {}) do req[k] = v end
    return req
  end

  --- Submits a request and runs one poll cycle, returning the decoded result.
  function env:roundtrip(command, payload, over)
    local req = self:request(command, payload, over)
    self:submit(req)
    self.bridge:poll_once()
    return self:result(req.request_id), req
  end

  return env
end

return H
