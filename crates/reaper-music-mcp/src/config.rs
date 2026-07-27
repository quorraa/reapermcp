//! Server configuration and how it is resolved.
//!
//! Two things must be discovered before the server can talk to REAPER: the IPC
//! directory and the installation token. Per `IPC_WIRE.md` §1 the server
//! **never guesses** the directory — a portable REAPER install can put it
//! anywhere — so it is taken from an explicit flag, an explicit environment
//! variable, or the bridge's own `config.json`, and from nowhere else.
//!
//! Resolution order, first match wins:
//!
//! 1. `--config <path>` — the bridge's `config.json`; `ipc_dir` and
//!    `instance_token` both come from it.
//! 2. `--ipc-dir <path>` — the directory itself; the token is read from
//!    `<ipc-dir>/../config.json` when that file exists.
//! 3. `QLABS_MCP_CONFIG` — as `--config`.
//! 4. `QLABS_MCP_IPC_DIR` — as `--ipc-dir`.
//! 5. Nothing. The server still starts, every REAPER-free tool still works, and
//!    the bridge tools return `BRIDGE_OFFLINE` with a remedy naming the flag.
//!
//! A `--ipc-dir` given alongside `--config` overrides the file's own value, so
//! a user can point a stock installation at a scratch directory without editing
//! the bridge's configuration.

use qjson::{json_obj, Json};
use reaper_ipc::{IpcConfig, IpcError};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Environment variable naming the bridge's `config.json`.
pub const ENV_CONFIG: &str = "QLABS_MCP_CONFIG";
/// Environment variable naming the IPC directory.
pub const ENV_IPC_DIR: &str = "QLABS_MCP_IPC_DIR";
/// Environment variable naming an external knowledge directory.
pub const ENV_KNOWLEDGE_DIR: &str = "QLABS_MCP_KNOWLEDGE_DIR";

/// Default per-request timeout for read-only bridge commands.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Default per-request timeout for the write commands.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Where the bridge configuration came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    /// A `config.json` at this path.
    ConfigFile(PathBuf),
    /// An IPC directory given directly, with the config file that supplied the
    /// token when there was one.
    IpcDir {
        /// The directory.
        dir: PathBuf,
        /// The config file the token came from, when one was found.
        config: Option<PathBuf>,
    },
    /// Nothing was configured.
    Unconfigured,
}

impl ConfigSource {
    /// A one-line human description.
    pub fn describe(&self) -> String {
        match self {
            ConfigSource::ConfigFile(p) => format!("config file {}", p.display()),
            ConfigSource::IpcDir { dir, config: None } => {
                format!("ipc directory {} (no config.json found)", dir.display())
            }
            ConfigSource::IpcDir {
                dir,
                config: Some(c),
            } => format!(
                "ipc directory {} with token from {}",
                dir.display(),
                c.display()
            ),
            ConfigSource::Unconfigured => "not configured".to_string(),
        }
    }
}

/// How the server was told to run.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// The bridge client configuration, when one could be resolved.
    pub ipc: Option<IpcConfig>,
    /// Why there is no bridge configuration, when there is not one.
    pub ipc_problem: Option<IpcError>,
    /// Where the configuration came from.
    pub source: ConfigSource,
    /// An external knowledge directory, when one was requested.
    pub knowledge_dir: Option<PathBuf>,
}

impl ServerConfig {
    /// An unconfigured server: no bridge, embedded knowledge.
    pub fn unconfigured() -> ServerConfig {
        ServerConfig {
            ipc: None,
            ipc_problem: None,
            source: ConfigSource::Unconfigured,
            knowledge_dir: None,
        }
    }

    /// Resolves the configuration from explicit paths and the environment.
    ///
    /// Never fails: a problem becomes [`ServerConfig::ipc_problem`] so the
    /// server can still serve every REAPER-free tool and `doctor` can still
    /// explain what is wrong.
    pub fn resolve(
        config_path: Option<&Path>,
        ipc_dir: Option<&Path>,
        knowledge_dir: Option<&Path>,
    ) -> ServerConfig {
        let env_config = std::env::var(ENV_CONFIG).ok().map(PathBuf::from);
        let env_ipc = std::env::var(ENV_IPC_DIR).ok().map(PathBuf::from);
        let env_knowledge = std::env::var(ENV_KNOWLEDGE_DIR).ok().map(PathBuf::from);

        let config_path = config_path.map(Path::to_path_buf).or(env_config);
        let ipc_dir = ipc_dir.map(Path::to_path_buf).or(env_ipc);
        let knowledge_dir = knowledge_dir.map(Path::to_path_buf).or(env_knowledge);

        let (ipc, ipc_problem, source) = match (&config_path, &ipc_dir) {
            (Some(cfg), over) => match IpcConfig::from_config_file(cfg) {
                Ok(mut c) => {
                    if let Some(d) = over {
                        c.ipc_dir = d.clone();
                    }
                    match c.validate() {
                        Ok(()) => (Some(c), None, ConfigSource::ConfigFile(cfg.clone())),
                        Err(e) => (None, Some(e), ConfigSource::ConfigFile(cfg.clone())),
                    }
                }
                Err(e) => (None, Some(e), ConfigSource::ConfigFile(cfg.clone())),
            },
            (None, Some(dir)) => {
                let sibling = dir.parent().map(|p| p.join("config.json"));
                let found = sibling.filter(|p| p.exists());
                match &found {
                    Some(p) => match IpcConfig::from_config_file(p) {
                        Ok(mut c) => {
                            c.ipc_dir = dir.clone();
                            match c.validate() {
                                Ok(()) => (
                                    Some(c),
                                    None,
                                    ConfigSource::IpcDir {
                                        dir: dir.clone(),
                                        config: found.clone(),
                                    },
                                ),
                                Err(e) => (
                                    None,
                                    Some(e),
                                    ConfigSource::IpcDir {
                                        dir: dir.clone(),
                                        config: found.clone(),
                                    },
                                ),
                            }
                        }
                        Err(e) => (
                            None,
                            Some(e),
                            ConfigSource::IpcDir {
                                dir: dir.clone(),
                                config: found.clone(),
                            },
                        ),
                    },
                    None => (
                        None,
                        Some(IpcError::new(
                            reaper_ipc::codes::INVALID_INSTANCE_TOKEN,
                            format!(
                                "no config.json next to {}; start the bridge once so it mints an \
                                 instance token, or pass --config",
                                dir.display()
                            ),
                        )),
                        ConfigSource::IpcDir {
                            dir: dir.clone(),
                            config: None,
                        },
                    ),
                }
            }
            (None, None) => (None, None, ConfigSource::Unconfigured),
        };

        ServerConfig {
            ipc,
            ipc_problem,
            source,
            knowledge_dir,
        }
    }

