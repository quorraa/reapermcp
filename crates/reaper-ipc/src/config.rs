//! Client configuration and `config.json` loading.
//!
//! `config.json` lives next to the ReaScript — at `<script dir>/config.json`,
//! **not** inside the IPC directory. The bridge mints `instance_token` on first
//! run and writes the file; the MCP server reads the token from there and must
//! never invent one.
//!
//! The IPC directory is resolved exactly as §1 specifies: `ipc_dir` from the
//! config when it is a non-empty string, otherwise `<script dir>/ipc`. There is
//! no guessing from `GetResourcePath()` — portable REAPER installations are
//! supported, so the server must be *told* the directory.

use crate::error::{codes, IpcError};
use crate::limits;
use qjson::Json;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Everything the client needs to talk to one bridge installation.
#[derive(Clone, Debug, PartialEq)]
pub struct IpcConfig {
    /// The IPC root. The client writes only inside `<ipc_dir>/commands/` and
    /// deletes only inside `<ipc_dir>/results/`.
    pub ipc_dir: PathBuf,
    /// The installation token, compared byte for byte by the bridge.
    ///
    /// This is not a secret that protects anything; it exists so an unrelated
    /// file dropped into the IPC directory is never executed as a command.
    pub instance_token: String,
    /// How long a call may take before it fails with `IPC_TIMEOUT`.
    ///
    /// Also sets `expires_at = now + request_timeout`.
    pub request_timeout: Duration,
    /// The upper bound of the result-polling backoff.
    ///
    /// Polling starts far tighter than this and grows towards it; see
    /// [`crate::client::BridgeClient::call`].
    pub poll_interval: Duration,
    /// Local cap on the serialised request, enforced before writing.
    pub max_request_bytes: usize,
    /// Local cap on a result file, enforced after reading.
    pub max_result_bytes: usize,
}

/// The default request timeout: generous enough for `stage_candidate`, which
/// has to open an undo block and mutate the project.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The default polling cap, the top of the wire spec's 20–50 ms guidance.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(50);

impl IpcConfig {
    /// A configuration with the default timings and the protocol's size caps.
    pub fn new(ipc_dir: impl Into<PathBuf>, instance_token: impl Into<String>) -> IpcConfig {
        IpcConfig {
            ipc_dir: ipc_dir.into(),
            instance_token: instance_token.into(),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            poll_interval: DEFAULT_POLL_INTERVAL,
            max_request_bytes: limits::MAX_REQUEST_BYTES,
            max_result_bytes: limits::MAX_RESULT_BYTES,
        }
    }

    /// Replaces the request timeout.
    pub fn with_request_timeout(mut self, d: Duration) -> IpcConfig {
        self.request_timeout = d;
        self
    }

    /// Replaces the polling cap.
    pub fn with_poll_interval(mut self, d: Duration) -> IpcConfig {
        self.poll_interval = d;
        self
    }

