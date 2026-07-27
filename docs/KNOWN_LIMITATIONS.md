# Known limitations

What this product does not do, where it is weak, and what you should not trust
it with. Written plainly, because a tool that edits your creative work should be
honest about its edges.

This is not a roadmap. It is a description of version 1.0.0 as built.

---

## Contents

1. [Scope: MIDI only](#1-scope-midi-only)
2. [No audio understanding](#2-no-audio-understanding)
3. [Melody extraction is an inference](#3-melody-extraction-is-an-inference)
4. [No plugins, no instruments, no sound](#4-no-plugins-no-instruments-no-sound)
5. [Style models are rule-based, not learned](#5-style-models-are-rule-based-not-learned)
6. [No candidate is guaranteed to be the right one](#6-no-candidate-is-guaranteed-to-be-the-right-one)
7. [The bridge must be running](#7-the-bridge-must-be-running)
8. [FNV-1a-64 is a change-detection hash](#8-fnv-1a-64-is-a-change-detection-hash)
9. [Commit, discard and undo are transaction-scoped](#9-commit-discard-and-undo-are-transaction-scoped)
10. [Percussion is not a drum-programming feature](#10-percussion-is-not-a-drum-programming-feature)
11. [Rule-to-test coverage is 205 of 237](#11-rule-to-test-coverage-is-205-of-237)
12. [Source licences are unverified](#12-source-licences-are-unverified)
13. [The MCP specification version was not re-verified](#13-the-mcp-specification-version-was-not-re-verified)
14. [The acceptance walkthrough has been run in full](#14-the-acceptance-walkthrough-has-been-run-in-full)
15. [MIDI writing limits](#15-midi-writing-limits)
16. [Analysis limits](#16-analysis-limits)
17. [Platform and packaging limits](#17-platform-and-packaging-limits)
18. [Explicitly deferred](#18-explicitly-deferred)
19. [`StateChangeCount` as a staging precondition](#19-statechangecount-as-a-staging-precondition)
20. [Parallel unisons are under-reported in one list](#20-parallel-unisons-are-under-reported-in-one-list)

---

## 1. Scope: MIDI only

Everything this product analyses and everything it generates is **MIDI note
data** in REAPER.

It reads note events from one MIDI take: pitch, start, end, velocity, channel,
mute and selection state, plus the project tempo and time-signature map. It
writes note events into MIDI items it created itself.

It does **not** read or write: audio items, CC or controller data, pitch bend,
sysex, aftertouch, automation envelopes, take markers, stretch markers, item
fades, track FX, sends other than a validated MIDI send you explicitly asked
for, tempo or time-signature markers, or any project setting.

There is one active or selected MIDI source at a time. Selecting several MIDI
items produces `MULTIPLE_MIDI_SOURCES`; the product does not merge them or pick
one for you.

Take pitch, rate and playrate are not modelled. PPQ-to-quarter-note conversion
goes through REAPER's own conversion functions, so whatever REAPER reports is
what the bridge reports, but nothing here reasons about a time-stretched take.

## 2. No audio understanding

There is no audio analysis of any kind, at all, anywhere.

- No audio-to-MIDI transcription.
- No chord recognition from audio.
- No stem separation.
- No pitch or beat detection from a waveform.
- No timbre, spectrum or loudness analysis.
- No mixing or mastering judgement.

An audio item in your project is invisible to this product. If your song exists
as audio, this tool has nothing to say about it until there is MIDI.

## 3. Melody extraction is an inference

When you hand it polyphonic material and ask for the melody, it is guessing —
in a disciplined, reported way, but guessing.

The `auto` mode prefers your explicit note selection, then tries to detect
monophony, then separates voices and takes the top line. That last step is where
it can be wrong: the top line is a good default and a bad universal rule. An
inner-voice melody under a sustained pad, a melody that crosses below a
countermelody, a keyboard part where the tune moves between hands — all of these
can be extracted incorrectly.

What it does about that:

- It always reports which extraction mode was actually used.
- It records its assumptions in `selection_assumptions` and in the analysis.
- It reports a confidence, and lowers it when it had to infer.
- It raises an `AMBIGUOUS_MELODY` warning when the request cannot be resolved
  unambiguously.
- It **never silently pretends polyphonic material is monophonic.**

What it does not do: get it right every time. If the extraction is wrong, the
harmony built on it is wrong. **Select the notes you mean**, or name an
explicit mode (`selected_notes`, `highest_voice`, `lowest_voice`,
`midi_channel`, `monophonic_voice`, `all_notes_as_harmony`), and check the
reported assumptions before you trust a candidate.

Voice separation is deterministic and cost-based, not perceptual. It favours
pitch proximity and continuity. Dense or crossing textures will defeat it.

## 4. No plugins, no instruments, no sound

Staged tracks are empty MIDI tracks. They contain notes and nothing else.

- No virtual instrument is chosen, installed, loaded or configured.
- No FX chain is created, copied or modified.
- No preset is browsed or recalled.
- No plugin state is read or written, by design.
- No track volume, pan, or routing is set beyond a MIDI send you explicitly
  requested.

**A freshly staged candidate makes no sound until you add an instrument
yourself.** That is expected behaviour, not a bug. Instrument profiles in the
knowledge bundle describe *ranges and idiomatic behaviour* — they inform what is
written, and they are not a link to any actual plugin.

Loading arbitrary plugins is also a security boundary this product declines to
cross; see [`SECURITY.md`](SECURITY.md).

## 5. Style models are rule-based, not learned

The musical judgement here is **147 hand-authored rules across 10 hand-authored
style profiles**, evaluated over a closed vocabulary of 116 predicates. There is
no machine learning, no corpus training, no statistical model of any repertoire.

What that gives you: every decision is inspectable, every rule has an id, every
score component is visible, and identical inputs give identical outputs. You can
read exactly why a chord was chosen and disagree with the reasoning in specific,
citable terms.

What it costs you:

- **The profiles encode conventions, not artists.** `neo_soul_rnb` is a set of
  defensible tendencies about extensions, voicings and harmonic rhythm. It is
  not a model of any particular musician, record or era, and it will not sound
  like one.
- **Coverage is finite.** 147 rules is a substantial encoding of common
  practice, jazz, pop and modal idiom, and it is not all of music theory. A
  situation no rule addresses gets no opinion — the engine reports
  `NotApplicable` rather than inventing one, which is correct but not helpful.
- **Rules can conflict, and the resolution is weighted, not wise.** Score
  weights come from the profile. Different weights would rank candidates
  differently, and no weighting is objectively right.
- **Nothing adapts.** The system does not learn from which candidates you keep.
  Same input tomorrow, same output tomorrow.

Some rules are engineering judgement rather than encoded theory. Those are
labelled `implementation_heuristic` with empty `source_refs`, so you can tell
them apart from rules with a citation. That labelling is deliberate: a plausible
citation attached to a guess would be worse than no citation.

## 6. No candidate is guaranteed to be the right one

**Nothing here guarantees that any generated candidate is the one you want, or
the one another musician would prefer.**

Scores rank candidates against encoded conventions and the parameters you gave.
They do not measure beauty, appropriateness to your song, emotional fit, or
whether the result is interesting. Musicians disagree about harmony
constantly — often correctly, in both directions.

This is why the design works the way it does:

- Several genuinely different candidates, not one answer.
- A visible score vector rather than one number.
- An explanation naming the rules and sources behind each choice.
- Rejected alternatives recorded in the trace.
- Nothing committed automatically, ever.

Treat the output as a competent, well-read collaborator's suggestions. Not as a
verdict.

A related, smaller caveat: **diversity is measured, not guaranteed to be
interesting.** The diversifier maximises measured distance across root motion,
functional path, modal source, bass contour, chord and extension family, voicing
family, harmonic rhythm, cadential behaviour and chromaticism. On short or
harmonically constrained material there may genuinely be few distinct good
options, and you will get near-neighbours because that is what exists.

## 7. The bridge must be running

Every operation that touches your project requires `QLabs_Reaper_MCP_Bridge.lua`
to be running as an action inside REAPER: `reaper.inspect_selection`,
`reaper.stage_candidate`, `reaper.commit_candidate`,
`reaper.discard_candidate` and `reaper.undo_last_generation`. Without it they
fail with `BRIDGE_OFFLINE`.

`reaper.status` is the deliberate exception: it succeeds either way, answering
`"bridge_connected": false` with a `bridge_offline` or `bridge_not_configured`
warning. Reporting whether the bridge is up is the whole point of it, so
failing when the bridge is down would make it useless precisely when it is
needed. Everything that does not touch your project — `theory.search`,
`music.analyze_selection` and the generation tools operating on an existing
snapshot, and every CLI subcommand — works with no bridge at all.

There is no way around this and it is not an oversight. REAPER exposes no
network API, no IPC endpoint and no headless mode that would let an external
process reach a running project. The only supported extension point is
ReaScript, running inside REAPER, which is exactly what the bridge is.

Consequences you will meet:

- **You must start it manually** after opening REAPER. No startup hook is
  installed, and SWS is deliberately not required.
- **It stops when REAPER closes**, and when you run the action a second time.
- **It is single-instance.** A second REAPER instance cannot run a second bridge
  against the same IPC directory; the lock prevents it.
- **A crashed REAPER leaves a stale lock**, which is taken over automatically
  after 10 seconds.
- **Latency is bounded by REAPER's UI timer.** The bridge scans at most every
  50 ms and handles at most 4 commands per defer tick (about 30 Hz), so a
  saturated queue drains at roughly 120 commands per second. This is fine for
  interactive use and is not a high-throughput channel.

Theory search, fixture analysis and generation from an already-captured snapshot
do not need the bridge.

## 8. FNV-1a-64 is a change-detection hash

All six content hashes — `midi_hash`, `note_selection_hash`, `tempo_map_hash`,
`timesig_map_hash`, `note_list_hash` and `snapshot_hash` — are **FNV-1a-64**,
rendered as `fnv1a64:` plus 16 hex digits.

**This is a non-cryptographic hash. It is not collision-proof.**

It reliably detects the thing it exists to detect: you moved the item, edited a
note, changed the tempo, changed the selection. It is *not* collision resistant
against a motivated adversary, and it must never be treated as an authorisation
or an integrity guarantee. A hash match means "probably unchanged", never
"verified".

That trade-off is accepted because the threat it would defend against does not
cross a boundary that exists here — the bridge is a local, same-user, file-based
channel, and anyone able to forge a snapshot hash could edit the project
directly instead. It is also 64 bits over data that changes constantly,
recomputed inside REAPER's UI thread, where a cryptographic hash would cost real
time for no real benefit.

If this ever becomes a boundary that matters, the decision must be revisited.
See [`SECURITY.md` §5](SECURITY.md#5-file-ipc-protections).

The same applies to the installation token: it stops a stray file from being
executed as a command. It is not a secret and it protects nothing from another
process running as you.

## 9. Commit, discard and undo are transaction-scoped

`reaper.commit_candidate`, `reaper.discard_candidate` and
`reaper.undo_last_generation` act **only on objects carrying matching ownership
tags**, and they **do not re-check the source snapshot**.

This is correct — they never touch the source material, so there is nothing for
a source check to protect — but it has consequences worth stating:

- You can commit or discard a staged candidate **after** editing the source
  material it was generated from. The operation succeeds, because it only acts
  on the generated tracks. It will not warn you that the candidate no longer
  matches what you have since written.
- Staleness is enforced at **staging** time, not at commit or discard time. If
  you edit the source between generating and staging, staging is rejected with
  `STALE_SNAPSHOT`. Once material is staged, that check has already happened and
  is not repeated.
- `undo_last_generation` will refuse (`UNDO_NOT_OWNED`) if anything else has
  happened in REAPER since the transaction, because REAPER's top undo entry
  would no longer be ours. Use discard instead in that situation.

The practical rule: **commit is about keeping tracks, not about re-validating
music.** If you have changed the source significantly, discard and regenerate.

## 10. Percussion is not a drum-programming feature

All 16 arrangement roles now have at least one catalogued pattern (32 patterns
in total), so no role relies on borrowing one from a neighbour. The substitution
mechanism is retained as a fallback — for a future role, or for an external
`--knowledge-dir` supplying a thinner catalogue — and when it fires it discloses
itself in the assignment's rationale and a `ROLE_SUBSTITUTED` warning, naming the
role whose pattern was borrowed. Role substitution is also a legitimate contrast
lever in its own right, which is why the mechanism exists at all.

The `percussion` role is the one to set expectations about. `arr_percussive_ostinato`
carries no harmony at all — it writes a fixed rhythmic contour rather than chord
tones — but version one writes ordinary pitched MIDI in a narrow band, **not** a
General MIDI drum map. Route it to a percussion or mallet instrument yourself, and
expect the written pitches to select articulations rather than to sound as pitches.
It is not a groove library, it does not know about drum maps, and it will not
produce an idiomatic drum part. If you want drums, program them.

Similarly, `arr_ear_candy_accent` is deliberately very sparse — one bright detail
per bar. Raising the density control turns it into an ostinato and destroys the
effect it exists for; the same is true of pushing `arr_ornamental_fill` past
roughly 0.3, which turns an ornament into a countermelody.

A test asserts that exactly these four roles need substitution, so if the
catalogue grows to cover one of them, the test will say so.

## 11. Rule-to-test coverage is 205 of 237

The knowledge bundle's 147 rules declare **237 distinct `test_id`s** — named
behaviours the rules assert. **205 of them (86.5%) have a behavioural test
behind them. 32 do not.**

The pending 32 are listed explicitly in
`crates/theory-kb/tests/test_id_coverage.json` under `pending`. They are
declared, not hidden, and the ledger is enforced: `implemented + pending` must
equal every `test_id` appearing anywhere in `knowledge/`, so a rule cannot
quietly lose coverage.

**What this means: 32 named behaviours that the knowledge bundle asserts are not
verified by the test suite.** Among them are augmented-sixth handling,
backdoor-dominant resolution, cadential six-four resolution, applied
leading-tone motion, and several ambiguity-reporting behaviours. Those rules
still fire and still affect scoring — they are just not pinned by a test, so a
regression in them would not be caught automatically.

This is the most concrete piece of unfinished work in the repository, and it is
the honest place to start if you are extending it. See
[`TESTING.md` §5](TESTING.md#5-the-rule-to-test-coverage-ledger).

## 12. Source licences are unverified

All **9 source records** in `knowledge/sources.json` carry:

```json
"license": "unverified-reference-only"
```

**The build environment had no network access**, so no source's licence terms,
canonical URL or locator could be checked. Rather than fabricate plausible
values, the fields say exactly what is true: unverified.

What was done instead, and is verifiable in the repository:

- **No source prose is reproduced anywhere.** All encoded rule text is original.
- Rules that are engineering judgement are labelled `implementation_heuristic`
  with empty `source_refs`, rather than being given a plausible-looking
  citation.
- Every non-heuristic rule names a real source id with a locator, so the claim
  is traceable even though the source itself is unverified.
- Anything that surfaces provenance also surfaces the fact that the licence is
  unverified. That is intentional.

**Before redistributing this knowledge bundle**, verify each source's terms and
replace `unverified-reference-only` with the real identifier. The checklist is
in [`RESEARCH_AND_PROVENANCE.md` §8](RESEARCH_AND_PROVENANCE.md) and the
per-source table is in [`knowledge/LICENSE.md`](../knowledge/LICENSE.md).

## 13. The MCP specification version was not re-verified

The server targets MCP specification version **`2025-11-25`**, the stable
specification named in the product brief.

The brief asked that modelcontextprotocol.io be checked for a newer stable
specification before implementation. **That check could not be performed**: the
documentation host is not reachable from the build environment (HTTP 403 through
the egress proxy).

So `2025-11-25` is the **selected** version, not a **verified-latest** version.
If a newer stable specification has since been published, this build does not
know about it. Recorded as decision D2 in
[`ARCHITECTURE.md`](ARCHITECTURE.md#d2--mcp-protocol-version).

Relatedly: because the build environment blocked crates.io, this workspace does
**not use the official MCP Rust SDK**. The JSON-RPC layer, the schema validator
and the protocol surface are hand-written. **MCP conformance is therefore this
project's own responsibility**, verified by its own protocol test suite rather
than inherited from an SDK. If your MCP client behaves oddly against this
server, a conformance gap on our side is a plausible explanation and worth
reporting.

## 14. The acceptance walkthrough has been run in full

`reaper/QLabs_Reaper_MCP_Smoke_Test.lua` **has now been run** against REAPER
7.78/x64 on Windows 11, unmodified, in a fresh project tab: **28 passed, 0
failed**. Source-MIDI-hash integrity, ownership tagging, preview status and
single-entry undo all held against the real host.

The bridge's behaviour is additionally covered by **184 automated cases against a
mock REAPER host**, and the mock is a serious one: it implements every `reaper.*`
function the bridge calls, including a real undo journal that records inverse
closures, replays them in reverse, preserves pointer identity and resurrects
deleted objects. A suite asserts the mock cannot fall behind the code. The smoke
test run is what establishes that the model and the product agree.

The manual acceptance walkthrough has since been run against the same host.
Stale-snapshot rejection, `reaper.commit_candidate`, `undo_last_generation`
through the MCP tool, and `UNDO_NOT_OWNED` after an unrelated REAPER action all
behaved as documented.

All sixteen steps have since been run, including the two that were qualified
here: the bridge registered and started through REAPER's **Actions list**, and
the brief's **natural-language request** driven from the server's own
`harmonize-selected-melody` prompt with the countermelody enabled.

Three notes worth keeping from doing them. The bridge resolves `lib/` relative
to its own location, so it only runs correctly from its installed directory. The
toolbar toggle works only when it is launched as an action, because
`reaper.get_action_context()` reports command id `0` for a command-line launch.
And what remains untested is not a step but a *surface*: of the nine prompts,
only `harmonize-selected-melody` has been exercised against a real host.

Running the walkthrough also uncovered a staging bug — a cached generation
returned a candidate bound to a superseded snapshot, so staging succeeded only
once per (music, profile, seed) per server process. That is fixed; the history is
in [`TESTING.md` §10](TESTING.md#10-the-in-reaper-smoke-test--executed).

Both procedures are written out in
[`TESTING.md` §10](TESTING.md#10-the-in-reaper-smoke-test--executed).

## 15. MIDI writing limits

- **Only note events are written.** No CC, pitch bend, sysex or automation.
- **Folder nesting is one level deep**, which matches the staging layout the
  product calls for. Deeper hierarchies would need a real depth-accumulation
  pass over REAPER's folder-depth field.
- **Notes are clamped into their item's bounds** before insertion. A note that
  would collapse to zero length after clamping is rejected as an invalid plan
  rather than silently dropped.
- **Note spelling does not survive into the MIDI.** MIDI stores a number.
  `PlannedNote::spelling` records the intended spelling in the plan and the
  trace, so the reasoning is preserved, but REAPER will show you the pitch its
  own way.
- **Hard caps per staging operation:** 20 000 notes per plan, 8 000 notes per
  item, 32 tracks, 64 items, 16 regions, 16 sends, 512 operations. Exceeding any
  of them rejects the whole plan before any mutation.
- **Regions are created but not managed.** The product does not rename, move or
  clean up regions it created in a previous transaction.
- **Without js_ReaScriptAPI, file ages fall back to first-seen.** This only
  affects garbage-collection timing inside the IPC directory, never musical
  behaviour.

## 16. Analysis limits

- **Chord detection needs enough material.** Sparse or highly ambiguous
  polyphony produces low-confidence chords, which is reported rather than
  hidden.
- **Key inference is a ranking, not an answer.** Short fragments, chromatic
  material and genuinely ambiguous modal writing produce close candidates and a
  small confidence gap. The `ambiguous` flag says so. A user hint overrides
  inference but is still scored, so you can see whether the music agreed.
- **Modulation needs a confirmation window.** Brief tonicizations are reported
  as tonicizations rather than key changes, which is usually right and
  occasionally not.
- **NCT classification can be genuinely ambiguous**, and returns several
  hypotheses with confidences rather than one forced answer. Downstream
  consumers must handle that.
- **Salience is a transparent weighted sum**, not a perceptual model. The
  weights are documented and inspectable precisely so you can see it is a
  heuristic.
- **Microtonality is not supported.** Version one is 12-tone equal temperament.
  A `cents` field exists on notes for future compatibility and is always `0.0`.
- **Analysis is per-selection.** There is no whole-project analysis, no
  cross-item structure detection, and no awareness of anything you did not
  select.

## 17. Platform and packaging limits

- **Windows 11 is the only supported installation platform.** The PowerShell
  scripts are the only installer. The Rust workspace builds and its tests pass
  on Linux (CI runs both Linux and Windows), and the bridge is portable Lua, but
  there is no macOS or Linux installer and neither has been through acceptance.
- **REAPER 7.x is the target.** The bridge refuses REAPER below major version 6
  with `UNSUPPORTED_REAPER_VERSION`.
- **No auto-update.** Upgrading means rebuilding and re-running the installer.
- **The installer cannot register the REAPER action for you**, and the
  uninstaller cannot remove it. REAPER's action list is REAPER's own state, and
  this product does not edit REAPER configuration files.
- **The IPC directory inherits its permissions from its parent.** The installer
  does not tighten the ACL. Do not place it somewhere other accounts can write.

## 18. Explicitly deferred

These are out of scope for version one by decision, not by oversight, and none
of them is present in any partial form:

FL Studio or any other DAW · audio-to-MIDI transcription · audio chord
recognition · stem separation · real-time note generation during playback · live
accompaniment · mixing or mastering · automatic plugin installation · automatic
virtual-instrument selection · preset browsing · cloud inference · remote
project control · a native C++ REAPER extension · microtonal generation beyond
retaining a future-compatible cents field · music-notation engraving ·
orchestration sample-library management.

## 19. `StateChangeCount` as a staging precondition

Every edit plan carries REAPER's project state-change counter among its seven
preconditions, alongside the project UUID, the item and take GUIDs, the MIDI
hash, the tempo-map hash and the item bounds.

That counter increments on **any** project edit — including a mere selection
change. So a perfectly benign interaction between generating a candidate and
staging it, a click on another track, a nudge of the edit cursor that REAPER
counts, can move the counter and produce a `PROJECT_CHANGED` / `STALE_SNAPSHOT`
rejection for material that did not actually change.

Nothing is lost when this happens: preconditions are evaluated before any object
is created, so a rejected plan writes nothing at all. The safe response is
always the same one the error's own `remedy` gives — call
`reaper.inspect_selection` again and regenerate. The regenerated candidate is
deterministic, so on genuinely unchanged material you get the same music back.

This is a **precision** problem, not a **safety** problem: the precondition is
too eager, never too permissive. If it proves noisy in real use, that one
precondition is the candidate for removal. The content hashes — `midi_hash`,
`tempo_map_hash`, `snapshot_hash` — are the substantive guard, and they change
only when the material does.

## 20. Parallel unisons are under-reported in one list

`harmony-engine`'s voice-leading report omits parallel unisons from its
`parallels` list. The list excludes zero-interval motion, so two voices moving
in parallel at the unison are simply not recorded there.

The rule still fires. `counterpoint.no_parallel_unisons` is evaluated normally,
matches normally, and applies its penalty normally, so the **score is correct**
and the **explanation is correct** — the rule appears in `rule_applications`
with its `score_delta` and its matched conditions, and `candidate.explain`
reports it like any other.

The consequence is narrow but real: a client reading the `parallels` array
directly, in a voicing result or a voice-leading audit, should not treat it as
the complete record of parallel motion. The rule applications are. Where the two
appear to disagree about unisons, the rule applications are right.

---

## Where to look next

| Question | Document |
|---|---|
| How is it put together? | [`ARCHITECTURE.md`](ARCHITECTURE.md) |
| What exactly is a chord here? | [`MUSIC_IR.md`](MUSIC_IR.md) |
| How do the rules work, and how do I add one? | [`THEORY_MODEL.md`](THEORY_MODEL.md) |
| Where did the theory come from? | [`RESEARCH_AND_PROVENANCE.md`](RESEARCH_AND_PROVENANCE.md) |
| What can it do to my machine? | [`SECURITY.md`](SECURITY.md) |
| What is actually tested? | [`TESTING.md`](TESTING.md) |
| How do I install it? | [`INSTALL_WINDOWS.md`](INSTALL_WINDOWS.md) |
