--[[
  protocol.lua -- IPC envelope validation, command allowlist, versions, limits
  and the canonical structured error codes.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT

  This module is the single authority for what the bridge will accept. Anything
  not explicitly permitted here is rejected before a single REAPER API call is
  made. Keep it in lockstep with docs/IPC_WIRE.md and schemas/ipc-*.schema.json.
--]]

local util = require("util")
local json = require("json")

local M = {}

--------------------------------------------------------------------------------
-- Versions
--------------------------------------------------------------------------------

--- Wire protocol identifier. Both sides must match exactly.
M.PROTOCOL_VERSION = "qlabs-reaper-ipc/1"

--- Semantic version of this bridge implementation.
M.BRIDGE_VERSION = "1.0.0"

--- Version of the project-level extended-state layout the bridge writes.
M.BRIDGE_SCHEMA_VERSION = "1"

--- Minimum REAPER major version supported.
M.MIN_REAPER_MAJOR = 6

--- Project extended-state section used for all bridge-owned project state.
M.EXT_SECTION = "QLABS_MCP"

--------------------------------------------------------------------------------
-- Limits (every value here is normative -- mirror in IPC_WIRE.md)
--------------------------------------------------------------------------------

M.LIMITS = {
  --- Maximum size in bytes of a `<id>.command.json` file.
  MAX_REQUEST_BYTES = 1048576,
  --- Maximum size in bytes of a `<id>.result.json` file the bridge will write.
  MAX_RESULT_BYTES = 8388608,
  --- Maximum length of the `request_id` string / filename stem.
  MAX_REQUEST_ID_LEN = 128,
  --- Maximum length of any identifier field (transaction id, plan id, ...).
  MAX_ID_LEN = 128,
  --- Maximum length of a free-form string field (labels, names, tag values).
  MAX_STRING_LEN = 512,
  --- Maximum notes the bridge will insert across one edit plan.
  MAX_GENERATED_NOTES = 20000,
  --- Maximum notes the bridge will insert into a single MIDI item.
  MAX_NOTES_PER_ITEM = 8000,
  --- Maximum tracks one stage operation may create.
  MAX_TRACKS_PER_STAGE = 32,
  --- Maximum MIDI items one stage operation may create.
  MAX_ITEMS_PER_STAGE = 64,
  --- Maximum regions one stage operation may create.
  MAX_REGIONS_PER_STAGE = 16,
  --- Maximum MIDI sends one stage operation may create.
  MAX_SENDS_PER_STAGE = 16,
  --- Maximum operations in one edit plan.
  MAX_OPERATIONS = 512,
  --- Maximum preconditions in one edit plan.
  MAX_PRECONDITIONS = 64,
  --- Maximum expected outputs in one edit plan.
  MAX_EXPECTED_OUTPUTS = 128,
  --- Maximum notes the bridge will read out of a source take in one inspection.
  MAX_READ_NOTES = 100000,
  --- Maximum ownership tags attachable to one object.
  MAX_TAGS_PER_OBJECT = 16,
  --- Bounded replay guard: number of recently seen request ids retained.
  SEEN_REQUEST_IDS = 512,
  --- Minimum seconds between directory scans.
  POLL_INTERVAL_SECONDS = 0.05,
  --- Maximum command files claimed per defer tick.
  MAX_COMMANDS_PER_TICK = 4,
  --- Seconds between heartbeat writes.
  HEARTBEAT_INTERVAL_SECONDS = 1.0,
  --- A lock whose heartbeat is older than this is stale and may be taken over.
  LOCK_STALE_SECONDS = 10,
  --- Seconds between stale-file garbage collection sweeps.
  GC_INTERVAL_SECONDS = 30,
  --- Age after which an unclaimed command file is discarded.
  STALE_COMMAND_SECONDS = 300,
  --- Age after which an abandoned `.processing.json` file is failed out.
  STALE_PROCESSING_SECONDS = 120,
  --- Age after which an uncollected result file is deleted.
  STALE_RESULT_SECONDS = 900,
  --- Age after which a stray `.tmp` file is deleted.
  STALE_TMP_SECONDS = 60,
  --- Age after which a `failed/` artefact is deleted.
  STALE_FAILED_SECONDS = 86400,
  --- Maximum log file size before rotation.
  LOG_MAX_BYTES = 1048576,
  --- Rotated log generations retained.
  LOG_KEEP = 3,
  --- Clock skew tolerated when evaluating `expires_at`, in seconds.
  CLOCK_SKEW_SECONDS = 5,
}

--------------------------------------------------------------------------------
-- Error codes
--------------------------------------------------------------------------------

