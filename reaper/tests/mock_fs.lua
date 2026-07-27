--[[
  mock_fs.lua -- deterministic in-memory filesystem implementing the `util.fs`
  backend interface, for host-free testing of the atomic IPC state machine.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT
--]]

local M = {}

local FS = {}
FS.__index = FS

--- Creates an empty in-memory filesystem.
--- `now_fn` supplies the clock used for mtimes (defaults to os.time).
function M.new(now_fn)
  return setmetatable({
    files = {},        -- normalised path -> { data = string, mtime = number }
    dirs = { [""] = true },
    now_fn = now_fn or os.time,
    ops = { write = 0, rename = 0, remove = 0, read = 0 },
  }, FS)
end

local function norm(p)
  p = tostring(p or ""):gsub("\\", "/")
  p = p:gsub("//+", "/")
  p = p:gsub("/$", "")
  return p
end

M.norm = norm

local function parent(p)
  return (norm(p):match("^(.*)/[^/]*$")) or ""
end

local function base(p)
  return (norm(p):match("([^/]*)$"))
end

--- Recursively creates a directory.
function FS:mkdirp(path)
  path = norm(path)
  local acc = ""
  for part in path:gmatch("[^/]+") do
    acc = (acc == "" and (path:sub(1, 1) == "/" and "/" .. part or part)) or (acc .. "/" .. part)
    self.dirs[norm(acc)] = true
  end
  self.dirs[path] = true
  return true
end

--- True when the path names an existing file.
function FS:exists(path)
  return self.files[norm(path)] ~= nil
end

--- True when the path names an existing directory.
function FS:isdir(path)
  return self.dirs[norm(path)] == true
end

function FS:read(path)
  path = norm(path)
  self.ops.read = self.ops.read + 1
  local f = self.files[path]
  if not f then return nil, "no such file: " .. path end
  return f.data
end

function FS:write(path, data)
  path = norm(path)
  local dir = parent(path)
  if dir ~= "" and not self.dirs[dir] then
    return false, "no such directory: " .. dir
  end
  self.ops.write = self.ops.write + 1
  self.files[path] = { data = tostring(data), mtime = self.now_fn() }
  return true
end

function FS:append(path, data)
  path = norm(path)
  local existing = self.files[path]
  return self:write(path, (existing and existing.data or "") .. tostring(data))
end

function FS:rename(from, to)
  from, to = norm(from), norm(to)
  local f = self.files[from]
  if not f then return false, "no such file: " .. from end
  local dir = parent(to)
  if dir ~= "" and not self.dirs[dir] then return false, "no such directory: " .. dir end
  self.ops.rename = self.ops.rename + 1
  self.files[to] = { data = f.data, mtime = f.mtime }
  self.files[from] = nil
  return true
end

function FS:remove(path)
  path = norm(path)
  if not self.files[path] then return false end
  self.ops.remove = self.ops.remove + 1
  self.files[path] = nil
  return true
end

function FS:listdir(dir)
  dir = norm(dir)
  local out = {}
  for path in pairs(self.files) do
    if parent(path) == dir then out[#out + 1] = base(path) end
  end
  table.sort(out)
  return out
end

function FS:size(path)
  local f = self.files[norm(path)]
  if not f then return nil end
  return #f.data
end

function FS:mtime(path)
  local f = self.files[norm(path)]
  if not f then return nil end
  return f.mtime
end

--- Test helper: forces a file's mtime, used to age files for GC tests.
function FS:set_mtime(path, t)
  local f = self.files[norm(path)]
  if f then f.mtime = t end
end

--- Test helper: total file count.
function FS:count()
  local n = 0
  for _ in pairs(self.files) do n = n + 1 end
  return n
end

return M
