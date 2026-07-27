"""Build one genre project end to end.

  1. write the melody into a fresh REAPER project tab
  2. let the MCP server analyse it and generate harmony / bass / countermelody
  3. stage the winning candidate (unmuted, so the project is audible)
  4. put an instrument and a genre-specific effect chain on every part
  5. save the .RPP and render a .wav

Steps 1, 4, 5 are ReaScript; step 2 and 3 go through the real MCP server over
stdio. The product does not choose instruments or effects by design, so those
are authored here.
"""
import os
import sys

from mcp_driver import Server
from reaper_exec import run_lua
from songs import SONGS, midi, validate

OUT_DIR = os.environ.get("QLABS_SONGS_DIR",
                         os.path.join(os.path.expanduser("~"), "qlabs-songs"))
DRUM_KICK, DRUM_SNARE, DRUM_HAT = 36, 38, 42


def lua_str(s):
    return "[[" + s + "]]"


def build_midi(song):
    """Create the project tab, the melody track and its MIDI item."""
    beats = song["bars"] * song["meter"][0]
    num, den = song["meter"]
    lines = [
        "reaper.Main_OnCommand(40859, 0)",
        "local proj = 0",
        "reaper.SetCurrentBPM(proj, %r, false)" % song["tempo"],
        "reaper.SetTempoTimeSigMarker(proj, -1, 0.0, -1, -1, %r, %d, %d, false)" % (
            song["tempo"], num, den),
        "reaper.InsertTrackAtIndex(0, true)",
        "local tr = reaper.GetTrack(proj, 0)",
        'reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "Melody", true)',
        "local t0 = reaper.TimeMap2_QNToTime(proj, 0.0)",
        "local t1 = reaper.TimeMap2_QNToTime(proj, %r)" % float(beats),
        "local item = reaper.CreateNewMIDIItemInProj(tr, t0, t1, false)",
        "local take = reaper.GetActiveTake(item)",
        "local N = {",
    ]
    for p, o, d, v in song["notes"]:
        lines.append("  {%d, %r, %r, %d}," % (midi(p), float(o), float(o + d), v))
    lines += [
        "}",
        "for _, n in ipairs(N) do",
        "  local sp = reaper.MIDI_GetPPQPosFromProjQN(take, n[2])",
        "  local ep = reaper.MIDI_GetPPQPosFromProjQN(take, n[3])",
        "  reaper.MIDI_InsertNote(take, true, false, sp, ep, 0, n[1], n[4], true)",
        "end",
        "reaper.MIDI_Sort(take)",
        'reaper.GetSetMediaItemTakeInfo_String(take, "P_NAME", %s, true)' % lua_str(song["id"]),
        "reaper.SelectAllMediaItems(proj, false)",
        "reaper.SetMediaItemSelected(item, true)",
        "reaper.GetSet_LoopTimeRange2(proj, true, true, t0, t1, false)",
        "reaper.UpdateArrange()",
        'say("built " .. #N .. " notes over %d bars")' % song["bars"],
    ]
    return run_lua("\n".join(lines))


