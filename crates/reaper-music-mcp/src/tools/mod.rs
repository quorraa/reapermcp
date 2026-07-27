//! The fourteen tools, and the envelope every one of them returns.
//!
//! [`call`] is the only entry point. It does four things in a fixed order:
//!
//! 1. resolves the tool name against the closed registry;
//! 2. validates the arguments against the tool's declared `inputSchema`;
//! 3. runs the tool;
//! 4. validates the result against the tool's declared `outputSchema`.
//!
//! Every failure at any of those steps becomes a `tools/call` **result** with
//! `isError: true` and a structured payload — never a JSON-RPC transport error.
//! That is what the MCP specification asks for, and it is also what makes a
//! failure legible to a model: a transport error says "the call broke", a
//! structured tool error says "the bridge is offline, here is how to start it".
//!
//! Every successful result carries both `structuredContent` (the validated
//! object) and a `content` array with the same object serialized as text, so a
//! client without structured-output support still sees everything.

pub mod analysis;
pub mod arrange;
pub mod explain;
pub mod harmony;
pub mod loops;
pub mod reaper;
pub mod theory;

use crate::error::{codes, ToolError};
use crate::server::{CallContext, ServerCore};
use crate::store::SnapshotRecord;
use qjson::{json_obj, Json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The style profile used when the caller does not name one.
pub const DEFAULT_PROFILE: &str = "common_practice";

/// The tie-breaking seed used when the caller does not supply one.
pub const DEFAULT_SEED: u64 = 20_260_726;

/// Runs one tool call, end to end.
pub fn call(core: &ServerCore, name: &str, args: &Json, ctx: &CallContext) -> Json {
    let Some(tool) = core.tools.get(name) else {
        return error_result(&unknown_tool(core, name));
    };

    let violations = tool.input.validate(args);
    if !violations.is_empty() {
        return error_result(&invalid_arguments(name, &violations));
    }

    crate::log::debug(&format!("running tool {name}"));
    let outcome = dispatch(core, name, args, ctx);

    match outcome {
        Ok(body) => {
            let violations = tool.output.validate(&body);
            if violations.is_empty() {
                success_result(&body)
            } else {
                // A tool that fails its own declared schema is a server bug.
                // Report it rather than serving output no client can rely on.
                crate::log::error(&format!(
                    "tool {name} produced output that fails its declared outputSchema: {}",
                    violations
                        .iter()
                        .map(|v| format!("{}{}: {}", v.instance_path, v.keyword, v.message))
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
                error_result(&output_violation(name, &violations))
            }
        }
        Err(e) => {
            crate::log::debug(&format!("tool {name} failed: {}", e.code));
            error_result(&e)
        }
    }
}

fn dispatch(
    core: &ServerCore,
    name: &str,
    args: &Json,
    ctx: &CallContext,
) -> Result<Json, ToolError> {
    match name {
        "reaper.status" => reaper::status(core),
        "reaper.inspect_selection" => reaper::inspect_selection(core, args, ctx),
        "theory.search" => theory::search(core, args),
        "music.analyze_selection" => analysis::analyze_selection(core, args, ctx),
        "harmony.generate_candidates" => harmony::generate_candidates(core, args, ctx),
        "harmony.reharmonize" => harmony::reharmonize(core, args, ctx),
        "voicing.generate" => harmony::voicing_generate(core, args, ctx),
        "arrangement.generate" => arrange::generate(core, args, ctx),
        "loop.audit" => loops::audit(core, args, ctx),
        "candidate.explain" => explain::explain(core, args, ctx),
        "reaper.stage_candidate" => reaper::stage_candidate(core, args, ctx),
        "reaper.commit_candidate" => reaper::commit_candidate(core, args, ctx),
        "reaper.discard_candidate" => reaper::discard_candidate(core, args, ctx),
        "reaper.undo_last_generation" => reaper::undo_last_generation(core, args, ctx),
        other => Err(unknown_tool(core, other)),
    }
}

/// The `tools/call` result for a successful call.
pub fn success_result(body: &Json) -> Json {
    json_obj! {
        "content" => Json::Arr(vec![json_obj! {
            "type" => "text",
            "text" => body.to_string_pretty(),
        }]),
        "structuredContent" => body.clone(),
        "isError" => false,
    }
}

/// The `tools/call` result for a failed call.
pub fn error_result(err: &ToolError) -> Json {
    let payload = err.to_json();
    json_obj! {
        "content" => Json::Arr(vec![json_obj! {
            "type" => "text",
            "text" => payload.to_string_pretty(),
        }]),
        "structuredContent" => payload,
        "isError" => true,
    }
}

fn unknown_tool(core: &ServerCore, name: &str) -> ToolError {
    ToolError::with_details(
        codes::UNKNOWN_TOOL,
        format!("{name} is not a tool this server exposes"),
        json_obj! {
            "requested" => name,
            "available" => Json::Arr(
                core.tools
                    .names()
                    .iter()
                    .map(|n| Json::Str((*n).to_string()))
                    .collect(),
            ),
        },
    )
    .remedy("call tools/list for the served set")
}

fn invalid_arguments(name: &str, violations: &[qjson::schema::Violation]) -> ToolError {
    ToolError::with_details(
        codes::INVALID_ARGUMENTS,
        format!(
            "arguments for {name} failed its declared input schema ({} problem{})",
            violations.len(),
            if violations.len() == 1 { "" } else { "s" }
        ),
        json_obj! {
            "tool" => name,
            "violations" => violations_json(violations),
        },
    )
    .remedy("read the tool's inputSchema in tools/list and correct the arguments")
}

fn output_violation(name: &str, violations: &[qjson::schema::Violation]) -> ToolError {
    ToolError::with_details(
        codes::OUTPUT_SCHEMA_VIOLATION,
        format!("{name} produced output that does not satisfy its declared outputSchema"),
        json_obj! {
            "tool" => name,
            "violations" => violations_json(violations),
        },
    )
    .remedy("this is a server bug; please report it with the violations above")
}

fn violations_json(violations: &[qjson::schema::Violation]) -> Json {
    Json::Arr(
        violations
            .iter()
            .take(20)
            .map(|v| {
                json_obj! {
                    "path" => v.instance_path.clone(),
                    "keyword" => v.keyword.clone(),
                    "message" => v.message.clone(),
                }
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

/// An optional string argument.
pub fn opt_str<'a>(args: &'a Json, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Json::as_str)
}

/// A string argument with a default.
pub fn str_or(args: &Json, key: &str, default: &str) -> String {
    opt_str(args, key).unwrap_or(default).to_string()
}

/// An optional finite number argument.
pub fn opt_f64(args: &Json, key: &str) -> Option<f64> {
    args.get(key)
        .and_then(Json::as_f64)
        .filter(|v| v.is_finite())
}

/// A number argument with a default.
pub fn f64_or(args: &Json, key: &str, default: f64) -> f64 {
    opt_f64(args, key).unwrap_or(default)
}

/// An optional integer argument.
pub fn opt_i64(args: &Json, key: &str) -> Option<i64> {
    args.get(key).and_then(Json::as_i64)
}

/// An integer argument with a default.
pub fn i64_or(args: &Json, key: &str, default: i64) -> i64 {
    opt_i64(args, key).unwrap_or(default)
}

/// A boolean argument with a default.
pub fn bool_or(args: &Json, key: &str, default: bool) -> bool {
    args.get(key).and_then(Json::as_bool).unwrap_or(default)
}

/// The seed argument, defaulting to a fixed constant so that a call made twice
/// with no seed returns the identical candidates.
pub fn seed_of(args: &Json) -> u64 {
    opt_i64(args, "seed")
        .map(|s| s as u64)
        .unwrap_or(DEFAULT_SEED)
}

/// The style profile argument, checked against the knowledge bundle.
pub fn profile_of(core: &ServerCore, args: &Json) -> Result<String, ToolError> {
    let id = str_or(args, "style_profile", DEFAULT_PROFILE);
    if core.knowledge.kb().profile(&id).is_none() {
        return Err(ToolError::with_details(
            codes::UNKNOWN_PROFILE,
            format!("no style profile {id} in this knowledge bundle"),
            json_obj! { "profile_id" => id.clone() },
        )
        .remedy("read theory://profiles for the served set"));
    }
    Ok(id)
}

/// Reads a required, server-issued id argument.
pub fn require_id(args: &Json, key: &str) -> Result<String, ToolError> {
    match opt_str(args, key) {
        Some(v) if crate::resources::is_safe_segment(v) => Ok(v.to_string()),
        Some(v) => Err(ToolError::with_details(
            codes::INVALID_ARGUMENT,
            format!("{key} is not a well-formed identifier"),
            json_obj! { "argument" => key, "value" => v },
        )),
        None => Err(ToolError::with_details(
            codes::INVALID_ARGUMENT,
            format!("{key} is required"),
            json_obj! { "argument" => key },
        )),
    }
}

/// Reads a stored snapshot, cloning it out of the store.
pub fn require_snapshot(
    core: &ServerCore,
    args: &Json,
    ctx: &CallContext,
) -> Result<SnapshotRecord, ToolError> {
    let id = require_id(args, "snapshot_id")?;
    core.with_store(|s| s.snapshot(&id, ctx.now).cloned())
}

/// A `warnings` array built from domain warnings.
pub fn warnings_json(warnings: &[music_domain::candidate::Warning]) -> Json {
    Json::Arr(warnings.iter().map(|w| w.to_json()).collect())
}

/// An empty `warnings` array.
pub fn no_warnings() -> Json {
    Json::Arr(Vec::new())
}

/// Bridges the IPC cancellation flag onto the engines' own flag type.
///
/// `reaper-ipc` and `harmony-engine` each define a `CancelFlag`, and neither
/// depends on the other, so the two cannot share an `Arc`. A watcher thread
/// polls the client-facing flag and mirrors it onto the engine flag, which is
/// what lets a `notifications/cancelled` stop a generation already in progress
/// rather than only being noticed once it finishes.
pub struct CancelBridge {
    engine: harmony_engine::CancelFlag,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for CancelBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelBridge")
            .field("cancelled", &self.engine.is_cancelled())
            .finish()
    }
}

impl CancelBridge {
    /// Starts mirroring `source` onto a fresh engine flag.
    pub fn new(source: &reaper_ipc::CancelFlag) -> CancelBridge {
        let engine = harmony_engine::CancelFlag::new();
        if source.is_cancelled() {
            engine.cancel();
            return CancelBridge {
                engine,
                stop: Arc::new(AtomicBool::new(true)),
                handle: None,
            };
        }
        let stop = Arc::new(AtomicBool::new(false));
        let watcher_stop = Arc::clone(&stop);
        let watcher_source = source.clone();
        let watcher_engine = engine.clone();
        let handle = std::thread::spawn(move || {
            while !watcher_stop.load(Ordering::Relaxed) {
                if watcher_source.is_cancelled() {
                    watcher_engine.cancel();
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
        CancelBridge {
            engine,
            stop,
            handle: Some(handle),
        }
    }

    /// The engine-facing flag.
    pub fn flag(&self) -> &harmony_engine::CancelFlag {
        &self.engine
    }
}

impl Drop for CancelBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::ServerCore;

    #[test]
    fn an_unknown_tool_is_a_structured_error_not_a_transport_error() {
        let core = ServerCore::offline();
        let r = call(
            &core,
            "execute_lua",
            &json_obj! {},
            &CallContext::detached(),
        );
        assert_eq!(r.get("isError"), Some(&Json::Bool(true)));
        let payload = r.get("structuredContent").unwrap();
        assert_eq!(
            payload.str_field("error_code").unwrap(),
            codes::UNKNOWN_TOOL
        );
        assert!(!r.arr_field("content").unwrap().is_empty());
    }

    #[test]
    fn invalid_arguments_produce_a_structured_error() {
        let core = ServerCore::offline();
        let r = call(
            &core,
            "harmony.generate_candidates",
            &json_obj! { "snapshot_id" => "s1", "candidate_count" => 99 },
            &CallContext::detached(),
        );
        assert_eq!(r.get("isError"), Some(&Json::Bool(true)));
        let payload = r.get("structuredContent").unwrap();
        assert_eq!(
            payload.str_field("error_code").unwrap(),
            codes::INVALID_ARGUMENTS
        );
        assert!(!payload
            .get("details")
            .unwrap()
            .arr_field("violations")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_successful_result_carries_both_representations() {
        let core = ServerCore::offline();
        let r = call(
            &core,
            "reaper.status",
            &json_obj! {},
            &CallContext::detached(),
        );
        assert_eq!(r.get("isError"), Some(&Json::Bool(false)));
        let structured = r.get("structuredContent").unwrap();
        let text = r.arr_field("content").unwrap()[0]
            .str_field("text")
            .unwrap()
            .to_string();
        assert_eq!(&Json::parse(&text).unwrap(), structured);
    }

    #[test]
    fn bridge_tools_report_offline_rather_than_hanging() {
        let core = ServerCore::offline();
        let r = call(
            &core,
            "reaper.inspect_selection",
            &json_obj! {},
            &CallContext::detached(),
        );
        assert_eq!(r.get("isError"), Some(&Json::Bool(true)));
        let payload = r.get("structuredContent").unwrap();
        assert_eq!(
            payload.str_field("error_code").unwrap(),
            reaper_ipc::codes::BRIDGE_OFFLINE
        );
        assert!(payload.get("remedy").is_some());
    }

    #[test]
    fn every_bridge_tool_fails_structurally_with_no_bridge() {
        // The transaction tools refuse an id this session did not issue before
        // they ever reach the bridge, which is a stricter refusal than
        // BRIDGE_OFFLINE and is the one that matters for safety. What must hold
        // for all of them is that they answer, quickly, with a known code.
        let core = ServerCore::offline();
        let known = [
            reaper_ipc::codes::BRIDGE_OFFLINE,
            reaper_ipc::codes::UNDO_NOT_OWNED,
            codes::UNKNOWN_TRANSACTION,
            codes::UNKNOWN_ID,
        ];
        for (name, args) in [
            ("reaper.inspect_selection", json_obj! {}),
            (
                "reaper.stage_candidate",
                json_obj! { "candidate_id" => "abc" },
            ),
            (
                "reaper.commit_candidate",
                json_obj! { "transaction_id" => "abc" },
            ),
            (
                "reaper.discard_candidate",
                json_obj! { "transaction_id" => "abc" },
            ),
            ("reaper.undo_last_generation", json_obj! {}),
        ] {
            let r = call(&core, name, &args, &CallContext::detached());
            assert_eq!(r.get("isError"), Some(&Json::Bool(true)), "{name}");
            let code = r
                .get("structuredContent")
                .unwrap()
                .str_field("error_code")
                .unwrap()
                .to_string();
            assert!(known.contains(&code.as_str()), "{name} gave {code}");
        }
    }

    #[test]
    fn argument_helpers_apply_their_defaults() {
        let args = json_obj! { "a" => "x", "n" => 3, "f" => 0.5, "b" => true };
        assert_eq!(str_or(&args, "a", "d"), "x");
        assert_eq!(str_or(&args, "missing", "d"), "d");
        assert_eq!(i64_or(&args, "n", 0), 3);
        assert_eq!(i64_or(&args, "missing", 7), 7);
        assert!((f64_or(&args, "f", 0.0) - 0.5).abs() < 1e-9);
        assert!(bool_or(&args, "b", false));
        assert!(!bool_or(&args, "missing", false));
        assert_eq!(seed_of(&json_obj! {}), DEFAULT_SEED);
        assert_eq!(seed_of(&json_obj! { "seed" => 42 }), 42);
    }

    #[test]
    fn profile_of_rejects_an_unknown_profile() {
        let core = ServerCore::offline();
        assert_eq!(profile_of(&core, &json_obj! {}).unwrap(), DEFAULT_PROFILE);
        let e = profile_of(&core, &json_obj! { "style_profile" => "nope" }).unwrap_err();
        assert_eq!(e.code, codes::UNKNOWN_PROFILE);
    }

    #[test]
    fn require_id_rejects_path_shaped_values() {
        for bad in ["../etc", "a/b", "", ".hidden"] {
            let e = require_id(&json_obj! { "candidate_id" => bad }, "candidate_id").unwrap_err();
            assert_eq!(e.code, codes::INVALID_ARGUMENT, "{bad}");
        }
        assert!(require_id(&json_obj! {}, "candidate_id").is_err());
        assert_eq!(
            require_id(&json_obj! { "candidate_id" => "ab-1" }, "candidate_id").unwrap(),
            "ab-1"
        );
    }

    #[test]
    fn the_cancel_bridge_mirrors_an_already_cancelled_flag() {
        let ipc = reaper_ipc::CancelFlag::new();
        ipc.cancel();
        let bridge = CancelBridge::new(&ipc);
        assert!(bridge.flag().is_cancelled());
    }

    #[test]
    fn the_cancel_bridge_mirrors_a_later_cancellation() {
        let ipc = reaper_ipc::CancelFlag::new();
        let bridge = CancelBridge::new(&ipc);
        assert!(!bridge.flag().is_cancelled());
        ipc.cancel();
        for _ in 0..100 {
            if bridge.flag().is_cancelled() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the engine flag was never mirrored");
    }

    #[test]
    fn every_registered_tool_is_dispatchable() {
        let core = ServerCore::offline();
        for name in core.tools.names() {
            let r = call(&core, name, &json_obj! {}, &CallContext::detached());
            let payload = r.get("structuredContent").unwrap();
            let code = payload.str_field("error_code").unwrap_or("");
            assert_ne!(code, codes::UNKNOWN_TOOL, "{name} is not dispatched");
        }
    }
}
