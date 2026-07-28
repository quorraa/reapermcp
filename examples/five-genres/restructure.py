"""Restructure the projects so they can actually be mixed.

Three problems, one rebuild:

1. All six drum sounds shared a single track, so one EQ, one compressor and one
   fader served kick, snare, hats and ride together. That is why the ride could
   not be turned down without gutting the kick. Drums now become a folder with
   Kick / Snare / Hats / Ride as separate tracks, each with its own processing
   and its own level.

2. Surge's String oscillator produced digital silence: the oscillator parameter
   slots are reused per type, REAPER caches their names from instantiation, and
   the slots inherited values meant for a Classic oscillator, leaving the
   exciter at zero. The blues lead and the jazz countermelody were inaudible.
   Those patches now use oscillators verified to sound.

3. Levels were guesses. They are set from measured stem loudness afterwards.
"""
import os
import sys

from reaper_exec import run_lua
from upgrade_instruments import KIT, PLAN

OUT_DIR = os.environ.get("QLABS_SONGS_DIR", r"C:\Wizardry\media\audio\qlabs-songs")
SAMPLES = os.path.join(OUT_DIR, "samples")

# which kit notes belong on which track, and the processing each one wants
DRUM_TRACKS = [
    ("Kick",  [36], ["VST: ReaEQ (Cockos)", "VST: ReaComp (Cockos)"]),
    ("Snare", [38, 39], ["VST: ReaEQ (Cockos)", "VST: ReaComp (Cockos)"]),
    ("Hats",  [42, 46], ["VST: ReaEQ (Cockos)"]),
    ("Ride",  [51], ["VST: ReaEQ (Cockos)"]),
]


def lua_str(s):
    return "[[" + s + "]]"


def restructure(song_id, bars, beats_per_bar):
    plan = PLAN[song_id]
    rpp = os.path.join(OUT_DIR, song_id + ".RPP")
    total = float(bars * beats_per_bar)
    patbeats = float(plan["bars_per_pattern"] * beats_per_bar) if plan["drums"] else 0.0

    L = []
    add = L.append
    add('reaper.Main_openProject("noprompt:" .. %s)' % lua_str(rpp))
    add("local proj = 0")
    add("""
local function del_track_named(name)
  for t = reaper.CountTracks(proj) - 1, 0, -1 do
    local tr = reaper.GetTrack(proj, t)
    local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
    if tn == name then reaper.DeleteTrack(tr) end
  end
end

local function load_sampler(tr, path, note)
  local fx = reaper.TrackFX_AddByName(tr, "VSTi: ReaSamplOmatic5000 (Cockos)", false, -1)
  if fx < 0 then return -1 end
  reaper.TrackFX_SetNamedConfigParm(tr, fx, "FILE0", path)
  reaper.TrackFX_SetNamedConfigParm(tr, fx, "DONE", "")
  reaper.TrackFX_SetParamNormalized(tr, fx, 3, note / 127.0)   -- note range start
  reaper.TrackFX_SetParamNormalized(tr, fx, 4, note / 127.0)   -- note range end
  reaper.TrackFX_SetParamNormalized(tr, fx, 5, (0 + 80) / 160.0)   -- no transposition
  reaper.TrackFX_SetParamNormalized(tr, fx, 6, (0 + 80) / 160.0)
  reaper.TrackFX_SetParamNormalized(tr, fx, 11, 0.0)           -- one-shot
  reaper.TrackFX_SetParamNormalized(tr, fx, 8, 1.0)            -- max voices
  return fx
end
""")

    if plan["drums"]:
        add('del_track_named("Drums")')
        for name, _notes, _fx in DRUM_TRACKS:
            add('del_track_named("%s")' % name)
        add("local base = reaper.CountTracks(proj)")
        add("reaper.InsertTrackAtIndex(base, true)")
        add("local folder = reaper.GetTrack(proj, base)")
        add('reaper.GetSetMediaTrackInfo_String(folder, "P_NAME", "Drums", true)')
        add('reaper.SetMediaTrackInfo_Value(folder, "I_FOLDERDEPTH", 1)')
        add("local KIT = {}")
        for note, fname in KIT.items():
            add("KIT[%d] = %s" % (note, lua_str(os.path.join(SAMPLES, fname))))
        add("local PAT = {")
        for note, off, vel in plan["drums"]:
            add("  {%d, %r, %d}," % (note, float(off), vel))
        add("}")
        add("local PATBEATS, TOTAL = %r, %r" % (patbeats, total))

        for i, (name, notes, fxs) in enumerate(DRUM_TRACKS):
            last = (i == len(DRUM_TRACKS) - 1)
            add('do')
            add('  local idx = reaper.CountTracks(proj)')
            add('  reaper.InsertTrackAtIndex(idx, true)')
            add('  local tr = reaper.GetTrack(proj, idx)')
            add('  reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "%s", true)' % name)
            add('  reaper.SetMediaTrackInfo_Value(tr, "I_FOLDERDEPTH", %d)' % (-1 if last else 0))
            add('  local NOTES = {%s}' % ", ".join("[%d]=true" % n for n in notes))
            add('  for note, path in pairs(KIT) do')
            add('    if NOTES[note] then load_sampler(tr, path, note) end')
            add('  end')
            for fxname in fxs:
                add('  reaper.TrackFX_AddByName(tr, %s, false, -1)' % lua_str(fxname))
            add('  local t0 = reaper.TimeMap2_QNToTime(proj, 0.0)')
            add('  local t1 = reaper.TimeMap2_QNToTime(proj, TOTAL)')
            add('  local item = reaper.CreateNewMIDIItemInProj(tr, t0, t1, false)')
            add('  local take = reaper.GetActiveTake(item)')
            add('  local hits = 0')
            add('  local b = 0.0')
            add('  while b < TOTAL do')
            add('    for _, h in ipairs(PAT) do')
            add('      if NOTES[h[1]] and (b + h[2]) < TOTAL then')
            add('        local sp = reaper.MIDI_GetPPQPosFromProjQN(take, b + h[2])')
            add('        local ep = reaper.MIDI_GetPPQPosFromProjQN(take, b + h[2] + 0.2)')
            add('        reaper.MIDI_InsertNote(take, false, false, sp, ep, 0, h[1], h[3], true)')
            add('        hits = hits + 1')
            add('      end')
            add('    end')
            add('    b = b + PATBEATS')
            add('  end')
            add('  reaper.MIDI_Sort(take)')
            add('  say("  ' + name + ': " .. hits .. " hits, "'
                ' .. reaper.TrackFX_GetCount(tr) .. " fx total")')
            add('end')

    add('say("  tracks now " .. reaper.CountTracks(proj))')
    add("reaper.Main_SaveProjectEx(proj, %s, 0)" % lua_str(rpp))
    add('say("  saved")')
    return run_lua("\n".join(L), timeout=420)


if __name__ == "__main__":
    from songs import SONGS
    which = sys.argv[1] if len(sys.argv) > 1 else None
    for s in SONGS:
        if which and which not in s["id"]:
            continue
        if not PLAN[s["id"]]["drums"]:
            print("%s: no drums, skipped" % s["id"])
            continue
        print("=" * 60)
        print(s["id"])
        for line in restructure(s["id"], s["bars"], s["meter"][0]):
            print(line)
