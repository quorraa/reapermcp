//! The MCP server: shared state, the dispatch loop, and cancellation.
//!
//! The loop is synchronous by design — there is no async runtime in this
//! workspace — but stdin is drained on its own thread so that a
//! `notifications/cancelled` arriving *during* a long generation takes effect
//! immediately instead of after it. The reader thread does exactly two things:
//! set a cancellation flag, and hand the message to the dispatcher. Everything
//! else happens on the main thread, in arrival order.
//!
//! # Lifecycle
//!
//! ```text
//! initialize          -> protocolVersion 2025-11-25 + capabilities
//! notifications/initialized
//! tools/list | tools/call | resources/* | prompts/* | ping | logging/setLevel
//! notifications/cancelled  (any time, handled out of band)
//! EOF                 -> graceful shutdown, exit 0
//! ```

use crate::bridge::Bridge;
use crate::config::ServerConfig;
use crate::error::ToolError;
use crate::rpc::{self, rpc_codes, Incoming, RpcError, Writer};
use crate::schema_gen::ToolRegistry;
use crate::store::SessionStore;
use crate::{MCP_PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION};
use qjson::uuid::UuidGen;
use qjson::{json_obj, Json};
use reaper_ipc::CancelFlag;
use std::collections::BTreeMap;
use std::io::BufRead;
use std::sync::{Arc, Mutex};
use theory_kb::KnowledgeBase;

/// Which knowledge bundle the server is serving.
///
/// The embedded bundle is a `&'static` produced once behind a `OnceLock`; an
/// external `--knowledge-dir` is owned. Wrapping both in one enum keeps every
/// call site free of that distinction at zero runtime cost.
#[derive(Debug)]
pub enum Knowledge {
    /// The bundle compiled into the binary.
    Embedded,
    /// A bundle loaded from disk and fully validated.
    External(Box<KnowledgeBase>),
}

impl Knowledge {
    /// The bundle.
    pub fn kb(&self) -> &KnowledgeBase {
        match self {
            Knowledge::Embedded => KnowledgeBase::embedded(),
            Knowledge::External(k) => k,
        }
    }

    /// Where the bundle came from, for `doctor` and `reaper.status`.
    pub fn origin(&self) -> &str {
        match self {
            Knowledge::Embedded => "embedded",
            Knowledge::External(k) => k.origin(),
        }
    }
}

/// State shared by every request.
#[derive(Debug)]
pub struct ServerCore {
    /// The knowledge bundle.
    pub knowledge: Knowledge,
    /// How the server was configured.
    pub config: ServerConfig,
    /// The REAPER bridge, which may be permanently offline.
    pub bridge: Bridge,
    /// Every tool, with compiled schemas.
    pub tools: ToolRegistry,
    /// The bounded, TTL'd session store.
    pub store: Mutex<SessionStore>,
    /// The id generator for transactions and plans.
    pub ids: Mutex<UuidGen>,
    /// When the process started, in Unix seconds.
    pub started_at: i64,
}

impl ServerCore {
    /// Builds the shared state.
    pub fn new(config: ServerConfig, bridge: Bridge, knowledge: Knowledge) -> Arc<ServerCore> {
        Arc::new(ServerCore {
            knowledge,
            config,
            bridge,
            tools: ToolRegistry::compile().expect("the declared tool schemas must compile"),
            store: Mutex::new(SessionStore::new()),
            ids: Mutex::new(UuidGen::process_unique()),
            started_at: qjson::time::unix_now(),
        })
    }

    /// A REAPER-free core with the embedded bundle: what the fixture commands
    /// and most tests run against.
    pub fn offline() -> Arc<ServerCore> {
        let config = ServerConfig::unconfigured();
        let bridge = Bridge::new(&config, None);
        ServerCore::new(config, bridge, Knowledge::Embedded)
    }

    /// Mints a fresh server-issued identifier.
    pub fn next_id(&self) -> String {
        match self.ids.lock() {
            Ok(g) => g.next(),
            Err(p) => p.into_inner().next(),
        }
    }

