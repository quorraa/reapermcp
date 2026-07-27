# Security and privacy

What this product protects, what it deliberately does not protect, and what it
does to your machine.

The short version: it is a **local, single-user, offline tool** that controls a
creative project. Its security posture is built around one idea — *never destroy
work* — and one architectural decision — *do not build the dangerous primitives
in the first place*.

---

## Contents

1. [Posture](#1-posture)
2. [Threat model](#2-threat-model)
3. [Trust boundaries](#3-trust-boundaries)
4. [Why arbitrary low-level tools are prohibited](#4-why-arbitrary-low-level-tools-are-prohibited)
5. [File-IPC protections](#5-file-ipc-protections)
6. [Remaining local-machine risks](#6-remaining-local-machine-risks)
7. [Data retention](#7-data-retention)
8. [Logging and redaction](#8-logging-and-redaction)
9. [Recovery procedures](#9-recovery-procedures)
10. [Reporting a vulnerability](#10-reporting-a-vulnerability)

---

## 1. Posture

| Property | Status |
|---|---|
| Network listener | **None.** No TCP, no HTTP, no WebSocket, anywhere in the workspace. |
| Outbound network | **None.** The server makes no network calls. |
| Analytics / telemetry | **None.** Nothing is measured, counted or reported anywhere. |
| Project upload | **Never.** Your MIDI does not leave the machine. |
| Cloud inference | **None.** All reasoning is local, from a compiled-in knowledge bundle. |
| Arbitrary code execution | **None.** No Lua evaluation, no shell, no REAPER action ids. |
| Arbitrary filesystem paths | **None.** No tool or resource URI can name a path. |
| Third-party dependencies | **Zero.** No supply chain to compromise. |
| `unsafe` Rust | **Forbidden** at the workspace level. |
| Elevation required | **None.** Installs per-user. |

Offline operation is a design requirement, not a side effect. Once installed,
the product never needs a network again.

---

## 2. Threat model

### What is in scope

**T1 — Destroying the user's work.** This is the primary threat and the one the
whole design is organised around. A music production project represents hours of
irreplaceable creative work. An automated system with write access to it can
destroy that work by overwriting a take, deleting a track, undoing an unrelated
action, or silently applying an edit to material the user has since changed.

*Mitigations:* the source take is never opened for writing; generated material
always lands on new tracks; staged material is muted and marked `preview`;
commit is explicit; discard and undo are scoped by ownership tag rather than by
name; a whole edit plan is validated before a single mutation; stale snapshots
are rejected; and every write is one REAPER undo entry with an owned label.

**T2 — A confused or adversarial model.** The MCP client is driven by a language
model. It may be wrong, it may be manipulated by content in the user's project,
or the user may simply have asked for something they will regret. The server
must not be a way to turn a bad instruction into an unbounded action.

*Mitigations:* the tool surface is semantic and closed. There is no tool that
takes a path, a script, a REAPER action id, or raw project state. The most
destructive thing any model can do through this server is create some tracks it
also knows how to remove.

**T3 — Unbounded resource use.** A pathological request should not hang REAPER,
fill the disk, or exhaust memory.

*Mitigations:* every axis is bounded — request size (1 MiB), result size
(8 MiB), notes per plan (20 000), notes per item (8 000), tracks (32), items
(64), regions (16), sends (16), operations per plan (512), preconditions (64),
expected outputs (128), notes read per inspection (100 000), beam width,
candidate pool size, path count, log size (1 MiB × 4), and file retention. The
bridge handles at most 4 commands per UI tick so it cannot stall REAPER's
interface. Search loops poll a cancellation flag.

**T4 — Stray files in the IPC directory being executed as commands.** The
transport is a shared directory. Something else writing there should not be able
to drive the bridge by accident.

*Mitigations:* an installation token must match exactly; the filename must end
in `.command.json`; the request id inside the envelope must equal the filename
stem and match a strict pattern; the protocol version must match; the request
must not be expired or a replay. Garbage collection only ever touches files
whose names end in one of the protocol's own suffixes — any other file in the
IPC tree is left alone forever.

**T5 — Acting on stale material.** Generating against a selection, then applying
the result after the user has edited it, produces musical nonsense at best and
destroys intent at worst.

*Mitigations:* six content hashes plus a plan-level precondition set. The bridge
re-derives the snapshot and refuses with `STALE_SNAPSHOT` rather than proceeding.

### What is explicitly out of scope

**Defending against another program running as the same user.** If code is
already executing with your account's privileges, it can read and write your
REAPER project directly, read `config.json`, and write into the IPC directory.
Nothing here can prevent that, and nothing here pretends to. The installation
token is *not* a secret that protects anything; it is a well-formedness check.
Treat this product's security boundary as **the process boundary of your user
account**, not tighter.

**Multi-user or multi-tenant isolation.** There is exactly one user, one REAPER
instance and one bridge. The single-instance lock is for correctness, not
security.

**Cryptographic integrity of snapshots.** `FNV-1a-64` is a change-detection
hash, not a message authentication code (see §5).

**The MCP host's own security.** How your MCP client authenticates, what it
sends where, and what else it can do are that client's concern.

**REAPER's own attack surface.** Project files, plugins and media are REAPER's
domain.

---

## 3. Trust boundaries

```
 ┌──────────────────┐
 │   MCP client     │   Untrusted input. Model-driven, possibly wrong.
 │   (the model)    │
 └────────┬─────────┘
          │  ── BOUNDARY 1 ──  JSON-RPC over stdio
          │     Every argument is schema-validated before it is used.
          │     Ranges, enums, finiteness and id ownership are enforced.
          │     Ids must have been issued by this server, this session.
          ▼
 ┌──────────────────┐
 │   MCP server     │   Trusted to be correct; has no dangerous primitives.
 │  (Rust process)  │   Runs as you. Reads config.json. Writes commands/.
 └────────┬─────────┘
          │  ── BOUNDARY 2 ──  the filesystem
          │     Server writes only in commands/, deletes only in results/.
          │     Never touches processing/, failed/, logs/, heartbeat.json,
          │     bridge.lock or config.json for writing.
          ▼
 ┌──────────────────┐
 │  <ipc-dir>       │   A shared directory. Same-user readable and writable.
 │  (the channel)   │   NOT a trust boundary against local code.
 └────────┬─────────┘
          │  ── BOUNDARY 3 ──  envelope validation, 20 ordered checks
          │     Token, protocol version, id pattern, id-equals-stem, replay,
          │     expiry, command allowlist, payload shape, preconditions.
          ▼
 ┌──────────────────┐
 │  Lua bridge      │   Trusted; the only thing that touches REAPER.
 │  (in REAPER)     │   Validates the whole plan before any mutation.
 └────────┬─────────┘
          │  ── BOUNDARY 4 ──  the ownership predicate
          │     Only objects tagged QLABS_OWNER + this transaction id may be
          │     modified or deleted. Names are never used.
          ▼
 ┌──────────────────┐
 │  Your project    │   Yours. The source take is never opened for writing.
 └──────────────────┘
```

**Boundary 1** is where model output stops being free-form. Every tool has a
compiled JSON Schema 2020-12 `inputSchema` that is enforced, not merely
advertised. Candidate counts are `1..=8`; normalized controls are `0..=1`;
numbers must be finite; profile ids must exist; ids must be ones this server
issued in this session and must not have expired. A validation failure returns a
structured tool error, not a crash and not a partial action.

**A resource URI can never name a filesystem path.** `analysis://{id}`,
`candidate://{id}`, `editplan://{id}` and `transaction://{id}` resolve
server-issued ids against an in-memory store. There is no `file://`, no path
traversal to attempt, and nothing on disk to reach.

**Boundary 2** is deliberately narrow. The Rust side has exactly two write
capabilities in the IPC tree: create a file in `commands/`, and delete a file in
`results/`. Everything else is read-only to it.

**Boundary 3** is the one that matters most, because it is where an untrusted
byte sequence becomes an action. The validation order is normative and
first-failure-wins, so a malformed request is rejected at the earliest possible
point.

**Boundary 4** is what makes discard and undo safe. Objects are identified by
extended-state tags, never by name. Rename a staged track to "Drums" and it is
still owned by its transaction; create your own track called "QLabs Candidate
01" and it is still untouchable.

---

## 4. Why arbitrary low-level tools are prohibited

These tools do not exist in this product:

```
execute_lua           execute_shell          run_reaper_action
write_arbitrary_midi  delete_track_by_name   edit_project_chunk
```

Nor does any filesystem tool, any REAPER API passthrough, or any generic "run
this" escape hatch. Correspondingly, the bridge's command allowlist has exactly
seven entries and none of them evaluates code, invokes an action id, or accepts
a path.

They are absent rather than disabled, and the reasons are worth stating
explicitly.

**A single unbounded tool erases every other control.** Bounds, schemas,
ownership tags, snapshot validation and undo labels are all defeated the moment
one tool can run arbitrary Lua inside REAPER. There is no partial version of
this: `execute_lua` *is* full control of the project, the filesystem, and
anything else REAPER can reach.

**`run_reaper_action` is the same problem wearing a different hat.** REAPER's
action list includes "Close project without saving", "Remove tracks", "Empty
undo history" and thousands more. Exposing action ids is exposing all of them,
including whatever a third-party extension has registered. An allowlist of safe
action ids would need auditing on every REAPER release and every user's
extension set — an unmaintainable promise.

**`delete_track_by_name` is unsafe by construction.** Names are user data, not
identity. Two tracks can share a name; the user can rename ours; the user can
name theirs like ours. Deleting by name means eventually deleting the wrong
thing. This is why the ownership predicate uses tags and why the bridge refuses
to identify objects by name anywhere.

**`edit_project_chunk` and `write_arbitrary_midi` bypass validation.** State
chunks and raw MIDI event streams are exactly the representations that the note
count limits, item bounds checks, pitch and velocity ranges, and
zero-length-note rejections exist to police.

**A model cannot be relied on to hold a dangerous tool safely.** Not because
models are bad, but because the failure mode is unrecoverable and the user is
not in the loop for every call. The right design is one where the worst
plausible outcome of a confused request is *some tracks you did not want*, which
you can then discard.

**What replaces them:** semantic tools that express musical intent — analyse,
harmonize, reharmonize, voice, arrange, audit, explain, stage, commit, discard,
undo. Every one of them has a bounded, validated, reversible effect.

---

## 5. File-IPC protections

The channel is a directory. These are the properties that make it safe enough
for its purpose.

**Atomic publication.** Both sides write a temp file **in the same directory as
its final name** and then rename. Same-directory rename is atomic on Windows and
on POSIX filesystems. Neither side ever reads a `.tmp`. If the reader can see
`<id>.result.json`, its contents are complete and final. Writing a temp file in
a system temp directory and moving it across filesystems would break this, so it
is forbidden.

**Atomic claim.** The bridge takes a command with a single rename into
`processing/`. If the rename fails, another instance won and it moves on
silently. There is no read-then-lock window.

**Single instance.** `bridge.lock` holds a 32-hex `pid_token` and a refreshed
`heartbeat_at`. A starting instance refuses to run if the lock is live and names
a different token; a running instance that finds a foreign token in the lock
shuts itself down at the next heartbeat. A crashed REAPER's orphaned lock is
recovered automatically after 10 seconds rather than requiring manual cleanup.

**The request id is the only caller-influenced value that reaches a path.** It
must match `^[A-Za-z0-9][A-Za-z0-9._-]*$`, be at most 128 bytes, contain no `..`
substring, and equal the stem of the file it arrived in. It is validated
*before* any path is constructed. The server generates ids from its own UUID
generator and never lets a caller influence them. A file whose stem fails the
pattern is quarantined without a reply.

**Ordered, first-failure validation.** Twenty checks in a fixed order: size,
JSON parse, object shape, protocol version, optional bridge-version pin,
instance token, id pattern, id-equals-stem, replay guard, expiry parse, expiry
with 5-second skew tolerance, `created_at` parse, command presence, command
allowlist, payload shape, `expected_project` shape, REAPER version support,
active project, preconditions, command-specific payload validation.

**Replay guard.** The last 512 request ids that reached the replay check are
remembered in memory. A reused id is `DUPLICATE_REQUEST`. Ids are never reused.

**Expiry.** Every request carries `expires_at`. A late request is
`EXPIRED_REQUEST`, with a 5-second clock-skew tolerance. A command file the
bridge never claims is deleted after 300 seconds.

**Size limits, enforced before writing.** The client refuses to write a request
larger than 1 MiB rather than writing it and letting the bridge reject it. The
bridge replaces an oversized result with a `RESULT_TOO_LARGE` error envelope.

**Bounded, targeted garbage collection.** Stale commands (300 s), abandoned
processing files (120 s, moved to `failed/` rather than deleted), uncollected
results (900 s), stray temp files (60 s) and old failure artefacts (24 h). GC
only ever touches files whose names end in one of the protocol's own suffixes,
inside the matching subdirectory. Any other file in the IPC tree is left alone
forever — including yours.

**Liveness before writing.** The client checks `heartbeat.json` first and
reports `BRIDGE_OFFLINE` *without writing a command file* when the bridge is not
alive. A dead bridge never accumulates a queue.

### The hash caveat, stated plainly

The content hashes are **FNV-1a-64**, rendered as `fnv1a64:` plus 16 lowercase
hex digits.

**This is a non-cryptographic hash.** It reliably detects accidental change —
you moved the item, edited a note, changed the tempo. It is **not** collision
resistant against a motivated adversary, and it is **not** an authentication
tag. Both sides must treat a hash match as "probably unchanged", never as an
authorisation.

That is an accepted trade-off, and the reason it is acceptable is that the
threat it would defend against does not cross a boundary that exists here: the
bridge is a local, same-user, file-based channel, and anyone able to forge a
snapshot hash could simply edit the project directly. It is also 64 bits over
data that changes constantly, evaluated dozens of times a second inside REAPER's
UI thread, where a cryptographic hash would be a real cost for no real gain.

If this ever becomes a boundary that matters, that decision must be revisited.

### The installation token, stated plainly

`instance_token` is 32 lowercase hex characters from a cryptographic RNG, and
comparison is exact. It exists so that **an unrelated file dropped into the IPC
directory is never executed as a command**.

It is **not a security boundary.** It sits in a plaintext file readable by your
own user account, next to the directory it protects. Any program running as you
can read it. Do not treat it as a secret that protects anything.

---

## 6. Remaining local-machine risks

Honestly enumerated. None of these are defended against, and none of them can be
from inside this product.

- **Any process running as your user account** can read `config.json`, write
  into `<ipc-dir>/commands/`, and therefore drive the bridge exactly as the
  server does. It can also just edit your `.rpp` file directly, which is
  strictly easier.
- **`<ipc-dir>` inherits its permissions from its parent.** The installer
  creates it inside the REAPER resource tree without tightening the ACL. If your
  REAPER resource directory is on a shared drive or a world-writable path, the
  IPC channel is as exposed as that path. Do not put it somewhere other accounts
  can write.
- **The bridge log may contain project metadata** — project name, project path,
  item and take GUIDs, note counts, error details. Full note content is
  redacted, but the log is not a secret store. It lives at
  `<ipc-dir>/logs/bridge.log`.
- **`<ipc-dir>/failed/` retains request bodies for 24 hours**, including edit
  plans with generated note data. It is diagnostic material, kept deliberately,
  and it is deleted on schedule.
- **`config.json.<timestamp>.bak` files** left by the installer contain old
  installation tokens. Delete them once you are sure you do not need to roll
  back.
- **The MCP host sees everything you ask about.** Analyses, candidates and
  traces are returned to the client, which is presumably a language-model
  application with its own data-handling behaviour. This product does not send
  anything anywhere, but it also does not control what your client does with a
  response.
- **REAPER plugins run in REAPER's process.** Nothing here sandboxes them, reads
  them, or is protected from them.
- **A staged candidate is real project content.** If you save the project while
  a preview is staged, it is saved with the project. That is normal REAPER
  behaviour and it is what makes discard and undo useful.

---

## 7. Data retention

Nothing musical is persisted by the MCP server. All of it is in memory, bounded
and TTL'd, and gone when the process exits.

| Data | Where | Retention |
|---|---|---|
| Snapshots, analyses, candidates, traces, edit plans, transactions | MCP server memory | TTL'd; bounded; destroyed on exit. **Never written to disk.** |
| IPC command files | `<ipc-dir>/commands/` | Seconds. Unclaimed files deleted after 300 s. |
| IPC result files | `<ipc-dir>/results/` | Read and deleted immediately by the client. Uncollected results deleted after 900 s. |
| In-flight requests | `<ipc-dir>/processing/` | Deleted on success. Abandoned after 120 s, moved to `failed/`. |
| Failed requests, with bodies | `<ipc-dir>/failed/` | **24 hours**, then deleted. |
| Bridge log | `<ipc-dir>/logs/bridge.log[.1..3]` | Rotated at 1 MiB, 3 generations kept. Bounded at ~4 MiB total. |
| Heartbeat, lock | `<ipc-dir>/` | Overwritten continuously; meaningless once the bridge stops. |
| Installation token, IPC directory, log level | `config.json` | Until you reinstall or uninstall. Backed up on replace. |
| Staged tracks and items | Your REAPER project | Until commit, discard, undo — or until you save the project with them in it. |
| Transaction records | Project ext state, in your `.rpp` | Capped at 32, newest last. Survives saving. |
| Knowledge bundle | Compiled into the binary | Immutable. |

**Cache location: there isn't one on disk.** The candidate cache is an in-memory
map keyed on (snapshot, profile, params, knowledge hash, seed), bounded, and
never consulted across a changed snapshot. There is no cache directory to clear,
no database, no temp files outside `<ipc-dir>`.

**To clear everything:** stop the bridge, delete `<ipc-dir>`, restart the
bridge. It recreates the tree at startup. Then discard or undo any staged
candidates still in your project.

---

## 8. Logging and redaction

- **`stdout` carries MCP protocol bytes only.** No banners, no logs, no progress
  text, no panic messages, no stack traces. A panic hook writes to `stderr` and
  returns a JSON-RPC error.
- **All server diagnostics go to `stderr`**, where your MCP host will surface
  them if it surfaces anything.
- **Full note content is never logged at normal level.** Counts, spans, hashes
  and ids are; the notes themselves are not.
- **The bridge log is bounded** at 1 MiB per file with 3 rotated generations,
  and defaults to `info`. Set `"log_level": "debug"` in `config.json` when
  diagnosing, and set it back afterwards — debug is more verbose about what
  passed through the channel.
- **`"console_log": true`** mirrors the bridge log into the ReaScript console.
  Useful while debugging, noisy otherwise.

---

## 9. Recovery procedures

### I staged something and want it gone

Ask for a discard, naming the transaction. It removes only objects carrying
`QLABS_OWNER = QLabs-Reaper-MCP` and that transaction id. A tagged track still
holding items that are not part of the transaction is kept, and you are told so.

If you would rather step back through REAPER's own history, use the undo tool.
It undoes at most one entry, and only when REAPER's top undo entry is exactly
this product's last owned transaction. If anything else happened in between it
returns `UNDO_NOT_OWNED` and names the entry it found — use discard instead.

Failing both, everything staged carries a `QLabs MCP: ` undo label and lives in
one clearly named folder track. Delete it by hand.

### A stage failed halfway

It should not have. The whole plan is validated before `Undo_BeginBlock2` is
called, so a plan that fails validation performs zero mutations and creates zero
undo entries. If a mutation fails after that, the bridge restores UI refresh,
closes the block, verifies that the new top undo entry is the failed owned
transaction, undoes it when safe, and returns a structured failure with
`error.details.rolled_back` telling you whether the rollback happened.

If `rolled_back` is `false`, the partial transaction is still tagged. Discard it
by transaction id.

### The bridge will not start: "already running"

A live instance holds `bridge.lock`. The message reports the lock's age.

- If REAPER really does have the bridge running, that is correct behaviour.
- If REAPER crashed, wait 10 seconds and try again — a stale lock is taken over
  automatically.
- Only if you are certain no bridge is running should you delete
  `<ipc-dir>/bridge.lock` by hand.

### Everything times out

The server and the bridge are looking at different directories. Compare
`heartbeat.json`'s `ipc_dir` with the `--ipc-dir` your MCP host passes. Run
`QLabs_Reaper_MCP_Status.lua` inside REAPER for the bridge's own view. Fix the
host configuration, or reinstall with the right `-IpcDirectory`.

### The token no longer matches

`INVALID_INSTANCE_TOKEN` means `config.json` and the server disagree. The bridge
logs a warning when it mints a new token. Re-run
`scripts\install.ps1 -PreserveToken`, or read the token out of `config.json` and
restart both sides.

### The IPC directory has filled up with junk

It is self-cleaning on the schedules in §7. If you want it clean now: stop the
bridge, delete `<ipc-dir>`, restart the bridge. Nothing of value lives there —
all state that matters is in your project or in the server's memory.

### I want to know exactly what was done to my project

Every staged, committed or discarded object carries tags recording the
transaction id, candidate id, plan id, source snapshot id, knowledge version,
creation time and commit time. The project's ext state holds the last 32
transaction records. Every REAPER undo entry this product created begins with
`QLabs MCP: `. Between those three, the audit trail is complete and lives with
your project rather than in a separate log.

### I need to remove all traces

```powershell
.\scripts\uninstall.ps1 -RemoveKnowledgeOverrides -RemoveIpcDirectory
```

Then delete any `config.json.*.bak` files, remove the action from REAPER's
action list, remove the server from your MCP host configuration, and discard any
staged candidates left in your projects.

---

## 10. Reporting a vulnerability

Open an issue at the project repository. Please include the version, the
platform, and a reproduction that does not require your project files.

Please do **not** report the following as vulnerabilities; they are documented,
accepted properties described above:

- FNV-1a-64 is not collision resistant (§5).
- The installation token is readable by the local user (§5).
- A process running as the same user can drive the bridge (§6).
- The bridge log contains project metadata (§6, §8).
