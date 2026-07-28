local LOG = [[C:\Wizardry\media\audio\reapermcp\examples\five-genres\exec_x002_1785256051.log]]
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
    {65, 0.0, 0.75, 83},
    {69, 0.75, 1.5, 85},
    {67, 1.5, 2.25, 77},
    {64, 2.25, 4.0, 76},
    {67, 4.0, 4.75, 76},
    {70, 4.75, 5.5, 84},
    {69, 5.5, 6.25, 79},
    {65, 6.25, 8.0, 84},
    {64, 8.0, 8.75, 80},
    {60, 8.75, 9.5, 88},
    {72, 13.0, 15.0, 76},
    {69, 16.0, 16.75, 78},
    {72, 16.75, 17.5, 73},
    {70, 17.5, 18.0, 88},
    {67, 18.0, 20.0, 83},
    {70, 20.0, 20.75, 85},
    {74, 20.75, 21.5, 78},
    {72, 21.5, 22.25, 74},
    {69, 22.25, 24.0, 81},
    {67, 24.0, 25.0, 76},
    {64, 25.0, 25.5, 86},
    {65, 27.5, 31.0, 76},
    {72, 32.0, 32.75, 82},
    {76, 32.75, 33.5, 86},
    {74, 33.5, 34.25, 76},
    {70, 34.25, 36.0, 87},
    {74, 36.0, 36.75, 83},
    {79, 36.75, 37.5, 94},
    {76, 37.5, 38.0, 76},
    {72, 38.0, 40.0, 86},
    {70, 40.0, 40.75, 83},
    {67, 40.75, 41.5, 82},
    {72, 45.0, 47.0, 76},
    {70, 48.0, 48.938, 88},
    {67, 48.938, 49.875, 84},
    {69, 49.875, 50.5, 72},
    {72, 50.5, 52.25, 85},
    {72, 52.25, 53.188, 72},
    {69, 53.188, 53.875, 72},
    {70, 53.875, 54.5, 87},
    {74, 54.5, 56.25, 83},
    {69, 56.25, 56.938, 73},
    {72, 56.938, 57.876000000000005, 85},
    {65, 59.5, 63.0, 76},
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
