#!/usr/bin/env lua5.4
--[[
  gen_fixtures.lua -- regenerates fixtures/mock-reaper/** from the live bridge
  implementation, so the cross-language fixtures can never silently drift from
  the Lua that produced them.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT

  Usage:  lua5.4 reaper/tests/gen_fixtures.lua [output-dir]
  Default output-dir: <repo>/fixtures/mock-reaper
--]]

local this_dir = (debug.getinfo(1, "S").source:sub(2):match("^(.*)[/\\][^/\\]*$")) or "."
package.path = table.concat({
  this_dir .. "/../lib/?.lua",
  this_dir .. "/?.lua",
  package.path,
}, ";")

local json = require("json")
local util = require("util")
local protocol = require("protocol")
local snapshot = require("snapshot")
local tagging = require("tagging")
local transactions = require("transactions")
local mock_reaper = require("mock_reaper")

local OUT = arg[1] or (this_dir .. "/../../fixtures/mock-reaper")

local function mkdir(p)
  os.execute(string.format("mkdir -p %q", p))
end

local function write(rel, value)
  local path = OUT .. "/" .. rel
  mkdir(path:match("^(.*)/[^/]*$"))
  local body = type(value) == "string" and value
    or (assert(json.encode(value, { indent = "  " })) .. "\n")
  local f = assert(io.open(path, "wb"))
  f:write(body)
  f:close()
  io.write("wrote ", rel, "\n")
end

--------------------------------------------------------------------------------
-- A deterministic mock scene
--------------------------------------------------------------------------------

local state, _, item, take = mock_reaper.with_melody({
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

-- Pin the project uuid so the fixtures are reproducible byte for byte.
local FIXED_UUID = "00000000-0000-4000-8000-000000000001"
tagging.set_proj_state(proj, tagging.PROJ_KEYS.PROJECT_UUID, FIXED_UUID)

local snap = assert(snapshot.build(proj, {
  source_mode = "auto", note_scope = "all", reaper_version = "7.22/linux-x86_64",
}))
snap.snapshot_id = "00000000-0000-4000-8000-0000000000aa"
snap.project_pointer = "MOCK"
snap.timestamp = 1785091879
snap.timestamp_iso = util.iso8601(1785091879)

--------------------------------------------------------------------------------
-- Hash vectors: canonical string in, prefixed hash out.
--------------------------------------------------------------------------------

local raw_notes = snapshot.read_take_notes(take)
local markers = snapshot.read_tempo_map(proj)

local vectors = {
  {
    name = "fnv1a64.empty",
    description = "FNV-1a 64 offset basis, hashing the empty byte string.",
    canonical_string = "",
    hash = util.hash_hex(""),
  },
  {
    name = "fnv1a64.a",
    description = "Single ASCII byte 0x61.",
    canonical_string = "a",
    hash = util.hash_hex("a"),
  },
  {
    name = "fnv1a64.foobar",
    description = "Standard FNV test vector.",
    canonical_string = "foobar",
    hash = util.hash_hex("foobar"),
  },
  {
    name = "midi_hash.empty_take",
    description = "midi_hash of a MIDI take that contains no notes.",
    canonical_string = snapshot.midi_canonical({}),
    hash = util.hash_hex(snapshot.midi_canonical({})),
  },
  {
    name = "midi_hash.one_note",
    description = "midi_hash of one note, PPQ 0..960, ch 0, pitch 60, vel 100, unmuted.",
    canonical_string = snapshot.midi_canonical({
      { start_ppq = 0.0, end_ppq = 960.0, channel = 0, pitch = 60, velocity = 100, muted = false },
    }),
    hash = util.hash_hex(snapshot.midi_canonical({
      { start_ppq = 0.0, end_ppq = 960.0, channel = 0, pitch = 60, velocity = 100, muted = false },
    })),
  },
  {
    name = "midi_hash.scene",
    description = "midi_hash of the four-note fixture melody.",
    canonical_string = snapshot.midi_canonical(raw_notes),
    hash = snap.midi_hash,
  },
  {
    name = "note_selection_hash.scene",
    description = "note_selection_hash of the fixture melody (notes 0..2 selected).",
    canonical_string = snapshot.selection_canonical(raw_notes),
    hash = snap.note_selection_hash,
  },
  {
    name = "tempo_map_hash.scene",
    description = "tempo_map_hash of a project with no explicit markers (120 bpm, 4/4).",
    canonical_string = snapshot.tempo_canonical(markers),
    hash = snap.tempo_map_hash,
  },
  {
    name = "timesig_map_hash.scene",
    description = "timesig_map_hash of the same project.",
    canonical_string = snapshot.timesig_canonical(markers),
    hash = snap.timesig_map_hash,
  },
  {
    name = "note_list_hash.scene",
    description = "note_list_hash of the extracted note list in project QN.",
    canonical_string = snapshot.note_list_canonical(snap.notes),
    hash = snap.note_list_hash,
  },
  {
    name = "snapshot_hash.scene",
    description = "snapshot_hash of the whole fixture snapshot.",
    canonical_string = snapshot.snapshot_canonical(snap),
    hash = snap.snapshot_hash,
  },
}

write("hashes/vectors.json", {
  algorithm = "FNV-1a 64-bit over the UTF-8 bytes of `canonical_string`",
  offset_basis = "0xcbf29ce484222325",
  prime = "0x100000001b3",
  output_format = "fnv1a64: followed by 16 lowercase zero-padded hex digits",
  note = "Non-cryptographic. Detects accidental change only.",
  float_format = "printf %.6f, negative zero normalised to 0.000000",
  vectors = json.array(vectors),
})

--------------------------------------------------------------------------------
-- Snapshot fixture
--------------------------------------------------------------------------------

write("snapshots/four-note-melody.snapshot.json", snapshot.to_wire(snap))

--------------------------------------------------------------------------------
-- Requests
--------------------------------------------------------------------------------

local CREATED = "2026-07-26T18:51:19Z"
local EXPIRES = "2026-07-26T18:51:49Z"
local TOKEN = "0123456789abcdef0123456789abcdef"

local function envelope(id, command, payload, over)
  local e = {
    protocol_version = protocol.PROTOCOL_VERSION,
    request_id = id,
    instance_token = TOKEN,
    created_at = CREATED,
    expires_at = EXPIRES,
    command = command,
    payload = payload or json.object({}),
  }
  for k, v in pairs(over or {}) do e[k] = v end
  return e
end

write("requests/valid-ping.command.json", envelope("valid-ping", "ping"))
write("requests/valid-status.command.json", envelope("valid-status", "status"))
write("requests/valid-inspect-selection.command.json",
  envelope("valid-inspect-selection", "inspect_selection", {
    source_mode = "auto",
    note_scope = "selected_or_all",
    melody_extraction = { mode = "auto" },
  }))
write("requests/valid-inspect-with-preconditions.command.json",
  envelope("valid-inspect-with-preconditions", "inspect_selection",
    { note_scope = "all" },
    { expected_project = {
      project_uuid = snap.project_uuid,
      source_item_guid = snap.item_guid,
      source_take_guid = snap.take_guid,
      midi_hash = snap.midi_hash,
      tempo_map_hash = snap.tempo_map_hash,
      snapshot_hash = snap.snapshot_hash,
    } }))
write("requests/valid-commit.command.json",
  envelope("valid-commit", "commit_candidate", { transaction_id = "tx-0001" }))
write("requests/valid-discard.command.json",
  envelope("valid-discard", "discard_candidate", { transaction_id = "tx-0001" }))
write("requests/valid-undo.command.json",
  envelope("valid-undo", "undo_last_generation", { transaction_id = "tx-0001" }))

write("requests/invalid-protocol-version.command.json",
  envelope("invalid-protocol-version", "ping", nil, { protocol_version = "qlabs-reaper-ipc/999" }))
write("requests/invalid-token.command.json",
  envelope("invalid-token", "ping", nil, { instance_token = "not-the-installation-token" }))
write("requests/invalid-expired.command.json",
  envelope("invalid-expired", "ping", nil, { expires_at = "2020-01-01T00:00:00Z" }))
write("requests/invalid-unknown-command.command.json",
  envelope("invalid-unknown-command", "execute_lua", { code = "os.exit()" }))
write("requests/invalid-request-id.command.json",
  envelope("../../etc/passwd", "ping"))
write("requests/invalid-payload-is-array.command.json",
  envelope("invalid-payload-is-array", "ping", json.array({ 1, 2, 3 })))
write("requests/invalid-truncated.command.json",
  '{"protocol_version": "qlabs-reaper-ipc/1", "request_id": "invalid-trunc')

--------------------------------------------------------------------------------
-- Edit plans
--------------------------------------------------------------------------------

local function base_plan()
  return {
    plan_id = "00000000-0000-4000-8000-0000000000b1",
    candidate_id = "00000000-0000-4000-8000-0000000000c1",
    transaction_id = "00000000-0000-4000-8000-0000000000d1",
    base_snapshot_id = snap.snapshot_id,
    base_snapshot_hash = snap.snapshot_hash,
    project_uuid = snap.project_uuid,
    knowledge_version = "2026.07.1",
    undo_label = "QLabs MCP: Stage candidate 00000000",
    operations = json.array({
      { op = "create_folder_track", temp_id = "folder", name = "QLabs Candidate 01 -- Warm Extended",
        tags = json.array({ json.array({ "QLABS_ROLE", "candidate_folder" }) }) },
      { op = "create_track", temp_id = "chords", parent = "folder", name = "Chords",
        tags = json.array({ json.array({ "QLABS_ROLE", "harmonic_bed" }) }) },
      { op = "create_track", temp_id = "bass", parent = "folder", name = "Bass",
        tags = json.array({ json.array({ "QLABS_ROLE", "bass" }) }) },
      { op = "create_midi_item", temp_id = "chord_item", track = "chords",
        start_qn = 0.0, end_qn = 8.0, muted = true, tags = json.array({}) },
      { op = "insert_notes", item = "chord_item", notes = json.array({
        { start_qn = 0.0, end_qn = 4.0, pitch = 60, velocity = 84, channel = 0, spelling = "C4" },
        { start_qn = 0.0, end_qn = 4.0, pitch = 64, velocity = 84, channel = 0, spelling = "E4" },
        { start_qn = 0.0, end_qn = 4.0, pitch = 67, velocity = 84, channel = 0, spelling = "G4" },
        { start_qn = 4.0, end_qn = 8.0, pitch = 62, velocity = 84, channel = 0, spelling = "D4" },
        { start_qn = 4.0, end_qn = 8.0, pitch = 65, velocity = 84, channel = 0, spelling = "F4" },
        { start_qn = 4.0, end_qn = 8.0, pitch = 69, velocity = 84, channel = 0, spelling = "A4" },
      }) },
      { op = "create_midi_item", temp_id = "bass_item", track = "bass",
        start_qn = 0.0, end_qn = 8.0, muted = true, tags = json.array({}) },
      { op = "insert_notes", item = "bass_item", notes = json.array({
        { start_qn = 0.0, end_qn = 4.0, pitch = 36, velocity = 96, channel = 0, spelling = "C2" },
        { start_qn = 4.0, end_qn = 8.0, pitch = 38, velocity = 96, channel = 0, spelling = "D2" },
      }) },
      { op = "set_track_mute", track = "chords", muted = true },
      { op = "set_track_mute", track = "bass", muted = true },
    }),
    preconditions = json.array({
      { type = "project_uuid", value = snap.project_uuid },
      { type = "item_guid_exists", value = snap.item_guid },
      { type = "take_guid_exists", value = snap.take_guid },
      { type = "midi_hash", value = snap.midi_hash },
      { type = "tempo_map_hash", value = snap.tempo_map_hash },
      { type = "item_bounds", start_qn = snap.item_position_qn, end_qn = snap.item_end_qn },
    }),
    expected_outputs = json.array({
      { temp_id = "folder", kind = "folder_track" },
      { temp_id = "chords", kind = "track" },
      { temp_id = "bass", kind = "track" },
      { temp_id = "chord_item", kind = "midi_item", note_count = 6 },
      { temp_id = "bass_item", kind = "midi_item", note_count = 2 },
    }),
  }
end

write("plans/valid-two-part-candidate.plan.json", base_plan())

local invalid = {}

local p = base_plan(); p.undo_label = "Delete all tracks"
invalid[#invalid + 1] = { file = "invalid-unowned-undo-label.plan.json", plan = p,
  expect = "INVALID_EDIT_PLAN" }

p = base_plan(); p.knowledge_version = "not a version"
invalid[#invalid + 1] = { file = "invalid-knowledge-version.plan.json", plan = p,
  expect = "KNOWLEDGE_INVALID" }

p = base_plan()
p.operations = json.array({ { op = "run_reaper_action", command_id = 40001 } })
invalid[#invalid + 1] = { file = "invalid-forbidden-operation.plan.json", plan = p,
  expect = "INVALID_EDIT_PLAN" }

p = base_plan()
p.operations[5].notes[1].end_qn = 999.0
invalid[#invalid + 1] = { file = "invalid-note-outside-item-bounds.plan.json", plan = p,
  expect = "INVALID_EDIT_PLAN" }

p = base_plan()
p.operations[5].notes[1].pitch = 200
invalid[#invalid + 1] = { file = "invalid-pitch-out-of-range.plan.json", plan = p,
  expect = "INVALID_EDIT_PLAN" }

p = base_plan()
p.operations[1].tags = json.array({ json.array({ "P_NAME", "renamed by the plan" }) })
invalid[#invalid + 1] = { file = "invalid-non-allowlisted-tag.plan.json", plan = p,
  expect = "INVALID_EDIT_PLAN" }

p = base_plan()
p.operations = json.array({
  { op = "create_midi_item", temp_id = "i", track = "declared_later", start_qn = 0, end_qn = 4 },
  { op = "create_track", temp_id = "declared_later", name = "T" },
})
p.expected_outputs = json.array({})
invalid[#invalid + 1] = { file = "invalid-forward-reference.plan.json", plan = p,
  expect = "INVALID_EDIT_PLAN" }

p = base_plan(); p.base_snapshot_hash = "fnv1a64:0000000000000000"
invalid[#invalid + 1] = { file = "invalid-stale-base-snapshot.plan.json", plan = p,
  expect = "STALE_SNAPSHOT" }

local index = {}
for _, entry in ipairs(invalid) do
  write("plans/" .. entry.file, entry.plan)
  index[#index + 1] = { file = entry.file, expected_error_code = entry.expect }
end
write("plans/invalid-index.json", {
  description = "Each plan here must be rejected with the given error code before any mutation.",
  plans = json.array(index),
})

--------------------------------------------------------------------------------
-- A real staged run, captured as a result fixture
--------------------------------------------------------------------------------

local staged, stage_err = transactions.stage(proj, base_plan(), { verify_snapshot = false })
if not staged then
  error("fixture staging failed: " .. tostring(stage_err and stage_err.message))
end

write("results/valid-stage.result.json", protocol.make_result({
  request_id = "valid-stage",
  command = "stage_candidate",
  result = staged,
  transaction_id = staged.transaction_id,
  reaper_version = "7.22/linux-x86_64",
  started_at = CREATED,
  completed_at = CREATED,
  duration_ms = 12.5,
}))

write("results/error-no-midi-source.result.json", protocol.make_result({
  request_id = "err-no-midi-source",
  command = "inspect_selection",
  error = protocol.err(protocol.ERR.NO_MIDI_SOURCE,
    "no MIDI source: open a MIDI editor or select exactly one MIDI item",
    { selected_item_count = 0 }),
  reaper_version = "7.22/linux-x86_64",
  started_at = CREATED,
  completed_at = CREATED,
  duration_ms = 0.9,
}))

write("results/error-stale-snapshot.result.json", protocol.make_result({
  request_id = "err-stale-snapshot",
  command = "stage_candidate",
  error = protocol.err(protocol.ERR.STALE_SNAPSHOT,
    "the project no longer matches the snapshot this plan was generated from; "
    .. "re-inspect and re-generate rather than writing stale material",
    { expected = snap.snapshot_hash, actual = "fnv1a64:0000000000000000" }),
  transaction_id = "00000000-0000-4000-8000-0000000000d1",
  reaper_version = "7.22/linux-x86_64",
  started_at = CREATED,
  completed_at = CREATED,
  duration_ms = 4.2,
}))

--------------------------------------------------------------------------------
-- Bridge-owned files
--------------------------------------------------------------------------------

write("bridge/heartbeat.json", {
  protocol_version = protocol.PROTOCOL_VERSION,
  bridge_version = protocol.BRIDGE_VERSION,
  bridge_schema_version = protocol.BRIDGE_SCHEMA_VERSION,
  reaper_version = "7.22/linux-x86_64",
  pid_token = "9f2c1a7b4e0d63558a1140fbb2c37e91",
  project_uuid = FIXED_UUID,
  project_name = "MockProject",
  project_path = "/projects/mock.rpp",
  ipc_dir = "/reaper/Scripts/QLabs-Reaper-MCP/ipc",
  status = "online",
  timestamp = 1785091879,
  timestamp_iso = util.iso8601(1785091879),
  uptime_seconds = 412.5,
  requests_processed = 17,
  requests_failed = 1,
  poll_interval_ms = 50,
  heartbeat_interval_ms = 1000,
  stale_after_seconds = protocol.LIMITS.LOCK_STALE_SECONDS,
  commands = json.array(protocol.COMMAND_NAMES),
})

write("bridge/bridge.lock", {
  pid_token = "9f2c1a7b4e0d63558a1140fbb2c37e91",
  bridge_version = protocol.BRIDGE_VERSION,
  protocol_version = protocol.PROTOCOL_VERSION,
  reaper_version = "7.22/linux-x86_64",
  ipc_dir = "/reaper/Scripts/QLabs-Reaper-MCP/ipc",
  acquired_at = 1785091400,
  acquired_at_iso = util.iso8601(1785091400),
  heartbeat_at = 1785091879,
  heartbeat_at_iso = util.iso8601(1785091879),
  stale_after_seconds = protocol.LIMITS.LOCK_STALE_SECONDS,
})

write("bridge/config.json", {
  instance_token = TOKEN,
  ipc_dir = json.null,
  poll_interval_ms = 50,
  heartbeat_interval_ms = 1000,
  log_level = "info",
  console_log = false,
})

write("bridge/limits.json", protocol.LIMITS)
write("bridge/error-codes.json", {
  description = "Every structured error code. BRIDGE_OFFLINE and IPC_TIMEOUT are "
    .. "produced by the Rust client only.",
  codes = json.array(protocol.ERROR_CODES),
  client_only = json.array({ "BRIDGE_OFFLINE", "IPC_TIMEOUT" }),
  commands = json.array(protocol.COMMAND_NAMES),
  operations = json.array(transactions.OPERATION_NAMES),
  tag_keys = json.array(tagging.TAG_KEY_NAMES),
})

io.write("\nfixtures written to ", OUT, "\n")