    /// The IPC directory, when one is configured.
    pub fn ipc_dir(&self) -> Option<&Path> {
        self.ipc.as_ref().map(|c| c.ipc_dir.as_path())
    }

    /// A redacted description, safe to serve to a client.
    ///
    /// The instance token is never included — it is not a secret that protects
    /// anything, but echoing it into a transcript is still pointless exposure.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "source" => self.source.describe(),
            "ipc_dir" => match self.ipc_dir() {
                Some(p) => Json::Str(p.display().to_string()),
                None => Json::Null,
            },
            "configured" => self.ipc.is_some(),
            "problem" => match &self.ipc_problem {
                Some(e) => Json::Str(format!("{}: {}", e.code, e.message)),
                None => Json::Null,
            },
            "knowledge_dir" => match &self.knowledge_dir {
                Some(p) => Json::Str(p.display().to_string()),
                None => Json::Null,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "qlabs-mcp-config-{tag}-{}-{}",
            std::process::id(),
            qjson::time::unix_now()
        ));
        std::fs::create_dir_all(&p).expect("temp dir");
        p
    }

    #[test]
    fn unconfigured_has_no_bridge_and_no_problem() {
        let c = ServerConfig::unconfigured();
        assert!(c.ipc.is_none());
        assert!(c.ipc_problem.is_none());
        assert_eq!(c.source, ConfigSource::Unconfigured);
        assert_eq!(c.to_json().get("configured"), Some(&Json::Bool(false)));
    }

    #[test]
    fn a_config_file_supplies_the_directory_and_token() {
        let dir = temp_dir("cfgfile");
        let cfg = dir.join("config.json");
        std::fs::write(
            &cfg,
            r#"{"instance_token":"0123456789abcdef0123456789abcdef","ipc_dir":null}"#,
        )
        .unwrap();
        let resolved = ServerConfig::resolve(Some(&cfg), None, None);
        assert!(resolved.ipc.is_some(), "{:?}", resolved.ipc_problem);
        assert_eq!(resolved.ipc_dir(), Some(dir.join("ipc").as_path()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_explicit_ipc_dir_overrides_the_file() {
        let dir = temp_dir("override");
        let cfg = dir.join("config.json");
        std::fs::write(
            &cfg,
            r#"{"instance_token":"0123456789abcdef0123456789abcdef"}"#,
        )
        .unwrap();
        let other = dir.join("elsewhere");
        let resolved = ServerConfig::resolve(Some(&cfg), Some(&other), None);
        assert_eq!(resolved.ipc_dir(), Some(other.as_path()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_ipc_dir_alone_finds_the_sibling_config() {
        let dir = temp_dir("sibling");
        std::fs::write(
            dir.join("config.json"),
            r#"{"instance_token":"0123456789abcdef0123456789abcdef"}"#,
        )
        .unwrap();
        let ipc = dir.join("ipc");
        std::fs::create_dir_all(&ipc).unwrap();
        let resolved = ServerConfig::resolve(None, Some(&ipc), None);
        assert!(resolved.ipc.is_some(), "{:?}", resolved.ipc_problem);
        assert_eq!(resolved.ipc_dir(), Some(ipc.as_path()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_ipc_dir_without_a_token_is_a_reported_problem_not_a_crash() {
        let dir = temp_dir("notoken");
        let ipc = dir.join("ipc");
        std::fs::create_dir_all(&ipc).unwrap();
        let resolved = ServerConfig::resolve(None, Some(&ipc), None);
        assert!(resolved.ipc.is_none());
        let problem = resolved.ipc_problem.expect("a problem");
        assert_eq!(problem.code, reaper_ipc::codes::INVALID_INSTANCE_TOKEN);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_config_file_is_reported_not_fatal() {
        let resolved =
            ServerConfig::resolve(Some(Path::new("/nonexistent/config.json")), None, None);
        assert!(resolved.ipc.is_none());
        assert!(resolved.ipc_problem.is_some());
        assert_ne!(resolved.to_json().get("problem"), Some(&Json::Null));
    }

    #[test]
    fn the_token_never_reaches_the_json_form() {
        let dir = temp_dir("redact");
        let cfg = dir.join("config.json");
        std::fs::write(
            &cfg,
            r#"{"instance_token":"deadbeefdeadbeefdeadbeefdeadbeef"}"#,
        )
        .unwrap();
        let resolved = ServerConfig::resolve(Some(&cfg), None, None);
        let text = resolved.to_json().to_string();
        assert!(!text.contains("deadbeef"), "{text}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
