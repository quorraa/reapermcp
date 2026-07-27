"""Drive qlabs-reaper-music-mcp over stdio as a real MCP host would.

One server process for the whole run, because snapshot/analysis/candidate ids
live in that process's session store. Steps are (tool, arguments) pairs; a step
may be a callable taking the accumulated results dict and returning that pair,
so later calls can use ids issued by earlier ones.
"""
import json
import os
import subprocess
import sys

EXE = os.path.join(os.environ["LOCALAPPDATA"], "Programs", "QLabs-Reaper-MCP",
                   "qlabs-reaper-music-mcp.exe")
CONFIG = os.path.join(os.environ["APPDATA"], "REAPER", "Scripts", "QLabs-Reaper-MCP",
                      "config.json")


class Server:
    def __init__(self, log_level="warn"):
        env = dict(os.environ, QLABS_MCP_LOG=log_level)
        self.p = subprocess.Popen(
            [EXE, "serve", "--config", CONFIG],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, encoding="utf-8", bufsize=1, env=env,
        )
        self._id = 0
        self.request("initialize", {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "qlabs-driver", "version": "1.0"},
        })
        self.notify("notifications/initialized")

    def _send(self, obj):
        self.p.stdin.write(json.dumps(obj) + "\n")
        self.p.stdin.flush()

    def notify(self, method, params=None):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        self._send(msg)

    def request(self, method, params=None):
        self._id += 1
        msg = {"jsonrpc": "2.0", "id": self._id, "method": method}
        if params is not None:
            msg["params"] = params
        self._send(msg)
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError("server closed stdout; stderr:\n" + self.p.stderr.read())
            reply = json.loads(line)
            if reply.get("id") == self._id:
                return reply

    def call(self, tool, arguments):
        reply = self.request("tools/call", {"name": tool, "arguments": arguments})
        if "error" in reply:
            return {"__protocol_error__": reply["error"]}
        result = reply["result"]
        if result.get("isError"):
            text = "".join(c.get("text", "") for c in result.get("content", []))
            try:
                return {"__tool_error__": json.loads(text)}
            except json.JSONDecodeError:
                return {"__tool_error__": text}
        if "structuredContent" in result:
            return result["structuredContent"]
        return {"__text__": "".join(c.get("text", "") for c in result.get("content", []))}

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()


def run(steps, log_level="warn"):
    srv = Server(log_level)
    out = {}
    try:
        for name, step in steps:
            tool, args = step(out) if callable(step) else step
            print("=" * 70)
            print("%s  ->  %s" % (name, tool))
            print("   args: %s" % json.dumps(args))
            res = out[name] = srv.call(tool, args)
            print(json.dumps(res, indent=2)[:4000])
            if "__tool_error__" in res or "__protocol_error__" in res:
                print("!! stopping: step %r failed" % name)
                break
    finally:
        srv.close()
    return out


if __name__ == "__main__":
    tool = sys.argv[1]
    args = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}
    run([(tool, (tool, args))])
