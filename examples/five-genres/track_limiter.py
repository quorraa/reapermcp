"""Limit the lead and harmony transients so the peak-limited mixes can sit louder.

Jazz and neo-soul stopped ~6 dB short of their loudness target with peaks already
at the ceiling: a 20 dB crest, set by the attack of the piano and the pad, not by
the drums. Bus compression on the kit cannot help with that, and measurably did
not - it took 2-3 dB and left crest slightly higher.

A limiter on the two tracks that own the transients is what actually lowers the
crest. Threshold is set relative to each track's own measured peak, so it takes
the tips off rather than flattening the part.
"""
import os
import sys

from loudness import measure
from reaper_exec import run_lua

OUT_DIR = os.environ.get("QLABS_SONGS_DIR", r"C:\Wizardry\media\audio\qlabs-songs")
STEM_DIR = os.path.join(OUT_DIR, "stems")

TRACKS = ("Melody", "QLabsHarmony")
BELOW_PEAK_DB = 6.0     # how far under the track's own peak the limiter bites
RELEASE_MS = 60.0

BODY = """
reaper.Main_openProject("noprompt:" .. RPP)
local proj = 0

for t = 0, reaper.CountTracks(proj) - 1 do
  local tr = reaper.GetTrack(proj, t)
  local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
  local want = WANT[tn]
  if want then
    -- never stack a second limiter if this is re-run
    for x = reaper.TrackFX_GetCount(tr) - 1, 0, -1 do
      local _, nm = reaper.TrackFX_GetFXName(tr, x, "")
      if tostring(nm):find("ReaLimit", 1, true) then reaper.TrackFX_Delete(tr, x) end
    end

    local fx = reaper.TrackFX_AddByName(tr, "VST: ReaLimit (Cockos)", false, -1)
    if fx < 0 then
      say("  " .. tn .. ": could not add ReaLimit")
    else
      -- Threshold and ceiling are searched against the plugin's own readout;
      -- the normalised ranges are not documented and guessing them is how you
      -- end up limiting nothing, or everything.
      local function seek(p, target)
        local lo, hi = 0.0, 1.0
        for _ = 1, 34 do
          local mid = (lo + hi) / 2
          reaper.TrackFX_SetParamNormalized(tr, fx, p, mid)
          local _, s = reaper.TrackFX_GetFormattedParamValue(tr, fx, p, "")
          local v = tonumber((tostring(s):match("(-?%d+%.?%d*)")))
          if v == nil then return false end
          if v < target then lo = mid else hi = mid end
        end
        return true
      end

      -- The wanted threshold is expressed at the render, but the limiter sits
      -- ahead of this track's fader and the master fader. Without removing that
      -- gain the threshold lands wherever the faders happen to be - and with a
      -- low fader that means limiting by tens of dB rather than shaving tips.
      local function todb(v) return 20 * math.log(math.max(v, 1e-9), 10) end
      local offset = todb(reaper.GetMediaTrackInfo_Value(tr, "D_VOL"))
                   + todb(reaper.GetMediaTrackInfo_Value(reaper.GetMasterTrack(proj), "D_VOL"))

      seek(0, want - offset)             -- threshold, at the limiter's own input
      seek(1, CEILING - offset)          -- ceiling likewise
      seek(2, RELEASE)
      say(string.format("      (%.1f dB of gain sits between here and the render)",
        offset))

      local function shown(p)
        local _, s = reaper.TrackFX_GetFormattedParamValue(tr, fx, p, "")
        return tostring(s)
      end
      say(string.format("  %-16s limiter: threshold %s  ceiling %s  release %s",
        tn, shown(0), shown(1), shown(2)))
    end
  end
end

reaper.Main_SaveProjectEx(proj, RPP, 0)
say("  saved")
"""


def apply_limiters(song_id):
    print("=" * 66)
    print(song_id)
    want = {}
    for track in TRACKS:
        p = os.path.join(STEM_DIR, "%s__%s.wav" % (song_id, track))
        m = measure(p) if os.path.exists(p) else None
        if m is None:
            print("  %-16s no stem measured, skipped" % track)
            continue
        want[track] = m["peak_db"] - BELOW_PEAK_DB
        print("  %-16s peak %.1f dBFS -> threshold %.1f dB"
              % (track, m["peak_db"], want[track]))
    if not want:
        return

    lines = ["local WANT = {}"]
    for track, thr in want.items():
        lines.append('WANT["%s"] = %r' % (track, float(thr)))
    body = ("local RPP = [[%s]]\nlocal CEILING = %r\nlocal RELEASE = %r\n%s\n%s"
            % (os.path.join(OUT_DIR, song_id + ".RPP"), -1.0, RELEASE_MS,
               "\n".join(lines), BODY))
    for line in run_lua(body, timeout=600):
        print(line)


if __name__ == "__main__":
    for s in (sys.argv[1:] or ["01_jazz_ballad", "02_neo_soul",
                               "03_drum_and_bass", "04_cinematic", "05_blues"]):
        apply_limiters(s)
