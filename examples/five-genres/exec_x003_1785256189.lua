local LOG = [[C:\Wizardry\media\audio\reapermcp\examples\five-genres\exec_x003_1785256189.log]]
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
    {63, 0.0, 1.0, 83},
    {65, 1.0, 1.75, 88},
    {68, 1.75, 2.75, 77},
    {66, 2.75, 4.0, 81},
    {65, 4.0, 5.0, 77},
    {66, 5.0, 5.75, 86},
    {70, 5.75, 6.75, 87},
    {68, 6.75, 8.0, 80},
    {61, 8.0, 8.75, 81},
    {60, 8.75, 9.5, 78},
    {70, 13.0, 15.0, 76},
    {66, 16.0, 17.0, 75},
    {68, 17.0, 18.0, 88},
    {72, 18.0, 18.75, 79},
    {70, 18.75, 20.0, 88},
    {68, 20.0, 20.75, 87},
    {70, 20.75, 21.75, 83},
    {73, 21.75, 22.5, 77},
    {72, 22.5, 24.0, 72},
    {65, 24.0, 25.0, 76},
    {63, 25.0, 25.5, 73},
    {63, 27.5, 31.0, 76},
    {70, 32.0, 33.0, 81},
    {72, 33.0, 34.0, 79},
    {75, 34.0, 34.5, 73},
    {73, 34.5, 36.0, 81},
    {72, 36.0, 37.0, 74},
    {73, 37.0, 38.0, 88},
    {79, 38.0, 38.75, 92},
    {75, 38.75, 40.0, 72},
    {68, 40.0, 40.75, 81},
    {66, 40.75, 41.5, 80},
    {70, 45.0, 47.0, 76},
    {61, 48.0, 49.188, 84},
    {60, 49.188, 50.188, 86},
    {68, 50.188, 51.125, 74},
    {58, 51.125, 52.25, 81},
    {63, 52.25, 53.188, 85},
    {61, 53.188, 54.188, 86},
    {58, 54.188, 55.375, 85},
    {60, 55.375, 56.0, 76},
    {60, 56.0, 57.188, 88},
    {61, 57.188, 57.876000000000005, 87},
    {63, 59.5, 63.0, 76},
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
