--[[
  snapshot.lua -- source resolution, MIDI reading, snapshot construction,
  canonical hashing and precondition checking.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT

  Hashing
  -------
  Every hash in this file is FNV-1a 64-bit over a canonical ASCII string, and is
  emitted as `fnv1a64:` followed by 16 lowercase hex digits. The canonical
  strings are specified byte-for-byte in docs/IPC_WIRE.md; the layouts below are
  the normative implementation of that spec. FNV-1a is a NON-CRYPTOGRAPHIC hash:
  it detects accidental change, not adversarial substitution. See
  docs/REAPER_BRIDGE.md for the collision-resistance caveat.
--]]

local util = require("util")
local json = require("json")
local protocol = require("protocol")
local tagging = require("tagging")

local M = {}

--- Rendering of an absent string inside a canonical hash string.
local NULL_TOKEN = "null"

local function s_or_null(v)
  if v == nil or v == json.null or v == "" then return NULL_TOKEN end
  return tostring(v)
end

local function f6(x)
  local s = util.fmt6(x)
  if s == nil then error("snapshot: non-representable number " .. tostring(x), 0) end
  return s
end

local function d(x)
  local s = util.fmtint(x)
  if s == nil then error("snapshot: non-integer value " .. tostring(x), 0) end
  return s
end

local function b01(v)
  return v and "1" or "0"
end

--------------------------------------------------------------------------------
-- Project access
--------------------------------------------------------------------------------

--- Returns the active project pointer and its file path.
--- Returns nil plus a structured error when there is no active project.
function M.active_project()
  if not reaper.EnumProjects then
    return nil, protocol.err(protocol.ERR.NO_ACTIVE_PROJECT, "host does not expose EnumProjects")
  end
  local proj, path = reaper.EnumProjects(-1, "")
  if proj == nil then
    return nil, protocol.err(protocol.ERR.NO_ACTIVE_PROJECT, "no active REAPER project")
  end
  return proj, path or ""
end

--- Project display name, best effort.
function M.project_name(proj, path)
  if reaper.GetProjectName then
    local ok, name = pcall(reaper.GetProjectName, proj, "")
    if ok and type(name) == "string" and name ~= "" then return name end
  end
  if reaper.GetSetProjectInfo_String then
    local ok, _, name = pcall(reaper.GetSetProjectInfo_String, proj, "PROJECT_NAME", "", false)
    if ok and type(name) == "string" and name ~= "" then return name end
  end
  if type(path) == "string" and path ~= "" then return util.basename(path) end
  return ""
end

--- Normalises a REAPER GUID string to lowercase without braces, or nil.
function M.norm_guid(g)
  if type(g) ~= "string" or g == "" then return nil end
  return (g:gsub("^%s*{", ""):gsub("}%s*$", "")):lower()
end

local function track_guid(tr)
  if reaper.GetTrackGUID then
    local g = reaper.GetTrackGUID(tr)
    if g then return M.norm_guid(g) end
  end
  local ok, g = reaper.GetSetMediaTrackInfo_String(tr, "GUID", "", false)
  if ok then return M.norm_guid(g) end
  return nil
end

local function item_guid(item)
  local ok, g = reaper.GetSetMediaItemInfo_String(item, "GUID", "", false)
  if ok then return M.norm_guid(g) end
  return nil
end

local function take_guid(take)
  local ok, g = reaper.GetSetMediaItemTakeInfo_String(take, "GUID", "", false)
  if ok then return M.norm_guid(g) end
  return nil
end

M.track_guid = track_guid
M.item_guid = item_guid
M.take_guid = take_guid

--- Finds a media item by GUID using safe count-and-get iteration.
function M.find_item_by_guid(proj, guid)
  guid = M.norm_guid(guid)
  if not guid then return nil end
  local n = reaper.CountMediaItems(proj)
  for i = 0, n - 1 do
    local it = reaper.GetMediaItem(proj, i)
    if it and item_guid(it) == guid then return it end
  end
  return nil
end

