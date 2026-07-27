# QLabs REAPER Music Intelligence MCP

A local **Model Context Protocol** server that gives an AI assistant real
music-theory competence inside **REAPER**, and a safe, non-destructive way to
act on it.

You select a MIDI melody in REAPER and ask, in plain language, for
harmonizations. The server analyses the actual notes, reasons over an executable
music-theory knowledge bundle, produces several genuinely different candidates,
explains each one with the rules and sources it applied, and stages the winner
onto **new, tagged, muted preview tracks**. Your original MIDI item is never
touched.

- **Local only.** No network listener, no telemetry, no upload. The MCP server
  speaks JSON-RPC over `stdio`; the REAPER half is reached through an atomic
  file-based IPC directory on the same machine.
- **Non-destructive by default.** Everything arrives as a preview. Nothing is
  kept until you explicitly commit, and nothing is removed except objects
  carrying this product's own ownership tags.
- **Explainable.** Every candidate carries a decision trace: score components,
  the theory rules that fired, the rule ids, and the source ids behind them.
- **Deterministic.** Same snapshot, knowledge version, style profile, parameters
  and seed produce byte-identical candidate ordering.
- **Offline.** The Rust workspace has **zero external crates** and the entire
  knowledge bundle is compiled into the binary.

Version **1.0.0**. MIT licensed. Windows 11 + REAPER 7.x is the primary
supported platform; the Rust workspace itself is platform-neutral and its CI
runs on Linux and Windows.

---

## Contents

