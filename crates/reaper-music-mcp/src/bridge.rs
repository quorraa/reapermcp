//! The REAPER bridge, as the tool layer sees it.
//!
//! Wraps [`reaper_ipc::BridgeClient`] and answers one question the tools should
//! not each have to answer for themselves: *what happens when REAPER is not
//! running?* The answer is always the same — a clean
//! [`BRIDGE_OFFLINE`][reaper_ipc::codes::BRIDGE_OFFLINE] tool error carrying a
//! remedy, never a hang and never a panic.
//!
//! Two clients are held rather than one so a read-only command cannot inherit
//! the write commands' 30-second deadline; §8 of `IPC_WIRE.md` recommends 5 s
//! for `ping` / `status` / `inspect_selection` and 30 s for the rest.

use crate::config::{ServerConfig, READ_TIMEOUT, WRITE_TIMEOUT};
use crate::error::ToolError;
use music_domain::plan::EditPlan;
use qjson::{json_obj, Json};
use reaper_ipc::{BridgeClient, CancelFlag, Filesystem, Heartbeat, IpcError, Snapshot};
use std::sync::Arc;

/// The bridge handle.
#[derive(Debug)]
pub struct Bridge {
    read: Option<BridgeClient>,
    write: Option<BridgeClient>,
    problem: Option<IpcError>,
}

impl Bridge {
    /// Builds a bridge from a resolved configuration.
    ///
    /// `fs` lets a test drive a [`reaper_ipc::MemFs`] or a scratch directory;
    /// production passes `None` and gets [`reaper_ipc::RealFs`].
    pub fn new(cfg: &ServerConfig, fs: Option<Arc<dyn Filesystem>>) -> Bridge {
        let Some(ipc) = cfg.ipc.clone() else {
            return Bridge {
                read: None,
                write: None,
                problem: Some(cfg.ipc_problem.clone().unwrap_or_else(|| {
                    IpcError::new(
                        reaper_ipc::codes::BRIDGE_OFFLINE,
                        "no IPC directory is configured; pass --ipc-dir or --config",
                    )
                })),
            };
        };

        let build = |timeout| {
            let c = ipc.clone().with_request_timeout(timeout);
            match &fs {
                Some(fs) => BridgeClient::with_fs(c, Arc::clone(fs)),
                None => BridgeClient::new(c),
            }
        };

        match (build(READ_TIMEOUT), build(WRITE_TIMEOUT)) {
            (Ok(read), Ok(write)) => Bridge {
                read: Some(read),
                write: Some(write),
                problem: None,
            },
            (Err(e), _) | (_, Err(e)) => Bridge {
                read: None,
                write: None,
                problem: Some(e),
            },
        }
    }

    /// A bridge that is permanently offline, with a stated reason.
    pub fn offline(reason: IpcError) -> Bridge {
        Bridge {
            read: None,
            write: None,
            problem: Some(reason),
        }
    }

    /// True when a client could be built. Says nothing about liveness.
    pub fn is_configured(&self) -> bool {
        self.read.is_some()
    }

    /// True when the bridge is configured and its heartbeat is fresh.
    pub fn is_online(&self) -> bool {
        self.read.as_ref().is_some_and(BridgeClient::is_online)
    }

    /// The configured IPC directory, when there is one.
    pub fn ipc_dir(&self) -> Option<&std::path::Path> {
        self.read.as_ref().map(|c| c.config().ipc_dir.as_path())
    }

    /// The heartbeat, or a structured reason why there is none.
    pub fn heartbeat(&self) -> Result<Heartbeat, ToolError> {
        Ok(self.read_client()?.heartbeat()?)
    }

    fn read_client(&self) -> Result<&BridgeClient, ToolError> {
        self.read.as_ref().ok_or_else(|| self.offline_error())
    }

    fn write_client(&self) -> Result<&BridgeClient, ToolError> {
        self.write.as_ref().ok_or_else(|| self.offline_error())
    }

