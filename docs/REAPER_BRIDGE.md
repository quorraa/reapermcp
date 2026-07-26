# The QLabs REAPER bridge

How the REAPER side of the QLabs REAPER Music Intelligence MCP works, why it is
built the way it is, and what its guarantees and limits actually are.

The byte-level wire contract is delivered separately to the Rust implementer as
`IPC_WIRE.md`, and is mirrored machine-readably in
[`schemas/ipc-request.schema.json`](../schemas/ipc-request.schema.json),
[`schemas/ipc-result.schema.json`](../schemas/ipc-result.schema.json) and the
generated fixtures in
[`fixtures/mock-reaper/`](../fixtures/mock-reaper/README.md). This document is
the prose companion: design, rationale, operations, failure modes.

---

## 1. Shape of the thing

```
┌──────────────────────┐        files only        ┌────────────────────────┐
│ qlabs-reaper-music-  │  ───────────────────────▶│ QLabs_Reaper_MCP_      │
│ mcp   (Rust, stdio)  │◀───────────────────────  │ Bridge.lua  (in REAPER)│
└──────────────────────┘   <ipc-dir>/commands     └────────────────────────┘
                           <ipc-dir>/results
```

- **No network port.** Ever. The two processes share a directory, nothing else.
- **No arbitrary execution.** There is no command that evaluates Lua, invokes an
  action id, or forwards arguments to the REAPER API.
- **No path input.** The only caller-controlled value that reaches a filesystem
  path is `request_id`, and it must match
  `^[A-Za-z0-9][A-Za-z0-9._-]*$`, be ≤128 bytes, contain no `..`, and equal the
  stem of the file it arrived in.
- **Everything of substance is in `reaper/lib`.** The ReaScript entry point only
  wires the engine to `reaper.defer` / `reaper.atexit` and the toolbar toggle,
  which is why the whole state machine is testable without REAPER.

Pure Lua 5.4, no external packages, no `require` outside `reaper/lib`. The lib
directory is located from `debug.getinfo(1,"S").source`, and the IPC directory
from `reaper.GetResourcePath()`-relative install location or an explicit
`config.json` override — never from a hardcoded `%APPDATA%` path, because
portable REAPER installations are a supported configuration.

---

## 2. The polling loop

`reaper.defer` re-invokes the loop at REAPER's UI rate (roughly 30 Hz). The loop
does not busy-wait and does not scan the directory on every call:

- A directory scan happens at most once per `POLL_INTERVAL_SECONDS` (0.05 s),
  measured with `reaper.time_precise()`.
- At most `MAX_COMMANDS_PER_TICK` (4) commands are claimed per tick, so a burst
  of work cannot stall REAPER's UI thread.
- The heartbeat is rewritten at most once per `HEARTBEAT_INTERVAL_SECONDS` (1 s).
- Stale-file garbage collection runs at most once per `GC_INTERVAL_SECONDS` (30 s).

The whole tick is wrapped in `xpcall`. A Lua error is logged and the loop
continues; it never escapes into REAPER's defer machinery and it never stops the
bridge. `reaper.atexit` writes a final `"offline"` heartbeat, drops the lock and
clears the toolbar toggle.

---

## 3. Single instance

`<ipc-dir>/bridge.lock` holds a JSON document with a `pid_token` (32 hex
characters minted at startup) and a `heartbeat_at` timestamp refreshed on every
heartbeat.

- **Starting.** If the lock exists, parses, names a different token, and its
  `heartbeat_at` is within `LOCK_STALE_SECONDS` (10 s), the new instance refuses
  to start and says so in a message box. Otherwise it takes the lock over — this
  is how a crashed REAPER's orphaned lock is recovered without manual cleanup.
- **Running.** Before each heartbeat write the instance re-reads the lock. If the
  token is no longer its own, it stops itself. Ordering matters here: the check
  runs *before* the write, because the write also refreshes the lock and would
  otherwise silently reclaim it.
- **Corrupt lock.** An unparseable lock file is treated as absent. It is better
  to start than to be permanently wedged by a truncated write.

---

## 4. Atomic file IPC

```
Rust:   commands/<id>.tmp        →(rename)→  commands/<id>.command.json
Bridge: commands/<id>.command.json →(rename)→ processing/<id>.processing.json
Bridge: results/<id>.result.tmp  →(rename)→  results/<id>.result.json
Bridge: processing/<id>.processing.json → deleted, or → failed/<id>.failed.json
Rust:   reads and deletes results/<id>.result.json
```

Why each part matters:

- **The temp file is a sibling of its final name.** Same filesystem, so the
  rename is atomic. A reader never observes a partial write.
