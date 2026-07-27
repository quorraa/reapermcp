//! Newline-delimited JSON-RPC 2.0 over stdio.
//!
//! The transport is deliberately tiny and hand-written: one JSON value per
//! line, UTF-8, `\n`-terminated. There is no SDK, no framing header and no
//! second channel.
//!
//! # stdout purity
//!
//! [`Writer`] is the **only** thing in this crate that writes to stdout, and it
//! only ever writes [`Json::to_string`] output followed by a single `\n`. Every
//! diagnostic goes through [`crate::log`] to stderr, and
//! [`crate::log::install_panic_hook`] keeps a panicking thread's message off
//! stdout too. That invariant is asserted end to end by the
//! `stdout_contains_no_non_mcp_text` test.

use qjson::{json_obj, Json, JsonMap};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

/// The JSON-RPC version string every message carries.
pub const JSONRPC_VERSION: &str = "2.0";

/// Standard JSON-RPC error codes, plus the one MCP adds.
pub mod rpc_codes {
    /// Invalid JSON was received.
    pub const PARSE_ERROR: i64 = -32700;
    /// The JSON sent is not a valid request object.
    pub const INVALID_REQUEST: i64 = -32600;
    /// The method does not exist.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// Invalid method parameters.
    pub const INVALID_PARAMS: i64 = -32602;
    /// Internal JSON-RPC error.
    pub const INTERNAL_ERROR: i64 = -32603;
    /// The server has not been initialized yet.
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
}

/// One inbound JSON-RPC message.
#[derive(Clone, Debug, PartialEq)]
pub struct Incoming {
    /// The request id, absent for a notification.
    pub id: Option<Json>,
    /// The method name.
    pub method: String,
    /// The parameters object; `Json::Null` when absent.
    pub params: Json,
}

impl Incoming {
    /// True when this message is a notification and must not be answered.
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    /// Reads a message from a parsed JSON value.
    ///
    /// Rejects anything that is not a JSON-RPC 2.0 request or notification.
    /// A response (a value carrying `result` or `error`) is rejected too: this
    /// server never issues requests, so it can never legitimately receive one.
    pub fn from_json(v: &Json) -> Result<Incoming, RpcError> {
        let Some(obj) = v.as_obj() else {
            return Err(RpcError::new(
                rpc_codes::INVALID_REQUEST,
                "a JSON-RPC message must be an object",
            ));
        };
        match obj.get("jsonrpc").and_then(Json::as_str) {
            Some(JSONRPC_VERSION) => {}
            Some(other) => {
                return Err(RpcError::new(
                    rpc_codes::INVALID_REQUEST,
                    format!("unsupported jsonrpc version {other:?}"),
                ))
            }
            None => {
                return Err(RpcError::new(
                    rpc_codes::INVALID_REQUEST,
                    "missing the jsonrpc field",
                ))
            }
        }
        let Some(method) = obj.get("method").and_then(Json::as_str) else {
            return Err(RpcError::new(
                rpc_codes::INVALID_REQUEST,
                "missing the method field",
            ));
        };
        let id = match obj.get("id") {
            None | Some(Json::Null) => None,
            Some(v @ (Json::Str(_) | Json::Int(_))) => Some(v.clone()),
            Some(other) => {
                return Err(RpcError::new(
                    rpc_codes::INVALID_REQUEST,
                    format!("id must be a string or an integer, not {}", other.type_name()),
                ))
            }
        };
        let params = match obj.get("params") {
            None | Some(Json::Null) => Json::Null,
            Some(v @ Json::Obj(_)) => v.clone(),
            Some(other) => {
                return Err(RpcError::new(
                    rpc_codes::INVALID_PARAMS,
                    format!("params must be an object, not {}", other.type_name()),
                ))
            }
        };
        Ok(Incoming {
            id,
            method: method.to_string(),
            params,
        })
    }
}

/// A JSON-RPC error object.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    /// The numeric code.
    pub code: i64,
    /// The human-readable message.
    pub message: String,
    /// Optional structured data.
    pub data: Option<Json>,
}

impl RpcError {
    /// An error with no `data`.
    pub fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// An error carrying structured `data`.
    pub fn with_data(code: i64, message: impl Into<String>, data: Json) -> RpcError {
        RpcError {
            code,
            message: message.into(),
            data: Some(data),
        }
    }

    /// The wire form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("code", Json::Int(self.code));
        m.insert("message", Json::Str(self.message.clone()));
        if let Some(d) = &self.data {
            m.insert("data", d.clone());
        }
        Json::Obj(m)
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

/// Builds a successful response envelope.
pub fn response(id: &Json, result: Json) -> Json {
    json_obj! {
        "jsonrpc" => JSONRPC_VERSION,
        "id" => id.clone(),
        "result" => result,
    }
}

/// Builds an error response envelope.
pub fn error_response(id: Option<&Json>, err: &RpcError) -> Json {
    json_obj! {
        "jsonrpc" => JSONRPC_VERSION,
        "id" => id.cloned().unwrap_or(Json::Null),
        "error" => err.to_json(),
    }
}

/// Builds a notification envelope.
pub fn notification(method: &str, params: Json) -> Json {
    json_obj! {
        "jsonrpc" => JSONRPC_VERSION,
        "method" => method,
        "params" => params,
    }
}

/// The single sink for protocol bytes.
///
/// Cloneable and `Send`, so a tool running on the request thread can emit
/// `notifications/progress` through the same lock the dispatcher writes
/// responses through, and the two can never interleave inside one line.
#[derive(Clone)]
pub struct Writer {
    inner: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writer").finish_non_exhaustive()
    }
}

