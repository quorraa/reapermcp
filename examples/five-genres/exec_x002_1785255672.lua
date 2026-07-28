local LOG = [[C:\Wizardry\media\audio\reapermcp\examples\five-genres\exec_x002_1785255672.log]]
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
  reaper.SetCurrentBPM(proj, 88.0, false)
  reaper.SetTempoTimeSigMarker(proj, -1, 0.0, -1, -1, 88.0, 4, 4, false)
  reaper.InsertTrackAtIndex(0, true)
  local tr = reaper.GetTrack(proj, 0)
  reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "Melody", true)
  local t0 = reaper.TimeMap2_QNToTime(proj, 0.0)
  local t1 = reaper.TimeMap2_QNToTime(proj, 64.0)
  local item = reaper.CreateNewMIDIItemInProj(tr, t0, t1, false)
  local take = reaper.GetActiveTake(item)
  local N = {
    {65, 0.0, 0.75, 85},
    {69, 0.75, 1.5, 77},
    {67, 1.5, 2.0, 76},
    {64, 2.0, 3.5, 76},
    {67, 4.0, 4.75, 84},
    {70, 4.75, 5.5, 79},
    {69, 5.5, 6.0, 84},
    {65, 6.0, 7.5, 80},
    {64, 8.0, 8.75, 88},
    {60, 8.75, 9.5, 78},
    {69, 10.0, 10.375, 81},
    {72, 10.375, 10.75, 73},
    {72, 12.75, 14.75, 76},
    {72, 16.0, 16.75, 85},
    {76, 16.75, 17.5, 78},
    {74, 17.5, 18.0, 74},
    {70, 18.25, 19.5, 81},
    {74, 20.0, 20.75, 76},
    {77, 21.0, 21.5, 86},
    {76, 21.5, 22.0, 82},
    {72, 22.0, 23.5, 86},
    {70, 24.25, 24.75, 76},
    {67, 25.0, 25.5, 87},
    {76, 26.0, 26.375, 83},
    {79, 26.375, 26.75, 80},
    {65, 28.75, 30.75, 76},
    {76, 32.25, 32.75, 86},
    {79, 32.75, 33.5, 83},
    {77, 33.5, 34.0, 82},
    {74, 34.0, 35.5, 88},
    {77, 36.0, 36.75, 88},
    {81, 36.75, 37.5, 98},
    {79, 37.5, 38.0, 72},
    {76, 38.0, 39.5, 85},
    {74, 40.25, 40.75, 72},
    {70, 41.0, 41.5, 72},
    {79, 42.0, 42.375, 87},
    {70, 42.375, 42.75, 83},
    {72, 44.75, 46.75, 76},
    {64, 48.25, 48.938, 73},
    {60, 48.938, 49.876000000000005, 76},
    {62, 50.125, 50.5, 73},
    {65, 50.5, 52.375, 88},
    {65, 52.25, 52.938, 72},
    {62, 52.938, 53.876000000000005, 87},
    {64, 53.875, 54.5, 83},
    {67, 54.5, 56.375, 75},
    {62, 56.0, 56.938, 88},
    {65, 56.938, 57.876000000000005, 87},
    {67, 58.0, 58.469, 73},
    {64, 58.469, 58.938, 75},
    {65, 60.75, 62.75, 76},
  }
  for _, n in ipairs(N) do
    local sp = reaper.MIDI_GetPPQPosFromProjQN(take, n[2])
    local ep = reaper.MIDI_GetPPQPosFromProjQN(take, n[3])
    reaper.MIDI_InsertNote(take, true, false, sp, ep, 0, n[1], n[4], true)
  end
  reaper.MIDI_Sort(take)
  reaper.GetSetMediaItemTakeInfo_String(take, "P_NAME", [[01_jazz_ballad]], true)
  reaper.SelectAllMediaItems(proj, false)
  reaper.SetMediaItemSelected(item, true)
  reaper.GetSet_LoopTimeRange2(proj, true, true, t0, t1, false)
  reaper.UpdateArrange()
  say("built " .. #N .. " notes over 16 bars")
end)
if not __ok then say("LUA_ERROR: " .. tostring(__err)) end

say("__DONE__")
__f:close()

-- Deliberately does not close anything by default. A workflow that spans
-- several calls - create a project here, inspect it there - is destroyed by an
-- automatic close in between. Call cleanup_tabs() when you actually want it.
if __SWEEP then __discard_tabs(1) end