    /// Runs `f` against the session store.
    pub fn with_store<T>(&self, f: impl FnOnce(&mut SessionStore) -> T) -> T {
        let mut guard = match self.store.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        f(&mut guard)
    }

    /// The knowledge bundle's content hash.
    pub fn knowledge_hash(&self) -> String {
        self.knowledge.kb().content_hash().to_string()
    }

    /// The knowledge bundle's version.
    pub fn knowledge_version(&self) -> String {
        self.knowledge.kb().version().to_string()
    }
}

/// What a running tool is allowed to know about its call.
pub struct CallContext {
    /// Set when the client cancels this request.
    pub cancel: CancelFlag,
    /// The client's progress token, when it supplied one.
    pub progress_token: Option<Json>,
    /// The protocol sink, for `notifications/progress`.
    pub writer: Option<Writer>,
    /// The wall clock this call is evaluated against.
    pub now: i64,
}

impl std::fmt::Debug for CallContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallContext")
            .field("cancelled", &self.cancel.is_cancelled())
            .field("has_progress_token", &self.progress_token.is_some())
            .finish()
    }
}

impl Default for CallContext {
    fn default() -> Self {
        CallContext {
            cancel: CancelFlag::new(),
            progress_token: None,
            writer: None,
            now: qjson::time::unix_now(),
        }
    }
}

impl CallContext {
    /// A context with no client attached, for the CLI and for tests.
    pub fn detached() -> CallContext {
        CallContext::default()
    }

    /// Emits `notifications/progress`, if the client asked for progress.
    pub fn report(&self, fraction: f64, message: &str) {
        let (Some(token), Some(writer)) = (&self.progress_token, &self.writer) else {
            return;
        };
        writer.send(&rpc::notification(
            "notifications/progress",
            json_obj! {
                "progressToken" => token.clone(),
                "progress" => fraction.clamp(0.0, 1.0),
                "total" => 1.0,
                "message" => message,
            },
        ));
    }

    /// `Err(CANCELLED)` when the client has cancelled the call.
    pub fn check_cancelled(&self) -> Result<(), ToolError> {
        if self.cancel.is_cancelled() {
            Err(ToolError::new(
                crate::error::codes::CANCELLED,
                "the client cancelled this request",
            ))
        } else {
            Ok(())
        }
    }
}

/// The dispatcher.
pub struct Server {
    /// Shared state.
    pub core: Arc<ServerCore>,
    writer: Writer,
    initialized: bool,
    cancels: Arc<Mutex<BTreeMap<String, CancelFlag>>>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("initialized", &self.initialized)
            .finish_non_exhaustive()
    }
}

/// Renders a JSON-RPC id as the key the cancellation registry uses.
fn id_key(id: &Json) -> String {
    match id {
        Json::Str(s) => format!("s:{s}"),
        Json::Int(i) => format!("i:{i}"),
        other => format!("o:{}", other.to_string()),
    }
}

