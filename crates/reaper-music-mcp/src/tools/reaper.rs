//! The six REAPER-facing tools.
//!
//! `reaper.status` works with no bridge at all — it is how a client finds out
//! that REAPER is not running. The other five need the bridge and return a
//! clean `BRIDGE_OFFLINE` when it is absent, with a remedy naming the action to
//! run.
//!
//! # Staging
//!
//! [`stage_candidate`] is the only tool in the server that writes into a user's
//! project. It accepts a server-issued candidate id and nothing else — there is
//! no path by which a client can supply raw notes, a track name to delete or a
//! project chunk to edit. The plan it sends is built by [`crate::staging`],
//! carries all seven preconditions, and is converted for the wire by
//! [`reaper_ipc::plan::plan_to_wire`].

use super::{bool_or, opt_str, require_id, str_or};
use crate::error::{codes, ToolError};
use crate::server::{CallContext, ServerCore};
use crate::staging::{self, StageOptions};
use crate::store::{ScopeEcho, SnapshotRecord, TransactionRecord};
use qjson::{json_obj, Json, JsonMap};

/// `reaper.status`.
pub fn status(core: &ServerCore) -> Result<Json, ToolError> {
    Ok(status_body(core))
}

/// The `reaper.status` body, shared with the `reaper://status` resource.
pub fn status_body(core: &ServerCore) -> Json {
    let mut warnings: Vec<Json> = Vec::new();
    let mut m = JsonMap::new();
    m.insert("ok", Json::Bool(true));

    let configured = core.bridge.is_configured();
    let heartbeat = core.bridge.heartbeat();
    let online = heartbeat.is_ok();

    if !configured {
        warnings.push(json_obj! {
            "code" => "bridge_not_configured",
            "message" => "no IPC directory is configured; pass --config or --ipc-dir to reach REAPER",
            "severity" => "minor",
        });
    } else if let Err(e) = &heartbeat {
        warnings.push(json_obj! {
            "code" => "bridge_offline",
            "message" => e.message.clone(),
            "severity" => "minor",
        });
    }

    m.insert("bridge_connected", Json::Bool(online));
    m.insert("bridge_configured", Json::Bool(configured));
    m.insert(
        "heartbeat_age_seconds",
        match &heartbeat {
            Ok(h) => Json::Float(h.age.as_secs_f64()),
            Err(_) => Json::Null,
        },
    );
    m.insert(
        "bridge_version",
        match &heartbeat {
            Ok(h) => Json::Str(h.bridge_version.clone()),
            Err(_) => Json::Null,
        },
    );
    m.insert(
        "reaper_version",
        match &heartbeat {
            Ok(h) => Json::Str(h.reaper_version.clone()),
            Err(_) => Json::Null,
        },
    );
    m.insert(
        "ipc_protocol_version",
        Json::Str(reaper_ipc::PROTOCOL_VERSION.to_string()),
    );
    m.insert(
        "mcp_protocol_version",
        Json::Str(crate::MCP_PROTOCOL_VERSION.to_string()),
    );
    m.insert(
        "server_version",
        Json::Str(crate::SERVER_VERSION.to_string()),
    );

    // Project detail needs a real round trip; only attempt it when the
    // heartbeat says the bridge is there, so an offline server never blocks.
    let project = if online { core.bridge.status().ok() } else { None };
    let field = |key: &str, fallback: Json| -> Json {
        project
            .as_ref()
            .and_then(|p| p.get(key).cloned())
            .filter(|v| !v.is_null())
            .unwrap_or(fallback)
    };

    m.insert("active_project", field("active_project", Json::Bool(false)));
    m.insert("project_uuid", field("project_uuid", Json::Null));
    m.insert("project_name", field("project_name", Json::Null));
    m.insert("project_path", field("project_path", Json::Null));
    m.insert("play_state", field("play_state", Json::Null));
    m.insert(
        "selected_item_count",
        match field("selected_item_count", Json::Int(0)) {
            Json::Int(i) => Json::Int(i.max(0)),
            _ => Json::Int(0),
        },
    );
    m.insert(
        "active_midi_editor",
        field("active_midi_editor", Json::Bool(false)),
    );
    m.insert("knowledge_version", Json::Str(core.knowledge_version()));
    m.insert("knowledge_hash", Json::Str(core.knowledge_hash()));
    m.insert(
        "ipc_dir",
        match core.bridge.ipc_dir() {
            Some(p) => Json::Str(p.display().to_string()),
            None => Json::Null,
        },
    );

    let mut session = match core.with_store(|s| s.stats()) {
        Json::Obj(o) => o,
        _ => JsonMap::new(),
    };
    session.insert(
        "uptime_seconds",
        Json::Int(qjson::time::unix_now() - core.started_at),
    );
    session.insert("knowledge_origin", Json::Str(core.knowledge.origin().to_string()));
    m.insert("session", Json::Obj(session));
    m.insert("warnings", Json::Arr(warnings));
    Json::Obj(m)
}

