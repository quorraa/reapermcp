# Architecture

How the QLabs REAPER Music Intelligence MCP is put together, why, and what each
part is responsible for.

Companion documents: [`MUSIC_IR.md`](MUSIC_IR.md) for the musical types,
[`THEORY_MODEL.md`](THEORY_MODEL.md) for the rule system, `MCP_API.md` for the
tool surface, [`REAPER_BRIDGE.md`](REAPER_BRIDGE.md) for the Lua side, and
[`SECURITY.md`](SECURITY.md) for the trust boundaries.

---

## Contents

1. [Shape of the system](#1-shape-of-the-system)
2. [The MCP server](#2-the-mcp-server)
3. [The theory engine](#3-the-theory-engine)
4. [The knowledge bundle](#4-the-knowledge-bundle)
5. [The analysis pipeline](#5-the-analysis-pipeline)
6. [The candidate solver](#6-the-candidate-solver)
7. [REAPER IPC](#7-reaper-ipc)
8. [Transaction lifecycle](#8-transaction-lifecycle)
9. [Data ownership](#9-data-ownership)
10. [Determinism](#10-determinism)
11. [Architecture decisions](#11-architecture-decisions)

---

## 1. Shape of the system

Two processes, one shared directory, no sockets.

```
  MCP host  ──stdio JSON-RPC──▶  qlabs-reaper-music-mcp  ──files──▶  <ipc-dir>
                                          │                              │
                                  music engine + knowledge          polled by
                                                                    defer() loop
                                                                         │
                                                                         ▼
                                                      QLabs_Reaper_MCP_Bridge.lua
                                                                         │
                                                            official ReaScript API
                                                                         ▼
                                                                  REAPER project
```

The Rust process never touches the REAPER project. It cannot: it has no REAPER
API, and the only channel to REAPER is a directory of JSON files that the bridge
polls, validates and executes against a closed allowlist of seven commands.

### Crate dependency graph

```
qjson
  └─ music-domain
        └─ theory-kb
              └─ music-analysis
                    └─ harmony-engine
                          ├─ arrangement-engine
                          └─ loop-engine
qjson, music-domain
  └─ reaper-ipc
everything
  └─ reaper-music-mcp        (binary: qlabs-reaper-music-mcp)
qjson, theory-kb
  └─ xtask
```

Every arrow is a path dependency inside this workspace. There are no others —
see [D1](#d1--zero-external-rust-dependencies).

The layering is deliberate: `music-analysis`, `harmony-engine`,
`arrangement-engine` and `loop-engine` know nothing about REAPER or MCP, and
`reaper-ipc` knows nothing about music theory. Only `reaper-music-mcp` sees
both halves. That is what makes the entire music engine testable against a
fixture corpus with no DAW present.

---

## 2. The MCP server

The `reaper-music-mcp` crate is the process. It is a **synchronous** JSON-RPC
2.0 server reading newline-delimited messages from `stdin` and writing them to
`stdout`. There is no async runtime and no MCP SDK
(see [D1](#d1--zero-external-rust-dependencies) and [D4](#d4--synchronous-server-threaded-ipc-waits)).

**Transport invariants.**

- `stdout` carries protocol bytes only. No banners, no logs, no progress text,
  no panic messages, no stack traces. A panic hook writes to `stderr` and
  returns a JSON-RPC error.
- All diagnostics go to `stderr`.
- No TCP or HTTP listener exists anywhere in the workspace.
- Graceful shutdown on EOF.

**Protocol surface.** `initialize` reports protocol version `2025-11-25`
(see [D2](#d2--mcp-protocol-version)), the server name and version, and
capabilities for tools, resources and prompts with `listChanged: false`. The
server handles `initialize`, `initialized`, `tools/list`, `tools/call`,
`resources/list`, `resources/templates/list`, `resources/read`, `prompts/list`,
`prompts/get`, `ping` and `notifications/cancelled`.

**Tool results** carry both `structuredContent`, validated against the tool's
declared `outputSchema`, and a `content` array with a serialized text fallback
for clients with limited structured-output support. An error *inside* a tool
returns `isError: true` with a structured payload — not a JSON-RPC transport
error. Argument validation failures are handled the same way, so a malformed
request never looks like a broken server.

**Long operations** emit `notifications/progress` when the caller supplied a
`progressToken`, and honour `notifications/cancelled` through a shared
`CancelFlag` that the search loops poll.

**The tool surface is closed and semantic.** There is no `execute_lua`,
`execute_shell`, `run_reaper_action`, `write_arbitrary_midi`,
`delete_track_by_name`, `edit_project_chunk`, filesystem passthrough or REAPER
API passthrough. Those are not disabled behind a flag; they do not exist. The
reasoning is in [`SECURITY.md`](SECURITY.md).

**Session store.** In-memory, bounded and TTL'd. It holds snapshots, analyses,
candidates, edit plans and transactions, keyed by server-issued ids. Expired ids
are rejected with a clear code rather than silently regenerated. Candidates are
cached on the tuple (snapshot, profile, params, knowledge hash, seed) and the
cache is never consulted across a changed snapshot. Nothing is written to disk;
nothing survives the process.

A **resource URI can never name a filesystem path.** Resource reads resolve
server-issued ids against the session store and enforce ownership and TTL.

The full tool, resource, prompt, argument, output and error reference is in
`docs/MCP_API.md`.

**CLI.** Besides `serve`, the binary offers `doctor` (human-readable and
`--json`), `validate-knowledge`, `list-profiles`, `analyze-fixture <file>`,
`generate-fixture <file>`, `print-mcp-config` and `version`. The fixture
commands run the real engine with no REAPER present, which is what makes
deterministic golden testing and offline debugging possible.

---

## 3. The theory engine

`theory-kb` turns `knowledge/` from data into behaviour.

**Loading and validation.** The bundle is parsed once behind a `OnceLock` from
an embedded copy, or loaded from an external override directory with
`KnowledgeBase::load_dir`. Validation is strict and rejects: schema violations,
duplicate ids, unresolved source references, profile-inheritance cycles, missing
parents, rules naming unknown profiles, scales, chord qualities or score
components, **unknown predicates**, unknown rule kinds/domains/events,
`score_weights` that do not cover exactly the 13 score components,
`rule_overrides` naming unknown rules, malformed degree strings, empty
`test_ids`, and a `content_sha256` mismatch. The content hash is always
recomputed and never trusted from the manifest.

**Profiles.** A `StyleProfile` may inherit from a parent. `resolve_profile`
flattens the chain, applies rule overrides (a multiplier, or disabled), and
normalizes the score weights so every one of the 13 components is present. The
result, a `ResolvedProfile`, is what every engine consults — no engine reads a
raw profile file.

**The rule engine.** Rules fire on events and are evaluated against a
`RuleContext`, an insertion-ordered bag of named facts that the engines fill in.
Conditions and exceptions are drawn from a **closed vocabulary of 116
predicates**. Three properties matter:

- A predicate whose inputs are absent evaluates to *unknown*, and a rule with any
  unknown condition reports `NotApplicable` rather than firing on a guess.
- Hard rule kinds are evaluated first and reported as `Violated`, so a caller can
  reject a candidate before any soft scoring runs.
- A predicate appearing in `knowledge/` that the engine does not implement is a
  **validation error**, not a silent no-op. A test asserts that the knowledge
  bundle's predicate set and `RuleEngine::known_predicates()` are equal in both
  directions.

Every evaluation produces a `RuleApplication` recording the rule id, its status,
its score delta, which conditions and exceptions matched, the source ids behind
it and a human explanation. Those applications are what decision traces are
built from — the explanation is assembled from structured decisions, never
invented prose.

**Theory search** powers the `theory.search` tool with deterministic scoring:
token overlap over ids, summaries, rationales and names, with exact-id and
exact-phrase boosts, ties broken by id, no randomness.

Rule kinds, the predicate table and the procedure for adding a rule safely are
in [`THEORY_MODEL.md`](THEORY_MODEL.md).

---

## 4. The knowledge bundle

`knowledge/` is data, not code, and it is the product's actual musical
competence. Version `1.0.0`.

| File | Contents | Count |
|---|---|---|
| `rules/harmony.json` | Harmony rules | 37 |
| `rules/voice_leading.json` | Voice-leading rules | 31 |
| `rules/extensions.json` | Extension and alteration rules | 20 |
| `rules/melody.json` | Melody rules | 17 |
| `rules/arrangement.json` | Arrangement rules | 15 |
| `rules/looping.json` | Looping rules | 14 |
| `rules/counterpoint.json` | Counterpoint rules | 13 |
| `profiles/*.json` | Style profiles | 10 |
| `scales.json` | Scale definitions | 35 |
| `chord_qualities.json` | Chord qualities | 51 |
| `chord_symbols.json` | Symbol aliases | 65 |
| `functions.json` | Function entries | 45 |
| `voicings.json` | Voicing templates | 38 |
| `progressions.json` | Progression schemas | 37 |
| `cadences.json` | Cadence schemas | 14 |
| `arrangement_patterns.json` | Arrangement patterns | 28 |
| `instrument_profiles.json` | Instrument profiles | 14 |
| `modes.json` | Mode records | 22 |
| `intervals.json` | Interval records | 28 |
| `sources.json` | Source records | 9 |
| `manifest.json` | Version, counts, file list, `content_sha256` | — |

**Total: 147 rules.**

**Embedding.** `crates/theory-kb/src/embedded.rs` is a generated file holding
`EMBEDDED_FILES: &[(&str, &str)]` built from `include_str!` of every file under
`knowledge/`. It is committed, regenerated with
`cargo run -p xtask -- regen-embedded`, and a test asserts the list matches
what is on disk so it can never silently drift. The consequence is that the
shipped binary carries its entire theory bundle and needs nothing from the
filesystem to reason.

**Overrides.** `--knowledge-dir` loads an external bundle instead. External
bundles are validated with the same strictness, and — unlike the embedded
bundle — a `manifest.json` whose `content_sha256` is the literal string
`"PENDING"` is rejected rather than accepted.

**Provenance.** Every rule either names real source ids in `sources.json` with
locators, or is explicitly `implementation_heuristic` with empty `source_refs`.
No plausible-looking citation is ever attached to engineering judgement, and no
source prose is reproduced anywhere. See
[`RESEARCH_AND_PROVENANCE.md`](RESEARCH_AND_PROVENANCE.md) and
[D6](#d6--knowledge-provenance-under-no-network-conditions).

---

## 5. The analysis pipeline

`music-analysis` turns a REAPER snapshot into an `Analysis`, which is the input
to everything downstream and is served to clients as `analysis://{id}`.

**Stage 1 — selection normalization and melody extraction.** Modes: `auto`,
`selected_notes`, `highest_voice`, `lowest_voice`, `midi_channel`,
`monophonic_voice`, `all_notes_as_harmony`. `auto` prefers selected notes, then
detects monophony, then separates voices and takes the top line — reporting the
assumption, a reduced confidence and an `AMBIGUOUS_MELODY` warning. It **never
silently pretends polyphonic material is monophonic.** The result splits the
snapshot into a `melody` NoteSet and an `accompaniment` NoteSet, and carries the
mode actually used, the assumptions, the confidence, the warnings and the voice
count.

Voice separation itself is deterministic: a cost-based assignment favouring
pitch proximity and continuity.

**Stage 2 — phrase and salience.** Phrases, subphrases, pickups, gaps and
motives, including motive transforms. Salience is a *transparent weighted sum*
over metric position, duration, phrase edges, leap targets, registral extremes,
motivic membership, cadential position and accent. The weights are public and
each note's per-component breakdown is retained, so a claim that a note is
structural can always be audited.

**Stage 3 — key and mode.** Produces **ranked candidates**, never one forced
major/minor label. Evidence includes duration-weighted pitch distribution,
metric placement, long notes, phrase edges, bass emphasis, cadential motion,
leading-tone presence, tonic/dominant emphasis, chord evidence when the material
is polyphonic, modal characteristic degrees and pedal tones. Key regions and
tonicizations are separate from the global ranking. A user-supplied hint
overrides inference but is still scored and reported, so you can see whether the
music agreed with you.

**Stage 4 — harmonic grid.** Chooses where chords may change. It must not
default to one chord per note; `auto` uses phrase structure, the profile's
harmonic-rhythm preference and salience. Each slot records its melody notes, its
structural notes, whether it is cadential, and its weight.

**Non-chord-tone classification** runs against a harmonic hypothesis and uses
surrounding motion, metric position and duration — never pitch membership alone.
An ambiguous note gets several hypotheses with confidences rather than one
forced answer.

**Chord detection** reads chords out of polyphonic material against the grid and
key analysis.

The `Analysis` bundles all of it plus loop observations, confidence, warnings,
the knowledge version and TTL timestamps. Its id is derived with
`uuid_from_name` over the snapshot id and the canonical JSON of the parameters,
so the same request always yields the same id — which is what makes golden
testing possible.

---

## 6. The candidate solver

`harmony-engine` is where candidates come from.

**Stage 5 — per-slot option pools.** For each grid slot, `slot_options` builds
`ChordOption`s from diatonic harmony, modal interchange, applied dominants,
tritone substitution, planing and the other strategies the brief enumerates.
Each option carries its function, roman numeral, source strategy, which chord
degrees the melody notes occupy, a local score vector, its rule applications and
its measured chromaticism and complexity. **Hard rule violations are filtered
here**, before any path search runs, so the search never spends its beam on
material that would be rejected anyway.

**Stage 6 — global path search.** A **beam search over whole phrases**, not
greedy per-slot selection. The transition cost includes voice leading,
functional or modal coherence, bass quality and phrase direction, so a locally
attractive chord that ruins the next three bars loses. The search is bounded on
every axis: beam width, options per slot, total paths and diversity paths. It
checks the cancellation flag in its inner loops and reports progress. Ties are
broken by a seeded deterministic RNG derived from the request seed and the slot
index — seeds are used *only* for tie-breaking, never to introduce variation.

**Stage 7 — voicing.** Voices are assigned **across time as a connected path**
(a minimum-cost assignment or a bounded dynamic program), preserving voice
identity, chord identity and the requested extensions. A chord is never
re-voiced in isolation. 13 voicing families are available.

**Voice-leading audit** reports connections, total motion, maximum leap,
parallels (including hidden), crossings, overlaps and unresolved tendency tones.
All of it is **profile-driven**: `strict_counterpoint` penalises parallel
perfects heavily, and a power-chord or planing context is not penalised at all.
Low-register spacing limits come from instrument-profile data, not from one
universal hardcoded number.

**Stage 8 — bass and countermelody.** Bass modes are `auto`, `roots`,
`inversions`, `stepwise`, `pedal`, `ostinato` and `contrary_motion`.
Countermelody generation considers complementary rhythm, register separation,
phrase gaps, motive relationship, contrary and oblique motion, rests and arrival
points; it is not note-for-note parallel motion unless that is what you asked
for.

**Stage 9 — arrangement.** `arrangement-engine` assigns roles from the
16-member `ArrangementRole` set, selects a pattern from the 28-entry catalogue
and an instrument from the 14 instrument profiles, allocates a register window,
realises the pattern's rational rhythm spec at the requested density, and
measures the result. Contrast is achieved through register, rhythm, density,
texture, harmony, articulation, silence and role substitution — never volume
alone. Masking avoidance uses register separation, onset-density control,
note-length control, role priority and reduced doubling. Every generated part
respects its instrument profile's range.

Four roles — `pulse`, `percussion`, `ornament` and `ear_candy` — have no pattern
of their own in the catalogue. Rather than fabricate a rhythm, the engine
borrows a catalogued pattern from a musically adjacent role along a fixed
preference chain and says so in the assignment's rationale. The substitution
table is `arrangement_engine::roles::ROLE_SUBSTITUTES`, and a test asserts that
exactly those four roles need it.

**Stage 10 — loop audit.** `loop-engine` evaluates the wrap against a declared
`LoopIntent`: final versus initial harmony, bass continuity, voice leading
across the boundary, unresolved tendencies, hanging notes, notes crossing the
boundary, pickup placement, tail requirements, harmonic rhythm at the wrap, and
layer removal that would eliminate an essential chord tone. Intent matters:
V-to-I across the wrap scores highly for `ClosedTonic` and is **not required**
for `ModalDrone`, which is served by common tones or a pedal. Loop length is
preserved by exact `BeatTime` equality, not float tolerance. Repairs are
suggested, never applied silently.

**Stage 11 — explanation.** `build_trace` assembles a `DecisionTrace` from the
structured decisions only: the candidate, analysis and snapshot ids, the
knowledge version, profile and seed, the assumptions, the confidence, the full
score vector, every rule application, the warnings, the resolved source ids, the
explanation and the rejected alternatives. It cites real rule ids and resolves
real source ids; if it cannot, that is a bug, not a prose problem.

**Stage 13 — diversity.** `diversify` measures similarity over root motion,
functional path, modal source, bass contour, chord family, extension family,
voicing family, harmonic rhythm, cadential behaviour and chromaticism, then
selects a spread. A three-candidate request returns genuinely distinct
strategies — functional, modal-common-tone, chromatic-bass-led — rather than the
same path with added ninths.

**Performance.** A 16-bar melody generates three candidates well under a second
in release mode. Everything is bounded: beam width, per-slot pool, total path
count, note count, recursion depth, file polling and log size.

---

## 7. REAPER IPC

The byte-level contract is normative and lives with the bridge documentation;
[`REAPER_BRIDGE.md`](REAPER_BRIDGE.md) is the prose companion and
`schemas/ipc-request.schema.json` / `schemas/ipc-result.schema.json` are the
machine-readable form. `reaper-ipc` is the Rust half.

**Transport: files only.** No socket, no port, no pipe. Protocol version
`qlabs-reaper-ipc/1`.

```
<ipc-dir>/
├── commands/     Rust writes here.   Bridge claims from here.
├── processing/   Bridge-owned. In-flight requests.
├── results/      Bridge writes here. Rust reads and deletes.
├── failed/       Bridge-owned. Quarantined requests, kept 24 h.
├── logs/         Bridge-owned. bridge.log[.1..3], rotated at 1 MiB.
├── heartbeat.json   Bridge-owned. Liveness and identity.
└── bridge.lock      Bridge-owned. Single-instance lock.
```

The Rust side **writes only inside `commands/`** and **deletes only inside
`results/`**. It never writes to, renames within or deletes from `processing/`,
`failed/` or `logs/`, and treats `heartbeat.json`, `bridge.lock` and
`config.json` as read-only.

**Atomicity** comes from same-directory temp-file-plus-rename on both sides. The
writer creates `<id>.tmp` beside its final name, flushes, closes and renames.
Neither side ever reads a `.tmp`. The bridge claims a command with a single
rename into `processing/`; if the rename fails, another instance won and it
moves on silently. A result file therefore appears atomically and completely: if
Rust can see `<id>.result.json`, its contents are final.

**Validation is a normative 20-step order** evaluated by the bridge, returning
on the first failure: size limit, JSON parse, object shape, protocol version,
optional bridge-version pin, instance token, request-id pattern, request-id
equals filename stem, replay guard, expiry with a 5-second clock-skew tolerance,
timestamp parsing, command presence, command allowlist, payload shape,
`expected_project` shape, REAPER version support, active project,
`expected_project` preconditions, and finally command-specific payload
validation.

**The command allowlist is exactly seven**: `ping`, `status`,
`inspect_selection`, `stage_candidate`, `commit_candidate`,
`discard_candidate`, `undo_last_generation`. There is deliberately no command
that evaluates Lua, runs an action id, reads or writes an arbitrary path, or
passes anything through to the REAPER API.

**The only caller-influenced value that reaches a filesystem path** is
`request_id`, which must match `^[A-Za-z0-9][A-Za-z0-9._-]*$`, be at most 128
bytes, contain no `..`, and equal the stem of the file it arrived in. The server
mints request ids from its own UUID generator and never lets a caller influence
them. A command file whose stem fails the pattern is quarantined without a
reply, so the client observes `IPC_TIMEOUT` rather than a parse error.

**Liveness.** The bridge is online if and only if `heartbeat.json` exists,
parses, has `status == "online"`, and its timestamp is within
`stale_after_seconds` (10). Otherwise the client reports `BRIDGE_OFFLINE`
*without writing a command file* — a dead bridge never accumulates a queue.

**Content hashes.** Six canonical FNV-1a-64 hashes over line-oriented canonical
strings with fixed field order and `%.6f` float formatting: `midi_hash` (whole
take, unfiltered), `note_selection_hash`, `tempo_map_hash`, `timesig_map_hash`,
`note_list_hash` (the filtered note array) and `snapshot_hash` (a `key=value`
block over the others plus source identity). `snapshot_hash` deliberately
excludes per-call values and the project state-change counter, which is what
makes it reproducible and therefore usable as a staleness token.

FNV-1a-64 is **not cryptographic**. It detects accidental change — a moved item,
an edited note, a tempo change. It is not collision-resistant against a
motivated adversary, and neither side may treat a hash match as an
authorisation. This is an accepted trade-off for a local, same-user, file-based
channel; see [`SECURITY.md`](SECURITY.md).

**Two wire forms, one translation boundary.** `music-domain` and the Lua bridge
disagree on three representations: the `Precondition` discriminator (`kind`
versus `type`), `BeatTime` (rational string versus number), and `tags` (object
versus array of pairs). `reaper_ipc::plan::plan_to_wire` /
`plan_from_wire` is the single place that translates, and it is tested against
the bridge's own generated fixtures. `EditPlan::to_json` remains the form served
by the `editplan://{id}` MCP resource, where rational strings are the better
representation. Sending `EditPlan::to_json` over IPC would be a bug.

---

## 8. Transaction lifecycle

Every write is a validated `EditPlan` generated by the server. Nothing else can
mutate the project.

```
   generate            stage                     listen             decide
      │                  │                          │                  │
      ▼                  ▼                          ▼                  ▼
  Candidate ──▶ EditPlan ──▶ preview tracks ──▶ (unmute, audition) ──▶ commit
   (in the       (7 pre-      muted, tagged                            discard
    session      conditions)  QLABS_STATUS=preview                     undo
    store)
```

**Plan construction.** A plan carries `plan_id`, `candidate_id`,
`transaction_id`, `base_snapshot_id`, `base_snapshot_hash`, `project_uuid`,
`knowledge_version`, an `undo_label`, its operations, its preconditions and its
expected outputs. The `undo_label` **must** begin with the exact prefix
`QLabs MCP: `; the bridge refuses to open an undo block with any other label,
and the undo tool refuses to undo any entry without it.

**Operations** are drawn from a seven-member allowlist:
`create_folder_track`, `create_track`, `create_midi_item`, `insert_notes`,
`set_track_mute`, `create_region`, `create_midi_send`. Temp ids must be unique
and **forward references are rejected** — an operation naming a temp id must
appear after the operation that declares it.

**Preconditions are how staleness is enforced.** The bridge evaluates the plan's
preconditions before opening the undo block, and every plan the server generates
carries the full set: `ProjectUuid`, `StateChangeCount`, `ItemGuidExists`,
`TakeGuidExists`, `MidiHash`, `TempoMapHash` and `ItemBounds`. Omitting any of
them would silently remove the product's central safety guarantee, so a
server-side test asserts that a generated plan carries all seven. Independently
of that list, the bridge always re-checks `plan.project_uuid` against the live
project and re-derives `base_snapshot_hash`.

Because the bridge re-derives the snapshot to check staleness, the
`stage_candidate` payload must **echo the scope** — the same `note_scope` and
`melody_extraction` the plan was generated against — or a spurious
`STALE_SNAPSHOT` results. The error's `details.rebuilt_with` distinguishes that
case from a genuine edit.

**Execution order**, from the brief and implemented by `transactions.lua`:

1. Validate the request and the whole plan.
2. Validate the source snapshot.
3. Begin a project-specific undo block.
4. Suppress UI refresh.
5. Execute the mutations.
6. Verify the outputs against `expected_outputs`.
7. Restore UI refresh.
8. End the undo block with the owned label.
9. Update the arrange view exactly once.
10. Return the transaction result.

**The entire plan is validated before `Undo_BeginBlock2` is called.** A plan that
fails validation performs zero mutations and creates zero undo entries. On a
mutation failure the bridge restores UI refresh, closes the block, verifies that
the newly created top undo entry is the failed owned transaction, undoes that
owned transaction when it is safe to do so, and returns a structured failure. It
never undoes an unrelated action, and `error.details.rolled_back` says whether
the partial transaction was reversed.

**Staging layout.** One level of folder nesting, per the brief:

```
QLabs Candidate 01 — Warm Extended
├── Chords
├── Bass
└── Countermelody
```

Items are aligned to the source musical range and exact QN loop length is
preserved. Preview tracks are muted by default. The source item is never
replaced, deleted or muted. No MIDI routing is created unless explicitly
requested, and a requested send is validated and identified in the result. No
plugin state is read, copied or manipulated.

**Commit** flips every object whose tags satisfy the ownership predicate *and*
whose `QLABS_STATUS` is `preview` to `committed`, sets `QLABS_COMMITTED_AT`, and
unmutes anything carrying `QLABS_PREVIEW_MUTED`. Nothing else in the project is
touched. Committing twice is a no-op. Commit means *retaining the generated
material as normal project content*, never merging destructively into the
source.

**Discard** deletes only objects carrying the matching ownership tags, items
first and then tracks. A tagged track that still holds items not belonging to
the transaction is **kept**, and a `track_retained` warning is returned.

**Undo** undoes at most one entry, and only when all of: the project ext state
holds a last-transaction record; the requested transaction id equals that
record's id; `Undo_CanUndo2` returns a non-empty string; that string equals the
record's `undo_label` exactly; and that string starts with `QLabs MCP: `.
Otherwise it returns `UNDO_NOT_OWNED` with the top undo entry named in the
details. **An unrelated REAPER undo entry is never undone.**

`commit`, `discard` and `undo` are **transaction-id scoped and do not re-check
the source snapshot.** They act only on objects carrying matching ownership
tags. This is correct — they never touch the source — but it is a real property
worth knowing, and it is recorded in
[`KNOWN_LIMITATIONS.md`](KNOWN_LIMITATIONS.md).

---

## 9. Data ownership

Who owns what, and what survives what.

| Data | Owner | Lifetime | Location |
|---|---|---|---|
| Your project and its MIDI | You / REAPER | Yours | Your `.rpp` and its media |
| Snapshot of a selection | MCP server session store | TTL'd, process lifetime | Memory only |
| `Analysis` | MCP server session store | TTL'd, process lifetime | Memory only |
| `Candidate`, `DecisionTrace` | MCP server session store | TTL'd, process lifetime | Memory only |
| `EditPlan`, transaction record | MCP server session store | TTL'd, process lifetime | Memory only |
| Staged tracks, items, takes | REAPER project, tagged | Until commit, discard or undo | Your project |
| Transaction records in the project | Bridge, project ext state | Capped at 32, newest last | Your `.rpp` |
| IPC command / result files | Bridge and server jointly | Seconds; GC'd at 300 s / 900 s | `<ipc-dir>` |
| Quarantined failed requests | Bridge | 24 hours | `<ipc-dir>/failed` |
| Bridge log | Bridge | 1 MiB × 4 generations | `<ipc-dir>/logs` |
| Installation token, `ipc_dir` | Installer, or the bridge on first run | Until reinstalled | `config.json` beside the ReaScript |
| Knowledge bundle | The binary | Compiled in | — |

**Ownership inside the project** is expressed with a closed allowlist of 14
`QLABS_*` tags written as REAPER extended object state under the `P_EXT:`
prefix. A plan supplying any other tag key is rejected as `INVALID_EDIT_PLAN`,
which is what stops the tag channel from becoming a general write primitive into
REAPER object state. Only `QLABS_ROLE` is ever supplied by a plan; the rest are
written by the bridge.

The **ownership predicate** — the only selector `commit_candidate` and
`discard_candidate` may use — is:

```
QLABS_OWNER == "QLabs-Reaper-MCP"  AND  QLABS_TRANSACTION_ID == <transaction_id>
```

**Track and item names are never used to identify objects.** Rename a staged
track to anything you like; the product still knows exactly what it owns, and
still refuses to touch anything it does not.

Nothing musical is persisted by the MCP server. Close the process and every
snapshot, analysis, candidate and plan is gone. What remains on disk is what
REAPER saved, the small IPC working files, and the bridge log.

---

## 10. Determinism

Determinism is a product requirement, not an optimization.

Given the same input, knowledge version, profile, parameters and seed, the
system produces **byte-identical** candidate ordering and candidate JSON. That
is what makes golden testing possible, what makes a decision trace meaningful,
and what makes a bug reproducible from a fixture.

How it is achieved:

- `qjson::JsonMap` is insertion-ordered and preserves order on serialization.
  Canonical JSON sorts keys explicitly for hashing.
- No `HashMap` iteration ever reaches output. Ordered output uses `BTreeMap` or
  insertion-ordered vectors throughout.
- `BeatTime` is an exact reduced rational, so time comparisons never depend on
  float tolerance ([D3](#d3--musical-time-is-an-exact-rational)).
- Randomness is a seeded, reproducible `DetRng` (SplitMix64-seeded
  xoshiro256\*\*) with label-derived sub-streams, so adding a new consumer never
  shifts an existing consumer's stream. It is used only for tie-breaking.
- Ids for analyses and other content-derived objects come from
  `uuid_from_name`, a stable v5-style derivation: same input, same id forever.
- The bridge orders notes by `(start_ppq, pitch, channel, end_ppq, velocity,
  original index)` — deterministic and independent of REAPER's internal event
  order — and re-sorts plan notes before insertion, so plan note order cannot
  affect the result.

---

## 11. Architecture decisions

These are the decisions taken during the build, with their context and
consequences.

### D1 — Zero external Rust dependencies

**Context.** The brief prefers `serde`, `serde_json`, `schemars`, `thiserror`,
`tracing`, `tokio`, `uuid`, `sha2`, `indexmap` and the official MCP Rust SDK.
The build environment's egress policy blocks `static.crates.io` (HTTP 403 on
every `.crate` download); the sparse index resolves but no package can be
fetched. Routing around an organization policy denial is not acceptable, and
vendoring hundreds of transitive crates through git mirrors is not a
maintainable substitute.

**Decision.** The workspace is built entirely on `std`, with zero external
dependencies and `unsafe_code = "forbid"` at the workspace level. The
capabilities the listed crates would have supplied are implemented in-repo in
the `qjson` crate: JSON value/parse/serialize, canonical JSON, a JSON Schema
2020-12 subset validator, a small regex engine, SHA-256, a deterministic PRNG,
UUID generation, and ISO-8601 time. Error types are hand-written
`std::error::Error` impls instead of `thiserror`. Logging is a small `stderr`
tracer instead of `tracing`. The MCP server is a synchronous `stdio` JSON-RPC
loop instead of `tokio` + `rmcp`.

**Consequences.**

- *Positive:* the product is trivially offline, has no supply chain, builds from
  a clean checkout with no network, and every byte of behaviour is inspectable
  and testable here. Determinism is easier to guarantee — no third-party
  iteration-order or float-formatting surprises.
- *Negative:* we own the correctness of the JSON, schema, hashing and JSON-RPC
  layers. This is mitigated by an unusually heavy unit-test burden on `qjson`
  (parser, schema, SHA-256 NIST vectors, RNG reproducibility) and by
  protocol-level tests on the MCP server.
- *Negative:* we are not using the official MCP SDK, so **MCP conformance is our
  responsibility** and is covered by the protocol test suite rather than
  inherited from the SDK.
- If a future build environment can reach crates.io, `qjson` is a narrow,
  well-tested seam that could be swapped for `serde_json` + `schemars` + `sha2`
  + `rand` without touching the music engine.

### D2 — MCP protocol version

Targeting **`2025-11-25`**, the stable specification named in the brief. The
brief asks that a newer stable version be checked for on
modelcontextprotocol.io; **that check could not be performed** because the
documentation host is not reachable from this build environment (HTTP 403
through the egress proxy). This is recorded honestly in the README,
`docs/MCP_API.md` and `CHANGELOG.md` rather than being claimed as verified.

If you are picking this project up with network access, verifying the current
stable specification version is the first thing worth doing.

### D3 — Musical time is an exact rational

`BeatTime` is a normalized rational number of quarter notes (`num/den`,
gcd-reduced). Loop boundary equality, bar-grid alignment and round-trip
provenance all require exact comparison; float PPQ cannot provide it. REAPER PPQ
and project-QN floats are retained on notes for provenance and round-trip
verification only. Floats coming from REAPER are snapped onto a 1/1920-quarter
grid, which represents 128th notes, triplets and quintuplets exactly.

### D4 — Synchronous server, threaded IPC waits

No async runtime. The MCP server reads newline-delimited JSON-RPC from stdin on
the main thread. Long operations report progress via notifications and check a
cancellation flag. The REAPER IPC client blocks on a bounded, backoff-polled
directory watch with an explicit timeout; the bridge itself is a `defer()` loop
inside REAPER.

### D5 — Extra crate `qjson`

The brief's crate layout is followed exactly, with one addition: `qjson`. It
exists because of D1 and nothing else. It carries no music-domain knowledge.

### D6 — Knowledge provenance under no-network conditions

Source licences and canonical locators could not be verified online during this
build. Rather than fabricate them, unverifiable fields are explicitly marked
`unverified-reference-only`, no source prose is reproduced anywhere, all encoded
rule text is original, and rules that are engineering judgement are labelled
`implementation_heuristic` with empty `source_refs` instead of being given a
plausible-looking citation.

All nine source records in `knowledge/sources.json` currently carry
`"license": "unverified-reference-only"`. The verification checklist for anyone
redistributing this bundle is in
[`RESEARCH_AND_PROVENANCE.md`](RESEARCH_AND_PROVENANCE.md).
