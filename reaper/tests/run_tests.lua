#!/usr/bin/env lua5.4
--[[
  run_tests.lua -- host-free test runner for the QLabs REAPER MCP bridge.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT

  Usage:  lua5.4 reaper/tests/run_tests.lua [pattern]

  Every module under reaper/lib is exercised against the in-memory mock REAPER
  host in mock_reaper.lua and the in-memory filesystem in mock_fs.lua. No REAPER
  installation and no external Lua package is required.
--]]

local this_dir = (debug.getinfo(1, "S").source:sub(2):match("^(.*)[/\\][^/\\]*$")) or "."
local lib_dir = this_dir .. "/../lib"
package.path = table.concat({
  lib_dir .. "/?.lua",
  this_dir .. "/?.lua",
  package.path,
}, ";")

local SUITES = {
  "test_json",
  "test_util",
  "test_protocol",
  "test_tagging",
  "test_snapshot",
  "test_transactions",
  "test_bridge",
}

local pattern = arg and arg[1] or nil

local passed, failed, skipped = 0, 0, 0
local failures = {}

local function run_suite(name)
  local mod = require(name)
  local cases = mod.cases or mod
  io.write(string.format("\n== %s ==\n", name))
  for i = 1, #cases do
    local case = cases[i]
    local label = case[1] or case.name
    local fn = case[2] or case.fn
    if pattern and not (name .. " " .. label):find(pattern) then
      skipped = skipped + 1
    else
      -- Reset shared module state between cases.
      package.loaded["util"].fs = package.loaded["util"].realfs
      local ok, err = xpcall(fn, function(e)
        if type(e) == "table" then
          return string.format("%s: %s\n%s", tostring(e.code), tostring(e.message),
            debug.traceback("", 2))
        end
        return debug.traceback(tostring(e), 2)
      end)
      if ok then
        passed = passed + 1
        io.write(string.format("  PASS  %s\n", label))
      else
        failed = failed + 1
        failures[#failures + 1] = { suite = name, case = label, err = err }
        io.write(string.format("  FAIL  %s\n", label))
      end
    end
  end
end

io.write("QLabs REAPER MCP bridge -- Lua test suite\n")
io.write(string.format("Lua: %s\n", _VERSION))

for i = 1, #SUITES do
  local ok, err = pcall(run_suite, SUITES[i])
  if not ok then
    failed = failed + 1
    failures[#failures + 1] = { suite = SUITES[i], case = "<suite load>", err = tostring(err) }
    io.write(string.format("  FAIL  <suite load>: %s\n", tostring(err)))
  end
end

if #failures > 0 then
  io.write("\n---- failures ----\n")
  for i = 1, #failures do
    local f = failures[i]
    io.write(string.format("\n[%s] %s\n%s\n", f.suite, f.case, f.err))
  end
end

io.write(string.format("\n================================\n"))
io.write(string.format("passed: %d   failed: %d   skipped: %d\n", passed, failed, skipped))
io.write(string.format("================================\n"))

os.exit(failed == 0 and 0 or 1)