--- Every structured error code in the protocol. Values are their own names so a
--- typo at a call site is a nil index rather than a silently wrong wire code.
--- Codes marked CLIENT are produced by the Rust side only; they are declared
--- here so both sides share one vocabulary.
M.ERR = {
  -- Transport / liveness -------------------------------------------------
  BRIDGE_OFFLINE = "BRIDGE_OFFLINE",                 -- CLIENT: no fresh heartbeat
  IPC_TIMEOUT = "IPC_TIMEOUT",                       -- CLIENT: no result in time
  BRIDGE_VERSION_MISMATCH = "BRIDGE_VERSION_MISMATCH",
  IPC_PROTOCOL_MISMATCH = "IPC_PROTOCOL_MISMATCH",
  INVALID_INSTANCE_TOKEN = "INVALID_INSTANCE_TOKEN",
  EXPIRED_REQUEST = "EXPIRED_REQUEST",
  PAYLOAD_TOO_LARGE = "PAYLOAD_TOO_LARGE",
  -- Project / source resolution -----------------------------------------
  NO_ACTIVE_PROJECT = "NO_ACTIVE_PROJECT",
  NO_MIDI_SOURCE = "NO_MIDI_SOURCE",
  MULTIPLE_MIDI_SOURCES = "MULTIPLE_MIDI_SOURCES",
  AMBIGUOUS_MELODY = "AMBIGUOUS_MELODY",
  SOURCE_ITEM_MISSING = "SOURCE_ITEM_MISSING",
  SOURCE_TAKE_MISSING = "SOURCE_TAKE_MISSING",
  -- Staleness ------------------------------------------------------------
  STALE_SNAPSHOT = "STALE_SNAPSHOT",
  PROJECT_CHANGED = "PROJECT_CHANGED",
  MIDI_CHANGED = "MIDI_CHANGED",
  TEMPO_MAP_CHANGED = "TEMPO_MAP_CHANGED",
  -- Plans / knowledge ----------------------------------------------------
  INVALID_EDIT_PLAN = "INVALID_EDIT_PLAN",
  KNOWLEDGE_INVALID = "KNOWLEDGE_INVALID",
  -- Host / undo ----------------------------------------------------------
  UNSUPPORTED_REAPER_VERSION = "UNSUPPORTED_REAPER_VERSION",
  UNDO_NOT_OWNED = "UNDO_NOT_OWNED",
  INTERNAL_BRIDGE_ERROR = "INTERNAL_BRIDGE_ERROR",
  -- Extensions beyond brief section 19 (documented in IPC_WIRE.md) --------
  MALFORMED_REQUEST = "MALFORMED_REQUEST",
  UNKNOWN_COMMAND = "UNKNOWN_COMMAND",
  DUPLICATE_REQUEST = "DUPLICATE_REQUEST",
  RESULT_TOO_LARGE = "RESULT_TOO_LARGE",
  TRANSACTION_NOT_FOUND = "TRANSACTION_NOT_FOUND",
}

