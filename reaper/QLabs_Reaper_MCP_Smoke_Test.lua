--[[
  QLabs_Reaper_MCP_Smoke_Test.lua
  Guarded, in-REAPER end-to-end smoke test for the QLabs MCP bridge.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT

  READ THIS BEFORE RUNNING.

  This script WRITES to the active REAPER project. It creates its own clearly
  marked material, stages a fixture candidate against it, and then undoes
  everything it did. It refuses to run against a project that has been saved to
  disk or that has unsaved changes, unless you deliberately flip
  ALLOW_MODIFY_THIS_PROJECT to true below.

  Recommended use: File > New Project, then run this script.

  Results are printed to the ReaScript console, one PASS/FAIL line per step.
--]]

--==============================================================================
-- USER SAFETY FLAG
--
-- Leave this false. Set it to true ONLY if you understand that this script will
-- create and delete tracks and items in the project that is currently open.
--==============================================================================
local ALLOW_MODIFY_THIS_PROJECT = false
--==============================================================================

local TEST_MARKER = "QLABS SMOKE TEST -- SAFE TO DELETE"

--------------------------------------------------------------------------------
-- Module loading
--------------------------------------------------------------------------------

local SCRIPT_PATH = debug.getinfo(1, "S").source
if SCRIPT_PATH:sub(1, 1) == "@" then SCRIPT_PATH = SCRIPT_PATH:sub(2) end
local SCRIPT_DIR = SCRIPT_PATH:match("^(.*)[/\\][^/\\]*$") or "."

-- The installation directory is the one that actually holds lib/. Normally that
-- is this script's own directory; when the script has been copied elsewhere we
-- fall back to the installed location under REAPER's resource path. Never
-- hardcoded: portable REAPER installs are supported.
local function qlabs_holds_lib(dir)
  local probe = io.open(dir .. "/lib/util.lua", "r")
  if probe then
    probe:close()
    return true
  end
  return false
end

