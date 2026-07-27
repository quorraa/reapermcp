# MCP API

The complete wire surface of the QLabs REAPER Music Intelligence MCP server:
every tool, every resource, every prompt, every argument, every output field,
every error code, and a worked end-to-end flow.

This document describes **version 1.0.0** of `qlabs-reaper-music-mcp`, speaking
MCP specification version **`2025-11-25`**.

> **Where the JSON came from.** Every JSON fragment below was captured from the
> release binary (`cargo build --release`, then `qlabs-reaper-music-mcp serve`)
> by piping JSON-RPC at its stdin and recording what came back on stdout. The
> schemas are `tools/list` output verbatim. Replies that need a live REAPER
> project were captured with the binary talking to a **stand-in bridge over the
> real file protocol** — the same arrangement
> [`crates/reaper-music-mcp/tests/acceptance.rs`](../crates/reaper-music-mcp/tests/acceptance.rs)
> uses. The server side of every byte shown is genuine; only REAPER itself is
> substituted. Where a reply could not be captured at all without REAPER, this
> document says so at that point rather than inventing one.

Companion documents: [`ARCHITECTURE.md`](ARCHITECTURE.md) for how the server is
put together, [`MUSIC_IR.md`](MUSIC_IR.md) for the musical types inside the
payloads, [`THEORY_MODEL.md`](THEORY_MODEL.md) for what a rule id means,
[`REAPER_BRIDGE.md`](REAPER_BRIDGE.md) for the other side of the bridge, and
[`KNOWN_LIMITATIONS.md`](KNOWN_LIMITATIONS.md) for what none of this does.

---

## Contents

