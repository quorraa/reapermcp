# Changelog

All notable changes to the QLabs REAPER Music Intelligence MCP are documented in
this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet.

## [1.0.0] — 2026-07-27

First release. A local MCP server that analyses MIDI in REAPER, generates
explained harmonization, voicing, arrangement and loop candidates from an
executable music-theory knowledge bundle, and stages them non-destructively onto
tagged preview tracks.

### Protocol versions

- **MCP specification version: `2025-11-25`.** This is the stable specification
  named in the product brief, reported by `initialize`, and recorded in the
  README and `docs/MCP_API.md`.

  The brief asked that modelcontextprotocol.io be checked for a newer stable
  specification before implementation. **That check could not be performed** —
  the documentation host is not reachable from the build environment (HTTP 403
  through the egress proxy). `2025-11-25` is the selected version, not a
  verified-latest version.
- **REAPER IPC protocol version: `qlabs-reaper-ipc/1`.**
- **REAPER bridge version: `1.0.0`**, project ext-state schema version `1`.
- **Knowledge bundle version: `1.0.0`**, schema version `1.0.0`.
- **Fixture format version: `1.0.0`.**

### Added

**MCP server** (`qlabs-reaper-music-mcp`)

- Hand-written JSON-RPC 2.0 server over newline-delimited `stdio`. `stdout`
  carries protocol bytes only; all diagnostics go to `stderr`. No TCP or HTTP
  listener anywhere.
- 14 tools: `reaper.status`, `reaper.inspect_selection`, `theory.search`,
  `music.analyze_selection`, `harmony.generate_candidates`,
  `harmony.reharmonize`, `voicing.generate`, `arrangement.generate`,
  `loop.audit`, `candidate.explain`, `reaper.stage_candidate`,
  `reaper.commit_candidate`, `reaper.discard_candidate`,
  `reaper.undo_last_generation`. Each carries a strict JSON Schema 2020-12
  `inputSchema` and a declared `outputSchema`, both compiled and enforced.
- 13 resources spanning bridge status, project and selection state, the theory
  catalogue, sources, profiles and rules, and server-issued analyses,
  candidates, decision traces, edit plans and transactions.
- 9 prompt templates for the common flows.
- Progress notifications and cancellation for long generation operations.
- CLI: `serve`, `doctor` (with `--json`), `validate-knowledge`,
  `list-profiles`, `analyze-fixture`, `generate-fixture`, `print-mcp-config`,
  `version`. The fixture commands exercise the real engine with no REAPER
  present.
- In-memory, bounded, TTL'd session store for snapshots, analyses, candidates,
  edit plans and transactions. Candidates cache on snapshot, profile, params,
  knowledge hash and seed, and never across a changed snapshot.

**Music engine**

- `music-domain`: exact-rational `BeatTime`, spelled pitch with enharmonic
  distinction, intervals, tempo/meter maps, notes and voices, a fully semantic
  chord model, musical structures, candidates, decision traces and edit plans.
- `theory-kb`: knowledge loading, full validation, a rule engine over a closed
  116-predicate vocabulary, profile inheritance and override resolution, and
  deterministic theory search.
- `music-analysis`: selection normalization and melody extraction, voice
  separation, phrase/subphrase/motive analysis, transparent weighted salience,
  ranked key and mode candidates with evidence, harmonic-grid selection,
  non-chord-tone classification and chord detection.
- `harmony-engine`: per-slot chord-option pools, global beam path search,
  voicing across 13 families, profile-driven voice-leading audit, bass and
  countermelody generation, reharmonization, candidate diversification and
  decision-trace construction.
- `arrangement-engine`: role assignment over 16 arrangement roles, pattern
  realisation, density and register control, masking analysis, energy curves
  and section planning.
- `loop-engine`: loop-boundary analysis, intent-aware wrap scoring, repair
  suggestions and carry policies, with exact loop-length preservation.

**Knowledge bundle** (`knowledge/`, version `1.0.0`, compiled into the binary)

- 147 theory rules — harmony 37, voice leading 31, extensions 20, melody 17,
  arrangement 15, looping 14, counterpoint 13.
- 10 style profiles: `common_practice`, `strict_counterpoint`, `jazz_standard`,
  `blues`, `pop_rock`, `neo_soul_rnb`, `modal_ambient`, `cinematic`,
  `electronic_loop`, `drum_and_bass`.
