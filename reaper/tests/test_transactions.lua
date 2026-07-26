--[[ test_transactions.lua -- plan validation, staged writes, undo safety,
     commit/discard tag scoping and UI-refresh bookkeeping. ]]

local H = require("harness")
local json = require("json")
local protocol = require("protocol")
local snapshot = require("snapshot")
local tagging = require("tagging")
local transactions = require("transactions")
local mock_reaper = require("mock_reaper")

local PPQ = mock_reaper.PPQ_PER_QN

--- Builds a host with a source melody and returns state, proj, snapshot.
local function scene()
  local state, track, item, take = mock_reaper.with_melody({
    start_qn = 0.0, end_qn = 8.0,
    notes = { { 0, 1, 60, 100, 0, true }, { 1, 2, 62, 100, 0, true } },
  })
  state:install()
  state:set_editor_take(take)
  local proj = state:project()
  local snap = assert(snapshot.build(proj, { reaper_version = "7.22" }))
  return state, proj, snap, track, item, take
end

--- A minimal valid stage plan: folder track + one child track + one item + notes.
local function make_plan(snap, over)
  local plan = {
    plan_id = "plan-0001",
    candidate_id = "cand-0001",
    transaction_id = "tx-0001",
    base_snapshot_id = snap.snapshot_id,
    base_snapshot_hash = snap.snapshot_hash,
    project_uuid = snap.project_uuid,
    knowledge_version = "2026.07.1",
    undo_label = "QLabs MCP: Stage candidate cand-000",
    operations = json.array({
      { op = "create_folder_track", temp_id = "folder", name = "QLabs Candidate 01",
        tags = json.array({ json.array({ "QLABS_ROLE", "folder" }) }) },
      { op = "create_track", temp_id = "chords", parent = "folder", name = "Chords",
        tags = json.array({ json.array({ "QLABS_ROLE", "harmonic_bed" }) }) },
      { op = "create_midi_item", temp_id = "item1", track = "chords",
        start_qn = 0.0, end_qn = 8.0, muted = true, tags = json.array({}) },
      { op = "insert_notes", item = "item1", notes = json.array({
        { start_qn = 0.0, end_qn = 2.0, pitch = 48, velocity = 90, channel = 0, spelling = "C3" },
        { start_qn = 0.0, end_qn = 2.0, pitch = 55, velocity = 90, channel = 0, spelling = "G3" },
        { start_qn = 2.0, end_qn = 4.0, pitch = 53, velocity = 90, channel = 0, spelling = "F3" },
      }) },
      { op = "set_track_mute", track = "chords", muted = true },
    }),
    preconditions = json.array({
      { type = "project_uuid", value = snap.project_uuid },
      { type = "take_guid_exists", value = snap.take_guid },
      { type = "midi_hash", value = snap.midi_hash },
      { type = "tempo_map_hash", value = snap.tempo_map_hash },
    }),
    expected_outputs = json.array({
      { temp_id = "folder", kind = "folder_track" },
      { temp_id = "chords", kind = "track" },
      { temp_id = "item1", kind = "midi_item", note_count = 3 },
    }),
  }
  for k, v in pairs(over or {}) do plan[k] = v end
  return plan
end

--- Returns a deep-ish signature of the source take so mutation can be detected.
local function take_signature(take)
  local parts = {}
  for i = 1, #take.notes do
    local n = take.notes[i]
    parts[#parts + 1] = string.format("%f/%f/%d/%d/%d", n.start_ppq, n.end_ppq,
      n.pitch, n.velocity, n.channel)
  end
  return table.concat(parts, ";")
end