def generate_and_stage(song):
    """Let the MCP server do the music. Returns a summary dict."""
    s = Server()
    try:
        snap = s.call("reaper.inspect_selection", {})
        if "snapshot_id" not in snap:
            raise SystemExit("inspect failed: %s" % snap)
        beats = float(song["bars"] * song["meter"][0])
        an = s.call("music.analyze_selection", {
            "snapshot_id": snap["snapshot_id"], "style_profile": song["profile"],
            "loop_span": {"start_qn": 0.0, "end_qn": beats}})
        args = {
            "snapshot_id": snap["snapshot_id"], "analysis_id": an["analysis_id"],
            "style_profile": song["profile"], "candidate_count": 3,
            "preserve_melody": True, "preserve_rhythm": True,
            "loop_intent": song["loop_intent"], "seed": song["seed"],
        }
        if song.get("countermelody"):
            args["countermelody"] = song["countermelody"]
        gen = s.call("harmony.generate_candidates", args)
        cands = gen["candidates"]
        best = cands[0]
        stg = s.call("reaper.stage_candidate", {
            "candidate_id": best["candidate_id"],
            "folder_name": song["title"], "muted": False})
        if "transaction_id" not in stg:
            raise SystemExit("stage failed: %s" % stg)
        audit = s.call("loop.audit", {
            "snapshot_id": snap["snapshot_id"], "candidate_id": best["candidate_id"],
            "loop_intent": song["loop_intent"]})
        return {
            "key": an["key"]["candidates"][0]["label"],
            "key_conf": an["key"]["candidates"][0]["confidence"],
            "phrases": len(an["phrases"]),
            "slots": an["grid"]["slot_count"],
            "strategy": best["strategy"],
            "score": best["score_total"],
            "chords": best["chords"],
            "parts": best.get("parts", []),
            "notes_staged": stg["note_count"],
            "tracks": [t["name"] for t in stg["tracks"]],
            "alts": [(c["strategy"], round(c["score_total"], 3)) for c in cands[1:]],
            "loop_ok": audit.get("compatible"),
            "loop_score": audit.get("score"),
            "loop_findings": [f.get("message") for f in audit.get("findings", [])],
        }
    finally:
        s.close()


ROLE_OF = {
    "QLabsMelody": "lead", "QLabsHarmony": "harmony",
    "QLabsBass": "bass", "QLabsCountermelody": "counter",
}