--- Finds a take by GUID using safe count-and-get iteration.
function M.find_take_by_guid(proj, guid)
  guid = M.norm_guid(guid)
  if not guid then return nil end
  local n = reaper.CountMediaItems(proj)
  for i = 0, n - 1 do
    local it = reaper.GetMediaItem(proj, i)
    if it then
      local tc = reaper.CountTakes(it)
      for t = 0, tc - 1 do
        local tk = reaper.GetTake(it, t)
        if tk and take_guid(tk) == guid then return tk, it end
      end
    end
  end
  return nil
end

--- Finds a track by GUID using safe count-and-get iteration.
function M.find_track_by_guid(proj, guid)
  guid = M.norm_guid(guid)
  if not guid then return nil end
  local n = reaper.CountTracks(proj)
  for i = 0, n - 1 do
    local tr = reaper.GetTrack(proj, i)
    if tr and track_guid(tr) == guid then return tr end
  end
  return nil
end

--------------------------------------------------------------------------------
-- Source resolution
--------------------------------------------------------------------------------

--- Valid values for `payload.source_mode`.
M.SOURCE_MODES = { auto = true, active_editor = true, selected_item = true }

local function take_is_valid_midi(proj, take)
  if take == nil then return false end
  if reaper.ValidatePtr2 and not reaper.ValidatePtr2(proj, take, "MediaItem_Take*") then
    return false
  end
  if not reaper.TakeIsMIDI(take) then return false end
  return true
end

local function editor_take(proj)
  if not reaper.MIDIEditor_GetActive then return nil end
  local hwnd = reaper.MIDIEditor_GetActive()
  if hwnd == nil then return nil end
  local take = reaper.MIDIEditor_GetTake(hwnd)
  if not take_is_valid_midi(proj, take) then return nil end
  return take
end