/// Reads the selection-scope arguments shared by `inspect_selection` and the
/// staleness re-derivation the bridge performs during staging.
pub fn scope_of(args: &Json) -> ScopeEcho {
    let extraction = args.get("melody_extraction");
    ScopeEcho {
        source_mode: str_or(args, "source_mode", "auto"),
        note_scope: str_or(args, "note_scope", "selected_or_all"),
        extraction_mode: extraction
            .and_then(|e| e.get("mode"))
            .and_then(Json::as_str)
            .unwrap_or("auto")
            .to_string(),
        extraction_channel: extraction.and_then(|e| e.get("channel")).and_then(Json::as_i64),
    }
}

/// `reaper.inspect_selection`.
pub fn inspect_selection(
    core: &ServerCore,
    args: &Json,
    ctx: &CallContext,
) -> Result<Json, ToolError> {
    let scope = scope_of(args);
    if scope.extraction_mode == "midi_channel" && scope.extraction_channel.is_none() {
        return Err(ToolError::with_details(
            codes::INVALID_ARGUMENT,
            "melody_extraction.channel is required when the mode is midi_channel",
            json_obj! { "argument" => "melody_extraction.channel" },
        ));
    }
    ctx.check_cancelled()?;
    ctx.report(0.1, "asking the bridge for the current selection");

    let snapshot = core.bridge.inspect(scope.to_json(), &ctx.cancel)?;
    ctx.report(0.7, "converting the snapshot");

    let notes = crate::convert::note_set_of(&snapshot)?;
    let (item_start, item_end) = crate::convert::item_bounds(&snapshot);
    crate::log::debug(&crate::log::note_summary(
        "inspected selection",
        notes.notes.len(),
        item_start,
        item_end,
    ));

    let record = SnapshotRecord {
        raw: snapshot.raw.clone(),
        scope: ScopeEcho {
            note_scope: snapshot
                .note_scope
                .clone()
                .unwrap_or_else(|| scope.note_scope.clone()),
            extraction_mode: snapshot
                .extraction_mode
                .clone()
                .unwrap_or_else(|| scope.extraction_mode.clone()),
            extraction_channel: snapshot.extraction_channel.or(scope.extraction_channel),
            source_mode: scope.source_mode.clone(),
        },
        snapshot: snapshot.clone(),
        notes,
    };
    let scope = record.scope.clone();
    let id = core.with_store(|s| s.put_snapshot(record.clone(), ctx.now));

    let raw = &snapshot.raw;
    let mut m = JsonMap::new();
    m.insert("ok", Json::Bool(true));
    m.insert("snapshot_id", Json::Str(id.clone()));
    m.insert("resource_uri", Json::Str("reaper://selection/current".into()));
    m.insert("project_uuid", opt_string(&snapshot.project_uuid));
    m.insert(
        "project_state_change_count",
        Json::Int(snapshot.project_state_change_count),
    );
    m.insert("track_guid", opt_string(&snapshot.track_guid));
    m.insert("item_guid", opt_string(&snapshot.item_guid));
    m.insert("take_guid", opt_string(&snapshot.take_guid));
    m.insert("item_start_qn", Json::Float(item_start));
    m.insert("item_end_qn", Json::Float(item_end));
    m.insert("item_length_qn", Json::Float(item_end - item_start));
    m.insert("is_loop_source", Json::Bool(snapshot.is_loop_source));
    m.insert("midi_hash", opt_string(&snapshot.midi_hash));
    m.insert("tempo_map_hash", opt_string(&snapshot.tempo_map_hash));
    m.insert("snapshot_hash", opt_string(&snapshot.snapshot_hash));
    m.insert("note_list_hash", opt_string(&snapshot.note_list_hash));
    m.insert("note_count", Json::Int(snapshot.note_count));
    m.insert("source_note_count", Json::Int(snapshot.source_note_count));
    m.insert(
        "notes",
        raw.get("notes").cloned().unwrap_or(Json::Arr(Vec::new())),
    );
    m.insert(
        "tempo_bpm_at_start",
        Json::Float(
            raw.get("tempo_at_item_start")
                .and_then(Json::as_f64)
                .unwrap_or(120.0),
        ),
    );
    m.insert(
        "time_signature_at_start",
        raw.get("time_signature_at_item_start")
            .cloned()
            .unwrap_or_else(|| json_obj! { "numerator" => 4, "denominator" => 4 }),
    );
    m.insert("note_scope", Json::Str(scope.note_scope.clone()));
    m.insert("extraction_mode", Json::Str(scope.extraction_mode.clone()));
    m.insert(
        "extraction_channel",
        match scope.extraction_channel {
            Some(c) => Json::Int(c),
            None => Json::Null,
        },
    );
    m.insert("resolved_by", opt_string(&snapshot.resolved_by));
    m.insert(
        "selection_assumptions",
        Json::Arr(
            snapshot
                .selection_assumptions
                .iter()
                .map(|a| Json::Str(a.clone()))
                .collect(),
        ),
    );
    m.insert("warnings", Json::Arr(snapshot.warnings.clone()));
    ctx.report(1.0, "snapshot stored");
    Ok(Json::Obj(m))
}

