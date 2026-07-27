//! `reaper-ipc` — the Rust half of the QLabs REAPER file IPC.
//!
//! The MCP server never links against REAPER. It talks to a ReaScript bridge
//! through a directory of JSON files, and this crate is that conversation: it
//! writes commands, waits for results, verifies what comes back, and turns
//! every failure into a structured [`IpcError`] with a stable code.
//!
//! There is no socket, no port, no pipe, and no command that can evaluate Lua,
//! run an action id, or name an arbitrary path. The allowlist in
//! [`limits::COMMANDS`] is the whole of the bridge's authority.
//!
//! # Modules
//!
//! | Module | Purpose |
//! |---|---|
//! | [`limits`] | the normative protocol constants and the command allowlist |
//! | [`error`] | [`IpcError`] and the closed error-code vocabulary |
//! | [`fs`] | the [`fs::Filesystem`] seam: [`fs::RealFs`] and [`fs::MemFs`] |
//! | [`hash`] | FNV-1a-64 and the versioned canonical strings |
//! | [`snapshot`] | parsing and independently verifying an `inspect_selection` result |
//! | [`protocol`] | request-id safety and the bridge's validation gauntlet |
//! | [`envelope`] | building requests and reading results |
//! | [`plan`] | the `EditPlan` wire form and its two documented divergences |
//! | [`config`] | [`IpcConfig`] and `config.json` loading |
//! | [`heartbeat`] | liveness from `heartbeat.json` |
//! | [`cancel`] | [`CancelFlag`] |
//! | [`client`] | [`BridgeClient`], the state machine |
//! | [`testing`] | [`testing::FakeBridge`], a deterministic bridge double |
//!
//! # The three invariants
//!
//! **Atomic writes.** Every published file is written to a temporary name *in
//! its final directory*, flushed and `sync_all()`ed, then renamed. A reader
//! either sees nothing or sees a complete document. Neither side ever reads a
//! `.tmp`.
//!
//! **One value reaches a path.** `request_id` is minted here, never accepted
//! from a caller, and validated against `^[A-Za-z0-9][A-Za-z0-9._-]*$` with no
//! `..` substring before any path is built from it. The client writes only
//! inside `commands/` and deletes only inside `results/`.
//!
//! **Hashes are checked, not trusted.** [`snapshot::Snapshot::verify`]
//! recomputes every hash it can from the data that travelled with it. A hash
//! that does not reproduce is an error, because a snapshot whose own hashes
//! disagree cannot serve as a staleness token.
//!
//! # Example
//!
//! ```no_run
//! use reaper_ipc::{BridgeClient, CancelFlag, IpcConfig};
//! use std::path::Path;
//!
//! # fn main() -> Result<(), reaper_ipc::IpcError> {
//! // The token comes from the bridge's config.json; never invent one.
//! let cfg = IpcConfig::from_config_file(Path::new("/reaper/Scripts/QLabs-Reaper-MCP/config.json"))?;
//! let client = BridgeClient::new(cfg)?;
//!
//! if !client.is_online() {
//!     // heartbeat() reports *why*: BRIDGE_OFFLINE or IPC_PROTOCOL_MISMATCH.
//!     return Err(client.heartbeat().expect_err("offline"));
//! }
//!
//! let cancel = CancelFlag::new();
//! let snapshot = client.inspect_snapshot(qjson::Json::Null, None, &cancel)?;
//! println!("{} notes, hash {:?}", snapshot.note_count, snapshot.snapshot_hash);
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

pub mod cancel;
pub mod client;
pub mod config;
pub mod envelope;
pub mod error;
pub mod fs;
pub mod hash;
pub mod heartbeat;
pub mod limits;
pub mod plan;
pub mod protocol;
pub mod snapshot;
pub mod testing;

pub use cancel::CancelFlag;
pub use client::BridgeClient;
pub use config::IpcConfig;
pub use envelope::{ExpectedProject, RequestEnvelope, ResultEnvelope};
pub use error::{codes, IpcError};
pub use fs::{Filesystem, MemFs, RealFs};
pub use heartbeat::Heartbeat;
pub use limits::{BRIDGE_VERSION, COMMANDS, PROTOCOL_VERSION};
pub use snapshot::Snapshot;

#[cfg(test)]
mod contract_surface {
    //! Compile-time assertions that the frozen `reaper-ipc` API exists with the
    //! signatures the MCP server crate is written against. If one of these
    //! stops compiling, a downstream crate has just been broken.