- 35 scales, 51 chord qualities, 65 chord-symbol aliases, 45 function entries,
  38 voicing templates, 37 progression schemas, 14 cadence schemas,
  28 arrangement patterns, 14 instrument profiles, 22 modes, 28 intervals.
- 9 source records, all currently marked `unverified-reference-only`.
- `manifest.json` carrying a `content_sha256` over the canonical JSON of every
  other knowledge file, recomputed and never trusted from the manifest.

**REAPER integration**

- `QLabs_Reaper_MCP_Bridge.lua`: a persistent, single-instance, `defer()`-driven
  bridge in pure Lua 5.4 using only the official ReaScript API — no SWS, no
  ReaPack, no JS_ReaScriptAPI.
- Atomic file IPC over a shared directory: temp-file-plus-rename on both sides,
  a rename-based claim, seven allowlisted commands, envelope validation with a
  normative 20-step order, replay guard, expiry with clock-skew tolerance, size
  limits and bounded garbage collection.
- Snapshot derivation with six canonical FNV-1a-64 content hashes
  (`midi_hash`, `note_selection_hash`, `tempo_map_hash`, `timesig_map_hash`,
  `note_list_hash`, `snapshot_hash`).
- Non-destructive staging: one level of folder nesting, muted preview tracks,
  a closed allowlist of 14 `QLABS_*` ownership tags written as `P_EXT:` object
  state. The source take is never opened for writing.
- Transaction lifecycle: whole-plan validation before any mutation, a single
  `QLabs MCP: `-prefixed undo block, ownership-scoped commit and discard, and an
  owned undo that refuses to touch an unrelated REAPER undo entry.
- `QLabs_Reaper_MCP_Status.lua` diagnostic script and a guarded
  `QLabs_Reaper_MCP_Smoke_Test.lua`.

**Tooling, tests and documentation**

- `xtask` gates: `validate-knowledge`, `stamp-manifest`, `validate-schemas`,
  `validate-fixtures`, `regen-embedded`, `check-all`.
- 1926 Rust tests passing across 36 test binaries and 9 doc-test targets, and
  184 Lua bridge cases against a mock REAPER host. See `docs/TESTING.md` for
  the per-crate breakdown and for what the figure does and does not cover.
- A 59-file fixture corpus including 13 byte-compared golden analyses and 33
  cross-language mock-REAPER fixtures generated from the live Lua
  implementation.
- A rule-to-test coverage ledger at `crates/theory-kb/tests/test_id_coverage.json`.
- Windows PowerShell `install.ps1`, `uninstall.ps1`, `package.ps1`,
  `run-doctor.ps1` and `validate-release.ps1`.
- `README.md`, `docs/ARCHITECTURE.md`, `docs/MUSIC_IR.md`,
  `docs/THEORY_MODEL.md`, `docs/RESEARCH_AND_PROVENANCE.md`,
  `docs/MCP_API.md`, `docs/REAPER_BRIDGE.md`, `docs/INSTALL_WINDOWS.md`,
  `docs/SECURITY.md`, `docs/TESTING.md`, `docs/KNOWN_LIMITATIONS.md`.

### Security

- Local-only operation: no network listener, no analytics, no telemetry, no
  project upload.
- No arbitrary code execution, no arbitrary REAPER actions, no arbitrary
  filesystem paths. `execute_lua`, `execute_shell`, `run_reaper_action`,
  `write_arbitrary_midi`, `delete_track_by_name` and `edit_project_chunk` are
  absent by design, not gated.
- Installation-token validation on every IPC request, payload and result size
  limits, and a request-id pattern that is the only caller-influenced value ever
  used to build a path.
- Stale-snapshot rejection before any mutation.
- Full note content is redacted from ordinary logs.

### Known limitations at 1.0.0

Recorded in full in `docs/KNOWN_LIMITATIONS.md`. In brief: MIDI only, no audio
understanding, no plugin or instrument automation, rule-based rather than
learned style models, melody extraction from polyphony is an inference, the
bridge must be running for project operations, FNV-1a-64 is a change-detection
hash and not collision-proof, `commit`/`discard`/`undo` are transaction-scoped
and do not re-check the source snapshot, four of the 16 arrangement roles are
served by documented pattern substitution, the rule-to-test coverage ledger
stands at 193 of 237, and all 9 source licences are marked
`unverified-reference-only`.

**The in-REAPER smoke test has not been executed**, because no REAPER host was
available in the build environment.

[Unreleased]: https://github.com/quorraa/reapermcp/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/quorraa/reapermcp/releases/tag/v1.0.0
