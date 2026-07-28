"""Put a compressor across the drum bus.

Three of the five mixes were peak-limited rather than loudness-limited: their
drum transients hit the ceiling long before the mix reached its loudness target,
so raising the master only bought clipping. Compressing the drum bus lowers the
crest factor, which is the only thing that lets the mix sit higher at the same
peak.

The compressor goes on the Drums *folder* track, so it acts on the kit as a
whole - that is what glues a kit together, and it is only possible now that kick,
snare, hats and ride are separate tracks feeding a common parent.

ReaComp mappings measured from the plugin:
  ratio    = 1 + norm * 99          (default 4.00 at norm 0.0303)
  attack   = norm * 500 ms          (default 3.0 at norm 0.0060)
  release  = norm * 5000 ms         (default 100 at norm 0.0200)
Threshold is searched against the plugin's own readout rather than assumed.
"""
import os
import subprocess
import sys

from reaper_exec import run_lua

OUT_DIR = os.environ.get("QLABS_SONGS_DIR", r"C:\Wizardry\media\audio\qlabs-songs")

# Threshold sits above the bus's *loudness*, not below its peak. Referencing the
# peak put the threshold barely under the loudest transient, so only the very
# tips crossed it and nothing was reduced. The drum bus has an ~18 dB crest, so
# a threshold a few dB over its loudness is what the transients actually meet.
THRESHOLD_ABOVE_LUFS_DB = 6.0
RATIO = 4.0
ATTACK_MS = 10.0        # glue, not peak control: let the stick through and
                        # squeeze what follows. A compressor cannot fix this
                        # mix's crest anyway - see the note in pass_over().
RELEASE_MS = 140.0
KNEE_DB = 6.0


def norm_ratio(r):
    return (r - 1.0) / 99.0


def norm_ms(ms, full):
    return max(0.0, min(1.0, ms / full))


def bus_measure(song_id):
    from loudness import measure
    p = os.path.join(OUT_DIR, "stems", "%s__DrumBus.wav" % song_id)
    return measure(p) if os.path.exists(p) else None


def add_bus_comp(song_id, makeup_gain=None):
    m = bus_measure(song_id)
    if m is None:
        print("  no DrumBus stem for %s" % song_id)
        return []
    threshold = m["lufs"] + THRESHOLD_ABOVE_LUFS_DB
    print("  bus %.1f LUFS / peak %.1f (crest %.1f) -> want %.1f dB at the render"
          % (m["lufs"], m["peak_db"], m["peak_db"] - m["lufs"], threshold))
    rpp = os.path.join(OUT_DIR, song_id + ".RPP")
    wav = os.path.join(OUT_DIR, song_id + ".wav")
    if os.path.exists(wav):
        os.remove(wav)
    L = []
    add = L.append
    add('reaper.Main_openProject("noprompt:" .. [[%s]])' % rpp)
    add("local proj = 0")
    add("local WANT_THRESH = %r" % float(threshold))  # corrected in Lua
    add("local MAKEUP = %s" % ("nil" if makeup_gain is None else repr(float(makeup_gain))))
    add("local N_RATIO   = %r" % norm_ratio(RATIO))
    add("local N_ATTACK  = %r" % norm_ms(ATTACK_MS, 500.0))
    add("local N_RELEASE = %r" % norm_ms(RELEASE_MS, 5000.0))
    add("local N_KNEE    = %r" % (KNEE_DB / 30.0))
    add("""
local function find_drum_folder()
  for t = 0, reaper.CountTracks(proj) - 1 do
    local tr = reaper.GetTrack(proj, t)
    local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
    if tn == "Drums" and reaper.GetMediaTrackInfo_Value(tr, "I_FOLDERDEPTH") == 1 then
      return tr
    end
  end
  return nil
end

local bus = find_drum_folder()
if not bus then say("  no Drums folder in this project") return end

-- do not stack a second compressor if this has already been run
for x = reaper.TrackFX_GetCount(bus) - 1, 0, -1 do
  local _, nm = reaper.TrackFX_GetFXName(bus, x, "")
  if nm:find("ReaComp", 1, true) then reaper.TrackFX_Delete(bus, x) end
end

local fx = reaper.TrackFX_AddByName(bus, "VST: ReaComp (Cockos)", false, -1)
if fx < 0 then say("  could not add ReaComp") return end

-- Referred to the render, but applied where the compressor actually sits: ahead
-- of the folder fader and the master fader. Skipping this correction is what
-- made the first two attempts reduce exactly nothing - the threshold was set
-- above the loudest thing on the bus rather than into it.
local function todb(v) return 20 * math.log(math.max(v, 1e-9), 10) end
local OFFSET = todb(reaper.GetMediaTrackInfo_Value(bus, "D_VOL"))
             + todb(reaper.GetMediaTrackInfo_Value(reaper.GetMasterTrack(proj), "D_VOL"))
WANT_THRESH = WANT_THRESH - OFFSET
say(string.format("  gain between compressor and render: %+.2f dB -> threshold %.1f dB",
  OFFSET, WANT_THRESH))

-- Threshold: search the normalised value that yields the wanted dB, because
-- the parameter range is not documented and guessing it is how you end up
-- compressing nothing or everything.
local lo, hi = 0.0, 1.0
for _ = 1, 40 do
  local mid = (lo + hi) / 2
  reaper.TrackFX_SetParamNormalized(bus, fx, 0, mid)
  local _, s = reaper.TrackFX_GetFormattedParamValue(bus, fx, 0, "")
  local v = tonumber((tostring(s):gsub("[^%-%d%.]", "")))
  if v == nil then break end
  if v < WANT_THRESH then lo = mid else hi = mid end
end
reaper.TrackFX_SetParamNormalized(bus, fx, 1, N_RATIO)
reaper.TrackFX_SetParamNormalized(bus, fx, 2, N_ATTACK)
reaper.TrackFX_SetParamNormalized(bus, fx, 3, N_RELEASE)
reaper.TrackFX_SetParamNormalized(bus, fx, 14, N_KNEE)

local function shown(p)
  local _, s = reaper.TrackFX_GetFormattedParamValue(bus, fx, p, "")
  return tostring(s)
end
say(string.format("BUSVOL %.9f", reaper.GetMediaTrackInfo_Value(bus, "D_VOL")))
if MAKEUP then
  -- set, not multiply: re-running must not compound the makeup
  reaper.SetMediaTrackInfo_Value(bus, "D_VOL", MAKEUP)
end
say(string.format("  bus comp: threshold %s  ratio %s  attack %s  release %s  knee %s  makeup applied",
  shown(0), shown(1), shown(2), shown(3), shown(14)))
""")
    add("reaper.Main_SaveProjectEx(proj, [[%s]], 0)" % rpp)
    add('reaper.GetSetProjectInfo_String(proj, "RENDER_FILE", [[%s]], true)' % OUT_DIR)
    add('reaper.GetSetProjectInfo_String(proj, "RENDER_PATTERN", [[%s]], true)' % song_id)
    add('reaper.GetSetProjectInfo(proj, "RENDER_SETTINGS", 0, true)')
    add('reaper.GetSetProjectInfo(proj, "RENDER_BOUNDSFLAG", 1, true)')
    add('reaper.GetSetProjectInfo(proj, "RENDER_CHANNELS", 2, true)')
    add('reaper.GetSetProjectInfo(proj, "RENDER_SRATE", 48000, true)')
    add('reaper.GetSetProjectInfo(proj, "RENDER_ADDTOPROJ", 0, true)')
    add("reaper.Main_OnCommand(41824, 0)")
    add('say("  rendered")')
    return run_lua("\n".join(L), timeout=1200)