local INSTALL_DIR
local TRIED = { SCRIPT_DIR }
if reaper and reaper.GetResourcePath then
  TRIED[#TRIED + 1] = reaper.GetResourcePath() .. "/Scripts/QLabs-Reaper-MCP"
end
for _, dir in ipairs(TRIED) do
  if not INSTALL_DIR and qlabs_holds_lib(dir) then INSTALL_DIR = dir end
end
INSTALL_DIR = INSTALL_DIR or SCRIPT_DIR

package.path = INSTALL_DIR .. "/lib/?.lua;" .. package.path

local ok_load, load_err = pcall(function()
  _G.QLABS_util = require("util")
  _G.QLABS_json = require("json")
  _G.QLABS_protocol = require("protocol")
  _G.QLABS_snapshot = require("snapshot")
  _G.QLABS_tagging = require("tagging")
  _G.QLABS_transactions = require("transactions")
end)
if not ok_load then
  reaper.ShowMessageBox("QLabs smoke test could not load lib/: " .. tostring(load_err),
    "QLabs REAPER MCP", 0)
  return
end

local util = _G.QLABS_util
local json = _G.QLABS_json
local protocol = _G.QLABS_protocol
local snapshot = _G.QLABS_snapshot
local tagging = _G.QLABS_tagging
local transactions = _G.QLABS_transactions

--------------------------------------------------------------------------------
-- Reporting
--------------------------------------------------------------------------------

local report = {}
local passed, failed = 0, 0

local function say(fmt, ...)
  local line = select("#", ...) > 0 and string.format(fmt, ...) or fmt
  report[#report + 1] = line
  reaper.ShowConsoleMsg(line .. "\n")
end

local function step(name, ok, detail)
  if ok then
    passed = passed + 1
    say("PASS  %s%s", name, detail and ("  -- " .. detail) or "")
  else
    failed = failed + 1
    say("FAIL  %s%s", name, detail and ("  -- " .. detail) or "")
  end
  return ok
end

local function abort(reason)
  say("")
  say("ABORTED: %s", reason)
  say("")
  reaper.ShowMessageBox("QLabs smoke test aborted.\n\n" .. reason, "QLabs REAPER MCP", 0)
end

say("================================================================")
say("QLabs REAPER MCP bridge -- in-REAPER smoke test")
say("bridge %s / protocol %s / REAPER %s", protocol.BRIDGE_VERSION,
  protocol.PROTOCOL_VERSION, tostring(reaper.GetAppVersion()))
say("================================================================")

--------------------------------------------------------------------------------
-- Safety gate
--------------------------------------------------------------------------------

local proj, proj_path = snapshot.active_project()
if proj == nil then
  abort("There is no active REAPER project.")
  return
end

local is_saved_project = type(proj_path) == "string" and proj_path ~= ""
local is_dirty = false
if reaper.IsProjectDirty then
  is_dirty = reaper.IsProjectDirty(proj) ~= 0
end

if not ALLOW_MODIFY_THIS_PROJECT then
  if is_saved_project or is_dirty then
    abort(table.concat({
      "This project is " .. (is_saved_project and "saved to disk" or "modified") .. ":",
      is_saved_project and ("  " .. proj_path) or "  (unsaved changes are present)",
      "",
      "The smoke test refuses to modify a real project.",
      "",
      "Either:",
      "  1. File > New Project, then run this script there, or",
      "  2. Open " .. SCRIPT_PATH,
      "     and set ALLOW_MODIFY_THIS_PROJECT = true.",
    }, "\n"))
    return
  end
  say("safety gate       : project is untitled and clean -- proceeding")
else
  say("safety gate       : ALLOW_MODIFY_THIS_PROJECT is true -- proceeding on the user's word")
end

local verr = protocol.check_reaper_version(reaper.GetAppVersion())
if verr then
  abort(verr.message)
  return
end

--------------------------------------------------------------------------------
-- Step 1: create clearly marked test material
--------------------------------------------------------------------------------

say("")
say("---- setup ----")

local SETUP_LABEL = transactions.UNDO_PREFIX .. "Smoke test setup"
local tracks_at_start = reaper.CountTracks(proj)

local test_track, test_item, test_take
local setup_ok, setup_err = pcall(function()
  reaper.Undo_BeginBlock2(proj)
  reaper.PreventUIRefresh(1)
  local index = reaper.CountTracks(proj)
  if reaper.InsertTrackInProject then
    reaper.InsertTrackInProject(proj, index, 0)
  else
    reaper.InsertTrackAtIndex(index, false)
  end
  test_track = reaper.GetTrack(proj, index)
  reaper.GetSetMediaTrackInfo_String(test_track, "P_NAME", TEST_MARKER, true)
  test_item = reaper.CreateNewMIDIItemInProj(test_track, 0.0, 8.0, true)
  test_take = reaper.GetActiveTake(test_item)
  -- A short C major scale fragment: four quarter notes.
  local pitches = { 60, 62, 64, 65 }
  for i = 1, #pitches do
    local sp = reaper.MIDI_GetPPQPosFromProjQN(test_take, (i - 1) * 1.0)
    local ep = reaper.MIDI_GetPPQPosFromProjQN(test_take, i * 1.0)
    reaper.MIDI_InsertNote(test_take, true, false, sp, ep, 0, pitches[i], 96, true)
  end
  reaper.MIDI_Sort(test_take)
  reaper.PreventUIRefresh(-1)
  reaper.Undo_EndBlock2(proj, SETUP_LABEL, -1)
  reaper.UpdateArrange()
end)

if not setup_ok then
  pcall(function() reaper.PreventUIRefresh(-1) end)
  pcall(function() reaper.Undo_EndBlock2(proj, SETUP_LABEL, -1) end)
  abort("could not create the test material: " .. tostring(setup_err))
  return
end

step("created marked test material", test_take ~= nil,
  string.format("track %q with 4 notes over 8 QN", TEST_MARKER))

local _, setup_notes = reaper.MIDI_CountEvts(test_take)
step("test material holds 4 notes", setup_notes == 4, tostring(setup_notes) .. " notes")

--------------------------------------------------------------------------------
-- Step 2: verify inspection
--------------------------------------------------------------------------------

say("")
say("---- inspection ----")

-- Select only our item so source resolution is unambiguous.
for i = 0, reaper.CountMediaItems(proj) - 1 do
  local it = reaper.GetMediaItem(proj, i)
  if it then reaper.SetMediaItemInfo_Value(it, "B_UISEL", it == test_item and 1 or 0) end
end

local snap, snap_err = snapshot.build(proj, {
  source_mode = "selected_item",
  note_scope = "all",
  reaper_version = reaper.GetAppVersion(),
})

if not step("snapshot built", snap ~= nil, snap_err and (snap_err.code .. ": " .. snap_err.message)) then
  abort("inspection failed; nothing was staged")
  return
end

step("snapshot resolved our item", snap.item_guid == snapshot.item_guid(test_item),
  tostring(snap.item_guid))
step("snapshot reports 4 notes", snap.note_count == 4, tostring(snap.note_count))
step("snapshot has a project uuid", type(snap.project_uuid) == "string" and #snap.project_uuid > 0,
  tostring(snap.project_uuid))
step("snapshot hash is well formed",
  type(snap.snapshot_hash) == "string" and snap.snapshot_hash:sub(1, 8) == "fnv1a64:",
  tostring(snap.snapshot_hash))
step("note positions are in project QN",
  math.abs(snap.notes[1].start_qn - 0.0) < 1e-6 and math.abs(snap.notes[4].end_qn - 4.0) < 1e-6,
  string.format("%.4f .. %.4f", snap.notes[1].start_qn, snap.notes[4].end_qn))

local source_midi_hash_before = snap.midi_hash

--------------------------------------------------------------------------------
-- Step 3: stage a fixture candidate
--------------------------------------------------------------------------------

say("")
say("---- staging ----")

local TX = "smoke-" .. tostring(util.now())
local plan = {
  plan_id = "smoke-plan",
  candidate_id = "smoke-candidate",
  transaction_id = TX,
  base_snapshot_id = snap.snapshot_id,
  base_snapshot_hash = snap.snapshot_hash,
  project_uuid = snap.project_uuid,
  knowledge_version = "smoke-fixture",
  undo_label = transactions.UNDO_PREFIX .. "Stage candidate smoke",
  operations = json.array({
    { op = "create_folder_track", temp_id = "folder",
      name = "QLabs Candidate 01 -- " .. TEST_MARKER,
      tags = json.array({ json.array({ "QLABS_ROLE", "candidate_folder" }) }) },
    { op = "create_track", temp_id = "chords", parent = "folder",
      name = "Chords -- " .. TEST_MARKER,
      tags = json.array({ json.array({ "QLABS_ROLE", "harmonic_bed" }) }) },
    { op = "create_midi_item", temp_id = "item", track = "chords",
      start_qn = snap.item_position_qn, end_qn = snap.item_end_qn, muted = true },
    { op = "insert_notes", item = "item", notes = json.array({
      { start_qn = snap.item_position_qn, end_qn = snap.item_position_qn + 4.0,
        pitch = 48, velocity = 90, channel = 0, spelling = "C3" },
      { start_qn = snap.item_position_qn, end_qn = snap.item_position_qn + 4.0,
        pitch = 52, velocity = 90, channel = 0, spelling = "E3" },
      { start_qn = snap.item_position_qn, end_qn = snap.item_position_qn + 4.0,
        pitch = 55, velocity = 90, channel = 0, spelling = "G3" },
    }) },
    { op = "set_track_mute", track = "chords", muted = true },
  }),
  preconditions = json.array({
    { type = "project_uuid", value = snap.project_uuid },
    { type = "item_guid_exists", value = snap.item_guid },
    { type = "take_guid_exists", value = snap.take_guid },
    { type = "midi_hash", value = snap.midi_hash },
    { type = "tempo_map_hash", value = snap.tempo_map_hash },
  }),
  expected_outputs = json.array({
    { temp_id = "folder", kind = "folder_track" },
    { temp_id = "chords", kind = "track" },
    { temp_id = "item", kind = "midi_item", note_count = 3 },
  }),
}

local tracks_before_stage = reaper.CountTracks(proj)
local staged, stage_err = transactions.stage(proj, plan, {
  source_mode = "selected_item", note_scope = "all",
  reaper_version = reaper.GetAppVersion(),
})

if not step("stage_candidate succeeded", staged ~= nil,
  stage_err and (stage_err.code .. ": " .. stage_err.message)) then
  say("")
  say("Attempting to clean up the test material...")
  if reaper.Undo_CanUndo2(proj) == SETUP_LABEL then reaper.Undo_DoUndo2(proj) end
  abort("staging failed")
  return
end

step("two tracks were created", reaper.CountTracks(proj) == tracks_before_stage + 2,
  string.format("%d -> %d", tracks_before_stage, reaper.CountTracks(proj)))
step("one item was created with 3 notes",
  #staged.items == 1 and staged.items[1].note_count == 3,
  string.format("%d item(s)", #staged.items))
step("UI refresh bookkeeping is balanced", transactions.ui_refresh_depth == 0,
  tostring(transactions.ui_refresh_depth))

--------------------------------------------------------------------------------
-- Step 4: confirm the source is unchanged
--------------------------------------------------------------------------------

say("")
say("---- source integrity ----")

local _, source_notes_after = reaper.MIDI_CountEvts(test_take)
step("source take still holds 4 notes", source_notes_after == 4, tostring(source_notes_after))

local live_notes = snapshot.read_take_notes(test_take)
local source_midi_hash_after = util.hash_hex(snapshot.midi_canonical(live_notes))
step("source MIDI hash is unchanged", source_midi_hash_after == source_midi_hash_before,
  source_midi_hash_after)
step("source item bounds are unchanged",
  math.abs(reaper.GetMediaItemInfo_Value(test_item, "D_POSITION") - snap.item_position_seconds) < 1e-9
  and math.abs(reaper.GetMediaItemInfo_Value(test_item, "D_LENGTH") - snap.item_length_seconds) < 1e-9)
step("source item carries no QLabs ownership tag",
  not tagging.is_owned("item", test_item))

--------------------------------------------------------------------------------
-- Step 5: confirm generated objects are tagged
--------------------------------------------------------------------------------

say("")
say("---- ownership tags ----")

local owned = tagging.collect_owned(proj, TX)
step("2 generated tracks carry matching tags", #owned.tracks == 2, tostring(#owned.tracks))
step("1 generated item carries matching tags", #owned.items == 1, tostring(#owned.items))
step("1 generated take carries matching tags", #owned.takes == 1, tostring(#owned.takes))

local tags_ok = true
local tag_detail = ""
for _, entry in ipairs(owned.tracks) do
  for _, key in ipairs({ "QLABS_OWNER", "QLABS_TRANSACTION_ID", "QLABS_CANDIDATE_ID",
    "QLABS_STATUS", "QLABS_SOURCE_SNAPSHOT", "QLABS_KNOWLEDGE_VERSION" }) do
    if entry.tags[key] == nil then
      tags_ok = false
      tag_detail = "missing " .. key
    end
  end
end
step("every required P_EXT tag is present on generated tracks", tags_ok, tag_detail)
step("generated objects are in preview status",
  owned.tracks[1] and owned.tracks[1].tags.QLABS_STATUS == "preview")
step("the staged transaction is recorded in project state",
  tagging.find_staged(proj, TX) ~= nil)

--------------------------------------------------------------------------------
-- Step 6: confirm one undo restores the prior state
--------------------------------------------------------------------------------

say("")
say("---- undo ----")

step("the top undo entry is our owned transaction",
  reaper.Undo_CanUndo2(proj) == plan.undo_label, tostring(reaper.Undo_CanUndo2(proj)))

local undone, undo_err = transactions.undo_last(proj, { transaction_id = TX })
step("undo_last_generation succeeded", undone ~= nil,
  undo_err and (undo_err.code .. ": " .. undo_err.message))

step("one undo removed every generated track",
  reaper.CountTracks(proj) == tracks_before_stage,
  string.format("%d tracks (expected %d)", reaper.CountTracks(proj), tracks_before_stage))
step("no objects carry the transaction tag any more",
  #tagging.collect_owned(proj, TX).tracks == 0)

local _, notes_after_undo = reaper.MIDI_CountEvts(test_take)
step("the source take survived the undo untouched", notes_after_undo == 4,
  tostring(notes_after_undo))

--------------------------------------------------------------------------------
-- Step 7: clean up our own test material
--------------------------------------------------------------------------------

say("")
say("---- cleanup ----")

if reaper.Undo_CanUndo2(proj) == SETUP_LABEL then
  reaper.Undo_DoUndo2(proj)
  reaper.UpdateArrange()
  step("test material removed by undoing the setup block",
    reaper.CountTracks(proj) == tracks_at_start,
    string.format("%d tracks (started with %d)", reaper.CountTracks(proj), tracks_at_start))
else
  step("test material removed", false,
    string.format("the setup undo entry is no longer on top (top is %q); "
      .. "delete the %q track manually",
      tostring(reaper.Undo_CanUndo2(proj)), TEST_MARKER))
end

--------------------------------------------------------------------------------
-- Summary
--------------------------------------------------------------------------------

say("")
say("================================================================")
say("RESULT: %d passed, %d failed", passed, failed)
say("================================================================")

reaper.ShowMessageBox(
  string.format("QLabs smoke test finished.\n\n%d passed, %d failed.\n\n"
    .. "See the ReaScript console for the per-step report.", passed, failed),
  "QLabs REAPER MCP", 0)