- **The bridge never reads `.tmp`.** The directory listing is filtered to names
  ending in exactly `.command.json`. A half-written request is invisible.
- **The claim is a rename.** Exactly one process can win it. If two bridge
  instances ever raced (they should not — see the lock), the loser's rename
  fails and it moves on silently.
- **Failures are preserved, not deleted.** A request that fails validation or
  execution ends up in `failed/` for diagnosis, and its structured error is
  still written as a normal result so the caller is never left waiting.

### Stale-file collection

| location | pattern | age | action |
|----------|---------|-----|--------|
| `commands/` | `*.tmp` | 60 s | delete |
| `commands/` | `*.command.json` | 300 s | delete |
| `processing/` | `*.processing.json` | 120 s | move to `failed/` |
| `results/` | `*.result.tmp` | 60 s | delete |
| `results/` | `*.result.json` | 900 s | delete |
| `failed/` | `*.failed.json` | 86400 s | delete |

**Only files matching those suffixes in those directories are ever touched.**
Anything else you leave in the IPC tree stays there forever.

Plain Lua has no `stat()`. When the host provides `reaper.JS_File_Stat` (the
js_ReaScriptAPI extension) the bridge uses a real mtime; otherwise it falls back
to a first-seen registry recorded the first time the file appears in a scan.
The consequence: without js_ReaScriptAPI, ages are measured from when *this*
bridge instance first noticed a file, so a restart resets the clock. That is
conservative in the right direction — nothing gets deleted too early.

### Logging

`logs/bridge.log`, appended, rotated at `LOG_MAX_BYTES` (1 MiB) keeping
`LOG_KEEP` (3) generations. Log writes are wrapped in `pcall`: a full disk
degrades logging, it does not break the bridge.

---

## 5. Validation

Every request runs the same gauntlet before a single REAPER call is made:

file size → JSON parse → object shape → protocol version → bridge version →
installation token → request-id shape → request-id/filename agreement →
replay guard → `expires_at` → `created_at` → command present → command
allowlisted → payload shape → `expected_project` shape → REAPER version →
active project → project preconditions → command payload.

Notes on a few of these:

- **The installation token is not security.** It is a 32-hex-character value
  shared through `config.json`. It exists so that an unrelated file dropped into
  the IPC directory is never executed as a command. It is compared with a
  constant-shape loop over all bytes (there is no reason to leak a prefix match),
  but anyone who can read the IPC directory can read the token. Filesystem
  permissions are the actual boundary.
- **Replay guard.** The last `SEEN_REQUEST_IDS` (512) processed ids are held in a
  bounded FIFO. A repeat is `DUPLICATE_REQUEST`. The set is in-memory, so it
  resets on restart; never reuse a request id.
- **Clock skew.** `expires_at` is compared against the bridge's wall clock with a
  `CLOCK_SKEW_SECONDS` (5 s) allowance. Both processes are on the same machine,
  so this is generosity rather than necessity.

The command allowlist is exactly seven entries:

```
ping   status   inspect_selection   stage_candidate
commit_candidate   discard_candidate   undo_last_generation
```

Anything else is `UNKNOWN_COMMAND`, including — deliberately — `execute_lua`,
`execute_shell`, `run_reaper_action`, `write_arbitrary_midi`,
`delete_track_by_name` and `edit_project_chunk`.

---

## 6. Reading: source resolution and snapshots

### Source order

1. The **active MIDI editor's take**, if there is one, its pointer validates, and
   it is MIDI.
2. Otherwise **exactly one selected media item** whose *active* take is MIDI.
   Zero is `NO_MIDI_SOURCE`; two or more is `MULTIPLE_MIDI_SOURCES`.
3. Otherwise a structured error.

`source_mode: "active_editor"` never falls back to the selection.
`source_mode: "selected_item"` never consults the editor.

All enumeration is safe count-and-get: count once, index within that count, stop
early if a getter returns false. No collection index is ever held across a
mutation — the read path performs no mutations at all, and the write path
resolves pointers before mutating and never re-indexes afterwards.

### What a note carries

Start and end in take PPQ, in project quarter notes (via
`MIDI_GetProjQNFromPPQPos`), in project seconds, and relative to the item start;
plus pitch, velocity, channel, muted and selected. Notes are returned sorted by
`(start_ppq, pitch, channel, end_ppq, velocity, original index)`, which makes the
array independent of REAPER's internal event ordering and therefore hashable.

### Extraction

