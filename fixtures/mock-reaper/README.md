# `fixtures/mock-reaper`

Cross-language fixtures for the QLabs REAPER MCP bridge. Everything here is
**generated**, never hand-edited:

```
lua5.4 reaper/tests/gen_fixtures.lua
```

`reaper/tests/test_fixtures.lua` re-derives every file from the live Lua
implementation on each test run, so these fixtures cannot silently drift from
the bridge. If that suite fails, either the bridge changed behaviour (regenerate
and tell the Rust side) or a regression was introduced.

The Rust `reaper-ipc` crate should treat these as its golden inputs.

## `hashes/vectors.json`

**The most important file here.** Each entry pairs an exact `canonical_string`
with the `hash` the bridge produces for it. A Rust implementation of FNV-1a-64
plus the canonical-string builders must reproduce every one byte for byte.

The three `fnv1a64.*` entries are plain algorithm vectors (`""`, `"a"`,
`"foobar"`) — start there when bringing up the Rust hash. The rest exercise the
real canonical layouts: `midi_hash`, `note_selection_hash`, `tempo_map_hash`,
`timesig_map_hash`, `note_list_hash`, `snapshot_hash`.

FNV-1a-64 is non-cryptographic; see `docs/REAPER_BRIDGE.md` for the caveat.

## `requests/`

Request envelopes exactly as they appear in `<ipc-dir>/commands/<id>.command.json`.

| file | outcome |
|------|---------|
| `valid-ping` | accepted |
| `valid-status` | accepted |
| `valid-inspect-selection` | accepted |
| `valid-inspect-with-preconditions` | accepted; carries a full `expected_project` block |
| `valid-commit` / `valid-discard` / `valid-undo` | accepted |
| `invalid-protocol-version` | `IPC_PROTOCOL_MISMATCH` |
| `invalid-token` | `INVALID_INSTANCE_TOKEN` |
| `invalid-expired` | `EXPIRED_REQUEST` |
| `invalid-unknown-command` | `UNKNOWN_COMMAND` |
| `invalid-request-id` | `MALFORMED_REQUEST` (path-escaping id) |
| `invalid-payload-is-array` | `MALFORMED_REQUEST` |
| `invalid-truncated` | not valid JSON at all; must fail to parse |

All request fixtures carry `expires_at = 2026-07-26T18:51:49Z`. Validate them
against a fixed "now" of `2026-07-26T18:51:20Z`, and use the installation token
`0123456789abcdef0123456789abcdef`.

## `plans/`

`valid-two-part-candidate.plan.json` is a complete `EditPlan`: a folder track,
two child tracks, two MIDI items, eight notes, five preconditions, five expected
outputs. It stages cleanly against the fixture scene when `note_scope: "all"` is
echoed in the payload.

`invalid-index.json` maps each rejected plan to the error code it must produce.
Every one of them must be refused **before** any REAPER mutation: zero tracks
created, zero project state change, zero undo entries.

## `snapshots/`

`four-note-melody.snapshot.json` is the `inspect_selection` result for the
fixture scene, with `snapshot_id`, `project_pointer` and timestamps pinned so
the file is byte-reproducible.

## `results/`

Result envelopes: one success (`stage_candidate`) and two structured failures
(`NO_MIDI_SOURCE`, `STALE_SNAPSHOT`). Use them to check that exactly one of
`result` / `error` is non-null and that `ok` is the discriminator.

## `bridge/`

`heartbeat.json`, `bridge.lock` and `config.json` in the shapes the bridge
writes, plus two machine-readable manifests:

- `limits.json` — every numeric limit, by name.
- `error-codes.json` — the full error-code list, the command allowlist, the edit
  operation allowlist and the ownership-tag allowlist.

## The fixture scene

One track, one MIDI item spanning project QN `0.0 .. 8.0` at 120 BPM 4/4 with no
explicit tempo markers, holding four quarter notes:

| index | QN | pitch | velocity | channel | selected |
|-------|----|-------|----------|---------|----------|
| 0 | 0.0–1.0 | 60 | 100 | 0 | yes |
| 1 | 1.0–2.0 | 62 | 96 | 0 | yes |
| 2 | 2.0–3.0 | 64 | 92 | 0 | yes |
| 3 | 3.0–4.0 | 65 | 88 | 0 | no |

The project UUID is pinned to `00000000-0000-4000-8000-000000000001`. GUIDs come
from the mock host's deterministic counter, so they are stable across runs.