def bus_lufs(song_id):
    m = bus_measure(song_id)
    return m["lufs"] if m else None


STRIP = """
local function find_drum_folder()
  for t = 0, reaper.CountTracks(0) - 1 do
    local tr = reaper.GetTrack(0, t)
    local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
    if tn == "Drums" and reaper.GetMediaTrackInfo_Value(tr, "I_FOLDERDEPTH") == 1 then
      return tr
    end
  end
end
reaper.Main_openProject("noprompt:" .. RPP)
local bus = find_drum_folder()
if bus then
  for x = reaper.TrackFX_GetCount(bus) - 1, 0, -1 do
    local _, nm = reaper.TrackFX_GetFXName(bus, x, "")
    if nm:find("ReaComp", 1, true) then reaper.TrackFX_Delete(bus, x) end
  end
  reaper.SetMediaTrackInfo_Value(bus, "D_VOL", 1.0)
  reaper.Main_SaveProjectEx(0, RPP, 0)
  say("  stripped previous bus comp, folder fader back to unity")
end
"""


def strip(song_id):
    """Remove any earlier compressor so the baseline is the uncompressed bus.

    Measuring "before" through a compressor left by the last run makes each pass
    derive its threshold from the previous pass's output, which walks the
    threshold down a little further every time it is run.
    """
    rpp = os.path.join(OUT_DIR, song_id + ".RPP")
    for line in run_lua("local RPP = [[%s]]\n%s" % (rpp, STRIP), timeout=600):
        print(line)


def restems(song_id):
    subprocess.run([sys.executable, "render_stems.py", song_id], check=False,
                   stdout=subprocess.DEVNULL)


def pass_over(song_id):
    print("=" * 66)
    print(song_id)
    strip(song_id)
    restems(song_id)
    b = bus_measure(song_id)
    if b is None:
        print("  no drum bus in this project, skipped")
        return
    before = b["lufs"]
    before_crest = b["peak_db"] - before
    print("  drum bus before: %.1f LUFS / peak %.1f (crest %.1f)"
          % (before, b["peak_db"], before_crest))

    base = 1.0
    for line in add_bus_comp(song_id):        # no makeup yet, just measure it
        if line.strip().startswith("BUSVOL "):
            base = float(line.strip().split()[1])
            continue
        print(line)

    restems(song_id)
    a = bus_measure(song_id)
    after = a["lufs"]
    loss = before - after
    print("  drum bus after:  %.1f LUFS / peak %.1f (crest %.1f)  "
          "-- compressor took %.1f dB, crest %+.1f dB"
          % (after, a["peak_db"], a["peak_db"] - after,
             loss, (a["peak_db"] - after) - (before_crest)))

    # Give back exactly what the compressor removed: same loudness, lower crest.
    for line in add_bus_comp(song_id, makeup_gain=base * 10 ** (loss / 20.0)):
        if not line.strip().startswith("BUSVOL "):
            print(line)
    print("  makeup %+.1f dB applied to the bus (fader %.4f -> %.4f)"
          % (loss, base, base * 10 ** (loss / 20.0)))


if __name__ == "__main__":
    for song_id in (sys.argv[1:] or ["01_jazz_ballad", "02_neo_soul",
                                     "03_drum_and_bass", "05_blues"]):
        pass_over(song_id)