fn opt_string(v: &Option<String>) -> Json {
    match v {
        Some(s) => Json::Str(s.clone()),
        None => Json::Null,
    }
}

/// `reaper.stage_candidate`.
pub fn stage_candidate(
    core: &ServerCore,
    args: &Json,
    ctx: &CallContext,
) -> Result<Json, ToolError> {
    let candidate_id = require_id(args, "candidate_id")?;
    let record = core.with_store(|s| s.candidate(&candidate_id, ctx.now).cloned())?;
    let snapshot = core.with_store(|s| s.snapshot(&record.snapshot_id, ctx.now).cloned())?;

    let options = StageOptions {
        track_name_prefix: str_or(args, "track_name_prefix", staging::DEFAULT_TRACK_PREFIX),
        folder_name: str_or(
            args,
            "folder_name",
            &format!(
                "{} {}",
                staging::DEFAULT_FOLDER_NAME,
                candidate_id.chars().take(8).collect::<String>()
            ),
        ),
        muted: bool_or(args, "muted", true),
        create_region: bool_or(args, "create_region", false),
        route_to_source_track: bool_or(args, "route_to_source_track", false),
    };
    let verify_snapshot = bool_or(args, "verify_snapshot", true);

    ctx.check_cancelled()?;
    ctx.report(0.2, "building the edit plan");

    let built = {
        let ids = match core.ids.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        staging::build_plan(
            &record.candidate,
            &snapshot,
            &core.knowledge_version(),
            &options,
            &ids,
        )?
    };

    let plan_id = built.plan.plan_id.clone();
    let transaction_id = built.plan.transaction_id.clone();
    let undo_label = built.plan.undo_label.clone();
    let precondition_kinds = staging::precondition_kinds(&built.plan);

    core.with_store(|s| {
        s.put_plan(
            crate::store::PlanRecord {
                plan: built.plan.clone(),
                candidate_id: candidate_id.clone(),
                snapshot_id: record.snapshot_id.clone(),
                scope: built.scope.clone(),
            },
            ctx.now,
        )
    });

    ctx.report(0.5, "staging into REAPER");
    let payload = staging::stage_payload(&built, verify_snapshot);
    let result = core.bridge.stage(&built.plan, payload.clone())?;

    core.with_store(|s| {
        s.put_transaction(
            TransactionRecord {
                transaction_id: transaction_id.clone(),
                plan_id: plan_id.clone(),
                candidate_id: candidate_id.clone(),
                snapshot_id: record.snapshot_id.clone(),
                status: "staged".to_string(),
                undo_label: undo_label.clone(),
                stage_result: result.clone(),
            },
            ctx.now,
        )
    });

    let arr = |key: &str| result.get(key).cloned().unwrap_or(Json::Arr(Vec::new()));
    ctx.report(1.0, "staged");
    Ok(json_obj! {
        "ok" => true,
        "transaction_id" => transaction_id.clone(),
        "plan_id" => plan_id.clone(),
        "candidate_id" => candidate_id,
        "snapshot_id" => record.snapshot_id.clone(),
        "undo_label" => result
            .get("undo_label")
            .and_then(Json::as_str)
            .unwrap_or(&undo_label)
            .to_string(),
        "status" => result.get("status").and_then(Json::as_str).unwrap_or("preview").to_string(),
        "tracks" => arr("tracks"),
        "items" => arr("items"),
        "regions" => arr("regions"),
        "sends" => arr("sends"),
        "note_count" => result
            .get("note_count")
            .and_then(Json::as_i64)
            .unwrap_or(built.note_count as i64),
        "project_state_change_count" => result
            .get("project_state_change_count")
            .and_then(Json::as_i64)
            .unwrap_or(0),
        "scope_echoed" => payload,
        "precondition_kinds" => Json::Arr(
            precondition_kinds.into_iter().map(Json::Str).collect()
        ),
        "plan_uri" => format!("editplan://{plan_id}"),
        "transaction_uri" => format!("transaction://{transaction_id}"),
        "warnings" => arr("warnings"),
    })
}

