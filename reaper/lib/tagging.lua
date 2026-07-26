--[[
  tagging.lua -- QLabs ownership tags on tracks/items/takes, project-level
  extended state, and persistent project UUID minting.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT

  Ownership is expressed exclusively through `P_EXT:QLABS_*` extended state.
  Nothing in this module -- and nothing that calls it -- may identify an object
  by track or item NAME. Names are cosmetic; tags are authority.
--]]

local util = require("util")
local json = require("json")
local protocol = require("protocol")

local M = {}

--------------------------------------------------------------------------------
-- Key vocabulary
--------------------------------------------------------------------------------

--- Prefix REAPER requires for user extended state on tracks/items/takes.
M.EXT_PREFIX = "P_EXT:"

--- Value written to QLABS_OWNER on every object this bridge creates.
M.OWNER_VALUE = "QLabs-Reaper-MCP"

--- The complete allowlist of tag names (without the `P_EXT:` prefix). An edit
--- plan may only carry tags whose key appears here, which is what stops a plan
--- from writing arbitrary REAPER object state through the tag channel.
M.TAG_KEYS = {
  QLABS_OWNER = true,
  QLABS_CANDIDATE_ID = true,
  QLABS_TRANSACTION_ID = true,
  QLABS_PLAN_ID = true,
  QLABS_STATUS = true,
  QLABS_SOURCE_SNAPSHOT = true,
  QLABS_KNOWLEDGE_VERSION = true,
  QLABS_ROLE = true,
  QLABS_TEMP_ID = true,
  QLABS_OBJECT_KIND = true,
  QLABS_PREVIEW_MUTED = true,
  QLABS_CREATED_AT = true,
  QLABS_COMMITTED_AT = true,
  QLABS_UNDO_LABEL = true,
}

