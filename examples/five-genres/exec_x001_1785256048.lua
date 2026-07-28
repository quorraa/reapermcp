local LOG = [[C:\Wizardry\media\audio\reapermcp\examples\five-genres\exec_x001_1785256048.log]]
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
    {69, 0.0, 2.0, 84},
    {72, 2.0, 3.0, 78},
    {74, 3.0, 4.0, 80},
    {72, 4.0, 7.0, 86},
    {69, 7.0, 8.0, 76},
    {67, 8.0, 10.0, 80},
    {70, 10.0, 11.0, 78},
    {74, 11.0, 12.0, 82},
    {72, 12.0, 16.0, 88},
    {65, 16.0, 17.0, 78},
    {69, 17.0, 18.0, 80},
    {72, 18.0, 20.0, 84},
    {70, 20.0, 22.0, 82},
    {67, 22.0, 24.0, 78},
    {69, 24.0, 26.0, 80},
    {65, 26.0, 28.0, 76},
    {67, 28.0, 32.0, 84},
    {74, 32.0, 34.0, 88},
    {72, 34.0, 35.0, 80},
    {69, 35.0, 36.0, 78},
    {70, 36.0, 39.0, 84},
    {67, 39.0, 40.0, 78},
    {76, 40.0, 42.0, 90},
    {74, 42.0, 44.0, 84},
    {72, 44.0, 48.0, 86},
    {69, 48.0, 49.0, 80},
    {72, 49.0, 50.0, 84},
    {77, 50.0, 52.0, 96},
    {76, 52.0, 54.0, 88},
    {72, 54.0, 56.0, 82},
    {74, 56.0, 57.0, 82},
    {72, 57.0, 58.0, 80},
    {69, 58.0, 59.0, 78},
    {67, 59.0, 60.0, 76},
    {65, 60.0, 64.0, 88},
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
