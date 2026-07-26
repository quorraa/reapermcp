--[[
  mock_reaper.lua -- an in-memory mock of every `reaper.*` function the QLabs
  bridge calls, so the whole Lua logic can be exercised without REAPER.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT

  Modelling notes
  ---------------
  * Undo is a real operation journal. Every mutating call inside an
    Undo_BeginBlock2/Undo_EndBlock2 pair records an inverse closure; DoUndo runs
    them in reverse. Pointer identity is preserved, and a deleted object is
    resurrected as the same Lua table, which is what makes "one undo restores
    the prior state" a meaningful assertion.
  * Tempo is constant per project (the bridge's QN conversions only need a
    monotone, invertible map). PPQ is 960 per quarter note and PPQ zero is the
    item start, matching REAPER's take-relative PPQ domain.
  * `ui_refresh_depth` is exposed so tests can assert PreventUIRefresh balance.
--]]

local M = {}

local PPQ_PER_QN = 960.0

--------------------------------------------------------------------------------
-- State
--------------------------------------------------------------------------------

local Mock = {}
Mock.__index = Mock

local function new_guid(state)
  state.guid_counter = state.guid_counter + 1
  local n = state.guid_counter
  return string.format("{%08X-%04X-%04X-%04X-%012X}", n, n % 0xFFFF, 0x4000 + (n % 0x0FFF),
    0x8000 + (n % 0x3FFF), n)
end

--- Creates a fresh mock host. `fs` is an optional mock_fs instance backing the
--- filesystem-facing REAPER calls.
function M.new(opts)
  opts = opts or {}
  local state = setmetatable({
    guid_counter = 0,
    resource_path = opts.resource_path or "/reaper",
    app_version = opts.app_version or "7.22/linux-x86_64",
    time = 0.0,
    ui_refresh_depth = 0,
    console = {},
    messages = {},
    deferred = {},
    atexit_fns = {},
    toggle_state = {},
    projects = {},
    active_project_index = 1,
    fs = opts.fs,
    arrange_updates = 0,
    editor_take = nil,
  }, Mock)
  state:new_project(opts.project_name or "MockProject", opts.project_path or "")
  return state
end

