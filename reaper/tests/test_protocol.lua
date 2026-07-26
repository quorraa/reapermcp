--[[ test_protocol.lua -- envelope validation, allowlist, error-code coverage. ]]

local H = require("harness")
local util = require("util")
local json = require("json")
local protocol = require("protocol")

local TOKEN = "0123456789abcdef0123456789abcdef"

local function envelope(over)
  local e = {
    protocol_version = protocol.PROTOCOL_VERSION,
    request_id = "req-0001",
    instance_token = TOKEN,
    created_at = util.iso8601(util.now()),
    expires_at = util.iso8601(util.now() + 60),
    command = "ping",
    payload = {},
  }
  for k, v in pairs(over or {}) do
    if v == "\0REMOVE" then e[k] = nil else e[k] = v end
  end
  return e
end

local function ctx(over)
  local c = { instance_token = TOKEN, now = util.now(), request_id_hint = "req-0001" }
  for k, v in pairs(over or {}) do c[k] = v end
  return c
end

return {
  { "every brief section 19 error code exists", function()
    local required = {
      "BRIDGE_OFFLINE", "BRIDGE_VERSION_MISMATCH", "IPC_TIMEOUT", "IPC_PROTOCOL_MISMATCH",
      "INVALID_INSTANCE_TOKEN", "EXPIRED_REQUEST", "PAYLOAD_TOO_LARGE", "NO_ACTIVE_PROJECT",
      "NO_MIDI_SOURCE", "MULTIPLE_MIDI_SOURCES", "AMBIGUOUS_MELODY", "SOURCE_ITEM_MISSING",
      "SOURCE_TAKE_MISSING", "STALE_SNAPSHOT", "PROJECT_CHANGED", "MIDI_CHANGED",
      "TEMPO_MAP_CHANGED", "INVALID_EDIT_PLAN", "KNOWLEDGE_INVALID",
      "UNSUPPORTED_REAPER_VERSION", "UNDO_NOT_OWNED", "INTERNAL_BRIDGE_ERROR",
    }
    for i = 1, #required do
      H.eq(protocol.ERR[required[i]], required[i], "missing error code " .. required[i])
    end
  end },

  { "command allowlist is exactly the seven permitted commands", function()
    H.eq(#protocol.COMMAND_NAMES, 7)
    local expect = { "commit_candidate", "discard_candidate", "inspect_selection", "ping",
      "stage_candidate", "status", "undo_last_generation" }
    for i = 1, #expect do H.eq(protocol.COMMAND_NAMES[i], expect[i]) end
    for _, forbidden in ipairs({ "execute_lua", "execute_shell", "run_reaper_action",
      "write_arbitrary_midi", "delete_track_by_name", "edit_project_chunk", "eval" }) do
      H.falsy(protocol.is_allowed_command(forbidden), forbidden .. " must never be allowlisted")
    end
  end },

  { "accepts a well-formed envelope", function()
    local req, err = protocol.validate_envelope(envelope(), ctx())
    H.ok(req, err and err.message)
    H.eq(req.command, "ping")
    H.eq(req.request_id, "req-0001")
    H.eq(req.spec.writes, false)
  end },

  { "rejects a mismatched protocol version", function()
    local _, err = protocol.validate_envelope(envelope({ protocol_version = "qlabs-reaper-ipc/9" }), ctx())
    H.err_code(err, protocol.ERR.IPC_PROTOCOL_MISMATCH)
    local _, err2 = protocol.validate_envelope(envelope({ protocol_version = "\0REMOVE" }), ctx())
    H.err_code(err2, protocol.ERR.IPC_PROTOCOL_MISMATCH)
  end },

  { "rejects an invalid instance token", function()
    local _, err = protocol.validate_envelope(envelope({ instance_token = "wrong" }), ctx())
    H.err_code(err, protocol.ERR.INVALID_INSTANCE_TOKEN)
    local _, e2 = protocol.validate_envelope(
      envelope({ instance_token = string.rep("f", #TOKEN) }), ctx())
    H.err_code(e2, protocol.ERR.INVALID_INSTANCE_TOKEN)
    local _, e3 = protocol.validate_envelope(envelope({ instance_token = "\0REMOVE" }), ctx())
    H.err_code(e3, protocol.ERR.INVALID_INSTANCE_TOKEN)
  end },

  { "rejects an expired request", function()
    local e = envelope({ expires_at = util.iso8601(util.now() - 3600) })
    local _, err = protocol.validate_envelope(e, ctx())
    H.err_code(err, protocol.ERR.EXPIRED_REQUEST)
  end },

  { "tolerates the documented clock skew", function()
    local e = envelope({ expires_at = util.iso8601(util.now() - 1) })
    local req = protocol.validate_envelope(e, ctx())
    H.ok(req, "1s past expiry is inside the 5s skew allowance")
  end },

  { "rejects an oversized request", function()
    local _, err = protocol.validate_envelope(envelope(),
      ctx({ raw_size = protocol.LIMITS.MAX_REQUEST_BYTES + 1 }))
    H.err_code(err, protocol.ERR.PAYLOAD_TOO_LARGE)
  end },

  { "rejects an unknown command", function()
    local _, err = protocol.validate_envelope(envelope({ command = "execute_lua" }), ctx())
    H.err_code(err, protocol.ERR.UNKNOWN_COMMAND)
    H.ok(err.details.allowed)
  end },

  { "rejects a duplicate request id", function()
    local seen = util.ring_set(8)
    seen:add("req-0001")
    local _, err = protocol.validate_envelope(envelope(), ctx({ seen = seen }))
    H.err_code(err, protocol.ERR.DUPLICATE_REQUEST)
  end },

  { "rejects a request id that disagrees with the filename", function()
    local _, err = protocol.validate_envelope(envelope({ request_id = "req-0002" }), ctx())
    H.err_code(err, protocol.ERR.MALFORMED_REQUEST)
  end },

  { "rejects a request id that could escape the ipc directory", function()
    for _, bad in ipairs({ "../../etc/passwd", "a/b", "a\\b", ".hidden", "x..y" }) do
      local _, err = protocol.validate_envelope(envelope({ request_id = bad }),
        ctx({ request_id_hint = nil }))
      H.err_code(err, protocol.ERR.MALFORMED_REQUEST, "must reject " .. bad)
    end
  end },

  { "rejects a bridge version requirement that cannot be met", function()
    local _, err = protocol.validate_envelope(envelope({ require_bridge_version = "99.0.0" }), ctx())
    H.err_code(err, protocol.ERR.BRIDGE_VERSION_MISMATCH)
    local req = protocol.validate_envelope(
      envelope({ require_bridge_version = protocol.BRIDGE_VERSION }), ctx())
    H.ok(req)
  end },

  { "rejects malformed envelopes", function()
    local _, e1 = protocol.validate_envelope("not an object", ctx())
    H.err_code(e1, protocol.ERR.MALFORMED_REQUEST)
    local _, e2 = protocol.validate_envelope({ 1, 2, 3 }, ctx())
    H.err_code(e2, protocol.ERR.MALFORMED_REQUEST)
    local _, e3 = protocol.validate_envelope(envelope({ expires_at = "yesterday" }), ctx())
    H.err_code(e3, protocol.ERR.MALFORMED_REQUEST)
    local _, e4 = protocol.validate_envelope(envelope({ payload = { 1, 2 } }), ctx())
    H.err_code(e4, protocol.ERR.MALFORMED_REQUEST)
    local _, e5 = protocol.validate_envelope(envelope({ command = "\0REMOVE" }), ctx())
    H.err_code(e5, protocol.ERR.MALFORMED_REQUEST)
  end },

  { "treats a null payload as an empty object", function()
    local req = protocol.validate_envelope(envelope({ payload = json.null }), ctx())
    H.ok(req)
    H.eq(type(req.payload), "table")
  end },

  { "gates unsupported REAPER versions", function()
    H.eq(protocol.check_reaper_version("7.22/linux-x86_64"), nil)
    H.eq(protocol.check_reaper_version("6.0"), nil)
    H.err_code(protocol.check_reaper_version("5.99"), protocol.ERR.UNSUPPORTED_REAPER_VERSION)
    H.err_code(protocol.check_reaper_version("banana"), protocol.ERR.UNSUPPORTED_REAPER_VERSION)
    H.err_code(protocol.check_reaper_version(nil), protocol.ERR.UNSUPPORTED_REAPER_VERSION)
  end },

  { "result envelopes carry exactly one of result or error", function()
    local ok_env = protocol.make_result({ request_id = "r", command = "ping", result = { a = 1 } })
    H.eq(ok_env.ok, true)
    H.eq(ok_env.error, json.null)
    local bad = protocol.make_result({ request_id = "r", command = "ping",
      error = protocol.err(protocol.ERR.NO_MIDI_SOURCE, "nope") })
    H.eq(bad.ok, false)
    H.eq(bad.result, json.null)
    H.eq(bad.error.code, "NO_MIDI_SOURCE")
    H.ok(json.encode(bad))
  end },

  { "limits are the documented numbers", function()
    local L = protocol.LIMITS
    H.eq(L.MAX_REQUEST_BYTES, 1048576)
    H.eq(L.MAX_RESULT_BYTES, 8388608)
    H.eq(L.MAX_GENERATED_NOTES, 20000)
    H.eq(L.MAX_TRACKS_PER_STAGE, 32)
    H.eq(L.POLL_INTERVAL_SECONDS, 0.05)
    H.eq(L.LOCK_STALE_SECONDS, 10)
    H.eq(L.SEEN_REQUEST_IDS, 512)
  end },
}
