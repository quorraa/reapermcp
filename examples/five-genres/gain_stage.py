"""Set every fader from measured stem loudness instead of from guesswork.

Each track is rendered alone and measured with BS.1770 integrated loudness, then
its fader is moved by exactly the difference between what it measured and what
the target balance says it should be, relative to the lead. Finally the master
is trimmed so the finished mix lands on a sensible absolute level.

The balance below is expressed in dB relative to the lead, which is the only way
it stays meaningful across five pieces whose instruments differ.
"""
import math
import os
import subprocess
import sys

from loudness import measure
from reaper_exec import run_lua

OUT_DIR = os.environ.get("QLABS_SONGS_DIR", r"C:\Wizardry\media\audio\qlabs-songs")
STEM_DIR = os.path.join(OUT_DIR, "stems")

TARGET_MASTER_LUFS = -16.0
TARGET_PEAK_DBFS = -1.5   # keep real headroom; loudness must not win over this

# dB relative to the lead
BALANCE = {
    "Melody": 0.0,
    "QLabsBass": -4.0,
    "QLabsHarmony": -9.0,
    "QLabsCountermelody": -14.0,
    "Kick": -3.0,
    "Snare": -6.0,
    "Ride": -13.0,
    "Hats": -15.0,
}

MAX_TRIM_DB = 40.0        # never move a fader by more than this in one pass

# No single track may peak above this. Balancing purely by integrated loudness
# ignores crest factor, and a sparsely played kick has low integrated loudness
# with very large individual hits: matching it to a loudness target drove its
# peaks past full scale, which no master trim can undo because the clipping
# happens upstream of the master fader.
TRACK_PEAK_CEILING_DBFS = -6.0


def stem_path(song_id, track):
    return os.path.join(STEM_DIR, "%s__%s.wav" % (song_id, track))


def measured(song_id):
    """-> {track: (lufs, peak_db)} for every stem that carries signal."""
    out = {}
    for track in BALANCE:
        p = stem_path(song_id, track)
        m = measure(p) if os.path.exists(p) else None
        if m is None:
            print("  %-20s no usable render, left alone" % track)
        elif m["lufs"] > -69.0:            # silent stems carry no information
            out[track] = (m["lufs"], m["peak_db"])
    return out


class NoLead(Exception):
    """The lead stem is missing or silent, so there is nothing to balance against."""


def gains_for(song_id):
    m = measured(song_id)
    if "Melody" not in m:
        # Not fatal to the batch: one song that fails to render should not stop
        # the other four from being staged.
        raise NoLead("%s: no lead stem to reference" % song_id)
    # Anchored on the lead: the lead never moves, everything else is placed
    # around it, and absolute level is the master's job.
    #
    # An earlier version subtracted the largest correction from every track so
    # that all corrections were cuts, to stay clear of REAPER's fader ceiling.
    # That diverges: when one track sits below its target and needs lifting, the
    # rule pulls every other track down by that amount instead, and the whole
    # mix walks toward silence a pass at a time.
    lead = m["Melody"][0]
    gains, capped = {}, []
    for track, (lufs, peak) in m.items():
        want = lead + BALANCE[track]
        g = want - lufs
        headroom = TRACK_PEAK_CEILING_DBFS - peak
        if g > headroom:               # would push this track's peaks too high
            capped.append((track, g, headroom))
            g = headroom
        gains[track] = max(-MAX_TRIM_DB, min(MAX_TRIM_DB, g))
    for track, wanted, allowed in capped:
        print("     %-20s loudness wants %+.1f dB but peaks allow %+.1f - capped"
              % (track, wanted, allowed))
    return gains, {k: v[0] for k, v in m.items()}


def apply_gains(song_id, gains, master_db=0.0):
    rpp = os.path.join(OUT_DIR, song_id + ".RPP")
    wav = os.path.join(OUT_DIR, song_id + ".wav")
    if os.path.exists(wav):
        os.remove(wav)
    L = []
    add = L.append
    add('reaper.Main_openProject("noprompt:" .. [[%s]])' % rpp)
    add("local proj = 0")
    add("local G = {}")
    for track, db in gains.items():
        add('G["%s"] = %r' % (track, float(10 ** (db / 20.0))))
    add("""
for t = 0, reaper.CountTracks(proj) - 1 do
  local tr = reaper.GetTrack(proj, t)
  local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
  local g = G[tn]
  if g then
    local v = reaper.GetMediaTrackInfo_Value(tr, "D_VOL") * g
    v = math.max(0.0001, math.min(4.0, v))
    reaper.SetMediaTrackInfo_Value(tr, "D_VOL", v)
    say(string.format("  %-20s x%.3f -> %.4f", tn, g, v))
  end
end
""")
    add("local master = reaper.GetMasterTrack(proj)")
    add('reaper.SetMediaTrackInfo_Value(master, "D_VOL",'
        ' reaper.GetMediaTrackInfo_Value(master, "D_VOL") * %r)' % float(10 ** (master_db / 20.0)))
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


TOLERANCE_DB = 1.5
MAX_PASSES = 2

