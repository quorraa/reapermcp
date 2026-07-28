local LOG = [[C:\Wizardry\media\audio\reapermcp\examples\five-genres\exec_x002_1785255232.log]]
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
  local proj = findproj([[Jazz Ballad in F]])
  if not proj then say('no project') return end
  reaper.SelectProjectInstance(proj)
  local ROLE = {}
  ROLE["QLabsMelody"] = "lead"
  ROLE["QLabsHarmony"] = "harmony"
  ROLE["QLabsBass"] = "bass"
  ROLE["QLabsCountermelody"] = "counter"
  local LEVEL = {}
  LEVEL["lead"] = 0.5
  LEVEL["harmony"] = 0.26
  LEVEL["bass"] = 0.4
  LEVEL["counter"] = 0.2
  local TIMBRE = {}
  TIMBRE["lead"] = {[0]=0.02, [1]=0.22, [2]=0.05, [3]=0.15, [4]=0.7, [6]=0.3, [7]=0.45, [9]=0.85}
  TIMBRE["harmony"] = {[0]=0.35, [1]=0.55, [2]=0.0, [3]=0.1, [4]=0.75, [6]=0.6, [7]=0.55, [9]=1.0}
  TIMBRE["bass"] = {[0]=0.02, [1]=0.15, [2]=0.0, [3]=0.2, [4]=0.65, [6]=0.3, [7]=0.55, [9]=0.8}
  TIMBRE["counter"] = {[0]=0.02, [1]=0.22, [2]=0.05, [3]=0.15, [4]=0.7, [6]=0.3, [7]=0.45, [9]=0.85}
  local FX = {}
  FX["lead"] = {[[VST: ReaEQ (Cockos)]], [[VST: ReaComp (Cockos)]]}
  FX["harmony"] = {[[VST: ReaEQ (Cockos)]], [[VST: ReaVerbate (Cockos)]]}
  FX["bass"] = {[[VST: ReaEQ (Cockos)]], [[VST: ReaComp (Cockos)]]}
  FX["counter"] = {[[VST: ReaEQ (Cockos)]], [[VST: ReaVerbate (Cockos)]]}
  FX["master"] = {[[VST: ReaVerbate (Cockos)]], [[VST: ReaLimit (Cockos)]]}
  
  for t = 0, reaper.CountTracks(proj) - 1 do
    local tr = reaper.GetTrack(proj, t)
    local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
    local role = ROLE[tn]
    if tn == "Melody" then role = "lead" end
    if role then
      -- the staged melody duplicates the source track, so silence one of them
      if tn == "QLabsMelody" then
        reaper.SetMediaTrackInfo_Value(tr, "B_MUTE", 1)
      else
        if reaper.TrackFX_GetCount(tr) == 0 then
          local ins = reaper.TrackFX_AddByName(tr, "VSTi: ReaSynth (Cockos)", false, -1)
          local tb = TIMBRE[role]
          if ins >= 0 and tb then
            for pidx, val in pairs(tb) do
              reaper.TrackFX_SetParamNormalized(tr, ins, pidx, val)
            end
          end
          local chain = FX[role]
          if chain then
            for _, fxname in ipairs(chain) do
              local fi = reaper.TrackFX_AddByName(tr, fxname, false, -1)
              if fi < 0 then say("  MISSING FX: " .. fxname) end
            end
          end
        end
        if LEVEL[role] then reaper.SetMediaTrackInfo_Value(tr, "D_VOL", LEVEL[role]) end
        say(string.format("  %-20s role=%-8s fx=%d vol=%.2f", tn, role,
          reaper.TrackFX_GetCount(tr), reaper.GetMediaTrackInfo_Value(tr, "D_VOL")))
      end
    end
  end
  
  local master = reaper.GetMasterTrack(proj)
  reaper.TrackFX_AddByName(master, [[VST: ReaVerbate (Cockos)]], false, -1)
  reaper.TrackFX_AddByName(master, [[VST: ReaLimit (Cockos)]], false, -1)
  reaper.SetMediaTrackInfo_Value(master, "D_VOL", 0.85)
  reaper.Main_SaveProjectEx(proj, [[C:/Wizardry/media/audio/qlabs-songs\01_jazz_ballad.RPP]], 0)
  say("saved " .. [[C:/Wizardry/media/audio/qlabs-songs\01_jazz_ballad.RPP]])
  reaper.GetSetProjectInfo_String(proj, "RENDER_FILE", [[C:/Wizardry/media/audio/qlabs-songs]], true)
  reaper.GetSetProjectInfo_String(proj, "RENDER_PATTERN", [[01_jazz_ballad]], true)
  reaper.GetSetProjectInfo(proj, "RENDER_SETTINGS", 0, true)
  reaper.GetSetProjectInfo(proj, "RENDER_BOUNDSFLAG", 1, true)
  reaper.GetSetProjectInfo(proj, "RENDER_CHANNELS", 2, true)
  reaper.GetSetProjectInfo(proj, "RENDER_SRATE", 48000, true)
  reaper.GetSetProjectInfo(proj, "RENDER_ADDTOPROJ", 0, true)
  reaper.Main_OnCommand(41824, 0)
  say("render issued")
end)
if not __ok then say("LUA_ERROR: " .. tostring(__err)) end

say("__DONE__")
__f:close()

-- Deliberately does not close anything by default. A workflow that spans
-- several calls - create a project here, inspect it there - is destroyed by an
-- automatic close in between. Call cleanup_tabs() when you actually want it.
if __SWEEP then __discard_tabs(1) end
