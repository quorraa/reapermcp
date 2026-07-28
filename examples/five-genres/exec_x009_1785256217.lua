local LOG = [[C:\Wizardry\media\audio\reapermcp\examples\five-genres\exec_x009_1785256217.log]]
local NL = string.char(10)
local __f = io.open(LOG, "w")
local function say(s) __f:write(tostring(s) .. NL) __f:flush() end
local function findproj(trackname)
  local pi = 0
  while true do
    local p = reaper.EnumProjects(pi, "")
    if not p then break end
    for t = 0, reaper.CountTracks(p) - 1 do
      local tr = reaper.GetTrack(p, t)
      local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
      if tn == trackname then return p, tr end
    end
    pi = pi + 1
  end
  return nil, nil
end
local __SWEEP = false
local __SCRATCH = [[C:/Wizardry/media/audio/reapermcp/examples/five-genres/discard]]

-- Sweep up before starting. Closing only the tabs this script opened is not
-- enough: a script that dies partway - crash, timeout, killed batch - orphans
-- its tab, and the orphans pile up until REAPER asks about every one of them at
-- exit. Everything here is a throwaway test project reopened from disk by name,
-- so anything already open can go.
local function __discard_tabs(keep)
  local closed = 0
  for _ = 1, 64 do
    local n = 0
    while reaper.EnumProjects(n, "") do n = n + 1 end
    if n <= keep then break end
    -- Save to a scratch path first: that clears REAPER's "modified" flag, so
    -- the close raises no dialog, and it never touches the real project file.
    reaper.Main_SaveProjectEx(0, __SCRATCH .. "/discard_" .. closed .. ".RPP", 0)
    reaper.Main_OnCommand(40860, 0)   -- File: Close current project tab
    closed = closed + 1
  end
  return closed
end

if __SWEEP then __discard_tabs(1) end
local __ntabs = 0
do
  local pi = 0
  while reaper.EnumProjects(pi, "") do pi = pi + 1 end
  __ntabs = pi
end
local __ok, __err = pcall(function()
  reaper.Main_OnCommand(40859, 0)
  local proj = 0
  reaper.SetCurrentBPM(proj, 100.0, false)
  reaper.SetTempoTimeSigMarker(proj, -1, 0.0, -1, -1, 100.0, 4, 4, false)
  reaper.InsertTrackAtIndex(0, true)
  local tr = reaper.GetTrack(proj, 0)
  reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "Melody", true)
  local t0 = reaper.TimeMap2_QNToTime(proj, 0.0)
  local t1 = reaper.TimeMap2_QNToTime(proj, 48.0)
  local item = reaper.CreateNewMIDIItemInProj(tr, t0, t1, false)
  local take = reaper.GetActiveTake(item)
  local N = {
    {64, 0.0, 1.0, 72},
    {69, 1.0, 1.75, 88},
    {67, 1.75, 2.75, 72},
    {64, 2.75, 4.0, 79},
    {67, 4.0, 5.0, 88},
    {70, 5.0, 5.75, 85},
    {69, 5.75, 6.75, 84},
    {67, 6.75, 8.0, 76},
    {71, 9.0, 11.0, 76},
    {69, 12.0, 13.0, 81},
    {71, 13.0, 13.75, 84},
    {70, 13.75, 14.5, 77},
    {69, 14.5, 16.0, 73},
    {70, 16.0, 17.0, 79},
    {74, 17.0, 17.75, 81},
    {71, 17.75, 18.5, 73},
    {64, 19.5, 23.0, 76},
    {76, 24.0, 25.0, 86},
    {81, 25.0, 26.0, 83},
    {79, 26.0, 26.75, 88},
    {76, 26.75, 28.25, 75},
    {79, 28.25, 29.0, 74},
    {82, 29.0, 29.75, 86},
    {81, 29.75, 30.5, 79},
    {79, 30.5, 32.0, 83},
    {71, 33.0, 35.0, 76},
    {67, 36.0, 37.25, 75},
    {62, 37.25, 38.188, 88},
    {64, 38.188, 39.125, 72},
    {67, 39.125, 40.0, 87},
    {69, 40.0, 41.25, 72},
    {64, 41.25, 42.188, 86},
    {67, 42.188, 43.5, 80},
    {64, 43.5, 47.0, 76},
  }
  for _, n in ipairs(N) do
    local sp = reaper.MIDI_GetPPQPosFromProjQN(take, n[2])
    local ep = reaper.MIDI_GetPPQPosFromProjQN(take, n[3])
    reaper.MIDI_InsertNote(take, true, false, sp, ep, 0, n[1], n[4], true)
  end
  reaper.MIDI_Sort(take)
  reaper.GetSetMediaItemTakeInfo_String(take, "P_NAME", [[05_blues]], true)
  reaper.SelectAllMediaItems(proj, false)
  reaper.SetMediaItemSelected(item, true)
  reaper.GetSet_LoopTimeRange2(proj, true, true, t0, t1, false)
  reaper.UpdateArrange()
  say("built " .. #N .. " notes over 12 bars")
end)
if not __ok then say("LUA_ERROR: " .. tostring(__err)) end

say("__DONE__")
__f:close()

-- Deliberately does not close anything by default. A workflow that spans
-- several calls - create a project here, inspect it there - is destroyed by an
-- automatic close in between. Call cleanup_tabs() when you actually want it.
if __SWEEP then __discard_tabs(1) end