- [Architecture](#architecture)
- [Features](#features)
- [Scope](#scope)
- [Zero external dependencies](#zero-external-dependencies)
- [MCP protocol version](#mcp-protocol-version)
- [Build](#build)
- [Install](#install)
- [Starting the REAPER bridge](#starting-the-reaper-bridge)
- [MCP host configuration](#mcp-host-configuration)
- [First-use workflow](#first-use-workflow)
- [Example requests](#example-requests)
- [Safety behaviour](#safety-behaviour)
- [Troubleshooting](#troubleshooting)
- [Known limitations](#known-limitations)
- [Documentation map](#documentation-map)

---

## Architecture

```
        ┌───────────────────────────────────────────────────────────────┐
        │  MCP host  (Claude Desktop, an IDE agent, any MCP client)     │
        └───────────────────────────────┬───────────────────────────────┘
                                        │  JSON-RPC 2.0, newline-delimited
                                        │  over stdin / stdout  (stdio)
        ┌───────────────────────────────▼───────────────────────────────┐
        │  qlabs-reaper-music-mcp        (Rust, synchronous, no async)  │
        │                                                               │
        │   ┌──────────────┐  ┌────────────────┐  ┌──────────────────┐  │
        │   │ tools /      │  │ session store  │  │ doctor / CLI     │  │
        │   │ resources /  │  │ snapshots      │  │ fixture commands │  │
        │   │ prompts      │  │ analyses       │  └──────────────────┘  │
        │   └──────┬───────┘  │ candidates     │                        │
        │          │          │ edit plans     │                        │
        │          │          │ transactions   │                        │
        │          │          └────────────────┘                        │
        │  ┌───────▼─────────────────────────────────────────────────┐  │
        │  │            the music engine (pure, REAPER-free)         │  │
        │  │                                                         │  │
        │  │  music-analysis ─▶ harmony-engine ─▶ arrangement-engine │  │
        │  │  extraction         candidate pool     roles, patterns  │  │
        │  │  phrase/salience    beam path search   density, masking │  │
        │  │  key / mode         voicing + VL       energy curve     │  │
        │  │  harmonic grid      bass, counter-     ┌──────────────┐ │  │
        │  │  NCT classify       melody, reharm     │ loop-engine  │ │  │
        │  │        │                  │            │ wrap audit   │ │  │
        │  │        └──────────┬───────┴────────────┴──────────────┘ │  │
        │  │                   ▼                                     │  │
        │  │        theory-kb  (rule engine + resolved profiles)     │  │
        │  │        music-domain (BeatTime, SpelledPitch, ChordSpec) │  │
        │  │        qjson  (JSON, schema, SHA-256, RNG, UUID, time)  │  │
        │  └─────────────────────────────────────────────────────────┘  │
        │                              │                                │
        │        knowledge/  ──────────┘  compiled in via include_str!  │
        │        147 rules · 10 profiles · 35 scales · 51 qualities     │
        └───────────────────────────────┬───────────────────────────────┘
                                        │  reaper-ipc
                                        │  atomic file IPC, no socket
        ┌───────────────────────────────▼───────────────────────────────┐
        │  <ipc-dir>/                                                   │
        │    commands/   Rust writes  ·  results/   Rust reads          │
        │    processing/ failed/ logs/  ·  heartbeat.json  bridge.lock  │
        └───────────────────────────────┬───────────────────────────────┘
                                        │  polled by reaper.defer()
        ┌───────────────────────────────▼───────────────────────────────┐
        │  QLabs_Reaper_MCP_Bridge.lua      (pure Lua 5.4, in REAPER)   │
        │    envelope validation · snapshot + hashing · edit plans      │
        │    ownership tags · undo blocks · single-instance lock        │
        └───────────────────────────────┬───────────────────────────────┘
                                        │  official ReaScript API only
        ┌───────────────────────────────▼───────────────────────────────┐
        │  REAPER 7.x — your project                                    │
        │    source MIDI item  (read only, never written)               │
        │    QLabs preview tracks  (created, tagged, muted, revertible) │
        └───────────────────────────────────────────────────────────────┘
```

The trust and blast-radius story falls straight out of that picture: the MCP
server never touches your project directly, the bridge accepts only seven
allowlisted commands, and there is no command that evaluates Lua, runs a REAPER
action id, or names an arbitrary filesystem path. See
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and
[`docs/SECURITY.md`](docs/SECURITY.md).

### Crates

| Crate | What it is |
|---|---|
| `qjson` | JSON value/parser/serializer, canonical JSON, JSON Schema 2020-12 subset, a small regex engine, SHA-256, deterministic RNG, UUIDs, ISO-8601. Exists only because of the zero-dependency constraint. |
| `music-domain` | The shared musical vocabulary: `BeatTime`, `SpelledPitch`, `Interval`, `ChordSpec`, `Note`, structures, candidates, edit plans. See [`docs/MUSIC_IR.md`](docs/MUSIC_IR.md). |
| `theory-kb` | Loads, validates and executes `knowledge/`. Rule engine, predicate vocabulary, profile resolution, theory search. |
| `music-analysis` | Selection normalization, voice separation, phrase and salience, key/mode candidates, harmonic grid, NCT classification, chord detection. |
| `harmony-engine` | Chord-option pools, global beam path search, voicing, voice-leading audit, bass, countermelody, reharmonization, diversity, decision traces. |
| `arrangement-engine` | Arrangement roles, catalogue patterns, density, register/masking, energy curve, section planning. |
| `loop-engine` | Loop-boundary analysis, intent-aware wrap scoring, repairs, carry policies. |
| `reaper-ipc` | The Rust half of the file IPC: envelopes, hashes, snapshots, edit-plan wire translation, heartbeat, timeouts. |
| `reaper-music-mcp` | The `qlabs-reaper-music-mcp` binary: the MCP server and the CLI. |
| `xtask` | Knowledge, schema and fixture validation gates. |

---

## Features

**Analysis**

- Melody extraction from monophonic or polyphonic material, with the extraction
  mode, its assumptions and its confidence always reported — never a silent
  guess that polyphony is a melody.
- Deterministic voice separation.
- Phrase, subphrase, pickup, gap and motive analysis, including motive
  transforms (transposition, sequence, inversion, retrograde, augmentation,
  diminution).
- Structural salience as a transparent weighted sum whose components are visible
  per note.
- Ranked key and mode candidates with per-source evidence, plus tonicization and
  key regions. Never one forced major/minor label.
- Harmonic-grid selection from phrase structure, salience and the profile's
  harmonic-rhythm preference — not one chord per note.
- Non-chord-tone classification from surrounding motion, metric position and
  duration, with multiple competing hypotheses where a note is genuinely
  ambiguous.
- Chord detection from polyphonic material.

**Generation**

- Multiple ranked harmonization candidates from a global beam search over whole
  phrases, not greedy per-slot selection.
- Reharmonization with independent preserve switches for melody, bass, cadence
  and harmonic rhythm.
- Chord voicings across 13 voicing families, assigned as connected voices across
  time rather than as unordered pitch sets per chord.
- Voice-leading audit: parallels, hidden parallels, crossings, overlaps,
  unresolved tendency tones, spacing and doubling — all profile-driven, so
  strict counterpoint and a power-chord idiom are judged by different standards.
- Bass generation (roots, inversions, stepwise, pedal, ostinato, contrary).
- Restrained countermelody generation that considers complementary rhythm,
  register separation, phrase gaps, motive relationship and rests.
- Arrangement-role generation over 16 roles with a 32-pattern catalogue,
  14 instrument profiles, density and register control, masking analysis and an
  energy curve.
- Loop auditing against a declared loop intent, with repair suggestions.
- Candidate diversity that measures similarity across root motion, functional
  path, modal source, bass contour, chord and extension family, voicing family,
  harmonic rhythm, cadential behaviour and chromaticism.

**Knowledge** (verified counts from `knowledge/manifest.json`, knowledge version
`1.0.0`)

| Item | Count |
|---|---|
| Theory rules | **147** — harmony 37, voice leading 31, extensions 20, melody 17, arrangement 15, looping 14, counterpoint 13 |
| Style profiles | **10** |
| Scales | **35** |
| Chord qualities | **51** |
| Chord-symbol aliases | **65** |
| Voicing templates | **38** |
| Progression schemas | **37** progressions + **14** cadences |
| Arrangement patterns | **32** |
| Instrument profiles | **14** |
| Modes | **22** |
| Intervals | **28** |
| Function entries | **45** |
| Sources | **9** |
| Rule predicates | **116**, a closed vocabulary the engine must implement in full |

The ten style profiles are `common_practice`, `strict_counterpoint`,
`jazz_standard`, `blues`, `pop_rock`, `neo_soul_rnb`, `modal_ambient`,
`cinematic`, `electronic_loop`, `drum_and_bass`.

**Integration**

- Non-destructive staging into a tagged folder of muted preview tracks.
- Explicit `commit`, ownership-scoped `discard`, ownership-scoped `undo`.
- Stale-snapshot rejection: if you edit the source after generating, staging is
  refused rather than applied to material that no longer matches.
- An in-REAPER status script and a guarded in-REAPER smoke test.

---

## Scope

**Version one includes:** REAPER 7.x only; Windows 11 as the acceptance
platform; a Rust MCP server over local `stdio`; a pure-Lua REAPER bridge; atomic
local file IPC; MIDI analysis and generation; one active or selected MIDI source
at a time; selected notes when notes are selected and the whole active take when
they are not; melody extraction; key/mode candidates; phrase, motif and
structural-note analysis; non-chord-tone classification; chord parsing and
construction; extensions and alterations; reharmonization; voice-leading
optimization; voicing generation; bass lines; restrained countermelody;
arrangement roles; loop-boundary analysis; multiple ranked candidates;
explanations with provenance; non-destructive preview tracks; commit, discard
and owned undo; and offline operation after installation.

**Explicitly deferred, and not present in any form:** FL Studio or any other
DAW; audio-to-MIDI transcription; audio chord recognition; stem separation;
real-time note generation during playback; live accompaniment; mixing or
mastering; automatic plugin installation; automatic virtual-instrument
selection; preset browsing; cloud inference; remote project control; a native
C++ REAPER extension; microtonal generation beyond retaining a
future-compatible cents field; music-notation engraving; and orchestration
sample-library management.

---

## Zero external dependencies

**The Rust workspace depends on no third-party crates at all.** `Cargo.lock`
contains only this workspace's own members. `unsafe_code = "forbid"` is set at
the workspace level. There is no async runtime, no `serde`, no `tokio`, no
`uuid`, no `sha2`, and no MCP SDK.

That was not the original plan. The brief asked for `serde`, `tokio`, the
official MCP Rust SDK and friends. The build environment's egress policy blocks
`static.crates.io` — the sparse index resolves, but every `.crate` download
returns HTTP 403. Routing around an organization's policy denial is not an
acceptable engineering move, and vendoring hundreds of transitive crates through
mirrors is not a maintainable substitute. So the capabilities those crates would
have provided are implemented in-repo, in `qjson`.

**What you get from that:** the product is trivially offline, has no supply
chain, builds from a clean checkout with no network, and every byte of its
behaviour is inspectable and testable here. Determinism gets easier, because
there are no third-party iteration-order or float-formatting surprises.

**What it costs:** correctness of the JSON, schema, hashing and JSON-RPC layers
is ours. That is mitigated by an unusually heavy unit-test burden on `qjson`
(parser round-trips, unicode and surrogate handling, depth limits, canonical
form stability, SHA-256 NIST vectors, RNG reproducibility, regex and schema
keyword cases) and by protocol-level tests on the MCP server. **We are not using
the official MCP SDK, so MCP conformance is our responsibility** and is covered
by our own protocol test suite rather than inherited.

If a future build environment can reach crates.io, `qjson` is a narrow,
well-tested seam that could be swapped for `serde_json` + `schemars` + `sha2` +
`rand` without touching the music engine.

This is recorded as decision **D1** in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md#d1--zero-external-rust-dependencies).

## MCP protocol version

The server targets MCP specification version **`2025-11-25`**, the stable
specification named in the product brief, and reports that string from
`initialize`.

**Caveat, stated plainly.** The brief asked that modelcontextprotocol.io be
checked for a newer stable specification before implementation. **That check
could not be performed**: the documentation host is not reachable from this
build environment (HTTP 403 through the egress proxy). `2025-11-25` is therefore
the *selected* version, not a *verified-latest* version. If a newer stable
specification has since been published, this build does not know about it.
Recorded as decision **D2**.

The IPC protocol between the Rust server and the Lua bridge is versioned
separately as **`qlabs-reaper-ipc/1`**; the bridge is version **`1.0.0`**.

---

## Build

Requirements: a stable Rust toolchain (edition 2021, `rust-version` 1.85) and,
if you want to run the bridge test suite, Lua 5.4. Nothing else. No network.

```sh
cargo build --workspace --release
```

The binary lands at `target/release/qlabs-reaper-music-mcp`
(`qlabs-reaper-music-mcp.exe` on Windows).

Quality gates, all of which run offline:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p xtask -- check-all          # knowledge, schemas, fixtures
cd reaper && lua tests/run_tests.lua     # 184 bridge cases, mock REAPER host
```

The Lua interpreter is `lua5.4` on Debian and Ubuntu, which is what CI uses,
and plain `lua` everywhere else including the official Windows build
(`winget install --id DEVCOM.Lua`). `scripts\validate-release.ps1` runs every
gate above and tries both names.

See [`docs/TESTING.md`](docs/TESTING.md) for every test group, the golden-fixture
regeneration commands, and the coverage ledger.

---

## Install

On Windows, from an elevated-not-required PowerShell:

```powershell
.\scripts\install.ps1
```

It detects your REAPER resource path, copies the release binary, the knowledge
bundle and the Lua bridge, mints a cryptographically random installation token,
writes `config.json`, backs up any QLabs config it replaces, prints the exact
next steps and the MCP host snippet, and runs `doctor`.

Useful switches:

```powershell
.\scripts\install.ps1 -ReaperResourcePath 'D:\REAPER-Portable'   # portable install
.\scripts\install.ps1 -Destination 'C:\Tools\QLabs'              # binary elsewhere
.\scripts\install.ps1 -WhatIf                                    # dry run
```

To remove it again:

```powershell
.\scripts\uninstall.ps1                       # keeps knowledge overrides
.\scripts\uninstall.ps1 -RemoveKnowledgeOverrides -RemoveIpcDirectory
```

Full walkthrough with screenshots-worth-of-detail:
[`docs/INSTALL_WINDOWS.md`](docs/INSTALL_WINDOWS.md).

There is no installer for macOS or Linux. The workspace builds and tests there,
and the bridge is portable Lua, but Windows 11 is the supported acceptance
platform and only the PowerShell scripts exist.

---

## Starting the REAPER bridge

The bridge is an ordinary ReaScript. Register it once:

```text
REAPER
→ Actions
→ Show action list
→ New action
→ Load ReaScript
→ Select QLabs_Reaper_MCP_Bridge.lua
→ Run the registered action
```

The script is at:

```text
<REAPER Resource Path>\Scripts\QLabs-Reaper-MCP\QLabs_Reaper_MCP_Bridge.lua
```

`<REAPER Resource Path>` is whatever *Options → Show REAPER resource path*
opens. Never assume `%APPDATA%\REAPER`: portable installations are supported,
and the bridge resolves its own location at runtime.

**The bridge must be running for every project operation.**
`reaper.inspect_selection`, `reaper.stage_candidate`, `commit`, `discard` and
`undo` all fail with `BRIDGE_OFFLINE` when it is not. Theory search, fixture
analysis and candidate generation from an existing snapshot do not need it.

`reaper.status` is the exception, deliberately: it **succeeds** with no bridge
running and answers `"bridge_connected": false` with a `bridge_offline` or
`bridge_not_configured` warning, so a client can diagnose the problem instead of
retrying blindly. It is the right first call of any session.

The bridge is a single-instance, `defer()`-driven polling loop. It writes a
heartbeat once a second, holds `bridge.lock`, and sets its own toolbar toggle
state so you can bind it to a button. It stops when you run the action again, or
when REAPER closes. Details in
[`docs/REAPER_BRIDGE.md`](docs/REAPER_BRIDGE.md).

---

## MCP host configuration

This is a normal local **stdio** MCP server. It opens no port. Host
configuration file locations vary by client and are not standardized — consult
your client's documentation for where its server list lives. The server itself
does not care which client it is.

A generic configuration looks like:

```json
{
  "mcpServers": {
    "qlabs-reaper-music": {
      "command": "C:\\Program Files\\QLabs\\qlabs-reaper-music-mcp.exe",
      "args": [
        "serve",
        "--ipc-dir",
        "C:\\Users\\you\\AppData\\Roaming\\REAPER\\Scripts\\QLabs-Reaper-MCP\\ipc"
      ]
    }
  }
}
```

`install.ps1` prints this snippet with your real paths already substituted, and
the server's own `print-mcp-config` subcommand emits it too:

```powershell
& 'C:\Program Files\QLabs\qlabs-reaper-music-mcp.exe' print-mcp-config
```

Backslashes must be escaped in JSON. Use the snippet the installer prints rather
than retyping paths.

Do not add any argument that opens a socket — there isn't one. Diagnostics go to
`stderr`, so a host that surfaces server logs will show them there; `stdout`
carries protocol bytes only.

---

## First-use workflow

1. Open REAPER and load or create a project.
2. Run the `QLabs_Reaper_MCP_Bridge.lua` action. The ReaScript console reports
   the IPC directory and the config path.
3. Select **one** MIDI item, or open a MIDI editor and select the notes you want
   to work on. One active or selected MIDI source at a time.
4. Start your MCP client. It connects to `qlabs-reaper-music-mcp` over stdio.
5. Ask it to check the connection — it calls `reaper.status`. You should see the
   bridge version, the REAPER version and an active project.
6. Ask it to look at your selection — it calls `reaper.inspect_selection`, which
   returns a **snapshot**: the notes, the tempo map, content hashes, and the
   assumptions it made about what you meant.
7. Ask for an analysis — `music.analyze_selection` gives you key candidates,
   phrases, structural notes, the harmonic grid it chose and why.
8. Ask for harmonizations — `harmony.generate_candidates` returns several ranked,
   genuinely different candidates, each with a decision trace.
9. Read the explanations. `candidate.explain` expands any one of them into the
   rules that fired and the sources behind them.
10. Stage the one you like — `reaper.stage_candidate` creates a new tagged folder
    of **muted** preview tracks. Your source item is untouched.
11. Unmute and listen.
12. Keep it with `reaper.commit_candidate`, throw it away with
    `reaper.discard_candidate`, or step back with
    `reaper.undo_last_generation`.

Steps 10–12 are the only ones that write anything, and every one of them is
reversible.

Every tool, argument, output and error is documented in
[`docs/MCP_API.md`](docs/MCP_API.md).

---

## Example requests

Things you can say to an MCP client connected to this server:

- *"What key is my selected melody in, and how confident are you?"*
- *"Analyse the selected notes. Which are structural and which are passing
  tones?"*
- *"Create three harmonizations. Preserve the melody and timing. Make one warm
  and extended, one dark and modal, and one chromatic. Add bass and a restrained
  countermelody. Keep the eight-bar region loopable."*
- *"Reharmonize this progression in a neo-soul style but keep the cadence and
  the bass line."*
- *"Give me smoother voicings for these chords — four voices, keep the top note,
  no leaps bigger than a fifth."*
- *"Why did you pick a tritone substitution in bar six?"*
- *"Arrange this sketch for pad, comping and bass with an energy curve that
  builds toward bar twelve."*
- *"Audit this eight-bar loop. Does it wrap cleanly, and what would fix it?"*
- *"Search the theory knowledge for rules about parallel fifths in modal
  contexts."*
- *"Stage candidate two, then undo it — I want to hear it and then decide."*

Prompt templates are provided for the common flows:
`analyze-selected-melody`, `harmonize-selected-melody`,
`reharmonize-selected-region`, `extend-selected-chords`,
`create-smooth-voicings`, `arrange-selected-sketch`,
`create-loopable-variants`, `audit-harmony-and-voice-leading` and
`explain-generated-candidate`.

---

## Safety behaviour

The system controls a creative project, so it defaults to preservation.

- **The source is read-only.** The take you selected is never opened for
  writing. Generated material always lands on new tracks.
- **Staging is a preview.** Staged tracks are created muted and tagged
  `QLABS_STATUS = preview`. Nothing is permanent until you commit.
- **Commit is explicit.** `reaper.commit_candidate` is the only thing that flips
  a preview to committed.
- **Discard is ownership-scoped.** It deletes only objects carrying
  `QLABS_OWNER = QLabs-Reaper-MCP` *and* your transaction id. Track and item
  **names are never used to identify objects**. A tagged track still holding
  items that are not part of the transaction is kept, and you are told so.
- **Undo is ownership-scoped.** `reaper.undo_last_generation` undoes at most one
  entry, and only when REAPER's top undo entry is exactly this product's last
  owned transaction label. An unrelated REAPER undo entry is never undone.
- **Stale snapshots are rejected.** If you edit the source between generating and
  staging, the bridge refuses with `STALE_SNAPSHOT` instead of applying a plan
  built against material that no longer exists.
- **The command surface is closed.** Seven bridge commands, none of which
  evaluates Lua, runs a REAPER action id, reads or writes an arbitrary path, or
  passes anything through to the REAPER API. No `execute_lua`, no
  `run_reaper_action`, no filesystem passthrough — not hidden behind a flag,
  simply absent.
- **Everything is bounded.** Request and result sizes, note counts, track counts,
  operation counts, beam width, candidate pools, log size and file retention all
  have hard caps.
- **Logs are redacted.** Full note content never appears in ordinary logs.

Read [`docs/SECURITY.md`](docs/SECURITY.md) for the threat model, the trust
boundaries and the recovery procedures.

---

## Troubleshooting

Start with the built-in diagnostics. On the Rust side:

```powershell
.\scripts\run-doctor.ps1
qlabs-reaper-music-mcp doctor --json
```

On the REAPER side, run `QLabs_Reaper_MCP_Status.lua` from the Action List. It
reports the resolved directories, the config, heartbeat age, lock holder, file
counts, the project UUID, what it would currently resolve as the MIDI source,
the live hashes, staged transactions and the top undo entry.

| Symptom | Likely cause and fix |
|---|---|
| `BRIDGE_OFFLINE` | The bridge action is not running, or the server points at a different `ipc_dir`. Compare `heartbeat.json`'s `ipc_dir` with the server's `--ipc-dir`. |
| `IPC_TIMEOUT` on everything | The server writes to a directory the bridge does not poll. Same comparison as above. |
| `INVALID_INSTANCE_TOKEN` | Server and `config.json` disagree on the token. Re-run `install.ps1`, or copy the token out of `config.json`. |
| "already running" on bridge start | A live instance holds `bridge.lock`. Check the age in the message. Delete the lock only if you are certain no bridge is running. |
| `NO_MIDI_SOURCE` | No MIDI editor open and not exactly one MIDI item selected. |
| `MULTIPLE_MIDI_SOURCES` | Two or more selected items have MIDI takes. Select one. |
| `AMBIGUOUS_MELODY` | The extraction you asked for cannot be resolved — e.g. `selected_only` with nothing selected, `midi_channel` with no notes on that channel, or `monophonic_voice` on genuinely polyphonic material. |
| `STALE_SNAPSHOT` on staging | The project changed after generation, **or** the staging call used a different `note_scope` / `melody_extraction` than the plan was built with. `details.rebuilt_with` says which. Re-inspect and regenerate. |
| `PROJECT_CHANGED` on everything | A `state_change_count` precondition is being enforced; that counter increments on any edit, including a selection change. |
| `UNDO_NOT_OWNED` | Something else happened after staging, so the top undo entry is no longer ours. `details.top_undo_entry` names it. Use `discard` instead. |
| Requests piling up in `commands/` | The bridge is not running or polls elsewhere. Unclaimed files are collected after 300 s. |
| Host shows no tools | The host is not launching the binary, or `serve` is missing from `args`. Check the host's own server log; ours goes to `stderr`. |
| Garbled protocol errors in the host | Something is writing to `stdout`. Nothing in this server should — report it as a bug with the stderr log. |

Failed IPC requests are kept in `<ipc-dir>/failed/` for 24 hours. The bridge log
is `<ipc-dir>/logs/bridge.log`; set `"log_level": "debug"` in `config.json` (and
`"console_log": true` to mirror it into the ReaScript console) when diagnosing.

More in [`docs/REAPER_BRIDGE.md` §12](docs/REAPER_BRIDGE.md) and
[`docs/KNOWN_LIMITATIONS.md`](docs/KNOWN_LIMITATIONS.md).

---

## Known limitations

The short version. The honest, complete version is
[`docs/KNOWN_LIMITATIONS.md`](docs/KNOWN_LIMITATIONS.md) — please read it.

- **MIDI only.** No audio is analysed, transcribed or generated. No stems, no
  chord recognition from audio, no audio understanding of any kind.
- **Melody extraction from polyphony is an inference.** It reports its mode, its
  assumptions and its confidence, and warns when ambiguous, but it can still
  pick the line you did not mean.
- **No plugins, no instruments.** Nothing is installed, selected, loaded or
  configured. Generated tracks have no FX and make no sound until you add an
  instrument yourself.
- **Style models are rule-based, not learned.** Ten hand-authored profiles over
  147 hand-authored rules. They encode defensible conventions, not a model of
  any particular artist or recording.
- **No candidate is guaranteed to be the one you want.** Scores rank against
  encoded conventions and your parameters. Musicians disagree; so will you,
  sometimes. That is why there are several candidates, why each explains itself,
  and why nothing is committed automatically.
- **The bridge must be running** for anything that touches your project.
- **The staleness hash is FNV-1a-64**, a change-detection hash. It is not
  collision-proof and is not a security control.
- **`commit`, `discard` and `undo` are transaction-scoped** and deliberately do
  not re-check the source snapshot.
- **`percussion` is not drum programming.** All 16 arrangement roles have
  catalogued patterns, but the percussion pattern writes pitched MIDI in a
  narrow band rather than a General MIDI drum map — route it to a percussion or
  mallet instrument yourself.
- **The rule-to-test coverage ledger stands at 205 of 237 test ids** (86.5%);
  the remaining 32 are declared pending, not silently missing.
- **Source licences are marked `unverified-reference-only`**, because the build
  had no network access to verify them.
- **The in-REAPER smoke test has been executed** on REAPER 7.78/x64 (Windows 11):
  **28 passed, 0 failed**. Parts of the manual acceptance walkthrough — stale-snapshot
  rejection, `commit_candidate`, and unrelated-action undo protection — remain
  unrun against a real host. See [`docs/TESTING.md`](docs/TESTING.md).

---

## Documentation map

| Document | What is in it |
|---|---|
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Components, pipeline, transaction lifecycle, data ownership, architecture decision records |
| [`docs/MUSIC_IR.md`](docs/MUSIC_IR.md) | The musical domain model, type by type and field by field |
| [`docs/THEORY_MODEL.md`](docs/THEORY_MODEL.md) | Rule kinds, predicates, profiles, chord semantics, how to add a rule safely |
| [`docs/RESEARCH_AND_PROVENANCE.md`](docs/RESEARCH_AND_PROVENANCE.md) | Source policy, registry, licensing posture, paraphrasing policy, confidence |
| [`docs/MCP_API.md`](docs/MCP_API.md) | Every tool, resource, prompt, argument, output and error |
| [`docs/REAPER_BRIDGE.md`](docs/REAPER_BRIDGE.md) | The Lua bridge: IPC, snapshots, hashing, tagging, undo, recovery, logs |
| [`docs/INSTALL_WINDOWS.md`](docs/INSTALL_WINDOWS.md) | Step-by-step Windows 11 + REAPER 7.x installation |
| [`docs/FIRST_SESSIONS.md`](docs/FIRST_SESSIONS.md) | Ready-to-run scenarios for the first real session in REAPER, with pass criteria |
| [`docs/SECURITY.md`](docs/SECURITY.md) | Threat model, trust boundaries, retention, recovery |
| [`docs/TESTING.md`](docs/TESTING.md) | Every test group, how to run it, golden regeneration, the smoke test |
| [`docs/KNOWN_LIMITATIONS.md`](docs/KNOWN_LIMITATIONS.md) | What this does not do, and where it is weak |
| [`CHANGELOG.md`](CHANGELOG.md) | Release history |
| [`knowledge/ATTRIBUTION.md`](knowledge/ATTRIBUTION.md), [`knowledge/LICENSE.md`](knowledge/LICENSE.md) | Knowledge-bundle attribution and licensing |

---

## Licence

MIT. See [`LICENSE`](LICENSE). The knowledge bundle's own terms and the
unverified status of its upstream references are in
[`knowledge/LICENSE.md`](knowledge/LICENSE.md).
