"""Build the instruments out of Surge parameters.

Surge's factory patches cannot be installed from a script: SetPreset with an
.fxp is refused, SetPreset with a .vstpreset and a written vst_chunk both report
success and change nothing, editing the state in the .RPP has no effect, and
MIDI program change is ignored. Writing parameters is the one route that reaches
the audio, so the instruments are built rather than loaded.

Each voice is specified in real units - hertz, milliseconds, decibels, cents -
and reached by searching the plugin's own readout, so the numbers here mean what
they say instead of being opaque 0..1 values.

What makes these read as instruments rather than tones, which is what the
previous single-oscillator Init Saw derivatives were: more than one oscillator,
unison detune for width, a filter envelope so the timbre moves as a note decays,
and an amplitude envelope shaped to how the instrument is actually played.
"""
import os
import sys

from reaper_exec import run_lua

OUT_DIR = os.environ.get("QLABS_SONGS_DIR", r"C:\Wizardry\media\audio\qlabs-songs")
HERE = os.path.dirname(os.path.abspath(__file__))

# value kinds: n = numeric, in the unit the readout uses; e = enum label substring
N, E = "n", "e"

# Every voice sends to the same reverb. Without this the send levels set below
# feed a slot that is switched off, which is its own way of sounding thin: a
# completely dry instrument reads as small no matter how it is built.
#
# The send reverb is the only effect used. Two other things were crashing Surge
# hard enough to take REAPER with it - the Windows event log named
# "Surge XT.vst3" faulting with 0xc0000005 both times:
#
#   * the extracted SurgeXTData factory library, now renamed aside. Nothing
#     needs it: the factory patches cannot be loaded from a script, which is why
#     these instruments are built from parameters in the first place.
#   * the per-scene insert effects (Ensemble on the pianos, Rotary on the
#     organ). Renders survive repeatedly without them and crash with them.
#
# Bisected one at a time against whether a full stem render completes.
SHARED = [
    ("FX S1 FX Type", E, "Reverb 2"),
    # Scene volume is pinned to Surge's default so a build fully determines the
    # instrument. It is a second gain stage in series with Global Volume, and
    # leaving it wherever a previous experiment left it is how the melody ended
    # up at -60 dB here and -22 dB there, rendering silent for reasons nothing
    # in the spec explained.
    ("A Volume", N, -3.0),
]


def _leveled(db):
    """Give a voice builder a fixed output level, appended to its spec.

    Output level belongs to the instrument, not to a single shared setting.
    These voices come from different synthesis and are nowhere near equal in
    level: the FM pianos clipped at full scale while the bass measured 17 dB
    below the lead and pinned its fader at REAPER's +12 dB ceiling. Balancing
    here keeps every fader in a usable range and the peaks honest.
    """
    def wrap(fn):
        def inner(*a, **kw):
            # Global Volume, with scene volume pinned in SHARED above. Both are
            # real gain stages in series; setting one and ignoring the other
            # leaves the instrument's level depending on history rather than on
            # this specification.
            return fn(*a, **kw) + [("Global Volume", N, db)]
        inner.__name__ = fn.__name__
        inner.__doc__ = fn.__doc__
        return inner
    return wrap


# ---------------------------------------------------------------- instruments

@_leveled(-22.0)
def electric_piano(bright=False, decay=2200.0):
    """Tine electric piano: FM gives the bell strike, a sine underneath the body."""
    return [
        ("A Osc 1 Type", E, "FM3" if bright else "FM2"),
        ("A Osc 1 Shape", N, 35.0 if bright else 22.0),
        ("A Osc 1 Unison Voices", N, 2),
        ("A Osc 1 Unison Detune", N, 6.0),
        ("A Osc 2 Type", E, "Sine"),
        ("A Osc 2 Mute", E, "Off"),
        ("A Osc 2 Volume", N, -13.0),
        ("A Filter 1 Type", E, "LP 12 dB"),
        ("A Filter 1 Cutoff", N, 4200.0 if bright else 3200.0),
        ("A Filter 1 Resonance", N, 6.0),
        ("A Filter 1 Keytrack", N, 55.0),
        ("A Filter 1 FEG Mod Amount", N, 22.0),
        ("A Filter EG Attack", N, 0.0),
        ("A Filter EG Decay", N, 700.0),
        ("A Filter EG Sustain", N, 8.0),
        ("A Amp EG Attack", N, 3.0),
        ("A Amp EG Decay", N, decay),
        ("A Amp EG Sustain", N, 18.0),
        ("A Amp EG Release", N, 450.0),
        ("A Send FX 1 Level", N, -17.0),
    ]