/// Reads a transaction id argument and checks this server issued it.
fn owned_transaction(
    core: &ServerCore,
    args: &Json,
    ctx: &CallContext,
    required: bool,
) -> Result<Option<TransactionRecord>, ToolError> {
    let Some(id) = opt_str(args, "transaction_id") else {
        if required {
            return Err(ToolError::with_details(
                codes::INVALID_ARGUMENT,
                "transaction_id is required",
                json_obj! { "argument" => "transaction_id" },
            ));
        }
        return Ok(None);
    };
    if !crate::resources::is_safe_segment(id) {
        return Err(ToolError::with_details(
            codes::INVALID_ARGUMENT,
            "transaction_id is not a well-formed identifier",
            json_obj! { "argument" => "transaction_id", "value" => id },
        ));
    }
    let record = core.with_store(|s| s.transaction(id, ctx.now).cloned());
    match record {
        Ok(r) => Ok(Some(r)),
        Err(e) if e.code == codes::UNKNOWN_ID => Err(ToolError::with_details(
            codes::UNKNOWN_TRANSACTION,
            format!("transaction {id} was not staged by this server session"),
            json_obj! { "transaction_id" => id },
        )
        .remedy("only a transaction id returned by reaper.stage_candidate can be acted on")),
        Err(e) => Err(e),
    }
}

