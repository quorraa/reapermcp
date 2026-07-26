--[[ test_fixtures.lua -- locks fixtures/mock-reaper/** against drift.

  These fixtures are the cross-language contract: the Rust IPC client is written
  against them. If a change to the Lua alters a canonical string, a hash, or an
  error classification, these tests fail and the fixtures must be regenerated
  with `lua5.4 reaper/tests/gen_fixtures.lua` -- and the Rust side told. ]]

local H = require("harness")
local json = require("json")
local util = require("util")
local protocol = require("protocol")
local snapshot = require("snapshot")
local tagging = require("tagging")
local transactions = require("transactions")
local mock_reaper = require("mock_reaper")

local TESTS_DIR = (debug.getinfo(1, "S").source:sub(2):match("^(.*)[/\\][^/\\]*$")) or "."
local FIXTURES = TESTS_DIR .. "/../../fixtures/mock-reaper"

local function read(rel)
  local f = io.open(FIXTURES .. "/" .. rel, "rb")
  if not f then return nil end
  local data = f:read("a")
  f:close()
  return data
end

local function load_json(rel)
  local raw = read(rel)
  H.ok(raw, "missing fixture " .. rel)
  local v, err = json.decode(raw)
  H.ok(v, "fixture " .. rel .. " is not valid JSON: " .. tostring(err))
  return v
end

--- Rebuilds the exact scene gen_fixtures.lua used, so GUIDs and hashes match.
local function fixture_scene()
  local state, _, _, take = mock_reaper.with_melody({
    start_qn = 0.0,
    end_qn = 8.0,
    notes = {
      { 0.0, 1.0, 60, 100, 0, true },
      { 1.0, 2.0, 62, 96, 0, true },
      { 2.0, 3.0, 64, 92, 0, true },
      { 3.0, 4.0, 65, 88, 0, false },
    },
  })
  state:install()
  state:set_editor_take(take)
  local proj = state:project()
  tagging.set_proj_state(proj, tagging.PROJ_KEYS.PROJECT_UUID,
    "00000000-0000-4000-8000-000000000001")
  return state, proj, take
end

return {
  { "hash vectors still reproduce", function()
    local doc = load_json("hashes/vectors.json")
    H.ok(#doc.vectors >= 10, "expected the full vector set")
    for _, v in ipairs(doc.vectors) do
      H.eq(util.hash_hex(v.canonical_string), v.hash,
        "hash vector drifted: " .. tostring(v.name))
    end
  end },

  { "canonical strings are regenerated identically from live state", function()
    local _, proj, take = fixture_scene()
    local doc = load_json("hashes/vectors.json")
    local by_name = {}
    for _, v in ipairs(doc.vectors) do by_name[v.name] = v end

    local raw_notes = snapshot.read_take_notes(take)
    H.eq(snapshot.midi_canonical(raw_notes), by_name["midi_hash.scene"].canonical_string)
    H.eq(snapshot.selection_canonical(raw_notes),
      by_name["note_selection_hash.scene"].canonical_string)

    local markers = snapshot.read_tempo_map(proj)
    H.eq(snapshot.tempo_canonical(markers), by_name["tempo_map_hash.scene"].canonical_string)
    H.eq(snapshot.timesig_canonical(markers), by_name["timesig_map_hash.scene"].canonical_string)

    local snap = H.ok(snapshot.build(proj, { note_scope = "all",
      reaper_version = "7.22/linux-x86_64" }))
    H.eq(snapshot.note_list_canonical(snap.notes),
      by_name["note_list_hash.scene"].canonical_string)
    H.eq(snapshot.snapshot_canonical(snap), by_name["snapshot_hash.scene"].canonical_string)
    H.eq(snap.snapshot_hash, by_name["snapshot_hash.scene"].hash)
  end },

  { "the published snapshot fixture matches a live build", function()
    local _, proj = fixture_scene()
    local fixture = load_json("snapshots/four-note-melody.snapshot.json")
    local snap = H.ok(snapshot.build(proj, { source_mode = "auto", note_scope = "all",
      reaper_version = "7.22/linux-x86_64" }))
    for _, field in ipairs({ "project_uuid", "track_guid", "item_guid", "take_guid",
      "item_position_qn", "item_end_qn", "item_length_qn", "note_count",
      "midi_hash", "note_selection_hash", "tempo_map_hash", "timesig_map_hash",
      "note_list_hash", "snapshot_hash" }) do
      H.eq(snap[field], fixture[field], "snapshot fixture drifted at " .. field)
    end
    H.eq(#fixture.notes, 4)
    H.eq(fixture.notes[1].pitch, 60)
  end },

  { "every valid request fixture passes envelope validation", function()
    for _, name in ipairs({ "valid-ping", "valid-status", "valid-inspect-selection",
      "valid-inspect-with-preconditions", "valid-commit", "valid-discard", "valid-undo" }) do
      local req = load_json("requests/" .. name .. ".command.json")
      local ok, err = protocol.validate_envelope(req, {
        instance_token = "0123456789abcdef0123456789abcdef",
        -- The fixtures carry a fixed expiry; validate against a fixed "now".
        now = util.parse_iso8601("2026-07-26T18:51:20Z"),
        request_id_hint = name,
      })
      H.ok(ok, name .. " should validate: " .. tostring(err and err.message))
    end
  end },

  { "every invalid request fixture is rejected with the expected code", function()
    local cases = {
      { "invalid-protocol-version", protocol.ERR.IPC_PROTOCOL_MISMATCH },
      { "invalid-token", protocol.ERR.INVALID_INSTANCE_TOKEN },
      { "invalid-expired", protocol.ERR.EXPIRED_REQUEST },
      { "invalid-unknown-command", protocol.ERR.UNKNOWN_COMMAND },
      { "invalid-request-id", protocol.ERR.MALFORMED_REQUEST },
      { "invalid-payload-is-array", protocol.ERR.MALFORMED_REQUEST },
    }
    for _, c in ipairs(cases) do
      local req = load_json("requests/" .. c[1] .. ".command.json")
      local _, err = protocol.validate_envelope(req, {
        instance_token = "0123456789abcdef0123456789abcdef",
        now = util.parse_iso8601("2026-07-26T18:51:20Z"),
        request_id_hint = c[1],
      })
      H.err_code(err, c[2], c[1])
    end
    -- The truncated fixture must not even parse.
    H.falsy(json.decode(read("requests/invalid-truncated.command.json")))
  end },

  { "the valid plan fixture stages cleanly", function()
    local state, proj = fixture_scene()
    local plan = load_json("plans/valid-two-part-candidate.plan.json")
    -- The plan was generated against a note_scope="all" snapshot, so the
    -- staleness re-derivation must use the same scope. This is the contract the
    -- wire spec states for stage_candidate's scope echo fields.
    local res, err = transactions.stage(proj, plan, { note_scope = "all" })
    H.ok(res, err and (err.code .. ": " .. err.message))
    H.eq(#res.tracks, 3)
    H.eq(#res.items, 2)
    H.eq(res.note_count, 8)
    H.eq(state.ui_refresh_depth, 0)
  end },

  { "every invalid plan fixture is rejected before any mutation", function()
    local index = load_json("plans/invalid-index.json")
    H.ok(#index.plans >= 8)
    for _, entry in ipairs(index.plans) do
      local state, proj = fixture_scene()
      local tracks_before = #proj.tracks
      local scc_before = proj.state_change_count
      local plan = load_json("plans/" .. entry.file)
      local res, err = transactions.stage(proj, plan, { note_scope = "all" })
      H.falsy(res, entry.file .. " must be rejected")
      H.err_code(err, entry.expected_error_code, entry.file)
      H.eq(#proj.tracks, tracks_before, entry.file .. " must not create tracks")
      H.eq(proj.state_change_count, scc_before, entry.file .. " must not change project state")
      H.eq(#proj.undo, 0, entry.file .. " must not create an undo entry")
      H.eq(state.ui_refresh_depth, 0, entry.file .. " must leave UI refresh balanced")
    end
  end },

  { "published limits and error codes match the implementation", function()
    local limits = load_json("bridge/limits.json")
    for k, v in pairs(protocol.LIMITS) do
      H.eq(limits[k], v, "limit drifted: " .. k)
    end
    local codes = load_json("bridge/error-codes.json")
    H.eq(#codes.codes, #protocol.ERROR_CODES)
    for i = 1, #protocol.ERROR_CODES do
      H.eq(codes.codes[i], protocol.ERROR_CODES[i])
    end
    for i = 1, #protocol.COMMAND_NAMES do
      H.eq(codes.commands[i], protocol.COMMAND_NAMES[i])
    end
    for i = 1, #transactions.OPERATION_NAMES do
      H.eq(codes.operations[i], transactions.OPERATION_NAMES[i])
    end
    for i = 1, #tagging.TAG_KEY_NAMES do
      H.eq(codes.tag_keys[i], tagging.TAG_KEY_NAMES[i])
    end
  end },

  { "the heartbeat and lock fixtures match the shapes the bridge writes", function()
    local hb = load_json("bridge/heartbeat.json")
    for _, f in ipairs({ "protocol_version", "bridge_version", "bridge_schema_version",
      "reaper_version", "pid_token", "project_uuid", "project_name", "project_path",
      "ipc_dir", "status", "timestamp", "timestamp_iso", "uptime_seconds",
      "requests_processed", "requests_failed", "poll_interval_ms",
      "heartbeat_interval_ms", "stale_after_seconds", "commands" }) do
      H.ok(hb[f] ~= nil, "heartbeat fixture is missing " .. f)
    end
    H.eq(hb.protocol_version, protocol.PROTOCOL_VERSION)
    H.eq(hb.timestamp_iso, util.iso8601(hb.timestamp))

    local lock = load_json("bridge/bridge.lock")
    for _, f in ipairs({ "pid_token", "bridge_version", "protocol_version", "reaper_version",
      "ipc_dir", "acquired_at", "acquired_at_iso", "heartbeat_at", "heartbeat_at_iso",
      "stale_after_seconds" }) do
      H.ok(lock[f] ~= nil, "lock fixture is missing " .. f)
    end
    H.eq(lock.stale_after_seconds, protocol.LIMITS.LOCK_STALE_SECONDS)

    local cfg = load_json("bridge/config.json")
    H.eq(#cfg.instance_token, 32)
    H.ok(cfg.ipc_dir == json.null)
  end },

  { "result fixtures satisfy the envelope invariants", function()
    for _, name in ipairs({ "valid-stage", "error-no-midi-source", "error-stale-snapshot" }) do
      local res = load_json("results/" .. name .. ".result.json")
      H.eq(res.protocol_version, protocol.PROTOCOL_VERSION)
      H.ok(type(res.ok) == "boolean")
      if res.ok then
        H.eq(res.error, json.null)
        H.ok(type(res.result) == "table")
      else
        H.eq(res.result, json.null)
        H.ok(type(res.error) == "table")
        H.ok(protocol.ERR[res.error.code] ~= nil, "unknown error code " .. tostring(res.error.code))
      end
    end
  end },
}
