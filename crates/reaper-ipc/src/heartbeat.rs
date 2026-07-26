//! Bridge liveness from `heartbeat.json`.
//!
//! The bridge rewrites this file atomically every second, and once more with
//! `"status": "offline"` at `atexit`.
//!
//! # Liveness rule (§11)
//!
//! The bridge is online iff the file exists, parses, `status == "online"`, and
//! `now - timestamp <= stale_after_seconds`. Anything else is
//! [`BRIDGE_OFFLINE`][crate::error::codes::BRIDGE_OFFLINE] and the client must
//! not write a command file.
//!
//! `timestamp` is the same machine's clock. If it appears to be in the future,
//! the age is treated as zero rather than as an error.

use crate::error::{codes, IpcError};
use crate::limits;
use qjson::Json;
use std::time::Duration;

/// A parsed, validated `heartbeat.json`.
#[derive(Clone, Debug)]
pub struct Heartbeat {
    /// The bridge's semantic version.
    pub bridge_version: String,
    /// Raw `GetAppVersion()`, or the empty string when the bridge sent `null`.
    pub reaper_version: String,
    /// The bridge's IPC protocol version.
    pub protocol_version: String,
    /// The persistent project UUID, `None` until a first inspection mints one.
    pub project_uuid: Option<String>,
    /// How old the heartbeat is, clamped at zero for a future timestamp.
    pub age: Duration,
    /// The unmodified document.
    pub raw: Json,
}

impl Heartbeat {
    /// Parses and validates a heartbeat document against a known `now`.
    ///
    /// Checks run in this order:
    ///
    /// 1. the body is an object;
    /// 2. `protocol_version` matches — a configuration error worth surfacing
    ///    even when the bridge is also stale, because `BRIDGE_OFFLINE` would
    ///    send the user looking for the wrong problem;
    /// 3. `status == "online"`;
    /// 4. `timestamp` is present and not stale.
    pub fn parse(doc: &Json, now_unix: i64) -> Result<Heartbeat, IpcError> {
        if doc.as_obj().is_none() {
            return Err(IpcError::new(
                codes::BRIDGE_OFFLINE,
                "heartbeat.json does not contain a JSON object",
            ));
        }

        // A *present but different* protocol version is a configuration error
        // worth naming. A missing one means the document is not a heartbeat at
        // all, which is plain `BRIDGE_OFFLINE`.
        let Some(protocol_version) = doc
            .get("protocol_version")
            .and_then(Json::as_str)
            .map(str::to_string)
        else {
            return Err(IpcError::new(
                codes::BRIDGE_OFFLINE,
                "heartbeat.json has no protocol_version",
            ));
        };
        if protocol_version != limits::PROTOCOL_VERSION {
            return Err(IpcError::with_details(
                codes::IPC_PROTOCOL_MISMATCH,
                format!(
                    "bridge speaks {protocol_version:?}, this client speaks {:?}",
                    limits::PROTOCOL_VERSION
                ),
                qjson::json_obj! {
                    "expected" => limits::PROTOCOL_VERSION,
                    "received" => protocol_version.clone(),
                },
            ));
        }

        let status = doc.get("status").and_then(Json::as_str).unwrap_or("");
        if status != "online" {
            return Err(IpcError::with_details(
                codes::BRIDGE_OFFLINE,
                format!("bridge status is {status:?}"),
                qjson::json_obj! { "status" => status },
            ));
        }

        let Some(timestamp) = doc.get("timestamp").and_then(Json::as_i64) else {
            return Err(IpcError::new(
                codes::BRIDGE_OFFLINE,
                "heartbeat.json has no integer timestamp",
            ));
        };

        let stale_after = doc
            .get("stale_after_seconds")
            .and_then(Json::as_i64)
            .filter(|v| *v > 0)
            .unwrap_or(limits::LOCK_STALE_SECONDS);

        // A heartbeat from the future means a clock hiccup, not staleness.
        let age_secs = (now_unix - timestamp).max(0);
        if age_secs > stale_after {
            return Err(IpcError::with_details(
                codes::BRIDGE_OFFLINE,
                format!("heartbeat is {age_secs}s old; stale after {stale_after}s"),
                qjson::json_obj! {
                    "age_seconds" => age_secs,
                    "stale_after_seconds" => stale_after,
                    "timestamp" => timestamp,
                },
            ));
        }

        Ok(Heartbeat {
            bridge_version: doc
                .get("bridge_version")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            reaper_version: doc
                .get("reaper_version")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            protocol_version,
            project_uuid: doc
                .get("project_uuid")
                .and_then(Json::as_str)
                .map(str::to_string),
            age: Duration::from_secs(age_secs as u64),
            raw: doc.clone(),
        })
    }