`note_scope` (`selected_or_all` | `selected_only` | `all`) and
`melody_extraction.mode` are honoured only where they are pure filters:
`selected_notes` forces `selected_only`, and `midi_channel` filters by channel.
The remaining modes (`highest_voice`, `lowest_voice`, `monophonic_voice`,
`all_notes_as_harmony`) are analysis decisions that belong server-side; the
bridge returns every in-scope note and records the fact in
`selection_assumptions`. `monophonic_voice` is the one exception where the bridge
still acts: it refuses with `AMBIGUOUS_MELODY` when the material is polyphonic,
because silently picking a voice would be a musical decision made in the wrong
place.

An empty take is not an error. It yields `note_count: 0` and an
`empty_selection` warning.

---

## 7. Hashing

Every hash the bridge emits is **FNV-1a 64-bit** over a canonical ASCII string,
rendered as `fnv1a64:` followed by 16 lowercase hex digits.

Six hashes, six canonical layouts:

| hash | over |
|------|------|
| `midi_hash` | every note in the take (unfiltered): index, start/end PPQ, channel, pitch, velocity, muted |
| `note_selection_hash` | the same notes: index, selected |
| `tempo_map_hash` | tempo/time-signature markers: index, time, QN, BPM, numerator, denominator, linear |
| `timesig_map_hash` | markers carrying a time signature: index, QN, numerator, denominator |
| `note_list_hash` | the filtered note list in project QN |
| `snapshot_hash` | a fixed-order `key=value` block combining the identity fields and the five hashes above |

Formatting rules that make this reproducible in another language: floats are
`printf("%.6f")` with negative zero normalised to `0.000000`; integers are plain
decimal; booleans are `1`/`0`; absent strings are the literal `null`; fields are
separated by `|` and every line ends with `\n`.

`snapshot_hash` deliberately excludes `snapshot_id`, `timestamp`,
`bridge_version`, `reaper_version`, `project_pointer` — all per-call values — and
also `project_state_change_count`. That last exclusion is a design decision worth
stating plainly: REAPER's state-change count increments on *any* project edit,
including a mere selection change, so folding it into the snapshot hash would
reduce the hash to a restatement of that counter rather than a content hash of
the musical material. Callers who genuinely want "nothing at all may have
happened" can enforce `expected_project.state_change_count` directly.

### The collision-resistance caveat

**FNV-1a-64 is not a cryptographic hash.** It is fast, trivially implementable in
pure Lua with Lua 5.4's wrapping 64-bit integer arithmetic, and reliable at
detecting the thing it is used to detect: a user edited a note, moved an item,
or changed the tempo between one call and the next. It is *not* collision
resistant against an adversary who can choose inputs, and a 64-bit digest has a
birthday bound around 2³² inputs even against chance.

This is an accepted trade-off, not an oversight:

- The channel is local, same-user, and file-based. There is no trust boundary
  that a hash collision would cross — an attacker who can craft MIDI to collide
  a hash can already just edit the project directly.
- The alternative, a pure-Lua SHA-256, is roughly an order of magnitude slower
  per byte in interpreted Lua and would run on the UI thread during every
  inspection of potentially tens of thousands of notes.

Both sides must therefore treat a hash match as *"probably unchanged"* and never
as an authorisation. If the threat model ever changes, the canonical strings are
already versioned (`qlabs.midi.v1`, `qlabs.snapshot.v1`, …) and the hash carries
an algorithm prefix (`fnv1a64:`), so swapping in a different digest is a
mechanical change on both sides.

---

## 8. Writing: edit plans and transaction safety

The bridge never invents material. `stage_candidate` consumes a server-generated
`EditPlan`, and the plan can only express seven operations:

```
create_folder_track   create_track   create_midi_item   insert_notes
set_track_mute        create_region  create_midi_send
```

### Validate everything first

The **complete** plan is validated before `Undo_BeginBlock2` is called: identity
fields, the owned undo-label prefix, the knowledge version, every operation's
shape and ranges, temp-id uniqueness, reference resolution (forward references
are rejected outright), note ranges against item bounds, tag keys against the
closed `QLABS_*` allowlist, and every size cap. A plan that fails validation
performs zero mutations and creates zero undo entries — there is a fixture and a
test for exactly that.

Preconditions are then checked against live project state, still before the undo
block: project UUID, state-change count, item and take GUIDs, MIDI hash, tempo
map hash, item bounds, and the plan's `base_snapshot_hash` against a freshly
derived snapshot. Per brief §21 the bridge does not silently regenerate against
changed source material; it returns `STALE_SNAPSHOT` and the server must
re-inspect.

### The transaction

```
Undo_BeginBlock2(proj)
PreventUIRefresh(1)
  … mutations …
  … verify expected outputs …
PreventUIRefresh(-1)
Undo_EndBlock2(proj, "QLabs MCP: …", -1)
UpdateArrange()
```