    fn offline_error(&self) -> ToolError {
        let problem = self.problem.clone().unwrap_or_else(|| {
            IpcError::new(
                reaper_ipc::codes::BRIDGE_OFFLINE,
                "the REAPER bridge is not configured",
            )
        });
        // Whatever the underlying reason, the code a client branches on is
        // BRIDGE_OFFLINE: from the caller's point of view there is no bridge.
        ToolError::with_details(
            reaper_ipc::codes::BRIDGE_OFFLINE,
            problem.message.clone(),
            json_obj! { "underlying_code" => problem.code.clone(), "details" => problem.details.clone() },
        )
        .remedy(
            "start REAPER, run the QLabs_Reaper_MCP_Bridge.lua action, and start this server \
             with --config <REAPER>/Scripts/QLabs-Reaper-MCP/config.json",
        )
    }

    /// `status`.
    pub fn status(&self) -> Result<Json, ToolError> {
        Ok(self.read_client()?.status()?)
    }

    /// `ping`.
    pub fn ping(&self) -> Result<Json, ToolError> {
        Ok(self.read_client()?.ping()?)
    }

    /// `inspect_selection`, parsed and hash-verified.
    pub fn inspect(&self, payload: Json, cancel: &CancelFlag) -> Result<Snapshot, ToolError> {
        Ok(self.read_client()?.inspect_snapshot(payload, None, cancel)?)
    }

    /// `stage_candidate`.
    ///
    /// The plan crosses the boundary through
    /// [`reaper_ipc::plan::plan_to_wire`], which `BridgeClient` applies
    /// internally — [`EditPlan::to_json`] is never sent.
    pub fn stage(&self, plan: &EditPlan, payload: Json) -> Result<Json, ToolError> {
        Ok(self.write_client()?.stage_candidate(plan, payload)?)
    }

    /// `commit_candidate`.
    pub fn commit(&self, transaction_id: &str) -> Result<Json, ToolError> {
        Ok(self.write_client()?.commit_candidate(transaction_id)?)
    }

    /// `discard_candidate`.
    pub fn discard(&self, transaction_id: &str) -> Result<Json, ToolError> {
        Ok(self.write_client()?.discard_candidate(transaction_id)?)
    }

    /// `undo_last_generation`.
    pub fn undo(&self, transaction_id: &str) -> Result<Json, ToolError> {
        Ok(self.write_client()?.undo_last_generation(transaction_id)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unconfigured_bridge_reports_offline_for_every_command() {
        let b = Bridge::new(&ServerConfig::unconfigured(), None);
        assert!(!b.is_configured());
        assert!(!b.is_online());
        assert!(b.ipc_dir().is_none());

        for e in [
            b.status().unwrap_err(),
            b.ping().unwrap_err(),
            b.commit("t1").unwrap_err(),
            b.discard("t1").unwrap_err(),
            b.undo("t1").unwrap_err(),
            b.heartbeat().unwrap_err(),
        ] {
            assert_eq!(e.code, reaper_ipc::codes::BRIDGE_OFFLINE);
            assert!(e.remedy.is_some());
        }
    }

    #[test]
    fn offline_keeps_the_stated_reason_in_the_details() {
        let b = Bridge::offline(IpcError::new(
            reaper_ipc::codes::INVALID_INSTANCE_TOKEN,
            "no token",
        ));
        let e = b.status().unwrap_err();
        assert_eq!(e.code, reaper_ipc::codes::BRIDGE_OFFLINE);
        assert_eq!(
            e.details.str_field("underlying_code").unwrap(),
            reaper_ipc::codes::INVALID_INSTANCE_TOKEN
        );
    }

    #[test]
    fn a_memfs_bridge_is_configured_but_not_online() {
        let fs: Arc<dyn Filesystem> = Arc::new(reaper_ipc::MemFs::new());
        let cfg = ServerConfig {
            ipc: Some(reaper_ipc::IpcConfig::new(
                std::path::PathBuf::from("/ipc"),
                "0123456789abcdef0123456789abcdef",
            )),
            ipc_problem: None,
            source: crate::config::ConfigSource::IpcDir {
                dir: std::path::PathBuf::from("/ipc"),
                config: None,
            },
            knowledge_dir: None,
        };
        let b = Bridge::new(&cfg, Some(fs));
        assert!(b.is_configured());
        assert!(!b.is_online(), "no heartbeat has been written yet");
        assert_eq!(
            b.status().unwrap_err().code,
            reaper_ipc::codes::BRIDGE_OFFLINE
        );
    }
}
