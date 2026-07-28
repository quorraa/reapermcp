"""Render one track alone and measure it - the ground truth for "did it load".

Parameter reads and vst_chunk both report a default instrument whether or not a
patch was installed, so neither can settle the question. Rendered audio can.
"""
import os
import sys

from loudness import measure
from reaper_exec import run_lua

OUT_DIR = os.environ.get("QLABS_SONGS_DIR", r"C:\Wizardry\media\audio\qlabs-songs")
STEM_DIR = os.path.join(OUT_DIR, "stems")

BODY = """
reaper.Main_openProject("noprompt:" .. RPP)
local proj = 0

local saved = {}
local keep = nil
for t = 0, reaper.CountTracks(proj) - 1 do
  local tr = reaper.GetTrack(proj, t)
  local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
  saved[t] = reaper.GetMediaTrackInfo_Value(tr, "D_VOL")
  if tn == TRACK then keep = t end
end
if not keep then say("track not found") return end

for t = 0, reaper.CountTracks(proj) - 1 do
  local tr = reaper.GetTrack(proj, t)
  local depth = reaper.GetMediaTrackInfo_Value(tr, "I_FOLDERDEPTH")
  reaper.SetMediaTrackInfo_Value(tr, "B_MUTE", 0)
  if t == keep or depth == 1 then
    reaper.SetMediaTrackInfo_Value(tr, "D_VOL", saved[t])
  else
    reaper.SetMediaTrackInfo_Value(tr, "D_VOL", 0.0)
  end
end

reaper.GetSetProjectInfo_String(proj, "RENDER_FILE", STEMDIR, true)
reaper.GetSetProjectInfo_String(proj, "RENDER_PATTERN", NAME, true)
reaper.GetSetProjectInfo(proj, "RENDER_SETTINGS", 0, true)
reaper.GetSetProjectInfo(proj, "RENDER_BOUNDSFLAG", 1, true)
reaper.GetSetProjectInfo(proj, "RENDER_CHANNELS", 2, true)
reaper.GetSetProjectInfo(proj, "RENDER_SRATE", 48000, true)
reaper.GetSetProjectInfo(proj, "RENDER_ADDTOPROJ", 0, true)
reaper.Main_OnCommand(41824, 0)

for t = 0, reaper.CountTracks(proj) - 1 do
  reaper.SetMediaTrackInfo_Value(reaper.GetTrack(proj, t), "D_VOL", saved[t])
end
say("rendered")
"""


def render(song_id, track, name):
    out = os.path.join(STEM_DIR, name + ".wav")
    if os.path.exists(out):
        os.remove(out)
    body = ("local RPP = [[%s]]\nlocal TRACK = [[%s]]\nlocal NAME = [[%s]]\n"
            "local STEMDIR = [[%s]]\n%s"
            % (os.path.join(OUT_DIR, song_id + ".RPP"), track, name,
               STEM_DIR, BODY))
    for line in run_lua(body, timeout=1200):
        if "not found" in line or "LUA_ERROR" in line:
            print("   " + line)
    if not os.path.exists(out):
        return None
    return measure(out)


if __name__ == "__main__":
    song, track, name = sys.argv[1], sys.argv[2], sys.argv[3]
    m = render(song, track, name)
    if m:
        print("%-28s %7.1f LUFS  peak %6.1f dBFS" % (name, m["lufs"], m["peak_db"]))
    else:
        print("%-28s no render produced" % name)
