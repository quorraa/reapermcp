"""Run a snippet of Lua inside the running REAPER and return what it printed.

Each call gets a uniquely named script file: REAPER caches compiled ReaScripts
by path, so reusing one path can silently re-run stale code (or nothing at all,
if the cached copy failed to compile).

The snippet gets a `say(...)` function that writes to a log file, and runs
inside a pcall so a Lua error is reported rather than lost in a message box.
"""
import os
import shutil
import subprocess
import time

SCRATCH = os.path.dirname(os.path.abspath(__file__))
# Throwaway projects, written only to clear REAPER's "modified" flag before a
# tab is closed. Created eagerly: REAPER will not make the directory itself, and
# a save into a missing path raises a modal "Error creating project file" that
# blocks every subsequent script.
DISCARD = os.path.join(SCRATCH, "discard").replace("\\", "/")
os.makedirs(DISCARD, exist_ok=True)

# An empty, already-saved project. Loading it into a tab with the "noprompt:"
# prefix discards whatever was there without asking, which leaves the tab
# holding an unmodified file - so closing it cannot raise a save prompt either.
# Saving each tab to a throwaway path was the previous approach and it still
# prompted; this does not.
_BLANK = os.path.join(DISCARD, "_blank.RPP")
if not os.path.exists(_BLANK):
    with open(_BLANK, "w", encoding="utf-8", newline="\n") as _fh:
        _fh.write('<REAPER_PROJECT 0.1 "7.78" 0\n  TEMPO 120 4 4\n>\n')
BLANK = _BLANK.replace("\\", "/")
REAPER = r"C:\Program Files\REAPER (x64)\reaper.exe"
_seq = [0]

PRELUDE = """local LOG = [[{log}]]
local NL = string.char(10)
local __f = io.open(LOG, "w")
local function say(s) __f:write(tostring(s) .. NL) __f:flush() end
local function findproj(trackname)
  local pi = 0
  while true do
    local p = reaper.EnumProjects(pi, "")
    if not p then break end
    for t = 0, reaper.CountTracks(p) - 1 do
      local tr = reaper.GetTrack(p, t)
      local _, tn = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
      if tn == trackname then return p, tr end
    end
    pi = pi + 1
  end
  return nil, nil
end
local __SWEEP = {sweep}
local __BLANK = [[{blank}]]

-- Sweep up before starting. Closing only the tabs this script opened is not
-- enough: a script that dies partway - crash, timeout, killed batch - orphans
-- its tab, and the orphans pile up until REAPER asks about every one of them at
-- exit. Everything here is a throwaway test project reopened from disk by name,
-- so anything already open can go.
local function __discard_tabs(keep)
  local closed = 0
  for _ = 1, 64 do
    local n = 0
    while reaper.EnumProjects(n, "") do n = n + 1 end
    if n <= keep then break end
    -- Load an empty saved project over the tab, then close it. "noprompt:"
    -- discards the current contents silently and what replaces it is
    -- unmodified, so neither step can raise a dialog.
    reaper.Main_openProject("noprompt:" .. __BLANK)
    reaper.Main_OnCommand(40860, 0)   -- File: Close current project tab
    closed = closed + 1
  end
  return closed
end

if __SWEEP then __discard_tabs(1) end
local __ntabs = 0
do
  local pi = 0
  while reaper.EnumProjects(pi, "") do pi = pi + 1 end
  __ntabs = pi
end
local __ok, __err = pcall(function()
{body}
end)
if not __ok then say("LUA_ERROR: " .. tostring(__err)) end

say("__DONE__")
__f:close()

-- Deliberately does not close anything by default. A workflow that spans
-- several calls - create a project here, inspect it there - is destroyed by an
-- automatic close in between. Call cleanup_tabs() when you actually want it.
if __SWEEP then __discard_tabs(1) end
"""


def remove_render(path, tries=6, wait=2.0):
    """Delete a previous render, waiting for REAPER to let go of it.

    REAPER will not overwrite a render silently - it raises a modal dialog and
    waits - so the file has to go first. It can still hold the handle for a
    moment after finishing, so a single delete attempt raises PermissionError
    and takes the run down with it.
    """
    for attempt in range(tries):
        if not os.path.exists(path):
            return True
        try:
            os.remove(path)
            return True
        except PermissionError:
            if attempt == tries - 1:
                print("  could not delete %s; REAPER still holds it" % path)
                return False
            time.sleep(wait)
        except OSError:
            return False
    return False


def reaper_running():
    try:
        out = subprocess.run(
            ["tasklist", "/FI", "IMAGENAME eq reaper.exe", "/NH"],
            capture_output=True, text=True, timeout=15).stdout
    except Exception:
        return True          # if the check itself fails, do not kill the run
    return "reaper.exe" in out.lower()