/// `reaper.commit_candidate`.
pub fn commit_candidate(
    core: &ServerCore,
    args: &Json,
    ctx: &CallContext,
) -> Result<Json, ToolError> {
    let record = owned_transaction(core, args, ctx, true)?.expect("required");
    let result = core.bridge.commit(&record.transaction_id)?;
    core.with_store(|s| s.set_transaction_status(&record.transaction_id, "committed"));
    Ok(json_obj! {
        "ok" => true,
        "transaction_id" => record.transaction_id.clone(),
        "status" => result.get("status").and_then(Json::as_str).unwrap_or("committed").to_string(),
        "undo_label" => result
            .get("undo_label")
            .and_then(Json::as_str)
            .unwrap_or(&record.undo_label)
            .to_string(),
        "committed_tracks" => result.get("committed_tracks").and_then(Json::as_i64).unwrap_or(0),
        "committed_items" => result.get("committed_items").and_then(Json::as_i64).unwrap_or(0),
        "committed_takes" => result.get("committed_takes").and_then(Json::as_i64).unwrap_or(0),
        "project_state_change_count" => result
            .get("project_state_change_count")
            .and_then(Json::as_i64)
            .unwrap_or(0),
        "warnings" => result.get("warnings").cloned().unwrap_or(Json::Arr(Vec::new())),
    })
}

/// `reaper.discard_candidate`.
pub fn discard_candidate(
    core: &ServerCore,
    args: &Json,
    ctx: &CallContext,
) -> Result<Json, ToolError> {
    let record = owned_transaction(core, args, ctx, true)?.expect("required");
    let result = core.bridge.discard(&record.transaction_id)?;
    core.with_store(|s| s.set_transaction_status(&record.transaction_id, "discarded"));
    Ok(json_obj! {
        "ok" => true,
        "transaction_id" => record.transaction_id.clone(),
        "undo_label" => result
            .get("undo_label")
            .and_then(Json::as_str)
            .unwrap_or(&record.undo_label)
            .to_string(),
        "removed_items" => result.get("removed_items").and_then(Json::as_i64).unwrap_or(0),
        "removed_tracks" => result.get("removed_tracks").and_then(Json::as_i64).unwrap_or(0),
        "retained_tracks" => result.get("retained_tracks").and_then(Json::as_i64).unwrap_or(0),
        "project_state_change_count" => result
            .get("project_state_change_count")
            .and_then(Json::as_i64)
            .unwrap_or(0),
        "warnings" => result.get("warnings").cloned().unwrap_or(Json::Arr(Vec::new())),
    })
}