The mutations run inside `xpcall`. The UI-refresh decrement and the
`Undo_EndBlock2` run **outside** it, on every path, driven by a depth counter —
so a raised error can never leave REAPER with UI refresh suppressed. The bridge
has a test that forces a host failure mid-transaction and asserts the counter is
back to zero.

On failure the bridge:

1. restores UI refresh;
2. closes the undo block with the *same owned label*, so the partial work is one
   identifiable entry;
3. compares the project state-change count before and after — if nothing
   changed, no undo entry exists and there is nothing to roll back;
4. otherwise checks that `Undo_CanUndo2` returns *exactly* that owned label, and
   only then calls `Undo_DoUndo2`;
5. returns a structured failure with `details.rolled_back`.

Step 4 is the whole point: **an unrelated action is never undone**. There is a
test that puts a user action on top of the undo stack, forces a staging failure,
and asserts the user's action is still there afterwards.

### MIDI writing specifics

- Items are created with `CreateNewMIDIItemInProj(track, start_qn, end_qn, true)`
  — the project-QN form, so item bounds are exact in musical time regardless of
  tempo.
- Notes are re-sorted deterministically by the bridge, inserted with
  `MIDI_InsertNote(..., noSortInOptional = true)`, then `MIDI_Sort` is called
  **once**, then the resulting event count is verified against the plan.
- Notes are clamped into the item's bounds before insertion; a note that would
  collapse to zero length is `INVALID_EDIT_PLAN`.
- **The source take is never opened for writing.** Nothing in the operation set
  can address it.
- `UpdateArrange()` is called exactly once, after the batch.

### Staging layout

One candidate per folder track, one nesting level, matching brief §23:

```
QLabs Candidate 01 — Warm Extended   (folder, I_FOLDERDEPTH = 1)
├── Chords                            (I_FOLDERDEPTH = 0)
├── Bass                              (I_FOLDERDEPTH = 0)
└── Countermelody                     (I_FOLDERDEPTH = -1, last child)
```

Folder depths are fixed up after creation, once child order is known. An empty
folder is flattened to depth 0 so it cannot swallow the rest of the track list.

---

## 9. Ownership

Generated objects are identified **only** by REAPER extended object state:

```
P_EXT:QLABS_OWNER == "QLabs-Reaper-MCP"  AND  P_EXT:QLABS_TRANSACTION_ID == <id>
```

Track and item **names are never used to identify anything**. Names are for
humans; they get renamed, duplicated, and copied between projects. There is a
test that puts an identically named untagged track next to a generated one and
asserts `discard_candidate` deletes only the tagged one.

The tag key allowlist is closed (14 keys). A plan that supplies any other key —
including real REAPER keys such as `P_NAME` or `GUID` — is `INVALID_EDIT_PLAN`.
That is what stops the tag channel from becoming a general write primitive into
REAPER object state.

Project-level state lives in `GetProjExtState`/`SetProjExtState` section
`QLABS_MCP`: the persistent project UUID (minted once, on first inspection), the
bridge schema version, a bounded list of staged transaction records, the last
committed transaction id, and a record of the last owned transaction.

### commit / discard / undo

- **`commit_candidate`** flips `QLABS_STATUS` from `preview` to `committed` on
  tag-matching objects, stamps `QLABS_COMMITTED_AT`, and unmutes anything that
  carried `QLABS_PREVIEW_MUTED`. Objects that are not tagged with this
  transaction, or that are not in `preview`, are untouched. Committing twice is a
  no-op. Commit does not merge into the source and does not delete anything.
- **`discard_candidate`** deletes tag-matching items, then tag-matching tracks —
  but a generated track that still holds items belonging to somebody else is
  **kept**, with a `track_retained` warning. Losing a user's work because they
  parked an item on a generated track would be unforgivable.
- **`undo_last_generation`** undoes at most one entry, and only when the project
  holds a last-transaction record, the requested id (if any) matches it,
  `Undo_CanUndo2` returns a non-empty string, that string equals the record's
  label exactly, and it starts with `QLabs MCP: `. Otherwise `UNDO_NOT_OWNED`,
  with the offending top entry in `details.top_undo_entry`.

---

## 10. Errors

Every failure is a structured `{code, message, details}` object inside a normal
result envelope. Branch on `code`; `details` is advisory and may change.

All twenty-two codes from brief §19 exist. `BRIDGE_OFFLINE` and `IPC_TIMEOUT` are
inherently client-side (they describe the absence of a bridge response) and are
produced by the Rust side; the bridge defines them so both halves share one
vocabulary.

