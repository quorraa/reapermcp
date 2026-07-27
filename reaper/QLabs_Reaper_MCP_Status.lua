--[[
  QLabs_Reaper_MCP_Status.lua
  One-shot diagnostic for the QLabs REAPER Music Intelligence MCP bridge.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT

  Run from the Actions list. It reads the installation config, the heartbeat and
  the lock file, counts pending IPC artefacts, and reports what the bridge would
  currently resolve as the MIDI source. It mutates NOTHING except that it may
  mint the persistent project UUID, which is what an inspection would do anyway.
--]]

local SCRIPT_PATH = debug.getinfo(1, "S").source
if SCRIPT_PATH:sub(1, 1) == "@" then SCRIPT_PATH = SCRIPT_PATH:sub(2) end
local SCRIPT_DIR = SCRIPT_PATH:match("^(.*)[/\\][^/\\]*$") or "."

-- The installation directory is the one that actually holds lib/. Normally that
-- is this script's own directory; when the script has been copied elsewhere we
-- fall back to the installed location under REAPER's resource path. One
-- directory is resolved rather than only the module path, because it also
-- decides which config.json and ipc/ this script reports on — reporting on a
-- different installation than the one it loaded would make the diagnosis wrong.
-- Never hardcoded: portable REAPER installs are supported.
local function qlabs_holds_lib(dir)
  local probe = io.open(dir .. "/lib/util.lua", "r")
  if probe then
    probe:close()
    return true
  end
  return false
end

