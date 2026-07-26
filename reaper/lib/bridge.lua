--[[
  bridge.lua -- the bridge engine: configuration, single-instance lock,
  heartbeat, the atomic file-IPC state machine, command dispatch and
  stale-file garbage collection.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT

  The engine is deliberately separated from the ReaScript entry point
  (QLabs_Reaper_MCP_Bridge.lua) so the whole state machine can be driven by the
  mock REAPER API and an in-memory filesystem in reaper/tests.

  Every path this module touches is derived from the configured IPC directory.
  A request can influence a filename only through `request_id`, which must match
  `util.ID_PATTERN` and equal the stem of the file it arrived in.
--]]

local util = require("util")
local json = require("json")
local protocol = require("protocol")
local tagging = require("tagging")
local snapshot = require("snapshot")
local transactions = require("transactions")

local M = {}

--- Filename suffixes making up the wire lifecycle.
M.SUFFIX = {
  TMP = ".tmp",
  COMMAND = ".command.json",
  PROCESSING = ".processing.json",
  RESULT_TMP = ".result.tmp",
  RESULT = ".result.json",
  FAILED = ".failed.json",
}

M.HEARTBEAT_FILE = "heartbeat.json"
M.LOCK_FILE = "bridge.lock"
M.CONFIG_FILE = "config.json"

--------------------------------------------------------------------------------
-- Configuration
--------------------------------------------------------------------------------

--- Default configuration. `ipc_dir` defaults to `<script dir>/ipc`.
function M.default_config()
  return {
    instance_token = "",
    ipc_dir = json.null,
    poll_interval_ms = math.floor(protocol.LIMITS.POLL_INTERVAL_SECONDS * 1000),
    heartbeat_interval_ms = math.floor(protocol.LIMITS.HEARTBEAT_INTERVAL_SECONDS * 1000),
    log_level = "info",
    console_log = false,
  }
end