MASTER_PREP = """
reaper.Main_openProject("noprompt:" .. RPP)
local m = reaper.GetMasterTrack(0)

-- The master fader is recomputed from scratch every run. It used to be
-- multiplied by each pass's correction, so it accumulated across runs and had
-- crept to +14 dB, which is most of why the mixes were slamming into the
-- limiter below.
say(string.format("  master fader was %+.1f dB%s",
  20 * math.log(math.max(reaper.GetMediaTrackInfo_Value(m, "D_VOL"), 1e-9), 10),
  RESET and ", reset to 0.0" or " (left as is)"))
reaper.SetMediaTrackInfo_Value(m, "D_VOL", 1.0)

-- The limiter comes out of the measurement path. Gain staging assumes moving a
-- fader by X dB moves the measured level by X dB; a limiter clamps whatever
-- arrives, so every stem measures limited, every correction is computed from a
-- clamped number, and the loop never converges. It also pins the reported peak
-- at 0 dBFS regardless of what the faders are doing.
for x = 0, reaper.TrackFX_GetCount(m) - 1 do
  local _, nm = reaper.TrackFX_GetFXName(m, x, "")
  if tostring(nm):find("ReaLimit", 1, true) then
    reaper.TrackFX_SetEnabled(m, x, LIMITER)
    say("  master limiter " .. (LIMITER and "re-enabled" or "bypassed for measurement"))
  end
end
-- Track faders are deliberately left alone. Resetting them here means a later
-- failure leaves the project with no balance at all, and the correction below is
-- anchored on the lead, so it converges from wherever the faders start.
reaper.Main_SaveProjectEx(0, RPP, 0)
"""


def master_prep(song_id, limiter=False, reset_fader=True):
    rpp = os.path.join(OUT_DIR, song_id + ".RPP")
    body = MASTER_PREP if reset_fader else MASTER_PREP.replace(
        'reaper.SetMediaTrackInfo_Value(m, "D_VOL", 1.0)', "")
    named = "\n".join('NAMED["%s"] = true' % t for t in BALANCE)
    return run_lua("local RPP = [[%s]]\nlocal LIMITER = %s\nlocal RESET = %s\n"
                   "local NAMED = {}\n%s\n%s"
                   % (rpp, "true" if limiter else "false",
                      "true" if reset_fader else "false", named, body),
                   timeout=600)


def restems(song_id):
    """Re-render the stems, and say so plainly if it did not work."""
    r = subprocess.run([sys.executable, "render_stems.py", song_id], check=False)
    if r.returncode != 0:
        print("  render_stems exited %d" % r.returncode)
        return False
    return True


def stage(song_id, passes=MAX_PASSES):
    print("=" * 66)
    print(song_id)
    for line in master_prep(song_id, limiter=False):
        print(line)
    # Measure the project as it is now. Stems left on disk describe whatever
    # state they were rendered in, which is not necessarily this one, and a
    # correction computed from them corrects the wrong thing.
    if not restems(song_id):
        print("  stem render failed; refusing to correct from stale stems")
        return
    for p in range(1, passes + 1):
        gains, m = gains_for(song_id)
        lead = m["Melody"]
        worst = max(abs(g) for g in gains.values())
        print("  -- pass %d (worst error %.1f dB)" % (p, worst))
        for track in sorted(gains, key=lambda k: -m[k]):
            print("     %-20s measured %7.1f  want %7.1f  trim %+6.1f dB"
                  % (track, m[track], lead + BALANCE[track], gains[track]))
        if worst <= TOLERANCE_DB:
            print("  converged")
            break
        for line in apply_gains(song_id, gains):
            print(line)
        # re-render the stems so the next pass measures what we just did
        if not restems(song_id):
            print("  stem render failed; stopping before it corrects from stale data")
            return

    # Render the mix before measuring it. When the balance converges on the
    # first pass nothing else renders, and the file left on disk belongs to an
    # earlier run: the trim then gets computed from a mix that no longer exists.
    # That is how a +1.8 dB trim came out 15 dB away from where it aimed.
    for line in apply_gains(song_id, {}):
        print(line)

    # master trim: one measurement, one correction
    wav = os.path.join(OUT_DIR, song_id + ".wav")
    if not os.path.exists(wav):
        print("  render missing, cannot trim master")
        return
    # The trim iterates. One correction is only exact when the measurement is
    # exact, and a clipped render reports its peak as -0.0 however far over it
    # actually is, so the first correction can only be a lower bound on what is
    # needed. Re-measuring after each move converges on it.
    for attempt in range(1, 4):
        got = measure(wav)
        if got is None:
            print("  render unreadable, cannot trim master")
            return
        # Whichever constraint binds first wins: hitting a loudness target by
        # pushing the peak to 0 dBFS is not a mix, it is a clipped mix.
        by_loud = TARGET_MASTER_LUFS - got["lufs"]
        by_peak = TARGET_PEAK_DBFS - got["peak_db"]
        trim = min(by_loud, by_peak)
        print("  mix measured %.1f LUFS (peak %.1f) -> trim %+.1f dB "
              "(loudness wants %+.1f, peak allows %+.1f)"
              % (got["lufs"], got["peak_db"], trim, by_loud, by_peak))
        if abs(trim) <= 0.5:
            break
        for line in apply_gains(song_id, {}, master_db=trim):
            print(line)

    got = measure(wav)
    if got:
        print("  final: %.1f LUFS, peak %.1f dBFS" % (got["lufs"], got["peak_db"]))

    print("  master limiter left bypassed: staging holds the peak at %.1f dBFS,"
          " which is a lower ceiling than the limiter was enforcing"
          % TARGET_PEAK_DBFS)


if __name__ == "__main__":
    for song_id in (sys.argv[1:] or ["01_jazz_ballad", "02_neo_soul",
                                     "03_drum_and_bass", "04_cinematic", "05_blues"]):
        try:
            stage(song_id)
        except NoLead as e:
            print("  %s - skipped" % e)
        except Exception as e:
            print("  %s: %s - skipped" % (type(e).__name__, e))
