--[[ test_tagging.lua -- ownership tags, project ext state, UUID minting. ]]

local H = require("harness")
local tagging = require("tagging")
local mock_reaper = require("mock_reaper")

local function host()
  local state = mock_reaper.new({})
  state:install()
  return state, state:project()
end

return {
  { "only allowlisted tag keys can be written", function()
    local state, proj = host()
    local tr = state:add_track(proj, "Chords")
    H.ok(tagging.set_tag("track", tr, "QLABS_OWNER", tagging.OWNER_VALUE))
    H.falsy(tagging.set_tag("track", tr, "P_NAME", "hacked"))
    H.falsy(tagging.set_tag("track", tr, "SOMETHING_ELSE", "x"))
    H.falsy(tagging.set_tag("track", tr, "QLABS_ARBITRARY", "x"))
    H.eq(tr.name, "Chords", "a rejected tag must not touch the track")
  end },

  { "rejects oversized tag values", function()
    local state, proj = host()
    local tr = state:add_track(proj, "T")
    H.falsy(tagging.set_tag("track", tr, "QLABS_ROLE", string.rep("x", 1000)))
    H.eq(tagging.get_tag("track", tr, "QLABS_ROLE"), nil)
  end },

  { "tags round-trip on tracks, items and takes", function()
    local state, proj = host()
    local tr = state:add_track(proj, "T")
    local item, take = state:add_midi_item(tr, 0, 4)
    for _, spec in ipairs({ { "track", tr }, { "item", item }, { "take", take } }) do
      tagging.set_tag(spec[1], spec[2], "QLABS_OWNER", tagging.OWNER_VALUE)
      tagging.set_tag(spec[1], spec[2], "QLABS_TRANSACTION_ID", "tx-1")
      H.eq(tagging.get_tag(spec[1], spec[2], "QLABS_TRANSACTION_ID"), "tx-1")
      H.ok(tagging.is_owned(spec[1], spec[2]))
      H.ok(tagging.matches_transaction(spec[1], spec[2], "tx-1"))
      H.falsy(tagging.matches_transaction(spec[1], spec[2], "tx-2"))
    end
  end },

  { "ownership requires both the owner tag and the transaction id", function()
    local state, proj = host()
    local tr = state:add_track(proj, "Impostor")
    tagging.set_tag("track", tr, "QLABS_TRANSACTION_ID", "tx-1")
    H.falsy(tagging.matches_transaction("track", tr, "tx-1"),
      "a transaction id without the owner tag is not ownership")
    tagging.set_tag("track", tr, "QLABS_OWNER", "someone-else")
    H.falsy(tagging.matches_transaction("track", tr, "tx-1"))
    tagging.set_tag("track", tr, "QLABS_OWNER", tagging.OWNER_VALUE)
    H.ok(tagging.matches_transaction("track", tr, "tx-1"))
    H.falsy(tagging.matches_transaction("track", tr, ""), "empty transaction ids match nothing")
    H.falsy(tagging.matches_transaction("track", tr, nil))
  end },

  { "collect_owned finds only matching objects and never uses names", function()
    local state, proj = host()
    local mine = state:add_track(proj, "QLabs Candidate 01")
    local theirs = state:add_track(proj, "QLabs Candidate 01") -- identical name, no tags
    local other_tx = state:add_track(proj, "QLabs Candidate 02")
    local mine_item = state:add_midi_item(mine, 0, 4)
    state:add_midi_item(theirs, 0, 4)

    for _, spec in ipairs({ { "track", mine }, { "item", mine_item } }) do
      tagging.set_tag(spec[1], spec[2], "QLABS_OWNER", tagging.OWNER_VALUE)
      tagging.set_tag(spec[1], spec[2], "QLABS_TRANSACTION_ID", "tx-1")
    end
    tagging.set_tag("track", other_tx, "QLABS_OWNER", tagging.OWNER_VALUE)
    tagging.set_tag("track", other_tx, "QLABS_TRANSACTION_ID", "tx-2")

    local owned = tagging.collect_owned(proj, "tx-1")
    H.eq(#owned.tracks, 1)
    H.eq(owned.tracks[1].obj, mine)
    H.eq(#owned.items, 1)
    H.eq(owned.items[1].obj, mine_item)

    local none = tagging.collect_owned(proj, "tx-does-not-exist")
    H.eq(#none.tracks, 0)
    H.eq(#none.items, 0)
  end },

  { "count_foreign_items ignores same-transaction items", function()
    local state, proj = host()
    local tr = state:add_track(proj, "T")
    local a = state:add_midi_item(tr, 0, 4)
    local b = state:add_midi_item(tr, 4, 8)
    tagging.set_tag("item", a, "QLABS_OWNER", tagging.OWNER_VALUE)
    tagging.set_tag("item", a, "QLABS_TRANSACTION_ID", "tx-1")
    H.eq(tagging.count_foreign_items(tr, "tx-1"), 1)
    tagging.set_tag("item", b, "QLABS_OWNER", tagging.OWNER_VALUE)
    tagging.set_tag("item", b, "QLABS_TRANSACTION_ID", "tx-1")
    H.eq(tagging.count_foreign_items(tr, "tx-1"), 0)
  end },

  { "project UUID is minted once and then stable", function()
    local _, proj = host()
    H.eq(tagging.get_proj_state(proj, tagging.PROJ_KEYS.PROJECT_UUID), nil)
    local id, minted = tagging.ensure_project_uuid(proj)
    H.ok(minted)
    H.ok(#id > 0)
    local id2, minted2 = tagging.ensure_project_uuid(proj)
    H.eq(id2, id)
    H.falsy(minted2)
    H.eq(tagging.get_proj_state(proj, tagging.PROJ_KEYS.SCHEMA_VERSION), "1")
  end },

  { "new_uuid produces distinct uuid-shaped ids", function()
    local seen = {}
    for _ = 1, 50 do
      local id = tagging.new_uuid()
      H.falsy(seen[id], "uuid collision")
      seen[id] = true
    end
  end },

  { "staged transaction records are bounded and searchable", function()
    local _, proj = host()
    for i = 1, tagging.MAX_STAGED_RECORDS + 5 do
      tagging.record_staged(proj, { transaction_id = "tx-" .. i, status = "preview" })
    end
    local list = tagging.get_staged(proj)
    H.eq(#list, tagging.MAX_STAGED_RECORDS)
    H.eq(tagging.find_staged(proj, "tx-1"), nil, "oldest records are trimmed")
    local found = tagging.find_staged(proj, "tx-37")
    H.ok(found)
    H.eq(found.status, "preview")
    H.ok(tagging.remove_staged(proj, "tx-37"))
    H.eq(tagging.find_staged(proj, "tx-37"), nil)
  end },

  { "record_staged replaces rather than duplicates", function()
    local _, proj = host()
    tagging.record_staged(proj, { transaction_id = "tx-a", status = "preview" })
    tagging.record_staged(proj, { transaction_id = "tx-a", status = "committed" })
    H.eq(#tagging.get_staged(proj), 1)
    H.eq(tagging.find_staged(proj, "tx-a").status, "committed")
  end },

  { "corrupt project ext state degrades gracefully", function()
    local _, proj = host()
    tagging.set_proj_state(proj, tagging.PROJ_KEYS.STAGED, "{ this is not json")
    H.eq(#tagging.get_staged(proj), 0)
    tagging.set_proj_state(proj, tagging.PROJ_KEYS.LAST_TRANSACTION, "[1,2,3]")
    H.eq(tagging.get_last_transaction(proj), nil)
  end },

  { "last transaction record round-trips and clears", function()
    local _, proj = host()
    tagging.set_last_transaction(proj, { transaction_id = "tx-9",
      undo_label = "QLabs MCP: Stage candidate tx-9", kind = "stage" })
    local rec = tagging.get_last_transaction(proj)
    H.eq(rec.transaction_id, "tx-9")
    tagging.set_last_transaction(proj, nil)
    H.eq(tagging.get_last_transaction(proj), nil)
  end },
}