--- Resolves the MIDI source in the order mandated by brief section 21:
--- 1. valid active MIDI editor take, 2. exactly one selected MIDI item with a
--- valid take, 3. structured error.
---
--- Returns `{ take, item, track, resolved_by }` or `nil, err`.
function M.resolve_source(proj, source_mode)
  source_mode = source_mode or "auto"
  if not M.SOURCE_MODES[source_mode] then
    return nil, protocol.err(protocol.ERR.MALFORMED_REQUEST,
      string.format("source_mode %q must be auto|active_editor|selected_item", tostring(source_mode)))
  end

  if source_mode == "auto" or source_mode == "active_editor" then
    local take = editor_take(proj)
    if take then
      local item = reaper.GetMediaItemTake_Item(take)
      return {
        take = take,
        item = item,
        track = item and reaper.GetMediaItem_Track(item) or nil,
        resolved_by = "active_editor",
      }
    end
    if source_mode == "active_editor" then
      return nil, protocol.err(protocol.ERR.NO_MIDI_SOURCE,
        "no active MIDI editor with a valid MIDI take")
    end
  end

  -- Selected-item path. Count first, then index; never hold indices across a
  -- mutation (this path performs none).
  local sel_count = reaper.CountSelectedMediaItems(proj)
  local candidates = {}
  for i = 0, sel_count - 1 do
    local item = reaper.GetSelectedMediaItem(proj, i)
    if item then
      local take = reaper.GetActiveTake(item)
      if take_is_valid_midi(proj, take) then
        candidates[#candidates + 1] = { item = item, take = take }
      end
    end
  end

  if #candidates == 0 then
    return nil, protocol.err(protocol.ERR.NO_MIDI_SOURCE,
      "no MIDI source: open a MIDI editor or select exactly one MIDI item",
      { selected_item_count = sel_count })
  end
  if #candidates > 1 then
    return nil, protocol.err(protocol.ERR.MULTIPLE_MIDI_SOURCES,
      string.format("%d selected MIDI items; select exactly one", #candidates),
      { midi_item_count = #candidates })
  end

  local c = candidates[1]
  return {
    take = c.take,
    item = c.item,
    track = reaper.GetMediaItem_Track(c.item),
    resolved_by = "selected_item",
  }
end

--------------------------------------------------------------------------------
-- MIDI reading
--------------------------------------------------------------------------------

local function note_sort_less(a, b)
  if a.start_ppq ~= b.start_ppq then return a.start_ppq < b.start_ppq end
  if a.pitch ~= b.pitch then return a.pitch < b.pitch end
  if a.channel ~= b.channel then return a.channel < b.channel end
  if a.end_ppq ~= b.end_ppq then return a.end_ppq < b.end_ppq end
  if a.velocity ~= b.velocity then return a.velocity < b.velocity end
  return a.raw_index < b.raw_index
end

--- Reads every note out of a MIDI take with safe count-and-get iteration.
--- Returns `notes` (deterministically sorted) plus `warnings`.
function M.read_take_notes(take)
  local warnings = {}
  local ok, notecnt = reaper.MIDI_CountEvts(take)
  if not ok then
    return {}, { { code = "midi_count_failed", message = "MIDI_CountEvts returned false" } }
  end
  notecnt = notecnt or 0
  local limit = protocol.LIMITS.MAX_READ_NOTES
  if notecnt > limit then
    warnings[#warnings + 1] = {
      code = "note_limit_reached",
      message = string.format("take holds %d notes; only the first %d were read", notecnt, limit),
    }
    notecnt = limit
  end
  local notes = {}
  for i = 0, notecnt - 1 do
    local got, selected, muted, startppq, endppq, chan, pitch, vel = reaper.MIDI_GetNote(take, i)
    if not got then break end
    notes[#notes + 1] = {
      raw_index = i,
      selected = selected and true or false,
      muted = muted and true or false,
      start_ppq = startppq + 0.0,
      end_ppq = endppq + 0.0,
      channel = math.tointeger(chan) or 0,
      pitch = math.tointeger(pitch) or 0,
      velocity = math.tointeger(vel) or 0,
    }
  end
  table.sort(notes, note_sort_less)
  return notes, warnings
end

--------------------------------------------------------------------------------
-- Canonical hash strings
--------------------------------------------------------------------------------

--- Canonical string for the take's MIDI content hash.
function M.midi_canonical(notes)
  local out = { "qlabs.midi.v1\n", d(#notes), "\n" }
  for i = 1, #notes do
    local n = notes[i]
    out[#out + 1] = table.concat({
      d(i - 1), f6(n.start_ppq), f6(n.end_ppq), d(n.channel), d(n.pitch), d(n.velocity), b01(n.muted),
    }, "|")
    out[#out + 1] = "\n"
  end
  return table.concat(out)
end

--- Canonical string for the note-selection hash.
function M.selection_canonical(notes)
  local out = { "qlabs.selection.v1\n", d(#notes), "\n" }
  for i = 1, #notes do
    out[#out + 1] = d(i - 1) .. "|" .. b01(notes[i].selected) .. "\n"
  end
  return table.concat(out)
end

--- Canonical string for the extracted note list hash (project-QN domain).
function M.note_list_canonical(notes)
  local out = { "qlabs.notes.v1\n", d(#notes), "\n" }
  for i = 1, #notes do
    local n = notes[i]
    out[#out + 1] = table.concat({
      d(i - 1), f6(n.start_qn), f6(n.end_qn), d(n.pitch), d(n.velocity), d(n.channel),
      b01(n.muted), b01(n.selected),
    }, "|")
    out[#out + 1] = "\n"
  end
  return table.concat(out)
end

--- Reads the project tempo/time-signature map into a normalised array.
--- When the project has no explicit markers a single synthetic marker carrying
--- the master tempo and the time signature at time zero is returned, so the
--- hash is still meaningful and stable.
function M.read_tempo_map(proj)
  local out = {}
  local n = 0
  if reaper.CountTempoTimeSigMarkers then
    n = reaper.CountTempoTimeSigMarkers(proj) or 0
  end
  for i = 0, n - 1 do
    local ok, timepos, _, _, bpm, num, den, linear = reaper.GetTempoTimeSigMarker(proj, i)
    if ok then
      local qn = 0.0
      if reaper.TimeMap2_timeToQN then qn = reaper.TimeMap2_timeToQN(proj, timepos) end
      out[#out + 1] = {
        index = i,
        time_seconds = (timepos or 0) + 0.0,
        qn = (qn or 0) + 0.0,
        bpm = (bpm or 0) + 0.0,
        timesig_num = math.tointeger(num) or 0,
        timesig_den = math.tointeger(den) or 0,
        linear = (linear == true or linear == 1) and true or false,
      }
    end
  end
  if #out == 0 then
    local bpm = reaper.Master_GetTempo and reaper.Master_GetTempo() or 120.0
    local num, den = 4, 4
    if reaper.TimeMap_GetTimeSigAtTime then
      local a, b = reaper.TimeMap_GetTimeSigAtTime(proj, 0)
      num = math.tointeger(a) or 4
      den = math.tointeger(b) or 4
    end
    out[1] = {
      index = 0, time_seconds = 0.0, qn = 0.0, bpm = bpm + 0.0,
      timesig_num = num, timesig_den = den, linear = false, synthetic = true,
    }
  end
  return out
end

--- Canonical string for the tempo-map hash.
function M.tempo_canonical(markers)
  local out = { "qlabs.tempo.v1\n", d(#markers), "\n" }
  for i = 1, #markers do
    local m = markers[i]
    out[#out + 1] = table.concat({
      d(i - 1), f6(m.time_seconds), f6(m.qn), f6(m.bpm),
      d(m.timesig_num), d(m.timesig_den), b01(m.linear),
    }, "|")
    out[#out + 1] = "\n"
  end
  return table.concat(out)
end

--- Canonical string for the time-signature-map hash: markers with an explicit
--- time signature only, tempo deliberately excluded.
function M.timesig_canonical(markers)
  local rows = {}
  for i = 1, #markers do
    local m = markers[i]
    if m.timesig_num and m.timesig_num > 0 and m.timesig_den and m.timesig_den > 0 then
      rows[#rows + 1] = m
    end
  end
  local out = { "qlabs.timesig.v1\n", d(#rows), "\n" }
  for i = 1, #rows do
    local m = rows[i]
    out[#out + 1] = table.concat({ d(i - 1), f6(m.qn), d(m.timesig_num), d(m.timesig_den) }, "|")
    out[#out + 1] = "\n"
  end
  return table.concat(out)
end

--- Canonical string for the snapshot hash. Deliberately excludes the timestamp,
--- bridge version, REAPER version and snapshot id so the value is reproducible
--- from project state alone.
function M.snapshot_canonical(s)
  local lines = {
    "qlabs.snapshot.v1",
    "project_uuid=" .. s_or_null(s.project_uuid),
    "state_change_count=" .. d(s.project_state_change_count),
    "track_guid=" .. s_or_null(s.track_guid),
    "item_guid=" .. s_or_null(s.item_guid),
    "take_guid=" .. s_or_null(s.take_guid),
    "item_position_seconds=" .. f6(s.item_position_seconds),
    "item_length_seconds=" .. f6(s.item_length_seconds),
    "item_position_qn=" .. f6(s.item_position_qn),
    "item_length_qn=" .. f6(s.item_length_qn),
    "is_loop_source=" .. b01(s.is_loop_source),
    "note_scope=" .. s_or_null(s.note_scope),
    "extraction_mode=" .. s_or_null(s.extraction_mode),
    "extraction_channel=" .. d(s.extraction_channel or -1),
    "midi_hash=" .. s_or_null(s.midi_hash),
    "tempo_map_hash=" .. s_or_null(s.tempo_map_hash),
    "timesig_map_hash=" .. s_or_null(s.timesig_map_hash),
    "note_selection_hash=" .. s_or_null(s.note_selection_hash),
    "note_list_hash=" .. s_or_null(s.note_list_hash),
    "note_count=" .. d(s.note_count),
  }
  return table.concat(lines, "\n") .. "\n"
end

--------------------------------------------------------------------------------
-- Snapshot construction
--------------------------------------------------------------------------------

--- Valid values for `payload.note_scope`.
M.NOTE_SCOPES = { selected_or_all = true, selected_only = true, all = true }

--- Valid values for `payload.melody_extraction.mode`. The bridge only applies
--- the two modes that are pure filters (`selected_notes`, `midi_channel`) and
--- the ambiguity gate for `monophonic_voice`; the remaining modes are analysis
--- decisions made server-side and are echoed back untouched.
M.EXTRACTION_MODES = {
  auto = true, selected_notes = true, highest_voice = true, lowest_voice = true,
  midi_channel = true, monophonic_voice = true, all_notes_as_harmony = true,
}

local function max_polyphony(notes)
  local best = 0
  for i = 1, #notes do
    local c = 0
    local t = notes[i].start_qn
    for j = 1, #notes do
      local n = notes[j]
      if n.start_qn <= t and n.end_qn > t then c = c + 1 end
    end
    if c > best then best = c end
  end
  return best
end

--- Builds the full snapshot for a resolved source.
---
--- opts:
---   source_mode        -- "auto" | "active_editor" | "selected_item"
---   note_scope         -- "selected_or_all" | "selected_only" | "all"
---   extraction_mode    -- see EXTRACTION_MODES
---   extraction_channel -- 0..15, required by mode "midi_channel"
---   reaper_version     -- string for provenance
---
--- Returns the snapshot table, or nil plus a structured error.
function M.build(proj, opts)
  opts = opts or {}
  local note_scope = opts.note_scope or "selected_or_all"
  if not M.NOTE_SCOPES[note_scope] then
    return nil, protocol.err(protocol.ERR.MALFORMED_REQUEST,
      string.format("note_scope %q must be selected_or_all|selected_only|all", tostring(note_scope)))
  end
  local mode = opts.extraction_mode or "auto"
  if not M.EXTRACTION_MODES[mode] then
    return nil, protocol.err(protocol.ERR.MALFORMED_REQUEST,
      string.format("melody_extraction.mode %q is not recognised", tostring(mode)))
  end
  local channel = opts.extraction_channel
  if channel ~= nil and (math.tointeger(channel) == nil or channel < 0 or channel > 15) then
    return nil, protocol.err(protocol.ERR.MALFORMED_REQUEST,
      "melody_extraction.channel must be an integer 0..15")
  end

  local src, err = M.resolve_source(proj, opts.source_mode)
  if not src then return nil, err end
  if src.item == nil then
    return nil, protocol.err(protocol.ERR.SOURCE_ITEM_MISSING, "resolved take has no media item")
  end

  local warnings = {}
  local project_uuid, minted = tagging.ensure_project_uuid(proj)
  if minted then
    warnings[#warnings + 1] = { code = "project_uuid_minted",
      message = "a persistent QLabs project UUID was created in project extended state" }
  end

  local item = src.item
  local take = src.take
  local pos_sec = reaper.GetMediaItemInfo_Value(item, "D_POSITION") + 0.0
  local len_sec = reaper.GetMediaItemInfo_Value(item, "D_LENGTH") + 0.0
  local pos_qn = reaper.TimeMap2_timeToQN(proj, pos_sec) + 0.0
  local end_qn = reaper.TimeMap2_timeToQN(proj, pos_sec + len_sec) + 0.0
  local loopsrc = (reaper.GetMediaItemInfo_Value(item, "B_LOOPSRC") or 0) ~= 0

  local raw_notes, read_warnings = M.read_take_notes(take)
  for i = 1, #read_warnings do warnings[#warnings + 1] = read_warnings[i] end

  local midi_hash = util.hash_hex(M.midi_canonical(raw_notes))
  local selection_hash = util.hash_hex(M.selection_canonical(raw_notes))

  local markers = M.read_tempo_map(proj)
  local tempo_hash = util.hash_hex(M.tempo_canonical(markers))
  local timesig_hash = util.hash_hex(M.timesig_canonical(markers))

  -- Apply scope / extraction filters.
  local any_selected = false
  for i = 1, #raw_notes do
    if raw_notes[i].selected then any_selected = true break end
  end
  local effective_scope = note_scope
  if mode == "selected_notes" then effective_scope = "selected_only" end
  local want_selected_only = (effective_scope == "selected_only")
    or (effective_scope == "selected_or_all" and any_selected)

  local assumptions = {}
  if effective_scope == "selected_or_all" then
    assumptions[#assumptions + 1] = any_selected
      and "used the selected notes because a note selection exists"
      or "no notes were selected; used every note in the take"
  end
  if mode ~= "auto" and mode ~= "selected_notes" and mode ~= "midi_channel" then
    assumptions[#assumptions + 1] =
      string.format("melody extraction mode %q is applied server-side; the bridge returned every in-scope note", mode)
  end

  if mode == "midi_channel" and channel == nil then
    return nil, protocol.err(protocol.ERR.AMBIGUOUS_MELODY,
      "melody_extraction.mode is midi_channel but no channel was supplied")
  end

  local extracted = {}
  for i = 1, #raw_notes do
    local n = raw_notes[i]
    local keep = true
    if want_selected_only and not n.selected then keep = false end
    if keep and mode == "midi_channel" and n.channel ~= channel then keep = false end
    if keep then
      local start_qn = reaper.MIDI_GetProjQNFromPPQPos(take, n.start_ppq) + 0.0
      local e_qn = reaper.MIDI_GetProjQNFromPPQPos(take, n.end_ppq) + 0.0
      local start_sec = reaper.MIDI_GetProjTimeFromPPQPos
        and (reaper.MIDI_GetProjTimeFromPPQPos(take, n.start_ppq) + 0.0)
        or reaper.TimeMap2_QNToTime(proj, start_qn) + 0.0
      local end_sec = reaper.MIDI_GetProjTimeFromPPQPos
        and (reaper.MIDI_GetProjTimeFromPPQPos(take, n.end_ppq) + 0.0)
        or reaper.TimeMap2_QNToTime(proj, e_qn) + 0.0
      extracted[#extracted + 1] = {
        index = #extracted,
        source_index = n.raw_index,
        start_ppq = n.start_ppq,
        end_ppq = n.end_ppq,
        start_qn = start_qn,
        end_qn = e_qn,
        duration_qn = e_qn - start_qn,
        item_relative_start_qn = start_qn - pos_qn,
        item_relative_end_qn = e_qn - pos_qn,
        start_seconds = start_sec,
        end_seconds = end_sec,
        pitch = n.pitch,
        velocity = n.velocity,
        channel = n.channel,
        muted = n.muted,
        selected = n.selected,
      }
    end
  end

  if want_selected_only and #extracted == 0 and #raw_notes > 0 then
    return nil, protocol.err(protocol.ERR.AMBIGUOUS_MELODY,
      "note scope requires selected notes but none are selected",
      { note_scope = note_scope, extraction_mode = mode })
  end
  if mode == "midi_channel" and #extracted == 0 then
    return nil, protocol.err(protocol.ERR.AMBIGUOUS_MELODY,
      string.format("no notes on MIDI channel %d", channel), { channel = channel })
  end
  if mode == "monophonic_voice" then
    local poly = max_polyphony(extracted)
    if poly > 1 then
      return nil, protocol.err(protocol.ERR.AMBIGUOUS_MELODY,
        string.format("monophonic_voice was requested but the material is %d-voice polyphonic", poly),
        { max_polyphony = poly })
    end
  end
  if #extracted == 0 then
    warnings[#warnings + 1] = { code = "empty_selection", message = "the resolved source contains no notes" }
  end

  local note_list_hash = util.hash_hex(M.note_list_canonical(extracted))

  local now = util.now()
  local snap = {
    snapshot_id = tagging.new_uuid(),
    project_pointer = tostring(proj),
    project_uuid = project_uuid,
    project_state_change_count = math.tointeger(reaper.GetProjectStateChangeCount(proj)) or 0,
    track_guid = src.track and track_guid(src.track) or nil,
    item_guid = item_guid(item),
    take_guid = take_guid(take),
    item_position_seconds = pos_sec,
    item_length_seconds = len_sec,
    item_position_qn = pos_qn,
    item_end_qn = end_qn,
    item_length_qn = end_qn - pos_qn,
    is_loop_source = loopsrc,
    note_scope = note_scope,
    extraction_mode = mode,
    extraction_channel = channel,
    resolved_by = src.resolved_by,
    midi_hash = midi_hash,
    note_selection_hash = selection_hash,
    tempo_map_hash = tempo_hash,
    timesig_map_hash = timesig_hash,
    note_list_hash = note_list_hash,
    note_count = #extracted,
    source_note_count = #raw_notes,
    notes = extracted,
    tempo_markers = markers,
    timestamp = now,
    timestamp_iso = util.iso8601(now),
    bridge_version = protocol.BRIDGE_VERSION,
    reaper_version = opts.reaper_version,
    selection_assumptions = assumptions,
    warnings = warnings,
  }
  snap.snapshot_hash = util.hash_hex(M.snapshot_canonical(snap))
  snap.take = take
  snap.item = item
  snap.track = src.track
  return snap
end

--- Strips host pointers so the snapshot can be JSON encoded.
function M.to_wire(snap)
  local out = {}
  for k, v in pairs(snap) do out[k] = v end
  out.take, out.item, out.track = nil, nil, nil
  out.notes = json.array(out.notes or {})
  out.tempo_markers = json.array(out.tempo_markers or {})
  out.warnings = json.array(out.warnings or {})
  out.selection_assumptions = json.array(out.selection_assumptions or {})
  if out.extraction_channel == nil then out.extraction_channel = json.null end
  if out.track_guid == nil then out.track_guid = json.null end
  if out.reaper_version == nil then out.reaper_version = json.null end
  return out
end

--------------------------------------------------------------------------------
-- Precondition checking
--------------------------------------------------------------------------------

--- Recomputes the hashes that the `expected_project` block can constrain,
--- without doing the full extraction. `ref` may carry `item`, `take` pointers.
local function live_state(proj, item, take)
  local st = {
    project_uuid = tagging.get_proj_state(proj, tagging.PROJ_KEYS.PROJECT_UUID),
    state_change_count = math.tointeger(reaper.GetProjectStateChangeCount(proj)) or 0,
  }
  local markers = M.read_tempo_map(proj)
  st.tempo_map_hash = util.hash_hex(M.tempo_canonical(markers))
  st.timesig_map_hash = util.hash_hex(M.timesig_canonical(markers))
  if take then
    local notes = M.read_take_notes(take)
    st.midi_hash = util.hash_hex(M.midi_canonical(notes))
    st.note_selection_hash = util.hash_hex(M.selection_canonical(notes))
  end
  if item then
    st.item_position_seconds = reaper.GetMediaItemInfo_Value(item, "D_POSITION") + 0.0
    st.item_length_seconds = reaper.GetMediaItemInfo_Value(item, "D_LENGTH") + 0.0
    st.item_position_qn = reaper.TimeMap2_timeToQN(proj, st.item_position_seconds) + 0.0
    st.item_end_qn = reaper.TimeMap2_timeToQN(proj,
      st.item_position_seconds + st.item_length_seconds) + 0.0
  end
  return st
end

M.live_state = live_state

--- Validates an `expected_project` block against live project state.
---
--- Field-to-error mapping (normative):
---   project_uuid        -> PROJECT_CHANGED
---   state_change_count  -> PROJECT_CHANGED
---   source_item_guid    -> SOURCE_ITEM_MISSING
---   source_take_guid    -> SOURCE_TAKE_MISSING
---   midi_hash           -> MIDI_CHANGED
---   tempo_map_hash      -> TEMPO_MAP_CHANGED
---   snapshot_hash       -> STALE_SNAPSHOT
---
--- Any field that is absent or JSON null is not enforced.
--- Returns `resolved` (a table with `item`/`take` when GUIDs were given), or
--- `nil, err`.
function M.check_expected_project(proj, expected, opts)
  opts = opts or {}
  local resolved = {}
  if expected == nil then return resolved end

  local want_uuid = protocol.opt_string(expected, "project_uuid")
  if want_uuid ~= nil then
    local have = tagging.get_proj_state(proj, tagging.PROJ_KEYS.PROJECT_UUID)
    if have == nil then
      return nil, protocol.err(protocol.ERR.PROJECT_CHANGED,
        "the active project carries no QLabs project UUID",
        { expected = want_uuid, actual = json.null })
    end
    if have ~= want_uuid then
      return nil, protocol.err(protocol.ERR.PROJECT_CHANGED,
        "the active project is not the project this request was built against",
        { expected = want_uuid, actual = have })
    end
  end

  local want_scc = protocol.opt_number(expected, "state_change_count")
  if want_scc ~= nil then
    local have = math.tointeger(reaper.GetProjectStateChangeCount(proj)) or 0
    if math.tointeger(want_scc) ~= have then
      return nil, protocol.err(protocol.ERR.PROJECT_CHANGED,
        string.format("project state change count is %d, expected %d", have, math.tointeger(want_scc) or -1),
        { expected = math.tointeger(want_scc), actual = have })
    end
  end

  local want_item = protocol.opt_string(expected, "source_item_guid")
  if want_item ~= nil then
    local item = M.find_item_by_guid(proj, want_item)
    if item == nil then
      return nil, protocol.err(protocol.ERR.SOURCE_ITEM_MISSING,
        string.format("source item %s is no longer present", want_item), { item_guid = want_item })
    end
    resolved.item = item
  end

  local want_take = protocol.opt_string(expected, "source_take_guid")
  if want_take ~= nil then
    local take, item = M.find_take_by_guid(proj, want_take)
    if take == nil then
      return nil, protocol.err(protocol.ERR.SOURCE_TAKE_MISSING,
        string.format("source take %s is no longer present", want_take), { take_guid = want_take })
    end
    resolved.take = take
    resolved.item = resolved.item or item
  end

  local st = live_state(proj, resolved.item, resolved.take)

  local want_midi = protocol.opt_string(expected, "midi_hash")
  if want_midi ~= nil then
    if st.midi_hash == nil then
      return nil, protocol.err(protocol.ERR.SOURCE_TAKE_MISSING,
        "midi_hash was supplied but no source take could be resolved")
    end
    if st.midi_hash ~= want_midi then
      return nil, protocol.err(protocol.ERR.MIDI_CHANGED,
        "the source MIDI has changed since the snapshot was taken",
        { expected = want_midi, actual = st.midi_hash })
    end
  end

  local want_tempo = protocol.opt_string(expected, "tempo_map_hash")
  if want_tempo ~= nil and st.tempo_map_hash ~= want_tempo then
    return nil, protocol.err(protocol.ERR.TEMPO_MAP_CHANGED,
      "the project tempo map has changed since the snapshot was taken",
      { expected = want_tempo, actual = st.tempo_map_hash })
  end

  local want_snapshot = protocol.opt_string(expected, "snapshot_hash")
  if want_snapshot ~= nil then
    -- The snapshot hash can only be recomputed by rebuilding the snapshot with
    -- the same scope/extraction settings that produced it.
    local rebuilt, rerr = M.build(proj, {
      source_mode = opts.source_mode or "auto",
      note_scope = opts.note_scope,
      extraction_mode = opts.extraction_mode,
      extraction_channel = opts.extraction_channel,
      reaper_version = opts.reaper_version,
    })
    if not rebuilt then
      return nil, protocol.err(protocol.ERR.STALE_SNAPSHOT,
        "cannot re-derive the snapshot to verify snapshot_hash: " .. tostring(rerr and rerr.message),
        { cause = rerr and rerr.code or json.null })
    end
    if rebuilt.snapshot_hash ~= want_snapshot then
      return nil, protocol.err(protocol.ERR.STALE_SNAPSHOT,
        "the project no longer matches the snapshot this request was built against",
        { expected = want_snapshot, actual = rebuilt.snapshot_hash })
    end
    resolved.snapshot = rebuilt
  end

  return resolved
end

return M
