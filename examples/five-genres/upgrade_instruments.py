"""Replace ReaSynth with sampled instruments, and give the drums real sounds.

Opens each saved project, swaps the instrument on every part for a
ReaSamplOmatic5000 loaded with a synthesised sample, adds a genre-appropriate
drum kit, then re-saves and re-renders. The generated music is untouched: this
only changes what plays it.

ReaSamplOmatic5000 facts established by probing it:
  * a sample is loaded with SetNamedConfigParm FILE0 then DONE
  * params 3/4 are the note range, normalised n/127
  * params 5/6 map note -> semitone offset linearly over -80..+80, so
    normalised (v + 80) / 160. The default maps the sample's root to note 69;
    our samples are rendered at C (note 60), so start = -60, end = +67 gives a
    true chromatic map with the root in the right place.
  * param 11 "Obey note-offs" must be on for sustained instruments and off for
    one-shot drums.
"""
import os

from reaper_exec import run_lua

OUT_DIR = os.environ.get("QLABS_SONGS_DIR",
                         os.path.join(os.path.expanduser("~"), "qlabs-songs"))
SAMPLES = os.path.join(OUT_DIR, "samples")
ROOT = 60

# note -> drum sample
KIT = {36: "drum_kick.wav", 38: "drum_snare.wav", 42: "drum_hat_closed.wav",
       46: "drum_hat_open.wav", 39: "drum_clap.wav", 51: "drum_ride.wav"}

# Per song: which sampled instrument plays which part, and the drum pattern.
# A pattern entry is (note, offset_in_beats_within_bar, velocity).
SWING = 0.66
PLAN = {
    "01_jazz_ballad": {
        "inst": {"lead": "epiano", "harmony": "pad", "bass": "upbass", "counter": "pluck"},
        "bars_per_pattern": 1,
        "drums": [(51, 0.0, 74), (51, SWING + 1, 52), (51, 2.0, 66), (51, SWING + 3, 52),
                  (38, 1.0, 46), (38, 3.0, 50), (36, 0.0, 62)],
        "drum_level": 0.30,
    },
    "02_neo_soul": {
        "inst": {"lead": "epiano", "harmony": "pad", "bass": "subbass", "counter": "epiano"},
        "bars_per_pattern": 1,
        "drums": [(36, 0.0, 96), (36, 2.5, 78), (38, 1.0, 88), (38, 3.0, 92),
                  (42, 0.0, 58), (42, 0.5, 40), (42, 1.0, 54), (42, 1.5, 38),
                  (42, 2.0, 56), (42, 2.5, 40), (42, 3.0, 54), (42, 3.5, 44)],
        "drum_level": 0.34,
    },
    "03_drum_and_bass": {
        "inst": {"lead": "brass", "harmony": "pad", "bass": "subbass"},
        "bars_per_pattern": 2,
        # two-bar amen-ish skeleton: kick 1 and 2.75, snare on 2 and 4
        "drums": [(36, 0.0, 110), (38, 1.0, 104), (36, 2.5, 96), (38, 3.0, 106),
                  (42, 0.5, 56), (42, 1.5, 52), (42, 2.0, 60), (42, 3.5, 54),
                  (36, 4.0, 108), (38, 5.0, 102), (36, 6.75, 94), (38, 7.0, 106),
                  (42, 4.5, 56), (42, 5.5, 52), (46, 6.0, 62), (42, 7.5, 54)],
        "drum_level": 0.44,
    },
    "04_cinematic": {
        "inst": {"lead": "pad", "harmony": "pad", "bass": "subbass", "counter": "pluck"},
        "bars_per_pattern": 0,          # no drums
        "drums": [],
        "drum_level": 0.0,
    },
    "05_blues": {
        "inst": {"lead": "pluck", "harmony": "epiano", "bass": "upbass"},
        "bars_per_pattern": 1,
        # shuffle: ride triplet feel, backbeat snare
        "drums": [(51, 0.0, 72), (51, SWING, 48), (51, 1.0, 66), (51, 1 + SWING, 46),
                  (51, 2.0, 70), (51, 2 + SWING, 48), (51, 3.0, 66), (51, 3 + SWING, 46),
                  (36, 0.0, 92), (36, 2.0, 84), (38, 1.0, 88), (38, 3.0, 90)],
        "drum_level": 0.32,
    },
}