--- Adds a project and makes it active.
function Mock:new_project(name, path)
  local proj = {
    __t = "project",
    name = name or "MockProject",
    path = path or "",
    tracks = {},
    ext = {},
    markers = {},
    state_change_count = 0,
    undo = {},
    redo = {},
    undo_depth = 0,
    journal = nil,
    tempo = 120.0,
    timesig_num = 4,
    timesig_den = 4,
    tempo_markers = {},
    play_state = 0,
    dirty = false,
  }
  self.projects[#self.projects + 1] = proj
  self.active_project_index = #self.projects
  return proj
end

--- The active project table.
function Mock:project()
  return self.projects[self.active_project_index]
end

local function touch(proj)
  proj.state_change_count = proj.state_change_count + 1
end

local function record(proj, inverse)
  if proj.journal then
    proj.journal[#proj.journal + 1] = inverse
  end
end

--------------------------------------------------------------------------------
-- Object construction helpers (test-facing)
--------------------------------------------------------------------------------

--- Appends a track to the project.
function Mock:add_track(proj, name, index)
  proj = proj or self:project()
  local tr = {
    __t = "track",
    proj = proj,
    guid = new_guid(self),
    name = name or "",
    ext = {},
    values = { B_MUTE = 0, I_FOLDERDEPTH = 0, I_SELECTED = 0 },
    items = {},
    sends = {},
  }
  index = index or #proj.tracks
  table.insert(proj.tracks, index + 1, tr)
  touch(proj)
  record(proj, function()
    for i = 1, #proj.tracks do
      if proj.tracks[i] == tr then table.remove(proj.tracks, i) break end
    end
  end)
  return tr
end

--- Creates a MIDI item on a track. Bounds are given in project quarter notes.
function Mock:add_midi_item(track, start_qn, end_qn)
  local proj = track.proj
  local item = {
    __t = "item",
    proj = proj,
    track = track,
    guid = new_guid(self),
    ext = {},
    selected = false,
    values = {
      D_POSITION = self:qn_to_time(proj, start_qn),
      D_LENGTH = self:qn_to_time(proj, end_qn) - self:qn_to_time(proj, start_qn),
      B_MUTE = 0,
      B_LOOPSRC = 0,
    },
    takes = {},
    active_take = 1,
  }
  local take = {
    __t = "take",
    proj = proj,
    item = item,
    guid = new_guid(self),
    ext = {},
    is_midi = true,
    name = "",
    notes = {},
  }
  item.takes[1] = take
  track.items[#track.items + 1] = item
  touch(proj)
  record(proj, function()
    for i = 1, #track.items do
      if track.items[i] == item then table.remove(track.items, i) break end
    end
  end)
  return item, take
end

--- Adds a note to a take. Positions are take PPQ.
function Mock:add_note(take, start_ppq, end_ppq, pitch, vel, chan, selected, muted)
  take.notes[#take.notes + 1] = {
    start_ppq = start_ppq + 0.0,
    end_ppq = end_ppq + 0.0,
    pitch = pitch,
    velocity = vel or 100,
    channel = chan or 0,
    selected = selected and true or false,
    muted = muted and true or false,
  }
  touch(take.proj)
  return take.notes[#take.notes]
end

--- Selects or deselects an item.
function Mock:select_item(item, selected)
  item.selected = selected and true or false
  touch(item.proj)
end

--- Sets the take returned by MIDIEditor_GetTake (nil closes the editor).
function Mock:set_editor_take(take)
  self.editor_take = take
end

--- Advances the monotonic clock.
function Mock:advance(seconds)
  self.time = self.time + seconds
  return self.time
end

--- Item start position in project quarter notes.
function Mock:qn_to_time(proj, qn)
  return qn * 60.0 / proj.tempo
end

function Mock:time_to_qn(proj, t)
  return t * proj.tempo / 60.0
end

local function item_start_qn(item)
  return item.values.D_POSITION * item.proj.tempo / 60.0
end

--- Iterates every live item in a project.
local function all_items(proj)
  local out = {}
  for i = 1, #proj.tracks do
    local tr = proj.tracks[i]
    for j = 1, #tr.items do out[#out + 1] = tr.items[j] end
  end
  return out
end

--------------------------------------------------------------------------------
-- The `reaper` API table
--------------------------------------------------------------------------------

--- Builds the `reaper` table for this mock host.
function Mock:api()
  local self_ = self
  local R = {}

  ----------------------------------------------------------------- environment
  function R.GetResourcePath() return self_.resource_path end
  function R.GetAppVersion() return self_.app_version end
  function R.time_precise() return self_.time end
  function R.ShowConsoleMsg(s) self_.console[#self_.console + 1] = tostring(s) end
  function R.ShowMessageBox(msg, title, _t)
    self_.messages[#self_.messages + 1] = { msg = msg, title = title }
    return 1
  end
  function R.defer(fn) self_.deferred[#self_.deferred + 1] = fn end
  function R.atexit(fn) self_.atexit_fns[#self_.atexit_fns + 1] = fn end
  function R.get_action_context() return false, "", 0, 40000, 0, -1, 0.0 end
  function R.SetToggleCommandState(sec, cmd, val)
    self_.toggle_state[tostring(sec) .. ":" .. tostring(cmd)] = val
    return true
  end
  function R.RefreshToolbar2() return true end
  function R.genGuid() return new_guid(self_) end
  function R.MarkProjectDirty(proj) proj.dirty = true end

  ------------------------------------------------------------------ filesystem
  function R.RecursiveCreateDirectory(path)
    if self_.fs then self_.fs:mkdirp(path) end
    return 1
  end
  function R.file_exists(path)
    if self_.fs then return self_.fs:exists(path) end
    return false
  end
  function R.EnumerateFiles(dir, index)
    if not self_.fs then return nil end
    local names = self_.fs:listdir(dir)
    return names[index + 1]
  end

  --------------------------------------------------------------------- project
  function R.EnumProjects(index)
    local proj
    if index == -1 then
      proj = self_:project()
    else
      proj = self_.projects[index + 1]
    end
    if proj == nil then return nil, "" end
    return proj, proj.path
  end
  function R.GetProjectStateChangeCount(proj) return (proj or self_:project()).state_change_count end
  function R.GetProjectName(proj) return (proj or self_:project()).name end
  function R.GetSetProjectInfo_String(proj, key, value, isSet)
    proj = proj or self_:project()
    if key == "PROJECT_NAME" then
      if isSet then proj.name = value touch(proj) return true end
      return true, proj.name
    end
    return false, ""
  end
  function R.GetPlayStateEx(proj) return (proj or self_:project()).play_state end
  function R.GetProjExtState(proj, section, key)
    proj = proj or self_:project()
    local sec = proj.ext[section]
    local v = sec and sec[key]
    if v == nil or v == "" then return 0, "" end
    return 1, v
  end
  function R.SetProjExtState(proj, section, key, value)
    proj = proj or self_:project()
    proj.ext[section] = proj.ext[section] or {}
    local prev = proj.ext[section][key]
    proj.ext[section][key] = value
    touch(proj)
    record(proj, function() proj.ext[section][key] = prev end)
    return 1
  end

  ------------------------------------------------------------------------ undo
  function R.Undo_BeginBlock2(proj)
    proj = proj or self_:project()
    proj.undo_depth = proj.undo_depth + 1
    if proj.undo_depth == 1 then proj.journal = {} end
  end
  function R.Undo_EndBlock2(proj, label, _flags)
    proj = proj or self_:project()
    if proj.undo_depth == 0 then return end
    proj.undo_depth = proj.undo_depth - 1
    if proj.undo_depth == 0 then
      local j = proj.journal
      proj.journal = nil
      if j and #j > 0 then
        proj.undo[#proj.undo + 1] = { label = label, inverses = j }
      end
    end
  end
  function R.Undo_CanUndo2(proj)
    proj = proj or self_:project()
    local top = proj.undo[#proj.undo]
    if not top then return nil end
    return top.label
  end
  function R.Undo_DoUndo2(proj)
    proj = proj or self_:project()
    local top = table.remove(proj.undo)
    if not top then return 0 end
    for i = #top.inverses, 1, -1 do top.inverses[i]() end
    touch(proj)
    proj.redo[#proj.redo + 1] = top
    return 1
  end
  function R.PreventUIRefresh(n) self_.ui_refresh_depth = self_.ui_refresh_depth + n end
  function R.UpdateArrange() self_.arrange_updates = self_.arrange_updates + 1 end
  function R.TrackList_AdjustWindows() return true end

  ---------------------------------------------------------------------- tracks
  function R.CountTracks(proj) return #(proj or self_:project()).tracks end
  function R.GetTrack(proj, i) return (proj or self_:project()).tracks[i + 1] end
  function R.InsertTrackInProject(proj, index, _flags)
    self_:add_track(proj or self_:project(), "", index)
    return true
  end
  function R.InsertTrackAtIndex(index, _defaults)
    self_:add_track(self_:project(), "", index)
    return true
  end
  function R.GetTrackGUID(tr) return tr.guid end
  function R.DeleteTrack(tr)
    local proj = tr.proj
    local removed_at
    for i = 1, #proj.tracks do
      if proj.tracks[i] == tr then removed_at = i table.remove(proj.tracks, i) break end
    end
    if not removed_at then return false end
    touch(proj)
    record(proj, function() table.insert(proj.tracks, removed_at, tr) end)
    return true
  end
  function R.CountTrackMediaItems(tr) return #tr.items end
  function R.GetTrackMediaItem(tr, i) return tr.items[i + 1] end
  function R.GetMediaTrackInfo_Value(tr, key) return tr.values[key] or 0 end
  function R.SetMediaTrackInfo_Value(tr, key, value)
    local prev = tr.values[key]
    tr.values[key] = value
    touch(tr.proj)
    record(tr.proj, function() tr.values[key] = prev end)
    return true
  end
  function R.GetSetMediaTrackInfo_String(tr, key, value, isSet)
    if key == "GUID" then
      if isSet then return false end
      return true, tr.guid
    elseif key == "P_NAME" then
      if isSet then
        local prev = tr.name
        tr.name = value
        touch(tr.proj)
        record(tr.proj, function() tr.name = prev end)
        return true
      end
      return true, tr.name
    elseif key:sub(1, 6) == "P_EXT:" then
      local k = key:sub(7)
      if isSet then
        local prev = tr.ext[k]
        tr.ext[k] = value
        touch(tr.proj)
        record(tr.proj, function() tr.ext[k] = prev end)
        return true
      end
      return true, tr.ext[k] or ""
    end
    return false, ""
  end
  function R.CreateTrackSend(src, dest)
    src.sends[#src.sends + 1] = { dest = dest, values = {} }
    touch(src.proj)
    local idx = #src.sends
    record(src.proj, function() table.remove(src.sends, idx) end)
    return idx - 1
  end
  function R.SetTrackSendInfo_Value(tr, _category, sendidx, key, value)
    local send = tr.sends[sendidx + 1]
    if not send then return false end
    send.values[key] = value
    return true
  end

  ----------------------------------------------------------------------- items
  function R.CountMediaItems(proj) return #all_items(proj or self_:project()) end
  function R.GetMediaItem(proj, i) return all_items(proj or self_:project())[i + 1] end
  function R.CountSelectedMediaItems(proj)
    local n = 0
    for _, it in ipairs(all_items(proj or self_:project())) do
      if it.selected then n = n + 1 end
    end
    return n
  end
  function R.GetSelectedMediaItem(proj, i)
    local n = 0
    for _, it in ipairs(all_items(proj or self_:project())) do
      if it.selected then
        if n == i then return it end
        n = n + 1
      end
    end
    return nil
  end
  function R.GetMediaItem_Track(item) return item.track end
  function R.GetMediaItemInfo_Value(item, key) return item.values[key] or 0 end
  function R.SetMediaItemInfo_Value(item, key, value)
    local prev = item.values[key]
    item.values[key] = value
    touch(item.proj)
    record(item.proj, function() item.values[key] = prev end)
    return true
  end
  function R.GetSetMediaItemInfo_String(item, key, value, isSet)
    if key == "GUID" then
      if isSet then return false end
      return true, item.guid
    elseif key:sub(1, 6) == "P_EXT:" then
      local k = key:sub(7)
      if isSet then
        local prev = item.ext[k]
        item.ext[k] = value
        touch(item.proj)
        record(item.proj, function() item.ext[k] = prev end)
        return true
      end
      return true, item.ext[k] or ""
    end
    return false, ""
  end
  function R.DeleteTrackMediaItem(track, item)
    local removed_at
    for i = 1, #track.items do
      if track.items[i] == item then removed_at = i table.remove(track.items, i) break end
    end
    if not removed_at then return false end
    touch(track.proj)
    record(track.proj, function() table.insert(track.items, removed_at, item) end)
    return true
  end
  function R.CreateNewMIDIItemInProj(track, start_in, end_in, qn_in)
    local proj = track.proj
    local s_qn, e_qn
    if qn_in then
      s_qn, e_qn = start_in, end_in
    else
      s_qn = self_:time_to_qn(proj, start_in)
      e_qn = self_:time_to_qn(proj, end_in)
    end
    local item = self_:add_midi_item(track, s_qn, e_qn)
    return item
  end

  ----------------------------------------------------------------------- takes
  function R.CountTakes(item) return #item.takes end
  function R.GetTake(item, i) return item.takes[i + 1] end
  function R.GetActiveTake(item) return item.takes[item.active_take] end
  function R.GetMediaItemTake_Item(take) return take.item end
  function R.GetMediaItemTake_Track(take) return take.item.track end
  function R.TakeIsMIDI(take) return take ~= nil and take.is_midi == true end
  function R.GetSetMediaItemTakeInfo_String(take, key, value, isSet)
    if key == "GUID" then
      if isSet then return false end
      return true, take.guid
    elseif key == "P_NAME" then
      if isSet then
        local prev = take.name
        take.name = value
        touch(take.proj)
        record(take.proj, function() take.name = prev end)
        return true
      end
      return true, take.name
    elseif key:sub(1, 6) == "P_EXT:" then
      local k = key:sub(7)
      if isSet then
        local prev = take.ext[k]
        take.ext[k] = value
        touch(take.proj)
        record(take.proj, function() take.ext[k] = prev end)
        return true
      end
      return true, take.ext[k] or ""
    end
    return false, ""
  end

  ------------------------------------------------------------------------ midi
  function R.MIDI_CountEvts(take)
    if take == nil then return false, 0, 0, 0 end
    return true, #take.notes, 0, 0
  end
  function R.MIDI_GetNote(take, i)
    local n = take.notes[i + 1]
    if not n then return false end
    return true, n.selected, n.muted, n.start_ppq, n.end_ppq, n.channel, n.pitch, n.velocity
  end
  function R.MIDI_InsertNote(take, selected, muted, startppq, endppq, chan, pitch, vel, _noSort)
    local note = {
      start_ppq = startppq + 0.0, end_ppq = endppq + 0.0, pitch = pitch,
      velocity = vel, channel = chan, selected = selected and true or false,
      muted = muted and true or false,
    }
    take.notes[#take.notes + 1] = note
    touch(take.proj)
    local idx = #take.notes
    record(take.proj, function() table.remove(take.notes, idx) end)
    return true
  end
  function R.MIDI_Sort(take)
    local before = {}
    for i = 1, #take.notes do before[i] = take.notes[i] end
    table.sort(take.notes, function(a, b)
      if a.start_ppq ~= b.start_ppq then return a.start_ppq < b.start_ppq end
      if a.pitch ~= b.pitch then return a.pitch < b.pitch end
      return a.channel < b.channel
    end)
    record(take.proj, function()
      for i = 1, #before do take.notes[i] = before[i] end
    end)
    return true
  end
  function R.MIDI_GetProjQNFromPPQPos(take, ppq)
    return item_start_qn(take.item) + ppq / PPQ_PER_QN
  end
  function R.MIDI_GetPPQPosFromProjQN(take, qn)
    return (qn - item_start_qn(take.item)) * PPQ_PER_QN
  end
  function R.MIDI_GetProjTimeFromPPQPos(take, ppq)
    local qn = R.MIDI_GetProjQNFromPPQPos(take, ppq)
    return self_:qn_to_time(take.proj, qn)
  end

  ------------------------------------------------------------------ midi editor
  function R.MIDIEditor_GetActive()
    if self_.editor_take == nil then return nil end
    return "mock-midi-editor-hwnd"
  end
  function R.MIDIEditor_GetTake(_hwnd) return self_.editor_take end

  ----------------------------------------------------------------------- tempo
  function R.CountTempoTimeSigMarkers(proj) return #(proj or self_:project()).tempo_markers end
  function R.GetTempoTimeSigMarker(proj, i)
    local m = (proj or self_:project()).tempo_markers[i + 1]
    if not m then return false end
    return true, m.time, m.measure or 0, m.beat or 0, m.bpm, m.num, m.den, m.linear or false
  end
  function R.TimeMap_GetTimeSigAtTime(proj, _t)
    proj = proj or self_:project()
    return proj.timesig_num, proj.timesig_den, proj.tempo
  end
  function R.TimeMap2_timeToQN(proj, t) return self_:time_to_qn(proj or self_:project(), t) end
  function R.TimeMap2_QNToTime(proj, qn) return self_:qn_to_time(proj or self_:project(), qn) end
  function R.Master_GetTempo() return self_:project().tempo end

  --------------------------------------------------------------------- markers
  function R.AddProjectMarker2(proj, isrgn, pos, rgnend, name, wantidx, color)
    proj = proj or self_:project()
    local idx = wantidx
    if idx == nil or idx < 0 then idx = #proj.markers + 1 end
    local marker = { index = idx, is_region = isrgn and true or false, pos = pos,
      rgnend = rgnend, name = name, color = color }
    proj.markers[#proj.markers + 1] = marker
    touch(proj)
    local at = #proj.markers
    record(proj, function() table.remove(proj.markers, at) end)
    return idx
  end

  ------------------------------------------------------------------- pointers
  function R.ValidatePtr2(proj, ptr, kind)
    proj = proj or self_:project()
    if type(ptr) ~= "table" then return false end
    if kind == "MediaTrack*" then
      for i = 1, #proj.tracks do if proj.tracks[i] == ptr then return true end end
      return false
    elseif kind == "MediaItem*" then
      for _, it in ipairs(all_items(proj)) do if it == ptr then return true end end
      return false
    elseif kind == "MediaItem_Take*" then
      for _, it in ipairs(all_items(proj)) do
        for j = 1, #it.takes do if it.takes[j] == ptr then return true end end
      end
      return false
    end
    return false
  end

  return R
end

--- Installs this mock as the global `reaper` table and returns it.
function Mock:install()
  _G.reaper = self:api()
  return _G.reaper
end

--- Convenience: builds a mock host with a track holding one MIDI item and the
--- given notes, returns `state, track, item, take`.
--- `notes` entries are `{start_qn, end_qn, pitch, velocity, channel, selected}`.
function M.with_melody(opts)
  opts = opts or {}
  local state = M.new(opts)
  local proj = state:project()
  local track = state:add_track(proj, opts.track_name or "Melody")
  local item, take = state:add_midi_item(track, opts.start_qn or 0.0, opts.end_qn or 4.0)
  for _, n in ipairs(opts.notes or {}) do
    local s = (n[1] - (opts.start_qn or 0.0)) * PPQ_PER_QN
    local e = (n[2] - (opts.start_qn or 0.0)) * PPQ_PER_QN
    state:add_note(take, s, e, n[3], n[4] or 100, n[5] or 0, n[6])
  end
  return state, track, item, take
end

--- Quarter-note resolution used by the mock.
M.PPQ_PER_QN = PPQ_PER_QN

return M