/// `reaper.undo_last_generation`.
///
/// The bridge is the authority on ownership — it refuses to undo anything whose
/// undo label is not this MCP's own. The server adds a second gate on top: the
/// transaction id, when supplied, must be one this session staged.
pub fn undo_last_generation(
    core: &ServerCore,
    args: &Json,
    ctx: &CallContext,
) -> Result<Json, ToolError> {
    let explicit = owned_transaction(core, args, ctx, false)?;
    let id = match &explicit {
        Some(r) => r.transaction_id.clone(),
        None => core
            .with_store(|s| s.latest_staged_transaction(ctx.now).cloned())
            .map(|r| r.transaction_id)
            .unwrap_or_default(),
    };
    if id.is_empty() {
        return Err(ToolError::new(
            reaper_ipc::codes::UNDO_NOT_OWNED,
            "this server session has not staged anything to undo",
        )
        .remedy("only a transaction staged in this session can be undone"));
    }
    let result = core.bridge.undo(&id)?;
    core.with_store(|s| s.set_transaction_status(&id, "undone"));
    Ok(json_obj! {
        "ok" => true,
        "undone" => result.get("undone").and_then(Json::as_bool).unwrap_or(false),
        "transaction_id" => result
            .get("transaction_id")
            .and_then(Json::as_str)
            .unwrap_or(&id)
            .to_string(),
        "undo_label" => result.get("undo_label").cloned().unwrap_or(Json::Null),
        "kind" => result.get("kind").cloned().unwrap_or(Json::Null),
        "project_state_change_count" => result
            .get("project_state_change_count")
            .and_then(Json::as_i64)
            .unwrap_or(0),
        "warnings" => result.get("warnings").cloned().unwrap_or(Json::Arr(Vec::new())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::ServerCore;

    #[test]
    fn status_works_with_no_bridge() {
        let core = ServerCore::offline();
        let body = status(&core).unwrap();
        assert_eq!(body.get("ok"), Some(&Json::Bool(true)));
        assert_eq!(body.get("bridge_connected"), Some(&Json::Bool(false)));
        assert_eq!(body.get("bridge_configured"), Some(&Json::Bool(false)));
        assert_eq!(
            body.str_field("mcp_protocol_version").unwrap(),
            crate::MCP_PROTOCOL_VERSION
        );
        assert_eq!(
            body.str_field("ipc_protocol_version").unwrap(),
            reaper_ipc::PROTOCOL_VERSION
        );
        assert!(!body.arr_field("warnings").unwrap().is_empty());
    }

    #[test]
    fn status_validates_against_its_declared_schema() {
        let core = ServerCore::offline();
        let tool = core.tools.get("reaper.status").unwrap();
        let body = status(&core).unwrap();
        assert!(tool.output.validate(&body).is_empty(), "{body:?}");
    }

    #[test]
    fn scope_defaults_match_the_wire_defaults() {
        let s = scope_of(&json_obj! {});
        assert_eq!(s.source_mode, "auto");
        assert_eq!(s.note_scope, "selected_or_all");
        assert_eq!(s.extraction_mode, "auto");
        assert_eq!(s.extraction_channel, None);
    }

    #[test]
    fn scope_reads_an_explicit_channel() {
        let s = scope_of(&json_obj! {
            "source_mode" => "selected_item",
            "note_scope" => "selected_only",
            "melody_extraction" => json_obj! { "mode" => "midi_channel", "channel" => 4 },
        });
        assert_eq!(s.extraction_mode, "midi_channel");
        assert_eq!(s.extraction_channel, Some(4));
    }

    #[test]
    fn midi_channel_mode_without_a_channel_is_refused_before_the_bridge() {
        let core = ServerCore::offline();
        let e = inspect_selection(
            &core,
            &json_obj! { "melody_extraction" => json_obj! { "mode" => "midi_channel" } },
            &CallContext::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::INVALID_ARGUMENT);
    }

    #[test]
    fn staging_refuses_a_candidate_this_server_did_not_issue() {
        let core = ServerCore::offline();
        let e = stage_candidate(
            &core,
            &json_obj! { "candidate_id" => "not-mine" },
            &CallContext::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::UNKNOWN_ID);
    }

    #[test]
    fn transaction_tools_refuse_a_transaction_this_server_did_not_stage() {
        let core = ServerCore::offline();
        for f in [
            commit_candidate as fn(&ServerCore, &Json, &CallContext) -> Result<Json, ToolError>,
            discard_candidate,
        ] {
            let e = f(
                &core,
                &json_obj! { "transaction_id" => "abc123" },
                &CallContext::detached(),
            )
            .unwrap_err();
            assert_eq!(e.code, codes::UNKNOWN_TRANSACTION);
        }
    }

    #[test]
    fn undo_without_a_staged_transaction_is_not_owned() {
        let core = ServerCore::offline();
        let e = undo_last_generation(&core, &json_obj! {}, &CallContext::detached()).unwrap_err();
        assert_eq!(e.code, reaper_ipc::codes::UNDO_NOT_OWNED);
    }

    #[test]
    fn a_path_shaped_transaction_id_is_refused() {
        let core = ServerCore::offline();
        let e = commit_candidate(
            &core,
            &json_obj! { "transaction_id" => "..%2f..%2fetc" },
            &CallContext::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::INVALID_ARGUMENT);
    }
}