@_leveled(-12.0)
def pad(cutoff=1600.0, attack=350.0, voices=4, detune=18.0, sub=True):
    """Wide sustained pad: many detuned voices, slow to arrive and to leave."""
    spec = [
        ("A Osc 1 Type", E, "Classic"),
        ("A Osc 1 Unison Voices", N, voices),
        ("A Osc 1 Unison Detune", N, detune),
        ("A Filter 1 Type", E, "LP 12 dB"),
        ("A Filter 1 Cutoff", N, cutoff),
        ("A Filter 1 Resonance", N, 8.0),
        ("A Filter 1 FEG Mod Amount", N, 10.0),
        ("A Filter EG Attack", N, attack),
        ("A Filter EG Decay", N, 1200.0),
        ("A Filter EG Sustain", N, 70.0),
        ("A Amp EG Attack", N, attack),
        ("A Amp EG Decay", N, 1500.0),
        ("A Amp EG Sustain", N, 85.0),
        ("A Amp EG Release", N, 1100.0),
        ("A Send FX 1 Level", N, -12.0),
    ]
    if sub:
        spec += [
            ("A Osc 2 Type", E, "Classic"),
            ("A Osc 2 Mute", E, "Off"),
            ("A Osc 2 Octave", N, -1),
            ("A Osc 2 Unison Voices", N, 1),
            ("A Osc 2 Volume", N, -9.0),
        ]
    return spec


@_leveled(-2.0)
def bass(cutoff=380.0, feg=30.0, sustain=32.0, resonance=12.0, octave=-1,
         detune=0.0, osc="Classic"):
    """Plucked bass: the filter envelope is what gives it attack and note shape."""
    spec = [
        ("A Osc 1 Type", E, osc),
        ("A Osc 1 Octave", N, octave),
        ("A Filter 1 Type", E, "LP 24 dB"),
        ("A Filter 1 Cutoff", N, cutoff),
        ("A Filter 1 Resonance", N, resonance),
        ("A Filter 1 Keytrack", N, 35.0),
        ("A Filter 1 FEG Mod Amount", N, feg),
        ("A Filter EG Attack", N, 0.0),
        ("A Filter EG Decay", N, 220.0),
        ("A Filter EG Sustain", N, 5.0),
        ("A Amp EG Attack", N, 6.0),
        ("A Amp EG Decay", N, 800.0),
        ("A Amp EG Sustain", N, sustain),
        ("A Amp EG Release", N, 220.0),
    ]
    if detune:
        spec += [
            ("A Osc 1 Unison Voices", N, 2),
            ("A Osc 1 Unison Detune", N, detune),
            ("A Osc 2 Type", E, "Classic"),
            ("A Osc 2 Mute", E, "Off"),
            ("A Osc 2 Octave", N, octave),
            ("A Osc 2 Pitch", N, 0.2),
            ("A Osc 2 Volume", N, -3.0),
        ]
    return spec


@_leveled(-10.0)
def reed(cutoff=3000.0, attack=45.0, width=78.0):
    """Woodwind: a narrow pulse is the reediness; it sustains while blown."""
    return [
        ("A Osc 1 Type", E, "Classic"),
        ("A Osc 1 Width 1", N, width),
        ("A Osc 1 Unison Voices", N, 2),
        ("A Osc 1 Unison Detune", N, 4.0),
        ("A Filter 1 Type", E, "LP 12 dB"),
        ("A Filter 1 Cutoff", N, cutoff),
        ("A Filter 1 Keytrack", N, 60.0),
        ("A Filter 1 FEG Mod Amount", N, 8.0),
        ("A Filter EG Attack", N, attack),
        ("A Filter EG Sustain", N, 80.0),
        ("A Amp EG Attack", N, attack),
        ("A Amp EG Decay", N, 900.0),
        ("A Amp EG Sustain", N, 100.0),
        ("A Amp EG Release", N, 200.0),
        ("A Send FX 1 Level", N, -15.0),
    ]


@_leveled(-8.0)
def flute():
    """Flute: sine tone plus breath noise, which is most of what identifies it."""
    return [
        ("A Osc 1 Type", E, "Sine"),
        ("A Osc 1 Unison Voices", N, 2),
        ("A Osc 1 Unison Detune", N, 3.0),
        ("A Noise Mute", E, "Off"),
        ("A Noise Volume", N, -26.0),
        ("A Noise Color", N, 45.0),
        ("A Filter 1 Type", E, "LP 12 dB"),
        ("A Filter 1 Cutoff", N, 4500.0),
        ("A Filter 1 Keytrack", N, 70.0),
        ("A Amp EG Attack", N, 85.0),
        ("A Amp EG Sustain", N, 100.0),
        ("A Amp EG Release", N, 260.0),
        ("A Send FX 1 Level", N, -11.0),
    ]


