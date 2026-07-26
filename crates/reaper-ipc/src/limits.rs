//! Normative protocol constants.
//!
//! Every value here is copied verbatim from §8 of the IPC wire specification
//! and is cross-checked against `fixtures/mock-reaper/bridge/limits.json` by a
//! test in this module, so the two can never silently drift.
//!
//! The bridge also returns the same table in `status.limits`; a client that
//! wants to detect a bridge built against different limits should compare
//! [`limits_json`] with what `status` reported rather than assuming.

use qjson::{json_obj, Json};

/// Wire protocol identifier. Both sides must match this exactly.
pub const PROTOCOL_VERSION: &str = "qlabs-reaper-ipc/1";

/// Bridge version this client was written against.
///
/// This is *not* sent by default: `require_bridge_version` is opt-in, because
/// pinning it turns every bridge upgrade into a hard failure.
pub const BRIDGE_VERSION: &str = "1.0.0";

/// Version of the project-level extended-state layout the bridge writes.
pub const BRIDGE_SCHEMA_VERSION: &str = "1";

/// Maximum size in bytes of a `<id>.command.json` file.
pub const MAX_REQUEST_BYTES: usize = 1_048_576;

/// Maximum size in bytes of a `<id>.result.json` file.
pub const MAX_RESULT_BYTES: usize = 8_388_608;

/// Maximum length in bytes of the `request_id` string and filename stem.
pub const MAX_REQUEST_ID_LEN: usize = 128;

/// Maximum length in bytes of any identifier field.
pub const MAX_ID_LEN: usize = 128;

/// Maximum length in bytes of a free-form string field.
pub const MAX_STRING_LEN: usize = 512;

/// Maximum notes the bridge will insert across one edit plan.
pub const MAX_GENERATED_NOTES: usize = 20_000;

/// Maximum notes the bridge will insert into a single MIDI item.
pub const MAX_NOTES_PER_ITEM: usize = 8_000;

/// Maximum tracks one stage operation may create.
pub const MAX_TRACKS_PER_STAGE: usize = 32;

/// Maximum MIDI items one stage operation may create.
pub const MAX_ITEMS_PER_STAGE: usize = 64;

/// Maximum regions one stage operation may create.
pub const MAX_REGIONS_PER_STAGE: usize = 16;

/// Maximum MIDI sends one stage operation may create.
pub const MAX_SENDS_PER_STAGE: usize = 16;

/// Maximum operations in one edit plan.
pub const MAX_OPERATIONS: usize = 512;

/// Maximum preconditions in one edit plan.
pub const MAX_PRECONDITIONS: usize = 64;

/// Maximum expected outputs in one edit plan.
pub const MAX_EXPECTED_OUTPUTS: usize = 128;

/// Maximum notes read from one take per inspection.
pub const MAX_READ_NOTES: usize = 100_000;

/// Maximum plan-supplied ownership tags per object.
pub const MAX_TAGS_PER_OBJECT: usize = 16;

/// Size of the bridge's in-memory replay-guard ring.
pub const SEEN_REQUEST_IDS: usize = 512;

/// Minimum wall time in seconds between the bridge's directory scans.
pub const POLL_INTERVAL_SECONDS: f64 = 0.05;

/// Command files the bridge claims per REAPER defer tick.
pub const MAX_COMMANDS_PER_TICK: usize = 4;

/// Seconds between the bridge's heartbeat writes.
pub const HEARTBEAT_INTERVAL_SECONDS: f64 = 1.0;

/// Lock/heartbeat staleness threshold, in seconds.
///
/// This is the default used when `heartbeat.json` does not carry its own
/// `stale_after_seconds`.
pub const LOCK_STALE_SECONDS: i64 = 10;

/// Seconds between the bridge's stale-file garbage-collection sweeps.
pub const GC_INTERVAL_SECONDS: i64 = 30;

/// Age in seconds after which an unclaimed `.command.json` is deleted.
pub const STALE_COMMAND_SECONDS: i64 = 300;