--- Ordered list of all codes, used by tests and documentation generation.
M.ERROR_CODES = (function()
  local ks = {}
  for k in pairs(M.ERR) do ks[#ks + 1] = k end
  table.sort(ks)
  return ks
end)()

--- Builds a structured error table.
function M.err(code, message, details)
  return {
    code = code or M.ERR.INTERNAL_BRIDGE_ERROR,
    message = util.truncate(tostring(message or code or "unspecified error"), 2000),
    details = details or json.object({}),
  }
end

--------------------------------------------------------------------------------
-- Command allowlist
--------------------------------------------------------------------------------

--- The complete set of commands the bridge will execute. There is deliberately
--- no command that evaluates Lua, runs an action id, or touches an arbitrary
--- path. Adding an entry here is the only way to widen the bridge's authority.
---
--- needs_project -- an active project must exist
--- writes        -- the command opens an undo block and mutates the project
M.COMMANDS = {
  ping                 = { needs_project = false, writes = false },
  status               = { needs_project = false, writes = false },
  inspect_selection    = { needs_project = true,  writes = false },
  stage_candidate      = { needs_project = true,  writes = true  },
  commit_candidate     = { needs_project = true,  writes = true  },
  discard_candidate    = { needs_project = true,  writes = true  },
  undo_last_generation = { needs_project = true,  writes = true  },
}

--- Sorted array of allowlisted command names.
M.COMMAND_NAMES = (function()
  local ks = {}
  for k in pairs(M.COMMANDS) do ks[#ks + 1] = k end
  table.sort(ks)
  return ks
end)()

--- True when `name` is allowlisted.
function M.is_allowed_command(name)
  return type(name) == "string" and M.COMMANDS[name] ~= nil
end

--------------------------------------------------------------------------------
-- Field helpers
--------------------------------------------------------------------------------

--- Reads a string field, treating JSON null and absence identically.
function M.opt_string(obj, key)
  local v = obj and obj[key]
  if v == nil or v == json.null then return nil end
  if type(v) ~= "string" then return nil, key .. " must be a string" end
  return v
end

--- Reads a number field, treating JSON null and absence identically.
function M.opt_number(obj, key)
  local v = obj and obj[key]
  if v == nil or v == json.null then return nil end
  if type(v) ~= "number" or not util.is_finite(v) then
    return nil, key .. " must be a finite number"
  end
  return v
end

--- Reads a boolean field, treating JSON null and absence identically.
function M.opt_bool(obj, key)
  local v = obj and obj[key]
  if v == nil or v == json.null then return nil end
  if type(v) ~= "boolean" then return nil, key .. " must be a boolean" end
  return v
end

--- Reads a table field, treating JSON null and absence identically.
function M.opt_table(obj, key)
  local v = obj and obj[key]
  if v == nil or v == json.null then return nil end
  if type(v) ~= "table" then return nil, key .. " must be an object or array" end
  return v
end

--------------------------------------------------------------------------------
-- Envelope validation
--------------------------------------------------------------------------------

--- Validates a decoded request envelope.
---
--- `ctx` fields:
---   instance_token   -- the configured installation token (string)
---   now              -- current Unix time in seconds
---   request_id_hint  -- filename stem the envelope must agree with
---   seen             -- util.ring_set of recently processed request ids
---   raw_size         -- byte size of the request file
---
--- Returns `request` on success, or `nil, err` where `err` is a structured
--- error table. The order of checks is normative: size, shape, protocol
--- version, bridge version, token, id, replay, expiry, command.
function M.validate_envelope(req, ctx)
  local L = M.LIMITS

  if ctx.raw_size and ctx.raw_size > L.MAX_REQUEST_BYTES then
    return nil, M.err(M.ERR.PAYLOAD_TOO_LARGE,
      string.format("request is %d bytes; limit is %d", ctx.raw_size, L.MAX_REQUEST_BYTES),
      { size = ctx.raw_size, limit = L.MAX_REQUEST_BYTES })
  end

  if type(req) ~= "table" or util.is_array(req) then
    return nil, M.err(M.ERR.MALFORMED_REQUEST, "request envelope must be a JSON object")
  end

  local pv = M.opt_string(req, "protocol_version")
  if pv == nil then
    return nil, M.err(M.ERR.IPC_PROTOCOL_MISMATCH, "missing protocol_version")
  end
  if pv ~= M.PROTOCOL_VERSION then
    return nil, M.err(M.ERR.IPC_PROTOCOL_MISMATCH,
      string.format("protocol_version %q is not %q", pv, M.PROTOCOL_VERSION),
      { expected = M.PROTOCOL_VERSION, received = pv })
  end

  local want_bridge = M.opt_string(req, "require_bridge_version")
  if want_bridge ~= nil and want_bridge ~= M.BRIDGE_VERSION then
    return nil, M.err(M.ERR.BRIDGE_VERSION_MISMATCH,
      string.format("bridge is %s; caller requires %s", M.BRIDGE_VERSION, want_bridge),
      { bridge_version = M.BRIDGE_VERSION, required = want_bridge })
  end

  local token = M.opt_string(req, "instance_token")
  if token == nil or token == "" then
    return nil, M.err(M.ERR.INVALID_INSTANCE_TOKEN, "missing instance_token")
  end
  if ctx.instance_token == nil or ctx.instance_token == "" then
    return nil, M.err(M.ERR.INVALID_INSTANCE_TOKEN, "bridge has no configured instance token")
  end
  if #token ~= #ctx.instance_token then
    return nil, M.err(M.ERR.INVALID_INSTANCE_TOKEN, "instance_token does not match this installation")
  end
  -- Length-independent comparison; the token is not a secret defence but there
  -- is no reason to leak a prefix match either.
  local diff = 0
  for i = 1, #token do
    diff = diff | (string.byte(token, i) ~ string.byte(ctx.instance_token, i))
  end
  if diff ~= 0 then
    return nil, M.err(M.ERR.INVALID_INSTANCE_TOKEN, "instance_token does not match this installation")
  end

  local rid = M.opt_string(req, "request_id")
  if rid == nil or not util.is_safe_id(rid, L.MAX_REQUEST_ID_LEN) then
    return nil, M.err(M.ERR.MALFORMED_REQUEST,
      "request_id must match " .. util.ID_PATTERN .. " and be 1-" .. L.MAX_REQUEST_ID_LEN .. " bytes")
  end
  if ctx.request_id_hint and rid ~= ctx.request_id_hint then
    return nil, M.err(M.ERR.MALFORMED_REQUEST,
      string.format("request_id %q does not match filename stem %q", rid, ctx.request_id_hint))
  end

  if ctx.seen and ctx.seen:contains(rid) then
    return nil, M.err(M.ERR.DUPLICATE_REQUEST,
      string.format("request_id %q was already processed", rid), { request_id = rid })
  end

  local expires = M.opt_string(req, "expires_at")
  if expires == nil then
    return nil, M.err(M.ERR.MALFORMED_REQUEST, "missing expires_at")
  end
  local exp = util.parse_iso8601(expires)
  if exp == nil then
    return nil, M.err(M.ERR.MALFORMED_REQUEST, "expires_at is not a supported ISO-8601 timestamp")
  end
  local now = ctx.now or util.now()
  if now > exp + L.CLOCK_SKEW_SECONDS then
    return nil, M.err(M.ERR.EXPIRED_REQUEST,
      string.format("request expired at %s (now %s)", expires, util.iso8601(now)),
      { expires_at = expires, now = util.iso8601(now) })
  end

  local created = M.opt_string(req, "created_at")
  if created ~= nil and util.parse_iso8601(created) == nil then
    return nil, M.err(M.ERR.MALFORMED_REQUEST, "created_at is not a supported ISO-8601 timestamp")
  end

  local cmd = M.opt_string(req, "command")
  if cmd == nil then
    return nil, M.err(M.ERR.MALFORMED_REQUEST, "missing command")
  end
  if not M.is_allowed_command(cmd) then
    return nil, M.err(M.ERR.UNKNOWN_COMMAND,
      string.format("command %q is not allowlisted", util.truncate(cmd, 64)),
      { allowed = M.COMMAND_NAMES })
  end

  local payload = req.payload
  if payload == nil or payload == json.null then
    payload = json.object({})
  elseif type(payload) ~= "table" or util.is_array(payload) then
    return nil, M.err(M.ERR.MALFORMED_REQUEST, "payload must be a JSON object")
  end

  local expected = req.expected_project
  if expected == json.null then expected = nil end
  if expected ~= nil and (type(expected) ~= "table" or util.is_array(expected)) then
    return nil, M.err(M.ERR.MALFORMED_REQUEST, "expected_project must be a JSON object or null")
  end

  return {
    protocol_version = pv,
    request_id = rid,
    instance_token = token,
    created_at = created,
    expires_at = expires,
    expires_unix = exp,
    command = cmd,
    payload = payload,
    expected_project = expected,
    spec = M.COMMANDS[cmd],
  }
end

--------------------------------------------------------------------------------
-- REAPER version gate
--------------------------------------------------------------------------------

--- Parses "7.22/linux-x86_64" style version strings into major, minor.
function M.parse_reaper_version(s)
  if type(s) ~= "string" then return nil end
  local maj, min = s:match("^(%d+)%.(%d+)")
  if not maj then
    maj = s:match("^(%d+)")
    if not maj then return nil end
    min = "0"
  end
  return tonumber(maj), tonumber(min)
end

--- Returns nil when the host version is supported, otherwise a structured error.
function M.check_reaper_version(version_string)
  local maj = M.parse_reaper_version(version_string)
  if maj == nil then
    -- Unknown format: refuse rather than guess.
    return M.err(M.ERR.UNSUPPORTED_REAPER_VERSION,
      string.format("cannot parse REAPER version %q", tostring(version_string)))
  end
  if maj < M.MIN_REAPER_MAJOR then
    return M.err(M.ERR.UNSUPPORTED_REAPER_VERSION,
      string.format("REAPER %s is older than the required major version %d",
        version_string, M.MIN_REAPER_MAJOR),
      { reaper_version = version_string, min_major = M.MIN_REAPER_MAJOR })
  end
  return nil
end

--------------------------------------------------------------------------------
-- Result envelope
--------------------------------------------------------------------------------

--- Builds a result envelope. Exactly one of `result` / `error` is non-null.
function M.make_result(opts)
  local ok = opts.error == nil
  return {
    protocol_version = M.PROTOCOL_VERSION,
    request_id = opts.request_id or "unknown",
    command = opts.command or json.null,
    ok = ok,
    result = ok and (opts.result or json.object({})) or json.null,
    error = ok and json.null or opts.error,
    transaction_id = opts.transaction_id or json.null,
    bridge_version = M.BRIDGE_VERSION,
    reaper_version = opts.reaper_version or json.null,
    started_at = opts.started_at or json.null,
    completed_at = opts.completed_at or util.iso8601(util.now()),
    duration_ms = opts.duration_ms or 0,
    warnings = opts.warnings or json.array({}),
  }
end

return M