@_leveled(-14.0)
def organ(octave=0, drive=0.0, release=70.0):
    """Drawbar organ: stacked sines at octave and twelfth, near-instant on and off."""
    spec = [
        ("A Osc 1 Type", E, "Sine"),
        ("A Osc 1 Octave", N, octave),
        ("A Osc 2 Type", E, "Sine"),
        ("A Osc 2 Mute", E, "Off"),
        ("A Osc 2 Octave", N, octave + 1),
        ("A Osc 2 Volume", N, -7.0),
        ("A Osc 3 Type", E, "Sine"),
        ("A Osc 3 Mute", E, "Off"),
        ("A Osc 3 Octave", N, octave + 1),
        ("A Osc 3 Pitch", N, 7.0),
        ("A Osc 3 Volume", N, -12.0),
        ("A Amp EG Attack", N, 4.0),
        ("A Amp EG Sustain", N, 100.0),
        ("A Amp EG Release", N, release),
        ("A Send FX 1 Level", N, -16.0),
    ]
    if drive:
        spec += [("A Waveshaper Type", E, "Soft"),
                 ("A Waveshaper Drive", N, drive)]
    return spec


@_leveled(-10.0)
def pluck(osc="Classic", cutoff=3400.0, decay=340.0, feg=26.0):
    return [
        ("A Osc 1 Type", E, osc),
        ("A Osc 1 Unison Voices", N, 2),
        ("A Osc 1 Unison Detune", N, 7.0),
        ("A Filter 1 Type", E, "LP 24 dB"),
        ("A Filter 1 Cutoff", N, cutoff),
        ("A Filter 1 Resonance", N, 14.0),
        ("A Filter 1 FEG Mod Amount", N, feg),
        ("A Filter EG Attack", N, 0.0),
        ("A Filter EG Decay", N, 180.0),
        ("A Filter EG Sustain", N, 0.0),
        ("A Amp EG Attack", N, 0.0),
        ("A Amp EG Decay", N, decay),
        ("A Amp EG Sustain", N, 0.0),
        ("A Amp EG Release", N, 220.0),
        ("A Send FX 1 Level", N, -14.0),
    ]


@_leveled(-12.0)
def lead(cutoff=6000.0, sync=0.0, voices=3):
    spec = [
        ("A Osc 1 Type", E, "Classic"),
        ("A Osc 1 Unison Voices", N, voices),
        ("A Osc 1 Unison Detune", N, 9.0),
        ("A Filter 1 Type", E, "LP 24 dB"),
        ("A Filter 1 Cutoff", N, cutoff),
        ("A Filter 1 Resonance", N, 18.0),
        ("A Filter 1 FEG Mod Amount", N, 18.0),
        ("A Filter EG Attack", N, 2.0),
        ("A Filter EG Decay", N, 420.0),
        ("A Filter EG Sustain", N, 45.0),
        ("A Amp EG Attack", N, 2.0),
        ("A Amp EG Decay", N, 500.0),
        ("A Amp EG Sustain", N, 72.0),
        ("A Amp EG Release", N, 180.0),
        ("A Send FX 1 Level", N, -16.0),
    ]
    if sync:
        spec += [("A Osc 1 Sync", N, sync)]
    return spec


@_leveled(-14.0)
def brass():
    """Synth brass: the slow filter sweep into the note is the whole character."""
    return [
        ("A Osc 1 Type", E, "Classic"),
        ("A Osc 1 Unison Voices", N, 3),
        ("A Osc 1 Unison Detune", N, 12.0),
        ("A Osc 2 Type", E, "Classic"),
        ("A Osc 2 Mute", E, "Off"),
        ("A Osc 2 Pitch", N, 0.1),
        ("A Osc 2 Volume", N, -4.0),
        ("A Filter 1 Type", E, "LP 24 dB"),
        ("A Filter 1 Cutoff", N, 2200.0),
        ("A Filter 1 Resonance", N, 12.0),
        ("A Filter 1 FEG Mod Amount", N, 26.0),
        ("A Filter EG Attack", N, 70.0),
        ("A Filter EG Decay", N, 600.0),
        ("A Filter EG Sustain", N, 55.0),
        ("A Amp EG Attack", N, 55.0),
        ("A Amp EG Decay", N, 900.0),
        ("A Amp EG Sustain", N, 88.0),
        ("A Amp EG Release", N, 320.0),
        ("A Send FX 1 Level", N, -10.0),
    ]


# ------------------------------------------------------------------- per song