    /// Loads `<script dir>/config.json`.
    ///
    /// `instance_token` is required and must be a non-empty string; the bridge
    /// writes one on first run, and a server that cannot find it should tell
    /// the user to start the bridge rather than mint its own.
    ///
    /// `ipc_dir` resolves relative to the config file's own directory, so a
    /// relative override in the file means what the bridge means by it.
    pub fn from_config_file(path: &Path) -> Result<IpcConfig, IpcError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| IpcError::io("could not read config.json", path, &e))?;
        let doc = Json::parse(&text).map_err(|e| {
            IpcError::with_details(
                codes::MALFORMED_REQUEST,
                format!("config.json is not valid JSON: {e}"),
                qjson::json_obj! { "path" => path.display().to_string() },
            )
        })?;
        let script_dir = path.parent().unwrap_or_else(|| Path::new("."));
        IpcConfig::from_config_json(&doc, script_dir)
    }

    /// Builds a configuration from an already-parsed `config.json` document and
    /// the directory that file lives in.
    pub fn from_config_json(doc: &Json, script_dir: &Path) -> Result<IpcConfig, IpcError> {
        if doc.as_obj().is_none() {
            return Err(IpcError::new(
                codes::MALFORMED_REQUEST,
                "config.json must contain a JSON object",
            ));
        }
        let token = match doc.get("instance_token") {
            Some(Json::Str(s)) if !s.is_empty() => s.clone(),
            _ => {
                return Err(IpcError::new(
                    codes::INVALID_INSTANCE_TOKEN,
                    "config.json has no non-empty instance_token; start the bridge once so it mints one",
                ))
            }
        };
        let ipc_dir = match doc.get("ipc_dir") {
            Some(Json::Str(s)) if !s.is_empty() => {
                let p = PathBuf::from(s);
                if p.is_absolute() {
                    p
                } else {
                    script_dir.join(p)
                }
            }
            _ => script_dir.join("ipc"),
        };
        let cfg = IpcConfig::new(ipc_dir, token);
        cfg.validate()?;
        Ok(cfg)
    }

    /// Checks the invariants the client depends on.
    ///
    /// A configuration that fails this must never be used to build paths.
    pub fn validate(&self) -> Result<(), IpcError> {
        let bad = |msg: &str| Err(IpcError::new(codes::MALFORMED_REQUEST, msg));

        if self.ipc_dir.as_os_str().is_empty() {
            return bad("ipc_dir must not be empty");
        }
        if self
            .ipc_dir
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return bad("ipc_dir must not contain a \"..\" component");
        }
        if self.instance_token.is_empty() {
            return Err(IpcError::new(
                codes::INVALID_INSTANCE_TOKEN,
                "instance_token must not be empty",
            ));
        }
        if self.instance_token.len() > limits::MAX_STRING_LEN {
            return Err(IpcError::new(
                codes::INVALID_INSTANCE_TOKEN,
                format!(
                    "instance_token must be at most {} bytes",
                    limits::MAX_STRING_LEN
                ),
            ));
        }
        if self.instance_token.bytes().any(|b| b < 0x20 || b == 0x7f) {
            return Err(IpcError::new(
                codes::INVALID_INSTANCE_TOKEN,
                "instance_token must not contain control characters",
            ));
        }
        if self.request_timeout.is_zero() {
            return bad("request_timeout must be greater than zero");
        }
        if self.poll_interval.is_zero() {
            return bad("poll_interval must be greater than zero");
        }
        if self.max_request_bytes == 0 || self.max_request_bytes > limits::MAX_REQUEST_BYTES {
            return bad("max_request_bytes must be in 1..=MAX_REQUEST_BYTES");
        }
        if self.max_result_bytes == 0 || self.max_result_bytes > limits::MAX_RESULT_BYTES {
            return bad("max_result_bytes must be in 1..=MAX_RESULT_BYTES");
        }
        Ok(())
    }

    /// `<ipc_dir>/commands` — the only directory the client writes into.
    pub fn commands_dir(&self) -> PathBuf {
        self.ipc_dir.join("commands")
    }

    /// `<ipc_dir>/results` — the only directory the client deletes from.
    pub fn results_dir(&self) -> PathBuf {
        self.ipc_dir.join("results")
    }

    /// `<ipc_dir>/heartbeat.json` — read-only.
    pub fn heartbeat_path(&self) -> PathBuf {
        self.ipc_dir.join("heartbeat.json")
    }

    /// `<ipc_dir>/bridge.lock` — read-only, diagnostics only.
    pub fn lock_path(&self) -> PathBuf {
        self.ipc_dir.join("bridge.lock")
    }

    /// The path a validated `request_id` maps to while being written.
    ///
    /// `id` must already have passed [`crate::protocol::validate_request_id`];
    /// this method re-checks rather than trusting the caller, because it is the
    /// last point before a path is built.
    pub fn command_tmp_path(&self, id: &str) -> Result<PathBuf, IpcError> {
        crate::protocol::validate_request_id(id)?;
        Ok(self
            .commands_dir()
            .join(format!("{id}{}", limits::SUFFIX_TMP)))
    }

    /// The path a validated `request_id` maps to once published.
    pub fn command_path(&self, id: &str) -> Result<PathBuf, IpcError> {
        crate::protocol::validate_request_id(id)?;
        Ok(self
            .commands_dir()
            .join(format!("{id}{}", limits::SUFFIX_COMMAND)))
    }

    /// The result path for a validated `request_id`.
    pub fn result_path(&self, id: &str) -> Result<PathBuf, IpcError> {
        crate::protocol::validate_request_id(id)?;
        Ok(self
            .results_dir()
            .join(format!("{id}{}", limits::SUFFIX_RESULT)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn recorded_config() -> Json {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mock-reaper/bridge/config.json"
        );
        let text = std::fs::read_to_string(path).expect("config fixture");
        Json::parse(&text).expect("parses")
    }

    #[test]
    fn loads_the_recorded_bridge_config() {
        let cfg = IpcConfig::from_config_json(&recorded_config(), Path::new("/scripts/QLabs"))
            .expect("load");
        assert_eq!(cfg.instance_token, TOKEN);
        // ipc_dir is null in the fixture, so it derives from the script dir.
        assert_eq!(cfg.ipc_dir, PathBuf::from("/scripts/QLabs/ipc"));
    }

    #[test]
    fn a_null_ipc_dir_derives_from_the_script_directory() {
        let doc = qjson::json_obj! { "instance_token" => TOKEN, "ipc_dir" => Json::Null };
        let cfg = IpcConfig::from_config_json(&doc, Path::new("/a/b")).expect("load");
        assert_eq!(cfg.ipc_dir, PathBuf::from("/a/b/ipc"));
    }

    #[test]
    fn an_empty_ipc_dir_string_derives_from_the_script_directory() {
        let doc = qjson::json_obj! { "instance_token" => TOKEN, "ipc_dir" => "" };
        let cfg = IpcConfig::from_config_json(&doc, Path::new("/a/b")).expect("load");
        assert_eq!(cfg.ipc_dir, PathBuf::from("/a/b/ipc"));
    }

    #[test]
    fn an_absolute_ipc_dir_override_is_used_verbatim() {
        let doc = qjson::json_obj! { "instance_token" => TOKEN, "ipc_dir" => "/var/qlabs/ipc" };
        let cfg = IpcConfig::from_config_json(&doc, Path::new("/a/b")).expect("load");
        assert_eq!(cfg.ipc_dir, PathBuf::from("/var/qlabs/ipc"));
    }

    #[test]
    fn a_relative_ipc_dir_override_resolves_against_the_script_directory() {
        let doc = qjson::json_obj! { "instance_token" => TOKEN, "ipc_dir" => "shared" };
        let cfg = IpcConfig::from_config_json(&doc, Path::new("/a/b")).expect("load");
        assert_eq!(cfg.ipc_dir, PathBuf::from("/a/b/shared"));
    }

    #[test]
    fn a_config_without_a_token_is_rejected() {
        let doc = qjson::json_obj! { "ipc_dir" => "/x" };
        let err = IpcConfig::from_config_json(&doc, Path::new("/a")).expect_err("no token");
        assert_eq!(err.code, codes::INVALID_INSTANCE_TOKEN);
    }

    #[test]
    fn a_config_with_an_empty_token_is_rejected() {
        let doc = qjson::json_obj! { "instance_token" => "" };
        let err = IpcConfig::from_config_json(&doc, Path::new("/a")).expect_err("empty token");
        assert_eq!(err.code, codes::INVALID_INSTANCE_TOKEN);
    }

    #[test]
    fn a_config_that_is_not_an_object_is_rejected() {
        let err = IpcConfig::from_config_json(&Json::Arr(vec![]), Path::new("/a"))
            .expect_err("not an object");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn from_config_file_reads_from_disk() {
        let dir = crate::fs::TempDir::new("cfg").expect("temp");
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            qjson::json_obj! { "instance_token" => TOKEN }.to_string(),
        )
        .expect("write");
        let cfg = IpcConfig::from_config_file(&path).expect("load");
        assert_eq!(cfg.instance_token, TOKEN);
        assert_eq!(cfg.ipc_dir, dir.path().join("ipc"));
    }

    #[test]
    fn from_config_file_reports_a_missing_file() {
        let err = IpcConfig::from_config_file(Path::new("/nonexistent/qlabs/config.json"))
            .expect_err("missing");
        assert_eq!(err.code, codes::IPC_IO_ERROR);
        assert_eq!(err.detail_str("os_error"), Some("not_found"));
    }

    #[test]
    fn from_config_file_reports_invalid_json() {
        let dir = crate::fs::TempDir::new("cfgbad").expect("temp");
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{ not json").expect("write");
        let err = IpcConfig::from_config_file(&path).expect_err("bad json");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn validate_accepts_the_default_configuration() {
        IpcConfig::new("/ipc", TOKEN).validate().expect("valid");
    }

    #[test]
    fn validate_rejects_an_empty_ipc_dir() {
        let cfg = IpcConfig::new("", TOKEN);
        assert_eq!(
            cfg.validate().expect_err("empty").code,
            codes::MALFORMED_REQUEST
        );
    }

    #[test]
    fn validate_rejects_a_parent_component_in_ipc_dir() {
        let cfg = IpcConfig::new("/ipc/../etc", TOKEN);
        let err = cfg.validate().expect_err("escape");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn validate_rejects_a_control_character_in_the_token() {
        let cfg = IpcConfig::new("/ipc", "abc\ndef");
        assert_eq!(
            cfg.validate().expect_err("control").code,
            codes::INVALID_INSTANCE_TOKEN
        );
    }

    #[test]
    fn validate_rejects_an_oversized_token() {
        let cfg = IpcConfig::new("/ipc", "a".repeat(limits::MAX_STRING_LEN + 1));
        assert_eq!(
            cfg.validate().expect_err("too long").code,
            codes::INVALID_INSTANCE_TOKEN
        );
    }

    #[test]
    fn validate_rejects_zero_durations() {
        let cfg = IpcConfig::new("/ipc", TOKEN).with_request_timeout(Duration::ZERO);
        assert!(cfg.validate().is_err());
        let cfg = IpcConfig::new("/ipc", TOKEN).with_poll_interval(Duration::ZERO);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_size_caps_above_the_protocol_limits() {
        let mut cfg = IpcConfig::new("/ipc", TOKEN);
        cfg.max_request_bytes = limits::MAX_REQUEST_BYTES + 1;
        assert!(cfg.validate().is_err());
        let mut cfg = IpcConfig::new("/ipc", TOKEN);
        cfg.max_result_bytes = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn derived_paths_live_under_the_ipc_root() {
        let cfg = IpcConfig::new("/ipc", TOKEN);
        assert_eq!(cfg.commands_dir(), PathBuf::from("/ipc/commands"));
        assert_eq!(cfg.results_dir(), PathBuf::from("/ipc/results"));
        assert_eq!(cfg.heartbeat_path(), PathBuf::from("/ipc/heartbeat.json"));
        assert_eq!(cfg.lock_path(), PathBuf::from("/ipc/bridge.lock"));
        assert_eq!(
            cfg.command_path("abc").expect("path"),
            PathBuf::from("/ipc/commands/abc.command.json")
        );
        assert_eq!(
            cfg.command_tmp_path("abc").expect("path"),
            PathBuf::from("/ipc/commands/abc.tmp")
        );
        assert_eq!(
            cfg.result_path("abc").expect("path"),
            PathBuf::from("/ipc/results/abc.result.json")
        );
    }

    #[test]
    fn path_construction_refuses_an_escaping_id() {
        let cfg = IpcConfig::new("/ipc", TOKEN);
        for bad in ["../../etc/passwd", "a/b", "..", ".hidden", ""] {
            assert!(cfg.command_path(bad).is_err(), "{bad:?}");
            assert!(cfg.command_tmp_path(bad).is_err(), "{bad:?}");
            assert!(cfg.result_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn builders_replace_only_what_they_name() {
        let cfg = IpcConfig::new("/ipc", TOKEN)
            .with_request_timeout(Duration::from_millis(7))
            .with_poll_interval(Duration::from_millis(3));
        assert_eq!(cfg.request_timeout, Duration::from_millis(7));
        assert_eq!(cfg.poll_interval, Duration::from_millis(3));
        assert_eq!(cfg.max_request_bytes, limits::MAX_REQUEST_BYTES);
    }
}
