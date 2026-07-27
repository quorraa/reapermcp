# Testing

Every test group in this repository, what it covers, and how to run it.

Two things to know before reading further:

1. **The in-REAPER smoke test has now been executed**, on Windows 11 with
   REAPER 7.78/x64: **28 passed, 0 failed**, and the manual acceptance
   walkthrough has been run alongside it. See
   [§10](#10-the-in-reaper-smoke-test--executed). All sixteen steps have now
   been run against a real host.
2. **Everything else runs offline.** The workspace has zero external
   dependencies and the knowledge bundle is compiled in, so no test needs a
   network, a DAW, or any external package beyond `lua5.4` for the bridge suite.

---

## Contents

- [Quick start](#quick-start)
- [1. Rust unit tests](#1-rust-unit-tests)
- [2. Rust integration tests](#2-rust-integration-tests)
- [3. Golden fixture tests](#3-golden-fixture-tests)
- [4. Knowledge, schema and fixture validation](#4-knowledge-schema-and-fixture-validation)
- [5. The rule-to-test coverage ledger](#5-the-rule-to-test-coverage-ledger)
- [6. IPC protocol tests](#6-ipc-protocol-tests)
- [7. Cross-language mock-REAPER fixtures](#7-cross-language-mock-reaper-fixtures)
- [8. The REAPER Lua bridge suite](#8-the-reaper-lua-bridge-suite)
- [9. MCP protocol tests](#9-mcp-protocol-tests)
- [10. The in-REAPER smoke test — EXECUTED](#10-the-in-reaper-smoke-test--executed)
- [Regenerating generated test data](#regenerating-generated-test-data)
- [Continuous integration](#continuous-integration)
- [Writing a new test](#writing-a-new-test)

---

## Quick start

Everything, in one command, with a pass/fail summary:

```powershell
.\scripts\validate-release.ps1
```

Or, individually:

```sh
cargo fmt --all --check                              # formatting
cargo clippy --workspace --all-targets -- -D warnings # lints
cargo test --workspace                                # the Rust suite
cargo run -p xtask -- check-all                       # knowledge, schemas, fixtures
cd reaper && lua5.4 tests/run_tests.lua               # the REAPER bridge suite
```

> **The Lua interpreter is named `lua5.4` on Debian and Ubuntu**, which is what
> CI installs. Elsewhere it is usually plain `lua` — the official Windows build
> installs `lua.exe`, so on Windows run `lua tests/run_tests.lua`.
> `validate-release.ps1` tries both.

**Measured results**, from running these in this repository:

| Suite | Result |
|---|---|
| `cargo test --workspace` | **2241 passed, 0 failed**, across 50 test binaries and doc-test targets |
| `lua5.4 tests/run_tests.lua` | **184 passed, 0 failed, 0 skipped**, across 8 suites |
| In-REAPER smoke test | **28 passed, 0 failed** on REAPER 7.78/x64 (Windows 11) |

Per-crate Rust totals:

| Crate | Tests |
|---|---|
| `reaper-ipc` | 330 |
| `reaper-music-mcp` | 308 |
| `harmony-engine` | 299 |
| `music-analysis` | 275 |
| `music-domain` | 260 |
| `arrangement-engine` | 225 |
| `theory-kb` | 202 |
| `qjson` | 169 |
| `loop-engine` | 156 |
| `xtask` | 17 |
| **total** | **2241** |

(Counts include each crate's unit tests, its integration test binaries and its
doc-tests. `reaper-music-mcp`'s 308 are the MCP protocol tests described in
[§9](#9-mcp-protocol-tests); they are listed here too, because
`cargo test --workspace` runs them and the table has to add up to what that
command reports.)

---

## 1. Rust unit tests

Unit tests live in `#[cfg(test)] mod tests` at the bottom of each module,
next to the code they exercise.

```sh
cargo test --workspace --lib
cargo test -p music-domain --lib             # one crate
cargo test -p qjson --lib json::tests        # one module
cargo test -p harmony-engine voicing         # by name substring
```

What each crate's unit tests are for:

| Crate | Focus |
|---|---|
| `qjson` | Parser round-trips, unicode escapes and surrogate pairs, depth-limit rejection, canonical-form stability, **SHA-256 NIST vectors**, RNG reproducibility, the regex engine, every supported JSON Schema keyword. This crate carries an unusually heavy test burden on purpose — see decision D1. |
| `music-domain` | Pitch spelling and enharmonic distinction, interval arithmetic, `BeatTime` rational invariants and grid snapping, chord-symbol parse/render/parse stability, note and note-set invariants, JSON round-trips for every type. |
| `theory-kb` | Loading, every validation rejection, profile inheritance and override resolution, the predicate evaluator, deterministic search scoring. |
| `music-analysis` | Extraction modes, voice separation, phrase and salience components, key evidence sources, grid selection, NCT classification. |
| `harmony-engine` | Option pools, path search, voicing, voice-leading audit, bass, countermelody, reharmonization, diversity. |
| `arrangement-engine` | Role assignment, pattern realisation, density, masking, energy, section planning. |
| `loop-engine` | Boundary detection, intent fitting, wrap scoring, carry policies, repairs. |
| `reaper-ipc` | Envelopes, hashes, snapshot derivation, plan wire translation, heartbeat, client state machine. |

---

## 2. Rust integration tests

Per-crate integration tests live in each crate's `tests/` directory and exercise
the crate through its public API only.

```sh
cargo test --workspace --tests
cargo test -p harmony-engine --test harmony_and_function
```

| File | Covers |
|---|---|
| `crates/music-analysis/tests/pipeline.rs` | The full analysis pipeline end to end |
| `crates/music-analysis/tests/golden.rs` | Byte-compared golden analyses — see [§3](#3-golden-fixture-tests) |
| `crates/music-analysis/tests/rule_coverage.rs` | The analysis crate's slice of the coverage ledger |
| `crates/harmony-engine/tests/candidate_generation.rs` | Candidate generation, determinism, preserve-melody and preserve-rhythm |
| `crates/harmony-engine/tests/harmony_and_function.rs` | ii–V–I analysis, dominant-to-tonic, tendency-tone resolution |
| `crates/harmony-engine/tests/extensions.rs` | Extension and alteration semantics |
| `crates/harmony-engine/tests/voice_leading_cases.rs` | Parallels, hidden parallels, spacing, doubling, per-profile behaviour |
| `crates/harmony-engine/tests/invariants.rs` | Hard invariants that must never break |
| `crates/harmony-engine/tests/performance.rs` | Generation stays within its time budget |
| `crates/arrangement-engine/tests/catalogue.rs` | Every pattern and instrument profile is usable |
| `crates/arrangement-engine/tests/planning.rs` | Role assignment and plan construction |
| `crates/arrangement-engine/tests/named_behaviours.rs` | The named behaviours from the brief |
| `crates/arrangement-engine/tests/contract.rs` | The crate's public contract |
| `crates/arrangement-engine/tests/property.rs` | Property-style invariants over generated inputs |
| `crates/loop-engine/tests/boundary_policies.rs` | Split, carry, truncate, rearticulate |
| `crates/loop-engine/tests/looping_behaviours.rs` | Intent-specific wrap behaviour |
| `crates/loop-engine/tests/determinism_and_rules.rs` | Determinism and rule firing |
| `crates/loop-engine/tests/knowledge_test_ids.rs` | The loop crate's slice of the coverage ledger |
| `crates/theory-kb/tests/rule_engine.rs` | Rule evaluation against contexts |
| `crates/theory-kb/tests/validation_rejections.rs` | Every documented validation rejection actually rejects |
| `crates/theory-kb/tests/embedded_parity.rs` | The embedded bundle matches `knowledge/` on disk |
| `crates/theory-kb/tests/test_id_coverage.rs` | The coverage ledger itself — see [§5](#5-the-rule-to-test-coverage-ledger) |
| `crates/reaper-ipc/tests/brief_s28_ipc.rs` | The brief's IPC test list — see [§6](#6-ipc-protocol-tests) |
| `crates/reaper-ipc/tests/fixtures.rs` | The cross-language fixtures — see [§7](#7-cross-language-mock-reaper-fixtures) |
| `crates/reaper-ipc/tests/round_trip.rs` | Request/result round-trips |

> **Note on test layout.** The brief's suggested layout has a workspace-level
> `tests/` directory split into `golden/`, `integration/`, `property/` and
> `protocol/`. Every suite lives inside its owning crate instead, which keeps a
> failing test next to the code that broke and makes `cargo test -p <crate>` a
> complete check for that crate. The four categories are all present — they are
> named in the table above — just located per crate rather than centrally.

---

## 3. Golden fixture tests

The strongest regression net in the repository. `music-analysis` analyses each
fixture and compares the canonical JSON **byte for byte** against a committed
expected output.

```sh
cargo test -p music-analysis --test golden
```

The corpus is `fixtures/`, with **59 JSON files**:

| Directory | Files | What |
|---|---|---|
| `fixtures/melodies/` | 7 | Melody fixtures: 8- and 16-bar C major, a Dorian vamp, a blues head, a chromatic descent, waltz suspensions, two-voice polyphony |
| `fixtures/progressions/` | 4 | ii–V–I, a twelve-bar blues, a modal planing loop, a reharmonization source |
| `fixtures/loops/` | 2 | A dominant wrap, and a pickup with a hanging note |
| `fixtures/expected/` | 13 | The committed golden outputs |
| `fixtures/mock-reaper/` | 33 | Cross-language IPC fixtures — see [§7](#7-cross-language-mock-reaper-fixtures) |

Golden outputs are named after the fixture id with slashes replaced by dashes:
`melodies/eight_bar_c_major` becomes
`fixtures/expected/melodies-eight_bar_c_major.json`.

### Regenerating the goldens

A golden test failure means the analysis changed. **Read the diff before
regenerating** — that diff is the entire value of the test.

```sh
MUSIC_ANALYSIS_WRITE_GOLDENS=1 cargo test -p music-analysis --test golden
```

```powershell
$env:MUSIC_ANALYSIS_WRITE_GOLDENS = '1'
cargo test -p music-analysis --test golden
Remove-Item Env:\MUSIC_ANALYSIS_WRITE_GOLDENS
```

Then `git diff fixtures/expected/` and satisfy yourself that every change is one
you meant to make. Commit the fixtures with the code change that caused them.

The same environment variable creates a golden for a newly added fixture that
does not have one yet.

### Adding a fixture

Write the JSON (the format is documented in the fixture loader and validated by
`xtask validate-fixtures`), run `cargo run -p xtask -- validate-fixtures` to
check it loads, then generate its golden as above. Positions are **rational
strings** — `"0"`, `"3"`, `"1/2"`, `"7/3"` — never bare floats, and pitches are
spelled — `"C4"`, `"Bb3"`, `"F#5"`.

### Fixture-driven CLI

The server binary can run the real engine against a fixture with no REAPER
present, which is the fastest way to debug an analysis or generation problem:

```sh
qlabs-reaper-music-mcp analyze-fixture  fixtures/melodies/eight_bar_c_major.json
qlabs-reaper-music-mcp generate-fixture fixtures/melodies/eight_bar_c_major.json
```

---

## 4. Knowledge, schema and fixture validation

```sh
cargo run -p xtask -- check-all
```

Runs all of:

| Command | Checks |
|---|---|
| `validate-knowledge` | Full `theory-kb` validation of `knowledge/` on disk **and** of the embedded bundle. Prints counts. |
| `validate-schemas` | Compiles every `schemas/*.schema.json` and validates each knowledge and fixture file against the schema that governs it. |
| `validate-fixtures` | Loads every `fixtures/**/*.json` through the domain fixture loader. |
| `stamp-manifest --check` | Confirms `manifest.json`'s `content_sha256` matches the canonical JSON of every other knowledge file. |
| `regen-embedded --check` | Confirms `crates/theory-kb/src/embedded.rs` matches what is on disk. |

Individually:

```sh
cargo run -p xtask -- validate-knowledge
cargo run -p xtask -- validate-knowledge --knowledge-dir /path/to/override
cargo run -p xtask -- validate-schemas
cargo run -p xtask -- validate-fixtures
cargo run -p xtask -- stamp-manifest        # rewrite the hash after editing knowledge
cargo run -p xtask -- regen-embedded        # rewrite embedded.rs after adding a file
cargo run -p xtask -- check-all --write     # let check-all fix drift instead of reporting it
```

Every command takes `--json` for machine-readable output.

**After editing anything under `knowledge/`**, run `stamp-manifest` and
`regen-embedded`, then `check-all`. Skipping this is the most common way to
break the build: validation recomputes the content hash and never trusts the
manifest's stored value.

Validation rejects, and there is a test for each: schema violations, duplicate
ids, unresolved source references, profile-inheritance cycles, missing parents,
rules naming unknown profiles, scales, chord qualities or score components,
unknown predicates, unknown rule kinds, domains or events, `score_weights` not
covering exactly the 13 score components, `rule_overrides` naming unknown rules,
degree strings failing `^[b#]{0,2}\d+$`, empty `test_ids`, and a
`content_sha256` mismatch.

---

## 5. The rule-to-test coverage ledger

`crates/theory-kb/tests/test_id_coverage.json` records which of the
`test_ids` declared by the 147 knowledge rules actually have a behavioural test
behind them.

```sh
cargo test -p theory-kb --test test_id_coverage
```

**Current state:**

| | |
|---|---|
| Total declared test ids | **237** |
| Implemented | **205** (86.5%) |
| Pending | **32** |

By crate: `harmony-engine` 128, `music-analysis` 26, `arrangement-engine` 20, `theory-kb` 17, `loop-engine` 14, `reaper-music-mcp` 12. `implemented` is the **union** of those lists
— several ids are covered from more than one crate — and that union is exactly
205.

The ledger is enforced, not decorative: `implemented + pending` must equal every
`test_id` appearing anywhere in `knowledge/`. A rule cannot quietly lose its
coverage, and a pending id cannot quietly disappear.

**The 44 pending ids are declared, not hidden.** They are listed in the ledger's
`pending` array. They represent behaviours the knowledge bundle asserts and the
test suite does not yet verify — genuinely untested claims, and the honest place
to start if you are extending this work. See
[`KNOWN_LIMITATIONS.md`](KNOWN_LIMITATIONS.md).

### Regenerating the ledger

After adding a test that covers a pending id:

```sh
THEORY_KB_WRITE_COVERAGE=1 cargo test -p theory-kb --test test_id_coverage
```

```powershell
$env:THEORY_KB_WRITE_COVERAGE = '1'
cargo test -p theory-kb --test test_id_coverage
Remove-Item Env:\THEORY_KB_WRITE_COVERAGE
```

Each crate contributes its own list under `by_crate`, so a later crate adding
coverage never has to touch another crate's entry.

---

## 6. IPC protocol tests

```sh
cargo test -p reaper-ipc --test brief_s28_ipc
```

The filesystem interaction sits behind a small trait, so these run against a
temporary directory with no REAPER and no bridge. They cover the brief's IPC
list in full:

- A partial `.tmp` file is ignored.
- A command is claimed atomically; a lost race is silent.
- An invalid instance token is rejected.
- An expired request is rejected.
- An oversized request is rejected **before** it is written.
- An unknown command is rejected, with the allowlist in the details.
- A duplicate request id is rejected.
- Results appear atomically and completely.
- A timeout produces `IPC_TIMEOUT` and does not delete the command file.
- A stale snapshot is rejected.
- A result arriving after a timeout is read, discarded and deleted.
- The client never writes outside `commands/` and never deletes outside
  `results/`.

---

## 7. Cross-language mock-REAPER fixtures

`fixtures/mock-reaper/` holds 33 generated files that pin the Rust and Lua sides
to the same wire contract: hash vectors with their exact canonical strings,
request envelopes valid and invalid, edit plans valid and invalid, result
envelopes, heartbeat and lock shapes, and the published limits and error codes.

They are **generated, never hand-edited**:

```sh
lua5.4 reaper/tests/gen_fixtures.lua
```

Both sides verify them:

```sh
cd reaper && lua5.4 tests/run_tests.lua      # test_fixtures.lua re-derives them all
cargo test -p reaper-ipc --test fixtures     # the Rust client consumes them
```

`reaper/tests/test_fixtures.lua` re-derives every file from the live Lua
implementation on each run, so the fixtures cannot silently drift from the
bridge. If that suite fails, either the bridge changed behaviour — regenerate,
and tell the Rust side — or a regression was introduced.

The most important file is `fixtures/mock-reaper/hashes/vectors.json`. Each
entry pairs an exact canonical string with the hash the bridge produces for it.
The three `fnv1a64.*` entries are plain algorithm vectors (`""`, `"a"`,
`"foobar"`); the rest exercise the real canonical layouts for `midi_hash`,
`note_selection_hash`, `tempo_map_hash`, `timesig_map_hash`, `note_list_hash`
and `snapshot_hash`. A Rust implementation must reproduce every one byte for
byte.

---

## 8. The REAPER Lua bridge suite

```sh
cd reaper
lua5.4 tests/run_tests.lua     # Debian/Ubuntu
lua tests/run_tests.lua        # Windows, macOS, and anywhere else
```

**184 cases across 8 suites, 0 failures.** No REAPER and no external Lua package
required. Measured on Lua 5.4.6.

| Suite | Cases | Covers |
|---|---|---|
| `test_bridge` | 44 | The engine: lock, heartbeat, the IPC state machine, dispatch, garbage collection, log rotation, error containment |
| `test_transactions` | 38 | Edit-plan validation and execution, undo blocks, rollback, ownership-scoped commit and discard |
| `test_snapshot` | 27 | Source resolution, MIDI reading, canonical hashing, extraction filters |
| `test_json` | 19 | The pure-Lua JSON encoder and decoder |
| `test_protocol` | 18 | Envelope validation order, the command allowlist, limits, error codes |
| `test_util` | 16 | Paths, time, FNV-1a-64, the swappable filesystem, bounded logging |
| `test_tagging` | 12 | `P_EXT` ownership tags, project ext state, project UUID minting |
| `test_fixtures` | 10 | Regeneration parity with `fixtures/mock-reaper/` |

Two mocks make this possible:

- **`mock_reaper.lua`** implements every `reaper.*` function the bridge calls,
  including a real undo journal — mutations record inverse closures and
  `Undo_DoUndo2` replays them in reverse, preserving pointer identity and
  resurrecting deleted objects — and a `PreventUIRefresh` depth counter.
- **`mock_fs.lua`** is an in-memory filesystem installed as `util.fs`, so the
  atomic-IPC state machine runs through the real code path rather than a
  simulation of it.

One suite asserts that the mock host exposes every `reaper.*` function the
bridge calls, so the mock cannot fall behind the code.

**Installing Lua on Windows** is not required to use the product. If you want to
run this suite:

```powershell
winget install --id DEVCOM.Lua --exact
```

That installs Lua 5.4.6 to `%LOCALAPPDATA%\Programs\Lua\bin` and puts it on
`PATH` as **`lua`** — there is no `lua5.4` executable on Windows, so use
`lua tests/run_tests.lua`. `validate-release.ps1` tries `lua5.4` first, falls
back to `lua`, and reports a clear **SKIP**, not a pass, when neither is found.

---

## 9. MCP protocol tests

The MCP server's own tests live in `crates/reaper-music-mcp`:

```sh
cargo test -p reaper-music-mcp
```

They cover the JSON-RPC layer and the protocol surface: `initialize` and the
reported protocol version, tool and resource and prompt listing, tool argument
validation against each declared `inputSchema`, tool output validation against
each declared `outputSchema`, structured tool errors versus transport errors,
resource ownership and TTL enforcement, cancellation, progress notifications,
and the invariant that `stdout` carries protocol bytes only.

Because this build does not use the official MCP SDK (decision D1), **MCP
conformance is verified here rather than inherited**. That makes this suite
load-bearing in a way it would not otherwise be.

> When this document was first written, `crates/reaper-music-mcp` was still
> being implemented and contributed **0** tests, which is why the total then
> read 1926. It now contributes **308** across 7 binaries, and the counts in
> [Quick start](#quick-start) have been re-measured to include them.
> `docs/MCP_API.md` documents the surface these tests exercise.

---

## 10. The in-REAPER smoke test — EXECUTED

```text
reaper/QLabs_Reaper_MCP_Smoke_Test.lua
```

### Status: EXECUTED — 28 passed, 0 failed

| | |
|---|---|
| Host | REAPER **7.78/x64**, Windows 11 Pro (26200) |
| Bridge | 1.0.0, IPC protocol `qlabs-reaper-ipc/1` |
| Result | **28 passed, 0 failed** |
| Script | run **unmodified**, in a fresh project tab, `ALLOW_MODIFY_THIS_PROJECT` left `false` |

Every group in the script passed: setup, inspection, staging, source integrity,
ownership tags, undo and cleanup. The checks that matter most for the safety
claims all held against the real host —

- the source take still held its 4 notes afterwards, with an **unchanged MIDI
  hash** and unchanged item bounds;
- the source item carried **no** QLabs ownership tag;
- every generated track, item and take carried matching tags, and every required
  `P_EXT` tag was present;
- the top undo entry was the owned transaction, and **one** undo removed every
  generated track while the source survived.

The 184 mock-host cases in [§8](#8-the-reaper-lua-bridge-suite) remain the
detailed coverage; this run is what establishes that the model and the product
agree. The REAPER-facing behaviour is no longer verified only against a model.

### How to run it

It is deliberately guarded, because it writes to the active project.

1. **File → New Project.** The script refuses to run against a project that has
   been saved to disk or that has unsaved changes.
2. Register it exactly like the bridge:
   *Actions → Show action list → New action → Load ReaScript →*
   `QLabs_Reaper_MCP_Smoke_Test.lua`.
3. Run it. Results print to the ReaScript console, one PASS/FAIL line per step.

To run it against a project that is saved or dirty, open the script and set
`ALLOW_MODIFY_THIS_PROJECT = true` near the top. Only do that if you understand
that the script will create and delete tracks and items in the open project.

### What it verifies

- It creates its own clearly marked material (`QLABS SMOKE TEST -- SAFE TO
  DELETE`).
- Inspection resolves the source and produces a snapshot.
- A fixture candidate stages cleanly.
- **The source item is unchanged afterwards.**
- The ownership tags exist on the staged objects.
- One undo restores the prior state.
- It cleans up after itself.

### The manual acceptance walkthrough

The smoke test is automated-ish but narrow. The full acceptance workflow from
the brief is a manual sequence. It has now been performed in full against REAPER
7.78/x64, driving the real `serve` binary over stdio. Steps are marked with what
actually happened, including where a result was a considered "no".

1. **[run]** Open REAPER.
2. **[run]** Run `QLabs_Reaper_MCP_Bridge.lua`, registered through
   *Actions → Show action list → New action → Load ReaScript* exactly as
   [§how to run it](#how-to-run-it) describes. REAPER wrote the registration to
   `reaper-kb.ini` — a file that did not previously exist, so nothing had ever
   been registered — and running the action brought the bridge online with a
   fresh `pid_token` and `uptime_seconds` reset to 0. `doctor` then reported
   14/14, and a full `status` -> `inspect` -> `analyze` -> `generate` -> `stage`
   -> `discard` round trip ran against it.

   Two things this path shows that a command-line launch does not. Running the
   action while an instance is already live raises REAPER's *ReaScript task
   control* dialog (terminate / new instance / cancel) rather than starting a
   second bridge. And the toolbar toggle only works here: the script reads its
   command id from `reaper.get_action_context()`, which is `0` for a
   command-line launch, so `set_toggle` is a no-op and a CLI-started bridge
   never lights its toolbar button.
3. **[run]** Select one MIDI melody item, or notes in the MIDI editor.
4. **[run]** The MCP client calls `reaper.status` — answered `bridge_connected`,
   bridge 1.0.0, REAPER 7.78/x64.
5. **[run]** The MCP client calls `reaper.inspect_selection`.
6. **[run]** The MCP client calls `music.analyze_selection`.
7. **[run]** The request was taken from the server's own
   `harmonize-selected-melody` prompt rather than invented — `prompts/get`
   returns the workflow as prose, naming each tool to call in order — and
   followed as an MCP host would, with the brief's countermelody and loop
   clauses included: `preserve_melody` and `preserve_rhythm` true,
   `countermelody: {enabled: true, density: 0.3, role: "counterlead"}` and
   `loop_intent: "closed_tonic"`.
8. **[run]** Three genuinely different candidates — `chromatic_bass_led`,
   `functional` and `modal_common_tone`, three distinct progressions — each
   carrying all 13 score components, up to 30 applied rule ids and 3 source ids.
   Every candidate carried four parts including the requested
   `bass:Bass` and `counterlead:Countermelody`, and staging produced a fifth
   track for the countermelody. Melody preservation was checked against the
   source rather than trusted: a staged item carried exactly the source's 54
   notes.

   `loop.audit` did real work rather than rubber-stamping, returning
   `compatible: false` at confidence 0.955 with two findings — *"the chordal
   seventh of Dm9 does not fall by step across the wrap"* (minor) and *"for
   closed_tonic intent, the wrap does not resolve to a tonic"* (moderate) — with
   `harmonic_wrap` reading *"Dm9 (tonic) to D9 (applied): connected by common
   tone rather than by function"*, and a proposed repair,
   `add_a_turnaround_before_the_wrap`. That verdict is correct: the candidate
   ends on Dm9 and opens on an applied D9, so the loop does not close as the
   intent demands.

   Candidate ordering, scores and chord symbols were **byte-identical** to the
   same music run through `generate-fixture` offline, on two different pieces —
   the REAPER path and the pure-engine path agree.
9. **[run]** Select one candidate.
10. **[run]** `reaper.stage_candidate` created a folder plus tagged
    melody/harmony/bass tracks and MIDI items.
11. **[run]** The original MIDI item was unchanged.
12. **[run]** A note was inserted into the source take, then the already-issued
    candidate was staged again. Refused with `PROJECT_CHANGED`: *"plan
    precondition state_change_count 25 does not match live 26"*. Note that
    `state_change_count` is the strictest of the seven and trips first, so this
    exercises the precondition machinery but does **not** isolate `midi_hash` —
    no edit can change the MIDI without also incrementing the counter.
13. **[run]** `reaper.discard_candidate` removed only that candidate's objects
    (3 items, 4 tracks, 0 retained).
14. **[run]** `reaper.commit_candidate` kept all 4 tracks and 3 items, flipped
    `status` from `preview` to `committed`, unmuted the preview-muted items
    (`B_MUTE` 1.0 -> 0.0) and cleared the `QLABS_PREVIEW_MUTED` marker tag.
15. **[run]** `undo_last_generation` through the MCP tool removed all 4 staged
    tracks in one undo, leaving nothing tagged behind.
16. **[run]** After staging, an unrelated user action (inserting a track inside
    its own undo block) was performed. `undo_last_generation` refused with
    `UNDO_NOT_OWNED`: *"the top undo entry is 'Acceptance: an unrelated user
    action', which was not created by this MCP"*. The unrelated entry was not
    undone.

> **A bug this walkthrough uncovered, since fixed.** On the first run, steps 12
> and 14–16 each needed a *distinct* `seed`, because staging succeeded only once
> per (music, profile, seed) per server process. `harmony.generate_candidates`
> caches by music, profile and seed; on a cache hit it echoed back the
> `snapshot_id` the caller passed but returned the candidate built against the
> *original* snapshot. `reaper.stage_candidate` takes only a `candidate_id`, so
> it derived the plan's preconditions from that original snapshot — and since
> staging itself increments `state_change_count`, every later attempt failed with
> `PROJECT_CHANGED`. The error's own remedy ("call `reaper.inspect_selection`
> again and regenerate") could not break the loop, because regenerating returned
> the same stale candidate.
>
> A cache hit now re-binds the cached candidates to the snapshot the caller
> actually holds, which is sound because the hit proves the snapshot hashes are
> equal. The steps above were re-run sharing a single seed, which the old build
> could not survive, and step 12 still rejects an edited source.

---

## Regenerating generated test data

Four things in this repository are generated and committed. Each has a check
that fails if it drifts.

| Artefact | Regenerate with | Checked by |
|---|---|---|
| `fixtures/expected/*.json` | `MUSIC_ANALYSIS_WRITE_GOLDENS=1 cargo test -p music-analysis --test golden` | the golden test itself |
| `fixtures/mock-reaper/**` | `lua5.4 reaper/tests/gen_fixtures.lua` | `reaper/tests/test_fixtures.lua`, and `cargo test -p reaper-ipc --test fixtures` |
| `crates/theory-kb/tests/test_id_coverage.json` | `THEORY_KB_WRITE_COVERAGE=1 cargo test -p theory-kb --test test_id_coverage` | `test_id_coverage.rs` |
| `crates/theory-kb/src/embedded.rs` | `cargo run -p xtask -- regen-embedded` | `embedded_parity.rs`, and `xtask regen-embedded --check` |
| `knowledge/manifest.json` `content_sha256` | `cargo run -p xtask -- stamp-manifest` | `xtask stamp-manifest --check`, and knowledge validation |

Regenerating is never a fix on its own. It is what you do *after* you have read
the diff and decided the change is correct.

---

## Continuous integration

`.github/workflows/ci.yml` runs two jobs on every push and pull request.

**Rust**, on `ubuntu-latest` and `windows-latest`, with `RUSTFLAGS: -D warnings`
and `--offline` on every cargo invocation:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --offline -- -D warnings
cargo test --workspace --all-features --offline
cargo run -p xtask --offline -- validate-knowledge
cargo run -p xtask --offline -- validate-schemas
cargo run -p xtask --offline -- validate-fixtures
cargo build --workspace --release --offline
cargo run -p reaper-music-mcp --release --offline -- doctor --json
```

`--offline` is not a workaround; it is an assertion. The build is expected to
have no network dependency at all, and CI fails if that ever stops being true.

**REAPER Lua bridge**, on `ubuntu-latest`:

```
sudo apt-get install -y lua5.4
cd reaper && lua5.4 tests/run_tests.lua
```

CI cannot run the in-REAPER smoke test, and does not pretend to.

---

## Writing a new test

- **Unit tests** go in `#[cfg(test)] mod tests` at the bottom of the module.
- **Integration tests** go in the owning crate's `tests/` directory and use only
  the public API.
- **Name the test after the behaviour**, not the function:
  `sus4_resolves_to_third`, not `test_voicing_3`.
- **If the behaviour is asserted by a knowledge rule**, use the rule's declared
  `test_id` as the test name, then move it from `pending` to your crate's list
  in the coverage ledger and regenerate.
- **Determinism is testable.** If your change involves ordering, add an
  assertion that the same seed and inputs produce the same output.
- **Do not add a fixture without a golden**, and do not add a golden without
  reading it.
- Run `cargo fmt` and `cargo clippy -p <your-crate> --all-targets -- -D warnings`
  before you call it done.