Five extension codes go beyond the brief's list, because the brief's list has no
member that honestly describes these conditions: `MALFORMED_REQUEST`,
`UNKNOWN_COMMAND`, `DUPLICATE_REQUEST`, `RESULT_TOO_LARGE`,
`TRANSACTION_NOT_FOUND`. They are documented in the wire spec and enumerated in
`fixtures/mock-reaper/bridge/error-codes.json`.

Any Lua error caught by the bridge's guards becomes `INTERNAL_BRIDGE_ERROR` with
the message and a traceback in `details`. Nothing throws into REAPER.

---

## 11. Testing

```
lua5.4 reaper/tests/run_tests.lua
```

184 cases across eight suites, no REAPER and no external Lua package required.
`mock_reaper.lua` implements every `reaper.*` function the bridge calls,
including a real undo journal (mutations record inverse closures; `Undo_DoUndo2`
replays them in reverse, preserving pointer identity and resurrecting deleted
objects) and a `PreventUIRefresh` depth counter. `mock_fs.lua` is an in-memory
filesystem installed as `util.fs`, so the atomic-IPC state machine is exercised
through the real code path.

`fixtures/mock-reaper/**` is generated by `reaper/tests/gen_fixtures.lua` and
re-verified by `test_fixtures.lua` on every run, so the cross-language contract
with the Rust IPC client — hash vectors, canonical strings, edit plans, error
classifications, limits — cannot drift silently.

`QLabs_Reaper_MCP_Smoke_Test.lua` is the in-REAPER counterpart. It refuses to
run against a saved or dirty project unless the user flips an in-script flag,
creates its own clearly marked material, verifies inspection, stages a fixture
candidate, confirms the source is unchanged, confirms the tags exist, confirms
one undo restores the prior state, cleans up after itself, and prints PASS/FAIL
per step.

---

## 12. Troubleshooting

Run `QLabs_Reaper_MCP_Status.lua`. It reports the resolved script and IPC
directories, whether `config.json` exists and (redacted) what token it carries,
heartbeat age and verdict, lock holder and staleness, per-directory file counts,
the active project and its UUID, what the bridge would currently resolve as the
MIDI source, the live hashes, the staged transaction list, and the top undo
entry.

| symptom | likely cause |
|---------|--------------|
| "already running" on startup | a live instance holds the lock; check the age reported in the message, delete `bridge.lock` only if you are certain |
| `BRIDGE_OFFLINE` from the server | the bridge script is not running, or the server is pointed at a different `ipc_dir` |
| `INVALID_INSTANCE_TOKEN` | the server and `config.json` disagree; the bridge logs a warning when it mints a new token |
| `IPC_TIMEOUT` on every request | the server writes to a different directory than the bridge polls — compare `heartbeat.json`'s `ipc_dir` with the server's `--ipc-dir` |
| `STALE_SNAPSHOT` on staging | the project changed after generation, **or** the payload's `note_scope` / `melody_extraction` differ from the snapshot they were built against — the error's `details.rebuilt_with` says which settings were used |
| `PROJECT_CHANGED` on everything | `expected_project.state_change_count` is being enforced; it increments on any edit including selection changes |
| `UNDO_NOT_OWNED` | the user did something after staging; `details.top_undo_entry` names it |
| `NO_MIDI_SOURCE` | no MIDI editor open and not exactly one MIDI item selected |
| requests pile up in `commands/` | the bridge is not running, or is running against a different `ipc_dir`; unclaimed files are collected after 300 s |

Failed requests and their bodies are kept in `failed/` for 24 hours. The log is
in `logs/bridge.log`; set `"log_level": "debug"` in `config.json` (and
`"console_log": true` to mirror it to the ReaScript console) when diagnosing.

---

## 13. Known limits

- **Folder nesting is one level deep.** That matches the staging layout the
  product calls for; deeper hierarchies would need a real depth-accumulation
  pass over `I_FOLDERDEPTH`.
- **Only note events are written.** No CC, pitch bend, sysex or automation. The
  brief permits raw MIDI-event APIs only after a well-tested implementation, and
  the safer note APIs are the right first release.
- **Take pitch/rate/playrate are not modelled.** PPQ↔QN conversion goes through
  REAPER's own `MIDI_GetProjQNFromPPQPos` / `MIDI_GetPPQPosFromProjQN`, so
  whatever REAPER reports is what the bridge reports, but nothing reasons about
  a stretched take.
- **No plugin or FX state is read or written**, by design (brief §23).
- **File ages fall back to first-seen** without js_ReaScriptAPI; see §4.
- **The hash is FNV-1a-64**; see §7 for the caveat.
