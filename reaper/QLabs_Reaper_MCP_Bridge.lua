--[[
  QLabs_Reaper_MCP_Bridge.lua
  Persistent ReaScript bridge for the QLabs REAPER Music Intelligence MCP.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT

  Install to:
    <REAPER Resource Path>/Scripts/QLabs-Reaper-MCP/QLabs_Reaper_MCP_Bridge.lua

  Run it from the Actions list (or bind it to a toolbar button). It runs a
  bounded `reaper.defer` polling loop and communicates with the MCP server
  exclusively through atomic file IPC inside its own `ipc/` directory. It opens
  no network port, evaluates no caller-supplied code, and runs no action ids.

  Everything of substance lives in lib/; this file only wires the engine to
  REAPER's defer/atexit lifecycle and the toolbar toggle state.
--]]

--------------------------------------------------------------------------------
-- Resolve the installation directory and put lib/ on the module path.
-- NEVER hardcode an install path: portable REAPER installs are supported.
--
-- The installation directory is the one that actually holds lib/. Normally that
-- is the script's own directory. When the script has been copied elsewhere —
-- a scratch copy, a second location, a file dragged out of the folder — we fall
-- back to the installed location under REAPER's resource path.
--
-- This deliberately resolves ONE directory rather than only fixing the module
-- path, because this same directory also decides which config.json and which
-- ipc/ the bridge uses. Loading lib/ from the installation while writing IPC
-- state beside a stray copy would split the installation in half and leave the
-- MCP server talking to a directory nothing answers on.
--------------------------------------------------------------------------------

local SCRIPT_PATH = debug.getinfo(1, "S").source
if SCRIPT_PATH:sub(1, 1) == "@" then SCRIPT_PATH = SCRIPT_PATH:sub(2) end
local SCRIPT_DIR = SCRIPT_PATH:match("^(.*)[/\\][^/\\]*$") or "."

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

package.path = table.concat({
  INSTALL_DIR .. "/lib/?.lua",
  package.path,
}, ";")

local ok_load, load_err = pcall(function()
  _G.QLABS_util = require("util")
  _G.QLABS_json = require("json")
  _G.QLABS_protocol = require("protocol")
  _G.QLABS_bridge = require("bridge")
end)

if not ok_load then
  reaper.ShowMessageBox(
    "QLabs REAPER MCP bridge could not load its libraries.\n\n"
    .. "Looked for lib/ under:\n  " .. table.concat(TRIED, "\n  ") .. "\n\n"
    .. tostring(load_err),
    "QLabs REAPER MCP", 0)
  return
end

local util = _G.QLABS_util
local protocol = _G.QLABS_protocol
local bridge_mod = _G.QLABS_bridge

--------------------------------------------------------------------------------
-- Toolbar toggle bookkeeping
--------------------------------------------------------------------------------

local toolbar = { section = nil, command = nil }

local function set_toggle(value)
  if toolbar.command == nil or toolbar.command == 0 then return end
  if reaper.SetToggleCommandState then
    reaper.SetToggleCommandState(toolbar.section, toolbar.command, value)
  end
  if reaper.RefreshToolbar2 then
    reaper.RefreshToolbar2(toolbar.section, toolbar.command)
  end
end

if reaper.get_action_context then
  local _, _, section, command = reaper.get_action_context()
  toolbar.section, toolbar.command = section, command
end

--------------------------------------------------------------------------------
-- Start
--------------------------------------------------------------------------------

local bridge = bridge_mod.new({ script_dir = INSTALL_DIR })

local started, reason, holder = bridge:start()

if not started then
  local msg = "QLabs REAPER MCP bridge is already running.\n\n" .. tostring(reason)
  if holder and holder.ipc_dir then
    msg = msg .. "\n\nIPC directory:\n" .. tostring(holder.ipc_dir)
  end
  msg = msg .. "\n\nIf you are certain no other instance is live, delete:\n"
    .. bridge:lock_path()
  reaper.ShowMessageBox(msg, "QLabs REAPER MCP", 0)
  return
end

set_toggle(1)

reaper.ShowConsoleMsg(string.format(
  "QLabs REAPER MCP bridge %s online\n  ipc dir : %s\n  config  : %s\n  protocol: %s\n",
  protocol.BRIDGE_VERSION, bridge.ipc_dir,
  tostring(bridge.config_path or util.join(INSTALL_DIR, "config.json")),
  protocol.PROTOCOL_VERSION))

if bridge.config_created then
  reaper.ShowConsoleMsg(
    "  NOTE: a new installation token was generated. Point the MCP server at\n"
    .. "        the config file above so both sides share the same token.\n")
end

--------------------------------------------------------------------------------
-- Defer loop
--------------------------------------------------------------------------------

local shutting_down = false

local function on_error(e)
  return debug.traceback(tostring(e), 2)
end

local function loop()
  if shutting_down then return end
  -- Every tick is guarded: an unexpected Lua error must never escape into
  -- REAPER's defer machinery, and must never stop the loop.
  local ok, alive = xpcall(bridge.tick, on_error, bridge)
  if not ok then
    pcall(function() bridge.log:error("tick failed: %s", tostring(alive)) end)
    alive = true
  end
  if alive == false then
    shutting_down = true
    return
  end
  reaper.defer(loop)
end

--------------------------------------------------------------------------------
-- Cleanup
--------------------------------------------------------------------------------

local function cleanup()
  shutting_down = true
  pcall(function() bridge:stop("atexit") end)
  pcall(function() set_toggle(0) end)
end

reaper.atexit(cleanup)
reaper.defer(loop)
