# First sessions in REAPER

A set of ready-to-run scenarios for the first time this product touches a real
REAPER project. Each one gives you exactly what to put in the project, exactly
what to ask the AI client, what it should do, and how to tell whether it worked.

Nothing here has been executed inside REAPER. The engine, the knowledge base and
the bridge logic are covered by 2238 Rust tests and 184 Lua tests, but the tests
run against a mock REAPER host — **these scenarios are the first real validation**,
so treat a failure as informative rather than surprising, and see
[§9](#9-when-something-goes-wrong) for what to capture.

Work through §1–§4 in order the first time. After that, jump to whatever you want.

---

## Contents

1. [Pre-flight](#1-pre-flight)
2. [How to read what comes back](#2-how-to-read-what-comes-back)
3. [Tier 0 — can it see me?](#3-tier-0--can-it-see-me)
4. [Tier 1 — analysis on known material](#4-tier-1--analysis-on-known-material)
5. [Tier 2 — harmonization](#5-tier-2--harmonization)
6. [Tier 3 — safety drills](#6-tier-3--safety-drills)
7. [Tier 4 — arrangement and looping](#7-tier-4--arrangement-and-looping)
8. [Creative prompt templates](#8-creative-prompt-templates)
9. [When something goes wrong](#9-when-something-goes-wrong)

---

## 1. Pre-flight

Run once per REAPER session.

1. `.\scripts\install.ps1` (first time only).
2. Open REAPER. **Actions → Show action list → New action → Load ReaScript →**
   `QLabs_Reaper_MCP_Bridge.lua`, then run it. Leave it running.
3. Ask your client: **"Check REAPER status."**

You want `bridge_connected: true`, a REAPER version, and a knowledge version.

> If `bridge_connected` is `false`, `reaper.status` still answers — that is
> deliberate, it is the tool whose job is to tell you the bridge is down. Every
> other project-touching tool returns `BRIDGE_OFFLINE`.

Use a **scratch project**, not something you care about, until you have finished
§6. The product is designed not to touch your source material, but you should
confirm that yourself rather than take it on trust.

---

## 2. How to read what comes back

Every candidate carries a decision trace. Four things are worth actually looking at:

| Field | What it tells you |
|---|---|
| `score.components` | Thirteen named components, not one number. If a candidate looks wrong, the component that is high tells you what the engine was optimising. |
| `rule_applications` | Real rule ids from `knowledge/`. `status` is `applied`, `bypassed`, `not_applicable` or `violated`. A `bypassed` entry names the exception that fired. |
| `confidence` + `warnings` | Low confidence and an `AMBIGUOUS_MELODY` warning together mean it guessed at which notes are the tune. |
| `assumptions` | What it decided for you — extraction mode, harmonic rhythm, tonal centre. |

Ask **"explain candidate 2 in detail"** at any point. The explanation is generated
from the structured decisions, not narrated after the fact, so if it cites
`extensions.guide_tones_have_highest_retention_priority` that rule genuinely fired.

---

## 3. Tier 0 — can it see me?

**Setup.** New project. One MIDI item, four bars. Draw any four notes.

**Ask:** *"Inspect my selection and tell me what you found."*

**Expected:** one `reaper.inspect_selection` call returning a snapshot id, your
note count, and the tempo/time signature.

**Pass if:**
- The note count matches what you drew.
- The reported tempo and meter match the project.
- No error.

**Then test the two failure modes deliberately:**

| Do this | Expect |
|---|---|
| Deselect everything, no MIDI editor open | `NO_MIDI_SOURCE` |
| Select **two** MIDI items | `MULTIPLE_MIDI_SOURCES` |
| Select some notes inside the item | Only those notes reported, `note_scope: selected_only` |

That third one matters: **selection is how you tell it what you mean.** If you
select nothing it uses the whole take.

---

## 4. Tier 1 — analysis on known material

This is the highest-value test in the document, because the expected answer is
already committed to the repository and you can compare directly.

**Setup.** New MIDI item, **110 BPM, 4/4, 8 bars**. Enter this melody — it is
`fixtures/melodies/eight_bar_c_major.json`, note for note:

| Bar | Notes (pitch @ beat, length in beats) |
|---|---|
| 1 | C4@1(1) E4@2(1) G4@3(1) E4@4(1) |
| 2 | F4@1(1) A4@2(1) G4@3(2) |
| 3 | D4@1(1) F4@2(1) A4@3(1) F4@4(1) |
| 4 | E4@1(1) G4@2(1) C5@3(2) |
| 5 | B4@1(1) A4@2(1) G4@3(1) F4@4(1) |
| 6 | E4@1(1) D4@2(1) C4@3(2) |
| 7 | E4@1(1) G4@2(1) C5@3(1) B4@4(1) |
| 8 | A4@1(1) F4@2(1) G4@3(1) C4@4(1) |

**Ask:** *"Analyse the selected melody."*

**Pass if the analysis reports:**
- **C major** as the top key candidate, confidence around **0.65**
- **2 phrases**
- **8 harmonic grid slots** — i.e. roughly one chord per bar, *not* one per note
- Range C4–C5

Compare against `fixtures/expected/melodies-eight_bar_c_major.json` in the repo.
It will not match byte for byte — your entry will differ slightly in velocity and
exact tick positions — but the key, phrase count and slot count should agree.

> **Why one chord per bar matters.** A naive harmonizer puts a chord under every
> note. If you get 29 slots instead of 8, the grid logic is not working and
> everything downstream will be wrong.

### 4b. The modal test

**Setup.** New item, **84 BPM, 4/4**. Enter:

`D4@1(2) F4@3(1) G4@4(1) | A4@1(2) B4@3(2) | A4@1(1) G4@2(1) F4@3(2) | E4@1(2) D4@3(2)`

**Ask:** *"What key is this in? Show me your top few candidates and the evidence."*

**Pass if:** **D Dorian** ranks above D natural minor. The distinguishing evidence
is the **B natural** — Dorian's characteristic 6. If it says D minor, the modal
detection is not weighting characteristic degrees.

This is the test that proves it is doing theory rather than pattern-matching a
major/minor template.

---

## 5. Tier 2 — harmonization

Use the eight-bar C major melody from §4.

**Ask:**

> *Create three harmonizations. Preserve the melody and timing. Make one warm and
> extended, one dark and modal, and one chromatic. Add bass and a restrained
> countermelody. Keep the eight-bar region loopable.*

**Expected tool sequence:** `reaper.status` → `reaper.inspect_selection` →
`music.analyze_selection` → `harmony.generate_candidates` → `candidate.explain` ×3.
**It should not stage anything yet.**

**Pass if:**
- Three candidates come back with **structurally different** chord sequences — not
  the same progression with extensions added. Expect roughly: one functional and
  cadence-directed, one modal or common-tone, one chromatic or bass-led.
- Every melody pitch and onset is preserved (ask it to confirm).
- Each explanation cites real rule ids and source ids.
- A loop report is included.

**Then push on it:**

| Ask | What it tests |
|---|---|
| *"Do the same but with the `jazz_standard` profile."* | Extensions, ii–V motion, guide-tone continuity appear |
| *"Now `strict_counterpoint`."* | Triadic, far fewer extensions, parallel fifths penalised |
| *"Now `blues`."* | Dominant sevenths treated as stable, no forced resolution |
| *"Now `modal_ambient`."* | Slower harmonic rhythm — expect ~3 chords over 8 bars, not 9 |
| *"Regenerate with seed 12345, twice."* | **Identical** output both times |
| *"Same seed, but complexity 0.2 instead of 0.8."* | Simpler chord vocabulary |

The profile test is the one to trust your ears on. If `strict_counterpoint` and
`blues` produce similar harmony, something is wrong.

---

## 6. Tier 3 — safety drills

**Do these on a scratch project.** They are the drills that tell you whether it is
safe to point at real work.

### 6a. Staging does not touch your source

1. Generate candidates (§5). Note your source item's exact contents.
2. *"Stage candidate 1."*
3. **Check:** new tracks appeared in a folder (Chords / Bass / Countermelody), and
   **your original MIDI item is byte-for-byte unchanged** — same notes, same
   position, same length, not muted.

**Fail loudly if the source changed at all.** That is the product's central promise.

### 6b. Discard removes only that candidate

1. Stage candidate 1. Stage candidate 2 as well.
2. Manually add a track of your own called `Chords` — *deliberately the same name*
   as a generated one.
3. *"Discard candidate 1."*
4. **Check:** candidate 1's tracks are gone; candidate 2's remain; **your own
   `Chords` track is untouched.**

Ownership is by hidden tag, never by track name. This drill proves it.

### 6c. Stale-snapshot rejection

1. Inspect and generate candidates.
2. **Before staging**, edit the source melody — move or add one note.
3. *"Stage candidate 1."*
4. **Expect a refusal:** `STALE_SNAPSHOT`. Nothing should be created.
5. *"Re-inspect and regenerate, then stage."* — now it should work.

This is the guard that stops it writing harmony for a melody you have since changed.

### 6d. Owned undo

1. Stage a candidate.
2. In REAPER, do something unrelated — rename a track, move an item.
3. *"Undo the last generation."*
4. **Expect `UNDO_NOT_OWNED`.** It must refuse, because the top undo entry is your
   action, not its own.
5. Undo your own action manually, then ask again — now it should undo its own
   transaction.

### 6e. Commit

1. Stage, then *"Commit the candidate."*
2. **Check:** generated tracks remain as normal project content, the source is
   still untouched, and one undo point exists.

---

## 7. Tier 4 — arrangement and looping

**Ask:** *"Arrange candidate 1 into lead, bass, pad, comping and counterlead parts
with a rising energy curve."*

**Pass if:** each part is in a sensibly different register, the pad sustains, the
comping follows a rhythmic grid, and the countermelody does **not** hit every
melody onset.

**Loop test.** Set an 8-bar loop over the region, then:

| Ask | Expect |
|---|---|
| *"Audit this loop for `closed_tonic` intent."* | High compatibility if it ends on V or I; suggestions if not |
| *"Now audit it for `modal_drone`."* | **Should not demand a dominant.** A modal loop is served by common tones or a pedal |
| Drag a note past the loop end, re-audit | Hanging note detected |
| Add a pickup before bar 1, re-audit | Pickup detected and reported |

The modal-drone case is the one to check carefully: if it insists on V–I for
every intent, the loop intent logic is not being applied.

---

## 8. Creative prompt templates

Once the drills pass, these are the prompts worth actually working with.

**Reharmonize something you already have**
> *Take my existing chords and reharmonize them three ways, preserving the melody
> and the cadence. Keep the harmonic rhythm. Use tritone substitutions and
> secondary dominants where they fit.*

**Voicings only**
> *Keep this progression exactly as it is, but generate three voicing options for
> four voices: one close, one drop-2, one rootless. Preserve the top voice.*

**Turn a chord sketch into an arrangement**
> *This is a chord sketch. Arrange it for pad, pluck, bass and a counterline, with
> the density low in the first four bars and building after that. Keep everything
> out of the lead's register.*

**Make a loop actually loop**
> *Audit this 4-bar loop for seamless colour, tell me what breaks the seam, and
> generate a variant that fixes it without changing the melody.*

**Explore a style honestly**
> *Harmonize this in `neo_soul_rnb`, then in `cinematic`, and tell me concretely
> what changed and why — which rules fired differently.*

**Ask it to justify itself**
> *Why did you choose that chord in bar 3? Which rules fired, what did they score,
> and what was the runner-up?*

That last one is the most useful habit. The engine keeps its rejected alternatives
and its score components; you can argue with it in specific terms.

---

## 9. When something goes wrong

Capture these four things — together they are almost always enough to diagnose it:

1. **The exact prompt** you gave.
2. **The tool call and its arguments** (ask the client to show them).
3. **The full error**, including the `code` and the `details` object. The codes are
   documented in [`MCP_API.md`](MCP_API.md) §6.
4. **`doctor --json` output**, and whether the bridge was running.

Useful first checks:

| Symptom | Look at |
|---|---|
| Everything returns `BRIDGE_OFFLINE` | Is the ReaScript still running? It stops when REAPER closes, and running the action a second time stops it |
| `STALE_SNAPSHOT` on every stage | Did the project change between inspect and stage? Note that the state-change counter also ticks on selection changes — see [`KNOWN_LIMITATIONS.md`](KNOWN_LIMITATIONS.md) §19 |
| Melody extracted wrongly | Select the notes you mean explicitly, or name a mode (`highest_voice`, `midi_channel`, …) |
| Harmony changes on every note | The grid is collapsing — capture the analysis output, this is a real bug |
| Candidates all sound alike | Capture all three chord sequences and the `candidate_diversity` score component |

If the bridge logs are needed, they are under the IPC directory's `logs/`, and
failed requests are quarantined in `failed/` rather than deleted.

---

## Appendix — what has and has not been proven

**Verified by automated tests:** the theory engine, chord semantics, key and NCT
analysis, candidate generation and diversity, voicing and voice leading,
arrangement realisation, loop auditing, the IPC protocol in both directions, the
MCP surface, and the bridge's logic against a mock REAPER host — including
staging, discard, commit, owned undo and stale-snapshot rejection.

**Not yet verified:** any of it running inside a real REAPER. Every scenario above
exercises a path that has only ever been tested against a mock. `reaper/QLabs_Reaper_MCP_Smoke_Test.lua`
runs an automated version of §3, §6a and §6d if you would rather start there —
it creates its own clearly marked material and refuses to run against a saved
production project unless you explicitly allow it.
