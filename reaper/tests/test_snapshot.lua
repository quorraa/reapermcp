--[[ test_snapshot.lua -- source resolution, note reading, hashing, staleness. ]]

local H = require("harness")
local protocol = require("protocol")
local snapshot = require("snapshot")
local tagging = require("tagging")
local util = require("util")
local mock_reaper = require("mock_reaper")

local PPQ = mock_reaper.PPQ_PER_QN

--- Builds a mock host with one four-bar melody item.
local function melody(opts)
  opts = opts or {}
  local state, track, item, take = mock_reaper.with_melody({
    start_qn = opts.start_qn or 0.0,
    end_qn = opts.end_qn or 8.0,
    notes = opts.notes or {
      { 0.0, 1.0, 60, 100, 0, true },
      { 1.0, 2.0, 62, 96, 0, true },
      { 2.0, 4.0, 64, 90, 0, false },
    },
  })
  state:install()
  return state, track, item, take
end

return {
  { "resolves the active MIDI editor take first", function()
    local state, _, item, take = melody()
    state:set_editor_take(take)
    state:select_item(item, false)
    local src = snapshot.resolve_source(state:project(), "auto")
    H.ok(src)
    H.eq(src.take, take)
    H.eq(src.resolved_by, "active_editor")
  end },

  { "falls back to exactly one selected MIDI item", function()
    local state, _, item, take = melody()
    state:set_editor_take(nil)
    state:select_item(item, true)
    local src = snapshot.resolve_source(state:project(), "auto")
    H.ok(src)
    H.eq(src.take, take)
    H.eq(src.resolved_by, "selected_item")
  end },

  { "returns NO_MIDI_SOURCE when nothing is selected", function()
    local state = melody()
    state:set_editor_take(nil)
    local _, err = snapshot.resolve_source(state:project(), "auto")
    H.err_code(err, protocol.ERR.NO_MIDI_SOURCE)
  end },

  { "returns MULTIPLE_MIDI_SOURCES for two selected items", function()
    local state, track, item = melody()
    local item2 = state:add_midi_item(track, 8, 12)
    state:set_editor_take(nil)
    state:select_item(item, true)
    state:select_item(item2, true)
    local _, err = snapshot.resolve_source(state:project(), "auto")
    H.err_code(err, protocol.ERR.MULTIPLE_MIDI_SOURCES)
    H.eq(err.details.midi_item_count, 2)
  end },

  { "active_editor mode does not fall back to selection", function()
    local state, _, item = melody()
    state:set_editor_take(nil)
    state:select_item(item, true)
    local _, err = snapshot.resolve_source(state:project(), "active_editor")
    H.err_code(err, protocol.ERR.NO_MIDI_SOURCE)
  end },

  { "rejects an unknown source mode", function()
    local state = melody()
    local _, err = snapshot.resolve_source(state:project(), "whatever")
    H.err_code(err, protocol.ERR.MALFORMED_REQUEST)
  end },

  { "reads notes deterministically regardless of insertion order", function()
    local state, _, _, take = melody({ notes = {} })
    state:add_note(take, 2 * PPQ, 3 * PPQ, 67, 80, 0, false)
    state:add_note(take, 0 * PPQ, 1 * PPQ, 60, 100, 0, true)
    state:add_note(take, 0 * PPQ, 1 * PPQ, 55, 100, 1, false)
    local notes = snapshot.read_take_notes(take)
    H.eq(#notes, 3)
    H.eq(notes[1].pitch, 55)
    H.eq(notes[2].pitch, 60)
    H.eq(notes[3].pitch, 67)
    H.eq(notes[1].channel, 1)
  end },

  { "builds a full snapshot with every required field", function()
    local state, _, item, take = melody()
    state:set_editor_take(take)
    local snap, err = snapshot.build(state:project(), { note_scope = "all", reaper_version = "7.22" })
    H.ok(snap, err and err.message)
    for _, field in ipairs({
      "snapshot_id", "project_pointer", "project_uuid", "project_state_change_count",
      "track_guid", "item_guid", "take_guid", "item_position_seconds", "item_length_seconds",
      "item_position_qn", "item_length_qn", "is_loop_source", "midi_hash", "tempo_map_hash",
      "timesig_map_hash", "note_selection_hash", "note_list_hash", "snapshot_hash",
      "timestamp", "timestamp_iso", "bridge_version", "reaper_version", "notes",
    }) do
      H.ok(snap[field] ~= nil, "snapshot is missing field " .. field)
    end
    H.eq(snap.item_guid, (item.guid:gsub("[{}]", "")):lower())
    H.eq(snap.note_count, 3)
    H.eq(snap.notes[1].pitch, 60)
    H.near(snap.notes[1].start_qn, 0.0)
    H.near(snap.notes[3].end_qn, 4.0)
    H.near(snap.notes[1].item_relative_start_qn, 0.0)
    H.eq(snap.notes[1].selected, true)
    H.eq(snap.notes[3].selected, false)
  end },

  { "item-relative positions account for a non-zero item start", function()
    local state, _, _, take = melody({ start_qn = 4.0, end_qn = 12.0,
      notes = { { 4.0, 5.0, 60, 100, 0, true }, { 5.0, 6.0, 62, 100, 0, true } } })
    state:set_editor_take(take)
    local snap = H.ok(snapshot.build(state:project(), { note_scope = "all" }))
    H.near(snap.item_position_qn, 4.0, 1e-6)
    H.near(snap.notes[1].start_qn, 4.0, 1e-6)
    H.near(snap.notes[1].item_relative_start_qn, 0.0, 1e-6)
  end },

  { "hashes are stable across repeated builds", function()
    local state, _, _, take = melody()
    state:set_editor_take(take)
    local a = H.ok(snapshot.build(state:project(), {}))
    local b = H.ok(snapshot.build(state:project(), {}))
    H.eq(a.snapshot_hash, b.snapshot_hash)
    H.eq(a.midi_hash, b.midi_hash)
    H.eq(a.note_list_hash, b.note_list_hash)
    H.ne(a.snapshot_id, b.snapshot_id, "snapshot ids are per call")
  end },

  { "changing a note changes the midi and snapshot hashes", function()
    local state, _, _, take = melody()
    state:set_editor_take(take)
    local before = H.ok(snapshot.build(state:project(), {}))
    take.notes[1].pitch = 61
    local after = H.ok(snapshot.build(state:project(), {}))
    H.ne(after.midi_hash, before.midi_hash)
    H.ne(after.snapshot_hash, before.snapshot_hash)
  end },

  { "changing only the selection changes selection but not midi hash", function()
    local state, _, _, take = melody()
    state:set_editor_take(take)
    local before = H.ok(snapshot.build(state:project(), { note_scope = "all" }))
    take.notes[3].selected = true
    local after = H.ok(snapshot.build(state:project(), { note_scope = "all" }))
    H.eq(after.midi_hash, before.midi_hash)
    H.ne(after.note_selection_hash, before.note_selection_hash)
  end },

  { "canonical hash strings have the documented layout", function()
    local notes = { { start_ppq = 0.0, end_ppq = 960.0, channel = 0, pitch = 60,
      velocity = 100, muted = false, selected = true } }
    H.eq(snapshot.midi_canonical(notes),
      "qlabs.midi.v1\n1\n0|0.000000|960.000000|0|60|100|0\n")
    H.eq(snapshot.selection_canonical(notes), "qlabs.selection.v1\n1\n0|1\n")
    H.eq(snapshot.midi_canonical({}), "qlabs.midi.v1\n0\n")
  end },

  { "changing the tempo map changes the tempo hash", function()
    local state, _, _, take = melody()
    state:set_editor_take(take)
    local before = H.ok(snapshot.build(state:project(), {}))
    state:project().tempo = 140.0
    local after = H.ok(snapshot.build(state:project(), {}))
    H.ne(after.tempo_map_hash, before.tempo_map_hash)
  end },

  { "note scope selected_only filters to the selection", function()
    local state, _, _, take = melody()
    state:set_editor_take(take)
    local snap = H.ok(snapshot.build(state:project(), { note_scope = "selected_only" }))
    H.eq(snap.note_count, 2)
    H.eq(snap.source_note_count, 3)
  end },

  { "note scope selected_or_all uses all notes when nothing is selected", function()
    local state, _, _, take = melody()
    state:set_editor_take(take)
    for i = 1, #take.notes do take.notes[i].selected = false end
    local snap = H.ok(snapshot.build(state:project(), { note_scope = "selected_or_all" }))
    H.eq(snap.note_count, 3)
    H.contains(snap.selection_assumptions[1], "no notes were selected")
  end },

  { "selected_only with no selection is AMBIGUOUS_MELODY", function()
    local state, _, _, take = melody()
    state:set_editor_take(take)
    for i = 1, #take.notes do take.notes[i].selected = false end
    local _, err = snapshot.build(state:project(), { note_scope = "selected_only" })
    H.err_code(err, protocol.ERR.AMBIGUOUS_MELODY)
  end },

  { "midi_channel extraction filters and reports empties", function()
    local state, _, _, take = melody()
    state:set_editor_take(take)
    take.notes[1].channel = 3
    local snap = H.ok(snapshot.build(state:project(),
      { note_scope = "all", extraction_mode = "midi_channel", extraction_channel = 3 }))
    H.eq(snap.note_count, 1)
    local _, err = snapshot.build(state:project(),
      { note_scope = "all", extraction_mode = "midi_channel", extraction_channel = 9 })
    H.err_code(err, protocol.ERR.AMBIGUOUS_MELODY)
    local _, err2 = snapshot.build(state:project(),
      { note_scope = "all", extraction_mode = "midi_channel" })
    H.err_code(err2, protocol.ERR.AMBIGUOUS_MELODY)
  end },

  { "monophonic_voice rejects polyphonic material", function()
    local state, _, _, take = melody({ notes = {} })
    state:set_editor_take(take)
    state:add_note(take, 0, PPQ, 60, 100, 0, false)
    state:add_note(take, 0, PPQ, 64, 100, 0, false)
    local _, err = snapshot.build(state:project(),
      { note_scope = "all", extraction_mode = "monophonic_voice" })
    H.err_code(err, protocol.ERR.AMBIGUOUS_MELODY)
    H.eq(err.details.max_polyphony, 2)
  end },

  { "rejects invalid scope and extraction arguments", function()
    local state, _, _, take = melody()
    state:set_editor_take(take)
    local proj = state:project()
    H.err_code(select(2, snapshot.build(proj, { note_scope = "nope" })),
      protocol.ERR.MALFORMED_REQUEST)
    H.err_code(select(2, snapshot.build(proj, { extraction_mode = "nope" })),
      protocol.ERR.MALFORMED_REQUEST)
    H.err_code(select(2, snapshot.build(proj, { extraction_channel = 42 })),
      protocol.ERR.MALFORMED_REQUEST)
  end },

  { "mints the project uuid during the first snapshot", function()
    local state, _, _, take = melody()
    state:set_editor_take(take)
    local proj = state:project()
    H.eq(tagging.get_proj_state(proj, tagging.PROJ_KEYS.PROJECT_UUID), nil)
    local snap = H.ok(snapshot.build(proj, {}))
    H.eq(snap.project_uuid, tagging.get_proj_state(proj, tagging.PROJ_KEYS.PROJECT_UUID))
    H.eq(snap.warnings[1].code, "project_uuid_minted")
  end },

  { "finds items, takes and tracks by GUID", function()
    local state, track, item, take = melody()
    local proj = state:project()
    H.eq(snapshot.find_item_by_guid(proj, item.guid), item)
    H.eq(snapshot.find_take_by_guid(proj, take.guid), take)
    H.eq(snapshot.find_track_by_guid(proj, track.guid), track)
    H.eq(snapshot.find_item_by_guid(proj, "{00000000-0000-0000-0000-000000000000}"), nil)
    H.eq(snapshot.find_item_by_guid(proj, "not-a-guid"), nil)
  end },

  { "expected_project validation maps each field to its error code", function()
    local state, _, item, take = melody()
    state:set_editor_take(take)
    local proj = state:project()
    local snap = H.ok(snapshot.build(proj, {}))

    -- all good
    H.ok(snapshot.check_expected_project(proj, {
      project_uuid = snap.project_uuid,
      state_change_count = snap.project_state_change_count,
      source_item_guid = snap.item_guid,
      source_take_guid = snap.take_guid,
      midi_hash = snap.midi_hash,
      tempo_map_hash = snap.tempo_map_hash,
    }))

    H.err_code(select(2, snapshot.check_expected_project(proj, { project_uuid = "wrong" })),
      protocol.ERR.PROJECT_CHANGED)
    H.err_code(select(2, snapshot.check_expected_project(proj, { state_change_count = 999999 })),
      protocol.ERR.PROJECT_CHANGED)
    H.err_code(select(2, snapshot.check_expected_project(proj,
      { source_item_guid = "00000000-0000-0000-0000-000000000000" })),
      protocol.ERR.SOURCE_ITEM_MISSING)
    H.err_code(select(2, snapshot.check_expected_project(proj,
      { source_take_guid = "00000000-0000-0000-0000-000000000000" })),
      protocol.ERR.SOURCE_TAKE_MISSING)
    H.err_code(select(2, snapshot.check_expected_project(proj,
      { source_take_guid = snap.take_guid, midi_hash = "fnv1a64:0000000000000000" })),
      protocol.ERR.MIDI_CHANGED)
    H.err_code(select(2, snapshot.check_expected_project(proj,
      { tempo_map_hash = "fnv1a64:0000000000000000" })),
      protocol.ERR.TEMPO_MAP_CHANGED)
    H.err_code(select(2, snapshot.check_expected_project(proj,
      { snapshot_hash = "fnv1a64:0000000000000000" })),
      protocol.ERR.STALE_SNAPSHOT)
    H.ok(snapshot.check_expected_project(proj, { snapshot_hash = snap.snapshot_hash }))
    H.eq(item.guid ~= nil, true)
  end },

  { "a midi_hash constraint without a resolvable take is refused", function()
    local state, _, _, take = melody()
    state:set_editor_take(take)
    local proj = state:project()
    local _, err = snapshot.check_expected_project(proj, { midi_hash = "fnv1a64:0000000000000000" })
    H.err_code(err, protocol.ERR.SOURCE_TAKE_MISSING)
  end },

  { "wire form drops host pointers and is JSON encodable", function()
    local state, _, _, take = melody()
    state:set_editor_take(take)
    local snap = H.ok(snapshot.build(state:project(), {}))
    local wire = snapshot.to_wire(snap)
    H.eq(wire.take, nil)
    H.eq(wire.item, nil)
    H.eq(wire.track, nil)
    local json = require("json")
    local s = json.encode(wire)
    H.ok(s, "wire snapshot must encode")
    H.contains(s, "snapshot_hash")
  end },

  { "note limit is enforced", function()
    local state, _, _, take = melody({ notes = {} })
    state:set_editor_take(take)
    local saved = protocol.LIMITS.MAX_READ_NOTES
    protocol.LIMITS.MAX_READ_NOTES = 3
    for i = 1, 6 do state:add_note(take, i * 10, i * 10 + 5, 60 + i, 90, 0, false) end
    local notes, warns = snapshot.read_take_notes(take)
    protocol.LIMITS.MAX_READ_NOTES = saved
    H.eq(#notes, 3)
    H.eq(warns[1].code, "note_limit_reached")
  end },

  { "empty take yields an empty_selection warning, not an error", function()
    local state, _, _, take = melody({ notes = {} })
    state:set_editor_take(take)
    local snap = H.ok(snapshot.build(state:project(), { note_scope = "all" }))
    H.eq(snap.note_count, 0)
    local found = false
    for _, w in ipairs(snap.warnings) do
      if w.code == "empty_selection" then found = true end
    end
    H.ok(found)
    H.eq(snap.midi_hash, util.hash_hex("qlabs.midi.v1\n0\n"))
  end },
}