def dress_and_save(song):
    """Instruments, effect chains, levels, drums, save and render."""
    beats = song["bars"] * song["meter"][0]
    L = []
    add = L.append
    add("local proj = findproj(%s)" % lua_str(song["title"]))
    add("if not proj then say('no project') return end")
    add("reaper.SelectProjectInstance(proj)")

    # Per-role instrument + timbre + fx + level.
    add("local ROLE = {}")
    for tname, role in ROLE_OF.items():
        if role not in song["timbre"] and role != "lead":
            continue
        add('ROLE["%s"] = "%s"' % (tname, role))
    add("local LEVEL = {}")
    for role, lvl in song["levels"].items():
        add('LEVEL["%s"] = %r' % (role, float(lvl)))

    add("local TIMBRE = {}")
    for role, params in song["timbre"].items():
        add('TIMBRE["%s"] = {%s}' % (
            role, ", ".join("[%d]=%r" % (k, float(v)) for k, v in sorted(params.items()))))

    add("local FX = {}")
    for role, chain in song["fx"].items():
        add('FX["%s"] = {%s}' % (role, ", ".join(lua_str(x) for x in chain)))

    add("""
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
""")

    if song.get("drums"):
        add("-- a plain two-step pattern; the product does not write drums")
        add("reaper.InsertTrackAtIndex(reaper.CountTracks(proj), true)")
        add("local dtr = reaper.GetTrack(proj, reaper.CountTracks(proj) - 1)")
        add('reaper.GetSetMediaTrackInfo_String(dtr, "P_NAME", "Drums", true)')
        add('local di = reaper.TrackFX_AddByName(dtr, "VSTi: ReaSynDr (Cockos)", false, -1)')
        add('say("  drums instrument index " .. tostring(di))')
        add("local dt0 = reaper.TimeMap2_QNToTime(proj, 0.0)")
        add("local dt1 = reaper.TimeMap2_QNToTime(proj, %r)" % float(beats))
        add("local ditem = reaper.CreateNewMIDIItemInProj(dtr, dt0, dt1, false)")
        add("local dtake = reaper.GetActiveTake(ditem)")
        add("local BEATS = %d" % beats)
        add("for b = 0, BEATS - 1 do")
        add("  local function hit(pitch, at, vel)")
        add("    local sp = reaper.MIDI_GetPPQPosFromProjQN(dtake, at)")
        add("    local ep = reaper.MIDI_GetPPQPosFromProjQN(dtake, at + 0.25)")
        add("    reaper.MIDI_InsertNote(dtake, false, false, sp, ep, 9, pitch, vel, true)")
        add("  end")
        add("  local inbar = b % 4")
        add("  if inbar == 0 then hit(%d, b, 110) end" % DRUM_KICK)
        add("  if inbar == 2 then hit(%d, b + 0.5, 100) end" % DRUM_KICK)
        add("  if inbar == 1 or inbar == 3 then hit(%d, b, 104) end" % DRUM_SNARE)
        add("  hit(%d, b, 70)" % DRUM_HAT)
        add("  hit(%d, b + 0.5, 56)" % DRUM_HAT)
        add("end")
        add("reaper.MIDI_Sort(dtake)")
        for fxname in song["fx"].get("drums", []):
            add('reaper.TrackFX_AddByName(dtr, %s, false, -1)' % lua_str(fxname))
        add("reaper.SetMediaTrackInfo_Value(dtr, \"D_VOL\", %r)" % float(song["levels"].get("drums", 0.4)))
        add('say("  Drums                role=drums   fx=" .. reaper.TrackFX_GetCount(dtr))')

    # Master chain and headroom.
    add("local master = reaper.GetMasterTrack(proj)")
    for fxname in song["fx"].get("master", []):
        add('reaper.TrackFX_AddByName(master, %s, false, -1)' % lua_str(fxname))
    add('reaper.SetMediaTrackInfo_Value(master, "D_VOL", 0.85)')

    rpp = os.path.join(OUT_DIR, song["id"] + ".RPP")
    add("reaper.Main_SaveProjectEx(proj, %s, 0)" % lua_str(rpp))
    add('say("saved " .. %s)' % lua_str(rpp))

    add('reaper.GetSetProjectInfo_String(proj, "RENDER_FILE", %s, true)' % lua_str(OUT_DIR))
    add('reaper.GetSetProjectInfo_String(proj, "RENDER_PATTERN", %s, true)' % lua_str(song["id"]))
    add('reaper.GetSetProjectInfo(proj, "RENDER_SETTINGS", 0, true)')
    add('reaper.GetSetProjectInfo(proj, "RENDER_BOUNDSFLAG", 1, true)')
    add('reaper.GetSetProjectInfo(proj, "RENDER_CHANNELS", 2, true)')
    add('reaper.GetSetProjectInfo(proj, "RENDER_SRATE", 48000, true)')
    add('reaper.GetSetProjectInfo(proj, "RENDER_ADDTOPROJ", 0, true)')
    add("reaper.Main_OnCommand(41824, 0)")
    add('say("render issued")')
    return run_lua("\n".join(L), timeout=180)


def build(song):
    print("=" * 78)
    print("%s  --  %s" % (song["title"], song["genre"]))
    print("=" * 78)
    os.makedirs(OUT_DIR, exist_ok=True)
    for line in build_midi(song):
        print("  " + line)
    info = generate_and_stage(song)
    print("  key       : %s (%.3f), %d phrases, %d chord slots" % (
        info["key"], info["key_conf"], info["phrases"], info["slots"]))
    print("  chosen    : %s (%.3f)  alts: %s" % (
        info["strategy"], info["score"], info["alts"]))
    print("  chords    : %s" % " | ".join(info["chords"]))
    print("  parts     : %s" % ", ".join(info["parts"]))
    print("  staged    : %d notes across %s" % (info["notes_staged"], info["tracks"]))
    print("  loop      : compatible=%s score=%s" % (info["loop_ok"], info["loop_score"]))
    for f in info["loop_findings"]:
        print("              - %s" % f)
    for line in dress_and_save(song):
        print("  " + line)
    return info


if __name__ == "__main__":
    validate()
    which = sys.argv[1] if len(sys.argv) > 1 else None
    for song in SONGS:
        if which and which not in song["id"]:
            continue
        build(song)
