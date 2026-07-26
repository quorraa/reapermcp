# QLabs REAPER MCP bridge (Lua)

The REAPER side of the QLabs REAPER Music Intelligence MCP. Pure Lua 5.4, no
external packages, no network port, no arbitrary code execution.

For the full protocol reference see [`../docs/REAPER_BRIDGE.md`](../docs/REAPER_BRIDGE.md).

## Layout

```
reaper/
├── QLabs_Reaper_MCP_Bridge.lua      persistent defer() polling bridge
├── QLabs_Reaper_MCP_Status.lua      one-shot diagnostic report
├── QLabs_Reaper_MCP_Smoke_Test.lua  guarded in-REAPER end-to-end test
├── config.json                      created on first run; holds the install token
├── lib/
│   ├── json.lua          pure-Lua JSON encode/decode (MIT, written for this project)
│   ├── util.lua          paths, time, FNV-1a-64, swappable filesystem, bounded logging
│   ├── protocol.lua      envelope validation, command allowlist, limits, error codes
│   ├── tagging.lua       P_EXT ownership tags, project ext state, project UUID
│   ├── snapshot.lua      source resolution, MIDI reading, canonical hashing
│   ├── transactions.lua  edit-plan validation and execution, undo blocks
│   └── bridge.lua        the engine: lock, heartbeat, IPC state machine, dispatch
├── tests/
│   ├── run_tests.lua     the test runner  (lua5.4 reaper/tests/run_tests.lua)
│   ├── gen_fixtures.lua  regenerates fixtures/mock-reaper/**
│   ├── mock_reaper.lua   in-memory mock of every reaper.* function used
│   ├── mock_fs.lua       in-memory filesystem backing util.fs
│   ├── harness.lua       assertions and the test environment builder
│   └── test_*.lua        per-module suites
└── ipc/                  created at runtime; see below
```

## Installing

Copy this whole directory to:

```
<REAPER Resource Path>/Scripts/QLabs-Reaper-MCP/
```

`<REAPER Resource Path>` is whatever `reaper.GetResourcePath()` returns —
Options > Show REAPER resource path. **Never assume `%APPDATA%\REAPER`**:
portable installs are supported and the bridge resolves its own location from
`debug.getinfo(1,"S").source`.

Then in REAPER: Actions > Show action list > New action > Load ReaScript, and
load `QLabs_Reaper_MCP_Bridge.lua`. Run it. The ReaScript console reports the IPC
directory and config path.

On first run the bridge creates `config.json` next to the script with a freshly
minted 32-hex-character installation token. Point the MCP server at that token
and at the IPC directory:

```
qlabs-reaper-music-mcp serve --ipc-dir "<resource path>/Scripts/QLabs-Reaper-MCP/ipc"
```

Binding the bridge to a toolbar button is supported: the script sets and clears
its own toggle state.

## Runtime directory

```
ipc/
├── commands/     the MCP server writes here
├── processing/   bridge-owned, in flight
├── results/      the bridge writes here, the server reads and deletes
├── failed/       bridge-owned, quarantined requests
├── logs/         bridge.log, rotated at 1 MiB, 3 generations kept
├── heartbeat.json
└── bridge.lock
```

Set `"ipc_dir"` in `config.json` to relocate it. Everything else stays put.

## Running the tests without REAPER

```
lua5.4 reaper/tests/run_tests.lua            # all suites
lua5.4 reaper/tests/run_tests.lua snapshot   # filter by substring
```

The suites drive the real bridge code against `mock_reaper.lua` (a faithful
in-memory REAPER host, including a working undo journal and a UI-refresh depth
counter) and `mock_fs.lua` (an in-memory filesystem installed as `util.fs`).
No REAPER installation is required and nothing is written outside memory.

`gen_fixtures.lua` regenerates `fixtures/mock-reaper/**`, which
`test_fixtures.lua` then re-verifies on every run — the cross-language contract
with the Rust IPC client cannot drift silently.

## Running the in-REAPER smoke test

`QLabs_Reaper_MCP_Smoke_Test.lua` writes to the active project, so it refuses to
run against a project that is saved to disk or has unsaved changes. Do
File > New Project first, then run it. If you really want it to run against the
current project, open the script and set `ALLOW_MODIFY_THIS_PROJECT = true`.

It creates its own clearly marked material, verifies inspection, stages a
fixture candidate, confirms the source is unchanged, confirms every generated
object is tagged, confirms one undo restores the prior state, then removes its
own material. One PASS/FAIL line per step goes to the ReaScript console.

## What the bridge will not do

There is no command that evaluates Lua, runs an action id, reads or writes a
path outside the configured IPC directory, deletes anything by name, or passes
values through to the REAPER API. The command allowlist is seven entries; the
edit-operation allowlist is seven entries; the ownership-tag key allowlist is
fourteen entries. All three are closed sets in `lib/protocol.lua`,
`lib/transactions.lua` and `lib/tagging.lua` respectively.

## Adding a command (for maintainers)

1. Add it to `protocol.COMMANDS` with `needs_project` / `writes`.
2. Add a handler to `bridge.handlers`.
3. Document the payload and result field-by-field in `docs/REAPER_BRIDGE.md`
   and in the IPC wire spec the Rust side is written against.
4. Add it to `schemas/ipc-request.schema.json` and `schemas/ipc-result.schema.json`.
5. Add tests, including at least one failure path.
6. Regenerate the fixtures.

Nothing works until step 1, which is the point.