    /// The bridge's own staleness threshold, or the protocol default.
    pub fn stale_after(&self) -> Duration {
        let secs = self
            .raw
            .get("stale_after_seconds")
            .and_then(Json::as_i64)
            .filter(|v| *v > 0)
            .unwrap_or(limits::LOCK_STALE_SECONDS);
        Duration::from_secs(secs as u64)
    }

    /// The `pid_token` identifying the live bridge instance.
    pub fn pid_token(&self) -> Option<&str> {
        self.raw.get("pid_token").and_then(Json::as_str)
    }

    /// The IPC directory the bridge believes it is serving.
    pub fn ipc_dir(&self) -> Option<&str> {
        self.raw.get("ipc_dir").and_then(Json::as_str)
    }

    /// The bridge's project-state schema version.
    pub fn bridge_schema_version(&self) -> Option<&str> {
        self.raw.get("bridge_schema_version").and_then(Json::as_str)
    }

    /// The command allowlist the bridge advertises.
    pub fn commands(&self) -> Vec<String> {
        match self.raw.get("commands") {
            Some(Json::Arr(a)) => a
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// True when the bridge advertises exactly the allowlist this build knows.
    ///
    /// A `false` is not fatal — it means the two sides were built from
    /// different revisions — but it is worth logging.
    pub fn commands_match_this_build(&self) -> bool {
        self.commands() == limits::COMMANDS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instant the recorded heartbeat was written.
    const HB_TS: i64 = 1_785_091_879;

    fn recorded() -> Json {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mock-reaper/bridge/heartbeat.json"
        );
        let text = std::fs::read_to_string(path).expect("heartbeat fixture");
        Json::parse(&text).expect("parses")
    }

    #[test]
    fn parses_the_recorded_heartbeat() {
        let hb = Heartbeat::parse(&recorded(), HB_TS).expect("online");
        assert_eq!(hb.bridge_version, "1.0.0");
        assert_eq!(hb.reaper_version, "7.22/linux-x86_64");
        assert_eq!(hb.protocol_version, limits::PROTOCOL_VERSION);
        assert_eq!(
            hb.project_uuid.as_deref(),
            Some("00000000-0000-4000-8000-000000000001")
        );
        assert_eq!(hb.age, Duration::ZERO);
        assert_eq!(hb.pid_token(), Some("9f2c1a7b4e0d63558a1140fbb2c37e91"));
        assert_eq!(hb.bridge_schema_version(), Some("1"));
        assert_eq!(hb.ipc_dir(), Some("/reaper/Scripts/QLabs-Reaper-MCP/ipc"));
    }

    #[test]
    fn the_recorded_heartbeat_advertises_this_builds_allowlist() {
        let hb = Heartbeat::parse(&recorded(), HB_TS).expect("online");
        assert!(hb.commands_match_this_build());
        assert_eq!(hb.commands().len(), limits::COMMANDS.len());
    }

    #[test]
    fn age_is_computed_from_the_timestamp() {
        let hb = Heartbeat::parse(&recorded(), HB_TS + 7).expect("online");
        assert_eq!(hb.age, Duration::from_secs(7));
    }

    #[test]
    fn a_heartbeat_exactly_at_the_staleness_threshold_is_still_online() {
        let hb = Heartbeat::parse(&recorded(), HB_TS + 10).expect("online");
        assert_eq!(hb.age, Duration::from_secs(10));
        assert_eq!(hb.stale_after(), Duration::from_secs(10));
    }

    #[test]
    fn a_heartbeat_one_second_past_the_threshold_is_offline() {
        let err = Heartbeat::parse(&recorded(), HB_TS + 11).expect_err("stale");
        assert_eq!(err.code, codes::BRIDGE_OFFLINE);
        assert!(err.is_bridge_offline());
        assert_eq!(
            err.details.get("age_seconds").and_then(Json::as_i64),
            Some(11)
        );
    }

    #[test]
    fn a_future_timestamp_is_treated_as_age_zero_not_as_stale() {
        let hb = Heartbeat::parse(&recorded(), HB_TS - 3600).expect("online");
        assert_eq!(hb.age, Duration::ZERO);
    }

    #[test]
    fn an_offline_status_is_bridge_offline() {
        let mut doc = recorded();
        if let Json::Obj(m) = &mut doc {
            m.insert("status", Json::Str("offline".into()));
        }
        let err = Heartbeat::parse(&doc, HB_TS).expect_err("offline");
        assert_eq!(err.code, codes::BRIDGE_OFFLINE);
        assert_eq!(err.detail_str("status"), Some("offline"));
    }

    #[test]
    fn a_missing_status_is_bridge_offline() {
        let mut doc = recorded();
        if let Json::Obj(m) = &mut doc {
            m.remove("status");
        }
        assert_eq!(
            Heartbeat::parse(&doc, HB_TS).expect_err("no status").code,
            codes::BRIDGE_OFFLINE
        );
    }

    #[test]
    fn a_missing_timestamp_is_bridge_offline() {
        let mut doc = recorded();
        if let Json::Obj(m) = &mut doc {
            m.remove("timestamp");
        }
        assert_eq!(
            Heartbeat::parse(&doc, HB_TS).expect_err("no ts").code,
            codes::BRIDGE_OFFLINE
        );
    }

    #[test]
    fn a_missing_protocol_version_is_bridge_offline_not_a_mismatch() {
        let mut doc = recorded();
        if let Json::Obj(m) = &mut doc {
            m.remove("protocol_version");
        }
        let err = Heartbeat::parse(&doc, HB_TS).expect_err("not a heartbeat");
        assert_eq!(err.code, codes::BRIDGE_OFFLINE);
    }

    #[test]
    fn a_foreign_protocol_version_is_a_protocol_mismatch_not_offline() {
        let mut doc = recorded();
        if let Json::Obj(m) = &mut doc {
            m.insert("protocol_version", Json::Str("qlabs-reaper-ipc/2".into()));
        }
        let err = Heartbeat::parse(&doc, HB_TS).expect_err("mismatch");
        assert_eq!(err.code, codes::IPC_PROTOCOL_MISMATCH);
        assert!(!err.is_bridge_offline());
        assert_eq!(err.detail_str("received"), Some("qlabs-reaper-ipc/2"));
    }

    #[test]
    fn a_protocol_mismatch_wins_over_staleness() {
        let mut doc = recorded();
        if let Json::Obj(m) = &mut doc {
            m.insert("protocol_version", Json::Str("qlabs-reaper-ipc/2".into()));
        }
        let err = Heartbeat::parse(&doc, HB_TS + 100_000).expect_err("mismatch");
        assert_eq!(err.code, codes::IPC_PROTOCOL_MISMATCH);
    }

    #[test]
    fn a_non_object_document_is_bridge_offline() {
        assert_eq!(
            Heartbeat::parse(&Json::Arr(vec![]), 0)
                .expect_err("array")
                .code,
            codes::BRIDGE_OFFLINE
        );
    }

    #[test]
    fn a_bridge_supplied_staleness_threshold_is_honoured() {
        let mut doc = recorded();
        if let Json::Obj(m) = &mut doc {
            m.insert("stale_after_seconds", Json::Int(60));
        }
        let hb = Heartbeat::parse(&doc, HB_TS + 45).expect("still online at 45s");
        assert_eq!(hb.stale_after(), Duration::from_secs(60));
        assert!(Heartbeat::parse(&doc, HB_TS + 61).is_err());
    }

    #[test]
    fn a_nonsense_staleness_threshold_falls_back_to_the_protocol_default() {
        let mut doc = recorded();
        if let Json::Obj(m) = &mut doc {
            m.insert("stale_after_seconds", Json::Int(0));
        }
        let hb = Heartbeat::parse(&doc, HB_TS + 5).expect("online");
        assert_eq!(
            hb.stale_after(),
            Duration::from_secs(limits::LOCK_STALE_SECONDS as u64)
        );
    }

    #[test]
    fn a_null_project_uuid_is_none() {
        let mut doc = recorded();
        if let Json::Obj(m) = &mut doc {
            m.insert("project_uuid", Json::Null);
        }
        let hb = Heartbeat::parse(&doc, HB_TS).expect("online");
        assert!(hb.project_uuid.is_none());
    }

    #[test]
    fn a_null_reaper_version_becomes_the_empty_string() {
        let mut doc = recorded();
        if let Json::Obj(m) = &mut doc {
            m.insert("reaper_version", Json::Null);
        }
        let hb = Heartbeat::parse(&doc, HB_TS).expect("online");
        assert_eq!(hb.reaper_version, "");
        assert!(hb.raw.get("reaper_version").expect("present").is_null());
    }

    #[test]
    fn a_differing_allowlist_is_detected_but_not_fatal() {
        let mut doc = recorded();
        if let Json::Obj(m) = &mut doc {
            m.insert("commands", Json::Arr(vec![Json::Str("ping".into())]));
        }
        let hb = Heartbeat::parse(&doc, HB_TS).expect("online");
        assert!(!hb.commands_match_this_build());
        assert_eq!(hb.commands(), vec!["ping".to_string()]);
    }
}