local INSTALL_DIR
local TRIED = { SCRIPT_DIR }
if reaper and reaper.GetResourcePath then
  TRIED[#TRIED + 1] = reaper.GetResourcePath() .. "/Scripts/QLabs-Reaper-MCP"
end
for _, dir in ipairs(TRIED) do
  if not INSTALL_DIR and qlabs_holds_lib(dir) then INSTALL_DIR = dir end
end
INSTALL_DIR = INSTALL_DIR or SCRIPT_DIR

package.path = INSTALL_DIR .. "/lib/?.lua;" .. package.path

local ok_load, load_err = pcall(function()
  _G.QLABS_util = require("util")
  _G.QLABS_json = require("json")
  _G.QLABS_protocol = require("protocol")
  _G.QLABS_bridge = require("bridge")
  _G.QLABS_snapshot = require("snapshot")
  _G.QLABS_tagging = require("tagging")
end)
if not ok_load then
  reaper.ShowMessageBox("QLabs status script could not load lib/: " .. tostring(load_err),
    "QLabs REAPER MCP", 0)
  return
end

local util = _G.QLABS_util
local json = _G.QLABS_json
local protocol = _G.QLABS_protocol
local bridge_mod = _G.QLABS_bridge
local snapshot = _G.QLABS_snapshot
local tagging = _G.QLABS_tagging

local out = {}
local function w(fmt, ...)
  out[#out + 1] = select("#", ...) > 0 and string.format(fmt, ...) or fmt
end

w("================================================================")
w("QLabs REAPER MCP bridge status")
w("================================================================")
w("bridge version    : %s", protocol.BRIDGE_VERSION)
w("ipc protocol      : %s", protocol.PROTOCOL_VERSION)
w("bridge schema     : %s", protocol.BRIDGE_SCHEMA_VERSION)
w("script directory  : %s", SCRIPT_DIR)
w("install directory : %s%s", INSTALL_DIR,
  INSTALL_DIR == SCRIPT_DIR and "" or "   (this script was run from elsewhere)")
w("resource path     : %s", tostring(reaper.GetResourcePath()))
w("REAPER version    : %s", tostring(reaper.GetAppVersion()))

local verr = protocol.check_reaper_version(reaper.GetAppVersion())
w("version supported : %s", verr and ("NO -- " .. verr.message) or "yes")

--------------------------------------------------------------------------------
-- Configuration
--------------------------------------------------------------------------------

local cfg_path = util.join(INSTALL_DIR, bridge_mod.CONFIG_FILE)
local cfg_raw = util.fs.read(cfg_path)
w("")
w("---- configuration ----")
w("config file       : %s", cfg_path)
w("config present    : %s", cfg_raw and "yes" or "NO (a token will be minted on first run)")

local cfg = bridge_mod.default_config()
if cfg_raw then
  local decoded = json.decode(cfg_raw)
  if type(decoded) == "table" then
    for k, v in pairs(decoded) do
      if v ~= json.null then cfg[k] = v end
    end
  else
    w("config parse      : FAILED -- the file is not valid JSON")
  end
end

local token = type(cfg.instance_token) == "string" and cfg.instance_token or ""
w("instance token    : %s", token == "" and "(unset)"
  or (token:sub(1, 4) .. string.rep("*", math.max(0, #token - 8)) .. token:sub(-4)))

local ipc_dir = (type(cfg.ipc_dir) == "string" and cfg.ipc_dir ~= "" and cfg.ipc_dir)
  or util.join(INSTALL_DIR, "ipc")
w("ipc directory     : %s", ipc_dir)
w("poll interval     : %s ms", tostring(cfg.poll_interval_ms))
w("heartbeat interval: %s ms", tostring(cfg.heartbeat_interval_ms))

--------------------------------------------------------------------------------
-- Heartbeat and lock
--------------------------------------------------------------------------------

w("")
w("---- liveness ----")

local hb_raw = util.fs.read(util.join(ipc_dir, bridge_mod.HEARTBEAT_FILE))
if hb_raw == nil then
  w("heartbeat         : ABSENT -- the bridge has never run against this ipc dir")
else
  local hb = json.decode(hb_raw)
  if type(hb) ~= "table" then
    w("heartbeat         : UNREADABLE (invalid JSON)")
  else
    local age = util.now() - (tonumber(hb.timestamp) or 0)
    w("heartbeat status  : %s", tostring(hb.status))
    w("heartbeat age     : %d s (stale after %d s)", age, protocol.LIMITS.LOCK_STALE_SECONDS)
    w("heartbeat verdict : %s",
      (hb.status == "online" and age <= protocol.LIMITS.LOCK_STALE_SECONDS)
      and "ONLINE" or "OFFLINE / STALE")
    w("heartbeat bridge  : %s", tostring(hb.bridge_version))
    w("heartbeat project : %s", tostring(hb.project_uuid))
    w("requests ok/fail  : %s / %s", tostring(hb.requests_processed), tostring(hb.requests_failed))
  end
end

local lock_raw = util.fs.read(util.join(ipc_dir, bridge_mod.LOCK_FILE))
if lock_raw == nil then
  w("lock file         : absent (no instance holds the lock)")
else
  local lock = json.decode(lock_raw)
  if type(lock) ~= "table" then
    w("lock file         : UNREADABLE (invalid JSON) -- delete it to recover")
  else
    local age = util.now() - (tonumber(lock.heartbeat_at) or 0)
    w("lock holder       : %s", tostring(lock.pid_token))
    w("lock heartbeat    : %d s ago (%s)", age,
      age > protocol.LIMITS.LOCK_STALE_SECONDS and "STALE, takeover permitted" or "live")
  end
end

--------------------------------------------------------------------------------
-- IPC directory contents
--------------------------------------------------------------------------------

w("")
w("---- ipc directory ----")
for _, sub in ipairs({ "commands", "processing", "results", "failed", "logs" }) do
  local dir = util.join(ipc_dir, sub)
  local entries = util.fs.listdir(dir)
  w("%-11s      : %d file(s)", sub, #entries)
end

--------------------------------------------------------------------------------
-- Project and source resolution
--------------------------------------------------------------------------------

w("")
w("---- active project ----")
local proj, path = snapshot.active_project()
if proj == nil then
  w("project           : NONE")
else
  w("project name      : %s", snapshot.project_name(proj, path))
  w("project path      : %s", (path ~= "" and path) or "(unsaved)")
  w("state change count: %d", reaper.GetProjectStateChangeCount(proj))
  local uuid = tagging.get_proj_state(proj, tagging.PROJ_KEYS.PROJECT_UUID)
  w("project uuid      : %s", uuid or "(not yet minted)")
  w("selected items    : %d", reaper.CountSelectedMediaItems(proj))
  w("midi editor open  : %s",
    (reaper.MIDIEditor_GetActive and reaper.MIDIEditor_GetActive() ~= nil) and "yes" or "no")
  w("play state        : %s", tostring(reaper.GetPlayStateEx(proj)))

  local staged = tagging.get_staged(proj)
  w("staged candidates : %d", #staged)
  for i = 1, #staged do
    local r = staged[i]
    if type(r) == "table" then
      w("  - %s  (%s, %s)", tostring(r.transaction_id), tostring(r.status),
        tostring(r.created_at))
    end
  end
  local last = tagging.get_last_transaction(proj)
  w("last transaction  : %s", last and
    string.format("%s [%s] %q", tostring(last.transaction_id), tostring(last.kind),
      tostring(last.undo_label)) or "(none)")
  w("top undo entry    : %s", tostring(reaper.Undo_CanUndo2(proj)))

  w("")
  w("---- source resolution ----")
  local src, serr = snapshot.resolve_source(proj, "auto")
  if not src then
    w("source            : %s -- %s", serr.code, serr.message)
  else
    w("source resolved by: %s", src.resolved_by)
    local snap, berr = snapshot.build(proj, { reaper_version = reaper.GetAppVersion() })
    if not snap then
      w("snapshot          : %s -- %s", berr.code, berr.message)
    else
      w("item guid         : %s", tostring(snap.item_guid))
      w("take guid         : %s", tostring(snap.take_guid))
      w("item bounds (QN)  : %.6f .. %.6f", snap.item_position_qn, snap.item_end_qn)
      w("notes in scope    : %d of %d in the take", snap.note_count, snap.source_note_count)
      w("midi hash         : %s", snap.midi_hash)
      w("tempo map hash    : %s", snap.tempo_map_hash)
      w("snapshot hash     : %s", snap.snapshot_hash)
      for _, warn in ipairs(snap.warnings or {}) do
        w("warning           : %s -- %s", warn.code, warn.message)
      end
    end
  end
end

w("")
w("---- allowlisted commands ----")
for _, c in ipairs(protocol.COMMAND_NAMES) do w("  %s", c) end
w("================================================================")

reaper.ShowConsoleMsg(table.concat(out, "\n") .. "\n")
