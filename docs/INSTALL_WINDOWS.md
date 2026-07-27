# Installing on Windows

Step-by-step installation of the QLabs REAPER Music Intelligence MCP on
**Windows 11** with **REAPER 7.x**. That combination is the primary supported
platform and the one acceptance was performed against.

Nothing here requires administrator rights, an internet connection, SWS,
ReaPack, JS_ReaScriptAPI or any other REAPER extension.

---

## Contents

- [Before you start](#before-you-start)
- [1. Get the files](#1-get-the-files)
- [2. Build the executable](#2-build-the-executable)
- [3. Run the installer](#3-run-the-installer)
- [4. Register the bridge in REAPER](#4-register-the-bridge-in-reaper)
- [5. Start the bridge](#5-start-the-bridge)
- [6. Configure your MCP host](#6-configure-your-mcp-host)
- [7. Verify](#7-verify)
- [What was installed where](#what-was-installed-where)
- [Portable REAPER](#portable-reaper)
- [Manual installation](#manual-installation)
- [Upgrading](#upgrading)
- [Uninstalling](#uninstalling)
- [If something goes wrong](#if-something-goes-wrong)

---

## Before you start

You need:

| | |
|---|---|
| **Windows 11** | Windows 10 is untested but nothing in the scripts is 11-specific. |
| **REAPER 7.x** | REAPER 6 is accepted by the bridge (it refuses anything below major version 6) but 7.x is the target. |
| **PowerShell 5.1 or later** | Ships with Windows. `$PSVersionTable.PSVersion` to check. PowerShell 7 also works. |
| **A stable Rust toolchain** | Only needed to *build*. Skip it if you have a prebuilt package. Get it from rustup. |
| **Lua 5.4** | Optional, and only for running the bridge test suite. Not needed to use the product. |

**Find your REAPER resource path now**, because you may need it. In REAPER:
*Options → Show REAPER resource path*. A File Explorer window opens on it. The
usual location for a normal installation is:

```text
C:\Users\<you>\AppData\Roaming\REAPER
```

For a **portable** installation it is the REAPER program folder itself. Do not
assume `%APPDATA%\REAPER` — check.

If PowerShell refuses to run the scripts, allow them for the current session
only:

```powershell
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass
```

That lasts until you close the window and changes nothing permanently.

---

## 1. Get the files

Either clone the repository:

```powershell
git clone https://github.com/quorraa/reapermcp.git
cd reapermcp
```

or unzip a staged package produced by `scripts\package.ps1` and `cd` into it. A
staged package already contains the executable, so you can skip step 2.

---

## 2. Build the executable

From the repository root:

```powershell
cargo build --workspace --release
```

The workspace has **zero external dependencies**, so this needs no network
access and downloads nothing. The result is:

```text
target\release\qlabs-reaper-music-mcp.exe
```

If you skip this, `install.ps1` will run the build for you when it cannot find
the binary — unless you pass `-SkipBuild`.

Optional, and worth doing once:

```powershell
.\scripts\validate-release.ps1
```

That runs formatting, lints, the Rust test suite, the knowledge/schema/fixture
gates, the Lua bridge suite and the release build, and prints a pass/fail
summary.

---

## 3. Run the installer

```powershell
.\scripts\install.ps1
```

Want to see exactly what it would do first? Every write is guarded:

```powershell
.\scripts\install.ps1 -WhatIf
```

The installer:

1. Locates the source tree and the release executable, building it if necessary.
2. Detects your REAPER resource path, or uses the one you gave it.
3. Creates the destination directories.
4. Copies `qlabs-reaper-music-mcp.exe`.
5. Copies the knowledge bundle. *(The bundle is also compiled into the binary;
   this copy is for inspection and for `--knowledge-dir` overrides.)*
6. Copies the Lua bridge — and **only** this product's own files. Unrelated
   scripts already in your REAPER `Scripts` directory are never removed,
   overwritten or read. `reaper.ini` is never touched.
7. Generates a cryptographically random 32-hex-character installation token
   from the OS RNG.
8. Backs up any existing QLabs `config.json` to
   `config.json.<timestamp>.bak` before replacing it.
9. Writes `config.json` — the shared configuration both the bridge and the MCP
   server read.
10. Writes `install-manifest.json`, so the uninstaller can remove exactly what
    was installed and nothing else.
11. Prints the next steps, the exact bridge path and an MCP host snippet with
    your real paths in it.
12. Runs `doctor`.

### Useful switches

| Switch | Effect |
|---|---|
| `-ReaperResourcePath 'D:\REAPER-Portable'` | Use a specific resource path. **Required for portable installs.** |
| `-Destination 'C:\Tools\QLabs'` | Put the executable and knowledge copy elsewhere. |
| `-IpcDirectory 'D:\qlabs-ipc'` | Use a specific IPC directory instead of `<script dir>\ipc`. |
| `-ExecutablePath '.\qlabs-reaper-music-mcp.exe'` | Install a binary from an explicit path (useful for staged packages). |
| `-PreserveToken` | Keep the token from an existing `config.json` so an already-configured host keeps working. |
| `-SkipBuild` | Never invoke cargo. Fails if the binary is missing. |
| `-SkipDoctor` | Do not run the post-install check. |
| `-Force` | Accept a resource path that does not look like a REAPER resource directory. |
| `-WhatIf` | Dry run. Writes nothing. |

### On the doctor result

`doctor` will very likely report a failure the first time, and that is
expected: the bridge has not been started yet, so there is no heartbeat to
check. Finish steps 4 and 5, then re-run:

```powershell
.\scripts\run-doctor.ps1
```

---

## 4. Register the bridge in REAPER

This is the one step the installer cannot do for you: REAPER's action list is
REAPER's own state, and this product does not edit REAPER configuration files.

**The procedure, exactly:**

```text
REAPER
→ Actions
→ Show action list
→ New action
→ Load ReaScript
→ Select QLabs_Reaper_MCP_Bridge.lua
→ Run the registered action
```

The file to select is the one the installer printed:

```text
<REAPER Resource Path>\Scripts\QLabs-Reaper-MCP\QLabs_Reaper_MCP_Bridge.lua
```

For a standard installation that is usually:

```text
C:\Users\<you>\AppData\Roaming\REAPER\Scripts\QLabs-Reaper-MCP\QLabs_Reaper_MCP_Bridge.lua
```

Notes:

- "New action" is a dropdown button at the top-right of the Actions window;
  "Load ReaScript..." is one of its entries.
- You only do this once. After that the action stays in the list under
  `Script: QLabs_Reaper_MCP_Bridge.lua`.
- You can assign it a keyboard shortcut or bind it to a toolbar button. The
  script sets and clears its own toggle state, so a toolbar button lights up
  while it is running.
- **SWS is not required**, and no startup-action hook is installed. Starting the
  bridge is a deliberate act.

---

## 5. Start the bridge

Run the action. The ReaScript console opens and reports the resolved IPC
directory and config path.

**The bridge must be running for every operation that touches your project.**
`reaper.status`, `reaper.inspect_selection`, `reaper.stage_candidate`,
`reaper.commit_candidate`, `reaper.discard_candidate` and
`reaper.undo_last_generation` all fail with `BRIDGE_OFFLINE` without it. Theory
search, fixture analysis and candidate generation from an existing snapshot do
not need it.

What it is doing while it runs:

- Polling `<ipc-dir>\commands\` at most every 50 ms, on REAPER's UI timer.
- Handling at most 4 commands per timer tick, so a burst cannot stall the UI.
- Rewriting `heartbeat.json` once a second.
- Holding `bridge.lock` so a second instance cannot start.
- Collecting stale files every 30 seconds.

Run the action again to stop it. Closing REAPER stops it too, and writes a final
`"offline"` heartbeat on the way out.

---

## 6. Configure your MCP host

This is a normal local **stdio** MCP server. It opens no network port.

Host configuration file locations differ from client to client and are not
standardized — consult your MCP client's own documentation for where its server
list lives. The configuration content is the same everywhere:

```json
{
  "mcpServers": {
    "qlabs-reaper-music": {
      "command": "C:\\Users\\you\\AppData\\Local\\Programs\\QLabs-Reaper-MCP\\qlabs-reaper-music-mcp.exe",
      "args": [
        "serve",
        "--ipc-dir",
        "C:\\Users\\you\\AppData\\Roaming\\REAPER\\Scripts\\QLabs-Reaper-MCP\\ipc"
      ]
    }
  }
}
```

Two things people get wrong here:

- **Backslashes must be doubled.** That is JSON escaping, not a typo.
- **`serve` is required.** Without it the binary runs a CLI subcommand instead
  of the server, and the host will report that the server exited.

Do not retype the paths. The installer printed this snippet with your real
paths, and you can regenerate it at any time:

```powershell
& "$env:LOCALAPPDATA\Programs\QLabs-Reaper-MCP\qlabs-reaper-music-mcp.exe" print-mcp-config
```

Restart your MCP client after editing its configuration.

---

## 7. Verify

Three checks, from three different angles.

**From PowerShell:**

```powershell
.\scripts\run-doctor.ps1
```

With the bridge running, every check should pass. `doctor` inspects
configuration, the IPC directory, permissions, the installation token,
knowledge validation, the knowledge hash, the bridge heartbeat and version, the
REAPER version, resource-path consistency, stale commands, whether the result
directory is writable, and the server version. Add `-Json` for machine-readable
output.

**From inside REAPER:** register and run `QLabs_Reaper_MCP_Status.lua` the same
way you registered the bridge. It reports the same ground truth from REAPER's
side: resolved directories, config presence and a redacted token, heartbeat age,
lock holder, per-directory file counts, the active project and its UUID, what it
would currently resolve as the MIDI source, the live hashes, staged transactions
and the top undo entry.

**From your assistant:** open a project, select one MIDI item, and ask it to
check the REAPER connection. It should report the bridge version, the REAPER
version and an active project. Then ask it to look at your selection.

---

## What was installed where

| Path | What |
|---|---|
| `%LOCALAPPDATA%\Programs\QLabs-Reaper-MCP\qlabs-reaper-music-mcp.exe` | The MCP server and CLI |
| `%LOCALAPPDATA%\Programs\QLabs-Reaper-MCP\knowledge\` | A copy of the theory bundle |
| `%LOCALAPPDATA%\Programs\QLabs-Reaper-MCP\install-manifest.json` | Exactly what was installed, for the uninstaller |
| `<resource>\Scripts\QLabs-Reaper-MCP\QLabs_Reaper_MCP_Bridge.lua` | The bridge |
| `<resource>\Scripts\QLabs-Reaper-MCP\QLabs_Reaper_MCP_Status.lua` | The diagnostic script |
| `<resource>\Scripts\QLabs-Reaper-MCP\QLabs_Reaper_MCP_Smoke_Test.lua` | The guarded in-REAPER smoke test |
| `<resource>\Scripts\QLabs-Reaper-MCP\lib\*.lua` | The bridge's seven modules |
| `<resource>\Scripts\QLabs-Reaper-MCP\config.json` | Installation token, IPC directory, log level |
| `<resource>\Scripts\QLabs-Reaper-MCP\ipc\` | The IPC working directory, created at runtime |

`config.json` is the file both halves read:

```json
{
  "instance_token": "0123456789abcdef0123456789abcdef",
  "ipc_dir": null,
  "poll_interval_ms": 50,
  "heartbeat_interval_ms": 1000,
  "log_level": "info",
  "console_log": false
}
```

`ipc_dir: null` means "`ipc` beside this file", which is what you want unless
you asked for something else. Set `"log_level": "debug"` and
`"console_log": true` when diagnosing a problem.

The `instance_token` is **not a security boundary.** It exists so that an
unrelated file dropped into the IPC directory is never executed as a command. It
does not protect anything from another program running as you — see
[`SECURITY.md`](SECURITY.md).

---

## Portable REAPER

Portable installations are fully supported. The bridge resolves its own location
at runtime and never hardcodes `%APPDATA%`.

You must tell the installer where REAPER lives, because there is nothing to
auto-detect:

```powershell
.\scripts\install.ps1 -ReaperResourcePath 'D:\REAPER-Portable'
```

For a portable install the resource path is the REAPER program folder itself —
the one containing `reaper.exe` and `reaper.ini`. Confirm with
*Options → Show REAPER resource path*.

If you later move the whole REAPER folder, the bridge keeps working, but the
`--ipc-dir` in your MCP host configuration will be stale. Re-run
`print-mcp-config` and update it.

---

## Manual installation

If you would rather not run a script, or you are on a machine where PowerShell
is locked down:

1. Copy `target\release\qlabs-reaper-music-mcp.exe` anywhere you like.
2. Copy the whole `reaper\` directory to
   `<resource>\Scripts\QLabs-Reaper-MCP\`. The `reaper\tests\` subdirectory is
   not needed at runtime.
3. Start the bridge (steps 4 and 5). On first run, **with no `config.json`
   present, the bridge mints its own token, writes `config.json` and logs a
   warning.** That is the supported path.
4. Open the `config.json` it wrote and read the `instance_token`. The MCP server
   reads that same file — it must never invent a token of its own.
5. Configure your MCP host with the executable path and
   `--ipc-dir <resource>\Scripts\QLabs-Reaper-MCP\ipc`.

Copying the knowledge bundle is optional: it is compiled into the executable.

---

## Upgrading

```powershell
git pull
cargo build --workspace --release
.\scripts\install.ps1 -PreserveToken
```

`-PreserveToken` keeps the existing installation token, so your MCP host
configuration and any running bridge keep working. Without it a new token is
minted, the old `config.json` is backed up, and **you must restart the bridge**
so it picks the new one up.

Stop the bridge before upgrading — the Lua files are replaced in place, and a
running bridge holds the old ones in memory until it restarts.

---

## Uninstalling

```powershell
.\scripts\uninstall.ps1
```

It removes only files this product installed, using the install manifest. It
preserves anything you added to the knowledge directory, and it leaves the IPC
directory alone because that is runtime state rather than an installed artefact.
`config.json` is backed up before it is deleted.

To remove everything:

```powershell
.\scripts\uninstall.ps1 -RemoveKnowledgeOverrides -RemoveIpcDirectory
```

Preview first with `-WhatIf`. The uninstaller refuses to run while the bridge
heartbeat says it is alive, unless you pass `-Force`.

Two things it cannot do for you:

- **Remove the action from REAPER's action list.** Do it in
  *Actions → Show action list*, find `QLabs_Reaper_MCP_Bridge`, and delete it.
- **Remove the server from your MCP host configuration.** Delete the
  `qlabs-reaper-music` entry yourself.

---

## If something goes wrong

| Symptom | What to do |
|---|---|
| `install.ps1` cannot find REAPER | Pass `-ReaperResourcePath` with the path from *Options → Show REAPER resource path*. |
| "cannot be loaded because running scripts is disabled" | `Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass`, then re-run in the same window. |
| "already running" when starting the bridge | A live instance holds `bridge.lock`. Check the age in the message. Only delete the lock if you are certain no bridge is running. |
| `BRIDGE_OFFLINE` from every tool | The bridge action is not running, or the server points at a different `ipc_dir`. Compare `heartbeat.json`'s `ipc_dir` with the `--ipc-dir` in your host config. |
| `INVALID_INSTANCE_TOKEN` | The server and `config.json` disagree. Re-run `install.ps1 -PreserveToken`, or restart the bridge after a reinstall. |
| `IPC_TIMEOUT` on everything | Same directory mismatch as `BRIDGE_OFFLINE`, but the bridge is alive somewhere else. |
| The host shows no tools | `serve` is missing from `args`, or the path to the exe is wrong. Check your host's own server log. |
| `NO_MIDI_SOURCE` | Open a MIDI editor, or select exactly one MIDI item. |
| Nothing at all is happening | Run `QLabs_Reaper_MCP_Status.lua` inside REAPER. It tells you what the bridge thinks is true. |

The bridge log is at `<ipc-dir>\logs\bridge.log`, rotated at 1 MiB with three
generations kept. Failed requests and their bodies are kept in `<ipc-dir>\failed\`
for 24 hours.

More: [`REAPER_BRIDGE.md`](REAPER_BRIDGE.md) for the bridge internals,
[`KNOWN_LIMITATIONS.md`](KNOWN_LIMITATIONS.md) for what is genuinely not
supported, and the README's troubleshooting table for the error-code index.
