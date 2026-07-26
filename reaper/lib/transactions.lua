--[[
  transactions.lua -- edit-plan validation and execution, undo blocks,
  commit / discard / undo, and UI-refresh bookkeeping.

  Copyright (c) 2026 QLabs
  SPDX-License-Identifier: MIT

  Invariants this module guarantees:
    * The COMPLETE plan is validated before `Undo_BeginBlock2` is called.
    * `PreventUIRefresh(1)` and `PreventUIRefresh(-1)` are always balanced, even
      when a mutation raises, because the decrement runs outside the xpcall.
    * A failed transaction is closed with its own owned label and then undone
      only when (a) the project state actually changed and (b) the top undo
      entry is that exact owned label. An unrelated action is never undone.
    * Object selection for discard/commit is by ownership tag only, never name.
--]]

local util = require("util")
local json = require("json")
local protocol = require("protocol")
local tagging = require("tagging")
local snapshot = require("snapshot")

local M = {}

--- Every owned undo label must start with this prefix.
M.UNDO_PREFIX = "QLabs MCP: "

--- Numeric tolerance for QN bounds comparisons.
M.QN_EPSILON = 1e-6

--------------------------------------------------------------------------------
-- Internal error helper
--------------------------------------------------------------------------------

local function fail(code, message, details)
  error(protocol.err(code, message, details), 0)
end

local function plan_error(message, details)
  return protocol.err(protocol.ERR.INVALID_EDIT_PLAN, message, details)
end

--------------------------------------------------------------------------------
-- Plan validation
--------------------------------------------------------------------------------

--- The complete allowlist of edit operations, mirroring
--- `music_domain::plan::EditOperation`.
M.OPERATIONS = {
  create_folder_track = true,
  create_track = true,
  create_midi_item = true,
  insert_notes = true,
  set_track_mute = true,
  create_region = true,
  create_midi_send = true,
}