return {
  ---------------------------------------------------------------- validation --
  { "accepts a well-formed plan", function()
    local _, _, snap = scene()
    local model, err = transactions.validate_plan(make_plan(snap))
    H.ok(model, err and err.message)
    H.eq(model.transaction_id, "tx-0001")
    H.eq(model.counts.notes, 3)
    H.eq(#model.ops, 5)
  end },

  { "rejects a plan that is not an object", function()
    H.err_code(select(2, transactions.validate_plan("nope")), protocol.ERR.INVALID_EDIT_PLAN)
    H.err_code(select(2, transactions.validate_plan({ 1, 2 })), protocol.ERR.INVALID_EDIT_PLAN)
  end },

  { "rejects missing identity fields", function()
    local _, _, snap = scene()
    for _, f in ipairs({ "plan_id", "candidate_id", "transaction_id", "base_snapshot_id",
      "project_uuid", "base_snapshot_hash" }) do
      local p = make_plan(snap)
      p[f] = nil
      H.err_code(select(2, transactions.validate_plan(p)), protocol.ERR.INVALID_EDIT_PLAN,
        "missing " .. f)
    end
  end },

  { "rejects an unowned undo label", function()
    local _, _, snap = scene()
    local p = make_plan(snap, { undo_label = "Delete all tracks" })
    H.err_code(select(2, transactions.validate_plan(p)), protocol.ERR.INVALID_EDIT_PLAN)
  end },

  { "rejects a bad knowledge version with KNOWLEDGE_INVALID", function()
    local _, _, snap = scene()
    for _, bad in ipairs({ "", "has spaces", string.rep("v", 80) }) do
      local p = make_plan(snap, { knowledge_version = bad })
      H.err_code(select(2, transactions.validate_plan(p)), protocol.ERR.KNOWLEDGE_INVALID)
    end
    local p = make_plan(snap)
    p.knowledge_version = nil
    H.err_code(select(2, transactions.validate_plan(p)), protocol.ERR.KNOWLEDGE_INVALID)
  end },

  { "rejects operations outside the allowlist", function()
    local _, _, snap = scene()
    local p = make_plan(snap)
    p.operations = json.array({ { op = "run_action", command_id = 40001 } })
    local _, err = transactions.validate_plan(p)
    H.err_code(err, protocol.ERR.INVALID_EDIT_PLAN)
    H.contains(err.message, "not an allowlisted operation")
  end },

  { "rejects forward references and duplicate temp ids", function()
    local _, _, snap = scene()
    local p = make_plan(snap)
    p.operations = json.array({
      { op = "create_midi_item", temp_id = "i", track = "later", start_qn = 0, end_qn = 4 },
      { op = "create_track", temp_id = "later", name = "T" },
    })
    H.contains(select(2, transactions.validate_plan(p)).message, "undeclared temp_id")

    local p2 = make_plan(snap)
    p2.operations = json.array({
      { op = "create_track", temp_id = "t", name = "A" },
      { op = "create_track", temp_id = "t", name = "B" },
    })
    H.contains(select(2, transactions.validate_plan(p2)).message, "redeclares temp_id")
  end },

  { "rejects notes outside item bounds", function()
    local _, _, snap = scene()
    local p = make_plan(snap)
    p.operations[4].notes[1].end_qn = 99.0
    local _, err = transactions.validate_plan(p)
    H.err_code(err, protocol.ERR.INVALID_EDIT_PLAN)
    H.contains(err.message, "outside item bounds")
  end },

  { "rejects invalid MIDI values", function()
    local _, _, snap = scene()
    local cases = {
      { field = "pitch", value = 128 }, { field = "pitch", value = -1 },
      { field = "velocity", value = 0 }, { field = "velocity", value = 200 },
      { field = "channel", value = 16 }, { field = "channel", value = -1 },
      { field = "pitch", value = 60.5 },
    }
    for _, c in ipairs(cases) do
      local p = make_plan(snap)
      p.operations[4].notes[1][c.field] = c.value
      H.err_code(select(2, transactions.validate_plan(p)), protocol.ERR.INVALID_EDIT_PLAN,
        string.format("%s=%s", c.field, tostring(c.value)))
    end
  end },

  { "rejects zero and negative durations", function()
    local _, _, snap = scene()
    local p = make_plan(snap)
    p.operations[4].notes[1].end_qn = p.operations[4].notes[1].start_qn
    H.err_code(select(2, transactions.validate_plan(p)), protocol.ERR.INVALID_EDIT_PLAN)
    local p2 = make_plan(snap)
    p2.operations[3].end_qn = p2.operations[3].start_qn
    H.err_code(select(2, transactions.validate_plan(p2)), protocol.ERR.INVALID_EDIT_PLAN)
  end },

  { "rejects non-finite numbers", function()
    local _, _, snap = scene()
    local p = make_plan(snap)
    p.operations[3].end_qn = math.huge
    H.err_code(select(2, transactions.validate_plan(p)), protocol.ERR.INVALID_EDIT_PLAN)
  end },

  { "rejects tag keys that are not allowlisted QLABS keys", function()
    local _, _, snap = scene()
    for _, key in ipairs({ "P_NAME", "GUID", "QLABS_EVIL", "B_MUTE" }) do
      local p = make_plan(snap)
      p.operations[1].tags = json.array({ json.array({ key, "x" }) })
      H.err_code(select(2, transactions.validate_plan(p)), protocol.ERR.INVALID_EDIT_PLAN, key)
    end
  end },

  { "enforces plan size limits", function()
    local _, _, snap = scene()
    local p = make_plan(snap)
    local ops = json.array({ { op = "create_track", temp_id = "t0", name = "T" },
      { op = "create_midi_item", temp_id = "i0", track = "t0", start_qn = 0, end_qn = 100000 } })
    local notes = json.array({})
    for i = 1, protocol.LIMITS.MAX_NOTES_PER_ITEM + 1 do
      notes[i] = { start_qn = 0, end_qn = 1, pitch = 60, velocity = 90, channel = 0 }
    end
    ops[3] = { op = "insert_notes", item = "i0", notes = notes }
    p.operations = ops
    p.expected_outputs = json.array({})
    local _, err = transactions.validate_plan(p)
    H.err_code(err, protocol.ERR.INVALID_EDIT_PLAN)
    H.contains(err.message, "limit is")
  end },

  { "rejects expected outputs referencing unknown temp ids or wrong kinds", function()
    local _, _, snap = scene()
    local p = make_plan(snap)
    p.expected_outputs = json.array({ { temp_id = "ghost", kind = "track" } })
    H.err_code(select(2, transactions.validate_plan(p)), protocol.ERR.INVALID_EDIT_PLAN)
    local p2 = make_plan(snap)
    p2.expected_outputs = json.array({ { temp_id = "item1", kind = "track" } })
    H.err_code(select(2, transactions.validate_plan(p2)), protocol.ERR.INVALID_EDIT_PLAN)
  end },

  { "rejects unknown precondition types", function()
    local _, _, snap = scene()
    local p = make_plan(snap)
    p.preconditions = json.array({ { type = "delete_everything", value = "yes" } })
    H.err_code(select(2, transactions.validate_plan(p)), protocol.ERR.INVALID_EDIT_PLAN)
  end },

  -------------------------------------------------------------- preconditions --
  { "precondition failures map to the right codes", function()
    local _, proj, snap = scene()
    local p = make_plan(snap)
    p.preconditions = json.array({ { type = "project_uuid", value = "not-this-project" } })
    local model = H.ok(transactions.validate_plan(p))
    H.err_code(select(2, transactions.check_plan_preconditions(proj, model, {})),
      protocol.ERR.PROJECT_CHANGED)

    local p2 = make_plan(snap)
    p2.preconditions = json.array({ { type = "state_change_count", value = 999999 } })
    local m2 = H.ok(transactions.validate_plan(p2))
    H.err_code(select(2, transactions.check_plan_preconditions(proj, m2, {})),
      protocol.ERR.PROJECT_CHANGED)

    local p3 = make_plan(snap)
    p3.preconditions = json.array({
      { type = "take_guid_exists", value = snap.take_guid },
      { type = "midi_hash", value = "fnv1a64:0000000000000000" } })
    local m3 = H.ok(transactions.validate_plan(p3))
    H.err_code(select(2, transactions.check_plan_preconditions(proj, m3, {})),
      protocol.ERR.MIDI_CHANGED)

    local p4 = make_plan(snap)
    p4.preconditions = json.array({ { type = "tempo_map_hash", value = "fnv1a64:0000000000000000" } })
    local m4 = H.ok(transactions.validate_plan(p4))
    H.err_code(select(2, transactions.check_plan_preconditions(proj, m4, {})),
      protocol.ERR.TEMPO_MAP_CHANGED)

    local p5 = make_plan(snap)
    p5.preconditions = json.array({
      { type = "item_guid_exists", value = snap.item_guid },
      { type = "item_bounds", start_qn = 99.0, end_qn = 111.0 } })
    local m5 = H.ok(transactions.validate_plan(p5))
    H.err_code(select(2, transactions.check_plan_preconditions(proj, m5, {})),
      protocol.ERR.STALE_SNAPSHOT)
  end },

  { "a stale base snapshot hash is refused before any mutation", function()
    local state, proj, snap, _, _, take = scene()
    local plan = make_plan(snap)
    -- The user edits the source after the plan was generated.
    state:add_note(take, 4 * PPQ, 5 * PPQ, 67, 100, 0, false)
    plan.preconditions = json.array({ { type = "take_guid_exists", value = snap.take_guid } })
    local tracks_before = #proj.tracks
    local _, err = transactions.stage(proj, plan, {})
    H.err_code(err, protocol.ERR.STALE_SNAPSHOT)
    H.eq(#proj.tracks, tracks_before, "no tracks may be created")
  end },

  ------------------------------------------------------------------- staging --
  { "stages a candidate, tags everything and leaves the source untouched", function()
    local state, proj, snap, _, _, take = scene()
    local before_sig = take_signature(take)
    local before_notes = #take.notes
    local res, err = transactions.stage(proj, make_plan(snap), {})
    H.ok(res, err and (err.code .. ": " .. err.message))

    H.eq(res.transaction_id, "tx-0001")
    H.eq(#res.tracks, 2)
    H.eq(#res.items, 1)
    H.eq(res.items[1].note_count, 3)
    H.eq(res.note_count, 3)

    -- Source is byte-identical.
    H.eq(take_signature(take), before_sig, "source take must not be mutated")
    H.eq(#take.notes, before_notes)

    -- Everything generated is tagged.
    local owned = tagging.collect_owned(proj, "tx-0001")
    H.eq(#owned.tracks, 2)
    H.eq(#owned.items, 1)
    H.eq(#owned.takes, 1)
    for _, e in ipairs(owned.tracks) do
      H.eq(e.tags.QLABS_OWNER, tagging.OWNER_VALUE)
      H.eq(e.tags.QLABS_STATUS, "preview")
      H.eq(e.tags.QLABS_CANDIDATE_ID, "cand-0001")
      H.eq(e.tags.QLABS_KNOWLEDGE_VERSION, "2026.07.1")
    end

    -- Folder layout.
    local folder, chords
    for i = 1, #proj.tracks do
      local t = proj.tracks[i]
      if t.name == "QLabs Candidate 01" then folder = t end
      if t.name == "Chords" then chords = t end
    end
    H.ok(folder)
    H.ok(chords)
    H.eq(folder.values.I_FOLDERDEPTH, 1)
    H.eq(chords.values.I_FOLDERDEPTH, -1)
    H.eq(chords.values.B_MUTE, 1)

    -- Notes landed inside item bounds and were sorted once.
    local gen_take = owned.takes[1].obj
    H.eq(#gen_take.notes, 3)
    for _, n in ipairs(gen_take.notes) do
      H.ok(n.start_ppq >= -1e-6 and n.end_ppq <= 8 * PPQ + 1e-6, "note escaped item bounds")
    end
    H.eq(gen_take.notes[1].pitch, 48)
    H.eq(gen_take.notes[2].pitch, 55)
    H.eq(gen_take.notes[3].pitch, 53)

    -- Bookkeeping.
    H.eq(state.ui_refresh_depth, 0, "UI refresh must be balanced")
    H.eq(transactions.ui_refresh_depth, 0)
    H.ok(tagging.find_staged(proj, "tx-0001"))
    H.eq(tagging.get_last_transaction(proj).undo_label, "QLabs MCP: Stage candidate cand-000")
    H.ok(state.arrange_updates > 0, "arrange view must be refreshed after the batch")
  end },

  { "an invalid plan causes zero mutations", function()
    local state, proj, snap = scene()
    local tracks_before = #proj.tracks
    local scc_before = proj.state_change_count
    local undo_before = #proj.undo

    local p = make_plan(snap)
    -- Valid up to the last operation, which is rejected.
    p.operations[#p.operations + 1] = { op = "create_midi_item", temp_id = "bad",
      track = "chords", start_qn = 4.0, end_qn = 1.0 }
    local res, err = transactions.stage(proj, p, {})
    H.falsy(res)
    H.err_code(err, protocol.ERR.INVALID_EDIT_PLAN)
    H.eq(#proj.tracks, tracks_before, "no tracks created")
    H.eq(proj.state_change_count, scc_before, "project state must not change")
    H.eq(#proj.undo, undo_before, "no undo entry may be created")
    H.eq(state.ui_refresh_depth, 0)
  end },

  { "UI refresh stays balanced after a forced mid-transaction failure", function()
    local state, proj, snap = scene()
    local tracks_before = #proj.tracks
    local original = _G.reaper.MIDI_Sort
    _G.reaper.MIDI_Sort = function() error("simulated host failure during MIDI_Sort") end

    local res, err = transactions.stage(proj, make_plan(snap), {})

    _G.reaper.MIDI_Sort = original

    H.falsy(res)
    H.err_code(err, protocol.ERR.INTERNAL_BRIDGE_ERROR)
    H.eq(state.ui_refresh_depth, 0, "PreventUIRefresh must be balanced after a failure")
    H.eq(transactions.ui_refresh_depth, 0)
    H.eq(err.details.rolled_back, true, "the failed owned transaction must be rolled back")
    H.eq(#proj.tracks, tracks_before, "rollback must remove the partial work")
    H.eq(#proj.undo, 0, "the failed transaction must not linger on the undo stack")
  end },

  { "a failure never undoes an unrelated action", function()
    local state, proj, snap = scene()
    -- Put an unrelated user action at the top of the undo stack.
    _G.reaper.Undo_BeginBlock2(proj)
    state:add_track(proj, "User Track")
    _G.reaper.Undo_EndBlock2(proj, "User: add track", -1)
    H.eq(_G.reaper.Undo_CanUndo2(proj), "User: add track")
    local user_tracks = #proj.tracks

    local original = _G.reaper.MIDI_InsertNote
    _G.reaper.MIDI_InsertNote = function() error("boom") end
    local _, err = transactions.stage(proj, make_plan(snap), {})
    _G.reaper.MIDI_InsertNote = original

    H.err_code(err, protocol.ERR.INTERNAL_BRIDGE_ERROR)
    H.eq(_G.reaper.Undo_CanUndo2(proj), "User: add track",
      "the unrelated user action must still be the top undo entry")
    H.eq(#proj.tracks, user_tracks, "only the failed transaction's work was removed")
  end },

  { "run_guarded refuses an unowned undo label", function()
    local _, proj = scene()
    local _, err = transactions.run_guarded(proj, "Not ours", function() return true end)
    H.err_code(err, protocol.ERR.INVALID_EDIT_PLAN)
  end },

  { "run_guarded creates no undo entry when the body does nothing", function()
    local _, proj = scene()
    local before = #proj.undo
    local v = transactions.run_guarded(proj, "QLabs MCP: no-op", function() return "done" end)
    H.eq(v, "done")
    H.eq(#proj.undo, before)
  end },

  { "expected output verification fails a mismatched note count", function()
    local state, proj, snap = scene()
    local p = make_plan(snap)
    p.expected_outputs = json.array({ { temp_id = "item1", kind = "midi_item", note_count = 99 } })
    local tracks_before = #proj.tracks
    local _, err = transactions.stage(proj, p, {})
    H.err_code(err, protocol.ERR.INTERNAL_BRIDGE_ERROR)
    H.contains(err.message, "expected 99")
    H.eq(#proj.tracks, tracks_before, "verification failure must roll the batch back")
    H.eq(state.ui_refresh_depth, 0)
  end },

  { "regions and MIDI sends are created only when the plan asks", function()
    local _, proj, snap, track = scene()
    local p = make_plan(snap)
    p.operations[#p.operations + 1] = { op = "create_region", temp_id = "rgn",
      name = "QLabs Candidate 01", start_qn = 0.0, end_qn = 8.0 }
    p.operations[#p.operations + 1] = { op = "create_midi_send", temp_id = "snd",
      from_track = "chords", to_track_guid = track.guid }
    p.expected_outputs = json.array({})
    local res = H.ok(transactions.stage(proj, p, {}))
    H.eq(#res.regions, 1)
    H.eq(#res.sends, 1)
    H.eq(#proj.markers, 1)
  end },

  { "a send to a non-existent track is refused", function()
    local _, proj, snap = scene()
    local p = make_plan(snap)
    p.operations[#p.operations + 1] = { op = "create_midi_send", from_track = "chords",
      to_track_guid = "{00000000-0000-0000-0000-000000000000}" }
    local tracks_before = #proj.tracks
    local _, err = transactions.stage(proj, p, {})
    H.err_code(err, protocol.ERR.INVALID_EDIT_PLAN)
    H.eq(#proj.tracks, tracks_before)
  end },

  ------------------------------------------------------------------- discard --
  { "discard removes only objects with matching tags", function()
    local state, proj, snap = scene()
    H.ok(transactions.stage(proj, make_plan(snap), {}))

    -- A second candidate under a different transaction, plus an untagged track
    -- with an identical name.
    local snap2 = H.ok(snapshot.build(proj, {}))
    local p2 = make_plan(snap2, { transaction_id = "tx-0002", plan_id = "plan-0002",
      candidate_id = "cand-0002", undo_label = "QLabs MCP: Stage candidate cand-000-2" })
    p2.preconditions = json.array({})
    H.ok(transactions.stage(proj, p2, {}))
    local decoy = state:add_track(proj, "QLabs Candidate 01")

    local tracks_before = #proj.tracks
    local res = H.ok(transactions.discard(proj, "tx-0001"))
    H.eq(res.removed_tracks, 2)
    H.eq(res.removed_items, 1)
    H.eq(#proj.tracks, tracks_before - 2)

    -- The decoy and the other transaction survive.
    local still_there = false
    for i = 1, #proj.tracks do if proj.tracks[i] == decoy then still_there = true end end
    H.ok(still_there, "an identically named untagged track must never be deleted")
    H.eq(#tagging.collect_owned(proj, "tx-0002").tracks, 2)
    H.eq(#tagging.collect_owned(proj, "tx-0001").tracks, 0)
    H.eq(state.ui_refresh_depth, 0)
    H.eq(tagging.find_staged(proj, "tx-0001"), nil)
  end },

  { "discard keeps a generated track that holds foreign items", function()
    local state, proj, snap = scene()
    H.ok(transactions.stage(proj, make_plan(snap), {}))
    local owned = tagging.collect_owned(proj, "tx-0001")
    local chords
    for _, e in ipairs(owned.tracks) do
      if e.tags.QLABS_TEMP_ID == "chords" then chords = e.obj end
    end
    H.ok(chords)
    state:add_midi_item(chords, 8, 12) -- an untagged user item

    local res = H.ok(transactions.discard(proj, "tx-0001"))
    H.eq(res.removed_tracks, 1, "only the empty folder track is removed")
    H.eq(res.retained_tracks, 1)
    H.eq(#res.warnings, 1)
    H.eq(res.warnings[1].code, "track_retained")
  end },

  { "discard of an unknown transaction is refused", function()
    local _, proj = scene()
    H.err_code(select(2, transactions.discard(proj, "tx-nope")),
      protocol.ERR.TRANSACTION_NOT_FOUND)
    H.err_code(select(2, transactions.discard(proj, "../etc/passwd")),
      protocol.ERR.MALFORMED_REQUEST)
  end },

  -------------------------------------------------------------------- commit --
  { "commit flips only matching preview objects", function()
    local state, proj, snap = scene()
    H.ok(transactions.stage(proj, make_plan(snap), {}))
    local snap2 = H.ok(snapshot.build(proj, {}))
    local p2 = make_plan(snap2, { transaction_id = "tx-0002", plan_id = "plan-0002",
      candidate_id = "cand-0002", undo_label = "QLabs MCP: Stage candidate cand-000-2" })
    p2.preconditions = json.array({})
    H.ok(transactions.stage(proj, p2, {}))

    local res = H.ok(transactions.commit(proj, "tx-0001"))
    H.eq(res.status, "committed")
    H.eq(res.committed_tracks, 2)
    H.eq(res.committed_items, 1)
    H.eq(res.committed_takes, 1)

    for _, e in ipairs(tagging.collect_owned(proj, "tx-0001").tracks) do
      H.eq(tagging.get_tag("track", e.obj, "QLABS_STATUS"), "committed")
    end
    for _, e in ipairs(tagging.collect_owned(proj, "tx-0002").tracks) do
      H.eq(tagging.get_tag("track", e.obj, "QLABS_STATUS"), "preview",
        "the other transaction must be untouched")
    end
    H.eq(tagging.get_proj_state(proj, tagging.PROJ_KEYS.LAST_COMMITTED), "tx-0001")
    H.eq(state.ui_refresh_depth, 0)
  end },

  { "commit is idempotent and reports zero further changes", function()
    local _, proj, snap = scene()
    H.ok(transactions.stage(proj, make_plan(snap), {}))
    H.ok(transactions.commit(proj, "tx-0001"))
    local again = H.ok(transactions.commit(proj, "tx-0001"))
    H.eq(again.committed_tracks, 0, "already-committed objects must not be touched again")
    H.eq(again.inspected_tracks, 2)
  end },

  { "commit unmutes preview-muted objects and clears the marker tag", function()
    local _, proj, snap = scene()
    H.ok(transactions.stage(proj, make_plan(snap), {}))
    local owned = tagging.collect_owned(proj, "tx-0001")
    local muted_track
    for _, e in ipairs(owned.tracks) do
      if e.tags.QLABS_PREVIEW_MUTED == "1" then muted_track = e.obj end
    end
    H.ok(muted_track, "the plan muted a track")
    H.eq(muted_track.values.B_MUTE, 1)
    H.ok(transactions.commit(proj, "tx-0001"))
    H.eq(muted_track.values.B_MUTE, 0)
    H.eq(tagging.get_tag("track", muted_track, "QLABS_PREVIEW_MUTED"), nil)
  end },

  { "commit of an unknown transaction is refused", function()
    local _, proj = scene()
    H.err_code(select(2, transactions.commit(proj, "tx-nope")),
      protocol.ERR.TRANSACTION_NOT_FOUND)
  end },

  ---------------------------------------------------------------------- undo --
  { "one undo restores the prior state", function()
    local _, proj, snap, _, _, take = scene()
    local tracks_before = #proj.tracks
    local sig_before = take_signature(take)
    H.ok(transactions.stage(proj, make_plan(snap), {}))
    H.eq(#proj.tracks, tracks_before + 2)

    local res = H.ok(transactions.undo_last(proj, {}))
    H.eq(res.undone, true)
    H.eq(res.transaction_id, "tx-0001")
    H.eq(#proj.tracks, tracks_before, "one undo must remove every generated track")
    H.eq(take_signature(take), sig_before, "the source must be unchanged")
    H.eq(#tagging.collect_owned(proj, "tx-0001").tracks, 0)
    H.eq(tagging.get_last_transaction(proj), nil)
  end },

  { "undo refuses when the top entry is not ours", function()
    local state, proj, snap = scene()
    H.ok(transactions.stage(proj, make_plan(snap), {}))
    -- The user performs an unrelated action afterwards.
    _G.reaper.Undo_BeginBlock2(proj)
    state:add_track(proj, "User Track")
    _G.reaper.Undo_EndBlock2(proj, "User: add track", -1)

    local _, err = transactions.undo_last(proj, {})
    H.err_code(err, protocol.ERR.UNDO_NOT_OWNED)
    H.eq(err.details.top_undo_entry, "User: add track")
    H.eq(_G.reaper.Undo_CanUndo2(proj), "User: add track", "nothing was undone")
  end },

  { "undo refuses a transaction id that is not the last one", function()
    local _, proj, snap = scene()
    H.ok(transactions.stage(proj, make_plan(snap), {}))
    local _, err = transactions.undo_last(proj, { transaction_id = "tx-other" })
    H.err_code(err, protocol.ERR.UNDO_NOT_OWNED)
  end },

  { "undo refuses when there is no owned record", function()
    local _, proj = scene()
    H.err_code(select(2, transactions.undo_last(proj, {})), protocol.ERR.UNDO_NOT_OWNED)
  end },

  { "undo refuses when the recorded label lost its owned prefix", function()
    local state, proj = scene()
    tagging.set_last_transaction(proj, { transaction_id = "tx-x", undo_label = "Rogue label",
      kind = "stage" })
    _G.reaper.Undo_BeginBlock2(proj)
    state:add_track(proj, "x")
    _G.reaper.Undo_EndBlock2(proj, "Rogue label", -1)
    local _, err = transactions.undo_last(proj, {})
    H.err_code(err, protocol.ERR.UNDO_NOT_OWNED)
  end },
}
