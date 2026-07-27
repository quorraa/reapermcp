--[[ test_bridge.lua -- config, single-instance lock, heartbeat, the atomic IPC
     state machine, dispatch and stale-file garbage collection. ]]

local H = require("harness")
local json = require("json")
local util = require("util")
local protocol = require("protocol")
local bridge_mod = require("bridge")
local tagging = require("tagging")
local snapshot = require("snapshot")
local mock_reaper = require("mock_reaper")

local function melody_env(opts)
  opts = opts or {}
  local env = H.env(opts)
  local proj = env.host:project()
  local track = env.host:add_track(proj, "Melody")
  local item, take = env.host:add_midi_item(track, 0.0, 8.0)
  env.host:add_note(take, 0, 960, 60, 100, 0, true)
  env.host:add_note(take, 960, 1920, 62, 100, 0, true)
  env.host:set_editor_take(take)
  env.track, env.item, env.take, env.proj = track, item, take, proj
  return env
end

return {
  ------------------------------------------------------------------- config --
  { "mints and persists an installation token when config is absent", function()
    local env = H.env({ no_bridge = true })
    local cfg, path, created = bridge_mod.load_config(env.script_dir)
    H.ok(created)
    H.eq(#cfg.instance_token, 32)
    H.ok(env.fs:exists(path))
    local again = bridge_mod.load_config(env.script_dir)
    H.eq(again.instance_token, cfg.instance_token, "the token must be stable once written")
  end },

  { "ipc dir defaults under the script dir and honours the override", function()
    local env = H.env({ no_bridge = true })
    local b = bridge_mod.new({ script_dir = env.script_dir,
      config = { instance_token = "t", ipc_dir = json.null, log_level = "off" } })
    H.eq(b.ipc_dir, util.join(env.script_dir, "ipc"))
    local b2 = bridge_mod.new({ script_dir = env.script_dir,
      config = { instance_token = "t", ipc_dir = "/elsewhere/ipc", log_level = "off" } })
    H.eq(b2.ipc_dir, "/elsewhere/ipc")
  end },

  { "start creates the whole directory tree", function()
    local env = H.env({})
    H.ok(env.start_ok, tostring(env.start_reason))
    for _, sub in ipairs({ "commands", "processing", "results", "failed", "logs" }) do
      H.ok(env.fs:isdir(env.ipc_dir .. "/" .. sub), "missing " .. sub)
    end
  end },

  ---------------------------------------------------------------- heartbeat --
  { "heartbeat carries the documented fields", function()
    local env = melody_env({})
    env.bridge:write_heartbeat("online")
    local hb = json.decode(env.fs:read(env.ipc_dir .. "/heartbeat.json"))
    H.eq(hb.protocol_version, protocol.PROTOCOL_VERSION)
    H.eq(hb.bridge_version, protocol.BRIDGE_VERSION)
    H.eq(hb.reaper_version, "7.22/linux-x86_64")
    H.eq(hb.status, "online")
    H.eq(hb.pid_token, env.bridge.pid_token)
    H.ok(type(hb.timestamp) == "number")
    H.eq(hb.timestamp_iso, util.iso8601(hb.timestamp))
    H.eq(hb.ipc_dir, env.ipc_dir)
    H.eq(#hb.commands, 7)
    H.eq(hb.project_path, "/projects/mock.rpp")
  end },

  { "heartbeat reports the project uuid once one exists", function()
    local env = melody_env({})
    local uuid = tagging.ensure_project_uuid(env.proj)
    env.bridge:write_heartbeat("online")
    local hb = json.decode(env.fs:read(env.ipc_dir .. "/heartbeat.json"))
    H.eq(hb.project_uuid, uuid)
  end },

  { "heartbeat is written on the configured cadence, not every tick", function()
    local env = H.env({})
    local first = json.decode(env.fs:read(env.ipc_dir .. "/heartbeat.json"))
    env.host:advance(0.1)
    env.bridge:tick()
    local second = json.decode(env.fs:read(env.ipc_dir .. "/heartbeat.json"))
    H.eq(second.uptime_seconds, first.uptime_seconds, "no heartbeat within the interval")
    env.host:advance(2.0)
    env.bridge:tick()
    local third = json.decode(env.fs:read(env.ipc_dir .. "/heartbeat.json"))
    H.ok(third.uptime_seconds > first.uptime_seconds, "heartbeat after the interval elapsed")
  end },

  { "stop writes an offline heartbeat and drops the lock", function()
    local env = H.env({})
    env.bridge:stop("test")
    local hb = json.decode(env.fs:read(env.ipc_dir .. "/heartbeat.json"))
    H.eq(hb.status, "offline")
    H.falsy(env.fs:exists(env.ipc_dir .. "/bridge.lock"))
  end },

  ------------------------------------------------------- single-instance lock --
  { "a second instance refuses to start while the first is live", function()
    local env = H.env({})
    H.ok(env.start_ok)
    local second = bridge_mod.new({ script_dir = env.script_dir, config = env.config,
      now_fn = function() return env.host.time end })
    local ok, reason = second:start()
    H.falsy(ok)
    H.contains(reason, "another QLabs bridge instance is live")
  end },

  { "a stale lock is taken over", function()
    local env = H.env({})
    local lock_path = env.ipc_dir .. "/bridge.lock"
    local lock = json.decode(env.fs:read(lock_path))
    lock.heartbeat_at = util.now() - (protocol.LIMITS.LOCK_STALE_SECONDS + 60)
    env.fs:write(lock_path, json.encode(lock))

    local second = bridge_mod.new({ script_dir = env.script_dir, config = env.config,
      now_fn = function() return env.host.time end })
    H.ok(second:start(), "a stale lock must be taken over")
    H.ok(second:owns_lock())
    H.falsy(env.bridge:owns_lock(), "the original instance no longer owns the lock")
  end },

  { "an instance that loses the lock shuts itself down on the next heartbeat", function()
    local env = H.env({})
    local lock = json.decode(env.fs:read(env.ipc_dir .. "/bridge.lock"))
    lock.pid_token = "some-other-instance"
    env.fs:write(env.ipc_dir .. "/bridge.lock", json.encode(lock))
    env.host:advance(5.0)
    local alive = env.bridge:tick()
    H.falsy(alive)
    H.eq(env.bridge.stop_reason, "lock_lost")
  end },

  { "a corrupt lock file does not block startup", function()
    local env = H.env({ no_start = true })
    env.fs:mkdirp(env.ipc_dir)
    env.fs:write(env.ipc_dir .. "/bridge.lock", "{ not json")
    H.ok(env.bridge:start())
  end },

  -------------------------------------------------------- IPC state machine --
  { "partial .tmp files are never read", function()
    local env = H.env({})
    env.fs:write(env.ipc_dir .. "/commands/req-partial.tmp", '{"incomplete":')
    H.eq(#env.bridge:list_commands(), 0)
    H.eq(env.bridge:poll_once(), 0)
    H.ok(env.fs:exists(env.ipc_dir .. "/commands/req-partial.tmp"),
      "an unrelated temp file must be left alone")
  end },

  { "a command file is claimed atomically and the result is written", function()
    local env = H.env({})
    local req = env:request("ping")
    env:submit(req)
    H.eq(env.bridge:poll_once(), 1)
    H.falsy(env.fs:exists(env.ipc_dir .. "/commands/" .. req.request_id .. ".command.json"))
    H.falsy(env.fs:exists(env.ipc_dir .. "/processing/" .. req.request_id .. ".processing.json"))
    local res = env:result(req.request_id)
    H.ok(res)
    H.eq(res.ok, true)
    H.eq(res.protocol_version, protocol.PROTOCOL_VERSION)
    H.eq(res.request_id, req.request_id)
    H.eq(res.command, "ping")
    H.eq(res.result.pong, true)
    H.eq(res.error, json.null)
    H.eq(res.bridge_version, protocol.BRIDGE_VERSION)
    H.ok(type(res.duration_ms) == "number")
    H.falsy(env.fs:exists(env.ipc_dir .. "/results/" .. req.request_id .. ".result.tmp"))
  end },

  { "only one bridge can claim the same command file", function()
    local env = H.env({})
    local second = bridge_mod.new({ script_dir = env.script_dir, config = env.config,
      now_fn = function() return env.host.time end })
    second.pid_token = "second"
    local req = env:request("ping")
    env:submit(req)
    local names = env.bridge:list_commands()
    H.eq(#names, 1)
    H.ok(env.bridge:process_file(names[1]))
    H.falsy(second:process_file(names[1]), "the loser must not process a claimed file")
  end },

  { "an invalid token is rejected and the file is quarantined", function()
    local env = H.env({})
    local req = env:request("ping", {}, { instance_token = "wrong-token" })
    env:submit(req)
    env.bridge:poll_once()
    local res = env:result(req.request_id)
    H.eq(res.ok, false)
    H.eq(res.error.code, protocol.ERR.INVALID_INSTANCE_TOKEN)
    H.ok(env.fs:exists(env.ipc_dir .. "/failed/" .. req.request_id .. ".failed.json"))
  end },

  { "an expired request is rejected", function()
    local env = H.env({})
    local req = env:request("ping", {}, { expires_at = util.iso8601(util.now() - 600) })
    env:submit(req)
    env.bridge:poll_once()
    H.eq(env:result(req.request_id).error.code, protocol.ERR.EXPIRED_REQUEST)
  end },

  { "an oversized request is rejected", function()
    local env = H.env({})
    local req = env:request("ping", { blob = string.rep("x", protocol.LIMITS.MAX_REQUEST_BYTES) })
    env:submit(req)
    env.bridge:poll_once()
    H.eq(env:result(req.request_id).error.code, protocol.ERR.PAYLOAD_TOO_LARGE)
  end },

  { "an unknown command is rejected", function()
    local env = H.env({})
    local req = env:request("execute_lua", { code = "os.exit()" })
    env:submit(req)
    env.bridge:poll_once()
    local res = env:result(req.request_id)
    H.eq(res.error.code, protocol.ERR.UNKNOWN_COMMAND)
    H.eq(#res.error.details.allowed, 7)
  end },

  { "a duplicate request id is rejected", function()
    local env = H.env({})
    local req = env:request("ping")
    env:submit(req)
    env.bridge:poll_once()
    H.eq(env:result(req.request_id).ok, true)
    env.fs:remove(env.ipc_dir .. "/results/" .. req.request_id .. ".result.json")
    env:submit(req)
    env.bridge:poll_once()
    H.eq(env:result(req.request_id).error.code, protocol.ERR.DUPLICATE_REQUEST)
  end },

  { "invalid JSON is rejected as MALFORMED_REQUEST", function()
    local env = H.env({})
    env:submit_raw('{"protocol_version": ', "req-broken")
    env.bridge:poll_once()
    local res = env:result("req-broken")
    H.eq(res.error.code, protocol.ERR.MALFORMED_REQUEST)
    H.ok(env.fs:exists(env.ipc_dir .. "/failed/req-broken.failed.json"))
  end },

  { "a mismatched protocol version is rejected", function()
    local env = H.env({})
    local req = env:request("ping", {}, { protocol_version = "qlabs-reaper-ipc/999" })
    env:submit(req)
    env.bridge:poll_once()
    H.eq(env:result(req.request_id).error.code, protocol.ERR.IPC_PROTOCOL_MISMATCH)
  end },

  { "a request id that disagrees with the filename is rejected", function()
    local env = H.env({})
    local req = env:request("ping")
    env:submit(req, "different-name")
    env.bridge:poll_once()
    H.eq(env:result("different-name").error.code, protocol.ERR.MALFORMED_REQUEST)
  end },

  { "a command file whose name cannot be an id is quarantined, not executed", function()
    local env = H.env({})
    env.fs:write(env.ipc_dir .. "/commands/.hidden.command.json", "{}")
    env.bridge:poll_once()
    H.falsy(env.fs:exists(env.ipc_dir .. "/commands/.hidden.command.json"))
    H.eq(#env.fs:listdir(env.ipc_dir .. "/results"), 0)
  end },

  { "the bridge processes at most the per-tick limit", function()
    local env = H.env({})
    for i = 1, protocol.LIMITS.MAX_COMMANDS_PER_TICK + 3 do
      env:submit(env:request("ping"), string.format("req-%03d", i))
    end
    H.eq(env.bridge:poll_once(), protocol.LIMITS.MAX_COMMANDS_PER_TICK)
    H.eq(#env.bridge:list_commands(), 3)
  end },

  { "the directory scan is gated by the poll interval", function()
    local env = H.env({})
    env:submit(env:request("ping"), "req-a")
    env.host:advance(1.0)
    env.bridge:tick()
    H.ok(env.fs:exists(env.ipc_dir .. "/results/req-a.result.json"))
    env:submit(env:request("ping"), "req-b")
    env.host:advance(0.001)
    env.bridge:tick()
    H.falsy(env.fs:exists(env.ipc_dir .. "/results/req-b.result.json"),
      "a second scan inside the poll interval must not happen")
    env.host:advance(0.2)
    env.bridge:tick()
    H.ok(env.fs:exists(env.ipc_dir .. "/results/req-b.result.json"))
  end },

  ---------------------------------------------------------------- dispatch --
  { "status reports bridge, project and command information", function()
    local env = melody_env({})
    local res = env:roundtrip("status")
    H.eq(res.ok, true)
    local s = res.result
    H.eq(s.bridge_connected, true)
    H.eq(s.ipc_protocol_version, protocol.PROTOCOL_VERSION)
    H.eq(s.reaper_version, "7.22/linux-x86_64")
    H.eq(s.active_project, true)
    H.eq(s.active_midi_editor, true)
    H.eq(s.selected_item_count, 0)
    H.eq(#s.commands, 7)
    H.eq(s.project_name, "MockProject")
  end },

  { "inspect_selection returns a full snapshot with notes", function()
    local env = melody_env({})
    local res = env:roundtrip("inspect_selection", { note_scope = "all" })
    H.eq(res.ok, true, res.error ~= json.null and res.error.message or "")
    local s = res.result
    H.eq(s.note_count, 2)
    H.eq(s.notes[1].pitch, 60)
    H.near(s.notes[1].start_qn, 0.0, 1e-6)
    H.near(s.notes[2].end_qn, 2.0, 1e-6)
    H.eq(s.resolved_by, "active_editor")
    H.ok(s.snapshot_hash:sub(1, 8) == "fnv1a64:")
    H.eq(s.time_signature_at_item_start.numerator, 4)
    H.near(s.tempo_at_item_start, 120.0, 1e-9)
  end },

  { "inspect_selection reports NO_MIDI_SOURCE structurally", function()
    local env = melody_env({})
    env.host:set_editor_take(nil)
    local res = env:roundtrip("inspect_selection")
    H.eq(res.ok, false)
    H.eq(res.error.code, protocol.ERR.NO_MIDI_SOURCE)
  end },

  { "expected_project preconditions are enforced on inspection", function()
    local env = melody_env({})
    local res = env:roundtrip("inspect_selection", { note_scope = "all" },
      { expected_project = { project_uuid = "not-the-right-project" } })
    H.eq(res.error.code, protocol.ERR.PROJECT_CHANGED)
  end },

  { "an unsupported REAPER version blocks project commands but not ping", function()
    local env = melody_env({ app_version = "5.99" })
    H.eq(env:roundtrip("ping").ok, true)
    local res = env:roundtrip("inspect_selection")
    H.eq(res.error.code, protocol.ERR.UNSUPPORTED_REAPER_VERSION)
  end },

  { "stage, commit, discard and undo round-trip over IPC", function()
    local env = melody_env({})
    local inspect = env:roundtrip("inspect_selection", { note_scope = "all" })
    H.eq(inspect.ok, true)
    local snap = inspect.result

    local plan = {
      plan_id = "plan-1", candidate_id = "cand-1", transaction_id = "tx-1",
      base_snapshot_id = snap.snapshot_id, base_snapshot_hash = snap.snapshot_hash,
      project_uuid = snap.project_uuid, knowledge_version = "2026.07.1",
      undo_label = "QLabs MCP: Stage candidate cand-1",
      operations = json.array({
        { op = "create_folder_track", temp_id = "f", name = "QLabs Candidate 01" },
        { op = "create_track", temp_id = "t", parent = "f", name = "Chords" },
        { op = "create_midi_item", temp_id = "i", track = "t", start_qn = 0.0, end_qn = 8.0 },
        { op = "insert_notes", item = "i", notes = json.array({
          { start_qn = 0.0, end_qn = 4.0, pitch = 48, velocity = 90, channel = 0 } }) },
      }),
      preconditions = json.array({ { type = "project_uuid", value = snap.project_uuid } }),
      expected_outputs = json.array({ { temp_id = "i", kind = "midi_item", note_count = 1 } }),
    }

    local staged = env:roundtrip("stage_candidate", { plan = plan,
      note_scope = "all" })
    H.eq(staged.ok, true, staged.error ~= json.null and staged.error.message or "")
    H.eq(staged.transaction_id, "tx-1")
    H.eq(#staged.result.tracks, 2)
    H.eq(staged.result.items[1].note_count, 1)
    H.eq(env.host.ui_refresh_depth, 0)
    H.eq(#env.take.notes, 2, "the source take is untouched")

    local committed = env:roundtrip("commit_candidate", { transaction_id = "tx-1" })
    H.eq(committed.ok, true)
    H.eq(committed.result.status, "committed")

    local discarded = env:roundtrip("discard_candidate", { transaction_id = "tx-1" })
    H.eq(discarded.ok, true)
    H.eq(discarded.result.removed_tracks, 2)

    local undone = env:roundtrip("undo_last_generation", {})
    H.eq(undone.ok, true, undone.error ~= json.null and undone.error.message or "")
    H.eq(undone.result.kind, "discard")
    H.eq(#tagging.collect_owned(env.proj, "tx-1").tracks, 2, "the discard was undone")
  end },

  { "stage_candidate without a plan is INVALID_EDIT_PLAN", function()
    local env = melody_env({})
    local res = env:roundtrip("stage_candidate", {})
    H.eq(res.error.code, protocol.ERR.INVALID_EDIT_PLAN)
  end },

  { "commit and discard require a transaction id", function()
    local env = melody_env({})
    H.eq(env:roundtrip("commit_candidate", {}).error.code, protocol.ERR.MALFORMED_REQUEST)
    H.eq(env:roundtrip("discard_candidate", {}).error.code, protocol.ERR.MALFORMED_REQUEST)
  end },

  { "an unowned undo label supplied by the caller is refused", function()
    local env = melody_env({})
    local res = env:roundtrip("commit_candidate",
      { transaction_id = "tx-1", undo_label = "Delete everything" })
    H.eq(res.error.code, protocol.ERR.MALFORMED_REQUEST)
  end },

  { "undo_last_generation returns UNDO_NOT_OWNED with nothing staged", function()
    local env = melody_env({})
    H.eq(env:roundtrip("undo_last_generation", {}).error.code, protocol.ERR.UNDO_NOT_OWNED)
  end },

  { "a handler that raises becomes INTERNAL_BRIDGE_ERROR, not a crash", function()
    local env = melody_env({})
    local original = bridge_mod.handlers.status
    bridge_mod.handlers.status = function() error("kaboom") end
    local res = env:roundtrip("status")
    bridge_mod.handlers.status = original
    H.eq(res.ok, false)
    H.eq(res.error.code, protocol.ERR.INTERNAL_BRIDGE_ERROR)
    H.contains(res.error.message, "kaboom")
    H.ok(env.bridge.running, "the polling loop must survive")
  end },

  { "an oversized result is replaced by RESULT_TOO_LARGE", function()
    local env = melody_env({})
    local saved = protocol.LIMITS.MAX_RESULT_BYTES
    protocol.LIMITS.MAX_RESULT_BYTES = 64
    local res = env:roundtrip("status")
    protocol.LIMITS.MAX_RESULT_BYTES = saved
    H.eq(res.ok, false)
    H.eq(res.error.code, protocol.ERR.RESULT_TOO_LARGE)
  end },

  ----------------------------------------------------------------------- gc --
  { "garbage collection removes aged artefacts and nothing else", function()
    local env = H.env({})
    local old = util.now() - 100000
    local paths = {
      { env.ipc_dir .. "/commands/stale.tmp", true },
      { env.ipc_dir .. "/commands/stale.command.json", true },
      { env.ipc_dir .. "/processing/stale.processing.json", true },
      { env.ipc_dir .. "/results/stale.result.json", true },
      { env.ipc_dir .. "/results/stale.result.tmp", true },
      { env.ipc_dir .. "/failed/stale.failed.json", true },
      { env.ipc_dir .. "/commands/notes.txt", false },
      { env.ipc_dir .. "/results/README.md", false },
    }
    for _, p in ipairs(paths) do
      env.fs:write(p[1], "{}")
      env.fs:set_mtime(p[1], old)
    end
    env.bridge:gc()
    for _, p in ipairs(paths) do
      if p[2] then
        H.falsy(env.fs:exists(p[1]), p[1] .. " should have been collected")
      else
        H.ok(env.fs:exists(p[1]), p[1] .. " is not ours and must be left alone")
      end
    end
    H.ok(env.fs:exists(env.ipc_dir .. "/failed/stale.failed.json") == false)
  end },

  { "an abandoned processing file is quarantined rather than deleted", function()
    local env = H.env({})
    local p = env.ipc_dir .. "/processing/req-abandoned.processing.json"
    env.fs:write(p, "{}")
    env.fs:set_mtime(p, util.now() - (protocol.LIMITS.STALE_PROCESSING_SECONDS + 60))
    env.bridge:gc()
    H.falsy(env.fs:exists(p))
    H.ok(env.fs:exists(env.ipc_dir .. "/failed/req-abandoned.failed.json"))
  end },

  { "fresh artefacts are not collected", function()
    local env = H.env({})
    local p = env.ipc_dir .. "/results/fresh.result.json"
    env.fs:write(p, "{}")
    env.bridge:gc()
    H.ok(env.fs:exists(p))
  end },

  { "the log file is bounded by rotation", function()
    local env = H.env({ log_level = "debug" })
    env.bridge.log.max_bytes = 4096
    env.bridge.log.keep = protocol.LIMITS.LOG_KEEP
    for i = 1, 400 do
      env.bridge.log:info("noisy log line %d %s", i, string.rep("y", 200))
    end
    local size = env.fs:size(env.ipc_dir .. "/logs/bridge.log")
    H.ok(size <= 4096 + 512, "log must stay bounded, got " .. tostring(size))
    H.ok(env.fs:exists(env.ipc_dir .. "/logs/bridge.log.1"))
    H.ok(env.fs:exists(env.ipc_dir .. "/logs/bridge.log.3"))
    H.falsy(env.fs:exists(env.ipc_dir .. "/logs/bridge.log.4"),
      "rotations beyond keep must be dropped")
  end },

  --------------------------------------------------------------- robustness --
  { "the bridge never touches files outside the ipc directory", function()
    local env = H.env({})
    env.fs:mkdirp("/outside")
    env.fs:write("/outside/secret.command.json", "{}")
    env:submit(env:request("ping"), "req-ok")
    env.bridge:poll_once()
    env.bridge:gc()
    H.ok(env.fs:exists("/outside/secret.command.json"))
  end },

  { "snapshot hashes agree between direct calls and the IPC result", function()
    local env = melody_env({})
    local direct = snapshot.build(env.proj, { note_scope = "all" })
    local res = env:roundtrip("inspect_selection", { note_scope = "all" })
    H.eq(res.result.snapshot_hash, direct.snapshot_hash)
    H.eq(res.result.midi_hash, direct.midi_hash)
  end },

  { "the mock host exposes every reaper function the bridge calls", function()
    local state = mock_reaper.new({})
    local api = state:api()
    local required = {
      "GetResourcePath", "GetAppVersion", "time_precise", "defer", "atexit",
      "ShowConsoleMsg", "ShowMessageBox", "RecursiveCreateDirectory", "EnumerateFiles",
      "file_exists", "get_action_context", "SetToggleCommandState", "RefreshToolbar2",
      "genGuid", "EnumProjects", "GetProjectStateChangeCount", "GetProjectName",
      "GetSetProjectInfo_String", "GetPlayStateEx", "GetProjExtState", "SetProjExtState",
      "Undo_BeginBlock2", "Undo_EndBlock2", "Undo_CanUndo2", "Undo_DoUndo2",
      "PreventUIRefresh", "UpdateArrange", "CountTracks", "GetTrack", "InsertTrackInProject",
      "InsertTrackAtIndex", "GetTrackGUID", "DeleteTrack", "CountTrackMediaItems",
      "GetTrackMediaItem", "GetMediaTrackInfo_Value", "SetMediaTrackInfo_Value",
      "GetSetMediaTrackInfo_String", "CreateTrackSend", "SetTrackSendInfo_Value",
      "CountMediaItems", "GetMediaItem", "CountSelectedMediaItems", "GetSelectedMediaItem",
      "GetMediaItem_Track", "GetMediaItemInfo_Value", "SetMediaItemInfo_Value",
      "GetSetMediaItemInfo_String", "DeleteTrackMediaItem", "CreateNewMIDIItemInProj",
      "CountTakes", "GetTake", "GetActiveTake", "GetMediaItemTake_Item", "TakeIsMIDI",
      "GetSetMediaItemTakeInfo_String", "MIDI_CountEvts", "MIDI_GetNote", "MIDI_InsertNote",
      "MIDI_Sort", "MIDI_GetProjQNFromPPQPos", "MIDI_GetPPQPosFromProjQN",
      "MIDI_GetProjTimeFromPPQPos", "MIDIEditor_GetActive", "MIDIEditor_GetTake",
      "CountTempoTimeSigMarkers", "GetTempoTimeSigMarker", "TimeMap_GetTimeSigAtTime",
      "TimeMap2_timeToQN", "TimeMap2_QNToTime", "Master_GetTempo", "AddProjectMarker2",
      "ValidatePtr2",
    }
    for i = 1, #required do
      H.eq(type(api[required[i]]), "function", "mock is missing reaper." .. required[i])
    end
  end },
}