    use super::*;
    use music_domain::plan::EditPlan;
    use qjson::Json;
    use std::path::Path;
    use std::time::Duration;

    #[test]
    fn ipc_config_signatures() {
        let _: fn(&Path) -> Result<IpcConfig, IpcError> = IpcConfig::from_config_file;
        let _: fn(&IpcConfig) -> Result<(), IpcError> = IpcConfig::validate;
        let cfg = IpcConfig::new("/ipc", "t");
        let _: &std::path::PathBuf = &cfg.ipc_dir;
        let _: &String = &cfg.instance_token;
        let _: Duration = cfg.request_timeout;
        let _: Duration = cfg.poll_interval;
        let _: usize = cfg.max_request_bytes;
        let _: usize = cfg.max_result_bytes;
    }

    #[test]
    fn bridge_client_signatures() {
        let _: fn(IpcConfig) -> Result<BridgeClient, IpcError> = BridgeClient::new;
        let _: fn(&BridgeClient) -> Result<Heartbeat, IpcError> = BridgeClient::heartbeat;
        let _: fn(&BridgeClient) -> bool = BridgeClient::is_online;
        /// The frozen signature of `BridgeClient::call`, named so the
        /// assertion below stays readable.
        type Call = fn(
            &BridgeClient,
            &str,
            Json,
            Option<&ExpectedProject>,
            &CancelFlag,
        ) -> Result<Json, IpcError>;
        let _: Call = BridgeClient::call;
        let _: fn(&BridgeClient) -> Result<Json, IpcError> = BridgeClient::status;
        let _: fn(&BridgeClient, Json) -> Result<Json, IpcError> = BridgeClient::inspect_selection;
        let _: fn(&BridgeClient, &EditPlan, Json) -> Result<Json, IpcError> =
            BridgeClient::stage_candidate;
        let _: fn(&BridgeClient, &str) -> Result<Json, IpcError> = BridgeClient::commit_candidate;
        let _: fn(&BridgeClient, &str) -> Result<Json, IpcError> = BridgeClient::discard_candidate;
        let _: fn(&BridgeClient, &str) -> Result<Json, IpcError> =
            BridgeClient::undo_last_generation;
    }

    #[test]
    fn heartbeat_fields() {
        let doc = qjson::json_obj! {
            "protocol_version" => PROTOCOL_VERSION,
            "bridge_version" => "1.0.0",
            "reaper_version" => "7.22/linux-x86_64",
            "project_uuid" => "p",
            "status" => "online",
            "timestamp" => qjson::time::unix_now(),
        };
        let hb = Heartbeat::parse(&doc, qjson::time::unix_now()).expect("online");
        let _: String = hb.bridge_version.clone();
        let _: String = hb.reaper_version.clone();
        let _: String = hb.protocol_version.clone();
        let _: Option<String> = hb.project_uuid.clone();
        let _: Duration = hb.age;
        let _: Json = hb.raw.clone();
    }

    #[test]
    fn expected_project_fields() {
        let e = ExpectedProject::default();
        let _: Option<String> = e.project_uuid;
        let _: Option<i64> = e.state_change_count;
        let _: Option<String> = e.source_item_guid;
        let _: Option<String> = e.source_take_guid;
        let _: Option<String> = e.midi_hash;
        let _: Option<String> = e.tempo_map_hash;
        let _: Option<String> = e.snapshot_hash;
    }

    #[test]
    fn cancel_flag_signatures() {
        let _: fn() -> CancelFlag = CancelFlag::new;
        let _: fn(&CancelFlag) = CancelFlag::cancel;
        let _: fn(&CancelFlag) -> bool = CancelFlag::is_cancelled;
        let _ = CancelFlag::default().clone();
    }

    #[test]
    fn ipc_error_signatures() {
        let e = IpcError::new(codes::IPC_TIMEOUT, "m");
        let _: String = e.code.clone();
        let _: String = e.message.clone();
        let _: Json = e.details.clone();
        let _: fn(&IpcError) -> bool = IpcError::is_bridge_offline;
        let _: fn(&IpcError) -> bool = IpcError::is_timeout;
        let _: fn(&IpcError) -> bool = IpcError::is_stale;
        let _: &dyn std::error::Error = &e;
    }

    #[test]
    fn the_protocol_version_is_the_one_the_workspace_agreed_on() {
        assert_eq!(PROTOCOL_VERSION, "qlabs-reaper-ipc/1");
        assert_eq!(BRIDGE_VERSION, "1.0.0");
        assert_eq!(COMMANDS.len(), 7);
    }
}