1. [Transport and lifecycle](#1-transport-and-lifecycle)
2. [Conventions](#2-conventions)
3. [Tools](#3-tools)
   - [3.1 `reaper.status`](#31-reaperstatus)
   - [3.2 `reaper.inspect_selection`](#32-reaperinspect_selection)
   - [3.3 `theory.search`](#33-theorysearch)
   - [3.4 `music.analyze_selection`](#34-musicanalyze_selection)
   - [3.5 `harmony.generate_candidates`](#35-harmonygenerate_candidates)
   - [3.6 `harmony.reharmonize`](#36-harmonyreharmonize)
   - [3.7 `voicing.generate`](#37-voicinggenerate)
   - [3.8 `arrangement.generate`](#38-arrangementgenerate)
   - [3.9 `loop.audit`](#39-loopaudit)
   - [3.10 `candidate.explain`](#310-candidateexplain)
   - [3.11 `reaper.stage_candidate`](#311-reaperstage_candidate)
   - [3.12 `reaper.commit_candidate`](#312-reapercommit_candidate)
   - [3.13 `reaper.discard_candidate`](#313-reaperdiscard_candidate)
   - [3.14 `reaper.undo_last_generation`](#314-reaperundo_last_generation)
   - [3.15 Tools that do not exist](#315-tools-that-do-not-exist)
4. [Resources](#4-resources)
5. [Prompts](#5-prompts)
6. [Error codes](#6-error-codes)
7. [Worked example: the acceptance flow](#7-worked-example-the-acceptance-flow)
8. [Where to look next](#8-where-to-look-next)

---

## 1. Transport and lifecycle

### 1.1 Framing

Newline-delimited JSON-RPC 2.0 over **stdio**. One JSON value per line, UTF-8,
`\n`-terminated. There is no `Content-Length` header, no second channel, no
socket and no port. The server is started as a subprocess by the MCP host:

```jsonc
{
  "mcpServers": {
    "qlabs-reaper-music": {
      "command": "/path/to/qlabs-reaper-music-mcp",
      "args": ["serve", "--config",
               "<REAPER resource path>/Scripts/QLabs-Reaper-MCP/config.json"],
      "env": { "QLABS_MCP_LOG": "warn" }
    }
  }
}
```

Reading rules the server applies to your stream:

- A blank or whitespace-only line is **skipped**, not a parse error.
- `params` must be absent, `null`, or an object. An array is
  `-32602 Invalid params`.
- `id` must be a string or an integer. `1.5` or `true` is `-32600`.
- A message carrying `result` or `error` is rejected: this server never issues
  requests, so it can never legitimately receive a response.

### 1.2 stdout carries protocol bytes only

**Nothing but JSON-RPC frames is ever written to stdout.** Diagnostics,
warnings, and the panic hook all write to **stderr**. This is enforced end to
end by `stdout_contains_no_non_mcp_text` and
`the_panic_hook_writes_to_stderr_and_never_to_stdout` in
[`tests/stdout_purity.rs`](../crates/reaper-music-mcp/tests/stdout_purity.rs),
which drive the real binary and assert that every stdout line parses as JSON.

Log verbosity is set with `QLABS_MCP_LOG` (`off | error | warn | info | debug |
trace`, default `warn`) or at runtime with `logging/setLevel`.

### 1.3 `initialize`

The first request must be `initialize`. Anything else before it — except `ping`
— is refused with `-32002`:

```json
{"jsonrpc":"2.0","id":1,"error":{"code":-32002,"message":"the client must send initialize before any other request"}}
```

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2025-11-25",
    "capabilities": {},
    "clientInfo": { "name": "qlabs-doc-capture", "version": "1.0.0" }
  }
}
```

Response, captured verbatim:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": "2025-11-25",
    "capabilities": {
      "tools": { "listChanged": false },
      "resources": { "subscribe": false, "listChanged": false },
      "prompts": { "listChanged": false },
      "logging": {}
    },
    "serverInfo": {
      "name": "qlabs-reaper-music-mcp",
      "title": "QLabs REAPER Music Intelligence",
      "version": "1.0.0"
    },
    "instructions": "QLabs REAPER Music Intelligence. Work in this order: reaper.status to confirm the bridge is running, reaper.inspect_selection to snapshot the selected MIDI, music.analyze_selection to read it, then harmony.generate_candidates for several genuinely different harmonizations. Use candidate.explain to compare them and loop.audit to check a loop point. Stage only the candidate the user chose, with reaper.stage_candidate; staging is additive and muted, and the source item is never modified. Commit or discard explicitly. Every id you pass back must be one this server issued."
  }
}
```

Notes on the handshake:

- The server **always answers with its own** `protocolVersion` (`2025-11-25`).
  A client asking for an older version still gets a successful handshake; the
  mismatch is logged to stderr as
  `client asked for protocol 2025-06-18; this server implements 2025-11-25` and
  nothing else changes. There is no negotiation.
- `capabilities` is exactly what is shown. **Resource subscriptions are not
  supported** (`subscribe: false`), and no `listChanged` notification is ever
  emitted: the tool, resource and prompt sets are fixed at compile time.
- `completions`, `sampling`, `elicitation` and `roots` are **not** advertised
  and not used. The server never calls back into the client.
- `instructions` is served for the host to put in front of the model.

Send `notifications/initialized` afterwards. The server logs it and answers
nothing, per spec.

### 1.4 Supported methods

| Method | Kind | Notes |
|---|---|---|
| `initialize` | request | Always answered; also allowed before initialization. |
| `ping` | request | Answered with `{}`. Allowed before `initialize`. |
| `tools/list` | request | The 14 tools with full input and output schemas. No pagination; there is no `nextCursor`. |
| `tools/call` | request | See [§2](#2-conventions). |
| `resources/list` | request | The 6 concrete resources. No pagination. |
| `resources/templates/list` | request | The 7 templated resources. |
| `resources/read` | request | See [§4](#4-resources). |
| `prompts/list` | request | The 9 prompts. |
| `prompts/get` | request | See [§5](#5-prompts). |
| `logging/setLevel` | request | `{"level": "debug"｜"info"｜"notice"｜"warning"｜"error"｜"critical"｜"alert"｜"emergency"}`. Answered with `{}`. Any other value is `-32602`. |
| `notifications/initialized` | notification | Logged. |
| `notifications/cancelled` | notification | See [§1.6](#16-cancellation). |

Anything else is `-32601`:

```json
{"jsonrpc":"2.0","id":31,"error":{"code":-32601,"message":"unknown method \"nosuch/method\""}}
```

`resources/subscribe`, `resources/unsubscribe`, `completion/complete` and
`logging/setLevel`'s notification counterpart `notifications/message` are **not**
implemented — the server writes its logs to stderr rather than pushing them to
the client.

### 1.5 Progress notifications

If `tools/call` carries `params._meta.progressToken`, the tool emits
`notifications/progress` on the same stdout writer, through the same lock, so a
notification can never interleave with a response inside one line.

Real notifications captured from `reaper.inspect_selection` and
`harmony.generate_candidates`:

```json
{"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"insp-1","progress":0.1,"total":1.0,"message":"asking the bridge for the current selection"}}
{"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"insp-1","progress":0.7,"total":1.0,"message":"converting the snapshot"}}
{"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"insp-1","progress":1.0,"total":1.0,"message":"snapshot stored"}}
{"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"gen-1","progress":0.19,"total":1.0,"message":"building the per-slot chord pools"}}
{"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"gen-1","progress":0.31000000000000005,"total":1.0,"message":"searching whole-phrase paths"}}
{"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"gen-1","progress":0.6300000000000001,"total":1.0,"message":"path search complete"}}
{"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"gen-1","progress":1.0,"total":1.0,"message":"candidates stored"}}
```

`progress` is a fraction clamped to `0.0..=1.0` and `total` is always `1.0`.
`progressToken` is echoed exactly as you sent it. **No progress is emitted when
you do not supply a token.** Seven tools report progress:
`reaper.inspect_selection`, `music.analyze_selection`,
`harmony.generate_candidates`, `harmony.reharmonize`, `voicing.generate`,
`arrangement.generate` and `reaper.stage_candidate`. `reaper.status`,
`theory.search`, `loop.audit`, `candidate.explain`, `reaper.commit_candidate`,
`reaper.discard_candidate` and `reaper.undo_last_generation` are fast enough
that they emit none, whether or not you supply a token.

### 1.6 Cancellation

`notifications/cancelled` with a `requestId` flips a cancellation flag.

```json
{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":99,"reason":"user pressed stop"}}
```

The reader runs on a dedicated thread and sets the flag **before** queueing the
message, so a cancellation arriving during a long generation takes effect
immediately rather than after it. A cancellation that arrives *before* the
request it names is also honoured: the flag is registered pre-cancelled and the
request sees it as soon as it starts.

Contrary to the spec's general advice, a cancelled `tools/call` **is still
answered** — with a normal result whose payload is an `isError` `CANCELLED`
envelope, so a client that is waiting on the id is never left hanging. Captured
by sending the cancellation first and the request second:

```json
{
  "ok": false,
  "error_code": "CANCELLED",
  "message": "the client cancelled this request",
  "details": {}
}
```

Cancellation is checked at stage boundaries inside each engine, so a cancelled
call leaves the session store untouched — nothing half-generated is ever
stored. `cancellation_does_not_corrupt_state` in
[`tests/mcp_protocol.rs`](../crates/reaper-music-mcp/tests/mcp_protocol.rs)
asserts this.

`reason` is accepted and ignored. A `requestId` for a request that has already
finished is accepted and has no effect.

### 1.7 Shutdown

There is no `shutdown` method. **EOF on stdin is the shutdown signal.** The
server drains the queue, logs `stdin closed; shutting down` to stderr, joins the
reader thread and exits `0`. An empty stream produces no output at all and exits
cleanly. Closing the pipe from the host is the correct and only way to stop it.

Everything the server remembers — snapshots, analyses, candidates, plans,
transactions — is **in memory only**. Killing the process discards all of it.
Objects already staged in REAPER are unaffected; they remain in the project as
muted, tagged preview tracks, and can be removed by hand or with REAPER's own
undo. See [`KNOWN_LIMITATIONS.md`](KNOWN_LIMITATIONS.md) §9.

---

## 2. Conventions

### 2.1 Every tool result carries both forms

A successful `tools/call` returns:

```jsonc
{
  "content": [ { "type": "text", "text": "…the same object, pretty-printed…" } ],
  "structuredContent": { "ok": true, "…": "…" },
  "isError": false
}
```

`structuredContent` is the validated object. `content[0].text` is the **same
object** serialized as pretty-printed JSON, so a client with no structured-output
support still sees everything. They never disagree — the text is produced from
the structured value.

Every successful payload has `ok: true` and a `warnings` array. Every payload is
validated against the tool's own declared `outputSchema` *before* it is served;
a payload that fails is replaced with an `OUTPUT_SCHEMA_VIOLATION` error rather
than served, because a result no client can rely on is worse than a failure. The
shared envelope is
[`schemas/mcp-tool-output.schema.json`](../schemas/mcp-tool-output.schema.json).

### 2.2 Tool failures are results, not transport errors

A tool that fails returns a **successful JSON-RPC response** whose result has
`isError: true`:

```jsonc
{
  "content": [ { "type": "text", "text": "…the same payload, pretty-printed…" } ],
  "structuredContent": {
    "ok": false,
    "error_code": "UNKNOWN_ID",
    "message": "no candidate with id not-mine",
    "details": { "kind": "candidate", "id": "not-mine" },
    "remedy": "use an id this server issued in the current session"
  },
  "isError": true
}
```

- `error_code` is a **stable** string from the closed vocabulary in
  [§6](#6-error-codes). **Branch on this.**
- `message` is human-readable and never contains note content.
- `details` is advisory and its shape depends on the code. **Never branch on
  it.**
- `remedy` is present when there is something useful to say.

JSON-RPC transport errors (`-32700`, `-32600`, `-32601`, `-32602`, `-32603`,
`-32002`) are reserved for protocol problems: unparseable JSON, a bad envelope,
an unknown method, a malformed `tools/call` params object, an unknown prompt, an
unreadable resource. A chord that cannot be voiced is not a protocol problem.
`an_invalid_argument_never_becomes_a_transport_error` asserts the boundary.

**One exception worth knowing:** `resources/read` and `prompts/get` are *not*
tool calls, so their failures *are* transport errors — with the same structured
payload attached as `error.data`. See [§4.4](#44-resource-read-failures) and
[§5.2](#52-prompt-failures).

### 2.3 Ids, ownership and TTLs

Every id a tool accepts is one **this server issued in this process**. There is
no way to name material this process did not create. Ids must match

```
^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$
```

with no `..` substring — the same pattern the IPC layer enforces on a request
id. That excludes `/`, `\`, `%`, `~`, drive letters, absolute paths and every
percent-encoded escape of the same, which is what makes an id unusable as a
filesystem path. In practice every id the server mints is a lowercase UUID.

| Kind | Prefix/URI | TTL | Max live | Issued by |
|---|---|---|---|---|
| snapshot | `reaper://selection/current` (latest only) | 1800 s (30 min) | 32 | `reaper.inspect_selection` |
| analysis | `analysis://{id}` | 3600 s (1 h) | 32 | `music.analyze_selection` |
| candidate | `candidate://{id}` | 3600 s (1 h) | 128 | the four generation tools |
| edit plan | `editplan://{id}` | 3600 s (1 h) | 64 | `reaper.stage_candidate` |
| transaction | `transaction://{id}` | 86 400 s (24 h) | 64 | `reaper.stage_candidate` |

Each bucket is bounded and evicts oldest-first when full. Expired entries are
swept after every message. An id whose entry is gone gives `UNKNOWN_ID`; an id
whose entry is present but past its TTL gives `EXPIRED_ID` with `created_at` and
`expired_at` in `details`. The two are deliberately distinguishable.

A candidate carries its own lifetime in its resource body:

```json
  "created_at": "2026-07-27T01:57:34Z",
  "expires_at": "2026-07-27T02:57:34Z"
```

### 2.4 Determinism and the seed contract

**Identical inputs give identical outputs.** There is no wall-clock, no
randomness and no thread scheduling anywhere in the musical decision path. The
seed is a *tie-break* seed, not an entropy source: it only decides between
options the scoring ranked equally.

- `seed` defaults to **`20260726`** on every tool that takes one. A call made
  twice with no seed returns byte-identical candidates.
- Candidate ids are derived deterministically. Regenerating the same material
  with the same parameters returns the **same ids**, not new ones.
- Generation is cached on
  `(snapshot hash, style profile, canonical parameters, knowledge hash, seed)`.
  The result carries `cached: true` on a hit. The key uses the snapshot **hash**,
  not the snapshot **id** — two inspections of an unmodified project produce
  different ids but the same hash, so the cache hits; a single edit in REAPER
  changes the hash, so a stale candidate can never be served for changed
  material.

Captured: the same `harmony.generate_candidates` arguments called twice returned
`"cached": false` then `"cached": true`, with identical `candidate_id` values
both times.

### 2.5 Warnings

`warnings` is an array of `{code, message, severity?}` where `severity` is
`info`, `minor`, `moderate` or `major`. A tool can succeed and still have
something to say. A real one from `music.analyze_selection` over a single-line
melody:

```json
{
  "code": "CHORDS_INFERRED_FROM_MELODY",
  "message": "the material is a single line, so the detected chords are what the melody implies rather than harmony that was played",
  "severity": "info"
}
```

### 2.6 Style profiles

Ten profiles, closed enum on every `style_profile` argument:

```
common_practice · strict_counterpoint · jazz_standard · blues · pop_rock
neo_soul_rnb · modal_ambient · cinematic · electronic_loop · drum_and_bass
```

The default where a tool takes `style_profile` is **`common_practice`**, except
`arrangement.generate`, which inherits the source candidate's profile. Because
the enum is closed, an unknown profile is rejected as `INVALID_ARGUMENTS` at the
schema gate; `UNKNOWN_PROFILE` reaches you only through
`theory://profiles/{profile_id}`.

### 2.7 Time

All musical time in arguments and results is in **project quarter notes**
(`_qn`), as `number`. Bounds are `-100000.0 .. 1000000.0`. Seconds appear only
inside a snapshot's note objects, where they are what REAPER reported.

---

## 3. Tools

`tools/list` returns 14 tools, in this order:

```
reaper.status · reaper.inspect_selection · theory.search · music.analyze_selection
harmony.generate_candidates · harmony.reharmonize · voicing.generate
arrangement.generate · loop.audit · candidate.explain
reaper.stage_candidate · reaper.commit_candidate · reaper.discard_candidate
reaper.undo_last_generation
```

Every `inputSchema` is `"additionalProperties": false` — an unrecognised
argument is an error, never silently ignored. Every tool declares an
`outputSchema` whose top-level `additionalProperties` is `false` as well.

Every tool carries MCP annotations. The ten read-only tools declare
`readOnlyHint: true, destructiveHint: false, idempotentHint: true`; the four
`reaper.*` write tools declare `readOnlyHint: false, destructiveHint: false,
idempotentHint: false, openWorldHint: true`. **No tool declares
`destructiveHint: true`** — nothing here destroys user material.

---

### 3.1 `reaper.status`

> Reports whether the REAPER bridge is running and what project it is attached
> to, plus this server's version and knowledge bundle. Works with no bridge
> running: the reply then says so rather than failing.

**Purpose.** The first call of any session. It is the one bridge-facing tool that
**succeeds when REAPER is not there** — it answers `bridge_connected: false`
rather than `BRIDGE_OFFLINE`, so a client can diagnose the problem instead of
retrying blindly.

**Arguments.** None. The schema is `{"type":"object","properties":{},
"required":[],"additionalProperties":false}` — passing anything is
`INVALID_ARGUMENTS`.

**Output.**

| Field | Type | Meaning |
|---|---|---|
| `ok` | `true` | Always. |
| `bridge_connected` | boolean | Heartbeat present, `online`, and fresh. |
| `bridge_configured` | boolean | An IPC directory was resolved at all. |
| `heartbeat_age_seconds` | number \| null | Null when there is no heartbeat. |
| `bridge_version` | string \| null | From `heartbeat.json`. |
| `reaper_version` | string \| null | Raw `GetAppVersion()`. |
| `ipc_protocol_version` | string | Always `qlabs-reaper-ipc/1`. |
| `mcp_protocol_version` | string | Always `2025-11-25`. |
| `server_version` | string | This binary. |
| `active_project` | boolean | |
| `project_uuid` | string \| null | Null until a first inspection mints one. |
| `project_name` | string \| null | |
| `project_path` | string \| null | `""` when unsaved. |
| `play_state` | integer \| null | REAPER's `GetPlayStateEx` bitmask. |
| `selected_item_count` | integer ≥ 0 | |
| `active_midi_editor` | boolean | |
| `knowledge_version` | string | Bundle version. |
| `knowledge_hash` | string | Bundle content SHA-256. |
| `ipc_dir` | string \| null | The resolved directory. |
| `session` | object | Store occupancy; see below. |
| `warnings` | array | |

`session` is `{snapshots, analyses, candidates, edit_plans, transactions,
cached_generations, uptime_seconds, knowledge_origin}`, where `knowledge_origin`
is `embedded` or a directory path.

Project detail (`active_project` through `active_midi_editor`) needs a real
round trip to the bridge, so it is only attempted when the heartbeat says the
bridge is there. An offline server never blocks on it.

**Errors.** None under normal operation. A `KNOWLEDGE_INVALID` is possible if an
external `--knowledge-dir` bundle fails validation, but that is caught at
startup.

**Example — bridge running.**

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"reaper.status","arguments":{}}}
```

`structuredContent`:

```json
{
  "ok": true,
  "bridge_connected": true,
  "bridge_configured": true,
  "heartbeat_age_seconds": 1.0,
  "bridge_version": "1.0.0",
  "reaper_version": "7.22/linux-x86_64",
  "ipc_protocol_version": "qlabs-reaper-ipc/1",
  "mcp_protocol_version": "2025-11-25",
  "server_version": "1.0.0",
  "active_project": true,
  "project_uuid": "00000000-0000-4000-8000-000000000001",
  "project_name": "AcceptanceProject",
  "project_path": "/projects/acceptance.rpp",
  "play_state": 0,
  "selected_item_count": 1,
  "active_midi_editor": true,
  "knowledge_version": "1.0.0",
  "knowledge_hash": "ad393aa7c3c1250fac38afa29d0b8425e372b605e6300af6bc538cd2aaad03cb",
  "ipc_dir": "/…/Scripts/QLabs-Reaper-MCP/ipc",
  "session": {
    "snapshots": 0,
    "analyses": 0,
    "candidates": 0,
    "edit_plans": 0,
    "transactions": 0,
    "cached_generations": 0,
    "uptime_seconds": 0,
    "knowledge_origin": "embedded"
  },
  "warnings": []
}
```

**Example — no bridge configured at all.** Captured by running `serve` with no
`--config` and no `--ipc-dir`:

```json
{
  "ok": true,
  "bridge_connected": false,
  "bridge_configured": false,
  "heartbeat_age_seconds": null,
  "bridge_version": null,
  "reaper_version": null,
  "ipc_protocol_version": "qlabs-reaper-ipc/1",
  "mcp_protocol_version": "2025-11-25",
  "server_version": "1.0.0",
  "active_project": false,
  "project_uuid": null,
  "project_name": null,
  "project_path": null,
  "play_state": null,
  "selected_item_count": 0,
  "active_midi_editor": false,
  "knowledge_version": "1.0.0",
  "knowledge_hash": "ad393aa7c3c1250fac38afa29d0b8425e372b605e6300af6bc538cd2aaad03cb",
  "ipc_dir": null,
  "session": { "snapshots": 0, "analyses": 0, "candidates": 0, "edit_plans": 0,
               "transactions": 0, "cached_generations": 0, "uptime_seconds": 0,
               "knowledge_origin": "embedded" },
  "warnings": [
    {
      "code": "bridge_not_configured",
      "message": "no IPC directory is configured; pass --config or --ipc-dir to reach REAPER",
      "severity": "minor"
    }
  ]
}
```

When the directory *is* configured but the bridge is not running, the warning
code is `bridge_offline` and its message is the heartbeat's own complaint
(missing file, stale timestamp, `status` not `online`).

---

### 3.2 `reaper.inspect_selection`

> Takes an immutable snapshot of the selected MIDI material and returns a
> snapshot id every later tool refers to. Reads only; never modifies the
> project.

**Purpose.** The entry point to everything musical. It resolves *what you meant
by "the selection"*, reads the notes, hashes them, and stores the result under a
snapshot id. It never writes.

**Arguments.**

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `source_mode` | string | no | `auto` | `auto` \| `active_editor` \| `selected_item` |
| `note_scope` | string | no | `selected_or_all` | `selected_or_all` \| `selected_only` \| `all` |
| `melody_extraction` | object | no | `{}` | see below |
| `melody_extraction.mode` | string | no | `auto` | `auto` \| `selected_notes` \| `highest_voice` \| `lowest_voice` \| `midi_channel` \| `monophonic_voice` \| `all_notes_as_harmony` |
| `melody_extraction.channel` | integer | conditional | — | `0..=15`; **required** when `mode` is `midi_channel` |

The scope you pass is remembered with the snapshot and is echoed back into the
staging payload later, so the bridge re-derives the snapshot the same way it was
first derived. Passing a different scope at staging time produces a spurious
`STALE_SNAPSHOT`; `details.rebuilt_with` distinguishes that from a genuine edit.

**Output.**

| Field | Type | Meaning |
|---|---|---|
| `ok` | `true` | |
| `snapshot_id` | string | Pass this to every later tool. |
| `resource_uri` | string | Always `reaper://selection/current`. |
| `project_uuid` | string \| null | Persistent, stored in project ext state. |
| `project_state_change_count` | integer | REAPER's counter; a staging precondition. |
| `track_guid`, `item_guid`, `take_guid` | string \| null | Normalised, no braces, lowercase. |
| `item_start_qn`, `item_end_qn`, `item_length_qn` | number | Item bounds in project QN. |
| `is_loop_source` | boolean | REAPER's `B_LOOPSRC`. |
| `midi_hash` | string \| null | FNV-1a-64 over the **whole** take. |
| `tempo_map_hash` | string \| null | |
| `snapshot_hash` | string \| null | The staleness token. Hash of the other hashes plus identity and bounds. |
| `note_list_hash` | string \| null | Over the **filtered** `notes` array. |
| `note_count` | integer | Notes in `notes`. |
| `source_note_count` | integer | Notes in the take before filtering. |
| `notes` | array of object | See below. |
| `tempo_bpm_at_start` | number | BPM at the item start. |
| `time_signature_at_start` | object | `{numerator, denominator}`. |
| `note_scope`, `extraction_mode` | string | The **effective** values, echoed. |
| `extraction_channel` | integer \| null | |
| `resolved_by` | string \| null | `active_editor` or `selected_item`. |
| `selection_assumptions` | array of string | Human-readable, e.g. what it did when the request was under-specified. |
| `warnings` | array | |

Each note object: `index`, `source_index`, `start_ppq`, `end_ppq`, `start_qn`,
`end_qn`, `duration_qn`, `item_relative_start_qn`, `item_relative_end_qn`,
`start_seconds`, `end_seconds`, `pitch` (0..127), `velocity`, `channel`
(0..15), `muted`, `selected`.

Every hash the bridge sends is **independently recomputed** by the client from
the data that came with it. A hash that does not reproduce is
`SNAPSHOT_HASH_MISMATCH` — an error, never a warning, because a snapshot whose
own hashes disagree cannot be used as a staleness token.

> **Deviation from brief §15.** The brief lists "Extraction confidence" among
> this tool's returns. The tool does not have an `extraction_confidence` field.
> Extraction confidence is reported by `music.analyze_selection` as
> `extraction.confidence`, alongside `extraction.mode_used` and
> `extraction.assumptions`. `reaper.inspect_selection` reports what it resolved
> (`resolved_by`, `extraction_mode`, `selection_assumptions`) but does not score
> it. The snapshot is a reading of the project; the confidence belongs to the
> inference, which happens in the analysis stage.

**Errors.** `BRIDGE_OFFLINE`, `IPC_TIMEOUT`, `NO_ACTIVE_PROJECT`,
`NO_MIDI_SOURCE`, `MULTIPLE_MIDI_SOURCES`, `AMBIGUOUS_MELODY`,
`UNSUPPORTED_REAPER_VERSION`, `SNAPSHOT_HASH_MISMATCH`, `INVALID_ARGUMENT`
(`midi_channel` without a channel), `INVALID_ARGUMENTS`, `CANCELLED`,
`INTERNAL_BRIDGE_ERROR`.

**Example.**

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call",
 "params":{"name":"reaper.inspect_selection","arguments":{},
           "_meta":{"progressToken":"insp-1"}}}
```

`structuredContent`, over a 29-note eight-bar melody in C major at 110 BPM
(`notes` elided after two entries — the real reply carries all 29):

```jsonc
{
  "ok": true,
  "snapshot_id": "00000000-0000-4000-8000-0000000000aa",
  "resource_uri": "reaper://selection/current",
  "project_uuid": "00000000-0000-4000-8000-000000000001",
  "project_state_change_count": 1,
  "track_guid": "00000001-0001-4001-8001-000000000001",
  "item_guid": "00000002-0002-4002-8002-000000000002",
  "take_guid": "00000003-0003-4003-8003-000000000003",
  "item_start_qn": 0.0,
  "item_end_qn": 32.0,
  "item_length_qn": 32.0,
  "is_loop_source": true,
  "midi_hash": "fnv1a64:4a314e84e9743f65",
  "tempo_map_hash": "fnv1a64:71fd6a6ecb0f88fd",
  "snapshot_hash": "fnv1a64:ba783f8d50ebc7ca",
  "note_list_hash": "fnv1a64:42521de79f7afb41",
  "note_count": 29,
  "source_note_count": 29,
  "notes": [
    {
      "channel": 0, "duration_qn": 1.0, "end_ppq": 960.0, "end_qn": 1.0,
      "end_seconds": 0.5454545454545454, "index": 0,
      "item_relative_end_qn": 1.0, "item_relative_start_qn": 0.0,
      "muted": false, "pitch": 60, "selected": true, "source_index": 0,
      "start_ppq": 0.0, "start_qn": 0.0, "start_seconds": 0.0, "velocity": 96
    },
    {
      "channel": 0, "duration_qn": 1.0, "end_ppq": 1920.0, "end_qn": 2.0,
      "end_seconds": 1.0909090909090908, "index": 1,
      "item_relative_end_qn": 2.0, "item_relative_start_qn": 1.0,
      "muted": false, "pitch": 64, "selected": true, "source_index": 1,
      "start_ppq": 960.0, "start_qn": 1.0, "start_seconds": 0.5454545454545454,
      "velocity": 96
    }
    // … 27 more
  ],
  "tempo_bpm_at_start": 110.0,
  "time_signature_at_start": { "denominator": 4, "numerator": 4 },
  "note_scope": "all",
  "extraction_mode": "auto",
  "extraction_channel": null,
  "resolved_by": "selected_item",
  "selection_assumptions": [],
  "warnings": []
}
```

**Example — no MIDI to read.** Captured with the stand-in bridge answering
`NO_MIDI_SOURCE`:

```json
{
  "ok": false,
  "error_code": "NO_MIDI_SOURCE",
  "message": "no active MIDI editor take and no selected MIDI item",
  "details": { "active_midi_editor": false, "selected_item_count": 0 },
  "remedy": "select one MIDI item, or open a MIDI editor, then retry"
}
```

**Example — `midi_channel` without a channel**, refused before the bridge is
ever contacted:

```json
{
  "ok": false,
  "error_code": "INVALID_ARGUMENT",
  "message": "melody_extraction.channel is required when the mode is midi_channel",
  "details": { "argument": "melody_extraction.channel" }
}
```

---

### 3.3 `theory.search`

> Searches the embedded music-theory bundle for rules, scales, chord qualities,
> progressions, cadences, profiles and sources. For explanation and discovery;
> it is not the harmonization engine.

**Purpose.** Look things up. This tool never influences generation — it exists so
a client can cite a rule and quote a source when explaining a decision. Needs no
bridge and no snapshot.

**Arguments.**

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `query` | string | **yes** | — | 1..512 chars |
| `domains` | array of string | no | all | items from `melody`, `harmony`, `extensions`, `voice_leading`, `counterpoint`, `arrangement`, `looping` |
| `profile` | string | no | none | one of the 10 style profiles |
| `kinds` | array of string | no | all | items from `hard_integrity`, `mathematical_invariant`, `strong_theory_principle`, `theory_default`, `style_sensitive_preference`, `arrangement_heuristic`, `loop_integrity`, `implementation_heuristic` |
| `sources` | array of string | no | all | free-form source ids |
| `max_results` | integer | no | `10` | `1..=100` |

**Output.** `ok`, `query`, `knowledge_version`, `knowledge_hash`,
`result_count`, `hits[]`, `source_ids[]`, `profiles[]`, `warnings[]`.

Each hit: `kind` (`rule`, `progression`, `cadence`, `scale`, `chord_quality`,
`voicing_template`, `arrangement_pattern`, `instrument_profile`, `profile`,
`source`, …), `id`, `title`, `summary`, `score` (a relevance number, not a
confidence), `source_ids[]`, and `detail` — the full record for that kind.

**Errors.** `INVALID_ARGUMENTS` (missing or oversized `query`, unknown enum
member), `KNOWLEDGE_INVALID`.

**Example.**

```json
{"jsonrpc":"2.0","id":16,"method":"tools/call",
 "params":{"name":"theory.search",
           "arguments":{"query":"parallel fifths","max_results":3}}}
```

`structuredContent` (the third hit and parts of `detail` elided; the real reply
is ~19 kB):

```jsonc
{
  "ok": true,
  "query": "parallel fifths",
  "knowledge_version": "1.0.0",
  "knowledge_hash": "ad393aa7c3c1250fac38afa29d0b8425e372b605e6300af6bc538cd2aaad03cb",
  "result_count": 3,
  "hits": [
    {
      "kind": "rule",
      "id": "voice_leading.power_chord_parallels_are_idiomatic",
      "title": "Successive power chords are parallel fifths by construction and must not be penalised.",
      "summary": "Successive power chords are parallel fifths by construction and must not be penalised.",
      "score": 21.0,
      "source_ids": ["open-music-theory"],
      "detail": {
        "id": "voice_leading.power_chord_parallels_are_idiomatic",
        "domain": "voice_leading",
        "kind": "style_sensitive_preference",
        "summary": "Successive power chords are parallel fifths by construction and must not be penalised.",
        "trigger": { "event": "voice_pair_motion", "voicing_family": "power" },
        "conditions": ["motion_is_parallel", "interval_is_perfect_fifth_or_octave"],
        "effect": { "score_delta": 0.0, "severity": "info", "score_component": "style_match" },
        "profiles": ["pop_rock", "blues", "…2 more"],
        "exceptions": ["strictness_is_common_practice"],
        "source_refs": [ { "source_id": "open-music-theory", "locator": "Blues harmony" } ],
        "confidence": 0.9,
        "rationale": "A power-chord riff is the clearest case in the whole rule set where a strong common-practice principle is simply the wrong tool.",
        "test_ids": ["power_chord_no_parallel_penalty"],
        "version": 1
      }
    },
    {
      "kind": "progression",
      "id": "prog_circle_of_fifths_diatonic",
      "title": "Diatonic circle of fifths",
      "summary": "Imaj7 - IVmaj7 - viiø7 - iii7 - vi7 - ii7 - V7 - Imaj7",
      "score": 17.0,
      "source_ids": ["open-music-theory", "mt21c"],
      "detail": { /* degrees, function_path, melody_compatibility, bass_implications, … */ }
    }
    // … 1 more
  ],
  "source_ids": ["mt21c", "open-music-theory"],
  "profiles": [],
  "warnings": []
}
```

---

### 3.4 `music.analyze_selection`

> Runs melody, key, phrase, salience, non-chord-tone and harmonic-grid analysis
> over a snapshot and returns an analysis id plus a ranked reading. Never
> asserts a single key; it ranks candidates and reports the evidence.

**Purpose.** Turn a snapshot into a *reading*. Everything downstream harmonizes
against this, not against raw notes.

**Arguments.**

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `snapshot_id` | string | **yes** | — | id pattern, 1..128 |
| `style_profile` | string | no | `common_practice` | the 10-profile enum |
| `melody_extraction` | object | no | `{mode:"auto"}` | as in [§3.2](#32-reaperinspect_selection) |
| `tonal_center` | object | no | inferred | `{tonic, scale_id}`, both required if present |
| `meter` | string | no | from the project | `^[0-9]{1,2}/[0-9]{1,2}$` |
| `harmonic_rhythm` | object | no | `{mode:"auto"}` | `mode`: `auto`\|`existing`\|`bars`\|`beats`; `value`: `0.0625..=64.0` |
| `strictness` | string | no | `balanced` | `exploratory` \| `balanced` \| `conservative` \| `common_practice_strict` |
| `loop_span` | object | no | none | `{start_qn, end_qn}`, both required if present, `-100000.0..1000000.0` |

`tonal_center.tonic` must parse as a pitch class (`C`, `F#`, `Bb`);
`tonal_center.scale_id` must exist in the bundle (`theory://catalog` lists them).
A hint **overrides** inference but is still scored, so you can see whether the
music agreed.

**Output.** `ok`, `analysis_id`, `snapshot_id`, `profile_id`, `resource_uri`,
`confidence`, `knowledge_version`, `key`, `phrases[]`, `motives[]`,
`structural_notes[]` (note indices), `nct_hypotheses[]`, `grid`, `melody`,
`extraction`, `detected_chords[]`, `loop_observations[]`, `warnings[]`.

- `key` is `{ambiguous, gap, candidates[], regions[]}`. `candidates[]` is
  **ranked**, each with `label`, `tonic`, `scale_id`, `score`, `confidence`,
  `is_modal` and a named `evidence[]` vector. `gap` is the margin between first
  and second; `ambiguous` is set when it is too small to call.
- `grid` is `{mode, rationale, slot_count, slots[]}`. `rationale` is prose
  explaining the harmonic rhythm it chose.
- `extraction` is `{mode_used, confidence, voice_count, melody_note_count,
  accompaniment_note_count, assumptions[]}`. This is where extraction confidence
  lives.
- `melody` is `{low, high, tessitura_low, tessitura_high, density,
  direction_changes, repeated_pitches, mean_interval, climax_note_id,
  leap_count}` — range and tessitura.
- The **full** report, several times larger, is at `analysis://{analysis_id}`.

**Errors.** `UNKNOWN_ID`, `EXPIRED_ID`, `INVALID_ARGUMENT` (bad tonic, unknown
scale, unparseable meter, unknown strictness), `INVALID_ARGUMENTS`,
`ANALYSIS_FAILED`, `CANCELLED`.

**Example.**

```json
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{
  "name":"music.analyze_selection",
  "arguments":{
    "snapshot_id":"00000000-0000-4000-8000-0000000000aa",
    "style_profile":"jazz_standard",
    "loop_span":{"start_qn":0.0,"end_qn":32.0}}}}
```

`structuredContent` (arrays elided after two entries; the real reply is ~48 kB):

```jsonc
{
  "ok": true,
  "analysis_id": "914030c1-9ad7-53b0-b59f-b2ad5bcbfc42",
  "snapshot_id": "00000000-0000-4000-8000-0000000000aa",
  "profile_id": "jazz_standard",
  "resource_uri": "analysis://914030c1-9ad7-53b0-b59f-b2ad5bcbfc42",
  "confidence": 0.807853,
  "knowledge_version": "1.0.0",
  "key": {
    "ambiguous": false,
    "gap": 0.448215,
    "candidates": [
      { "label": "C major", "tonic": "C", "scale_id": "major",
        "score": 6.670961, "confidence": 0.651175, "is_modal": false,
        "evidence": [ {"name":"pitch_distribution","value":1.0},
                      {"name":"collection_parsimony","value":1.0} /* …11 more */ ] },
      { "label": "C bebop_dominant", "tonic": "C", "scale_id": "bebop_dominant",
        "score": 6.222746, "confidence": 0.607423, "is_modal": true,
        "evidence": [ /* …13 */ ] }
      // … 6 more
    ],
    "regions": [
      { "start_qn": 0.0, "end_qn": 32.0, "scale_id": "major",
        "confidence": 0.566668, "is_tonicization": false,
        "evidence": ["phrase_edges=1.000", "pitch_distribution=1.000", "…1 more"] }
    ]
  },
  "phrases": [
    { "id": 0, "start_qn": 0.0,  "end_qn": 16.0, "note_count": 14,
      "cadence": "imperfect_authentic", "is_pickup": false, "confidence": 0.625 },
    { "id": 1, "start_qn": 16.0, "end_qn": 32.0, "note_count": 15,
      "cadence": "imperfect_authentic", "is_pickup": false, "confidence": 0.9 }
  ],
  "motives": [
    { "id": 0, "occurrences": 2, "salience": 0.6948275862068966,
      "interval_profile": [3, 5, "…2 more"], "transforms": ["exact", "varied"] }
    // … 1 more
  ],
  "structural_notes": [0, 13, "…1 more"],
  "nct_hypotheses": [
    { "note_id": 0, "kind": "chord_tone", "confidence": 0.9,
      "rationale": "sounds the 1 of the C in place" }
    // … 31 more
  ],
  "grid": {
    "mode": "auto",
    "rationale": "the jazz_standard profile prefers 2 chord(s) per bar, giving a base slot of 2 quarter notes; 7 slot(s) were merged because the melody gave no reason to change harmony; phrase boundaries were kept as harmonic boundaries",
    "slot_count": 9,
    "slots": [
      { "start_qn": 0.0, "end_qn": 4.0, "is_cadential": false,
        "weight": 0.554767, "melody_notes": [0, 1, "…2 more"] }
      // … 8 more
    ]
  },
  "melody": {
    "low": "C4", "high": "C5", "tessitura_low": 62, "tessitura_high": 71,
    "density": 3.625, "direction_changes": 11, "repeated_pitches": 0,
    "mean_interval": 2.9285714285714284, "climax_note_id": 13, "leap_count": 15
  },
  "extraction": {
    "mode_used": "monophonic_voice",
    "confidence": 0.92,
    "voice_count": 1,
    "melody_note_count": 29,
    "accompaniment_note_count": 0,
    "assumptions": [
      "every note in the take is selected, so nothing was narrowed",
      "the material is monophonic — no two notes ever sound together — so it is the melody as written"
    ]
  },
  "detected_chords": ["C", "Fadd9", "…7 more"],
  "loop_observations": [],
  "warnings": [
    { "code": "CHORDS_INFERRED_FROM_MELODY",
      "message": "the material is a single line, so the detected chords are what the melody implies rather than harmony that was played",
      "severity": "info" }
  ]
}
```

---

### 3.5 `harmony.generate_candidates`

> Generates genuinely different harmonizations of the analyzed melody, each with
> a full decision trace naming the theory rules and sources behind it.
> Preserves the melody and its timing unless told otherwise.

**Purpose.** The main event. Several structurally distinct harmonizations, each
scored on a visible vector and each explaining itself.

**Arguments.**

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `snapshot_id` | string | **yes** | — | id pattern |
| `analysis_id` | string | no | analyses the snapshot | id pattern; must belong to the same snapshot |
| `style_profile` | string | no | `common_practice` | the 10-profile enum |
| `candidate_count` | integer | no | `3` | `1..=8` |
| `preserve_melody` | boolean | no | `true` | |
| `preserve_rhythm` | boolean | no | `true` | |
| `harmonic_rhythm` | object | no | `{mode:"auto"}` | `mode`: `auto`\|`existing`\|`bars`\|`beats`; `value`: `0.0625..=64.0` |
| `complexity` | number | no | `0.5` | `0.0..=1.0` |
| `chromaticism` | number | no | `0.35` | `0.0..=1.0` |
| `extension_density` | number | no | `0.4` | `0.0..=1.0` |
| `bass_motion` | string | no | `auto` | `auto` \| `roots` \| `inversions` \| `stepwise` \| `pedal` \| `ostinato` \| `contrary_motion` |
| `countermelody` | object | no | disabled | `{enabled: bool, density: 0.0..=1.0, role: <arrangement role>}` |
| `loop_intent` | string | no | none | `closed_tonic` \| `open_dominant` \| `modal_drone` \| `seamless_color` \| `transition_ready` \| `one_shot_ending` |
| `strictness` | string | no | `balanced` | `exploratory` \| `balanced` \| `conservative` \| `common_practice_strict` |
| `voice_count` | integer | no | `4` | `2..=8` |
| `seed` | integer | no | `20260726` | `0..=i64::MAX` |

`countermelody.role` accepts any of the 16 arrangement roles: `lead`,
`counterlead`, `bass`, `harmonic_bed`, `pad`, `comping`, `pulse`, `ostinato`,
`riff`, `percussion`, `impact`, `transition`, `texture`, `ambience`, `ornament`,
`ear_candy`.

If you omit `analysis_id`, the server reuses an analysis already computed for
that snapshot under the same profile, or runs one. If you supply one that
belongs to a *different* snapshot, the call is refused with `INVALID_ARGUMENT`
rather than silently harmonizing one piece of music against another's reading.

**Output.** `ok`, `snapshot_id`, `analysis_id`, `profile_id`, `seed`, `cached`,
`candidate_count`, `candidates[]`, `knowledge_version`, `warnings[]`.

Each candidate: `candidate_id`, `kind` (`harmonization`), `label`, `strategy`,
`resource_uri`, `trace_uri`, `chord_count`, `note_count`, `chords[]` (symbols),
`confidence`, `score_total`, `score_components[]` (`{name, value}` for all 13
components), `rule_ids[]`, `source_ids[]`, `loop` (object or null),
`explanation` (prose), `parts[]` (`"role:Name"` strings).

The 13 score components are `melody_fit`, `harmonic_coherence`,
`functional_or_modal_coherence`, `voice_leading`, `extension_appropriateness`,
`style_match`, `phrase_direction`, `bass_quality`, `arrangement_clarity`,
`loop_compatibility`, `complexity_target`, `chromaticism_target`,
`candidate_diversity`. `score_total` is their profile-weighted sum; the weights
are readable at `theory://profiles/{id}`.

**Errors.** `UNKNOWN_ID`, `EXPIRED_ID`, `INVALID_ARGUMENT` (bad enum member,
mismatched `analysis_id`), `INVALID_ARGUMENTS`, `GENERATION_FAILED`,
`ANALYSIS_FAILED`, `CANCELLED`.

**Example — the §33 request.**

```json
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{
  "name":"harmony.generate_candidates",
  "arguments":{
    "snapshot_id":"00000000-0000-4000-8000-0000000000aa",
    "analysis_id":"914030c1-9ad7-53b0-b59f-b2ad5bcbfc42",
    "style_profile":"jazz_standard",
    "candidate_count":3,
    "preserve_melody":true,
    "preserve_rhythm":true,
    "complexity":0.65,
    "chromaticism":0.35,
    "extension_density":0.55,
    "bass_motion":"auto",
    "countermelody":{"enabled":true,"density":0.25},
    "loop_intent":"closed_tonic",
    "strictness":"balanced",
    "seed":12345},
  "_meta":{"progressToken":"gen-1"}}}
```

`structuredContent`, first candidate in full, the other two summarised
(the real reply is ~28 kB):

```jsonc
{
  "ok": true,
  "snapshot_id": "00000000-0000-4000-8000-0000000000aa",
  "analysis_id": "914030c1-9ad7-53b0-b59f-b2ad5bcbfc42",
  "profile_id": "jazz_standard",
  "seed": 12345,
  "cached": false,
  "candidate_count": 3,
  "candidates": [
    {
      "candidate_id": "c8dff250-e434-503c-be52-c132279f75cb",
      "kind": "harmonization",
      "label": "Functional & cadence-directed",
      "strategy": "functional",
      "resource_uri": "candidate://c8dff250-e434-503c-be52-c132279f75cb",
      "trace_uri": "candidate://c8dff250-e434-503c-be52-c132279f75cb/trace",
      "chord_count": 9,
      "note_count": 83,
      "chords": ["Am11","A9(13)","Dm11","A9(13)","D9(13)","E9(13)","Am11","Dm7","Am11"],
      "confidence": 0.780111,
      "score_total": 0.8056194220207943,
      "score_components": [
        { "name": "melody_fit",                    "value": 0.7759840548317621 },
        { "name": "harmonic_coherence",            "value": 1.0025605833333335 },
        { "name": "functional_or_modal_coherence", "value": 1.446879871111111  },
        { "name": "voice_leading",                 "value": 1.0635802469135802 },
        { "name": "extension_appropriateness",     "value": 0.6757362013888889 },
        { "name": "style_match",                   "value": 0.66102400875      },
        { "name": "phrase_direction",              "value": 0.4444444444444444 },
        { "name": "bass_quality",                  "value": 1.1768582161111114 },
        { "name": "arrangement_clarity",           "value": 0.36922725         },
        { "name": "loop_compatibility",            "value": 0.36922725         },
        { "name": "complexity_target",             "value": 0.5647998844444444 },
        { "name": "chromaticism_target",           "value": 0.5701681828703705 },
        { "name": "candidate_diversity",           "value": 1.0                }
      ],
      "rule_ids": [
        "extensions.avoid_tone_is_profile_sensitive",
        "extensions.fifth_is_omitted_first",
        "extensions.guide_tones_have_highest_retention_priority",
        "harmony.ii_v_i_is_preferred_cadential_path",
        "harmony.melody_note_must_be_chord_tone_or_classified",
        "voice_leading.chordal_seventh_resolves_down",
        "voice_leading.minimal_total_motion"
        // … 16 more
      ],
      "source_ids": ["mt21c", "mt21c-mode-mixture", "open-music-theory"],
      "loop": null,
      "explanation": "In C major under the jazz_standard profile, this candidate is the functional & cadence-directed reading: Am11 | A9(13) | Dm11 | A9(13) | D9(13) | E9(13) | Am11 | Dm7 | Am11. In roman numerals that is vi11 V13/ii ii11 V13/ii V13/V V13/vi vi11 ii4/3 vi11. It is shaped by extensions.guide_tones_have_highest_retention_priority (+3.75), harmony.ii_v_i_is_preferred_cadential_path (+3.50), voice_leading.chordal_seventh_resolves_down (+3.00). 3 rules were set aside here because the style or the context makes them exception: extensions.avoid_tone_is_profile_sensitive, extensions.fifth_is_omitted_first, harmony.chromatic_mediant_is_not_diatonic. The realisation moves 83 semitones in total with a largest leap of 9.",
      "parts": ["lead:Melody", "harmonic_bed:Harmony", "bass:Bass", "counterlead:Countermelody"]
    }
    // d0a75ab5-9d2b-5665-8cfb-0f67c9a9fc92  modal_common_tone   "Modal & common-tone"
    //   0.7574  Am11 Am11 Dm11 Am11 D9(13) E9(13) Am11 D9(13) Am11
    // a94d41ef-ca8a-56e7-be80-a2c94427db41  chromatic_bass_led  "Chromatic & bass-led"
    //   0.7612  Fm11 Gm11 Bb9(13) C7#9#11 Fm11 Bb9(13) C7#9#11 Fm11 Bb9(13)
  ],
  "knowledge_version": "1.0.0",
  "warnings": []
}
```

Note the three `strategy` values are genuinely distinct — `functional`,
`modal_common_tone`, `chromatic_bass_led` — and every candidate keeps
`lead:Melody` and adds `bass:Bass` and `counterlead:Countermelody` as requested.
`the_acceptance_workflow_runs_end_to_end` asserts exactly this: three unique
strategies, a `lead:` part on every candidate, and non-empty `chords`,
`score_components` and `rule_ids`.

---

### 3.6 `harmony.reharmonize`

> Reharmonizes the chords detected in a snapshot, or a generated candidate's
> chords, under explicit preservation constraints.

**Purpose.** Start from harmony that already exists — detected in the source or
produced by an earlier candidate — and transform it while holding named things
still.

**Arguments.**

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `snapshot_id` | string | conditional | — | id pattern; required unless `candidate_id` is given |
| `analysis_id` | string | no | analyses the snapshot | id pattern |
| `candidate_id` | string | no | — | id pattern; when present, the candidate's chords are the input and its snapshot and analysis are used |
| `style_profile` | string | no | `common_practice` | the 10-profile enum |
| `candidate_count` | integer | no | `3` | `1..=8` |
| `preserve_melody` | boolean | no | `true` | |
| `preserve_bass` | boolean | no | `false` | |
| `preserve_cadence` | boolean | no | `true` | |
| `preserve_harmonic_rhythm` | boolean | no | `true` | |
| `families` | array of string | no | any | allowed chord/transformation family ids; empty means any |
| `complexity` | number | no | `0.6` | `0.0..=1.0` |
| `chromaticism` | number | no | `0.5` | `0.0..=1.0` |
| `seed` | integer | no | `20260726` | `0..=i64::MAX` |

`families` is *not* a closed enum here — it is matched against the family ids in
the knowledge bundle, and an id nothing matches simply narrows the pool to
nothing.

**Output.** Identical in shape to `harmony.generate_candidates`. Candidate
`kind` is `reharmonization`.

**Errors.** `UNKNOWN_ID`, `EXPIRED_ID`, `INVALID_ARGUMENT` — including
`"there is no existing harmony to reharmonize"` when the analysis detected no
chords and no `candidate_id` was given — `INVALID_ARGUMENTS`,
`GENERATION_FAILED`, `CANCELLED`.

**Example.**

```json
{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{
  "name":"harmony.reharmonize",
  "arguments":{
    "snapshot_id":"00000000-0000-4000-8000-0000000000aa",
    "analysis_id":"914030c1-9ad7-53b0-b59f-b2ad5bcbfc42",
    "style_profile":"jazz_standard",
    "candidate_count":2,
    "preserve_melody":true,
    "preserve_cadence":true,
    "seed":7}}}
```

`structuredContent` (candidate bodies elided to the fields that differ):

```jsonc
{
  "ok": true,
  "snapshot_id": "00000000-0000-4000-8000-0000000000aa",
  "analysis_id": "914030c1-9ad7-53b0-b59f-b2ad5bcbfc42",
  "profile_id": "jazz_standard",
  "seed": 7,
  "cached": false,
  "candidate_count": 2,
  "candidates": [
    {
      "candidate_id": "e8bc6b0c-b01f-567d-8f5b-4f9b2f9af56a",
      "kind": "reharmonization",
      "label": "Functional & cadence-directed",
      "strategy": "functional",
      "chords": ["Cmaj9","Am9","Dm9","Cmaj9","Bb9","Cmaj9","C9","Fm9","Cmaj9"],
      "score_total": 0.9204920562129549,
      "parts": ["lead:Melody", "harmonic_bed:Harmony", "bass:Bass"],
      "explanation": "In C major under the jazz_standard profile, this candidate is the functional & cadence-directed reading: Cmaj9 | Am9 | Dm9 | Cmaj9 | Bb9 | Cmaj9 | C9 | Fm9 | Cmaj9. In roman numerals that is Imaj9 vi9 ii9 Imaj9 bVII9 Imaj9 V9/IV iv9 Imaj9. …"
      /* candidate_id, resource_uri, trace_uri, chord_count, note_count,
         confidence, score_components[13], rule_ids[], source_ids[], loop */
    },
    {
      "candidate_id": "35a219f4-57f0-54d9-a1e6-b04b852b4784",
      "kind": "reharmonization",
      "label": "Modal & common-tone",
      "strategy": "modal_common_tone",
      "chords": ["Cmaj9","Am9","Dm9","C9","Am9","Cmaj9","C9","Fm9","Cmaj9"],
      "score_total": 0.869608887699875
    }
  ],
  "knowledge_version": "1.0.0",
  "warnings": []
}
```

---

### 3.7 `voicing.generate`

> Re-voices a generated candidate's chords across a set of voicing families,
> keeping voice identity across time and reporting the voice-leading audit for
> each variant.

**Purpose.** Same chords, different spacing. One new candidate per voicing
family, each carrying its own voice-leading audit.

**Arguments.**

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `candidate_id` | string | **yes** | — | id pattern |
| `style_profile` | string | no | **the source candidate's profile** | the 10-profile enum |
| `voice_count` | integer | no | `4` | `2..=8` |
| `families` | array of string | no | `["close","drop2","shell"]` | items from `close`, `open`, `drop2`, `drop3`, `shell`, `rootless`, `spread`, `quartal`, `quintal`, `cluster`, `power`, `upper_structure`, `pedal` |
| `low` | integer | no | `48` (C3) | `0..=127` |
| `high` | integer | no | `84` (C6) | `0..=127` |
| `preserve_top` | boolean | no | `true` | keeps the melody as the top voice |
| `preserve_bass` | boolean | no | `false` | |
| `max_leap` | integer | no | `12` | `1..=36`, semitones |
| `instrument_profile` | string | no | `piano_keys` | an instrument-profile id from the bundle |
| `candidate_count` | integer | no | number of `families` | `1..=8` |
| `seed` | integer | no | `20260726` | `0..=i64::MAX` |

> **Deviation from brief §15.** The brief describes this tool as generating
> voicings "for a candidate **or chord sequence**". The built tool takes a
> server-issued `candidate_id` only — there is no argument that accepts a raw
> chord list. That is deliberate: accepting arbitrary chord input would be a
> second, unvalidated path into the generation engine and into staging. Reach a
> raw progression by inspecting and analyzing it first, or by generating a
> candidate from it with `harmony.reharmonize`.

**Output.** Identical in shape to `harmony.generate_candidates`. Candidate
`kind` is `voicing`, `strategy` is `voicing_<family>`, and `explanation` is the
voice-leading audit in prose. One candidate is produced per requested family, in
the order you listed them, up to `candidate_count`.

A voicing candidate's score is the **voice-leading report's own** score vector
rather than a fresh harmonic evaluation — the chords were not re-chosen — so the
harmonic components (`melody_fit`, `harmonic_coherence`, …) read `0.0` and
`score_total` is correspondingly lower. Voicing scores are comparable to each
other and **not** to a harmonization's.

**Errors.** `UNKNOWN_ID`, `EXPIRED_ID`, `INVALID_ARGUMENT` (a range that cannot
hold the requested voices, unknown instrument profile), `INVALID_ARGUMENTS`,
`GENERATION_FAILED`, `CANCELLED`.

**Example.**

```json
{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{
  "name":"voicing.generate",
  "arguments":{
    "candidate_id":"c8dff250-e434-503c-be52-c132279f75cb",
    "style_profile":"jazz_standard",
    "voice_count":4,
    "preserve_top":true,
    "families":["close","drop2"],
    "seed":3}}}
```

`structuredContent` (elided):

```jsonc
{
  "ok": true,
  "snapshot_id": "00000000-0000-4000-8000-0000000000aa",
  "analysis_id": "914030c1-9ad7-53b0-b59f-b2ad5bcbfc42",
  "profile_id": "jazz_standard",
  "seed": 3,
  "cached": false,
  "candidate_count": 2,
  "candidates": [
    {
      "candidate_id": "b636fcb6-ab5a-5e3b-81e8-fca009bfbd7f",
      "kind": "voicing",
      "label": "close voicing",
      "strategy": "voicing_close",
      "chord_count": 9,
      "note_count": 72,
      "score_total": 0.2726530612244899,
      "explanation": "close voicing of candidate c8dff250-e434-503c-be52-c132279f75cb: 81 semitones of total motion, largest leap 8, 1 parallel perfect interval(s), 0 crossing(s), 4 overlap(s).",
      "rule_ids": ["counterpoint.avoid_simultaneous_leaps",
                   "counterpoint.prefer_contrary_motion", "…7 more"],
      "parts": ["lead:Melody", "bass:Bass", "…2 more"]
    },
    {
      "candidate_id": "72dffe38-a53b-5e05-8a56-e7f1123446d5",
      "kind": "voicing",
      "label": "drop2 voicing",
      "strategy": "voicing_drop2",
      "note_count": 71,
      "score_total": 0.31727891156462595,
      "explanation": "drop2 voicing of candidate c8dff250-e434-503c-be52-c132279f75cb: 71 semitones of total motion, largest leap 8, 1 parallel perfect interval(s), 0 crossing(s), 3 overlap(s)."
    }
  ],
  "knowledge_version": "1.0.0",
  "warnings": []
}
```

> **Reading `parallels` carefully.** The voice-leading report's `parallels` list
> excludes zero-interval motion, so **parallel unisons do not appear in it**,
> even though `counterpoint.no_parallel_unisons` fires and penalises them. The
> rule applications, not the `parallels` array, are the complete record. See
> [`KNOWN_LIMITATIONS.md`](KNOWN_LIMITATIONS.md) §20.

---

### 3.8 `arrangement.generate`

> Turns a harmonization candidate into role-based parts — bass, pad, comping,
> pulse and the rest — respecting instrument ranges, an energy curve and a
> masking budget.

**Purpose.** Take one candidate and lay it out as playable parts with an energy
shape, then report where the parts collide.

**Arguments.**

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `candidate_id` | string | **yes** | — | id pattern |
| `style_profile` | string | no | **the source candidate's profile** | the 10-profile enum |
| `roles` | array of string | no | the profile's own set | items from the 16 arrangement roles |
| `energy_curve` | array of object | no | derived | `[{qn, value}]`, both required; `value` `0.0..=1.0` |
| `density` | number | no | `0.5` | `0.0..=1.0` |
| `texture_pattern` | string | no | none | must be an arrangement-pattern id in the bundle |
| `register_spread` | number | no | `0.5` | `0.0..=1.0` |
| `sections` | array of object | no | one section | `[{id, start_qn, end_qn, role?, energy?}]`; `end_qn` must exceed `start_qn` |
| `preserve_melody` | boolean | no | `true` | |
| `loop_intent` | string | no | none | the 6-intent enum |
| `seed` | integer | no | `20260726` | `0..=i64::MAX` |

The 16 roles: `lead`, `counterlead`, `bass`, `harmonic_bed`, `pad`, `comping`,
`pulse`, `ostinato`, `riff`, `percussion`, `impact`, `transition`, `texture`,
`ambience`, `ornament`, `ear_candy`. Four of them (`pulse`, `percussion`,
`ornament`, `ear_candy`) have no catalogue pattern of their own and are served
by documented substitution — disclosed in the assignment's `rationale`.

**Output.** `ok`, `candidate_id` (a **new** candidate), `source_candidate_id`,
`resource_uri`, `trace_uri`, `profile_id`, `seed`, `parts[]`, `assignments[]`,
`energy[]`, `masking`, `score_total`, `note_count`, `knowledge_version`,
`warnings[]`.

- `parts[]`: `{role, name, note_count, low_pitch, high_pitch,
  instrument_profile, polyphonic}`.
- `assignments[]`: `{role, pattern_id, instrument_profile, register_low,
  register_high, density, polyphony, priority, sections[], rationale}`.
- `masking`: `{onset_collisions_before, onset_collisions_after,
  register_overlap_before, register_overlap_after, suggestions[]}`.

The result is itself a candidate, so it can be explained, loop-audited and
staged like any other.

**Errors.** `UNKNOWN_ID`, `EXPIRED_ID`, `INVALID_ARGUMENT` (unknown role,
unknown pattern id, a section that ends before it starts, an energy point
missing `qn` or `value`), `INVALID_ARGUMENTS`, `ARRANGEMENT_FAILED`,
`CANCELLED`.

**Example.**

```json
{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{
  "name":"arrangement.generate",
  "arguments":{
    "candidate_id":"c8dff250-e434-503c-be52-c132279f75cb",
    "style_profile":"jazz_standard",
    "roles":["bass","pad","comping"],
    "seed":5}}}
```

`structuredContent` (third part and assignment elided):

```jsonc
{
  "ok": true,
  "candidate_id": "668aec6e-16ac-5670-92d8-5cfcb494b47d",
  "source_candidate_id": "c8dff250-e434-503c-be52-c132279f75cb",
  "resource_uri": "candidate://668aec6e-16ac-5670-92d8-5cfcb494b47d",
  "trace_uri": "candidate://668aec6e-16ac-5670-92d8-5cfcb494b47d/trace",
  "profile_id": "jazz_standard",
  "seed": 5,
  "parts": [
    { "role": "bass", "name": "bass — Walking bass", "note_count": 23,
      "low_pitch": 42, "high_pitch": 60,
      "instrument_profile": "bass", "polyphonic": false },
    { "role": "pad", "name": "pad — Sustained pad", "note_count": 38,
      "low_pitch": 69, "high_pitch": 96,
      "instrument_profile": "pad", "polyphonic": true }
    // … 1 more
  ],
  "assignments": [
    { "role": "bass", "pattern_id": "arr_walking_bass", "instrument_profile": "bass",
      "register_low": 36, "register_high": 56, "density": 0.71875,
      "polyphony": 1, "priority": 1, "sections": ["section_1", "section_2"],
      "rationale": "Walking bass realises bass on Bass in MIDI 33..60: on_grid activity, root_and_line responsibility, midground priority" },
    { "role": "pad", "pattern_id": "arr_sustained_pad", "instrument_profile": "pad",
      "register_low": 68, "register_high": 96, "density": 0.25,
      "polyphony": 5, "priority": 2, "sections": ["section_1", "section_2"],
      "rationale": "Sustained pad realises pad on Sustained pad in MIDI 48..88: sustained activity, full responsibility, background priority" }
    // … 1 more
  ],
  "energy": [
    { "qn": 0.0, "value": 0.35 },
    { "qn": 4.0, "value": 0.38749999999999996 }
    // … 7 more
  ],
  "masking": {
    "onset_collisions_before": 76,
    "onset_collisions_after": 76,
    "register_overlap_before": 0.5135135135135135,
    "register_overlap_after": 0.2459016393442623,
    "suggestions": [
      "onset-density control: thin the background parts off the foreground's onsets",
      "contrary rhythmic activity: give a background part an offbeat or complementary grid"
      // … 4 more
    ]
  },
  "score_total": 0.22491264265268954,
  "note_count": 137,
  "knowledge_version": "1.0.0",
  "warnings": []
}
```

---

### 3.9 `loop.audit`

> Audits the wrap point of a loop: harmonic, bass and voice-leading continuity,
> hanging notes, pickup and tail behaviour, and concrete repairs. Works on a
> snapshot, an analysis or a generated candidate.

**Purpose.** Answer "does this loop?" with reasons and fixes, not a verdict.

**Arguments.**

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `snapshot_id` | string | conditional | — | id pattern |
| `analysis_id` | string | conditional | — | id pattern |
| `candidate_id` | string | conditional | — | id pattern |
| `transaction_id` | string | conditional | — | id pattern; resolves to the candidate that transaction staged |
| `style_profile` | string | no | `common_practice` | the 10-profile enum |
| `loop_intent` | string | no | `closed_tonic` | the 6-intent enum |
| `loop_span` | object | no | the material's own span | `{start_qn, end_qn}`, both required if present |

**Target resolution order**, first match wins: `candidate_id` → `transaction_id`
(→ its candidate) → `analysis_id` → `snapshot_id`. Supply at least one; with
none, the tool falls through to `snapshot_id` and fails with `INVALID_ARGUMENT`.
This is how the brief's "audit a **staged** candidate" is served — a transaction
resolves to the candidate it staged, so there is no separate code path.

**Output.**

| Field | Type | Meaning |
|---|---|---|
| `ok` | `true` | |
| `target_kind` | string | `candidate` \| `analysis` \| `snapshot` |
| `target_id` | string \| null | |
| `intent` | string \| null | The intent it audited against. |
| `loop_start_qn`, `loop_end_qn` | number | |
| `compatible` | boolean | The overall verdict. |
| `score` | number | |
| `confidence` | number | |
| `harmonic_wrap` | string | Prose. |
| `bass_wrap` | string | Prose. |
| `voice_leading_wrap` | string | Prose. |
| `hanging_notes` | array of integer | Note indices sounding across the wrap. |
| `crossing_notes` | array of integer | |
| `pickup_qn`, `tail_qn` | number | |
| `length_exact` | boolean | Whether the span is a whole number of bars. |
| `findings` | array | `{code, message, severity}`. |
| `repairs` | array | `{id, description, severity, applies_to[], rationale, rule_ids[]}`. |
| `warnings` | array | Mirrors `findings`. |

A `modal_drone` loop does not need a dominant resolution, and the audit does not
demand one — the intent changes what counts as a finding.

**Errors.** `UNKNOWN_ID`, `EXPIRED_ID`, `UNKNOWN_TRANSACTION`,
`INVALID_ARGUMENT` (unknown intent, no target), `INVALID_ARGUMENTS`,
`LOOP_AUDIT_FAILED`, `CANCELLED`.

**Example.**

```json
{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{
  "name":"loop.audit",
  "arguments":{
    "candidate_id":"c8dff250-e434-503c-be52-c132279f75cb",
    "loop_intent":"closed_tonic",
    "loop_span":{"start_qn":0.0,"end_qn":32.0}}}}
```

`structuredContent`, complete:

```json
{
  "ok": true,
  "target_kind": "candidate",
  "target_id": "c8dff250-e434-503c-be52-c132279f75cb",
  "intent": "closed_tonic",
  "loop_start_qn": 0.0,
  "loop_end_qn": 32.0,
  "compatible": true,
  "score": 0.69742,
  "confidence": 0.930235,
  "harmonic_wrap": "Am11 (tonic) to Am11 (tonic): already closed before the wrap",
  "bass_wrap": "A2 to E2, -5 semitones, by fifth",
  "voice_leading_wrap": "5 voices into 6, total motion 0 semitones, largest upper-voice move 0, 4 common pitch classes, one held in the same voice",
  "hanging_notes": [],
  "crossing_notes": [],
  "pickup_qn": 0.0,
  "tail_qn": 0.0,
  "length_exact": true,
  "findings": [
    {
      "code": "LOOP_UNRESOLVED_TENDENCY",
      "message": "the chordal seventh of Am11 does not fall by step across the wrap",
      "severity": "minor"
    },
    {
      "code": "LOOP_HARMONIC_RHYTHM_CHANGE",
      "message": "the last slot lasts 2 but the first lasts 4, so the loop stumbles at the wrap",
      "severity": "moderate"
    }
  ],
  "repairs": [
    {
      "id": "equalise_the_harmonic_rhythm_at_the_wrap",
      "description": "give the last slot the same length as the first, or accept the change as a cadential arrival",
      "severity": "moderate",
      "applies_to": [],
      "rationale": "the last slot being half the length of the first is a common artefact of generating a turnaround, and it is audible immediately on repeat",
      "rule_ids": ["looping.harmonic_rhythm_stable_at_wrap"]
    }
  ],
  "warnings": [
    {
      "code": "LOOP_UNRESOLVED_TENDENCY",
      "message": "the chordal seventh of Am11 does not fall by step across the wrap",
      "severity": "minor"
    },
    {
      "code": "LOOP_HARMONIC_RHYTHM_CHANGE",
      "message": "the last slot lasts 2 but the first lasts 4, so the loop stumbles at the wrap",
      "severity": "moderate"
    }
  ]
}
```

---

### 3.10 `candidate.explain`

> Returns the decision trace behind a server-issued candidate: score components,
> the theory rules that fired, the sources behind them, the assumptions made and
> the alternatives rejected.

**Purpose.** Make a candidate accountable. Every rule id is real, every source id
resolves to a registry entry.

**Arguments.**

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `candidate_id` | string | **yes** | — | id pattern |
| `detail` | string | no | `detailed` | `concise` \| `detailed` |

`concise` caps `rules` at the **8** strongest applications; `detailed` returns
all of them. Nothing else differs — `score`, `sources`, `assumptions` and
`rejected_alternatives` are the same either way. Rules are sorted by absolute
`score_delta` descending, then by `rule_id`, so the order never depends on a
hash iteration. Anything the summary leaves out is at
`candidate://{id}/trace`.

**Output.** `ok`, `candidate_id`, `detail`, `label`, `strategy`, `snapshot_id`,
`analysis_id`, `profile_id`, `seed`, `knowledge_version`, `confidence`,
`explanation`, `assumptions[]`, `score`, `rules[]`, `sources[]`,
`rejected_alternatives[]`, `chords[]`, `trace_uri`, `warnings[]`.

- `score` is `{total, components[]}` where each component is
  `{name, value, weight, weighted}` — the raw value, the profile weight, and the
  product. The weights sum to 1.
- Each rule: `{rule_id, status, score_delta, score_component, domain, kind,
  summary, explanation, matched_conditions[], matched_exceptions[],
  source_ids[], resolved}`. `status` is `applied` or `exception`; `resolved`
  says whether every cited source id was found in the registry.
- Each source: `{source_id, title, authors[], publisher, url, license,
  source_type}`.
- `rejected_alternatives[]` are compact path strings —
  `Chord/melody-note-index>Chord/…` — naming the harmonisations the diversifier
  discarded.

**Errors.** `UNKNOWN_ID`, `EXPIRED_ID`, `INVALID_ARGUMENT`,
`INVALID_ARGUMENTS`, `CANCELLED`.

**Example.**

```json
{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{
  "name":"candidate.explain",
  "arguments":{"candidate_id":"c8dff250-e434-503c-be52-c132279f75cb",
               "detail":"detailed"}}}
```

`structuredContent` (25 rules and 13 components elided to two each; the real
reply is ~57 kB):

```jsonc
{
  "ok": true,
  "candidate_id": "c8dff250-e434-503c-be52-c132279f75cb",
  "detail": "detailed",
  "label": "Functional & cadence-directed",
  "strategy": "functional",
  "snapshot_id": "00000000-0000-4000-8000-0000000000aa",
  "analysis_id": "914030c1-9ad7-53b0-b59f-b2ad5bcbfc42",
  "profile_id": "jazz_standard",
  "seed": 12345,
  "knowledge_version": "1.0.0",
  "confidence": 0.780111,
  "explanation": "In C major under the jazz_standard profile, this candidate is the functional & cadence-directed reading: Am11 | A9(13) | Dm11 | A9(13) | D9(13) | E9(13) | Am11 | Dm7 | Am11. …",
  "assumptions": [
    "the passage was read as C major with confidence 0.65",
    "every note in the take is selected, so nothing was narrowed"
    // … 3 more
  ],
  "score": {
    "total": 0.8056194220207943,
    "components": [
      { "name": "melody_fit", "value": 0.7759840548317621,
        "weight": 0.08843537414965986, "weighted": 0.06862444022321706 },
      { "name": "harmonic_coherence", "value": 1.0025605833333335,
        "weight": 0.08843537414965986, "weighted": 0.0886618202947846 }
      // … 11 more
    ]
  },
  "rules": [
    {
      "rule_id": "extensions.guide_tones_have_highest_retention_priority",
      "status": "applied",
      "score_delta": 3.75,
      "score_component": "extension_appropriateness",
      "domain": "extensions",
      "kind": "theory_default",
      "summary": "When voices are scarce, the third and seventh are kept before any other chord member.",
      "explanation": "When voices are scarce, the third and seventh are kept before any other chord member. Here, both guide tones are sounding. Credited +3.75 to extension appropriateness.",
      "matched_conditions": ["guide_tones_present"],
      "matched_exceptions": [],
      "source_ids": ["open-music-theory"],
      "resolved": true
    },
    {
      "rule_id": "harmony.ii_v_i_is_preferred_cadential_path",
      "status": "applied",
      "score_delta": 3.5,
      "score_component": "functional_or_modal_coherence",
      "domain": "harmony",
      "kind": "theory_default",
      "summary": "A predominant, dominant, tonic path is the strongest available approach to a cadence.",
      "explanation": "A predominant, dominant, tonic path is the strongest available approach to a cadence. Here, the chord functions as a predominant and phrase analysis marks this slot as a cadential arrival. Credited +3.50 to functional or modal coherence.",
      "matched_conditions": ["function_is_predominant", "cadence_is_expected_at_this_slot"],
      "matched_exceptions": [],
      "source_ids": ["open-music-theory"],
      "resolved": true
    }
    // … 23 more
  ],
  "sources": [
    { "source_id": "mt21c",
      "title": "Music Theory for the 21st-Century Classroom",
      "authors": [], "publisher": "University of Puget Sound",
      "url": "https://musictheory.pugetsound.edu/mt21c/MusicTheory.html",
      "license": "unverified-reference-only", "source_type": "open_textbook" }
    // … 2 more
  ],
  "rejected_alternatives": [
    "Am11/9>A9(13)/9>Dm11/2>C/G/7>Am11/9>E9(13)/4>Am11/9>D9(13)/2>Am11/9",
    "Am11/9>A9(13)/9>Dm11/2>C/G/7>Am11/9>E9(13)/4>Am11/9>Dm7/9>Am11/9"
    // … 2 more
  ],
  "chords": ["Am11", "A9(13)", "…7 more"],
  "trace_uri": "candidate://c8dff250-e434-503c-be52-c132279f75cb/trace",
  "warnings": [
    { "code": "UNRESOLVED_TENDENCY",
      "message": "9 tendency tones are left unresolved",
      "severity": "minor" }
  ]
}
```

---

### 3.11 `reaper.stage_candidate`

> Writes a server-issued candidate into REAPER as new, muted, tagged tracks in
> their own folder. The source item is never touched. The plan carries the full
> precondition set, so material edited since the snapshot is rejected rather
> than overwritten.

**Purpose.** The **only** tool in the server that writes into a user's project.
It accepts a candidate id and layout preferences — nothing else. There is no
argument that carries notes, a track name to delete, or a project chunk. The
plan is built server-side from the stored candidate.

**Arguments.**

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `candidate_id` | string | **yes** | — | id pattern |
| `track_name_prefix` | string | no | `"QLabs "` | ≤ 64 chars |
| `folder_name` | string | no | `"QLabs Candidate <first 8 of candidate id>"` | ≤ 128 chars |
| `muted` | boolean | no | `true` | |
| `create_region` | boolean | no | `false` | |
| `route_to_source_track` | boolean | no | `false` | creates one validated MIDI send |
| `verify_snapshot` | boolean | no | `true` | |

Defaults, restated because they are the safety posture: **do not modify the
source; create new tracks; create no send unless you ask; tag every object;
one candidate per folder; mute everything.**

**Output.**

| Field | Type | Meaning |
|---|---|---|
| `ok` | `true` | |
| `transaction_id` | string | Pass to commit / discard / undo. |
| `plan_id` | string | Readable at `editplan://{plan_id}`. |
| `candidate_id`, `snapshot_id` | string | Provenance. |
| `undo_label` | string | Always begins `QLabs MCP: `. |
| `status` | string | `preview` after staging. |
| `tracks[]` | array | `{temp_id, kind, guid, name}` as REAPER created them. |
| `items[]` | array | `{temp_id, guid, take_guid, note_count, start_qn, end_qn}`. |
| `regions[]` | array | `{temp_id, marker_index, name}`. |
| `sends[]` | array | `{temp_id, send_index, to_track_guid}`. |
| `note_count` | integer | Notes written across the transaction. |
| `project_state_change_count` | integer | REAPER's counter after the edit. |
| `scope_echoed` | object | The scope block the bridge was asked to re-derive with. |
| `precondition_kinds[]` | array of string | The seven, listed below. |
| `plan_uri`, `transaction_uri` | string | |
| `warnings[]` | array | |

**The seven preconditions**, all always present:

```
project_uuid · state_change_count · item_guid_exists · take_guid_exists
midi_hash · tempo_map_hash · item_bounds
```

The bridge evaluates them before touching anything and refuses the whole plan if
any fails. `state_change_count` is the loosest of them — REAPER's counter
increments on any project edit, **including a mere selection change** — so a
benign interaction between generating and staging can produce a spurious
`PROJECT_CHANGED` / `STALE_SNAPSHOT`. The safe response is to re-inspect and
regenerate. See [`KNOWN_LIMITATIONS.md`](KNOWN_LIMITATIONS.md) §19.

**Errors.** `UNKNOWN_ID`, `EXPIRED_ID`, `BRIDGE_OFFLINE`, `IPC_TIMEOUT`,
`STALE_SNAPSHOT`, `PROJECT_CHANGED`, `MIDI_CHANGED`, `TEMPO_MAP_CHANGED`,
`SOURCE_ITEM_MISSING`, `SOURCE_TAKE_MISSING`, `INVALID_EDIT_PLAN`,
`KNOWLEDGE_INVALID`, `NO_ACTIVE_PROJECT`, `INVALID_ARGUMENTS`, `CANCELLED`,
`INTERNAL_BRIDGE_ERROR`.

**Example.**

```json
{"jsonrpc":"2.0","id":19,"method":"tools/call","params":{
  "name":"reaper.stage_candidate",
  "arguments":{"candidate_id":"c8dff250-e434-503c-be52-c132279f75cb",
               "create_region":true},
  "_meta":{"progressToken":"stage-1"}}}
```

`structuredContent`, complete:

```json
{
  "ok": true,
  "transaction_id": "f5155949-9309-4f56-8073-8df3daa774fa",
  "plan_id": "2ffc612c-11d5-4db6-ba1c-30a1037464b4",
  "candidate_id": "c8dff250-e434-503c-be52-c132279f75cb",
  "snapshot_id": "00000000-0000-4000-8000-0000000000aa",
  "undo_label": "QLabs MCP: Stage candidate f5155949",
  "status": "preview",
  "tracks": [
    { "guid": "…", "kind": "create_folder_track", "name": "QLabs Candidate c8dff250", "temp_id": "f0" },
    { "guid": "…", "kind": "create_track", "name": "QLabsMelody",        "temp_id": "t0" },
    { "guid": "…", "kind": "create_track", "name": "QLabsHarmony",       "temp_id": "t1" },
    { "guid": "…", "kind": "create_track", "name": "QLabsBass",          "temp_id": "t2" },
    { "guid": "…", "kind": "create_track", "name": "QLabsCountermelody", "temp_id": "t3" }
  ],
  "items": [
    { "end_qn": 32.0, "guid": "…", "note_count": 29, "start_qn": 0.0, "take_guid": "…", "temp_id": "i0" },
    { "end_qn": 32.0, "guid": "…", "note_count": 35, "start_qn": 0.0, "take_guid": "…", "temp_id": "i1" },
    { "end_qn": 32.0, "guid": "…", "note_count":  9, "start_qn": 0.0, "take_guid": "…", "temp_id": "i2" },
    { "end_qn": 32.0, "guid": "…", "note_count": 10, "start_qn": 0.0, "take_guid": "…", "temp_id": "i3" }
  ],
  "regions": [ { "marker_index": 0, "name": "QLabs Candidate c8dff250", "temp_id": "r0" } ],
  "sends": [],
  "note_count": 83,
  "project_state_change_count": 2,
  "scope_echoed": {
    "source_mode": "auto",
    "note_scope": "all",
    "melody_extraction": { "mode": "auto" },
    "verify_snapshot": true
  },
  "precondition_kinds": [
    "project_uuid", "state_change_count", "item_guid_exists",
    "take_guid_exists", "midi_hash", "tempo_map_hash", "item_bounds"
  ],
  "plan_uri": "editplan://2ffc612c-11d5-4db6-ba1c-30a1037464b4",
  "transaction_uri": "transaction://f5155949-9309-4f56-8073-8df3daa774fa",
  "warnings": []
}
```

*(The `guid` and `take_guid` values above are the stand-in bridge's; a real
REAPER returns REAPER GUIDs in the same positions.)*

**The operations the plan may contain** are a closed set of seven:
`create_folder_track`, `create_track`, `create_midi_item`, `insert_notes`,
`set_track_mute`, `create_region`, `create_midi_send`. There is no delete, no
modify, no move. `the_acceptance_workflow_runs_end_to_end` asserts that no
operation in the plan names the source item GUID or the source take GUID
anywhere in its serialized form.

---

### 3.12 `reaper.commit_candidate`

> Flips a staged transaction's tagged objects from preview to committed and
> unmutes them. The source item is never merged into or deleted.

**Purpose.** Keep what you staged.

**Arguments.**

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `transaction_id` | string | **yes** | — | id pattern; must be one **this session** staged |

**Output.** `ok`, `transaction_id`, `status` (`committed`), `undo_label`,
`committed_tracks`, `committed_items`, `committed_takes`,
`project_state_change_count`, `warnings[]`.

Commit is **transaction-scoped and deliberately does not re-check the source
snapshot** — it acts on objects this transaction tagged, not on the source
material. See [`KNOWN_LIMITATIONS.md`](KNOWN_LIMITATIONS.md) §9.

**Errors.** `UNKNOWN_TRANSACTION` (not staged by this session),
`INVALID_ARGUMENT` / `INVALID_ARGUMENTS` (malformed id), `TRANSACTION_NOT_FOUND`
(the bridge found no tagged object), `BRIDGE_OFFLINE`, `IPC_TIMEOUT`,
`NO_ACTIVE_PROJECT`, `INTERNAL_BRIDGE_ERROR`.

**Example.**

```json
{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{
  "name":"reaper.commit_candidate",
  "arguments":{"transaction_id":"86177154-5aa4-4367-adb6-1e619bd5d897"}}}
```

```json
{
  "ok": true,
  "transaction_id": "86177154-5aa4-4367-adb6-1e619bd5d897",
  "status": "committed",
  "undo_label": "QLabs MCP: Commit candidate 86177154",
  "committed_tracks": 3,
  "committed_items": 2,
  "committed_takes": 2,
  "project_state_change_count": 5,
  "warnings": []
}
```

**Example — a transaction this session did not stage.** Note the code is
`UNKNOWN_TRANSACTION`, not `UNKNOWN_ID`: the server distinguishes "you named an
id I never issued" from "you named a transaction I never staged".

```json
{
  "ok": false,
  "error_code": "UNKNOWN_TRANSACTION",
  "message": "transaction abc123 was not staged by this server session",
  "details": { "transaction_id": "abc123" },
  "remedy": "only a transaction id returned by reaper.stage_candidate can be acted on"
}
```

**Example — a path-shaped id**, rejected at the schema gate before any lookup:

```json
{
  "ok": false,
  "error_code": "INVALID_ARGUMENTS",
  "message": "arguments for reaper.commit_candidate failed its declared input schema (1 problem)",
  "details": {
    "tool": "reaper.commit_candidate",
    "violations": [
      { "path": "/transaction_id", "keyword": "pattern",
        "message": "\"../etc\" does not match /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/" }
    ]
  },
  "remedy": "read the tool's inputSchema in tools/list and correct the arguments"
}
```

---

### 3.13 `reaper.discard_candidate`

> Deletes only the objects carrying this transaction's QLabs ownership tags.
> Objects are never identified by track or item name.

**Purpose.** Throw one candidate away without touching anything else.

**Arguments.** Identical to `reaper.commit_candidate`: `transaction_id`,
required, id pattern, must be one this session staged.

**Output.** `ok`, `transaction_id`, `undo_label`, `removed_items`,
`removed_tracks`, `retained_tracks`, `project_state_change_count`, `warnings[]`.

`retained_tracks` counts tagged tracks that were **kept** because they still hold
items that are not part of this transaction. When that happens you are told, in
`warnings`.

**Errors.** Same set as `reaper.commit_candidate`.

**Example.**

```json
{"jsonrpc":"2.0","id":21,"method":"tools/call","params":{
  "name":"reaper.discard_candidate",
  "arguments":{"transaction_id":"f5155949-9309-4f56-8073-8df3daa774fa"}}}
```

```json
{
  "ok": true,
  "transaction_id": "f5155949-9309-4f56-8073-8df3daa774fa",
  "undo_label": "QLabs MCP: Discard candidate f5155949",
  "removed_items": 2,
  "removed_tracks": 3,
  "retained_tracks": 0,
  "project_state_change_count": 3,
  "warnings": []
}
```

---

### 3.14 `reaper.undo_last_generation`

> Undoes at most one undo entry, and only when it is this server's own last
> transaction. Anything else returns `UNDO_NOT_OWNED`; an unrelated REAPER
> action is never undone.

**Purpose.** Step back one of *our* edits. Never anyone else's.

**Arguments.**

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `transaction_id` | string | no | this session's most recent staged transaction | id pattern |

**Two gates, both must pass.** The server checks that the transaction id (when
supplied) is one this session staged. The bridge then checks that REAPER's top
undo entry is exactly that transaction's own `QLabs MCP: …` label. If anything
else has happened in REAPER since, the undo is refused rather than reaching for
someone else's work.

**Output.** `ok`, `undone` (boolean), `transaction_id`, `undo_label`, `kind`
(`stage` \| `commit` \| `discard`), `project_state_change_count`, `warnings[]`.

**Errors.** `UNDO_NOT_OWNED`, `UNKNOWN_TRANSACTION`, `INVALID_ARGUMENT`,
`INVALID_ARGUMENTS`, `BRIDGE_OFFLINE`, `IPC_TIMEOUT`, `NO_ACTIVE_PROJECT`,
`INTERNAL_BRIDGE_ERROR`.

**Example — an owned undo.**

```json
{"jsonrpc":"2.0","id":23,"method":"tools/call","params":{
  "name":"reaper.undo_last_generation",
  "arguments":{"transaction_id":"86177154-5aa4-4367-adb6-1e619bd5d897"}}}
```

```json
{
  "ok": true,
  "undone": true,
  "transaction_id": "86177154-5aa4-4367-adb6-1e619bd5d897",
  "undo_label": "QLabs MCP: Commit candidate 86177154",
  "kind": "stage",
  "project_state_change_count": 6,
  "warnings": []
}
```

**Example — the named transaction is no longer on top.** Captured by staging
twice and then asking to undo the *first* transaction:

```json
{
  "ok": false,
  "error_code": "UNDO_NOT_OWNED",
  "message": "the top undo entry is not this MCP's last owned transaction",
  "details": {
    "expected": "985a4d75-6f4d-40df-9724-77ac6c490c34",
    "top_undo_entry": "QLabs MCP: Stage candidate 985a4d75"
  }
}
```

**Example — nothing staged in this session at all.** This one is produced by the
server before the bridge is contacted, so it is reproducible with no REAPER:

```json
{
  "ok": false,
  "error_code": "UNDO_NOT_OWNED",
  "message": "this server session has not staged anything to undo",
  "details": {},
  "remedy": "only a transaction staged in this session can be undone"
}
```

---

### 3.15 Tools that do not exist

The following are **absent**, not gated behind a flag or a permission:

```
execute_lua · execute_shell · run_reaper_action · write_arbitrary_midi
delete_track_by_name · edit_project_chunk
```

Nor is there any filesystem passthrough or REAPER API passthrough. Calling one
gets a normal `UNKNOWN_TOOL` result listing the fourteen that do exist:

```json
{
  "ok": false,
  "error_code": "UNKNOWN_TOOL",
  "message": "harmony.invent is not a tool this server exposes",
  "details": {
    "requested": "harmony.invent",
    "available": [
      "reaper.status", "reaper.inspect_selection", "theory.search",
      "music.analyze_selection", "harmony.generate_candidates",
      "harmony.reharmonize", "voicing.generate", "arrangement.generate",
      "loop.audit", "candidate.explain", "reaper.stage_candidate",
      "reaper.commit_candidate", "reaper.discard_candidate",
      "reaper.undo_last_generation"
    ]
  },
  "remedy": "call tools/list for the served set"
}
```

`no_prohibited_tool_is_listed` and
`calling_a_prohibited_tool_is_a_structured_unknown_tool_error` in
[`tests/mcp_protocol.rs`](../crates/reaper-music-mcp/tests/mcp_protocol.rs)
assert both halves of that.

---

## 4. Resources

Thirteen resources across six schemes: **6 concrete** in `resources/list` and
**7 templated** in `resources/templates/list`. Every one serves
`mimeType: "application/json"`.

### 4.1 URI grammar

The parser is **closed**. There is no fallback branch that reads from disk.

```
uri        = scheme "://" path
scheme     = "reaper" | "theory" | "analysis" | "candidate" | "editplan" | "transaction"
path       = one of the thirteen shapes below
segment    = ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$   , containing no ".."
uri length = at most 512 bytes
```

The thirteen shapes:

| URI | Kind | Body |
|---|---|---|
| `reaper://status` | concrete | Exactly `reaper.status`'s output object. |
| `reaper://project/current` | concrete | `{bridge_connected, project_uuid, project_name, project_path, active_project, play_state, selected_item_count, active_midi_editor}` |
| `reaper://selection/current` | concrete | The **most recent** snapshot, verbatim as the bridge sent it. |
| `theory://catalog` | concrete | Knowledge version, manifest hash, domains, profiles, counts, chord symbols, capabilities. |
| `theory://sources` | concrete | `{knowledge_version, count, sources[]}` — the whole source registry. |
| `theory://profiles` | concrete | `{knowledge_version, count, profiles[]}` — id, name, parent, uri. |
| `theory://profiles/{profile_id}` | template | Resolved profile: `{id, chain[], weights{}, fields{}, rule_overrides[], raw{}}`. |
| `theory://rules/{domain}` | template | `{domain, knowledge_version, count, rules[]}`. |
| `analysis://{analysis_id}` | template | The complete analysis report. |
| `candidate://{candidate_id}` | template | Chords, parts, trace, provenance, `created_at`/`expires_at`. |
| `candidate://{candidate_id}/trace` | template | Score, rule applications, sources, assumptions, rejected alternatives. |
| `editplan://{plan_id}` | template | The plan in domain form, plus `scope` and `precondition_kinds`. |
| `transaction://{transaction_id}` | template | `{transaction_id, plan_id, candidate_id, snapshot_id, status, undo_label, stage_result}`. |

`{domain}` is one of `melody`, `harmony`, `extensions`, `voice_leading`,
`counterpoint`, `arrangement`, `looping`. `{profile_id}` is one of the ten
profiles.

Because a variable segment cannot contain `/`, `\`, `%`, `~`, `:` or `..`, a
resource URI **can never name a filesystem path**. Even a segment that satisfied
the pattern could only index the in-memory session store — there is no code path
from a URI to `open()`. `a_resource_uri_can_never_name_a_filesystem_path`
asserts this against a list of escape attempts.

### 4.2 Ownership and TTL

- `analysis://`, `candidate://`, `candidate://…/trace`, `editplan://` and
  `transaction://` resolve **only** against ids this process issued, and only
  while the entry is inside its TTL. See [§2.3](#23-ids-ownership-and-ttls) for
  the numbers.
- `reaper://selection/current` serves the **latest** snapshot only; there is no
  `reaper://selection/{id}`. Before any inspection it fails.
- `theory://*` resources are static for the life of the process and have no TTL.
- `reaper://status` and `reaper://project/current` are computed on each read.

### 4.3 Reading

```json
{"jsonrpc":"2.0","id":6,"method":"resources/read",
 "params":{"uri":"editplan://2ffc612c-11d5-4db6-ba1c-30a1037464b4"}}
```

The reply always has this shape — one text content block carrying the
pretty-printed JSON body:

```jsonc
{
  "jsonrpc": "2.0",
  "id": 6,
  "result": {
    "contents": [
      {
        "uri": "editplan://2ffc612c-11d5-4db6-ba1c-30a1037464b4",
        "mimeType": "application/json",
        "text": "{\n  \"plan_id\": …\n}"
      }
    ]
  }
}
```

`text` parsed, for that plan (operations, preconditions and expected outputs
elided; the real body is ~24 kB):

```jsonc
{
  "plan_id": "2ffc612c-11d5-4db6-ba1c-30a1037464b4",
  "candidate_id": "c8dff250-e434-503c-be52-c132279f75cb",
  "transaction_id": "f5155949-9309-4f56-8073-8df3daa774fa",
  "base_snapshot_id": "00000000-0000-4000-8000-0000000000aa",
  "base_snapshot_hash": "fnv1a64:ba783f8d50ebc7ca",
  "project_uuid": "00000000-0000-4000-8000-000000000001",
  "knowledge_version": "1.0.0",
  "undo_label": "QLabs MCP: Stage candidate f5155949",
  "operations": [
    { "op": "create_folder_track", "temp_id": "f0",
      "name": "QLabs Candidate c8dff250",
      "tags": { "QLABS_ROLE": "candidate_folder" } },
    { "op": "create_track", "temp_id": "t0", "parent": "f0",
      "name": "QLabsMelody", "tags": { "QLABS_ROLE": "lead" } }
    // … 12 more
  ],
  "preconditions": [
    { "kind": "project_uuid",       "value": "00000000-0000-4000-8000-000000000001" },
    { "kind": "state_change_count", "value": 1 },
    { "kind": "item_guid_exists",   "value": "00000002-0002-4002-8002-000000000002" },
    { "kind": "take_guid_exists",   "value": "00000003-0003-4003-8003-000000000003" },
    { "kind": "midi_hash",          "value": "fnv1a64:4a314e84e9743f65" },
    { "kind": "tempo_map_hash",     "value": "fnv1a64:71fd6a6ecb0f88fd" },
    { "kind": "item_bounds",        "start_qn": 0.0, "end_qn": 32.0 }
  ],
  "expected_outputs": [
    { "temp_id": "f0", "kind": "folder_track", "note_count": null },
    { "temp_id": "t0", "kind": "track",        "note_count": null }
    // … 7 more
  ],
  "scope": {
    "source_mode": "auto",
    "note_scope": "all",
    "melody_extraction": { "mode": "auto" }
  },
  "precondition_kinds": [
    "project_uuid", "state_change_count", "item_guid_exists",
    "take_guid_exists", "midi_hash", "tempo_map_hash", "item_bounds"
  ]
}
```

> **Domain form versus wire form.** The resource shows the plan in its **domain**
> form, where a precondition has a `kind` and rational quarter notes are exact.
> What crosses to the bridge is the **wire** form, where the same field is
> `type` and tags are `[[key, value]]` pairs rather than an object. The
> conversion is one-way, at the boundary.
> `the_plan_the_bridge_receives_is_the_wire_form` in `acceptance.rs` asserts the
> wire form has `type` and never `kind`, and that tags are pairs. The wire body
> actually written to `<ipc-dir>/commands/` during the capture began:
>
> ```jsonc
> { "preconditions": [
>     { "type": "project_uuid",       "value": "00000000-0000-4000-8000-000000000001" },
>     { "type": "state_change_count", "value": 1 } ],
>   "operations": [
>     { "op": "create_folder_track", "temp_id": "f0",
>       "name": "QLabs Candidate c8dff250",
>       "tags": [["QLABS_ROLE", "candidate_folder"]] } ] }
> ```

**Other real bodies.** `theory://catalog`:

```jsonc
{
  "knowledge_version": "1.0.0",
  "schema_version": "1.0.0",
  "content_sha256": "ad393aa7c3c1250fac38afa29d0b8425e372b605e6300af6bc538cd2aaad03cb",
  "manifest_sha256": "ad393aa7c3c1250fac38afa29d0b8425e372b605e6300af6bc538cd2aaad03cb",
  "generated_at": "2026-07-26T00:00:00Z",
  "counts": {
    "arrangement_patterns": 32, "cadences": 14, "chord_qualities": 51,
    "chord_symbols": 65, "functions": 45, "instrument_profiles": 14,
    "intervals": 28, "modes": 22, "profiles": 10, "progression_schemas": 51,
    "progressions": 37, "rules": 147, "scales": 35, "sources": 9,
    "voicing_templates": 38
  },
  "rules_by_domain": {
    "arrangement": 15, "counterpoint": 13, "extensions": 20, "harmony": 37,
    "looping": 14, "melody": 17, "voice_leading": 31
  },
  "collections": { /* profiles, scales, chord_qualities, voicing_templates,
                      progressions, cadences, arrangement_patterns,
                      instrument_profiles, modes, intervals, sources */ },
  "predicates": [ "altered_tone_resolves_by_step", "bass_supplies_root", /* …116 total */ ],
  "supported_chord_symbols": [ "maj13", "maj11", "maj9", "maj7", "Δ", /* …65 total */ ],
  "generation_capabilities": {
    "harmonization": true, "reharmonization": true, "voicing": true,
    "arrangement": true, "loop_audit": true, "countermelody": true,
    "bass": true, "staging": true, "max_candidates": 8,
    "profiles": [ /* the 10 */ ],
    "voicing_families": [ /* the 13 */ ],
    "arrangement_roles": [ /* the 16 */ ]
  },
  "server_version": "1.0.0",
  "mcp_protocol_version": "2025-11-25"
}
```

`theory://profiles/jazz_standard`, weights in full:

```json
{
  "id": "jazz_standard",
  "chain": ["jazz_standard", "common_practice"],
  "weights": {
    "arrangement_clarity": 0.06802721088435375,
    "bass_quality": 0.07482993197278913,
    "candidate_diversity": 0.06122448979591837,
    "chromaticism_target": 0.06802721088435375,
    "complexity_target": 0.06802721088435375,
    "extension_appropriateness": 0.108843537414966,
    "functional_or_modal_coherence": 0.0816326530612245,
    "harmonic_coherence": 0.08843537414965986,
    "loop_compatibility": 0.04081632653061225,
    "melody_fit": 0.08843537414965986,
    "phrase_direction": 0.06802721088435375,
    "style_match": 0.08843537414965986,
    "voice_leading": 0.09523809523809523
  },
  "rule_overrides": [
    { "rule_id": "counterpoint.dissonance_is_prepared", "multiplier": 0.3, "enabled": true }
  ]
}
```

*(`fields` and `raw` elided: 22 flattened profile fields plus the unresolved
source object.)*

`theory://rules/counterpoint` — `{domain, knowledge_version, count: 13, rules[]}`,
first rule complete:

```json
{
  "id": "counterpoint.dissonance_is_prepared",
  "domain": "counterpoint",
  "kind": "strong_theory_principle",
  "summary": "In strict counterpoint a dissonance is approached as a consonance in the same voice.",
  "trigger": { "event": "voice_pair_motion" },
  "conditions": ["dissonance_is_on_strong_beat", "dissonance_is_prepared"],
  "effect": { "score_delta": 3.0, "severity": "moderate", "score_component": "voice_leading" },
  "profiles": ["strict_counterpoint", "common_practice"],
  "exceptions": ["modal_center_is_active", "intentional_cluster", "pedal_point_is_active"],
  "source_refs": [ { "source_id": "mt21c", "locator": "Counterpoint" } ],
  "confidence": 0.9,
  "rationale": "Preparation is what makes a dissonance sound intentional. Outside strict profiles, unprepared dissonance is normal and this rule is simply not applied.",
  "test_ids": ["dissonance_prepared_strict", "unprepared_dissonance_allowed_jazz"],
  "version": 1
}
```

`transaction://{id}`:

```jsonc
{
  "transaction_id": "f5155949-9309-4f56-8073-8df3daa774fa",
  "plan_id": "2ffc612c-11d5-4db6-ba1c-30a1037464b4",
  "candidate_id": "c8dff250-e434-503c-be52-c132279f75cb",
  "snapshot_id": "00000000-0000-4000-8000-0000000000aa",
  "status": "staged",
  "undo_label": "QLabs MCP: Stage candidate f5155949",
  "stage_result": { /* the bridge's own stage_candidate result, verbatim */ }
}
```

`status` is `staged`, `committed`, `discarded` or `undone`, updated as the
transaction moves.

`candidate://{id}` top level: `id`, `kind`, `label`, `strategy`, `chords[]`,
`parts[]`, `trace{}`, `loop_report`, `created_at`, `expires_at`, `snapshot_id`,
`analysis_id`, `profile_id`, `knowledge_hash`, `seed`.
`candidate://{id}/trace` top level: `candidate_id`, `analysis_id`,
`snapshot_id`, `knowledge_version`, `profile_id`, `seed`, `assumptions[]`,
`confidence`, `score{}`, `rule_applications[]`, `warnings[]`, `source_ids[]`,
`explanation`, `rejected_alternatives[]`.
`analysis://{id}` top level: `id`, `snapshot_id`, `profile_id`,
`knowledge_version`, `confidence`, `extraction{}`, `phrases{}`, `salience{}`,
`melody{}`, `key{}`, `grid{}`, `ncts{}`, `detected_chords[]`,
`loop_observations[]`, `warnings[]` — a superset of the analysis tool's summary.

### 4.4 Resource read failures

A failed `resources/read` is a **JSON-RPC error** with code `-32002`
(MCP's "resource not found"), carrying the same structured payload as `data`:

**Unrecognised URI:**

```json
{
  "jsonrpc": "2.0",
  "id": 30,
  "error": {
    "code": -32002,
    "message": "theory://nope is not a resource this server serves",
    "data": {
      "ok": false,
      "error_code": "UNKNOWN_RESOURCE",
      "message": "theory://nope is not a resource this server serves",
      "details": { "uri": "theory://nope", "reason": "unrecognised shape or unsafe segment" },
      "remedy": "call resources/list and resources/templates/list for the served set"
    }
  }
}
```

`details.reason` is one of `too long`, `no scheme`, or
`unrecognised shape or unsafe segment`.

**Unknown profile** — the one path by which `UNKNOWN_PROFILE` reaches a client,
since every tool's `style_profile` is a closed enum:

```json
{
  "jsonrpc": "2.0",
  "id": 28,
  "error": {
    "code": -32002,
    "message": "no style profile no_such_profile",
    "data": {
      "ok": false,
      "error_code": "UNKNOWN_PROFILE",
      "message": "no style profile no_such_profile",
      "details": { "profile_id": "no_such_profile" },
      "remedy": "read theory://profiles for the served set"
    }
  }
}
```

**No selection inspected yet:**

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "error": {
    "code": -32002,
    "message": "no selection has been inspected in this session",
    "data": {
      "ok": false,
      "error_code": "UNKNOWN_ID",
      "message": "no selection has been inspected in this session",
      "details": {},
      "remedy": "call reaper.inspect_selection first"
    }
  }
}
```

An unknown rule domain gives `UNKNOWN_RESOURCE` with `details.known` listing the
seven domains. An expired id gives `EXPIRED_ID` under the same `-32002`.

---

## 5. Prompts

Nine user-controlled prompt templates. Each renders to **one** user message
telling the client model which semantic tools to call and in what order.

**A prompt is text.** It cannot call a tool, cannot supply a snapshot id and
cannot widen a schema. Every instruction it produces still goes through
`tools/call`, where the input schema is enforced exactly as for a hand-written
call. Argument values are interpolated verbatim into the rendered text — asking
for 99 candidates in a prompt argument produces a message saying "99", and the
resulting tool call is still rejected by `candidate_count`'s `maximum: 8`.

| Prompt | Arguments (required in **bold**) | What it instructs |
|---|---|---|
| `analyze-selected-melody` | `style_profile` | `reaper.status` → `reaper.inspect_selection` → `music.analyze_selection`; then summarise the ranked key candidates and their evidence, the phrase structure and pickup, structural notes, likely non-chord tones and why, the harmonic rhythm the grid suggests, and range and tessitura. Explicitly: say where the reading is ambiguous, generate nothing, stage nothing. |
| `harmonize-selected-melody` | `style_profile`, `candidate_count`, `focus` | The full §33 workflow: status (stop if offline) → inspect → analyze → `harmony.generate_candidates` with `preserve_melody` and `preserve_rhythm` → `candidate.explain` on each. Then compare: name each strategy, quote the separating score components, name the rules and sources. **"Do not call `reaper.stage_candidate` until I tell you which candidate I want. When I do, stage that one candidate id and nothing else."** |
| `reharmonize-selected-region` | `style_profile`, `candidate_count`, `preserve` | inspect → analyze → `harmony.reharmonize` for three candidates under the named preservation constraints. Report chord by chord what was substituted, which transformation family it came from, and what the melody note became. Flag anything that weakened a cadence. Stage nothing until chosen. |
| `extend-selected-chords` | `style_profile`, `focus` | inspect → analyze → `harmony.reharmonize` with `preserve_cadence` and `preserve_harmonic_rhythm` true and high `complexity`. Per chord: which extension or alteration was added, why it is available over that melody note, where an avoid tone was deliberately left out. |
| `create-smooth-voicings` | **`candidate_id`**, `style_profile`, `voice_count` | `voicing.generate` with `preserve_top` true and several families. Per variant: total voice motion, largest leap, parallel perfects, crossings and overlaps, unresolved tendency tones. Recommend one. |
| `arrange-selected-sketch` | **`candidate_id`**, `style_profile`, `roles` | `arrangement.generate` with the named roles, then read the masking report. What each part does, its register, its density, where the arrangement uses silence rather than volume. Name any collision and what you would change. |
| `create-loopable-variants` | `style_profile`, `candidate_count`, `loop_intent` | inspect → analyze with the loop span → `harmony.generate_candidates` with `loop_intent` → `loop.audit` on each. Report harmonic, bass and voice-leading wrap, hanging and crossing notes, pickup and tail. Explicitly: **"a modal drone loop does not need a dominant resolution — do not force one."** |
| `audit-harmony-and-voice-leading` | `style_profile` | inspect → analyze → `loop.audit` if the region loops → `theory.search` for the rules behind anything flagged, so every criticism cites a rule id and a source. Be explicit when something is only a problem under a strict profile. **Do not generate or stage.** |
| `explain-generated-candidate` | **`candidate_id`**, `detail` | `candidate.explain`, then read `candidate://{id}/trace` for anything the summary left out. Walk through the assumed key reading, the chord per grid slot and why, the score components, the rules that fired and their sources, the assumptions, and the rejected alternatives. **"Quote real rule ids and source ids — do not paraphrase them away."** |

### 5.1 Fetching a prompt

```json
{"jsonrpc":"2.0","id":11,"method":"prompts/get","params":{
  "name":"harmonize-selected-melody",
  "arguments":{"style_profile":"jazz_standard","candidate_count":"3","focus":"warm and open"}}}
```

Response, verbatim:

```json
{
  "jsonrpc": "2.0",
  "id": 11,
  "result": {
    "description": "The full workflow: status, inspect, analyze, generate several genuinely different harmonizations, explain the differences, stage only on request.",
    "messages": [
      {
        "role": "user",
        "content": {
          "type": "text",
          "text": "Harmonize the melody I have selected in REAPER. I am after: warm and open.\n\n1. Call `reaper.status`. If the bridge is offline, stop and tell me how to start it.\n2. Call `reaper.inspect_selection`.\n3. Call `music.analyze_selection` on the snapshot.\n4. Call `harmony.generate_candidates` for 3 candidates with the `jazz_standard` style profile, with `preserve_melody` and `preserve_rhythm` both true.\n5. Call `candidate.explain` on each candidate.\n\nThen compare them for me: name each strategy, quote the score components that separate them, and name the theory rules and sources each one leaned on. Say which one you would pick and why.\n\nDo not call `reaper.stage_candidate` until I tell you which candidate I want. When I do, stage that one candidate id and nothing else."
        }
      }
    ]
  }
}
```

All prompt arguments are strings on the wire, including `candidate_count`.
Omitted optional arguments fall back to a neutral phrase — with no
`style_profile`, the rendered text says *"using the style profile that best fits
what you hear"* rather than naming one.

### 5.2 Prompt failures

Both are JSON-RPC `-32602` with the structured payload in `data`.

**Unknown prompt:**

```json
{
  "jsonrpc": "2.0",
  "id": 19,
  "error": {
    "code": -32602,
    "message": "unknown prompt \"no-such-prompt\"",
    "data": {
      "ok": false,
      "error_code": "UNKNOWN_PROMPT",
      "message": "unknown prompt no-such-prompt",
      "details": { "prompt": "no-such-prompt" }
    }
  }
}
```

**Missing required argument:**

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "error": {
    "code": -32602,
    "message": "prompt create-smooth-voicings needs the candidate_id argument",
    "data": {
      "ok": false,
      "error_code": "INVALID_ARGUMENTS",
      "message": "prompt create-smooth-voicings needs the candidate_id argument",
      "details": { "prompt": "create-smooth-voicings", "argument": "candidate_id" }
    }
  }
}
```

---

## 6. Error codes

Two vocabularies meet in one field. The server's own codes cover what goes wrong
before a command is ever written; the bridge's §19 codes cover what goes wrong
in REAPER. Both arrive as `structuredContent.error_code` on an `isError` result
(or as `error.data.error_code` for `resources/read` and `prompts/get`).

The complete closed set is enumerated in
[`schemas/mcp-tool-output.schema.json`](../schemas/mcp-tool-output.schema.json).
**An unknown code must be treated as a generic failure, not a crash.**

### 6.1 Server codes

| Code | Cause | What the client should do |
|---|---|---|
| `UNKNOWN_TOOL` | The named tool does not exist. `details.available` lists the fourteen. | Call `tools/list`. Do not retry. |
| `INVALID_ARGUMENTS` | The arguments failed the declared `inputSchema`. `details.violations[]` gives `{path, keyword, message}` for up to 20 problems. | Read the schema, fix the call. Do not retry unchanged. |
| `INVALID_ARGUMENT` | Well-typed but semantically unusable: `midi_channel` with no channel, a tonic that is not a pitch class, an unknown scale or pattern id, a section that ends before it starts, an `analysis_id` from a different snapshot. `details.argument` names it. | Fix that one argument. |
| `UNKNOWN_ID` | A server-issued id this process never issued, or one already swept. `details.kind` says which kind. | Re-derive the id: inspect, analyze or regenerate. Never fabricate one. |
| `EXPIRED_ID` | The entry existed but its TTL elapsed. `details` carries `created_at` and `expired_at`. | Take a fresh snapshot and regenerate. |
| `UNKNOWN_TRANSACTION` | A transaction id not staged by **this session**. | Only act on an id `reaper.stage_candidate` returned. |
| `UNKNOWN_RESOURCE` | The URI is not one of the thirteen shapes, or a segment is unsafe. | Call `resources/list` and `resources/templates/list`. |
| `UNKNOWN_PROMPT` | No such prompt. | Call `prompts/list`. |
| `UNKNOWN_PROFILE` | A style profile not in the bundle. Reachable only via `theory://profiles/{id}` — tool arguments are closed enums. | Read `theory://profiles`. |
| `KNOWLEDGE_INVALID` | An external `--knowledge-dir` bundle failed validation, or a plan's `knowledge_version` is missing or malformed. | Run `qlabs-reaper-music-mcp validate-knowledge`. This is a deployment fault. |
| `ANALYSIS_FAILED` | The analysis engine failed for a reason with no more specific code. | Report it. Retrying identical input gives the identical failure. |
| `GENERATION_FAILED` | Harmonization, reharmonization or voicing failed. | As above. Loosening `strictness` or lowering `candidate_count` sometimes helps. |
| `ARRANGEMENT_FAILED` | The arrangement engine failed. | As above. |
| `LOOP_AUDIT_FAILED` | The loop audit could not be performed. | Check the loop span is inside the material. |
| `CANCELLED` | You sent `notifications/cancelled` for this request. | Nothing. The session store is untouched. |
| `OUTPUT_SCHEMA_VIOLATION` | A tool produced output failing its own declared `outputSchema`. **Always a server bug.** `details.violations[]` is the evidence. | Report it with the violations. Do not retry. |
| `STORE_FULL` | Declared in the vocabulary; **not emitted by this build** — buckets evict oldest-first rather than refusing. | Treat as a generic failure if you ever see it. |
| `INTERNAL_ERROR` | Anything with no more specific code. | Report it. |

### 6.2 Bridge-originated codes

These reach a client through `reaper.inspect_selection`,
`reaper.stage_candidate`, `reaper.commit_candidate`,
`reaper.discard_candidate` and `reaper.undo_last_generation`. Details are the
bridge's own, passed through unflattened.

| Code | Cause | What the client should do |
|---|---|---|
| `BRIDGE_OFFLINE` | `heartbeat.json` missing, unparseable, not `online`, or stale (>10 s), or no IPC directory configured. Client-produced. | Tell the user to start REAPER and run the `QLabs_Reaper_MCP_Bridge.lua` action. The `remedy` field says exactly this. Do not poll tightly. |
| `IPC_TIMEOUT` | No result file appeared before the deadline (5 s read, 30 s write). Client-produced. | Usually the server writes to a directory the bridge does not poll. Compare `heartbeat.json`'s `ipc_dir` with `--ipc-dir`. |
| `IPC_IO_ERROR` | A local filesystem operation on the IPC directory failed. Client-produced. | Check permissions on the IPC directory. |
| `IPC_CANCELLED` | The in-flight bridge request was cancelled. Client-produced. | Nothing. |
| `SNAPSHOT_HASH_MISMATCH` | A hash the bridge sent does not reproduce from the data sent with it. Client-produced. | Never treat the snapshot as valid. Report it — the two sides disagree about the hash contract. |
| `IPC_PROTOCOL_MISMATCH` | Bridge and server speak different IPC protocol versions. | Reinstall so both come from one release. |
| `BRIDGE_VERSION_MISMATCH` | `require_bridge_version` was sent and does not match. | As above. Version pinning is opt-in. |
| `INVALID_INSTANCE_TOKEN` | Server and `config.json` disagree on the installation token. | Re-run the installer, or copy the token out of `config.json`. |
| `EXPIRED_REQUEST` | The request outlived `expires_at` before the bridge claimed it. | Retry. If persistent, the bridge is not keeping up or the clocks disagree. |
| `PAYLOAD_TOO_LARGE` | The command file exceeds 1 MiB. | Stage fewer parts, or a shorter region. |
| `RESULT_TOO_LARGE` | The encoded result would exceed 8 MiB. | Narrow `note_scope`, or select less material. |
| `MALFORMED_REQUEST` | A structurally invalid request reached the bridge. Should not happen from this client. | Report it. |
| `UNKNOWN_COMMAND` | A command not on the bridge's seven-command allowlist. Should not happen. | Report it. |
| `DUPLICATE_REQUEST` | A request id was already processed by this bridge instance. Should not happen. | Report it. |
| `NO_ACTIVE_PROJECT` | A project command arrived with no project open. | Ask the user to open or create a project. |
| `NO_MIDI_SOURCE` | No active MIDI editor take **and** not exactly one MIDI item selected. | Select one MIDI item, or open a MIDI editor. |
| `MULTIPLE_MIDI_SOURCES` | Two or more selected items have valid MIDI takes. | Select exactly one. The server never picks for you. |
| `AMBIGUOUS_MELODY` | The requested extraction cannot be resolved: `selected_only` with nothing selected, `midi_channel` with no notes on that channel, `monophonic_voice` on genuinely polyphonic material. | Name an explicit `melody_extraction.mode`, or select the notes you mean. |
| `UNSUPPORTED_REAPER_VERSION` | `GetAppVersion()` unparseable, or major version below 6. | Upgrade REAPER. |
| `SOURCE_ITEM_MISSING` | The item GUID the plan names is no longer in the project. | Re-inspect and regenerate. |
| `SOURCE_TAKE_MISSING` | The take GUID is gone, or a `midi_hash` constraint has no take to hash. | Re-inspect and regenerate. |
| `STALE_SNAPSHOT` | `snapshot_hash`, `base_snapshot_hash` or `item_bounds` no longer match. `details.rebuilt_with` names the scope the bridge re-derived with. | **Re-inspect and regenerate.** If `rebuilt_with` differs from the scope you inspected with, the mismatch is the scope, not an edit. |
| `PROJECT_CHANGED` | `project_uuid` or `state_change_count` mismatch. The counter increments on any edit, including a selection change. | Re-inspect and regenerate. See [`KNOWN_LIMITATIONS.md`](KNOWN_LIMITATIONS.md) §19. |
| `MIDI_CHANGED` | `midi_hash` mismatch — the notes really did change. | Re-inspect and regenerate. |
| `TEMPO_MAP_CHANGED` | `tempo_map_hash` mismatch. | Re-inspect and regenerate. |
| `INVALID_EDIT_PLAN` | A structural plan violation, or a mutation the plan cannot express. Should not happen from this client. | Report it with `plan_uri`. |
| `TRANSACTION_NOT_FOUND` | Commit or discard found no object carrying the transaction's tags. | The objects were already removed, by hand or by REAPER's undo. Nothing to do. |
| `UNDO_NOT_OWNED` | REAPER's top undo entry is not this MCP's last owned transaction. `details.top_undo_entry` names what is on top. | Use `reaper.discard_candidate` instead. Never work around this. |
| `INTERNAL_BRIDGE_ERROR` | A Lua error caught by the bridge's guards. | Report it with `<ipc-dir>/logs/bridge.log`. |

### 6.3 Transport-level codes

| Code | Meaning | Example message |
|---|---|---|
| `-32700` | Parse error. | `could not parse JSON: json syntax: invalid literal at byte 0; …` — answered with `"id": null`. |
| `-32600` | Invalid request. | `unsupported jsonrpc version "1.0"`, `missing the method field`, `id must be a string or an integer, not float`. |
| `-32601` | Method not found. | `unknown method "nosuch/method"`. |
| `-32602` | Invalid params. | `tools/call needs a name`; `arguments must be an object, not array`; `level must be one of the MCP logging levels`; every `prompts/get` failure. |
| `-32603` | Internal JSON-RPC error. | Defined by the transport and **never emitted by this build** — an internal failure inside a tool becomes an `INTERNAL_ERROR` result instead. |
| `-32002` | Server not initialized **and** resource not found — MCP reuses the number. | `the client must send initialize before any other request`; every `resources/read` failure. |

---

## 7. Worked example: the acceptance flow

This is brief §33 end to end, annotated. Steps 1–3 (open REAPER, run the bridge
action, select a MIDI item) happen outside the protocol. Steps 4–16 are below,
with the ids and values from a real captured session over a 29-note, eight-bar
C-major melody at 110 BPM, 4/4, loop region 0–32 QN.

`crates/reaper-music-mcp/tests/acceptance.rs` executes exactly this sequence
against a stand-in bridge and asserts each step; the test names are cited where
they apply.

---

**4. Confirm the bridge.**

```json
→ {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"reaper.status","arguments":{}}}
← "bridge_connected": true, "active_project": true, "reaper_version": "7.22/…"
```

If `bridge_connected` were `false`, stop here and tell the user to run the
action. This is the only bridge tool that answers rather than failing.
*(`the_acceptance_workflow_runs_end_to_end` asserts `bridge_connected`,
`active_project` and `reaper_version`.)*

---

**5. Snapshot the selection.**

```json
→ {"name":"reaper.inspect_selection","arguments":{}}
← "snapshot_id": "00000000-0000-4000-8000-0000000000aa",
  "note_count": 29,
  "snapshot_hash": "fnv1a64:ba783f8d50ebc7ca",
  "item_start_qn": 0.0, "item_end_qn": 32.0
```

Keep the `snapshot_id`. Keep the `snapshot_hash` too — it is what step 12 will
turn on. The same snapshot is readable verbatim at
`reaper://selection/current`, and reading it back returns the same
`snapshot_id`. *(Asserted: `note_count > 0`, `snapshot_hash` starts `fnv1a64:`,
and the resource's `snapshot_id` equals the tool's.)*

---

**6. Analyze it.**

```json
→ {"name":"music.analyze_selection",
   "arguments":{"snapshot_id":"00000000-…-aa","style_profile":"jazz_standard",
                "loop_span":{"start_qn":0.0,"end_qn":32.0}}}
← "analysis_id": "914030c1-9ad7-53b0-b59f-b2ad5bcbfc42",
  "confidence": 0.807853,
  "key": { "ambiguous": false, "gap": 0.448215,
           "candidates": [ "C major" 0.651175, "C bebop_dominant" 0.607423, … 6 more ] },
  "phrases": [ 0–16 imperfect_authentic, 16–32 imperfect_authentic ]
```

The reading is **ranked**, not asserted — eight key candidates with evidence
vectors, and a `gap` you can judge. *(Asserted: more than one key candidate, and
a non-empty `phrases` array — "the reading must be ranked, not asserted".)*

---

**7–8. Three genuinely different harmonizations.**

The user's request — *"Create three harmonizations. Preserve the melody and
timing. Make one warm and extended, one dark and modal, and one chromatic. Add
bass and a restrained countermelody. Keep the eight-bar region loopable."* —
becomes one call:

```json
→ {"name":"harmony.generate_candidates","arguments":{
     "snapshot_id":"00000000-…-aa",
     "analysis_id":"914030c1-…","style_profile":"jazz_standard",
     "candidate_count":3,"preserve_melody":true,"preserve_rhythm":true,
     "complexity":0.65,"chromaticism":0.35,"extension_density":0.55,
     "bass_motion":"auto","countermelody":{"enabled":true,"density":0.25},
     "loop_intent":"closed_tonic","strictness":"balanced","seed":12345}}
```

```
← c8dff250-…  functional           "Functional & cadence-directed"  0.8056
              Am11 A9(13) Dm11 A9(13) D9(13) E9(13) Am11 Dm7 Am11
← d0a75ab5-…  modal_common_tone    "Modal & common-tone"            0.7574
              Am11 Am11 Dm11 Am11 D9(13) E9(13) Am11 D9(13) Am11
← a94d41ef-…  chromatic_bass_led   "Chromatic & bass-led"           0.7612
              Fm11 Gm11 Bb9(13) C7#9#11 Fm11 Bb9(13) C7#9#11 Fm11 Bb9(13)
```

Three distinct `strategy` values, not three variations on one. Every candidate
carries `lead:Melody` (the melody survives), `bass:Bass` and
`counterlead:Countermelody`, a 13-component score vector, real `rule_ids` and
resolvable `source_ids`. *(Asserted: exactly three candidates, three unique
strategies, non-empty `chords` / `score_components` / `rule_ids`, and a part
starting `lead:` and one starting `bass:` on each.)*

Explain each one:

```json
→ {"name":"candidate.explain","arguments":{"candidate_id":"c8dff250-…","detail":"detailed"}}
← 25 rule applications, each with status, score_delta, matched_conditions and source_ids;
  13 weighted score components; 3 resolved sources; 4 rejected alternatives.
```

*(Asserted: non-empty `rules`, non-empty `score.components`, a non-empty `title`
on every source, and `rule_applications` present at `candidate://{id}/trace`.)*

Audit the loop on each:

```json
→ {"name":"loop.audit","arguments":{"candidate_id":"c8dff250-…",
    "loop_intent":"closed_tonic","loop_span":{"start_qn":0.0,"end_qn":32.0}}}
← "compatible": true, "score": 0.69742,
  "harmonic_wrap": "Am11 (tonic) to Am11 (tonic): already closed before the wrap",
  "findings": [ LOOP_UNRESOLVED_TENDENCY (minor), LOOP_HARMONIC_RHYTHM_CHANGE (moderate) ],
  "repairs":  [ equalise_the_harmonic_rhythm_at_the_wrap ]
```

*(Asserted: `intent` echoes `closed_tonic`, and `harmonic_wrap` and `repairs`
are present.)*

---

**9–10. Stage the chosen candidate.**

```json
→ {"name":"reaper.stage_candidate",
   "arguments":{"candidate_id":"c8dff250-…","create_region":true}}
← "transaction_id": "f5155949-9309-4f56-8073-8df3daa774fa",
  "plan_id":       "2ffc612c-11d5-4db6-ba1c-30a1037464b4",
  "status": "preview",
  "undo_label": "QLabs MCP: Stage candidate f5155949",
  "note_count": 83,
  "tracks": [ folder "QLabs Candidate c8dff250", QLabsMelody, QLabsHarmony,
              QLabsBass, QLabsCountermelody ],
  "items":  [ 4 items, 29 / 35 / 9 / 10 notes, all 0–32 QN ],
  "regions": [ "QLabs Candidate c8dff250" ],
  "precondition_kinds": [ project_uuid, state_change_count, item_guid_exists,
                          take_guid_exists, midi_hash, tempo_map_hash, item_bounds ]
```

Five new tracks in their own folder, all muted, all tagged. Nothing existing was
touched. *(Asserted: `status == "preview"`, `undo_label` starts `QLabs MCP: `,
`note_count > 0`, non-empty `tracks` and `items`, all seven precondition kinds
present, and `scope_echoed` carrying `note_scope` and `melody_extraction`.)*

---

**11. The original item is unchanged.**

Read the plan back and check it:

```json
→ {"method":"resources/read","params":{"uri":"editplan://2ffc612c-…"}}
```

Every operation is one of `create_folder_track`, `create_track`,
`create_midi_item`, `insert_notes`, `set_track_mute`, `create_region`,
`create_midi_send` — there is no delete and no modify — and **no operation names
the source item GUID or the source take GUID anywhere in its serialized form**.
That is asserted directly, by substring, over every operation in the plan.

---

**12. A stale snapshot is rejected.**

The user edits the source between generating and staging. The next staging
attempt fails:

```json
→ {"name":"reaper.stage_candidate","arguments":{"candidate_id":"69cf860c-…"}}
← {
    "ok": false,
    "error_code": "STALE_SNAPSHOT",
    "message": "the source material changed since the plan was generated",
    "details": {
      "expected": "fnv1a64:ba783f8d50ebc7ca",
      "actual":   "fnv1a64:6a0af0aab31145b7",
      "rebuilt_with": { "note_scope": "all", "source_mode": "auto" },
      "rolled_back": false,
      "transaction_id": "69cf860c-5a01-46bc-aae8-62f5416b76ed"
    },
    "remedy": "the source material changed since the snapshot was taken; call reaper.inspect_selection again and regenerate"
  }
```

**Nothing was written.** The preconditions are evaluated before any object is
created, so a rejected plan leaves the project exactly as it was —
`a_stale_snapshot_is_rejected` asserts the bridge staged nothing and recorded no
plan.

Read `details` carefully. `rebuilt_with` echoes the scope the bridge re-derived
the snapshot with. If it matches the scope you inspected with — as above — the
material genuinely changed. If it differs, the failure is a **scope mismatch**,
not an edit, and the fix is to pass the same scope rather than to regenerate.

**Recovery** is exactly the remedy:

```json
→ {"name":"reaper.inspect_selection","arguments":{}}
← "snapshot_id": "00000000-0000-4000-8000-0000000000bb"   ← a new id, a new hash

→ {"name":"harmony.generate_candidates",
   "arguments":{"snapshot_id":"00000000-…-bb","candidate_count":1,"seed":1}}
← a different candidate_id — the changed snapshot regenerates rather than
  serving a cache hit, because the cache is keyed on the snapshot hash

→ {"name":"reaper.stage_candidate","arguments":{"candidate_id":"<the new one>"}}
← "status": "preview"
```

*(Asserted: the error code and that the remedy mentions `inspect_selection`;
that `details.rebuilt_with` is present; that nothing was staged; that the
regenerated candidate id differs from the stale one; and that staging then
succeeds.)*

---

**13. Discard removes only that candidate.**

```json
→ {"name":"reaper.discard_candidate",
   "arguments":{"transaction_id":"f5155949-…"}}
← "removed_items": 2, "removed_tracks": 3, "retained_tracks": 0,
  "undo_label": "QLabs MCP: Discard candidate f5155949"
```

Objects are found by their `QLABS_OWNER` and transaction tags, never by name.
*(Asserted: the transaction id round-trips, `removed_tracks > 0`, and the
transaction moved out of `staged` and into `discarded`.)*

---

**14. Stage again, then commit.**

```json
→ {"name":"reaper.stage_candidate","arguments":{"candidate_id":"c8dff250-…"}}
← "transaction_id": "86177154-5aa4-4367-adb6-1e619bd5d897"   ← a fresh id
→ {"name":"reaper.commit_candidate","arguments":{"transaction_id":"86177154-…"}}
← "status": "committed", "committed_tracks": 3, "committed_items": 2,
  "committed_takes": 2, "undo_label": "QLabs MCP: Commit candidate 86177154"
```

Restaging mints a **new** transaction id; ids are never reused.
*(Asserted: the second transaction id differs from the first, `status ==
"committed"`, and `committed_tracks > 0`.)*

---

**15. Undo acts only on the owned transaction.**

```json
→ {"name":"reaper.undo_last_generation","arguments":{"transaction_id":"86177154-…"}}
← "undone": true, "undo_label": "QLabs MCP: Commit candidate 86177154", "kind": "stage"
```

*(Asserted: `undone == true` and the label starts `QLabs MCP: `.)*

---

**16. An unrelated undo entry is never undone.**

Ask to undo something that is no longer on top and it is refused rather than
reaching past it:

```json
← {
    "ok": false,
    "error_code": "UNDO_NOT_OWNED",
    "message": "the top undo entry is not this MCP's last owned transaction",
    "details": {
      "expected": "985a4d75-6f4d-40df-9724-77ac6c490c34",
      "top_undo_entry": "QLabs MCP: Stage candidate 985a4d75"
    }
  }
```

`undo_refuses_when_the_last_owned_transaction_is_not_the_one_named` asserts the
code and that the details name what is actually on top. The correct response is
`reaper.discard_candidate`, which is tag-scoped and does not care what REAPER's
undo stack looks like.

---

**Throughout.** The transaction record stays readable at
`transaction://{transaction_id}` for 24 hours, carrying `status`, `undo_label`,
`plan_id`, `candidate_id`, `snapshot_id` and the bridge's own staging result —
so a client can reconstruct what happened long after the tool result has scrolled
away.

---

## 8. Where to look next

| Question | Document |
|---|---|
| How is the server put together? | [`ARCHITECTURE.md`](ARCHITECTURE.md) |
| What exactly is a chord, a note, a part? | [`MUSIC_IR.md`](MUSIC_IR.md) |
| What does a rule id mean, and how do the profiles work? | [`THEORY_MODEL.md`](THEORY_MODEL.md) |
| Where did the theory and the source ids come from? | [`RESEARCH_AND_PROVENANCE.md`](RESEARCH_AND_PROVENANCE.md) |
| How does the other side of the bridge behave? | [`REAPER_BRIDGE.md`](REAPER_BRIDGE.md) |
| What can this do to my machine? | [`SECURITY.md`](SECURITY.md) |
| What is actually tested? | [`TESTING.md`](TESTING.md) |
| What does none of this do? | [`KNOWN_LIMITATIONS.md`](KNOWN_LIMITATIONS.md) |
| How do I install it? | [`INSTALL_WINDOWS.md`](INSTALL_WINDOWS.md) |