def run_lua(body, timeout=45, keep_open=True, sweep_tabs=False):
    """Execute `body` in REAPER; return its say() output as a list of lines.

    Project tabs are left alone unless `sweep_tabs` asks for them to be closed.
    Two reasons this is not automatic:

    * A workflow can span several calls - build a project in one, let the server
      inspect it in the next. Closing tabs in between destroys it.
    * Plugins restore their state asynchronously after a project loads, so
      reading plugin state in the same call that opened the project sees
      defaults and reports, wrongly, that nothing loaded. That too needs the
      project to survive between calls.

    Nothing here creates a tab incidentally: scripts open projects with the
    `noprompt:` prefix, which loads into the current tab. Call `cleanup_tabs()`
    when you want the session tidied.
    """
    _seq[0] += 1
    tag = "x%03d_%d" % (_seq[0], int(time.time()))
    script = os.path.join(SCRATCH, "exec_%s.lua" % tag)
    log = os.path.join(SCRATCH, "exec_%s.log" % tag)
    indented = "\n".join("  " + ln for ln in body.strip().splitlines())
    with open(script, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(PRELUDE.format(log=log, body=indented,
                                blank=BLANK, sweep="true" if sweep_tabs else "false"))

    # Compile-check before handing it to REAPER. A syntax error otherwise shows
    # up as a blocking dialog inside REAPER and a timeout out here, which is a
    # slow and confusing way to learn about a stray bracket.
    luac = (shutil.which("luac") or shutil.which("luac5.4")
            or os.path.join(os.environ.get("LOCALAPPDATA", ""),
                            "Programs", "Lua", "bin", "luac.exe"))
    if not os.path.isfile(luac):
        raise RuntimeError(
            "luac not found (%s). The compile pre-check is not optional: without "
            "it a syntax error becomes a blocking dialog in REAPER and a timeout "
            "here." % luac)
    chk = subprocess.run([luac, "-p", script], capture_output=True, text=True)
    if chk.returncode != 0:
        raise SyntaxError("generated Lua does not compile:\n%s\n%s"
                          % (chk.stdout.strip(), chk.stderr.strip()))

    # Popen, not run: when REAPER is already running this hands the script over
    # and exits immediately, but when it is not, it becomes the new REAPER
    # process and does not return until REAPER quits. Waiting on that blocks
    # forever and looks exactly like a hung script.
    proc = subprocess.Popen([REAPER, "-nonewinst", script],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    deadline = time.time() + timeout
    next_liveness = time.time() + 10.0
    while time.time() < deadline:
        # REAPER crashes under heavy renders, and a crash is indistinguishable
        # from a slow render if all you do is wait: the log simply never gets its
        # last line. Noticing the process is gone turns a 25 minute timeout into
        # an immediate, accurate failure.
        if time.time() > next_liveness:
            next_liveness = time.time() + 10.0
            if not reaper_running():
                raise RuntimeError(
                    "REAPER is no longer running - it exited or crashed while "
                    "executing %s. Check %%LOCALAPPDATA%%\\CrashDumps." % tag)

        if os.path.exists(log):
            with open(log, encoding="utf-8") as fh:
                lines = fh.read().splitlines()
            if lines and lines[-1] == "__DONE__":
                # The tab save/close runs after __DONE__ is written, so REAPER is
                # still busy for a moment. Launching the next script into that
                # window makes it time out, which looks like a hang but is a race.
                time.sleep(1.5)
                return [ln for ln in lines[:-1]
                        if not ln.startswith("__CLOSED__")]
        time.sleep(0.25)

    cold_start = proc.poll() is None
    raise TimeoutError(
        "REAPER did not finish %s within %ss%s"
        % (tag, timeout,
           ". The launcher became the REAPER process, so REAPER was not running "
           "and the new instance is waiting at its startup screen" if cold_start
           else ""))


def cleanup_tabs(timeout=300):
    """Close every project tab except one.

    Runs through the prelude's post-completion sweep rather than as a script
    body. Closing tabs inside the body terminates the script when it closes the
    tab it is itself running in, so it never reports finishing and the call
    times out even though the work was done.
    """
    return run_lua('say("sweeping project tabs")', timeout=timeout,
                   sweep_tabs=True)


if __name__ == "__main__":
    import sys
    if "--cleanup" in sys.argv:
        for line in cleanup_tabs():
            print(line)
    else:
        for line in run_lua('say("reaper " .. reaper.GetAppVersion())'):
            print(line)