BUILD = {
    "01_jazz_ballad": {
        "Melody":             ("electric piano", electric_piano()),
        "QLabsHarmony":       ("electric piano pad", electric_piano(decay=1400.0)),
        "QLabsBass":          ("upright bass", bass(cutoff=330.0, feg=32.0, sustain=28.0)),
        "QLabsCountermelody": ("clarinet", reed(cutoff=2600.0, attack=55.0, width=80.0)),
    },
    "02_neo_soul": {
        "Melody":             ("FM electric piano", electric_piano(bright=True, decay=1700.0)),
        "QLabsHarmony":       ("warm pad", pad(cutoff=1400.0, attack=420.0)),
        "QLabsBass":          ("rubber bass", bass(cutoff=240.0, feg=36.0, sustain=45.0,
                                                   resonance=26.0)),
        "QLabsCountermelody": ("pluck", pluck(decay=380.0)),
    },
    "03_drum_and_bass": {
        "Melody":             ("sync lead", lead(cutoff=6500.0, sync=7.0)),
        "QLabsHarmony":       ("supersaw", pad(cutoff=3800.0, attack=180.0, voices=5,
                                               detune=26.0)),
        "QLabsBass":          ("reese bass", bass(cutoff=520.0, feg=14.0, sustain=100.0,
                                                  resonance=16.0, detune=28.0)),
        "QLabsCountermelody": ("FM pluck", pluck(osc="FM2", cutoff=4200.0, decay=260.0)),
    },
    "04_cinematic": {
        "Melody":             ("synth brass", brass()),
        "QLabsHarmony":       ("verb pad", pad(cutoff=1150.0, attack=620.0, detune=14.0)),
        "QLabsBass":          ("sub bass", bass(cutoff=190.0, feg=8.0, sustain=100.0,
                                                resonance=4.0, osc="Sine")),
        "QLabsCountermelody": ("flute", flute()),
    },
    "05_blues": {
        "Melody":             ("drawbar organ", organ(drive=9.0)),
        "QLabsHarmony":       ("organ pad", organ(octave=-1, release=90.0)),
        "QLabsBass":          ("electric bass", bass(cutoff=420.0, feg=28.0, sustain=35.0)),
        "QLabsCountermelody": ("electric guitar", pluck(cutoff=2800.0, decay=650.0, feg=20.0)),
    },
}


def param_index():
    idx = {}
    with open(os.path.join(HERE, "surge_params.tsv"), encoding="utf-8") as fh:
        for line in fh:
            parts = line.rstrip("\n").split("\t")
            if len(parts) >= 2:
                idx[parts[1]] = int(parts[0])
    return idx


LUA = """
local ok, S = pcall(dofile, HELPER)
if not ok then say("helper failed: " .. tostring(S)) return end

reaper.Main_openProject("noprompt:" .. RPP)
local proj = 0

for t = 0, reaper.CountTracks(proj) - 1 do
  local tr = reaper.GetTrack(proj, t)
  local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
  local spec = SPEC[tn]
  if spec then
    local fx = -1
    for x = 0, reaper.TrackFX_GetCount(tr) - 1 do
      local _, nm = reaper.TrackFX_GetFXName(tr, x, "")
      if tostring(nm):lower():find("surge", 1, true) then fx = x break end
    end
    if fx < 0 then
      say(string.format("  %-20s no Surge on this track", tn))
    else
      local bad = 0
      for _, e in ipairs(spec) do
        local p, kind, v = e[1], e[2], e[3]
        local done
        if kind == "e" then done = S.setenum(tr, fx, p, v)
        else done = S.setnum(tr, fx, p, v) end
        if not done then bad = bad + 1 say("      could not set param " .. p) end
      end
      say(string.format("  %-20s %-20s %d settings%s", tn, LABEL[tn], #spec,
        bad > 0 and string.format(" (%d failed)", bad) or ""))
    end
  end
end

reaper.Main_SaveProjectEx(proj, RPP, 0)
say("  saved")
"""


def build(song_id):
    idx = param_index()
    spec = BUILD[song_id]

    lines = ["local SPEC = {}", "local LABEL = {}"]
    for track, (label, entries) in spec.items():
        entries = SHARED + entries
        lines.append('LABEL["%s"] = "%s"' % (track, label))
        lines.append('SPEC["%s"] = {' % track)
        for name, kind, value in entries:
            if name not in idx:
                raise KeyError("%s: unknown Surge parameter %r" % (song_id, name))
            if kind == E:
                lines.append('  {%d, "e", "%s"},' % (idx[name], value))
            else:
                lines.append('  {%d, "n", %r},' % (idx[name], float(value)))
        lines.append("}")

    body = ("local HELPER = [[%s]]\nlocal RPP = [[%s]]\n%s\n%s"
            % (os.path.join(HERE, "surge_set.lua"),
               os.path.join(OUT_DIR, song_id + ".RPP"),
               "\n".join(lines), LUA))
    print("=" * 66)
    print(song_id)
    for line in run_lua(body, timeout=600):
        print(line)


if __name__ == "__main__":
    for s in (sys.argv[1:] or list(BUILD)):
        build(s)