/// Age in seconds after which an abandoned `.processing.json` moves to `failed/`.
pub const STALE_PROCESSING_SECONDS: i64 = 120;

/// Age in seconds after which an uncollected `.result.json` is deleted.
pub const STALE_RESULT_SECONDS: i64 = 900;

/// Age in seconds after which a stray `.tmp` / `.result.tmp` is deleted.
pub const STALE_TMP_SECONDS: i64 = 60;

/// Age in seconds after which a `failed/` artefact is deleted.
pub const STALE_FAILED_SECONDS: i64 = 86_400;

/// Log rotation threshold in bytes.
pub const LOG_MAX_BYTES: usize = 1_048_576;

/// Rotated log generations the bridge keeps.
pub const LOG_KEEP: usize = 3;

/// Tolerance in seconds applied to `expires_at` on the bridge side.
pub const CLOCK_SKEW_SECONDS: i64 = 5;

/// Maximum length in bytes of an error `message` string.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 2000;

/// The mandatory prefix of every undo label this MCP creates.
///
/// The bridge refuses to open an undo block with any other label, and
/// `undo_last_generation` refuses to undo an entry without it. That prefix is
/// the entire mechanism by which the MCP guarantees it never undoes a user's
/// own edit.
pub const UNDO_LABEL_PREFIX: &str = "QLabs MCP: ";

/// Maximum length in bytes of an undo label.
pub const MAX_UNDO_LABEL_BYTES: usize = 200;

/// Filename suffix of a request the client is still writing.
pub const SUFFIX_TMP: &str = ".tmp";

/// Filename suffix of a request ready to be claimed.
pub const SUFFIX_COMMAND: &str = ".command.json";

/// Filename suffix of a request the bridge has claimed.
pub const SUFFIX_PROCESSING: &str = ".processing.json";

/// Filename suffix of a result the bridge is still writing.
pub const SUFFIX_RESULT_TMP: &str = ".result.tmp";

/// Filename suffix of a complete result.
pub const SUFFIX_RESULT: &str = ".result.json";

/// Filename suffix of a quarantined request.
pub const SUFFIX_FAILED: &str = ".failed.json";

/// The complete command allowlist, sorted, exactly as the bridge reports it.
///
/// There is deliberately no command that evaluates Lua, runs an action id,
/// reads or writes an arbitrary path, or passes anything through to the REAPER
/// API.
pub const COMMANDS: &[&str] = &[
    "commit_candidate",
    "discard_candidate",
    "inspect_selection",
    "ping",
    "stage_candidate",
    "status",
    "undo_last_generation",
];

/// True when `name` is on the command allowlist.
pub fn is_allowed_command(name: &str) -> bool {
    COMMANDS.contains(&name)
}

/// Commands that require an active REAPER project.
pub fn command_needs_project(name: &str) -> bool {
    !matches!(name, "ping" | "status")
}

