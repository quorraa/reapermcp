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
# throwaway projects written only to clear REAPER's "modified" flag
DISCARD = os.path.join(SCRATCH, "discard").replace("\\", "/")
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
local __CLOSE = {close}
local __SCRATCH = [[{scratch}]]

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
    -- Save to a scratch path first: that clears REAPER's "modified" flag, so
    -- the close raises no dialog, and it never touches the real project file.
    reaper.Main_SaveProjectEx(0, __SCRATCH .. "/discard_" .. closed .. ".RPP", 0)
    reaper.Main_OnCommand(40860, 0)   -- File: Close current project tab
    closed = closed + 1
  end
  return closed
end

if __CLOSE then __discard_tabs(1) end
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

-- Close every project tab this script opened, so REAPER does not end the
-- session with a hundred of them. Scripts that mean to change a project save it
-- themselves; nothing here writes to a real project file.
--
-- This runs *after* the log is finalised on purpose: closing the tab the script
-- is running in terminates the script, so anything after it may never execute.
if __CLOSE then __discard_tabs(1) end
"""


def reaper_running():
    try:
        out = subprocess.run(
            ["tasklist", "/FI", "IMAGENAME eq reaper.exe", "/NH"],
            capture_output=True, text=True, timeout=15).stdout
    except Exception:
        return True          # if the check itself fails, do not kill the run
    return "reaper.exe" in out.lower()


def run_lua(body, timeout=45, keep_open=False):
    """Execute `body` in REAPER; return its say() output as a list of lines.

    `keep_open` leaves any project tab the script opened in place. Plugins
    restore their state asynchronously after a project loads, so anything that
    inspects plugin state has to open the project in one call and read it in a
    later one; reading in the same call sees defaults and reports, wrongly, that
    nothing loaded.
    """
    _seq[0] += 1
    tag = "x%03d_%d" % (_seq[0], int(time.time()))
    script = os.path.join(SCRATCH, "exec_%s.lua" % tag)
    log = os.path.join(SCRATCH, "exec_%s.log" % tag)
    indented = "\n".join("  " + ln for ln in body.strip().splitlines())
    with open(script, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(PRELUDE.format(log=log, body=indented, scratch=DISCARD,
                                close="false" if keep_open else "true"))

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


CLEANUP = """
-- Close every project tab except one, whatever left them behind. A run that
-- died partway leaves its tabs open and modified, and REAPER asks about each of
-- them the next time it tries to quit. Saving to a throwaway path clears the
-- modified flag without touching the real project file.
local closed = 0
for _ = 1, 64 do
  local n = 0
  while reaper.EnumProjects(n, "") do n = n + 1 end
  if n <= 1 then break end
  reaper.Main_SaveProjectEx(0, SCRATCH .. "/leftover_" .. closed .. ".RPP", 0)
  reaper.Main_OnCommand(40860, 0)
  closed = closed + 1
end
say("closed " .. closed .. " leftover project tab(s)")
"""


def cleanup_tabs(timeout=120):
    """Close project tabs left behind by earlier runs."""
    return run_lua("local SCRATCH = [[%s]]\n%s" % (DISCARD, CLEANUP),
                   timeout=timeout)


if __name__ == "__main__":
    import sys
    if "--cleanup" in sys.argv:
        for line in cleanup_tabs():
            print(line)
    else:
        for line in run_lua('say("reaper " .. reaper.GetAppVersion())'):
            print(line)