impl Server {
    /// Builds a dispatcher over `core`, writing to `writer`.
    pub fn new(core: Arc<ServerCore>, writer: Writer) -> Server {
        Server {
            core,
            writer,
            initialized: false,
            cancels: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// True once `initialize` has been answered.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// The shared cancellation registry, so a reader thread can flip a flag.
    pub fn cancels(&self) -> Arc<Mutex<BTreeMap<String, CancelFlag>>> {
        Arc::clone(&self.cancels)
    }

    /// Marks a request cancelled. Returns true when the id was in flight.
    pub fn cancel_request(registry: &Mutex<BTreeMap<String, CancelFlag>>, id: &Json) -> bool {
        let key = id_key(id);
        let mut guard = match registry.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        match guard.get(&key) {
            Some(flag) => {
                flag.cancel();
                true
            }
            None => {
                // Register the flag anyway: a cancellation that overtakes its
                // request must still be honoured when the request arrives.
                let flag = CancelFlag::new();
                flag.cancel();
                guard.insert(key, flag);
                false
            }
        }
    }

    fn take_flag(&self, id: &Json) -> CancelFlag {
        let key = id_key(id);
        let mut guard = match self.cancels.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        match guard.get(&key) {
            Some(existing) => existing.clone(),
            None => {
                let flag = CancelFlag::new();
                guard.insert(key, flag.clone());
                flag
            }
        }
    }

    fn release_flag(&self, id: &Json) {
        let key = id_key(id);
        let mut guard = match self.cancels.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.remove(&key);
    }

    /// Handles one parsed message, returning the response to send, if any.
    pub fn handle(&mut self, message: &Json) -> Option<Json> {
        let incoming = match Incoming::from_json(message) {
            Ok(m) => m,
            Err(e) => {
                let id = message.get("id").filter(|v| !v.is_null()).cloned();
                return Some(rpc::error_response(id.as_ref(), &e));
            }
        };
        self.handle_incoming(&incoming)
    }

    /// Handles one already-validated message.
    pub fn handle_incoming(&mut self, incoming: &Incoming) -> Option<Json> {
        crate::log::debug(&format!("<- {}", incoming.method));

        if incoming.is_notification() {
            self.handle_notification(incoming);
            return None;
        }
        let id = incoming.id.clone().unwrap_or(Json::Null);

        if !self.initialized && !matches!(incoming.method.as_str(), "initialize" | "ping") {
            return Some(rpc::error_response(
                Some(&id),
                &RpcError::new(
                    rpc_codes::SERVER_NOT_INITIALIZED,
                    "the client must send initialize before any other request",
                ),
            ));
        }

        let result = match incoming.method.as_str() {
            "initialize" => self.initialize(&incoming.params),
            "ping" => Ok(Json::Obj(Default::default())),
            "tools/list" => Ok(json_obj! { "tools" => self.core.tools.list_json() }),
            "tools/call" => self.tools_call(&id, &incoming.params),
            "resources/list" => Ok(json_obj! {
                "resources" => crate::resources::list_json(),
            }),
            "resources/templates/list" => Ok(json_obj! {
                "resourceTemplates" => crate::resources::templates_json(),
            }),
            "resources/read" => crate::resources::read_request(&self.core, &incoming.params),
            "prompts/list" => Ok(json_obj! { "prompts" => crate::prompts::list_json() }),
            "prompts/get" => crate::prompts::get_request(&incoming.params),
            "logging/setLevel" => self.set_level(&incoming.params),
            other => Err(RpcError::new(
                rpc_codes::METHOD_NOT_FOUND,
                format!("unknown method {other:?}"),
            )),
        };

        Some(match result {
            Ok(value) => rpc::response(&id, value),
            Err(e) => rpc::error_response(Some(&id), &e),
        })
    }

    fn handle_notification(&mut self, incoming: &Incoming) {
        match incoming.method.as_str() {
            "notifications/initialized" => {
                crate::log::info("client reported initialized");
            }
            "notifications/cancelled" => {
                if let Some(id) = incoming.params.get("requestId") {
                    let known = Server::cancel_request(&self.cancels, id);
                    crate::log::info(&format!(
                        "cancellation for request {} ({})",
                        id.to_string(),
                        if known { "in flight" } else { "not in flight" }
                    ));
                }
            }
            other => {
                crate::log::debug(&format!("ignoring unknown notification {other}"));
            }
        }
    }

    fn initialize(&mut self, params: &Json) -> Result<Json, RpcError> {
        if let Some(v) = params.get("protocolVersion").and_then(Json::as_str) {
            if v != MCP_PROTOCOL_VERSION {
                crate::log::warn(&format!(
                    "client asked for protocol {v}; this server implements {MCP_PROTOCOL_VERSION}"
                ));
            }
        }
        self.initialized = true;
        Ok(json_obj! {
            "protocolVersion" => MCP_PROTOCOL_VERSION,
            "capabilities" => json_obj! {
                "tools" => json_obj! { "listChanged" => false },
                "resources" => json_obj! { "subscribe" => false, "listChanged" => false },
                "prompts" => json_obj! { "listChanged" => false },
                "logging" => Json::Obj(Default::default()),
            },
            "serverInfo" => json_obj! {
                "name" => SERVER_NAME,
                "title" => "QLabs REAPER Music Intelligence",
                "version" => SERVER_VERSION,
            },
            "instructions" => INSTRUCTIONS,
        })
    }

    fn set_level(&mut self, params: &Json) -> Result<Json, RpcError> {
        let level = params
            .get("level")
            .and_then(Json::as_str)
            .and_then(map_syslog_level)
            .ok_or_else(|| {
                RpcError::new(
                    rpc_codes::INVALID_PARAMS,
                    "level must be one of the MCP logging levels",
                )
            })?;
        crate::log::set_level(level);
        Ok(Json::Obj(Default::default()))
    }

    fn tools_call(&mut self, id: &Json, params: &Json) -> Result<Json, RpcError> {
        let name = params
            .get("name")
            .and_then(Json::as_str)
            .ok_or_else(|| RpcError::new(rpc_codes::INVALID_PARAMS, "tools/call needs a name"))?
            .to_string();
        let arguments = match params.get("arguments") {
            None | Some(Json::Null) => Json::Obj(Default::default()),
            Some(v @ Json::Obj(_)) => v.clone(),
            Some(other) => {
                return Err(RpcError::new(
                    rpc_codes::INVALID_PARAMS,
                    format!("arguments must be an object, not {}", other.type_name()),
                ))
            }
        };
        let progress_token = params
            .get("_meta")
            .and_then(|m| m.get("progressToken"))
            .cloned();

        let ctx = CallContext {
            cancel: self.take_flag(id),
            progress_token,
            writer: Some(self.writer.clone()),
            now: qjson::time::unix_now(),
        };
        let outcome = crate::tools::call(&self.core, &name, &arguments, &ctx);
        self.release_flag(id);
        Ok(outcome)
    }

    /// Runs the loop until EOF, then returns.
    ///
    /// `reader` is drained on a dedicated thread so `notifications/cancelled`
    /// is honoured while a tool is still running.
    pub fn serve<R: BufRead + Send + 'static>(mut self, mut reader: R) {
        let (tx, rx) = std::sync::mpsc::channel::<Json>();
        let cancels = self.cancels();
        let reader_thread = std::thread::spawn(move || {
            while let Some(next) = rpc::read_message(&mut reader) {
                match next {
                    Ok(value) => {
                        // Out of band: flip the flag before queueing, so a long
                        // call already in progress sees it.
                        if value.get("method").and_then(Json::as_str)
                            == Some("notifications/cancelled")
                        {
                            if let Some(rid) = value.get("params").and_then(|p| p.get("requestId")) {
                                Server::cancel_request(&cancels, rid);
                            }
                        }
                        if tx.send(value).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        if tx
                            .send(json_obj! { "__parse_error" => e.message.clone() })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
        });

        crate::log::info(&format!(
            "serving MCP {MCP_PROTOCOL_VERSION} over stdio ({} tools)",
            self.core.tools.all().len()
        ));

        for message in rx {
            if let Some(problem) = message.get("__parse_error").and_then(Json::as_str) {
                self.writer.send(&rpc::error_response(
                    None,
                    &RpcError::new(rpc_codes::PARSE_ERROR, problem),
                ));
                continue;
            }
            if let Some(response) = self.handle(&message) {
                self.writer.send(&response);
            }
            self.core.with_store(|s| s.sweep(qjson::time::unix_now()));
        }

        crate::log::info("stdin closed; shutting down");
        let _ = reader_thread.join();
    }
}

/// Maps an MCP/syslog logging level onto this crate's own levels.
fn map_syslog_level(name: &str) -> Option<crate::log::Level> {
    match name {
        "debug" => Some(crate::log::Level::Debug),
        "info" | "notice" => Some(crate::log::Level::Info),
        "warning" => Some(crate::log::Level::Warn),
        "error" | "critical" | "alert" | "emergency" => Some(crate::log::Level::Error),
        _ => None,
    }
}

/// The `instructions` string returned by `initialize`.
pub const INSTRUCTIONS: &str = "\
QLabs REAPER Music Intelligence. Work in this order: reaper.status to confirm the bridge is \
running, reaper.inspect_selection to snapshot the selected MIDI, music.analyze_selection to read \
it, then harmony.generate_candidates for several genuinely different harmonizations. Use \
candidate.explain to compare them and loop.audit to check a loop point. Stage only the candidate \
the user chose, with reaper.stage_candidate; staging is additive and muted, and the source item \
is never modified. Commit or discard explicitly. Every id you pass back must be one this server \
issued.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::tests_support::Sink;

    fn server() -> (Server, Sink) {
        let sink = Sink::default();
        let w = Writer::new(Box::new(sink.clone()));
        (Server::new(ServerCore::offline(), w), sink)
    }

    fn request(id: i64, method: &str, params: Json) -> Json {
        json_obj! {
            "jsonrpc" => "2.0",
            "id" => id,
            "method" => method,
            "params" => params,
        }
    }

    fn init(s: &mut Server) -> Json {
        s.handle(&request(
            1,
            "initialize",
            json_obj! { "protocolVersion" => MCP_PROTOCOL_VERSION },
        ))
        .expect("a response")
    }

    #[test]
    fn initialize_reports_the_protocol_and_capabilities() {
        let (mut s, _) = server();
        let r = init(&mut s);
        let result = r.get("result").expect("result");
        assert_eq!(
            result.str_field("protocolVersion").unwrap(),
            MCP_PROTOCOL_VERSION
        );
        let caps = result.get("capabilities").unwrap();
        for k in ["tools", "resources", "prompts"] {
            assert!(caps.get(k).is_some(), "{k} capability");
        }
        assert_eq!(
            caps.get("tools").unwrap().get("listChanged"),
            Some(&Json::Bool(false))
        );
        assert_eq!(
            result.get("serverInfo").unwrap().str_field("name").unwrap(),
            SERVER_NAME
        );
        assert!(s.is_initialized());
    }

    #[test]
    fn requests_before_initialize_are_refused() {
        let (mut s, _) = server();
        let r = s.handle(&request(1, "tools/list", Json::Null)).unwrap();
        assert_eq!(
            r.get("error").unwrap().i64_field("code"),
            Ok(rpc_codes::SERVER_NOT_INITIALIZED)
        );
    }

    #[test]
    fn ping_works_before_initialize() {
        let (mut s, _) = server();
        let r = s.handle(&request(1, "ping", Json::Null)).unwrap();
        assert!(r.get("result").is_some());
    }

    #[test]
    fn tools_list_returns_every_tool() {
        let (mut s, _) = server();
        init(&mut s);
        let r = s.handle(&request(2, "tools/list", Json::Null)).unwrap();
        let tools = r.get("result").unwrap().arr_field("tools").unwrap();
        assert_eq!(tools.len(), 14);
    }

    #[test]
    fn an_unknown_method_is_method_not_found() {
        let (mut s, _) = server();
        init(&mut s);
        let r = s.handle(&request(2, "nope/nope", Json::Null)).unwrap();
        assert_eq!(
            r.get("error").unwrap().i64_field("code"),
            Ok(rpc_codes::METHOD_NOT_FOUND)
        );
    }

    #[test]
    fn notifications_get_no_response() {
        let (mut s, _) = server();
        init(&mut s);
        let n = json_obj! { "jsonrpc" => "2.0", "method" => "notifications/initialized" };
        assert!(s.handle(&n).is_none());
    }

    #[test]
    fn a_malformed_message_gets_an_invalid_request_error() {
        let (mut s, _) = server();
        let r = s
            .handle(&json_obj! { "id" => 4, "method" => "ping" })
            .unwrap();
        assert_eq!(
            r.get("error").unwrap().i64_field("code"),
            Ok(rpc_codes::INVALID_REQUEST)
        );
        assert_eq!(r.get("id"), Some(&Json::Int(4)));
    }

    #[test]
    fn cancellation_flips_the_flag_even_when_it_arrives_first() {
        let (mut s, _) = server();
        let registry = s.cancels();
        assert!(!Server::cancel_request(&registry, &Json::Int(9)));
        let flag = s.take_flag(&Json::Int(9));
        assert!(flag.is_cancelled());
        let _ = &mut s;
    }

    #[test]
    fn cancelling_an_in_flight_request_is_reported_as_known() {
        let (s, _) = server();
        let flag = s.take_flag(&Json::Int(3));
        assert!(!flag.is_cancelled());
        assert!(Server::cancel_request(&s.cancels, &Json::Int(3)));
        assert!(flag.is_cancelled());
    }

    #[test]
    fn a_cancelled_context_reports_cancelled() {
        let ctx = CallContext::detached();
        assert!(ctx.check_cancelled().is_ok());
        ctx.cancel.cancel();
        assert_eq!(
            ctx.check_cancelled().unwrap_err().code,
            crate::error::codes::CANCELLED
        );
    }

    #[test]
    fn progress_is_only_emitted_when_a_token_was_supplied() {
        let sink = Sink::default();
        let w = Writer::new(Box::new(sink.clone()));
        let silent = CallContext {
            writer: Some(w.clone()),
            ..CallContext::detached()
        };
        silent.report(0.5, "half");
        assert!(sink.text().is_empty());

        let loud = CallContext {
            writer: Some(w),
            progress_token: Some(Json::Str("tok".into())),
            ..CallContext::detached()
        };
        loud.report(0.5, "half");
        let messages = sink.messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].str_field("method").unwrap(),
            "notifications/progress"
        );
    }

    #[test]
    fn logging_set_level_is_accepted_and_validated() {
        let (mut s, _) = server();
        init(&mut s);
        let before = crate::log::level();
        let r = s
            .handle(&request(
                2,
                "logging/setLevel",
                json_obj! { "level" => "error" },
            ))
            .unwrap();
        assert!(r.get("result").is_some());
        let bad = s
            .handle(&request(
                3,
                "logging/setLevel",
                json_obj! { "level" => "loud" },
            ))
            .unwrap();
        assert_eq!(
            bad.get("error").unwrap().i64_field("code"),
            Ok(rpc_codes::INVALID_PARAMS)
        );
        crate::log::set_level(before);
    }

    #[test]
    fn tools_call_without_a_name_is_invalid_params() {
        let (mut s, _) = server();
        init(&mut s);
        let r = s.handle(&request(2, "tools/call", json_obj! {})).unwrap();
        assert_eq!(
            r.get("error").unwrap().i64_field("code"),
            Ok(rpc_codes::INVALID_PARAMS)
        );
    }

    #[test]
    fn tools_call_with_array_arguments_is_invalid_params() {
        let (mut s, _) = server();
        init(&mut s);
        let r = s
            .handle(&request(
                2,
                "tools/call",
                json_obj! { "name" => "reaper.status", "arguments" => Json::Arr(vec![]) },
            ))
            .unwrap();
        assert!(r.get("error").is_some());
    }

    #[test]
    fn the_knowledge_bundle_is_reachable() {
        let core = ServerCore::offline();
        assert_eq!(core.knowledge.origin(), "embedded");
        assert!(!core.knowledge_version().is_empty());
        assert_eq!(core.knowledge_hash().len(), 64);
    }

    #[test]
    fn ids_are_unique_and_pattern_safe() {
        let core = ServerCore::offline();
        let a = core.next_id();
        let b = core.next_id();
        assert_ne!(a, b);
        for id in [a, b] {
            assert!(reaper_ipc::protocol::is_safe_id(&id, 128), "{id}");
        }
    }

    #[test]
    fn instructions_name_the_workflow() {
        assert!(INSTRUCTIONS.contains("reaper.inspect_selection"));
        assert!(INSTRUCTIONS.contains("reaper.stage_candidate"));
    }

    #[test]
    fn syslog_levels_map_onto_our_own() {
        assert_eq!(map_syslog_level("debug"), Some(crate::log::Level::Debug));
        assert_eq!(map_syslog_level("emergency"), Some(crate::log::Level::Error));
        assert_eq!(map_syslog_level("shouty"), None);
    }
}
