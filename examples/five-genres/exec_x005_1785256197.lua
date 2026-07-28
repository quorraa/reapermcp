local LOG = [[C:\Wizardry\media\audio\reapermcp\examples\five-genres\exec_x005_1785256197.log]]
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
  reaper.SetCurrentBPM(proj, 174.0, false)
  reaper.SetTempoTimeSigMarker(proj, -1, 0.0, -1, -1, 174.0, 4, 4, false)
  reaper.InsertTrackAtIndex(0, true)
  local tr = reaper.GetTrack(proj, 0)
  reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "Melody", true)
  local t0 = reaper.TimeMap2_QNToTime(proj, 0.0)
  local t1 = reaper.TimeMap2_QNToTime(proj, 64.0)
  local item = reaper.CreateNewMIDIItemInProj(tr, t0, t1, false)
  local take = reaper.GetActiveTake(item)
  local N = {
    {69, 0.0, 0.5, 84},
    {76, 0.5, 1.5, 86},
    {74, 1.5, 2.5, 76},
    {71, 2.5, 4.0, 78},
    {71, 4.0, 4.5, 77},
    {77, 4.5, 5.5, 84},
    {76, 5.5, 6.5, 88},
    {72, 6.5, 8.0, 83},
    {67, 8.0, 8.5, 80},
    {72, 8.5, 9.0, 86},
    {76, 13.0, 15.0, 76},
    {72, 16.0, 16.5, 82},
    {79, 16.5, 17.5, 86},
    {77, 17.5, 18.5, 76},
    {74, 18.5, 20.0, 83},
    {74, 20.0, 20.5, 87},
    {81, 20.5, 21.5, 87},
    {79, 21.5, 22.5, 78},
    {76, 22.5, 24.0, 87},
    {71, 24.0, 24.5, 72},
    {64, 24.5, 25.0, 78},
    {69, 27.5, 31.0, 76},
    {76, 32.0, 32.5, 77},
    {83, 32.5, 33.5, 75},
    {81, 33.5, 34.75, 81},
    {77, 34.75, 36.0, 79},
    {77, 36.0, 36.5, 80},
    {86, 36.5, 37.5, 98},
    {83, 37.5, 38.5, 81},
    {79, 38.5, 40.0, 80},
    {74, 40.0, 40.5, 87},
    {67, 40.5, 41.0, 78},
    {76, 45.0, 47.0, 76},
    {67, 48.0, 48.625, 81},
    {72, 48.625, 50.125, 79},
    {74, 50.125, 51.125, 73},
    {65, 51.125, 52.25, 83},
    {69, 52.25, 52.625, 87},
    {74, 52.625, 53.875, 80},
    {64, 53.875, 55.375, 74},
    {67, 55.375, 56.25, 76},
    {65, 56.25, 56.625, 77},
    {72, 56.625, 57.25, 78},
    {69, 59.5, 63.0, 76},
  }
  for _, n in ipairs(N) do
    local sp = reaper.MIDI_GetPPQPosFromProjQN(take, n[2])
    local ep = reaper.MIDI_GetPPQPosFromProjQN(take, n[3])
    reaper.MIDI_InsertNote(take, true, false, sp, ep, 0, n[1], n[4], true)
  end
  reaper.MIDI_Sort(take)
  reaper.GetSetMediaItemTakeInfo_String(take, "P_NAME", [[03_drum_and_bass]], true)
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