--- Sorted array of operation names.
M.OPERATION_NAMES = (function()
  local ks = {}
  for k in pairs(M.OPERATIONS) do ks[#ks + 1] = k end
  table.sort(ks)
  return ks
end)()

--- Precondition discriminators, mirroring `music_domain::plan::Precondition`.
M.PRECONDITIONS = {
  project_uuid = true,
  state_change_count = true,
  item_guid_exists = true,
  take_guid_exists = true,
  midi_hash = true,
  tempo_map_hash = true,
  item_bounds = true,
}

--- Kinds an `expected_outputs` entry may declare.
M.OUTPUT_KINDS = {
  track = true, folder_track = true, midi_item = true, region = true, send = true,
}

local function check_id(v, field, maxlen)
  if type(v) ~= "string" or v == "" then
    return nil, plan_error(field .. " must be a non-empty string")
  end
  if #v > (maxlen or protocol.LIMITS.MAX_ID_LEN) then
    return nil, plan_error(field .. " exceeds " .. (maxlen or protocol.LIMITS.MAX_ID_LEN) .. " bytes")
  end
  return v
end

local function check_text(v, field, maxlen)
  if type(v) ~= "string" then
    return nil, plan_error(field .. " must be a string")
  end
  if #v > (maxlen or protocol.LIMITS.MAX_STRING_LEN) then
    return nil, plan_error(field .. " exceeds " .. (maxlen or protocol.LIMITS.MAX_STRING_LEN) .. " bytes")
  end
  if v:find("[%z\1-\8\11\12\14-\31]") then
    return nil, plan_error(field .. " contains control characters")
  end
  return v
end

local function check_qn(v, field)
  if type(v) ~= "number" or not util.is_finite(v) then
    return nil, plan_error(field .. " must be a finite number")
  end
  if v < -100000 or v > 1000000 then
    return nil, plan_error(field .. " is outside the supported quarter-note range")
  end
  return v + 0.0
end

local function check_int(v, field, lo, hi)
  if type(v) ~= "number" then return nil, plan_error(field .. " must be a number") end
  local i = math.tointeger(v)
  if i == nil then return nil, plan_error(field .. " must be an integer") end
  if i < lo or i > hi then
    return nil, plan_error(string.format("%s must be in %d..%d", field, lo, hi))
  end
  return i
end

local function check_tags(raw, field)
  local out = {}
  if raw == nil or raw == json.null then return out end
  if not util.is_array(raw) then
    return nil, plan_error(field .. " must be an array of [key, value] pairs")
  end
  if #raw > protocol.LIMITS.MAX_TAGS_PER_OBJECT then
    return nil, plan_error(field .. " carries more than " ..
      protocol.LIMITS.MAX_TAGS_PER_OBJECT .. " tags")
  end
  for i = 1, #raw do
    local pair = raw[i]
    local k, v
    if util.is_array(pair) and #pair == 2 then
      k, v = pair[1], pair[2]
    elseif type(pair) == "table" then
      k, v = pair.key, pair.value
    end
    if type(k) ~= "string" or not tagging.is_tag_key(k) then
      return nil, plan_error(string.format("%s[%d] key %q is not an allowlisted QLABS_* tag",
        field, i, tostring(k)))
    end
    if type(v) ~= "string" then
      return nil, plan_error(string.format("%s[%d] value must be a string", field, i))
    end
    local _, terr = check_text(v, string.format("%s[%d] value", field, i))
    if terr then return nil, terr end
    out[#out + 1] = { key = k, value = v }
  end
  return out
end

--- Validates an entire edit plan without touching REAPER.
--- Returns a normalised plan model, or `nil, err` with code INVALID_EDIT_PLAN
--- (or KNOWLEDGE_INVALID for a bad knowledge version).
function M.validate_plan(plan)
  local L = protocol.LIMITS
  if type(plan) ~= "table" or util.is_array(plan) then
    return nil, plan_error("edit plan must be a JSON object")
  end

  local model = { symbols = {}, ops = {}, counts = {
    tracks = 0, items = 0, regions = 0, sends = 0, notes = 0,
  } }

  for _, f in ipairs({ "plan_id", "candidate_id", "transaction_id", "base_snapshot_id", "project_uuid" }) do
    local v, e = check_id(plan[f], f)
    if not v then return nil, e end
    model[f] = v
  end
  if not util.is_safe_id(model.transaction_id, L.MAX_ID_LEN) then
    return nil, plan_error("transaction_id must match " .. util.ID_PATTERN)
  end

  local bsh, e = check_id(plan.base_snapshot_hash, "base_snapshot_hash", 128)
  if not bsh then return nil, e end
  model.base_snapshot_hash = bsh

  local kv = plan.knowledge_version
  if type(kv) ~= "string" or kv == "" or #kv > 64 or not kv:match("^[A-Za-z0-9._+%-]+$") then
    return nil, protocol.err(protocol.ERR.KNOWLEDGE_INVALID,
      "knowledge_version must be a non-empty [A-Za-z0-9._+-] string of at most 64 bytes",
      { knowledge_version = tostring(kv) })
  end
  model.knowledge_version = kv

  local label, lerr = check_text(plan.undo_label, "undo_label", 200)
  if not label then return nil, lerr end
  if label:sub(1, #M.UNDO_PREFIX) ~= M.UNDO_PREFIX then
    return nil, plan_error(string.format("undo_label must begin with %q", M.UNDO_PREFIX))
  end
  model.undo_label = label

  -- Operations ---------------------------------------------------------------
  local ops = plan.operations
  if not util.is_array(ops) then
    return nil, plan_error("operations must be an array")
  end
  if #ops == 0 then
    return nil, plan_error("operations must contain at least one operation")
  end
  if #ops > L.MAX_OPERATIONS then
    return nil, plan_error(string.format("plan has %d operations; limit is %d", #ops, L.MAX_OPERATIONS))
  end

  local function declare(temp_id, kind, index)
    if not util.is_safe_id(temp_id, 64) then
      return plan_error(string.format("operations[%d].temp_id must match %s", index, util.ID_PATTERN))
    end
    if model.symbols[temp_id] then
      return plan_error(string.format("operations[%d] redeclares temp_id %q", index, temp_id))
    end
    model.symbols[temp_id] = { kind = kind, index = index }
    return nil
  end

  local function resolve(temp_id, want_kind, index, field)
    if type(temp_id) ~= "string" then
      return nil, plan_error(string.format("operations[%d].%s must be a string temp_id", index, field))
    end
    local sym = model.symbols[temp_id]
    if not sym then
      return nil, plan_error(string.format("operations[%d].%s references undeclared temp_id %q "
        .. "(forward references are not permitted)", index, field, temp_id))
    end
    if want_kind == "track" then
      if sym.kind ~= "track" and sym.kind ~= "folder_track" then
        return nil, plan_error(string.format("operations[%d].%s must reference a track", index, field))
      end
    elseif sym.kind ~= want_kind then
      return nil, plan_error(string.format("operations[%d].%s must reference a %s", index, field, want_kind))
    end
    return sym
  end

  for i = 1, #ops do
    local op = ops[i]
    if type(op) ~= "table" or util.is_array(op) then
      return nil, plan_error(string.format("operations[%d] must be an object", i))
    end
    local kind = op.op
    if type(kind) ~= "string" or not M.OPERATIONS[kind] then
      return nil, plan_error(string.format("operations[%d].op %q is not an allowlisted operation",
        i, tostring(kind)), { allowed = M.OPERATION_NAMES })
    end
    local norm = { op = kind, index = i }

    if kind == "create_folder_track" or kind == "create_track" then
      model.counts.tracks = model.counts.tracks + 1
      if model.counts.tracks > L.MAX_TRACKS_PER_STAGE then
        return nil, plan_error(string.format("plan creates more than %d tracks", L.MAX_TRACKS_PER_STAGE))
      end
      local derr = declare(op.temp_id, kind == "create_folder_track" and "folder_track" or "track", i)
      if derr then return nil, derr end
      norm.temp_id = op.temp_id
      local nm, nerr = check_text(op.name, string.format("operations[%d].name", i), 200)
      if not nm then return nil, nerr end
      norm.name = nm
      if kind == "create_track" then
        local parent = op.parent
        if parent ~= nil and parent ~= json.null then
          local sym, perr = resolve(parent, "folder_track", i, "parent")
          if not sym then return nil, perr end
          norm.parent = parent
        end
      end
      local tags, terr = check_tags(op.tags, string.format("operations[%d].tags", i))
      if not tags then return nil, terr end
      norm.tags = tags

    elseif kind == "create_midi_item" then
      model.counts.items = model.counts.items + 1
      if model.counts.items > L.MAX_ITEMS_PER_STAGE then
        return nil, plan_error(string.format("plan creates more than %d items", L.MAX_ITEMS_PER_STAGE))
      end
      local derr = declare(op.temp_id, "midi_item", i)
      if derr then return nil, derr end
      norm.temp_id = op.temp_id
      local sym, perr = resolve(op.track, "track", i, "track")
      if not sym then return nil, perr end
      norm.track = op.track
      local s, serr = check_qn(op.start_qn, string.format("operations[%d].start_qn", i))
      if s == nil then return nil, serr end
      local e2, eerr = check_qn(op.end_qn, string.format("operations[%d].end_qn", i))
      if e2 == nil then return nil, eerr end
      if e2 <= s + M.QN_EPSILON then
        return nil, plan_error(string.format("operations[%d]: end_qn must be greater than start_qn", i))
      end
      norm.start_qn, norm.end_qn = s, e2
      local muted = op.muted
      if muted ~= nil and muted ~= json.null and type(muted) ~= "boolean" then
        return nil, plan_error(string.format("operations[%d].muted must be a boolean", i))
      end
      norm.muted = muted == true
      local tags, terr = check_tags(op.tags, string.format("operations[%d].tags", i))
      if not tags then return nil, terr end
      norm.tags = tags
      model.symbols[op.temp_id].start_qn = s
      model.symbols[op.temp_id].end_qn = e2
      model.symbols[op.temp_id].note_count = 0

    elseif kind == "insert_notes" then
      local sym, perr = resolve(op.item, "midi_item", i, "item")
      if not sym then return nil, perr end
      norm.item = op.item
      local notes = op.notes
      if not util.is_array(notes) then
        return nil, plan_error(string.format("operations[%d].notes must be an array", i))
      end
      local out = {}
      for k = 1, #notes do
        local n = notes[k]
        local where = string.format("operations[%d].notes[%d]", i, k)
        if type(n) ~= "table" or util.is_array(n) then
          return nil, plan_error(where .. " must be an object")
        end
        local ns, nse = check_qn(n.start_qn, where .. ".start_qn")
        if ns == nil then return nil, nse end
        local ne, nee = check_qn(n.end_qn, where .. ".end_qn")
        if ne == nil then return nil, nee end
        if ne <= ns then
          return nil, plan_error(where .. ": end_qn must be greater than start_qn")
        end
        if ns < sym.start_qn - M.QN_EPSILON or ne > sym.end_qn + M.QN_EPSILON then
          return nil, plan_error(string.format(
            "%s spans %.6f..%.6f which is outside item bounds %.6f..%.6f",
            where, ns, ne, sym.start_qn, sym.end_qn))
        end
        local pitch, pe = check_int(n.pitch, where .. ".pitch", 0, 127)
        if pitch == nil then return nil, pe end
        local vel, ve = check_int(n.velocity, where .. ".velocity", 1, 127)
        if vel == nil then return nil, ve end
        local chan, ce = check_int(n.channel, where .. ".channel", 0, 15)
        if chan == nil then return nil, ce end
        local nmuted = n.muted
        if nmuted ~= nil and nmuted ~= json.null and type(nmuted) ~= "boolean" then
          return nil, plan_error(where .. ".muted must be a boolean")
        end
        local spelling = n.spelling
        if spelling ~= nil and spelling ~= json.null then
          local sp, spe = check_text(spelling, where .. ".spelling", 16)
          if not sp then return nil, spe end
          spelling = sp
        else
          spelling = nil
        end
        out[#out + 1] = {
          start_qn = ns, end_qn = ne, pitch = pitch, velocity = vel,
          channel = chan, muted = nmuted == true, spelling = spelling,
        }
      end
      sym.note_count = (sym.note_count or 0) + #out
      if sym.note_count > L.MAX_NOTES_PER_ITEM then
        return nil, plan_error(string.format("item %q would receive %d notes; limit is %d",
          op.item, sym.note_count, L.MAX_NOTES_PER_ITEM))
      end
      model.counts.notes = model.counts.notes + #out
      if model.counts.notes > L.MAX_GENERATED_NOTES then
        return nil, plan_error(string.format("plan inserts %d notes; limit is %d",
          model.counts.notes, L.MAX_GENERATED_NOTES))
      end
      -- Deterministic note ordering, independent of how the server emitted them.
      table.sort(out, function(a, b)
        if a.start_qn ~= b.start_qn then return a.start_qn < b.start_qn end
        if a.pitch ~= b.pitch then return a.pitch < b.pitch end
        if a.channel ~= b.channel then return a.channel < b.channel end
        if a.end_qn ~= b.end_qn then return a.end_qn < b.end_qn end
        return a.velocity < b.velocity
      end)
      norm.notes = out

    elseif kind == "set_track_mute" then
      local sym, perr = resolve(op.track, "track", i, "track")
      if not sym then return nil, perr end
      norm.track = op.track
      if type(op.muted) ~= "boolean" then
        return nil, plan_error(string.format("operations[%d].muted must be a boolean", i))
      end
      norm.muted = op.muted

    elseif kind == "create_region" then
      model.counts.regions = model.counts.regions + 1
      if model.counts.regions > L.MAX_REGIONS_PER_STAGE then
        return nil, plan_error(string.format("plan creates more than %d regions", L.MAX_REGIONS_PER_STAGE))
      end
      local nm, nerr = check_text(op.name, string.format("operations[%d].name", i), 200)
      if not nm then return nil, nerr end
      norm.name = nm
      local s, serr = check_qn(op.start_qn, string.format("operations[%d].start_qn", i))
      if s == nil then return nil, serr end
      local e2, eerr = check_qn(op.end_qn, string.format("operations[%d].end_qn", i))
      if e2 == nil then return nil, eerr end
      if e2 <= s then
        return nil, plan_error(string.format("operations[%d]: end_qn must be greater than start_qn", i))
      end
      norm.start_qn, norm.end_qn = s, e2
      if op.temp_id ~= nil and op.temp_id ~= json.null then
        local derr = declare(op.temp_id, "region", i)
        if derr then return nil, derr end
        norm.temp_id = op.temp_id
      end

    elseif kind == "create_midi_send" then
      model.counts.sends = model.counts.sends + 1
      if model.counts.sends > L.MAX_SENDS_PER_STAGE then
        return nil, plan_error(string.format("plan creates more than %d sends", L.MAX_SENDS_PER_STAGE))
      end
      local sym, perr = resolve(op.from_track, "track", i, "from_track")
      if not sym then return nil, perr end
      norm.from_track = op.from_track
      local g = op.to_track_guid
      if type(g) ~= "string" or snapshot.norm_guid(g) == nil then
        return nil, plan_error(string.format("operations[%d].to_track_guid must be a REAPER GUID", i))
      end
      norm.to_track_guid = g
      if op.temp_id ~= nil and op.temp_id ~= json.null then
        local derr = declare(op.temp_id, "send", i)
        if derr then return nil, derr end
        norm.temp_id = op.temp_id
      end
    end

    model.ops[#model.ops + 1] = norm
  end

  -- Preconditions ------------------------------------------------------------
  local pres = plan.preconditions
  if pres == nil or pres == json.null then pres = {} end
  if not util.is_array(pres) then
    return nil, plan_error("preconditions must be an array")
  end
  if #pres > L.MAX_PRECONDITIONS then
    return nil, plan_error("plan carries more than " .. L.MAX_PRECONDITIONS .. " preconditions")
  end
  model.preconditions = {}
  for i = 1, #pres do
    local p = pres[i]
    if type(p) ~= "table" or util.is_array(p) then
      return nil, plan_error(string.format("preconditions[%d] must be an object", i))
    end
    local t = p.type
    if type(t) ~= "string" or not M.PRECONDITIONS[t] then
      return nil, plan_error(string.format("preconditions[%d].type %q is not recognised", i, tostring(t)))
    end
    local entry = { type = t }
    if t == "item_bounds" then
      local s, serr = check_qn(p.start_qn, string.format("preconditions[%d].start_qn", i))
      if s == nil then return nil, serr end
      local e2, eerr = check_qn(p.end_qn, string.format("preconditions[%d].end_qn", i))
      if e2 == nil then return nil, eerr end
      entry.start_qn, entry.end_qn = s, e2
    elseif t == "state_change_count" then
      local v = p.value
      if math.tointeger(v) == nil then
        return nil, plan_error(string.format("preconditions[%d].value must be an integer", i))
      end
      entry.value = math.tointeger(v)
    else
      local v, verr = check_id(p.value, string.format("preconditions[%d].value", i), 256)
      if not v then return nil, verr end
      entry.value = v
    end
    model.preconditions[#model.preconditions + 1] = entry
  end

  -- Expected outputs ---------------------------------------------------------
  local outs = plan.expected_outputs
  if outs == nil or outs == json.null then outs = {} end
  if not util.is_array(outs) then
    return nil, plan_error("expected_outputs must be an array")
  end
  if #outs > L.MAX_EXPECTED_OUTPUTS then
    return nil, plan_error("plan carries more than " .. L.MAX_EXPECTED_OUTPUTS .. " expected outputs")
  end
  model.expected_outputs = {}
  for i = 1, #outs do
    local o = outs[i]
    if type(o) ~= "table" or util.is_array(o) then
      return nil, plan_error(string.format("expected_outputs[%d] must be an object", i))
    end
    local tid = o.temp_id
    if type(tid) ~= "string" or model.symbols[tid] == nil then
      return nil, plan_error(string.format("expected_outputs[%d].temp_id %q was never declared",
        i, tostring(tid)))
    end
    local kind = o.kind
    if type(kind) ~= "string" or not M.OUTPUT_KINDS[kind] then
      return nil, plan_error(string.format("expected_outputs[%d].kind %q is not recognised",
        i, tostring(kind)))
    end
    if kind ~= model.symbols[tid].kind then
      return nil, plan_error(string.format("expected_outputs[%d] declares kind %q but temp_id %q is a %s",
        i, kind, tid, model.symbols[tid].kind))
    end
    local nc = o.note_count
    if nc ~= nil and nc ~= json.null then
      local n, nerr = check_int(nc, string.format("expected_outputs[%d].note_count", i),
        0, L.MAX_NOTES_PER_ITEM)
      if n == nil then return nil, nerr end
      nc = n
    else
      nc = nil
    end
    model.expected_outputs[#model.expected_outputs + 1] = { temp_id = tid, kind = kind, note_count = nc }
  end

  return model
end

--------------------------------------------------------------------------------
-- Precondition checking against the live project
--------------------------------------------------------------------------------

--- Evaluates a validated plan's preconditions against live project state.
--- Returns a table of resolved pointers, or `nil, err`.
function M.check_plan_preconditions(proj, model, ctx)
  ctx = ctx or {}
  local resolved = {}

  for i = 1, #model.preconditions do
    local p = model.preconditions[i]
    if p.type == "project_uuid" then
      local have = tagging.get_proj_state(proj, tagging.PROJ_KEYS.PROJECT_UUID)
      if have ~= p.value then
        return nil, protocol.err(protocol.ERR.PROJECT_CHANGED,
          "plan precondition project_uuid does not match the active project",
          { expected = p.value, actual = have or json.null })
      end
    elseif p.type == "state_change_count" then
      local have = math.tointeger(reaper.GetProjectStateChangeCount(proj)) or 0
      if have ~= p.value then
        return nil, protocol.err(protocol.ERR.PROJECT_CHANGED,
          string.format("plan precondition state_change_count %d does not match live %d", p.value, have),
          { expected = p.value, actual = have })
      end
    elseif p.type == "item_guid_exists" then
      local item = snapshot.find_item_by_guid(proj, p.value)
      if not item then
        return nil, protocol.err(protocol.ERR.SOURCE_ITEM_MISSING,
          string.format("plan precondition item %s is missing", p.value), { item_guid = p.value })
      end
      resolved.item = resolved.item or item
    elseif p.type == "take_guid_exists" then
      local take, item = snapshot.find_take_by_guid(proj, p.value)
      if not take then
        return nil, protocol.err(protocol.ERR.SOURCE_TAKE_MISSING,
          string.format("plan precondition take %s is missing", p.value), { take_guid = p.value })
      end
      resolved.take = take
      resolved.item = resolved.item or item
    end
  end

  -- Hash preconditions need the resolved pointers, so evaluate them second.
  local st = snapshot.live_state(proj, resolved.item, resolved.take)
  for i = 1, #model.preconditions do
    local p = model.preconditions[i]
    if p.type == "midi_hash" then
      if st.midi_hash == nil then
        return nil, protocol.err(protocol.ERR.SOURCE_TAKE_MISSING,
          "plan declares a midi_hash precondition but no take_guid_exists precondition resolved a take")
      end
      if st.midi_hash ~= p.value then
        return nil, protocol.err(protocol.ERR.MIDI_CHANGED,
          "source MIDI changed since the plan was generated",
          { expected = p.value, actual = st.midi_hash })
      end
    elseif p.type == "tempo_map_hash" then
      if st.tempo_map_hash ~= p.value then
        return nil, protocol.err(protocol.ERR.TEMPO_MAP_CHANGED,
          "tempo map changed since the plan was generated",
          { expected = p.value, actual = st.tempo_map_hash })
      end
    elseif p.type == "item_bounds" then
      if st.item_position_qn == nil then
        return nil, protocol.err(protocol.ERR.SOURCE_ITEM_MISSING,
          "plan declares an item_bounds precondition but no item was resolved")
      end
      if math.abs(st.item_position_qn - p.start_qn) > 1e-4
        or math.abs(st.item_end_qn - p.end_qn) > 1e-4 then
        return nil, protocol.err(protocol.ERR.STALE_SNAPSHOT,
          string.format("source item bounds are %.6f..%.6f, plan expected %.6f..%.6f",
            st.item_position_qn, st.item_end_qn, p.start_qn, p.end_qn),
          { expected_start_qn = p.start_qn, expected_end_qn = p.end_qn,
            actual_start_qn = st.item_position_qn, actual_end_qn = st.item_end_qn })
      end
    end
  end

  -- Snapshot hash: the plan's base_snapshot_hash must still describe the source.
  if ctx.verify_snapshot ~= false and resolved.take ~= nil then
    local rebuilt = snapshot.build(proj, {
      source_mode = ctx.source_mode or "auto",
      note_scope = ctx.note_scope,
      extraction_mode = ctx.extraction_mode,
      extraction_channel = ctx.extraction_channel,
      reaper_version = ctx.reaper_version,
    })
    if rebuilt and rebuilt.snapshot_hash ~= model.base_snapshot_hash then
      return nil, protocol.err(protocol.ERR.STALE_SNAPSHOT,
        "the project no longer matches the snapshot this plan was generated from; "
        .. "re-inspect and re-generate rather than writing stale material",
        { expected = model.base_snapshot_hash, actual = rebuilt.snapshot_hash })
    end
  end

  -- Plan-level project uuid must always agree with the live project.
  local live_uuid = tagging.get_proj_state(proj, tagging.PROJ_KEYS.PROJECT_UUID)
  if live_uuid ~= nil and model.project_uuid ~= live_uuid then
    return nil, protocol.err(protocol.ERR.PROJECT_CHANGED,
      "plan.project_uuid does not match the active project",
      { expected = model.project_uuid, actual = live_uuid })
  end

  return resolved
end

--------------------------------------------------------------------------------
-- Guarded transaction runner
--------------------------------------------------------------------------------

--- Snapshot of the process-wide UI-refresh depth this module has requested.
--- Exposed for tests: it must be zero before and after every transaction.
M.ui_refresh_depth = 0

local function handler(e)
  if type(e) == "table" and e.code then
    e.traceback = debug.traceback("", 2)
    return e
  end
  return protocol.err(protocol.ERR.INTERNAL_BRIDGE_ERROR, tostring(e),
    { traceback = debug.traceback("", 2) })
end

--- Runs `body` inside a project undo block with UI refresh suppressed.
---
--- Returns `value` on success, or `nil, err, rolled_back` on failure. UI-refresh
--- balance and undo-block closure are guaranteed on every path because they run
--- outside the xpcall.
function M.run_guarded(proj, undo_label, body)
  if type(undo_label) ~= "string" or undo_label:sub(1, #M.UNDO_PREFIX) ~= M.UNDO_PREFIX then
    return nil, protocol.err(protocol.ERR.INVALID_EDIT_PLAN,
      string.format("refusing to open an undo block with unowned label %q", tostring(undo_label))), false
  end

  local scc_before = math.tointeger(reaper.GetProjectStateChangeCount(proj)) or 0

  reaper.Undo_BeginBlock2(proj)
  local block_open = true
  reaper.PreventUIRefresh(1)
  M.ui_refresh_depth = M.ui_refresh_depth + 1

  local ok, res = xpcall(body, handler)

  -- Always restore UI refresh, whatever happened.
  while M.ui_refresh_depth > 0 do
    reaper.PreventUIRefresh(-1)
    M.ui_refresh_depth = M.ui_refresh_depth - 1
  end
  while M.ui_refresh_depth < 0 do
    reaper.PreventUIRefresh(1)
    M.ui_refresh_depth = M.ui_refresh_depth + 1
  end

  if block_open then
    reaper.Undo_EndBlock2(proj, undo_label, -1)
    block_open = false
  end

  local rolled_back = false
  if not ok then
    local scc_after = math.tointeger(reaper.GetProjectStateChangeCount(proj)) or 0
    if scc_after > scc_before then
      local top = reaper.Undo_CanUndo2(proj)
      -- Undo ONLY when the entry at the top of the stack is this exact owned
      -- transaction. Never undo an unrelated action.
      if type(top) == "string" and top == undo_label then
        reaper.Undo_DoUndo2(proj)
        rolled_back = true
      end
    end
  end

  if reaper.UpdateArrange then reaper.UpdateArrange() end

  if not ok then return nil, res, rolled_back end
  return res
end

--------------------------------------------------------------------------------
-- Operation execution
--------------------------------------------------------------------------------

local function insert_track(proj, index)
  if reaper.InsertTrackInProject then
    reaper.InsertTrackInProject(proj, index, 0)
  elseif reaper.InsertTrackAtIndex then
    reaper.InsertTrackAtIndex(index, false)
    if reaper.TrackList_AdjustWindows then reaper.TrackList_AdjustWindows(false) end
  else
    fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "host exposes no track-insertion API")
  end
  local tr = reaper.GetTrack(proj, index)
  if tr == nil then
    fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "track insertion did not produce a track")
  end
  return tr
end

local function base_tags(model, kind)
  return {
    { key = "QLABS_OWNER", value = tagging.OWNER_VALUE },
    { key = "QLABS_TRANSACTION_ID", value = model.transaction_id },
    { key = "QLABS_CANDIDATE_ID", value = model.candidate_id },
    { key = "QLABS_PLAN_ID", value = model.plan_id },
    { key = "QLABS_STATUS", value = tagging.STATUS_PREVIEW },
    { key = "QLABS_SOURCE_SNAPSHOT", value = model.base_snapshot_id },
    { key = "QLABS_KNOWLEDGE_VERSION", value = model.knowledge_version },
    { key = "QLABS_OBJECT_KIND", value = kind },
    { key = "QLABS_UNDO_LABEL", value = model.undo_label },
  }
end

local function tag_object(objkind, obj, model, kind, temp_id, extra)
  local list = base_tags(model, kind)
  if temp_id then list[#list + 1] = { key = "QLABS_TEMP_ID", value = temp_id } end
  list[#list + 1] = { key = "QLABS_CREATED_AT", value = util.iso8601(util.now()) }
  for i = 1, #(extra or {}) do list[#list + 1] = extra[i] end
  local ok, err = tagging.apply_tags(objkind, obj, list)
  if not ok then fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "failed to write ownership tags: " .. tostring(err)) end
end

--- Applies a validated plan's operations. Raises structured errors on failure.
--- Returns the created-object table keyed by temp id.
function M.apply_operations(proj, model)
  local created = {}
  local folders = {}
  local track_order = {}

  for i = 1, #model.ops do
    local op = model.ops[i]

    if op.op == "create_folder_track" or op.op == "create_track" then
      local index = reaper.CountTracks(proj)
      local tr = insert_track(proj, index)
      reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", op.name, true)
      local kind = op.op == "create_folder_track" and "folder_track" or "track"
      tag_object("track", tr, model, kind, op.temp_id, op.tags)
      created[op.temp_id] = { kind = kind, track = tr, name = op.name,
        guid = snapshot.track_guid(tr), parent = op.parent }
      track_order[#track_order + 1] = op.temp_id
      if kind == "folder_track" then
        folders[op.temp_id] = { children = {} }
      elseif op.parent and folders[op.parent] then
        table.insert(folders[op.parent].children, op.temp_id)
      end

    elseif op.op == "create_midi_item" then
      local parent = created[op.track]
      if not parent or not parent.track then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "internal: unresolved track for " .. tostring(op.temp_id))
      end
      if not reaper.CreateNewMIDIItemInProj then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "host lacks CreateNewMIDIItemInProj")
      end
      -- qnInOptional = true: start/end are project quarter notes.
      local item = reaper.CreateNewMIDIItemInProj(parent.track, op.start_qn, op.end_qn, true)
      if item == nil then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "CreateNewMIDIItemInProj returned nil")
      end
      if op.muted then reaper.SetMediaItemInfo_Value(item, "B_MUTE", 1) end
      local extra = op.muted and { { key = "QLABS_PREVIEW_MUTED", value = "1" } } or nil
      local merged = {}
      for k = 1, #op.tags do merged[#merged + 1] = op.tags[k] end
      for k = 1, #(extra or {}) do merged[#merged + 1] = extra[k] end
      tag_object("item", item, model, "midi_item", op.temp_id, merged)
      local take = reaper.GetActiveTake(item)
      if take == nil then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "created MIDI item has no active take")
      end
      tag_object("take", take, model, "midi_take", op.temp_id, op.tags)
      created[op.temp_id] = {
        kind = "midi_item", item = item, take = take, track = parent.track,
        guid = snapshot.item_guid(item), take_guid = snapshot.take_guid(take),
        start_qn = op.start_qn, end_qn = op.end_qn, note_count = 0,
      }

    elseif op.op == "insert_notes" then
      local target = created[op.item]
      if not target or not target.take then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "internal: unresolved item for insert_notes")
      end
      local take = target.take
      local before_ok, before_notes = reaper.MIDI_CountEvts(take)
      if not before_ok then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "MIDI_CountEvts failed before insertion")
      end
      for k = 1, #op.notes do
        local n = op.notes[k]
        -- Clamp to item bounds so nothing ever escapes the item.
        local s = math.max(n.start_qn, target.start_qn)
        local e = math.min(n.end_qn, target.end_qn)
        if e <= s then
          fail(protocol.ERR.INVALID_EDIT_PLAN,
            string.format("note %d collapses to zero length after clamping to item bounds", k))
        end
        local sp = reaper.MIDI_GetPPQPosFromProjQN(take, s)
        local ep = reaper.MIDI_GetPPQPosFromProjQN(take, e)
        if ep <= sp then
          fail(protocol.ERR.INVALID_EDIT_PLAN,
            string.format("note %d has a non-positive PPQ length", k))
        end
        -- noSortInOptional = true: defer sorting until the batch is complete.
        local inserted = reaper.MIDI_InsertNote(take, false, n.muted, sp, ep,
          n.channel, n.pitch, n.velocity, true)
        if inserted == false then
          fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, string.format("MIDI_InsertNote failed for note %d", k))
        end
      end
      reaper.MIDI_Sort(take)
      local after_ok, after_notes = reaper.MIDI_CountEvts(take)
      if not after_ok then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "MIDI_CountEvts failed after insertion")
      end
      local expected = (before_notes or 0) + #op.notes
      if (after_notes or 0) ~= expected then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR,
          string.format("note verification failed: take holds %d notes, expected %d",
            after_notes or -1, expected),
          { actual = after_notes, expected = expected })
      end
      target.note_count = after_notes

    elseif op.op == "set_track_mute" then
      local t = created[op.track]
      if not t or not t.track then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "internal: unresolved track for set_track_mute")
      end
      reaper.SetMediaTrackInfo_Value(t.track, "B_MUTE", op.muted and 1 or 0)
      if op.muted then
        tagging.set_tag("track", t.track, "QLABS_PREVIEW_MUTED", "1")
      end

    elseif op.op == "create_region" then
      if not reaper.AddProjectMarker2 then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "host lacks AddProjectMarker2")
      end
      local s = reaper.TimeMap2_QNToTime(proj, op.start_qn)
      local e = reaper.TimeMap2_QNToTime(proj, op.end_qn)
      local idx = reaper.AddProjectMarker2(proj, true, s, e, op.name, -1, 0)
      if idx == nil or idx < 0 then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "AddProjectMarker2 failed")
      end
      if op.temp_id then
        created[op.temp_id] = { kind = "region", marker_index = idx, name = op.name,
          start_qn = op.start_qn, end_qn = op.end_qn }
      end

    elseif op.op == "create_midi_send" then
      local from = created[op.from_track]
      if not from or not from.track then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "internal: unresolved from_track for create_midi_send")
      end
      local dest = snapshot.find_track_by_guid(proj, op.to_track_guid)
      if dest == nil then
        fail(protocol.ERR.INVALID_EDIT_PLAN,
          string.format("send destination track %s does not exist", op.to_track_guid),
          { to_track_guid = op.to_track_guid })
      end
      if not reaper.CreateTrackSend then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "host lacks CreateTrackSend")
      end
      local sidx = reaper.CreateTrackSend(from.track, dest)
      if sidx == nil or sidx < 0 then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR, "CreateTrackSend failed")
      end
      if reaper.SetTrackSendInfo_Value then
        -- MIDI-only send: audio source channel off, all MIDI channels through.
        reaper.SetTrackSendInfo_Value(from.track, 0, sidx, "I_SRCCHAN", -1)
        reaper.SetTrackSendInfo_Value(from.track, 0, sidx, "I_MIDIFLAGS", 0)
      end
      if op.temp_id then
        created[op.temp_id] = { kind = "send", send_index = sidx,
          from = from.guid, to_track_guid = op.to_track_guid }
      end
    end
  end

  -- Folder depth fixup: one level of nesting, matching the staging layout in
  -- brief section 23. Done after creation so child order is known.
  for temp_id, folder in pairs(folders) do
    local f = created[temp_id]
    if f and f.track then
      reaper.SetMediaTrackInfo_Value(f.track, "I_FOLDERDEPTH", 1)
      local kids = folder.children
      for k = 1, #kids do
        local child = created[kids[k]]
        if child and child.track then
          reaper.SetMediaTrackInfo_Value(child.track, "I_FOLDERDEPTH", k == #kids and -1 or 0)
        end
      end
      if #kids == 0 then
        -- An empty folder must not swallow the rest of the track list.
        reaper.SetMediaTrackInfo_Value(f.track, "I_FOLDERDEPTH", 0)
      end
    end
  end

  return created
end

--- Verifies the plan's expected outputs against what was actually created.
function M.verify_outputs(model, created)
  for i = 1, #model.expected_outputs do
    local exp = model.expected_outputs[i]
    local got = created[exp.temp_id]
    if not got then
      fail(protocol.ERR.INTERNAL_BRIDGE_ERROR,
        string.format("expected output %q was not created", exp.temp_id))
    end
    if got.kind ~= exp.kind then
      fail(protocol.ERR.INTERNAL_BRIDGE_ERROR,
        string.format("expected output %q is a %s, expected %s", exp.temp_id, got.kind, exp.kind))
    end
    if exp.note_count ~= nil then
      local actual = got.note_count or 0
      if actual ~= exp.note_count then
        fail(protocol.ERR.INTERNAL_BRIDGE_ERROR,
          string.format("expected output %q holds %d notes, expected %d",
            exp.temp_id, actual, exp.note_count),
          { temp_id = exp.temp_id, actual = actual, expected = exp.note_count })
      end
    end
  end
end

--------------------------------------------------------------------------------
-- stage_candidate
--------------------------------------------------------------------------------

--- Validates and executes a stage plan. Returns a result table or `nil, err`.
function M.stage(proj, plan, ctx)
  ctx = ctx or {}
  local model, verr = M.validate_plan(plan)
  if not model then return nil, verr end

  local _, perr = M.check_plan_preconditions(proj, model, ctx)
  if perr then return nil, perr end

  local created
  local value, ferr, rolled = M.run_guarded(proj, model.undo_label, function()
    created = M.apply_operations(proj, model)
    M.verify_outputs(model, created)
    return created
  end)

  if not value then
    ferr = ferr or protocol.err(protocol.ERR.INTERNAL_BRIDGE_ERROR, "stage failed with no error detail")
    ferr.details = ferr.details or {}
    if type(ferr.details) == "table" then
      ferr.details.rolled_back = rolled
      ferr.details.transaction_id = model.transaction_id
    end
    return nil, ferr
  end

  local tracks, items, regions, sends = {}, {}, {}, {}
  for temp_id, obj in pairs(value) do
    if obj.kind == "track" or obj.kind == "folder_track" then
      tracks[#tracks + 1] = { temp_id = temp_id, kind = obj.kind, guid = obj.guid or json.null,
        name = obj.name }
    elseif obj.kind == "midi_item" then
      items[#items + 1] = { temp_id = temp_id, guid = obj.guid or json.null,
        take_guid = obj.take_guid or json.null, note_count = obj.note_count or 0,
        start_qn = obj.start_qn, end_qn = obj.end_qn }
    elseif obj.kind == "region" then
      regions[#regions + 1] = { temp_id = temp_id, marker_index = obj.marker_index, name = obj.name }
    elseif obj.kind == "send" then
      sends[#sends + 1] = { temp_id = temp_id, send_index = obj.send_index,
        to_track_guid = obj.to_track_guid }
    end
  end
  table.sort(tracks, function(a, b) return a.temp_id < b.temp_id end)
  table.sort(items, function(a, b) return a.temp_id < b.temp_id end)
  table.sort(regions, function(a, b) return a.temp_id < b.temp_id end)
  table.sort(sends, function(a, b) return a.temp_id < b.temp_id end)

  local record = {
    transaction_id = model.transaction_id,
    candidate_id = model.candidate_id,
    plan_id = model.plan_id,
    undo_label = model.undo_label,
    base_snapshot_id = model.base_snapshot_id,
    base_snapshot_hash = model.base_snapshot_hash,
    knowledge_version = model.knowledge_version,
    status = tagging.STATUS_PREVIEW,
    kind = "stage",
    created_at = util.iso8601(util.now()),
  }
  tagging.record_staged(proj, record)
  tagging.set_last_transaction(proj, record)

  return {
    transaction_id = model.transaction_id,
    candidate_id = model.candidate_id,
    plan_id = model.plan_id,
    undo_label = model.undo_label,
    status = tagging.STATUS_PREVIEW,
    tracks = json.array(tracks),
    items = json.array(items),
    regions = json.array(regions),
    sends = json.array(sends),
    note_count = model.counts.notes,
    project_state_change_count = math.tointeger(reaper.GetProjectStateChangeCount(proj)) or 0,
    warnings = json.array({}),
  }
end

--------------------------------------------------------------------------------
-- commit_candidate
--------------------------------------------------------------------------------

local function short(tx)
  return tostring(tx):sub(1, 8)
end

--- Flips ownership status from preview to committed on tag-matching objects
--- only. Objects that are not tagged with this transaction id, or that are not
--- in preview status, are left completely untouched.
function M.commit(proj, tx, opts)
  opts = opts or {}
  if not util.is_safe_id(tx, protocol.LIMITS.MAX_ID_LEN) then
    return nil, protocol.err(protocol.ERR.MALFORMED_REQUEST, "transaction_id is not a valid identifier")
  end
  local record = tagging.find_staged(proj, tx)
  local owned = tagging.collect_owned(proj, tx)
  if #owned.tracks == 0 and #owned.items == 0 and #owned.takes == 0 then
    return nil, protocol.err(protocol.ERR.TRANSACTION_NOT_FOUND,
      string.format("no objects in the active project carry transaction id %s", tx),
      { transaction_id = tx })
  end

  local label = opts.undo_label or (M.UNDO_PREFIX .. "Commit candidate " .. short(tx))
  local changed = { tracks = 0, items = 0, takes = 0 }
  local now = util.iso8601(util.now())

  local value, ferr, rolled = M.run_guarded(proj, label, function()
    local function flip(kind, entry)
      local obj = entry.obj
      if tagging.get_tag(kind, obj, "QLABS_STATUS") ~= tagging.STATUS_PREVIEW then
        return false
      end
      tagging.set_tag(kind, obj, "QLABS_STATUS", tagging.STATUS_COMMITTED)
      tagging.set_tag(kind, obj, "QLABS_COMMITTED_AT", now)
      if tagging.get_tag(kind, obj, "QLABS_PREVIEW_MUTED") == "1" then
        if kind == "track" then
          reaper.SetMediaTrackInfo_Value(obj, "B_MUTE", 0)
        elseif kind == "item" then
          reaper.SetMediaItemInfo_Value(obj, "B_MUTE", 0)
        end
        tagging.clear_tag(kind, obj, "QLABS_PREVIEW_MUTED")
      end
      return true
    end
    for i = 1, #owned.tracks do
      if flip("track", owned.tracks[i]) then changed.tracks = changed.tracks + 1 end
    end
    for i = 1, #owned.items do
      if flip("item", owned.items[i]) then changed.items = changed.items + 1 end
    end
    for i = 1, #owned.takes do
      if flip("take", owned.takes[i]) then changed.takes = changed.takes + 1 end
    end
    return true
  end)

  if not value then
    if type(ferr) == "table" and type(ferr.details) == "table" then
      ferr.details.rolled_back = rolled
    end
    return nil, ferr
  end

  if record then
    record.status = tagging.STATUS_COMMITTED
    record.committed_at = now
    tagging.record_staged(proj, record)
  end
  tagging.set_last_committed(proj, tx)
  tagging.set_last_transaction(proj, {
    transaction_id = tx, undo_label = label, kind = "commit",
    candidate_id = record and record.candidate_id or json.null, created_at = now,
  })

  return {
    transaction_id = tx,
    status = tagging.STATUS_COMMITTED,
    undo_label = label,
    committed_tracks = changed.tracks,
    committed_items = changed.items,
    committed_takes = changed.takes,
    inspected_tracks = #owned.tracks,
    inspected_items = #owned.items,
    inspected_takes = #owned.takes,
    project_state_change_count = math.tointeger(reaper.GetProjectStateChangeCount(proj)) or 0,
  }
end

--------------------------------------------------------------------------------
-- discard_candidate
--------------------------------------------------------------------------------

--- Deletes only objects carrying matching ownership tags. Track NAMES are never
--- consulted. A generated track that still holds foreign items is kept and a
--- warning is reported instead.
function M.discard(proj, tx, opts)
  opts = opts or {}
  if not util.is_safe_id(tx, protocol.LIMITS.MAX_ID_LEN) then
    return nil, protocol.err(protocol.ERR.MALFORMED_REQUEST, "transaction_id is not a valid identifier")
  end
  local owned = tagging.collect_owned(proj, tx)
  if #owned.tracks == 0 and #owned.items == 0 then
    return nil, protocol.err(protocol.ERR.TRANSACTION_NOT_FOUND,
      string.format("no objects in the active project carry transaction id %s", tx),
      { transaction_id = tx })
  end

  local label = opts.undo_label or (M.UNDO_PREFIX .. "Discard candidate " .. short(tx))
  local removed = { items = 0, tracks = 0 }
  local warnings = {}

  local value, ferr, rolled = M.run_guarded(proj, label, function()
    -- Items first: deleting a track invalidates its item pointers.
    for i = 1, #owned.items do
      local entry = owned.items[i]
      local item, track = entry.obj, entry.track
      local valid = true
      if reaper.ValidatePtr2 then
        valid = reaper.ValidatePtr2(proj, item, "MediaItem*") and true or false
      end
      if valid and track ~= nil then
        if not tagging.matches_transaction("item", item, tx) then
          fail(protocol.ERR.INTERNAL_BRIDGE_ERROR,
            "refusing to delete an item whose ownership tag no longer matches")
        end
        if reaper.DeleteTrackMediaItem(track, item) then
          removed.items = removed.items + 1
        end
      end
    end
    -- Then tracks, only when nothing foreign survives on them.
    for i = 1, #owned.tracks do
      local tr = owned.tracks[i].obj
      local valid = true
      if reaper.ValidatePtr2 then
        valid = reaper.ValidatePtr2(proj, tr, "MediaTrack*") and true or false
      end
      if valid then
        if not tagging.matches_transaction("track", tr, tx) then
          fail(protocol.ERR.INTERNAL_BRIDGE_ERROR,
            "refusing to delete a track whose ownership tag no longer matches")
        end
        local foreign = tagging.count_foreign_items(tr, tx)
        if foreign > 0 then
          local _, nm = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
          warnings[#warnings + 1] = {
            code = "track_retained",
            message = string.format(
              "track %q still holds %d item(s) that are not part of this transaction and was kept",
              tostring(nm), foreign),
          }
        else
          reaper.DeleteTrack(tr)
          removed.tracks = removed.tracks + 1
        end
      end
    end
    return true
  end)

  if not value then
    if type(ferr) == "table" and type(ferr.details) == "table" then
      ferr.details.rolled_back = rolled
    end
    return nil, ferr
  end

  tagging.remove_staged(proj, tx)
  tagging.set_last_transaction(proj, {
    transaction_id = tx, undo_label = label, kind = "discard",
    created_at = util.iso8601(util.now()),
  })

  return {
    transaction_id = tx,
    undo_label = label,
    removed_items = removed.items,
    removed_tracks = removed.tracks,
    retained_tracks = #owned.tracks - removed.tracks,
    warnings = json.array(warnings),
    project_state_change_count = math.tointeger(reaper.GetProjectStateChangeCount(proj)) or 0,
  }
end

--------------------------------------------------------------------------------
-- undo_last_generation
--------------------------------------------------------------------------------

--- Undoes the most recent owned transaction, and only that. Returns
--- UNDO_NOT_OWNED whenever the top of REAPER's undo stack is not this bridge's
--- own last transaction, or when the caller names a different transaction.
function M.undo_last(proj, opts)
  opts = opts or {}
  local record = tagging.get_last_transaction(proj)
  if record == nil or type(record.undo_label) ~= "string" then
    return nil, protocol.err(protocol.ERR.UNDO_NOT_OWNED,
      "this project holds no record of a QLabs transaction to undo")
  end

  if opts.transaction_id ~= nil and opts.transaction_id ~= record.transaction_id then
    return nil, protocol.err(protocol.ERR.UNDO_NOT_OWNED,
      string.format("the last QLabs transaction is %s, not %s",
        tostring(record.transaction_id), tostring(opts.transaction_id)),
      { requested = opts.transaction_id, last_transaction_id = record.transaction_id })
  end

  local top = reaper.Undo_CanUndo2(proj)
  if type(top) ~= "string" or top == "" then
    return nil, protocol.err(protocol.ERR.UNDO_NOT_OWNED, "REAPER's undo stack is empty")
  end
  if top ~= record.undo_label then
    return nil, protocol.err(protocol.ERR.UNDO_NOT_OWNED,
      string.format("the top undo entry is %q, which was not created by this MCP", top),
      { top_undo_entry = top, expected = record.undo_label })
  end
  if top:sub(1, #M.UNDO_PREFIX) ~= M.UNDO_PREFIX then
    return nil, protocol.err(protocol.ERR.UNDO_NOT_OWNED,
      string.format("undo entry %q does not carry the owned label prefix", top))
  end

  local ok = reaper.Undo_DoUndo2(proj)
  if ok == false or ok == 0 then
    return nil, protocol.err(protocol.ERR.INTERNAL_BRIDGE_ERROR, "Undo_DoUndo2 refused the undo")
  end
  if reaper.UpdateArrange then reaper.UpdateArrange() end

  if record.kind == "stage" then
    tagging.remove_staged(proj, record.transaction_id)
  end
  tagging.set_last_transaction(proj, nil)

  return {
    undone = true,
    transaction_id = record.transaction_id,
    undo_label = record.undo_label,
    kind = record.kind or "unknown",
    project_state_change_count = math.tointeger(reaper.GetProjectStateChangeCount(proj)) or 0,
  }
end

return M
