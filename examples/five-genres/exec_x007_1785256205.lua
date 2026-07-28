local LOG = [[C:\Wizardry\media\audio\reapermcp\examples\five-genres\exec_x007_1785256205.log]]
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
  reaper.SetCurrentBPM(proj, 60.0, false)
  reaper.SetTempoTimeSigMarker(proj, -1, 0.0, -1, -1, 60.0, 4, 4, false)
  reaper.InsertTrackAtIndex(0, true)
  local tr = reaper.GetTrack(proj, 0)
  reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "Melody", true)
  local t0 = reaper.TimeMap2_QNToTime(proj, 0.0)
  local t1 = reaper.TimeMap2_QNToTime(proj, 64.0)
  local item = reaper.CreateNewMIDIItemInProj(tr, t0, t1, false)
  local take = reaper.GetActiveTake(item)
  local N = {
    {62, 0.0, 2.25, 80},
    {67, 2.25, 4.0, 82},
    {64, 4.0, 4.25, 74},
    {65, 4.25, 6.0, 82},
    {69, 6.0, 8.0, 82},
    {67, 8.0, 8.25, 78},
    {60, 8.25, 10.25, 81},
    {55, 10.25, 12.0, 74},
    {69, 13.0, 15.0, 76},
    {69, 16.0, 18.25, 73},
    {74, 18.25, 20.0, 86},
    {72, 20.0, 20.25, 87},
    {70, 20.25, 22.0, 80},
    {76, 22.0, 24.0, 73},
    {67, 24.0, 26.0, 72},
    {62, 27.5, 31.0, 76},
    {69, 32.0, 34.0, 72},
    {74, 34.0, 36.0, 79},
    {72, 36.0, 36.25, 78},
    {70, 36.25, 38.0, 72},
    {78, 38.0, 40.0, 99},
    {67, 40.0, 40.25, 88},
    {74, 40.25, 42.25, 76},
    {62, 42.25, 44.0, 73},
    {69, 45.0, 47.0, 76},
    {67, 48.0, 50.5, 86},
    {62, 50.5, 52.25, 82},
    {69, 52.25, 53.0, 77},
    {64, 53.0, 54.5, 72},
    {64, 54.5, 56.0, 74},
    {65, 56.0, 58.5, 86},
    {62, 59.5, 63.0, 76},
  }
  for _, n in ipairs(N) do
    local sp = reaper.MIDI_GetPPQPosFromProjQN(take, n[2])
    local ep = reaper.MIDI_GetPPQPosFromProjQN(take, n[3])
    reaper.MIDI_InsertNote(take, true, false, sp, ep, 0, n[1], n[4], true)
  end
  reaper.MIDI_Sort(take)
  reaper.GetSetMediaItemTakeInfo_String(take, "P_NAME", [[04_cinematic]], true)
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
