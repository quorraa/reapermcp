"""Genre patches for Surge XT, built by parameter.

Surge exposes no presets to the host, rejects TrackFX_SetPreset, and its VST3
state is not the .fxp payload, so the 637 factory patches cannot be loaded by
script. Its 2858 parameters *are* fully addressable, so the patches here are
built directly. Enum encodings below were measured, not guessed.

Scene A parameter indices, from dumping the plugin:
   237 Volume            248 Feedback           252/253 Waveshaper type/drive
   256 Osc1 Type         257 Osc1 Octave        258 Osc1 Pitch
   259 Osc1 Shape        263 Osc1 Sync          264 Osc1 Unison Detune
   265 Osc1 Unison Voices
   268 Osc2 Type         269 Osc2 Octave        271 Osc2 Shape
   280 Osc3 Type         281 Osc3 Octave
   292 Osc1 Volume       296 Osc2 Volume        300 Osc3 Volume
   312 Noise Volume      317 Filter1 Type       319 Cutoff   320 Resonance
   329 Amp EG Attack     331 Decay              333 Sustain  334 Release
"""

# measured enum -> normalised value
OSC = {"classic": 0.000, "sine": 0.050, "wavetable": 0.150, "fm3": 0.425,
       "fm2": 0.500, "window": 0.600, "modern": 0.700, "string": 0.775,
       "twist": 0.875, "alias": 0.975}
FILT = {"off": 0.000, "lp12": 0.025, "lp24": 0.050, "ladder": 0.100,
        "hp12": 0.125, "bp12": 0.175, "vintage": 0.300, "obxd12": 0.325,
        "k35": 0.400, "diode": 0.450}
SHAPER = {"off": 0.000, "soft": 0.025, "hard": 0.050, "sine": 0.100,
          "fuzz": 0.575, "fuzz_soft": 0.600, "ojd": 0.975}
UNISON = {1: 0.000, 8: 0.475, 9: 0.500, 12: 0.700, 16: 0.975}

# Octave/pitch centre on 0.5 == no shift.
OCT = {-2: 0.167, -1: 0.333, 0: 0.500, 1: 0.667, 2: 0.833}

PATCHES = {
    # tine electric piano: FM operator pair, quick decay, moderate sustain
    "rhodes": {256: OSC["fm2"], 292: 1.00, 296: 0.00,
               317: FILT["lp24"], 319: 0.56, 320: 0.10,
               329: 0.00, 331: 0.46, 333: 0.32, 334: 0.34,
               252: SHAPER["soft"], 253: 0.55, 237: 0.86},
    # wide detuned saw pad, slow in and out
    "warmpad": {256: OSC["classic"], 265: UNISON[8], 264: 0.28, 292: 0.92,
                268: OSC["classic"], 269: OCT[0], 296: 0.55,
                317: FILT["lp24"], 319: 0.36, 320: 0.14,
                329: 0.38, 331: 0.52, 333: 0.82, 334: 0.58, 237: 0.84},
    # brighter pad that can carry a melody rather than sit under one
    "brightpad": {256: OSC["classic"], 265: UNISON[8], 264: 0.22, 292: 0.95,
                  268: OSC["sine"], 269: OCT[1], 296: 0.60,
                  317: FILT["lp24"], 319: 0.66, 320: 0.12,
                  329: 0.16, 331: 0.45, 333: 0.85, 334: 0.50, 237: 0.92},
    # sine sub with a little drive
    "subbass": {256: OSC["sine"], 257: OCT[-1], 292: 1.00, 296: 0.00,
                317: FILT["lp24"], 319: 0.26, 320: 0.05,
                252: SHAPER["soft"], 253: 0.62,
                329: 0.00, 331: 0.42, 333: 0.90, 334: 0.24, 237: 0.88},
    # hard synced lead for the drum and bass
    "synclead": {256: OSC["classic"], 263: 0.34, 265: UNISON[8], 264: 0.22,
                 292: 1.00, 317: FILT["obxd12"], 319: 0.58, 320: 0.34,
                 252: SHAPER["fuzz_soft"], 253: 0.58,
                 329: 0.02, 331: 0.36, 333: 0.70, 334: 0.26, 237: 0.84},
    # drawbar-ish organ: stacked sines, no filter, instant on and off
    "organ": {256: OSC["sine"], 292: 1.00,
              268: OSC["sine"], 269: OCT[1], 296: 0.72,
              280: OSC["sine"], 281: OCT[2], 300: 0.45,
              317: FILT["off"], 329: 0.00, 331: 0.90, 333: 1.00, 334: 0.05,
              237: 0.82},
    # Plucked tone. NOT Surge's String oscillator: its parameter slots are
    # reused per oscillator type and REAPER caches the names from
    # instantiation, so the slots kept values meant for a Classic oscillator
    # and the exciter sat at zero - the patch rendered as silence. A saw with a
    # fast decay is a synth pluck rather than a physical model, but it sounds.
    "pluck": {256: OSC["classic"], 259: 0.30, 292: 1.00, 296: 0.00,
              317: FILT["lp24"], 319: 0.60, 320: 0.22,
              252: SHAPER["soft"], 253: 0.50,
              329: 0.00, 331: 0.30, 333: 0.08, 334: 0.26, 237: 0.90},
    # same, driven, for the blues lead
    "pluck_drive": {256: OSC["classic"], 259: 0.42, 292: 1.00,
                    268: OSC["classic"], 269: OCT[0], 271: 0.20, 296: 0.45,
                    317: FILT["obxd12"], 319: 0.64, 320: 0.30,
                    252: SHAPER["fuzz_soft"], 253: 0.62,
                    329: 0.00, 331: 0.36, 333: 0.14, 334: 0.30, 237: 0.92},
}

# role -> patch, per song
ASSIGN = {
    "01_jazz_ballad":   {"lead": "rhodes",   "harmony": "warmpad",
                         "bass": "subbass",  "counter": "pluck"},
    "02_neo_soul":      {"lead": "rhodes",   "harmony": "warmpad",
                         "bass": "subbass",  "counter": "warmpad"},
    "03_drum_and_bass": {"lead": "synclead", "harmony": "warmpad",
                         "bass": "subbass"},
    "04_cinematic":     {"lead": "brightpad", "harmony": "warmpad",
                         "bass": "subbass",  "counter": "pluck"},
    "05_blues":         {"lead": "pluck_drive", "harmony": "organ",
                         "bass": "subbass"},
}

LEVELS = {
    "01_jazz_ballad":   {"lead": 0.40, "harmony": 0.20, "bass": 0.30, "counter": 0.16},
    "02_neo_soul":      {"lead": 0.38, "harmony": 0.20, "bass": 0.34, "counter": 0.15},
    "03_drum_and_bass": {"lead": 0.30, "harmony": 0.16, "bass": 0.36},
    "04_cinematic":     {"lead": 0.72, "harmony": 0.46, "bass": 0.60, "counter": 0.38},
    "05_blues":         {"lead": 0.34, "harmony": 0.22, "bass": 0.32},
}

if __name__ == "__main__":
    for song, roles in ASSIGN.items():
        print(song)
        for role, patch in roles.items():
            print("   %-8s %-14s %d params" % (role, patch, len(PATCHES[patch])))