--- Sorted array form of TAG_KEYS.
M.TAG_KEY_NAMES = (function()
  local ks = {}
  for k in pairs(M.TAG_KEYS) do ks[#ks + 1] = k end
  table.sort(ks)
  return ks
end)()

--- Status values the bridge writes to QLABS_STATUS.
M.STATUS_PREVIEW = "preview"
M.STATUS_COMMITTED = "committed"

--- True when `key` is an allowlisted bare tag name.
function M.is_tag_key(key)
  return type(key) == "string" and M.TAG_KEYS[key] == true
end

--------------------------------------------------------------------------------
-- Low-level object tag access
--------------------------------------------------------------------------------

local ACCESSORS = {
  track = { get = "GetSetMediaTrackInfo_String", set = "GetSetMediaTrackInfo_String" },
  item = { get = "GetSetMediaItemInfo_String", set = "GetSetMediaItemInfo_String" },
  take = { get = "GetSetMediaItemTakeInfo_String", set = "GetSetMediaItemTakeInfo_String" },
}

local function accessor(kind)
  local a = ACCESSORS[kind]
  if not a then error("tagging: unknown object kind " .. tostring(kind), 0) end
  local fn = reaper[a.get]
  if not fn then error("tagging: host lacks " .. a.get, 0) end
  return fn
end

--- Reads one tag. Returns nil when unset. `kind` is "track" | "item" | "take".
function M.get_tag(kind, obj, key)
  if not M.is_tag_key(key) then return nil end
  local fn = accessor(kind)
  local ok, val = fn(obj, M.EXT_PREFIX .. key, "", false)
  if not ok then return nil end
  if val == nil or val == "" then return nil end
  return val
end

--- Writes one tag. Returns true on success. Rejects non-allowlisted keys and
--- oversized values without touching the host.
function M.set_tag(kind, obj, key, value)
  if not M.is_tag_key(key) then return false, "tag key not allowlisted: " .. tostring(key) end
  value = tostring(value == nil and "" or value)
  if #value > protocol.LIMITS.MAX_STRING_LEN then
    return false, "tag value exceeds " .. protocol.LIMITS.MAX_STRING_LEN .. " bytes"
  end
  local fn = accessor(kind)
  local ok = fn(obj, M.EXT_PREFIX .. key, value, true)
  return ok and true or false
end

--- Clears one tag by writing an empty value.
function M.clear_tag(kind, obj, key)
  return M.set_tag(kind, obj, key, "")
end

--- Reads every allowlisted tag present on an object into a plain table.
function M.get_all_tags(kind, obj)
  local out = {}
  for i = 1, #M.TAG_KEY_NAMES do
    local k = M.TAG_KEY_NAMES[i]
    local v = M.get_tag(kind, obj, k)
    if v ~= nil then out[k] = v end
  end
  return out
end

--- Applies a list of `{key, value}` pairs. Returns true, or false plus a message.
function M.apply_tags(kind, obj, pairs_list)
  for i = 1, #pairs_list do
    local pair = pairs_list[i]
    local k, v
    if util.is_array(pair) then
      k, v = pair[1], pair[2]
    elseif type(pair) == "table" then
      k, v = pair.key, pair.value
    end
    local ok, err = M.set_tag(kind, obj, k, v)
    if not ok then return false, err end
  end
  return true
end

--- True when the object carries this bridge's owner tag.
function M.is_owned(kind, obj)
  return M.get_tag(kind, obj, "QLABS_OWNER") == M.OWNER_VALUE
end

--- True when the object is owned AND its transaction tag equals `tx`.
--- This is the ONLY predicate discard/commit may use to select objects.
function M.matches_transaction(kind, obj, tx)
  if type(tx) ~= "string" or tx == "" then return false end
  if not M.is_owned(kind, obj) then return false end
  return M.get_tag(kind, obj, "QLABS_TRANSACTION_ID") == tx
end

--------------------------------------------------------------------------------
-- Project extended state
--------------------------------------------------------------------------------

--- Project ext-state keys the bridge owns.
M.PROJ_KEYS = {
  PROJECT_UUID = "project_uuid",
  STAGED = "staged_transactions",
  LAST_COMMITTED = "last_committed_transaction",
  SCHEMA_VERSION = "bridge_schema_version",
  LAST_TRANSACTION = "last_transaction",
  KNOWLEDGE_VERSION = "knowledge_version",
}

--- Reads a project ext-state string, or nil when unset/empty.
function M.get_proj_state(proj, key)
  local ok, val = reaper.GetProjExtState(proj, protocol.EXT_SECTION, key)
  if ok == 0 or val == nil or val == "" then return nil end
  return val
end

--- Writes a project ext-state string.
function M.set_proj_state(proj, key, value)
  reaper.SetProjExtState(proj, protocol.EXT_SECTION, key, tostring(value or ""))
  return true
end

--- Reads a JSON-encoded project ext-state value; returns `fallback` on absence
--- or on any decode failure (corrupt state must never break the bridge).
function M.get_proj_json(proj, key, fallback)
  local raw = M.get_proj_state(proj, key)
  if raw == nil then return fallback end
  local v, _ = json.decode(raw)
  if v == nil then return fallback end
  return v
end

--- Writes a JSON-encoded project ext-state value.
function M.set_proj_json(proj, key, value)
  local s = json.encode(value)
  if s == nil then return false end
  return M.set_proj_state(proj, key, s)
end

--------------------------------------------------------------------------------
-- UUID minting
--------------------------------------------------------------------------------

local function normalise_guid(g)
  if type(g) ~= "string" then return nil end
  g = g:gsub("^%s*{", ""):gsub("}%s*$", ""):lower()
  if g:match("^%x%x%x%x%x%x%x%x%-%x%x%x%x%-%x%x%x%x%-%x%x%x%x%-%x%x%x%x%x%x%x%x%x%x%x%x$") then
    return g
  end
  return nil
end

local mint_counter = 0

--- Produces a lowercase UUID-shaped identifier. Uses `reaper.genGuid` when the
--- host provides it; otherwise falls back to a time+counter+math.random mix.
--- Uniqueness, not cryptographic unpredictability, is the requirement here.
function M.new_uuid()
  if reaper and reaper.genGuid then
    local g = normalise_guid(reaper.genGuid(""))
    if g then return g end
  end
  mint_counter = mint_counter + 1
  local seed = string.format("%d:%d:%d:%s", util.now(), mint_counter,
    math.floor(((reaper and reaper.time_precise and reaper.time_precise()) or os.clock()) * 1e6),
    tostring(math.random(0, 2 ^ 31 - 1)))
  local a = string.format("%016x", util.fnv1a64(seed))
  local b = string.format("%016x", util.fnv1a64(seed .. "|salt"))
  return string.format("%s-%s-4%s-%s%s-%s",
    a:sub(1, 8), a:sub(9, 12), a:sub(14, 16),
    string.sub("89ab", (util.fnv1a64(seed .. "|v") & 3) + 1, (util.fnv1a64(seed .. "|v") & 3) + 1),
    b:sub(1, 3), b:sub(4, 15))
end

--- Returns the project's persistent UUID, minting and storing one on first use.
--- Minting marks the project dirty exactly once; subsequent calls are read-only.
function M.ensure_project_uuid(proj)
  local existing = M.get_proj_state(proj, M.PROJ_KEYS.PROJECT_UUID)
  if existing ~= nil then return existing, false end
  local id = M.new_uuid()
  M.set_proj_state(proj, M.PROJ_KEYS.PROJECT_UUID, id)
  M.set_proj_state(proj, M.PROJ_KEYS.SCHEMA_VERSION, protocol.BRIDGE_SCHEMA_VERSION)
  return id, true
end

--------------------------------------------------------------------------------
-- Staged transaction bookkeeping
--------------------------------------------------------------------------------

--- Maximum staged transaction records retained in project ext state.
M.MAX_STAGED_RECORDS = 32

--- Returns the staged transaction records as an array (possibly empty).
function M.get_staged(proj)
  local v = M.get_proj_json(proj, M.PROJ_KEYS.STAGED, nil)
  if type(v) ~= "table" or not util.is_array(v) then return json.array({}) end
  return v
end

--- Finds a staged record by transaction id.
function M.find_staged(proj, tx)
  local list = M.get_staged(proj)
  for i = 1, #list do
    if type(list[i]) == "table" and list[i].transaction_id == tx then
      return list[i], i
    end
  end
  return nil
end

--- Appends (or replaces) a staged transaction record, trimming the oldest.
function M.record_staged(proj, record)
  local list = M.get_staged(proj)
  local _, idx = M.find_staged(proj, record.transaction_id)
  if idx then
    list[idx] = record
  else
    list[#list + 1] = record
  end
  while #list > M.MAX_STAGED_RECORDS do table.remove(list, 1) end
  M.set_proj_json(proj, M.PROJ_KEYS.STAGED, json.array(list))
  return true
end

--- Removes a staged transaction record.
function M.remove_staged(proj, tx)
  local list = M.get_staged(proj)
  local _, idx = M.find_staged(proj, tx)
  if idx then table.remove(list, idx) end
  M.set_proj_json(proj, M.PROJ_KEYS.STAGED, json.array(list))
  return idx ~= nil
end

--- Stores the record describing the most recent owned undo entry. This is what
--- `undo_last_generation` consults before touching REAPER's undo stack.
function M.set_last_transaction(proj, record)
  if record == nil then
    return M.set_proj_state(proj, M.PROJ_KEYS.LAST_TRANSACTION, "")
  end
  return M.set_proj_json(proj, M.PROJ_KEYS.LAST_TRANSACTION, record)
end

--- Reads the most recent owned undo record, or nil.
function M.get_last_transaction(proj)
  local v = M.get_proj_json(proj, M.PROJ_KEYS.LAST_TRANSACTION, nil)
  if type(v) ~= "table" or util.is_array(v) then return nil end
  return v
end

--- Records the last committed transaction id.
function M.set_last_committed(proj, tx)
  return M.set_proj_state(proj, M.PROJ_KEYS.LAST_COMMITTED, tx or "")
end

--------------------------------------------------------------------------------
-- Owned-object discovery
--------------------------------------------------------------------------------

--- Walks the project with safe count-and-get iteration and returns every object
--- carrying the owner tag and the given transaction id.
---
--- Returns `{ tracks = {...}, items = {...}, takes = {...} }` where each element
--- is `{ obj = <pointer>, track = <MediaTrack for items>, tags = {...} }`.
--- Indices are never retained past this call; callers mutate from the returned
--- pointer list only.
function M.collect_owned(proj, tx)
  local out = { tracks = {}, items = {}, takes = {} }
  if type(tx) ~= "string" or tx == "" then return out end

  local track_count = reaper.CountTracks(proj)
  for ti = 0, track_count - 1 do
    local tr = reaper.GetTrack(proj, ti)
    if tr then
      if M.matches_transaction("track", tr, tx) then
        out.tracks[#out.tracks + 1] = { obj = tr, tags = M.get_all_tags("track", tr) }
      end
    end
  end

  local item_count = reaper.CountMediaItems(proj)
  for ii = 0, item_count - 1 do
    local item = reaper.GetMediaItem(proj, ii)
    if item then
      if M.matches_transaction("item", item, tx) then
        out.items[#out.items + 1] = {
          obj = item,
          track = reaper.GetMediaItem_Track(item),
          tags = M.get_all_tags("item", item),
        }
      end
      local take_count = reaper.CountTakes(item)
      for tki = 0, take_count - 1 do
        local take = reaper.GetTake(item, tki)
        if take and M.matches_transaction("take", take, tx) then
          out.takes[#out.takes + 1] = { obj = take, item = item, tags = M.get_all_tags("take", take) }
        end
      end
    end
  end

  return out
end

--- Counts non-owned items remaining on a track, used to decide whether a
--- generated folder track is safe to delete during discard.
function M.count_foreign_items(track, tx)
  local n = 0
  local count = reaper.CountTrackMediaItems(track)
  for i = 0, count - 1 do
    local item = reaper.GetTrackMediaItem(track, i)
    if item and not M.matches_transaction("item", item, tx) then
      n = n + 1
    end
  end
  return n
end

return M