impl Writer {
    /// Wraps any writer as the protocol sink.
    pub fn new(w: Box<dyn Write + Send>) -> Writer {
        Writer {
            inner: Arc::new(Mutex::new(w)),
        }
    }

    /// The stdout sink used in production.
    pub fn stdout() -> Writer {
        Writer::new(Box::new(std::io::stdout()))
    }

    /// Writes one message as a single line and flushes.
    ///
    /// A write failure is reported to stderr and swallowed: a broken pipe means
    /// the client has gone, and the loop's own EOF handling shuts the server
    /// down cleanly a moment later.
    pub fn send(&self, message: &Json) {
        let mut line = message.to_string();
        line.push('\n');
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Err(e) = guard.write_all(line.as_bytes()) {
            crate::log::warn(&format!("could not write to stdout: {e}"));
            return;
        }
        if let Err(e) = guard.flush() {
            crate::log::warn(&format!("could not flush stdout: {e}"));
        }
    }
}

/// Reads newline-delimited JSON values from `reader`.
///
/// Yields `None` at EOF. A line that is empty or whitespace-only is skipped,
/// which keeps a client that pads its stream from tripping a parse error.
pub fn read_message<R: BufRead>(reader: &mut R) -> Option<Result<Json, RpcError>> {
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return None,
            Ok(_) => {}
            Err(e) => {
                return Some(Err(RpcError::new(
                    rpc_codes::PARSE_ERROR,
                    format!("could not read from stdin: {e}"),
                )))
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        return Some(Json::parse(trimmed).map_err(|e| {
            RpcError::new(rpc_codes::PARSE_ERROR, format!("could not parse JSON: {e}"))
        }));
    }
}

/// Test-only helpers shared across this crate's module tests.
#[cfg(test)]
pub mod tests_support {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    /// A `Write` that records everything, shareable across a test.
    #[derive(Clone, Default)]
    pub struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Sink {
        /// Everything written so far, as text.
        pub fn text(&self) -> String {
            String::from_utf8(self.0.lock().expect("sink lock").clone()).expect("utf-8")
        }

        /// Every line written so far, parsed as JSON.
        pub fn messages(&self) -> Vec<qjson::Json> {
            self.text()
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| qjson::Json::parse(l).expect("every stdout line must be JSON"))
                .collect()
        }
    }

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("sink lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_a_request() {
        let v = Json::parse(r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#).unwrap();
        let m = Incoming::from_json(&v).unwrap();
        assert_eq!(m.method, "ping");
        assert_eq!(m.id, Some(Json::Int(1)));
        assert!(!m.is_notification());
    }

    #[test]
    fn parses_a_notification() {
        let v = Json::parse(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap();
        let m = Incoming::from_json(&v).unwrap();
        assert!(m.is_notification());
        assert_eq!(m.params, Json::Null);
    }

    #[test]
    fn rejects_a_bad_version() {
        let v = Json::parse(r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#).unwrap();
        let e = Incoming::from_json(&v).unwrap_err();
        assert_eq!(e.code, rpc_codes::INVALID_REQUEST);
    }

    #[test]
    fn rejects_a_non_object_message() {
        let e = Incoming::from_json(&Json::Arr(vec![])).unwrap_err();
        assert_eq!(e.code, rpc_codes::INVALID_REQUEST);
    }

    #[test]
    fn rejects_array_params() {
        let v = Json::parse(r#"{"jsonrpc":"2.0","id":1,"method":"x","params":[1]}"#).unwrap();
        let e = Incoming::from_json(&v).unwrap_err();
        assert_eq!(e.code, rpc_codes::INVALID_PARAMS);
    }

    #[test]
    fn rejects_a_float_id() {
        let v = Json::parse(r#"{"jsonrpc":"2.0","id":1.5,"method":"x"}"#).unwrap();
        assert!(Incoming::from_json(&v).is_err());
    }

    #[test]
    fn reads_and_skips_blank_lines() {
        let mut c = Cursor::new("\n\n{\"jsonrpc\":\"2.0\",\"method\":\"a\"}\n");
        let v = read_message(&mut c).unwrap().unwrap();
        assert_eq!(v.str_field("method").unwrap(), "a");
        assert!(read_message(&mut c).is_none());
    }

    #[test]
    fn writer_emits_one_line_per_message() {
        let sink = tests_support::Sink::default();
        let w = Writer::new(Box::new(sink.clone()));
        w.send(&json_obj! { "a" => 1 });
        w.send(&json_obj! { "b" => 2 });
        let text = sink.text();
        assert_eq!(text.lines().count(), 2);
        for line in text.lines() {
            assert!(Json::parse(line).is_ok(), "{line}");
        }
        assert_eq!(sink.messages().len(), 2);
    }

    #[test]
    fn error_response_uses_null_id_when_unknown() {
        let v = error_response(None, &RpcError::new(rpc_codes::PARSE_ERROR, "bad"));
        assert_eq!(v.get("id"), Some(&Json::Null));
    }
}
