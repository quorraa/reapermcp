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
REAPER = os.environ.get("QLABS_REAPER_EXE",
                        r"C:\Program Files\REAPER (x64)\reaper.exe")
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
local __ok, __err = pcall(function()
{body}
end)
if not __ok then say("LUA_ERROR: " .. tostring(__err)) end
say("__DONE__")
__f:close()
"""


def run_lua(body, timeout=45):
    """Execute `body` in REAPER; return its say() output as a list of lines."""
    _seq[0] += 1
    tag = "x%03d_%d" % (_seq[0], int(time.time()))
    script = os.path.join(SCRATCH, "exec_%s.lua" % tag)
    log = os.path.join(SCRATCH, "exec_%s.log" % tag)
    indented = "\n".join("  " + ln for ln in body.strip().splitlines())
    with open(script, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(PRELUDE.format(log=log, body=indented))

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

    subprocess.run([REAPER, "-nonewinst", script], check=False,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    deadline = time.time() + timeout
    while time.time() < deadline:
        if os.path.exists(log):
            with open(log, encoding="utf-8") as fh:
                lines = fh.read().splitlines()
            if lines and lines[-1] == "__DONE__":
                return lines[:-1]
        time.sleep(0.25)
    raise TimeoutError("REAPER did not finish %s within %ss" % (tag, timeout))


if __name__ == "__main__":
    for line in run_lua('say("reaper " .. reaper.GetAppVersion())'):
        print(line)