TRACK_ROLE = {"Melody": "lead", "QLabsHarmony": "harmony",
              "QLabsBass": "bass", "QLabsCountermelody": "counter"}


def norm_note(n):
    return n / 127.0


def norm_pitch(v):
    return (v + 80.0) / 160.0


def lua_str(s):
    return "[[" + s + "]]"


def upgrade(song_id, bars, beats_per_bar):
    plan = PLAN[song_id]
    rpp = os.path.join(OUT_DIR, song_id + ".RPP")
    # REAPER prompts if the render target exists, which blocks the whole batch
    # behind a modal dialog. Clear it first.
    stale = os.path.join(OUT_DIR, song_id + ".wav")
    if os.path.exists(stale):
        os.remove(stale)
    L = []
    add = L.append
    add("reaper.Main_OnCommand(40859, 0)")
    add('reaper.Main_openProject("noprompt:" .. %s)' % lua_str(rpp))
    add("local proj = 0")

    add("local ROLEINST = {}")
    for role, inst in plan["inst"].items():
        add('ROLEINST["%s"] = %s' % (
            role, lua_str(os.path.join(SAMPLES, "inst_%s_root%d.wav" % (inst, ROOT)))))
    add("local TRACKROLE = {}")
    for tname, role in TRACK_ROLE.items():
        add('TRACKROLE["%s"] = "%s"' % (tname, role))

    add("""
local function load_sampler(tr, path, lo, hi, chromatic, obey)
  local fx = reaper.TrackFX_AddByName(tr, "VSTi: ReaSamplOmatic5000 (Cockos)", false, -1)
  if fx < 0 then say("  could not add RS5k") return -1 end
  reaper.TrackFX_SetNamedConfigParm(tr, fx, "FILE0", path)
  reaper.TrackFX_SetNamedConfigParm(tr, fx, "DONE", "")
  reaper.TrackFX_SetParamNormalized(tr, fx, 3, lo / 127.0)
  reaper.TrackFX_SetParamNormalized(tr, fx, 4, hi / 127.0)
  if chromatic then
    reaper.TrackFX_SetParamNormalized(tr, fx, 5, (-60 + 80) / 160.0)
    reaper.TrackFX_SetParamNormalized(tr, fx, 6, (67 + 80) / 160.0)
  else
    reaper.TrackFX_SetParamNormalized(tr, fx, 5, (0 + 80) / 160.0)
    reaper.TrackFX_SetParamNormalized(tr, fx, 6, (0 + 80) / 160.0)
  end
  reaper.TrackFX_SetParamNormalized(tr, fx, 11, obey and 1.0 or 0.0)
  reaper.TrackFX_SetParamNormalized(tr, fx, 8, 1.0)   -- max voices
  return fx
end

-- Swap the synth on every melodic part for the sampler, keeping the effects.
for t = 0, reaper.CountTracks(proj) - 1 do
  local tr = reaper.GetTrack(proj, t)
  local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
  local role = TRACKROLE[tn]
  local path = role and ROLEINST[role]
  if path then
    -- drop the old instrument, which is always first in the chain
    local _, first = reaper.TrackFX_GetFXName(tr, 0, "")
    if first:find("ReaSynth", 1, true) then reaper.TrackFX_Delete(tr, 0) end
    local fx = load_sampler(tr, path, 0, 127, true, true)
    -- move the sampler to the head of the chain, ahead of the effects
    if fx > 0 then reaper.TrackFX_CopyToTrack(tr, fx, tr, 0, true) end
    local _, nowfirst = reaper.TrackFX_GetFXName(tr, 0, "")
    say(string.format("  %-22s %-8s -> %s", tn, role,
      (nowfirst:gsub("^VSTi: ", ""):gsub(" %(Cockos%)", ""))))
  end
end
""")

    if plan["drums"]:
        add("local KIT = {}")
        for note, fname in KIT.items():
            add("KIT[%d] = %s" % (note, lua_str(os.path.join(SAMPLES, fname))))
        add("local PAT = {")
        for note, off, vel in plan["drums"]:
            add("  {%d, %r, %d}," % (note, float(off), vel))
        add("}")
        add("local PATBEATS = %r" % float(plan["bars_per_pattern"] * beats_per_bar))
        add("local TOTALBEATS = %r" % float(bars * beats_per_bar))
        add("""
-- remove any previous drum track (the ReaSynDr one that never sounded)
for t = reaper.CountTracks(proj) - 1, 0, -1 do
  local tr = reaper.GetTrack(proj, t)
  local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
  if tn == "Drums" then reaper.DeleteTrack(tr) end
end
reaper.InsertTrackAtIndex(reaper.CountTracks(proj), true)
local dtr = reaper.GetTrack(proj, reaper.CountTracks(proj) - 1)
reaper.GetSetMediaTrackInfo_String(dtr, "P_NAME", "Drums", true)
-- one sampler instance per kit piece; each answers only to its own note
local pieces = 0
for note, path in pairs(KIT) do
  if load_sampler(dtr, path, note, note, false, false) >= 0 then pieces = pieces + 1 end
end
say("  Drums                  kit pieces = " .. pieces)
local dt0 = reaper.TimeMap2_QNToTime(proj, 0.0)
local dt1 = reaper.TimeMap2_QNToTime(proj, TOTALBEATS)
local ditem = reaper.CreateNewMIDIItemInProj(dtr, dt0, dt1, false)
local dtake = reaper.GetActiveTake(ditem)
local hits = 0
local base = 0.0
while base < TOTALBEATS do
  for _, h in ipairs(PAT) do
    local at = base + h[2]
    if at < TOTALBEATS then
      local sp = reaper.MIDI_GetPPQPosFromProjQN(dtake, at)
      local ep = reaper.MIDI_GetPPQPosFromProjQN(dtake, at + 0.2)
      reaper.MIDI_InsertNote(dtake, false, false, sp, ep, 0, h[1], h[3], true)
      hits = hits + 1
    end
  end
  base = base + PATBEATS
end
reaper.MIDI_Sort(dtake)
say("  Drums                  " .. hits .. " hits over " .. TOTALBEATS .. " beats")
reaper.TrackFX_AddByName(dtr, "VST: ReaEQ (Cockos)", false, -1)
reaper.TrackFX_AddByName(dtr, "VST: ReaComp (Cockos)", false, -1)
""")
        add('reaper.SetMediaTrackInfo_Value(dtr, "D_VOL", %r)' % float(plan["drum_level"]))

    add("reaper.Main_SaveProjectEx(proj, %s, 0)" % lua_str(rpp))
    add('say("  saved " .. %s)' % lua_str(rpp))
    add('reaper.GetSetProjectInfo_String(proj, "RENDER_FILE", %s, true)' % lua_str(OUT_DIR))
    add('reaper.GetSetProjectInfo_String(proj, "RENDER_PATTERN", %s, true)' % lua_str(song_id))
    add('reaper.GetSetProjectInfo(proj, "RENDER_SETTINGS", 0, true)')
    add('reaper.GetSetProjectInfo(proj, "RENDER_BOUNDSFLAG", 1, true)')
    add('reaper.GetSetProjectInfo(proj, "RENDER_CHANNELS", 2, true)')
    add('reaper.GetSetProjectInfo(proj, "RENDER_SRATE", 48000, true)')
    add('reaper.GetSetProjectInfo(proj, "RENDER_ADDTOPROJ", 0, true)')
    add("reaper.Main_OnCommand(41824, 0)")
    add('say("  render issued")')
    return run_lua("\n".join(L), timeout=240)


if __name__ == "__main__":
    import sys
    from songs import SONGS
    which = sys.argv[1] if len(sys.argv) > 1 else None
    for s in SONGS:
        if which and which not in s["id"]:
            continue
        print("=" * 70)
        print(s["title"])
        for line in upgrade(s["id"], s["bars"], s["meter"][0]):
            print(line)
