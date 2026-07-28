"""Render each track of a project on its own, so its level can be measured.

Soloing one track at a time and rendering the master is slower than REAPER's
stem mode but completely unambiguous about which file is which track, which
matters more here than speed.
"""
import os
import sys

from reaper_exec import run_lua

OUT_DIR = os.environ.get("QLABS_SONGS_DIR", r"C:\Wizardry\media\audio\qlabs-songs")
STEM_DIR = os.path.join(OUT_DIR, "stems")

# tracks worth measuring; the folder track and the muted duplicate are skipped
MEASURE = ("Melody", "QLabsHarmony", "QLabsBass", "QLabsCountermelody",
           "Kick", "Snare", "Hats", "Ride")


def lua_str(s):
    return "[[" + s + "]]"


def render_stems(song_id):
    os.makedirs(STEM_DIR, exist_ok=True)
    # REAPER prompts instead of overwriting, and a modal dialog stalls the whole
    # run while leaving the previous (possibly wrong) file in place.
    for fn in os.listdir(STEM_DIR):
        if fn.startswith(song_id + "__"):
            os.remove(os.path.join(STEM_DIR, fn))
    rpp = os.path.join(OUT_DIR, song_id + ".RPP")
    L = []
    add = L.append
    add('reaper.Main_openProject("noprompt:" .. %s)' % lua_str(rpp))
    add("local proj = 0")
    add("local WANT = {%s}" % ", ".join('["%s"]=true' % t for t in MEASURE))
    add("local STEMDIR = %s" % lua_str(STEM_DIR))
    add("local SONG = %s" % lua_str(song_id))
    add("""
-- Isolate by VOLUME, not mute or solo. These parts live inside a folder and
-- route through its parent: muting everything else also mutes the parent and
-- silences the children, while setting I_SOLO directly does not reproduce
-- REAPER's implicit parent handling, which left the last track in the folder
-- rendering as digital silence. Zeroing the volume of the other *leaf* tracks
-- and leaving folder parents at unity has no such semantics to get wrong.
local saved = {}
for t = 0, reaper.CountTracks(proj) - 1 do
  local tr = reaper.GetTrack(proj, t)
  saved[t] = { reaper.GetMediaTrackInfo_Value(tr, "D_VOL"),
               reaper.GetMediaTrackInfo_Value(tr, "B_MUTE") }
end

local function is_folder(tr)
  return reaper.GetMediaTrackInfo_Value(tr, "I_FOLDERDEPTH") == 1
end

local function render_only(keep_idx, name)
  for t = 0, reaper.CountTracks(proj) - 1 do
    local tr = reaper.GetTrack(proj, t)
    reaper.SetMediaTrackInfo_Value(tr, "B_MUTE", 0)
    if t == keep_idx or is_folder(tr) then
      reaper.SetMediaTrackInfo_Value(tr, "D_VOL", saved[t][1])
    else
      reaper.SetMediaTrackInfo_Value(tr, "D_VOL", 0.0)
    end
  end
  reaper.GetSetProjectInfo_String(proj, "RENDER_FILE", STEMDIR, true)
  reaper.GetSetProjectInfo_String(proj, "RENDER_PATTERN", SONG .. "__" .. name, true)
  reaper.GetSetProjectInfo(proj, "RENDER_SETTINGS", 0, true)
  reaper.GetSetProjectInfo(proj, "RENDER_BOUNDSFLAG", 1, true)
  reaper.GetSetProjectInfo(proj, "RENDER_CHANNELS", 2, true)
  reaper.GetSetProjectInfo(proj, "RENDER_SRATE", 48000, true)
  reaper.GetSetProjectInfo(proj, "RENDER_ADDTOPROJ", 0, true)
  reaper.Main_OnCommand(41824, 0)
  say("  rendered stem: " .. name)
end

for t = 0, reaper.CountTracks(proj) - 1 do
  local tr = reaper.GetTrack(proj, t)
  local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
  if WANT[tn] and saved[t][2] == 0 then
    render_only(t, tn)
  end
end

-- the whole kit together, which is what the bus compressor actually sees
local DRUMTRACKS = { Kick=true, Snare=true, Hats=true, Ride=true }
local any = false
for t = 0, reaper.CountTracks(proj) - 1 do
  local tr = reaper.GetTrack(proj, t)
  local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
  if DRUMTRACKS[tn] then any = true end
end
if any then
  for t = 0, reaper.CountTracks(proj) - 1 do
    local tr = reaper.GetTrack(proj, t)
    local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
    reaper.SetMediaTrackInfo_Value(tr, "B_MUTE", 0)
    if DRUMTRACKS[tn] or is_folder(tr) then
      reaper.SetMediaTrackInfo_Value(tr, "D_VOL", saved[t][1])
    else
      reaper.SetMediaTrackInfo_Value(tr, "D_VOL", 0.0)
    end
  end
  reaper.GetSetProjectInfo_String(proj, "RENDER_FILE", STEMDIR, true)
  reaper.GetSetProjectInfo_String(proj, "RENDER_PATTERN", SONG .. "__DrumBus", true)
  reaper.GetSetProjectInfo(proj, "RENDER_SETTINGS", 0, true)
  reaper.GetSetProjectInfo(proj, "RENDER_BOUNDSFLAG", 1, true)
  reaper.GetSetProjectInfo(proj, "RENDER_CHANNELS", 2, true)
  reaper.GetSetProjectInfo(proj, "RENDER_SRATE", 48000, true)
  reaper.GetSetProjectInfo(proj, "RENDER_ADDTOPROJ", 0, true)
  reaper.Main_OnCommand(41824, 0)
  say("  rendered stem: DrumBus")
end

for t = 0, reaper.CountTracks(proj) - 1 do
  local tr = reaper.GetTrack(proj, t)
  reaper.SetMediaTrackInfo_Value(tr, "D_VOL", saved[t][1])
  reaper.SetMediaTrackInfo_Value(tr, "B_MUTE", saved[t][2])
end
say("  levels restored")
""")
    return run_lua("\n".join(L), timeout=1500)


if __name__ == "__main__":
    ids = sys.argv[1:] or ["01_jazz_ballad", "02_neo_soul", "03_drum_and_bass",
                           "04_cinematic", "05_blues"]
    for song_id in ids:
        print(song_id)
        for line in render_stems(song_id):
            print(line)