/// Every limit by name, in the shape the bridge returns under `status.limits`.
pub fn limits_json() -> Json {
    json_obj! {
        "CLOCK_SKEW_SECONDS" => CLOCK_SKEW_SECONDS,
        "GC_INTERVAL_SECONDS" => GC_INTERVAL_SECONDS,
        "HEARTBEAT_INTERVAL_SECONDS" => HEARTBEAT_INTERVAL_SECONDS,
        "LOCK_STALE_SECONDS" => LOCK_STALE_SECONDS,
        "LOG_KEEP" => LOG_KEEP,
        "LOG_MAX_BYTES" => LOG_MAX_BYTES,
        "MAX_COMMANDS_PER_TICK" => MAX_COMMANDS_PER_TICK,
        "MAX_EXPECTED_OUTPUTS" => MAX_EXPECTED_OUTPUTS,
        "MAX_GENERATED_NOTES" => MAX_GENERATED_NOTES,
        "MAX_ID_LEN" => MAX_ID_LEN,
        "MAX_ITEMS_PER_STAGE" => MAX_ITEMS_PER_STAGE,
        "MAX_NOTES_PER_ITEM" => MAX_NOTES_PER_ITEM,
        "MAX_OPERATIONS" => MAX_OPERATIONS,
        "MAX_PRECONDITIONS" => MAX_PRECONDITIONS,
        "MAX_READ_NOTES" => MAX_READ_NOTES,
        "MAX_REGIONS_PER_STAGE" => MAX_REGIONS_PER_STAGE,
        "MAX_REQUEST_BYTES" => MAX_REQUEST_BYTES,
        "MAX_REQUEST_ID_LEN" => MAX_REQUEST_ID_LEN,
        "MAX_RESULT_BYTES" => MAX_RESULT_BYTES,
        "MAX_SENDS_PER_STAGE" => MAX_SENDS_PER_STAGE,
        "MAX_STRING_LEN" => MAX_STRING_LEN,
        "MAX_TAGS_PER_OBJECT" => MAX_TAGS_PER_OBJECT,
        "MAX_TRACKS_PER_STAGE" => MAX_TRACKS_PER_STAGE,
        "POLL_INTERVAL_SECONDS" => POLL_INTERVAL_SECONDS,
        "SEEN_REQUEST_IDS" => SEEN_REQUEST_IDS,
        "STALE_COMMAND_SECONDS" => STALE_COMMAND_SECONDS,
        "STALE_FAILED_SECONDS" => STALE_FAILED_SECONDS,
        "STALE_PROCESSING_SECONDS" => STALE_PROCESSING_SECONDS,
        "STALE_RESULT_SECONDS" => STALE_RESULT_SECONDS,
        "STALE_TMP_SECONDS" => STALE_TMP_SECONDS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits_fixture() -> Json {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mock-reaper/bridge/limits.json"
        );
        let text = std::fs::read_to_string(path).expect("limits fixture");
        Json::parse(&text).expect("limits fixture parses")
    }

    #[test]
    fn every_limit_matches_the_recorded_bridge_manifest() {
        let want = limits_fixture();
        let got = limits_json();
        let want_obj = want.as_obj().expect("object");
        let got_obj = got.as_obj().expect("object");
        for (k, v) in want_obj.iter() {
            let mine = got_obj
                .get(k)
                .unwrap_or_else(|| panic!("missing limit {k}"));
            assert_eq!(
                mine.as_f64(),
                v.as_f64(),
                "limit {k} differs: rust {mine:?} vs bridge {v:?}"
            );
        }
        assert_eq!(want_obj.len(), got_obj.len(), "limit count differs");
    }

    #[test]
    fn command_allowlist_matches_the_recorded_bridge_manifest() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mock-reaper/bridge/error-codes.json"
        );
        let text = std::fs::read_to_string(path).expect("error-codes fixture");
        let doc = Json::parse(&text).expect("parses");
        let names: Vec<&str> = doc
            .arr_field("commands")
            .expect("commands")
            .iter()
            .map(|v| v.as_str().expect("string"))
            .collect();
        assert_eq!(names, COMMANDS);
    }

    #[test]
    fn allowlist_is_sorted_and_closed() {
        let mut sorted = COMMANDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, COMMANDS);
        assert!(is_allowed_command("ping"));
        assert!(!is_allowed_command("execute_lua"));
        assert!(!is_allowed_command("PING"));
        assert!(!is_allowed_command(""));
    }

    #[test]
    fn only_ping_and_status_run_without_a_project() {
        assert!(!command_needs_project("ping"));
        assert!(!command_needs_project("status"));
        for c in COMMANDS
            .iter()
            .filter(|c| !matches!(**c, "ping" | "status"))
        {
            assert!(command_needs_project(c), "{c} should need a project");
        }
    }

    #[test]
    fn protocol_version_is_the_frozen_string() {
        assert_eq!(PROTOCOL_VERSION, "qlabs-reaper-ipc/1");
    }
}