--- Generates a 32 hex-character installation token.
function M.new_token()
  local parts = {}
  for i = 1, 2 do
    local seed = string.format("%s|%d|%d|%s", tagging.new_uuid(), util.now(), i,
      tostring((reaper and reaper.time_precise and reaper.time_precise()) or os.clock()))
    parts[#parts + 1] = string.format("%016x", util.fnv1a64(seed))
  end
  return table.concat(parts)
end

--- Loads `<script_dir>/config.json`, filling in defaults. When the file is
--- absent (or carries no token) a token is minted and the file is written so
--- both sides of the IPC share one installation token.
--- Returns `config, path, created`.
function M.load_config(script_dir)
  local path = util.join(script_dir, M.CONFIG_FILE)
  local cfg = M.default_config()
  local created = false
  local raw = util.fs.read(path)
  if raw then
    local decoded = json.decode(raw)
    if type(decoded) == "table" and not util.is_array(decoded) then
      for k, v in pairs(decoded) do
        if v ~= json.null then cfg[k] = v end
      end
    end
  end
  if type(cfg.instance_token) ~= "string" or cfg.instance_token == "" then
    cfg.instance_token = M.new_token()
    created = true
  end
  if type(cfg.poll_interval_ms) ~= "number" or cfg.poll_interval_ms < 10 then
    cfg.poll_interval_ms = 50
  end
  if type(cfg.heartbeat_interval_ms) ~= "number" or cfg.heartbeat_interval_ms < 200 then
    cfg.heartbeat_interval_ms = 1000
  end
  if created then
    local out = {
      instance_token = cfg.instance_token,
      ipc_dir = cfg.ipc_dir,
      poll_interval_ms = cfg.poll_interval_ms,
      heartbeat_interval_ms = cfg.heartbeat_interval_ms,
      log_level = cfg.log_level,
      console_log = cfg.console_log,
    }
    util.atomic_write(path, json.encode(out, { indent = "  " }) .. "\n")
  end
  return cfg, path, created
end

--------------------------------------------------------------------------------
-- Bridge object
--------------------------------------------------------------------------------

local Bridge = {}
Bridge.__index = Bridge
M.Bridge = Bridge

--- Creates a bridge instance.
---
--- opts:
---   script_dir  -- directory holding the ReaScript and config.json (required)
---   ipc_dir     -- explicit IPC directory override (defaults to config/derived)
---   config      -- pre-loaded config table (skips reading config.json)
---   now_fn      -- monotonic clock, defaults to reaper.time_precise
function M.new(opts)
  opts = opts or {}
  local script_dir = opts.script_dir or "."
  local cfg = opts.config
  local cfg_path, cfg_created
  if cfg == nil then
    cfg, cfg_path, cfg_created = M.load_config(script_dir)
  else
    local merged = M.default_config()
    for k, v in pairs(cfg) do
      if v ~= json.null then merged[k] = v end
    end
    cfg = merged
  end

  local ipc_dir = opts.ipc_dir
  if ipc_dir == nil and type(cfg.ipc_dir) == "string" and cfg.ipc_dir ~= "" then
    ipc_dir = cfg.ipc_dir
  end
  if ipc_dir == nil then
    ipc_dir = util.join(script_dir, "ipc")
  end

  local self = setmetatable({
    script_dir = script_dir,
    ipc_dir = ipc_dir,
    config = cfg,
    config_path = cfg_path,
    config_created = cfg_created or false,
    dirs = {
      commands = util.join(ipc_dir, "commands"),
      processing = util.join(ipc_dir, "processing"),
      results = util.join(ipc_dir, "results"),
      failed = util.join(ipc_dir, "failed"),
      logs = util.join(ipc_dir, "logs"),
    },
    seen = util.ring_set(protocol.LIMITS.SEEN_REQUEST_IDS),
    first_seen = {},
    running = false,
    pid_token = nil,
    started_at = util.now(),
    started_mono = 0,
    requests_processed = 0,
    requests_failed = 0,
    last_poll = -1e9,
    last_heartbeat = -1e9,
    last_gc = -1e9,
    now_fn = opts.now_fn,
    toolbar = nil,
    stop_reason = json.null,
  }, Bridge)

  self.poll_interval = math.max(cfg.poll_interval_ms, 10) / 1000.0
  self.heartbeat_interval = math.max(cfg.heartbeat_interval_ms, 200) / 1000.0
  self.log = util.logger(self.dirs.logs, {
    name = "bridge.log",
    max_bytes = protocol.LIMITS.LOG_MAX_BYTES,
    keep = protocol.LIMITS.LOG_KEEP,
    level = cfg.log_level,
    console = cfg.console_log == true,
  })
  return self
end

--- Monotonic seconds. Prefers `reaper.time_precise`.
function Bridge:mono()
  if self.now_fn then return self.now_fn() end
  if reaper and reaper.time_precise then return reaper.time_precise() end
  return os.clock()
end

--- Creates every IPC subdirectory.
function Bridge:ensure_dirs()
  util.fs.mkdirp(self.ipc_dir)
  for _, d in pairs(self.dirs) do util.fs.mkdirp(d) end
  return true
end

function Bridge:path(which, name)
  return util.join(self.dirs[which], name)
end

--------------------------------------------------------------------------------
-- Single-instance lock
--------------------------------------------------------------------------------

function Bridge:lock_path()
  return util.join(self.ipc_dir, M.LOCK_FILE)
end

function Bridge:heartbeat_path()
  return util.join(self.ipc_dir, M.HEARTBEAT_FILE)
end

--- Reads and decodes the lock file, or nil.
function Bridge:read_lock()
  local raw = util.fs.read(self:lock_path())
  if not raw then return nil end
  local v = json.decode(raw)
  if type(v) ~= "table" or util.is_array(v) then return nil end
  return v
end

--- Attempts to become the single live bridge instance.
---
--- Returns `true` on success, or `false, reason, holder` when a live instance
--- already holds the lock. A lock whose `heartbeat_at` is older than
--- LIMITS.LOCK_STALE_SECONDS is considered stale and is taken over.
function Bridge:acquire_lock()
  local existing = self:read_lock()
  local now = util.now()
  if existing and type(existing.heartbeat_at) == "number" then
    local age = now - existing.heartbeat_at
    if age <= protocol.LIMITS.LOCK_STALE_SECONDS and existing.pid_token ~= self.pid_token then
      return false, string.format(
        "another QLabs bridge instance is live (token %s, heartbeat %ds ago)",
        tostring(existing.pid_token), math.floor(age)), existing
    end
  end
  self.pid_token = self.pid_token or M.new_token()
  self.lock_taken_over = existing ~= nil
  local ok = self:write_lock(now, now)
  if not ok then return false, "could not write the lock file" end
  return true
end

function Bridge:write_lock(acquired_at, heartbeat_at)
  local doc = {
    pid_token = self.pid_token,
    bridge_version = protocol.BRIDGE_VERSION,
    protocol_version = protocol.PROTOCOL_VERSION,
    reaper_version = self.reaper_version or json.null,
    ipc_dir = self.ipc_dir,
    acquired_at = acquired_at or self.lock_acquired_at or util.now(),
    heartbeat_at = heartbeat_at or util.now(),
    stale_after_seconds = protocol.LIMITS.LOCK_STALE_SECONDS,
  }
  doc.acquired_at_iso = util.iso8601(doc.acquired_at)
  doc.heartbeat_at_iso = util.iso8601(doc.heartbeat_at)
  self.lock_acquired_at = doc.acquired_at
  local s = json.encode(doc, { indent = "  " })
  if s == nil then return false end
  return util.atomic_write(self:lock_path(), s .. "\n")
end

--- True when the lock file still names this instance.
function Bridge:owns_lock()
  local l = self:read_lock()
  if l == nil then return false end
  return l.pid_token == self.pid_token
end

--- Removes the lock when it belongs to this instance.
function Bridge:release_lock()
  local l = self:read_lock()
  if l and l.pid_token == self.pid_token then
    util.fs.remove(self:lock_path())
    return true
  end
  return false
end

--------------------------------------------------------------------------------
-- Heartbeat
--------------------------------------------------------------------------------

--- Builds the heartbeat document.
function Bridge:heartbeat_doc(status)
  local proj_uuid, proj_name, proj_path = json.null, json.null, json.null
  if reaper and reaper.EnumProjects then
    local ok, proj, path = pcall(function()
      local p, fn = reaper.EnumProjects(-1, "")
      return p, fn
    end)
    if ok and proj ~= nil then
      proj_path = path or ""
      local uok, uuid = pcall(tagging.get_proj_state, proj, tagging.PROJ_KEYS.PROJECT_UUID)
      if uok and uuid then proj_uuid = uuid end
      local nok, name = pcall(snapshot.project_name, proj, path)
      if nok and name then proj_name = name end
    end
  end
  local now = util.now()
  return {
    protocol_version = protocol.PROTOCOL_VERSION,
    bridge_version = protocol.BRIDGE_VERSION,
    bridge_schema_version = protocol.BRIDGE_SCHEMA_VERSION,
    reaper_version = self.reaper_version or json.null,
    pid_token = self.pid_token or json.null,
    project_uuid = proj_uuid,
    project_name = proj_name,
    project_path = proj_path,
    ipc_dir = self.ipc_dir,
    status = status or "online",
    timestamp = now,
    timestamp_iso = util.iso8601(now),
    uptime_seconds = math.max(0, self:mono() - self.started_mono),
    requests_processed = self.requests_processed,
    requests_failed = self.requests_failed,
    poll_interval_ms = math.floor(self.poll_interval * 1000),
    heartbeat_interval_ms = math.floor(self.heartbeat_interval * 1000),
    stale_after_seconds = protocol.LIMITS.LOCK_STALE_SECONDS,
    commands = json.array(protocol.COMMAND_NAMES),
  }
end

--- Writes heartbeat.json atomically and refreshes the lock timestamp.
function Bridge:write_heartbeat(status)
  local doc = self:heartbeat_doc(status)
  local s = json.encode(doc, { indent = "  " })
  if s == nil then return false end
  local ok = util.atomic_write(self:heartbeat_path(), s .. "\n")
  self:write_lock(self.lock_acquired_at, doc.timestamp)
  return ok
end

--------------------------------------------------------------------------------
-- File age tracking
--------------------------------------------------------------------------------

--- Age of a file in seconds. Uses a real mtime when the host can supply one,
--- otherwise falls back to when this process first observed the file.
function Bridge:file_age(path)
  local mt = util.fs.mtime and util.fs.mtime(path)
  if type(mt) == "number" and mt > 0 then
    return math.max(0, util.now() - mt)
  end
  local seen = self.first_seen[path]
  if seen == nil then
    self.first_seen[path] = util.now()
    return 0
  end
  return math.max(0, util.now() - seen)
end

--------------------------------------------------------------------------------
-- IPC state machine
--------------------------------------------------------------------------------

local function ends_with(s, suffix)
  return #s >= #suffix and s:sub(- #suffix) == suffix
end

--- True when a directory entry name is safe to join onto a directory path.
local function safe_entry(name)
  if type(name) ~= "string" or name == "" then return false end
  if name:find("[/\\]") then return false end
  if name == "." or name == ".." then return false end
  if name:find("%z") then return false end
  return true
end

--- Lists pending command files, oldest-name-first for determinism.
--- `.tmp` files are never returned: the bridge must never read a partial write.
function Bridge:list_commands()
  local out = {}
  local entries = util.fs.listdir(self.dirs.commands)
  for i = 1, #entries do
    local name = entries[i]
    if safe_entry(name) and ends_with(name, M.SUFFIX.COMMAND) then
      out[#out + 1] = name
    end
  end
  table.sort(out)
  return out
end

--- Writes the result envelope using the mandated `.result.tmp` -> `.result.json`
--- rename. Oversized results are replaced by a RESULT_TOO_LARGE error envelope.
function Bridge:write_result(request_id, envelope)
  local body = json.encode(envelope, { indent = "  " })
  if body == nil then
    envelope = protocol.make_result({
      request_id = request_id,
      command = envelope.command,
      error = protocol.err(protocol.ERR.INTERNAL_BRIDGE_ERROR, "result could not be encoded as JSON"),
      reaper_version = self.reaper_version,
    })
    body = json.encode(envelope, { indent = "  " }) or "{}"
  end
  body = body .. "\n"
  if #body > protocol.LIMITS.MAX_RESULT_BYTES then
    local trimmed = protocol.make_result({
      request_id = request_id,
      command = envelope.command,
      error = protocol.err(protocol.ERR.RESULT_TOO_LARGE,
        string.format("result is %d bytes; limit is %d", #body, protocol.LIMITS.MAX_RESULT_BYTES),
        { size = #body, limit = protocol.LIMITS.MAX_RESULT_BYTES }),
      reaper_version = self.reaper_version,
    })
    body = (json.encode(trimmed, { indent = "  " }) or "{}") .. "\n"
  end
  local tmp = self:path("results", request_id .. M.SUFFIX.RESULT_TMP)
  local final = self:path("results", request_id .. M.SUFFIX.RESULT)
  local ok, err = util.atomic_write_pair(tmp, final, body)
  if not ok then
    self.log:error("failed to write result for %s: %s", request_id, tostring(err))
  end
  return ok
end

--- Claims and processes one command file. Returns true when a file was handled.
function Bridge:process_file(name)
  local stem = name:sub(1, #name - #M.SUFFIX.COMMAND)
  local cmd_path = self:path("commands", name)

  -- A request id that cannot be a safe filename is quarantined immediately: it
  -- would otherwise dictate the result path.
  if not util.is_safe_id(stem, protocol.LIMITS.MAX_REQUEST_ID_LEN) then
    self.log:warn("quarantining command file with unusable name: %s", name)
    util.fs.rename(cmd_path, self:path("failed", "unnamed-" .. tostring(util.now()) .. M.SUFFIX.FAILED))
    return true
  end

  local proc_path = self:path("processing", stem .. M.SUFFIX.PROCESSING)
  -- Atomic claim: exactly one bridge instance can win this rename.
  local claimed = util.fs.rename(cmd_path, proc_path)
  if not claimed then
    self.log:debug("could not claim %s (already claimed or removed)", name)
    return false
  end

  local started_mono = self:mono()
  local started_at = util.iso8601(util.now())
  local raw = util.fs.read(proc_path)
  local size = raw and #raw or 0

  local envelope
  local req, verr

  if raw == nil then
    verr = protocol.err(protocol.ERR.INTERNAL_BRIDGE_ERROR, "claimed command file could not be read")
  else
    local decoded, derr = json.decode(raw)
    if decoded == nil then
      verr = protocol.err(protocol.ERR.MALFORMED_REQUEST, tostring(derr), { bytes = size })
      if size > protocol.LIMITS.MAX_REQUEST_BYTES then
        verr = protocol.err(protocol.ERR.PAYLOAD_TOO_LARGE,
          string.format("request is %d bytes; limit is %d", size, protocol.LIMITS.MAX_REQUEST_BYTES),
          { size = size, limit = protocol.LIMITS.MAX_REQUEST_BYTES })
      end
    else
      req, verr = protocol.validate_envelope(decoded, {
        instance_token = self.config.instance_token,
        now = util.now(),
        request_id_hint = stem,
        seen = self.seen,
        raw_size = size,
      })
    end
  end

  local result, err, transaction_id
  if req then
    self.seen:add(req.request_id)
    result, err, transaction_id = self:dispatch(req)
  else
    err = verr
  end

  envelope = protocol.make_result({
    request_id = stem,
    command = req and req.command or nil,
    result = result,
    error = err,
    transaction_id = transaction_id,
    reaper_version = self.reaper_version,
    started_at = started_at,
    duration_ms = (self:mono() - started_mono) * 1000.0,
  })

  self:write_result(stem, envelope)

  if err then
    self.requests_failed = self.requests_failed + 1
    self.log:warn("request %s (%s) failed: %s %s", stem,
      tostring(req and req.command or "?"), tostring(err.code), tostring(err.message))
    if not util.fs.rename(proc_path, self:path("failed", stem .. M.SUFFIX.FAILED)) then
      util.fs.remove(proc_path)
    end
  else
    self.requests_processed = self.requests_processed + 1
    self.log:info("request %s (%s) ok in %.1fms", stem, tostring(req.command),
      (self:mono() - started_mono) * 1000.0)
    util.fs.remove(proc_path)
  end
  self.first_seen[cmd_path] = nil
  self.first_seen[proc_path] = nil
  return true
end

--- Scans the commands directory and processes up to
--- LIMITS.MAX_COMMANDS_PER_TICK files. Returns the number processed.
function Bridge:poll_once()
  local names = self:list_commands()
  local n = 0
  for i = 1, #names do
    if n >= protocol.LIMITS.MAX_COMMANDS_PER_TICK then break end
    local ok, handled = xpcall(self.process_file, function(e)
      return debug.traceback(tostring(e), 2)
    end, self, names[i])
    if not ok then
      self.log:error("unhandled error processing %s: %s", names[i], tostring(handled))
    elseif handled then
      n = n + 1
    end
  end
  return n
end

--------------------------------------------------------------------------------
-- Command dispatch
--------------------------------------------------------------------------------

local handlers = {}

--- Extracts the extraction options shared by inspection and staleness checks.
local function extraction_opts(payload, reaper_version)
  local me = protocol.opt_table(payload, "melody_extraction") or {}
  local channel = protocol.opt_number(me, "channel")
  return {
    source_mode = protocol.opt_string(payload, "source_mode") or "auto",
    note_scope = protocol.opt_string(payload, "note_scope") or "selected_or_all",
    extraction_mode = protocol.opt_string(me, "mode") or "auto",
    extraction_channel = channel and math.tointeger(channel) or nil,
    reaper_version = reaper_version,
  }
end

handlers.ping = function(self, _req)
  return {
    pong = true,
    bridge_version = protocol.BRIDGE_VERSION,
    protocol_version = protocol.PROTOCOL_VERSION,
    server_time = util.iso8601(util.now()),
    uptime_seconds = math.max(0, self:mono() - self.started_mono),
  }
end

handlers.status = function(self, _req)
  local hb = self:heartbeat_doc("online")
  local proj = nil
  if reaper.EnumProjects then proj = (reaper.EnumProjects(-1, "")) end
  local play_state, selected_items, editor_open = json.null, 0, false
  local knowledge_version = json.null
  if proj ~= nil then
    if reaper.GetPlayStateEx then play_state = reaper.GetPlayStateEx(proj) end
    selected_items = reaper.CountSelectedMediaItems(proj) or 0
    local kv = tagging.get_proj_state(proj, tagging.PROJ_KEYS.KNOWLEDGE_VERSION)
    if kv then knowledge_version = kv end
  end
  if reaper.MIDIEditor_GetActive and reaper.MIDIEditor_GetActive() ~= nil then
    editor_open = true
  end
  return {
    bridge_connected = true,
    bridge_version = protocol.BRIDGE_VERSION,
    bridge_schema_version = protocol.BRIDGE_SCHEMA_VERSION,
    ipc_protocol_version = protocol.PROTOCOL_VERSION,
    reaper_version = self.reaper_version or json.null,
    heartbeat_age_seconds = 0,
    heartbeat_timestamp = hb.timestamp,
    active_project = proj ~= nil,
    project_uuid = hb.project_uuid,
    project_name = hb.project_name,
    project_path = hb.project_path,
    play_state = play_state,
    selected_item_count = selected_items,
    active_midi_editor = editor_open,
    knowledge_version = knowledge_version,
    uptime_seconds = hb.uptime_seconds,
    requests_processed = self.requests_processed,
    requests_failed = self.requests_failed,
    ipc_dir = self.ipc_dir,
    commands = json.array(protocol.COMMAND_NAMES),
    limits = protocol.LIMITS,
  }
end

handlers.inspect_selection = function(self, req, proj)
  local opts = extraction_opts(req.payload, self.reaper_version)
  local resolved, cerr = snapshot.check_expected_project(proj, req.expected_project, opts)
  if not resolved then return nil, cerr end
  local snap, serr = snapshot.build(proj, opts)
  if not snap then return nil, serr end
  local wire = snapshot.to_wire(snap)
  wire.tempo_at_item_start = (function()
    local markers = snap.tempo_markers
    local bpm = markers[1] and markers[1].bpm or 120.0
    for i = 1, #markers do
      if markers[i].qn <= snap.item_position_qn + 1e-9 then bpm = markers[i].bpm end
    end
    return bpm
  end)()
  local num, den = 4, 4
  if reaper.TimeMap_GetTimeSigAtTime then
    local a, b = reaper.TimeMap_GetTimeSigAtTime(proj, snap.item_position_seconds)
    num = math.tointeger(a) or 4
    den = math.tointeger(b) or 4
  end
  wire.time_signature_at_item_start = { numerator = num, denominator = den }
  return wire
end

handlers.stage_candidate = function(self, req, proj)
  local plan = protocol.opt_table(req.payload, "plan")
  if plan == nil then
    return nil, protocol.err(protocol.ERR.INVALID_EDIT_PLAN, "payload.plan is required")
  end
  local ctx = extraction_opts(req.payload, self.reaper_version)
  ctx.verify_snapshot = protocol.opt_bool(req.payload, "verify_snapshot")
  local res, err = transactions.stage(proj, plan, ctx)
  if not res then
    return nil, err, (type(plan) == "table" and type(plan.transaction_id) == "string")
      and plan.transaction_id or nil
  end
  return res, nil, res.transaction_id
end

handlers.commit_candidate = function(self, req, proj)
  local tx = protocol.opt_string(req.payload, "transaction_id")
  if tx == nil then
    return nil, protocol.err(protocol.ERR.MALFORMED_REQUEST, "payload.transaction_id is required")
  end
  local label = protocol.opt_string(req.payload, "undo_label")
  if label ~= nil and label:sub(1, #transactions.UNDO_PREFIX) ~= transactions.UNDO_PREFIX then
    return nil, protocol.err(protocol.ERR.MALFORMED_REQUEST,
      string.format("undo_label must begin with %q", transactions.UNDO_PREFIX))
  end
  local res, err = transactions.commit(proj, tx, { undo_label = label })
  if not res then return nil, err, tx end
  return res, nil, tx
end

handlers.discard_candidate = function(self, req, proj)
  local tx = protocol.opt_string(req.payload, "transaction_id")
  if tx == nil then
    return nil, protocol.err(protocol.ERR.MALFORMED_REQUEST, "payload.transaction_id is required")
  end
  local label = protocol.opt_string(req.payload, "undo_label")
  if label ~= nil and label:sub(1, #transactions.UNDO_PREFIX) ~= transactions.UNDO_PREFIX then
    return nil, protocol.err(protocol.ERR.MALFORMED_REQUEST,
      string.format("undo_label must begin with %q", transactions.UNDO_PREFIX))
  end
  local res, err = transactions.discard(proj, tx, { undo_label = label })
  if not res then return nil, err, tx end
  return res, nil, tx
end

handlers.undo_last_generation = function(self, req, proj)
  local tx = protocol.opt_string(req.payload, "transaction_id")
  local res, err = transactions.undo_last(proj, { transaction_id = tx })
  if not res then return nil, err, tx end
  return res, nil, res.transaction_id
end

M.handlers = handlers

--- Executes one validated request. Every path is guarded so a Lua error becomes
--- INTERNAL_BRIDGE_ERROR rather than an exception escaping into the defer loop.
--- Returns `result, err, transaction_id`.
function Bridge:dispatch(req)
  local handler = handlers[req.command]
  if handler == nil then
    return nil, protocol.err(protocol.ERR.UNKNOWN_COMMAND,
      string.format("command %q has no handler", req.command))
  end

  local proj
  if req.spec.needs_project then
    local verr = protocol.check_reaper_version(self.reaper_version)
    if verr then return nil, verr end
    local p, perr = snapshot.active_project()
    if not p then return nil, perr end
    proj = p
  end

  local ok, a, b, c = xpcall(handler, function(e)
    if type(e) == "table" and e.code then
      e.traceback = debug.traceback("", 2)
      return e
    end
    return protocol.err(protocol.ERR.INTERNAL_BRIDGE_ERROR, tostring(e),
      { traceback = debug.traceback("", 2) })
  end, self, req, proj)

  if not ok then
    -- `a` holds the structured error produced by the message handler.
    return nil, a
  end
  return a, b, c
end

--------------------------------------------------------------------------------
-- Garbage collection
--------------------------------------------------------------------------------

local GC_RULES = {
  { dir = "commands", suffix = M.SUFFIX.TMP, limit = "STALE_TMP_SECONDS", action = "remove" },
  { dir = "commands", suffix = M.SUFFIX.COMMAND, limit = "STALE_COMMAND_SECONDS", action = "remove" },
  { dir = "processing", suffix = M.SUFFIX.PROCESSING, limit = "STALE_PROCESSING_SECONDS", action = "fail" },
  { dir = "results", suffix = M.SUFFIX.RESULT_TMP, limit = "STALE_TMP_SECONDS", action = "remove" },
  { dir = "results", suffix = M.SUFFIX.RESULT, limit = "STALE_RESULT_SECONDS", action = "remove" },
  { dir = "failed", suffix = M.SUFFIX.FAILED, limit = "STALE_FAILED_SECONDS", action = "remove" },
}

--- Removes aged IPC artefacts. Files that do not carry one of the protocol's
--- own suffixes are left completely untouched.
function Bridge:gc()
  local removed, failed_out = 0, 0
  local live = {}
  for i = 1, #GC_RULES do
    local rule = GC_RULES[i]
    local dir = self.dirs[rule.dir]
    local entries = util.fs.listdir(dir)
    for j = 1, #entries do
      local name = entries[j]
      if safe_entry(name) and ends_with(name, rule.suffix) then
        local path = util.join(dir, name)
        live[path] = true
        local age = self:file_age(path)
        if age > protocol.LIMITS[rule.limit] then
          if rule.action == "fail" then
            local stem = name:sub(1, #name - #rule.suffix)
            if util.is_safe_id(stem, protocol.LIMITS.MAX_REQUEST_ID_LEN) then
              util.fs.rename(path, self:path("failed", stem .. M.SUFFIX.FAILED))
            else
              util.fs.remove(path)
            end
            failed_out = failed_out + 1
          else
            util.fs.remove(path)
            removed = removed + 1
          end
          live[path] = nil
          self.first_seen[path] = nil
        end
      end
    end
  end
  -- Prune first-seen entries for files that are gone, so the table stays bounded.
  for path in pairs(self.first_seen) do
    if not live[path] then self.first_seen[path] = nil end
  end
  if removed > 0 or failed_out > 0 then
    self.log:info("gc removed %d stale file(s), quarantined %d", removed, failed_out)
  end
  return removed, failed_out
end

--------------------------------------------------------------------------------
-- Lifecycle
--------------------------------------------------------------------------------

--- Prepares directories, acquires the lock and writes the first heartbeat.
--- Returns true, or false plus a human-readable reason.
function Bridge:start()
  self.started_mono = self:mono()
  self.started_at = util.now()
  if reaper and reaper.GetAppVersion then
    self.reaper_version = reaper.GetAppVersion()
  end
  self:ensure_dirs()

  local ok, reason, holder = self:acquire_lock()
  if not ok then
    self.log:warn("refusing to start: %s", tostring(reason))
    return false, reason, holder
  end
  self.running = true
  self:write_heartbeat("online")
  self:gc()
  self.last_gc = self:mono()
  self.last_heartbeat = self:mono()
  self.log:info("bridge %s online; ipc_dir=%s reaper=%s", protocol.BRIDGE_VERSION,
    self.ipc_dir, tostring(self.reaper_version))
  if self.config_created then
    self.log:warn("a new installation token was generated in %s; the MCP server must use it",
      tostring(self.config_path))
  end
  return true
end

--- One iteration of the polling loop. Safe to call at REAPER's defer rate: the
--- directory scan is gated to at most one per `poll_interval` seconds.
function Bridge:tick()
  if not self.running then return false end
  local now = self:mono()

  if now - self.last_poll >= self.poll_interval then
    self.last_poll = now
    self:poll_once()
  end

  if now - self.last_heartbeat >= self.heartbeat_interval then
    self.last_heartbeat = now
    -- Ownership is checked BEFORE the heartbeat write, because the write also
    -- refreshes the lock and would otherwise silently reclaim it.
    if not self:owns_lock() then
      self.log:warn("another instance took the lock; shutting this one down")
      self.stop_reason = "lock_lost"
      self.running = false
      pcall(function() self:write_heartbeat("offline") end)
      return false
    end
    self:write_heartbeat("online")
  end

  if now - self.last_gc >= protocol.LIMITS.GC_INTERVAL_SECONDS then
    self.last_gc = now
    self:gc()
  end

  return true
end

--- Marks the bridge offline, writes a final heartbeat and drops the lock.
function Bridge:stop(reason)
  if not self.running and self.stopped then return false end
  self.running = false
  self.stopped = true
  self.stop_reason = reason or self.stop_reason or "atexit"
  pcall(function() self:write_heartbeat("offline") end)
  pcall(function() self:release_lock() end)
  self.log:info("bridge offline (%s); processed=%d failed=%d",
    tostring(self.stop_reason), self.requests_processed, self.requests_failed)
  return true
end

return M
