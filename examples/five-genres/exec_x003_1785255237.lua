local LOG = [[C:\Wizardry\media\audio\reapermcp\examples\five-genres\exec_x003_1785255237.log]]
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
  reaper.SetCurrentBPM(proj, 76.0, false)
  reaper.SetTempoTimeSigMarker(proj, -1, 0.0, -1, -1, 76.0, 4, 4, false)
  reaper.InsertTrackAtIndex(0, true)
  local tr = reaper.GetTrack(proj, 0)
  reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "Melody", true)
  local t0 = reaper.TimeMap2_QNToTime(proj, 0.0)
  local t1 = reaper.TimeMap2_QNToTime(proj, 64.0)
  local item = reaper.CreateNewMIDIItemInProj(tr, t0, t1, false)
  local take = reaper.GetActiveTake(item)
  local N = {
    {66, 0.0, 0.5, 76},
    {68, 0.75, 1.5, 88},
    {72, 1.75, 2.5, 77},
    {70, 2.75, 4.0, 81},
    {68, 4.0, 4.5, 77},
    {70, 5.0, 5.5, 86},
    {73, 5.75, 6.5, 87},
    {72, 6.75, 8.0, 80},
    {65, 8.0, 8.5, 81},
    {63, 8.75, 9.5, 78},
    {70, 10.0, 10.25, 73},
    {72, 10.375, 10.75, 88},
    {70, 14.5, 15.75, 74},
    {70, 16.0, 16.5, 79},
    {72, 17.0, 17.5, 88},
    {75, 17.75, 18.5, 87},
    {73, 18.5, 20.0, 83},
    {72, 20.0, 20.5, 77},
    {73, 20.75, 21.5, 72},
    {77, 21.75, 22.5, 76},
    {75, 22.75, 24.0, 73},
    {68, 24.0, 24.5, 81},
    {66, 25.0, 25.5, 79},
    {73, 26.0, 26.25, 73},
    {75, 26.375, 26.75, 81},
    {65, 30.5, 31.75, 74},
    {77, 32.0, 32.5, 83},
    {78, 33.0, 33.5, 102},
    {70, 34.0, 34.5, 78},
    {68, 34.75, 36.0, 72},
    {78, 36.0, 36.5, 81},
    {68, 36.75, 37.5, 80},
    {72, 38.0, 38.5, 82},
    {70, 38.75, 40.0, 84},
    {75, 40.0, 40.5, 81},
    {73, 41.0, 41.5, 85},
    {68, 42.0, 42.25, 86},
    {70, 42.375, 42.75, 85},
    {73, 46.5, 47.75, 74},
    {61, 48.0, 48.625, 76},
    {60, 48.938, 49.876000000000005, 88},
    {68, 50.438, 51.126000000000005, 87},
    {58, 51.125, 53.0, 74},
    {63, 52.25, 52.625, 77},
    {61, 52.938, 53.876000000000005, 78},
    {58, 54.188, 55.126000000000005, 83},
    {60, 55.125, 57.0, 77},
    {60, 56.25, 56.625, 82},
    {61, 56.938, 57.876000000000005, 77},
    {65, 58.0, 58.312, 79},
    {63, 58.469, 58.938, 73},
    {63, 62.5, 63.75, 74},
  }
  for _, n in ipairs(N) do
    local sp = reaper.MIDI_GetPPQPosFromProjQN(take, n[2])
    local ep = reaper.MIDI_GetPPQPosFromProjQN(take, n[3])
    reaper.MIDI_InsertNote(take, true, false, sp, ep, 0, n[1], n[4], true)
  end
  reaper.MIDI_Sort(take)
  reaper.GetSetMediaItemTakeInfo_String(take, "P_NAME", [[02_neo_soul]], true)
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
