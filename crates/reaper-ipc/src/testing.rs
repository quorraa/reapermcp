//! A fake bridge, for testing the client without REAPER.
//!
//! [`FakeBridge`] scans an IPC directory, claims commands exactly the way
//! `bridge.lua` does — one `rename` into `processing/`, no reading of `.tmp`
//! files — runs the same envelope validation the real bridge runs
//! ([`crate::protocol::validate_envelope`]), and publishes results through the
//! mandated `.result.tmp` → `.result.json` rename.
//!
//! It is driven by [`FakeBridge::pump`], which performs exactly one defer tick.
//! Combined with [`crate::client::BridgeClient::with_poll_hook`] this makes
//! round trips, timeouts, error propagation and cancellation fully
//! deterministic: no threads, no scheduler dependence, no wall-clock waits.
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use reaper_ipc::{fs::MemFs, testing::FakeBridge};
//! let fs = Arc::new(MemFs::new());
//! let bridge = Arc::new(FakeBridge::new(fs.clone(), "/ipc", "token"));
//! bridge.write_heartbeat("online");
//! let b = bridge.clone();
//! // ... build a client over `fs` with `.with_poll_hook(Arc::new(move |_| { b.pump(); }))`
//! ```
//!
//! This module is compiled into the library rather than gated behind
//! `#[cfg(test)]` so integration tests under `tests/` — and downstream crates
//! that need a bridge-shaped stub — can use it.

use crate::envelope::serialize;
use crate::error::{codes, IpcError};
use crate::fs::Filesystem;
use crate::hash;
use crate::limits;
use crate::protocol::{self, ValidatedRequest, ValidationContext};
use qjson::{Json, JsonMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// What the fake bridge should do with a request that passed validation.
pub enum Response {
    /// Reply with a success envelope carrying this result object.
    Ok(Json),
    /// Reply with a failure envelope carrying this error.
    Err(IpcError),
    /// Write no result at all.
    ///
    /// Models the real bridge's quarantine path, where a command file whose
    /// stem is unusable is moved to `failed/` and the client only ever learns
    /// about it by timing out.
    Silent,
}

/// The signature of a custom responder.
pub type Responder = Box<dyn Fn(&ValidatedRequest) -> Response + Send + Sync>;

#[derive(Debug, Default)]
struct FakeState {
    seen: Vec<String>,
    claimed: u64,
    processed: u64,
    failed: u64,
    quarantined: Vec<String>,
}

/// A bridge-shaped test double.
pub struct FakeBridge {
    fs: Arc<dyn Filesystem>,
    ipc_dir: PathBuf,
    instance_token: String,
    bridge_version: String,
    responder: Responder,
    now: Mutex<i64>,
    state: Mutex<FakeState>,
}

impl std::fmt::Debug for FakeBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeBridge")
            .field("ipc_dir", &self.ipc_dir)
            .finish_non_exhaustive()
    }
}

impl FakeBridge {
    /// Creates a fake bridge over `fs`, serving `ipc_dir`, and creates the
    /// whole IPC tree the way the real bridge does at startup.
    pub fn new(
        fs: Arc<dyn Filesystem>,
        ipc_dir: impl Into<PathBuf>,
        instance_token: impl Into<String>,
    ) -> FakeBridge {
        let ipc_dir = ipc_dir.into();
        for sub in ["commands", "processing", "results", "failed", "logs"] {
            let _ = fs.create_dir_all(&ipc_dir.join(sub));
        }
        let _ = fs.create_dir_all(&ipc_dir);
        FakeBridge {
            fs,
            ipc_dir,
            instance_token: instance_token.into(),
            bridge_version: limits::BRIDGE_VERSION.to_string(),
            responder: Box::new(default_responder),
            now: Mutex::new(qjson::time::unix_now()),
            state: Mutex::new(FakeState::default()),
        }
    }

    /// Replaces the responder. See [`Response`].
    pub fn with_responder(mut self, responder: Responder) -> FakeBridge {
        self.responder = responder;
        self
    }

    /// Replies to every request with this error, whatever the command.
    pub fn always_failing(self, error: IpcError) -> FakeBridge {
        self.with_responder(Box::new(move |_| Response::Err(error.clone())))
    }

    /// Claims every command but never writes a result, leaving each request in
    /// `processing/` as an in-flight job. Use to drive the client to
    /// `IPC_TIMEOUT` while still exercising the claim.
    pub fn always_silent(self) -> FakeBridge {
        self.with_responder(Box::new(|_| Response::Silent))
    }

    /// Reports a different bridge version, for `require_bridge_version` tests.
    pub fn with_bridge_version(mut self, version: impl Into<String>) -> FakeBridge {
        self.bridge_version = version.into();
        self
    }

    /// Overrides the clock the validator compares `expires_at` against.
    pub fn set_now(&self, unix: i64) {
        *self.now.lock().expect("now lock") = unix;
    }

    /// The current fake clock.
    pub fn now(&self) -> i64 {
        *self.now.lock().expect("now lock")
    }

    /// Writes `heartbeat.json` with the given status, timestamped now.
    pub fn write_heartbeat(&self, status: &str) {
        let doc = qjson::json_obj! {
            "protocol_version" => limits::PROTOCOL_VERSION,
            "bridge_version" => self.bridge_version.clone(),
            "bridge_schema_version" => limits::BRIDGE_SCHEMA_VERSION,
            "reaper_version" => "7.22/linux-x86_64",
            "pid_token" => "9f2c1a7b4e0d63558a1140fbb2c37e91",
            "project_uuid" => "00000000-0000-4000-8000-000000000001",
            "project_name" => "MockProject",
            "project_path" => "/projects/mock.rpp",
            "ipc_dir" => self.ipc_dir.display().to_string(),
            "status" => status,
            "timestamp" => qjson::time::unix_now(),
            "timestamp_iso" => qjson::time::now_iso8601(),
            "uptime_seconds" => 412.5,
            "requests_processed" => self.processed() as i64,
            "requests_failed" => self.failed() as i64,
            "poll_interval_ms" => 50,
            "heartbeat_interval_ms" => 1000,
            "stale_after_seconds" => limits::LOCK_STALE_SECONDS,
            "commands" => Json::Arr(limits::COMMANDS.iter().map(|c| Json::Str((*c).to_string())).collect()),
        };
        let _ = self.fs.write_sync(
            &self.ipc_dir.join("heartbeat.json"),
            serialize(&doc).as_bytes(),
        );
    }

    /// Requests successfully executed.
    pub fn processed(&self) -> u64 {
        self.state.lock().expect("state lock").processed
    }

    /// Requests that produced an error envelope.
    pub fn failed(&self) -> u64 {
        self.state.lock().expect("state lock").failed
    }

    /// Command files claimed, successful or not.
    pub fn claimed(&self) -> u64 {
        self.state.lock().expect("state lock").claimed
    }

    /// Request ids the replay guard has recorded.
    pub fn seen(&self) -> Vec<String> {
        self.state.lock().expect("state lock").seen.clone()
    }

    /// Files moved to `failed/` because their stem was not a usable filename.
    pub fn quarantined(&self) -> Vec<String> {
        self.state.lock().expect("state lock").quarantined.clone()
    }

    fn path(&self, sub: &str, name: &str) -> PathBuf {
        self.ipc_dir.join(sub).join(name)
    }

    /// Lists claimable command files: names ending in exactly `.command.json`,
    /// sorted. A `.tmp` is never returned — the bridge must never read a
    /// partial write.
    pub fn list_commands(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .fs
            .list_dir(&self.ipc_dir.join("commands"))
            .unwrap_or_default()
            .into_iter()
            .filter(|n| {
                n.ends_with(limits::SUFFIX_COMMAND)
                    && n.len() > limits::SUFFIX_COMMAND.len()
                    && !n.contains('/')
                    && !n.contains('\\')
            })
            .collect();
        out.sort();
        out
    }

    /// Runs one defer tick: claims and processes up to
    /// [`limits::MAX_COMMANDS_PER_TICK`] commands. Returns how many were
    /// handled.
    pub fn pump(&self) -> usize {
        let mut handled = 0;
        for name in self.list_commands() {
            if handled >= limits::MAX_COMMANDS_PER_TICK {
                break;
            }
            if self.process_one(&name) {
                handled += 1;
            }
        }
        handled
    }

    /// Pumps until nothing is left to do, or `max_ticks` elapse.
    pub fn drain(&self, max_ticks: usize) -> usize {
        let mut total = 0;
        for _ in 0..max_ticks {
            let n = self.pump();
            total += n;
            if n == 0 {
                break;
            }
        }
        total
    }

    fn process_one(&self, name: &str) -> bool {
        let stem = &name[..name.len() - limits::SUFFIX_COMMAND.len()];
        let cmd_path = self.path("commands", name);

        // A stem that cannot be a safe filename would otherwise dictate the
        // result path. Quarantine it; no result is ever written, so the client
        // learns about it by timing out.
        if !protocol::is_safe_id(stem, limits::MAX_REQUEST_ID_LEN) {
            let target = self.path(
                "failed",
                &format!("unnamed-{}{}", self.now(), limits::SUFFIX_FAILED),
            );
            let _ = self.fs.rename(&cmd_path, &target);
            self.state
                .lock()
                .expect("state lock")
                .quarantined
                .push(name.to_string());
            return true;
        }

        // The atomic claim: exactly one renamer can win.
        let proc_path = self.path(
            "processing",
            &format!("{stem}{}", limits::SUFFIX_PROCESSING),
        );
        if self.fs.rename(&cmd_path, &proc_path).is_err() {
            return false;
        }
        self.state.lock().expect("state lock").claimed += 1;

        let started_at = qjson::time::iso8601_from_unix(self.now());
        let raw = self.fs.read(&proc_path).unwrap_or_default();
        let size = raw.len();

        let mut command: Option<String> = None;
        let mut transaction_id: Option<String> = None;
        let outcome: Result<Json, IpcError>;
        let mut silent = false;

        match String::from_utf8(raw).ok().map(|t| Json::parse(&t)) {
            None => {
                outcome = Err(IpcError::new(
                    codes::MALFORMED_REQUEST,
                    "request body is not valid UTF-8",
                ));
            }
            Some(Err(e)) => {
                outcome = if size > limits::MAX_REQUEST_BYTES {
                    Err(IpcError::with_details(
                        codes::PAYLOAD_TOO_LARGE,
                        format!(
                            "request is {size} bytes; limit is {}",
                            limits::MAX_REQUEST_BYTES
                        ),
                        qjson::json_obj! { "size" => size, "limit" => limits::MAX_REQUEST_BYTES },
                    ))
                } else {
                    Err(IpcError::with_details(
                        codes::MALFORMED_REQUEST,
                        format!("request body is not valid JSON: {e}"),
                        qjson::json_obj! { "bytes" => size },
                    ))
                };
            }
            Some(Ok(doc)) => {
                let seen = self.seen();
                let ctx = ValidationContext {
                    instance_token: &self.instance_token,
                    now: self.now(),
                    request_id_hint: Some(stem),
                    seen: &seen,
                    raw_size: Some(size),
                    bridge_version: &self.bridge_version,
                };
                match protocol::validate_envelope(&doc, &ctx) {
                    Err(e) => outcome = Err(e),
                    Ok(req) => {
                        command = Some(req.command.clone());
                        transaction_id = transaction_id_of(&req);
                        self.state
                            .lock()
                            .expect("state lock")
                            .seen
                            .push(req.request_id.clone());
                        match (self.responder)(&req) {
                            Response::Ok(v) => outcome = Ok(v),
                            Response::Err(e) => outcome = Err(e),
                            Response::Silent => {
                                silent = true;
                                outcome = Ok(Json::Obj(JsonMap::new()));
                            }
                        }
                    }
                }
            }
        }

        if silent {
            // The request stays in `processing/` exactly as it would if the
            // bridge were still working on it — or had been abandoned there.
            // The client learns nothing and must time out.
            return true;
        }

        let is_err = outcome.is_err();
        let envelope = self.make_result(
            stem,
            command.as_deref(),
            &outcome,
            transaction_id.as_deref(),
            &started_at,
        );
        self.write_result(stem, &envelope);

        {
            let mut st = self.state.lock().expect("state lock");
            if is_err {
                st.failed += 1;
            } else {
                st.processed += 1;
            }
        }

        if is_err {
            let failed = self.path("failed", &format!("{stem}{}", limits::SUFFIX_FAILED));
            if self.fs.rename(&proc_path, &failed).is_err() {
                let _ = self.fs.remove_file(&proc_path);
            }
        } else {
            let _ = self.fs.remove_file(&proc_path);
        }
        true
    }

    fn make_result(
        &self,
        request_id: &str,
        command: Option<&str>,
        outcome: &Result<Json, IpcError>,
        transaction_id: Option<&str>,
        started_at: &str,
    ) -> Json {
        let mut m = JsonMap::new();
        m.insert(
            "protocol_version",
            Json::Str(limits::PROTOCOL_VERSION.into()),
        );
        m.insert("request_id", Json::Str(request_id.into()));
        m.insert(
            "command",
            command.map(|c| Json::Str(c.into())).unwrap_or(Json::Null),
        );
        m.insert("ok", Json::Bool(outcome.is_ok()));
        match outcome {
            Ok(v) => {
                m.insert("result", v.clone());
                m.insert("error", Json::Null);
            }
            Err(e) => {
                m.insert("result", Json::Null);
                m.insert("error", e.to_json());
            }
        }
        m.insert(
            "transaction_id",
            transaction_id
                .map(|t| Json::Str(t.into()))
                .unwrap_or(Json::Null),
        );
        m.insert("bridge_version", Json::Str(self.bridge_version.clone()));
        m.insert("reaper_version", Json::Str("7.22/linux-x86_64".into()));
        m.insert("started_at", Json::Str(started_at.into()));
        m.insert(
            "completed_at",
            Json::Str(qjson::time::iso8601_from_unix(self.now())),
        );
        m.insert("duration_ms", Json::Float(1.25));
        m.insert("warnings", Json::Arr(Vec::new()));
        Json::Obj(m)
    }

    /// Publishes a result through `.result.tmp` → `.result.json`, replacing an
    /// oversized body with a `RESULT_TOO_LARGE` envelope exactly as the bridge
    /// does.
    fn write_result(&self, request_id: &str, envelope: &Json) {
        let mut body = serialize(envelope);
        if body.len() > limits::MAX_RESULT_BYTES {
            let trimmed = self.make_result(
                request_id,
                envelope.get("command").and_then(Json::as_str),
                &Err(IpcError::with_details(
                    codes::RESULT_TOO_LARGE,
                    format!(
                        "result is {} bytes; limit is {}",
                        body.len(),
                        limits::MAX_RESULT_BYTES
                    ),
                    qjson::json_obj! { "size" => body.len(), "limit" => limits::MAX_RESULT_BYTES },
                )),
                None,
                "",
            );
            body = serialize(&trimmed);
        }
        let tmp = self.path(
            "results",
            &format!("{request_id}{}", limits::SUFFIX_RESULT_TMP),
        );
        let final_path = self.path("results", &format!("{request_id}{}", limits::SUFFIX_RESULT));
        if self.fs.write_sync(&tmp, body.as_bytes()).is_ok() {
            let _ = self.fs.rename(&tmp, &final_path);
        }
    }
}

fn transaction_id_of(req: &ValidatedRequest) -> Option<String> {
    if let Some(t) = req.payload.get("transaction_id").and_then(Json::as_str) {
        return Some(t.to_string());
    }
    req.payload
        .get("plan")
        .and_then(|p| p.get("transaction_id"))
        .and_then(Json::as_str)
        .map(str::to_string)
}

/// The default responder: realistic `ping` and `status` results, and an echo
/// for everything else.
///
/// Tests that need a real snapshot or staging result install their own
/// responder; the point of the default is that a round trip works out of the
/// box.
fn default_responder(req: &ValidatedRequest) -> Response {
    match req.command.as_str() {
        "ping" => Response::Ok(qjson::json_obj! {
            "pong" => true,
            "bridge_version" => limits::BRIDGE_VERSION,
            "protocol_version" => limits::PROTOCOL_VERSION,
            "server_time" => qjson::time::now_iso8601(),
            "uptime_seconds" => 412.5,
        }),
        "status" => Response::Ok(qjson::json_obj! {
            "bridge_connected" => true,
            "bridge_version" => limits::BRIDGE_VERSION,
            "bridge_schema_version" => limits::BRIDGE_SCHEMA_VERSION,
            "ipc_protocol_version" => limits::PROTOCOL_VERSION,
            "reaper_version" => "7.22/linux-x86_64",
            "heartbeat_age_seconds" => 0,
            "heartbeat_timestamp" => qjson::time::unix_now(),
            "active_project" => true,
            "project_uuid" => "00000000-0000-4000-8000-000000000001",
            "project_name" => "MockProject",
            "project_path" => "/projects/mock.rpp",
            "play_state" => 0,
            "selected_item_count" => 1,
            "active_midi_editor" => true,
            "knowledge_version" => "2026.07.1",
            "uptime_seconds" => 412.5,
            "requests_processed" => 17,
            "requests_failed" => 1,
            "ipc_dir" => "/ipc",
            "commands" => Json::Arr(limits::COMMANDS.iter().map(|c| Json::Str((*c).to_string())).collect()),
            "limits" => limits::limits_json(),
        }),
        _ => Response::Ok(qjson::json_obj! {
            "echo_command" => req.command.clone(),
            "echo_payload" => req.payload.clone(),
        }),
    }
}

/// The four-note fixture scene, built from scratch with correct hashes.
///
/// Useful as a `Response::Ok` body for `inspect_selection` in round-trip tests.
/// The hashes are computed by [`crate::hash`], so this exercises the whole
/// canonicalisation path rather than replaying a recorded blob.
pub fn scene_snapshot() -> Json {
    let take: Vec<hash::TakeNote> = SCENE
        .iter()
        .map(|(qn, pitch, vel, sel)| hash::TakeNote {
            start_ppq: qn * 960.0,
            end_ppq: (qn + 1.0) * 960.0,
            channel: 0,
            pitch: *pitch,
            velocity: *vel,
            muted: false,
            selected: *sel,
        })
        .collect();
    let list: Vec<hash::ListNote> = SCENE
        .iter()
        .map(|(qn, pitch, vel, sel)| hash::ListNote {
            start_qn: *qn,
            end_qn: qn + 1.0,
            pitch: *pitch,
            velocity: *vel,
            channel: 0,
            muted: false,
            selected: *sel,
        })
        .collect();
    let markers = [hash::TempoMarker {
        time_seconds: 0.0,
        qn: 0.0,
        bpm: 120.0,
        timesig_num: 4,
        timesig_den: 4,
        linear: false,
    }];

    let midi_hash = hash::hash_canonical(&hash::midi_canonical(&take).expect("canonical"));
    let selection_hash =
        hash::hash_canonical(&hash::selection_canonical(&take).expect("canonical"));
    let note_list_hash =
        hash::hash_canonical(&hash::note_list_canonical(&list).expect("canonical"));
    let tempo_hash = hash::hash_canonical(&hash::tempo_canonical(&markers).expect("canonical"));
    let timesig_hash = hash::hash_canonical(&hash::timesig_canonical(&markers).expect("canonical"));

    let fields = hash::SnapshotFields {
        project_uuid: Some("00000000-0000-4000-8000-000000000001".into()),
        track_guid: Some("00000001-0001-4001-8001-000000000001".into()),
        item_guid: Some("00000002-0002-4002-8002-000000000002".into()),
        take_guid: Some("00000003-0003-4003-8003-000000000003".into()),
        item_position_seconds: 0.0,
        item_length_seconds: 4.0,
        item_position_qn: 0.0,
        item_length_qn: 8.0,
        is_loop_source: false,
        note_scope: Some("all".into()),
        extraction_mode: Some("auto".into()),
        extraction_channel: None,
        midi_hash: Some(midi_hash.clone()),
        tempo_map_hash: Some(tempo_hash.clone()),
        timesig_map_hash: Some(timesig_hash.clone()),
        note_selection_hash: Some(selection_hash.clone()),
        note_list_hash: Some(note_list_hash.clone()),
        note_count: SCENE.len() as i64,
    };
    let snapshot_hash =
        hash::hash_canonical(&hash::snapshot_canonical(&fields).expect("canonical"));

    let notes: Vec<Json> = SCENE
        .iter()
        .enumerate()
        .map(|(i, (qn, pitch, vel, sel))| {
            qjson::json_obj! {
                "index" => i as i64,
                "source_index" => i as i64,
                "start_ppq" => qn * 960.0,
                "end_ppq" => (qn + 1.0) * 960.0,
                "start_qn" => *qn,
                "end_qn" => qn + 1.0,
                "duration_qn" => 1.0,
                "item_relative_start_qn" => *qn,
                "item_relative_end_qn" => qn + 1.0,
                "start_seconds" => qn * 0.5,
                "end_seconds" => (qn + 1.0) * 0.5,
                "pitch" => *pitch,
                "velocity" => *vel,
                "channel" => 0,
                "muted" => false,
                "selected" => *sel,
            }
        })
        .collect();

    qjson::json_obj! {
        "snapshot_id" => "00000000-0000-4000-8000-0000000000aa",
        "project_pointer" => "MOCK",
        "project_uuid" => "00000000-0000-4000-8000-000000000001",
        "project_state_change_count" => 7,
        "track_guid" => "00000001-0001-4001-8001-000000000001",
        "item_guid" => "00000002-0002-4002-8002-000000000002",
        "take_guid" => "00000003-0003-4003-8003-000000000003",
        "item_position_seconds" => 0.0,
        "item_length_seconds" => 4.0,
        "item_position_qn" => 0.0,
        "item_end_qn" => 8.0,
        "item_length_qn" => 8.0,
        "is_loop_source" => false,
        "note_scope" => "all",
        "extraction_mode" => "auto",
        "extraction_channel" => Json::Null,
        "resolved_by" => "active_editor",
        "midi_hash" => midi_hash,
        "note_selection_hash" => selection_hash,
        "tempo_map_hash" => tempo_hash.clone(),
        "timesig_map_hash" => timesig_hash,
        "note_list_hash" => note_list_hash,
        "snapshot_hash" => snapshot_hash,
        "note_count" => SCENE.len() as i64,
        "source_note_count" => SCENE.len() as i64,
        "notes" => Json::Arr(notes),
        "tempo_markers" => qjson::json_arr![qjson::json_obj!{
            "index" => 0,
            "time_seconds" => 0.0,
            "qn" => 0.0,
            "bpm" => 120.0,
            "timesig_num" => 4,
            "timesig_den" => 4,
            "linear" => false,
            "synthetic" => true,
        }],
        "tempo_at_item_start" => 120.0,
        "time_signature_at_item_start" => qjson::json_obj!{ "numerator" => 4, "denominator" => 4 },
        "timestamp" => 1785091879,
        "timestamp_iso" => "2026-07-26T18:51:19Z",
        "bridge_version" => limits::BRIDGE_VERSION,
        "reaper_version" => "7.22/linux-x86_64",
        "selection_assumptions" => Json::Arr(Vec::new()),
        "warnings" => Json::Arr(Vec::new()),
    }
}

/// `(start_qn, pitch, velocity, selected)` for the four-note fixture melody.
const SCENE: [(f64, i64, i64, bool); 4] = [
    (0.0, 60, 100, true),
    (1.0, 62, 96, true),
    (2.0, 64, 92, true),
    (3.0, 65, 88, false),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemFs;
    use crate::snapshot::Snapshot;
    use std::path::Path;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn setup() -> (MemFs, FakeBridge) {
        let fs = MemFs::new();
        let bridge = FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN);
        (fs, bridge)
    }

    fn write_command(fs: &MemFs, stem: &str, doc: &Json) {
        fs.plant(format!("/ipc/commands/{stem}.command.json"), serialize(doc));
    }

    fn envelope(stem: &str, command: &str) -> Json {
        qjson::json_obj! {
            "protocol_version" => limits::PROTOCOL_VERSION,
            "request_id" => stem,
            "instance_token" => TOKEN,
            "created_at" => qjson::time::now_iso8601(),
            "expires_at" => qjson::time::iso8601_from_unix(qjson::time::unix_now() + 30),
            "command" => command,
            "payload" => qjson::json_obj!{},
        }
    }

    fn read_result(fs: &MemFs, stem: &str) -> Json {
        let bytes = fs
            .read(Path::new(&format!("/ipc/results/{stem}.result.json")))
            .expect("result file");
        Json::parse(&String::from_utf8(bytes).expect("utf8")).expect("json")
    }

    #[test]
    fn new_creates_the_whole_ipc_tree() {
        let (fs, _b) = setup();
        for sub in ["commands", "processing", "results", "failed", "logs"] {
            assert!(
                fs.list_dir(&Path::new("/ipc").join(sub)).is_ok(),
                "{sub} missing"
            );
        }
    }

    #[test]
    fn a_ping_round_trips() {
        let (fs, b) = setup();
        write_command(&fs, "r1", &envelope("r1", "ping"));
        assert_eq!(b.pump(), 1);
        let res = read_result(&fs, "r1");
        assert_eq!(res.get("ok").and_then(Json::as_bool), Some(true));
        assert_eq!(
            res.get("result")
                .and_then(|r| r.get("pong"))
                .and_then(Json::as_bool),
            Some(true)
        );
        assert_eq!(b.processed(), 1);
        assert_eq!(b.failed(), 0);
    }

    #[test]
    fn the_command_file_is_consumed_and_the_claim_is_atomic() {
        let (fs, b) = setup();
        write_command(&fs, "r1", &envelope("r1", "ping"));
        b.pump();
        assert!(fs
            .list_dir(Path::new("/ipc/commands"))
            .expect("ls")
            .is_empty());
        assert!(fs
            .list_dir(Path::new("/ipc/processing"))
            .expect("ls")
            .is_empty());
        assert_eq!(b.claimed(), 1);
        // A second pump has nothing to claim.
        assert_eq!(b.pump(), 0);
        assert_eq!(b.claimed(), 1);
    }

    #[test]
    fn a_partial_tmp_file_is_ignored() {
        let (fs, b) = setup();
        fs.plant("/ipc/commands/r1.tmp", "{\"protocol_version\": \"qlabs-rea");
        assert_eq!(b.list_commands().len(), 0);
        assert_eq!(b.pump(), 0);
        assert!(fs.exists(Path::new("/ipc/commands/r1.tmp")), "left alone");
        assert!(fs
            .list_dir(Path::new("/ipc/results"))
            .expect("ls")
            .is_empty());
    }

    #[test]
    fn results_are_published_atomically_with_no_tmp_left_behind() {
        let (fs, b) = setup();
        write_command(&fs, "r1", &envelope("r1", "ping"));
        b.pump();
        let names = fs.list_dir(Path::new("/ipc/results")).expect("ls");
        assert_eq!(names, vec!["r1.result.json"]);
        assert!(!names.iter().any(|n| n.ends_with(".result.tmp")));
    }

    #[test]
    fn an_invalid_token_is_rejected_with_the_documented_code() {
        let (fs, b) = setup();
        let mut doc = envelope("r1", "ping");
        if let Json::Obj(m) = &mut doc {
            m.insert("instance_token", Json::Str("wrong".into()));
        }
        write_command(&fs, "r1", &doc);
        b.pump();
        let res = read_result(&fs, "r1");
        assert_eq!(res.get("ok").and_then(Json::as_bool), Some(false));
        assert_eq!(
            res.get("error")
                .and_then(|e| e.get("code"))
                .and_then(Json::as_str),
            Some(codes::INVALID_INSTANCE_TOKEN)
        );
        assert_eq!(b.failed(), 1);
    }

    #[test]
    fn an_expired_request_is_rejected() {
        let (fs, b) = setup();
        let mut doc = envelope("r1", "ping");
        if let Json::Obj(m) = &mut doc {
            m.insert("expires_at", Json::Str("2020-01-01T00:00:00Z".into()));
        }
        write_command(&fs, "r1", &doc);
        b.pump();
        assert_eq!(
            read_result(&fs, "r1")
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(Json::as_str),
            Some(codes::EXPIRED_REQUEST)
        );
    }

    #[test]
    fn an_unknown_command_is_rejected_and_lists_the_allowlist() {
        let (fs, b) = setup();
        write_command(&fs, "r1", &envelope("r1", "execute_lua"));
        b.pump();
        let res = read_result(&fs, "r1");
        let err = res.get("error").expect("error");
        assert_eq!(
            err.get("code").and_then(Json::as_str),
            Some(codes::UNKNOWN_COMMAND)
        );
        assert_eq!(
            err.get("details")
                .and_then(|d| d.get("allowed"))
                .and_then(Json::as_arr)
                .map(<[Json]>::len),
            Some(limits::COMMANDS.len())
        );
        // `command` is null because the envelope never resolved to one.
        assert!(res.get("command").expect("present").is_null());
    }

    #[test]
    fn a_duplicate_request_id_is_rejected_on_the_second_attempt() {
        let (fs, b) = setup();
        write_command(&fs, "r1", &envelope("r1", "ping"));
        b.pump();
        assert_eq!(
            read_result(&fs, "r1").get("ok").and_then(Json::as_bool),
            Some(true)
        );
        fs.remove_file(Path::new("/ipc/results/r1.result.json"))
            .expect("collect");
        write_command(&fs, "r1", &envelope("r1", "ping"));
        b.pump();
        assert_eq!(
            read_result(&fs, "r1")
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(Json::as_str),
            Some(codes::DUPLICATE_REQUEST)
        );
        assert_eq!(b.seen(), vec!["r1".to_string()]);
    }

    #[test]
    fn a_request_id_that_disagrees_with_the_stem_is_rejected() {
        let (fs, b) = setup();
        write_command(&fs, "r1", &envelope("something-else", "ping"));
        b.pump();
        assert_eq!(
            read_result(&fs, "r1")
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(Json::as_str),
            Some(codes::MALFORMED_REQUEST)
        );
    }

    #[test]
    fn an_oversized_request_is_rejected_with_payload_too_large() {
        let (fs, b) = setup();
        let mut doc = envelope("r1", "ping");
        if let Json::Obj(m) = &mut doc {
            m.insert(
                "payload",
                qjson::json_obj! { "blob" => "x".repeat(limits::MAX_REQUEST_BYTES + 16) },
            );
        }
        write_command(&fs, "r1", &doc);
        b.pump();
        assert_eq!(
            read_result(&fs, "r1")
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(Json::as_str),
            Some(codes::PAYLOAD_TOO_LARGE)
        );
    }

    #[test]
    fn a_body_that_is_not_json_is_rejected_as_malformed() {
        let (fs, b) = setup();
        fs.plant("/ipc/commands/r1.command.json", "{ not json");
        b.pump();
        assert_eq!(
            read_result(&fs, "r1")
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(Json::as_str),
            Some(codes::MALFORMED_REQUEST)
        );
    }

    #[test]
    fn a_failed_request_is_quarantined_into_failed() {
        let (fs, b) = setup();
        write_command(&fs, "r1", &envelope("r1", "execute_lua"));
        b.pump();
        assert_eq!(
            fs.list_dir(Path::new("/ipc/failed")).expect("ls"),
            vec!["r1.failed.json"]
        );
    }

    #[test]
    fn a_command_file_with_an_unusable_stem_gets_no_result_at_all() {
        let (fs, b) = setup();
        // A stem the id pattern rejects; the real bridge quarantines it silently.
        fs.plant("/ipc/commands/..sneaky.command.json", "{}");
        assert_eq!(b.pump(), 1);
        assert!(fs
            .list_dir(Path::new("/ipc/results"))
            .expect("ls")
            .is_empty());
        assert_eq!(b.quarantined(), vec!["..sneaky.command.json".to_string()]);
        assert_eq!(b.claimed(), 0, "an unusable stem is never claimed");
    }

    #[test]
    fn at_most_four_commands_are_claimed_per_tick() {
        let (fs, b) = setup();
        for i in 0..10 {
            let stem = format!("r{i}");
            write_command(&fs, &stem, &envelope(&stem, "ping"));
        }
        assert_eq!(b.pump(), limits::MAX_COMMANDS_PER_TICK);
        assert_eq!(b.drain(10), 10 - limits::MAX_COMMANDS_PER_TICK);
        assert!(fs
            .list_dir(Path::new("/ipc/commands"))
            .expect("ls")
            .is_empty());
    }

    #[test]
    fn a_silent_responder_writes_no_result() {
        let fs = MemFs::new();
        let b = FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN).always_silent();
        write_command(&fs, "r1", &envelope("r1", "ping"));
        assert_eq!(b.pump(), 1);
        assert!(fs
            .list_dir(Path::new("/ipc/results"))
            .expect("ls")
            .is_empty());
        assert_eq!(b.claimed(), 1);
    }

    #[test]
    fn a_failing_responder_propagates_its_code_and_details() {
        let fs = MemFs::new();
        let b = FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN).always_failing(
            IpcError::with_details(
                codes::STALE_SNAPSHOT,
                "moved on",
                qjson::json_obj! { "expected" => "fnv1a64:aaaaaaaaaaaaaaaa" },
            ),
        );
        write_command(&fs, "r1", &envelope("r1", "ping"));
        b.pump();
        let err = read_result(&fs, "r1").get("error").cloned().expect("error");
        assert_eq!(
            err.get("code").and_then(Json::as_str),
            Some(codes::STALE_SNAPSHOT)
        );
        assert_eq!(
            err.get("details")
                .and_then(|d| d.get("expected"))
                .and_then(Json::as_str),
            Some("fnv1a64:aaaaaaaaaaaaaaaa")
        );
    }

    #[test]
    fn require_bridge_version_is_enforced_against_the_fakes_version() {
        let fs = MemFs::new();
        let b = FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN).with_bridge_version("2.0.0");
        let mut doc = envelope("r1", "ping");
        if let Json::Obj(m) = &mut doc {
            m.insert("require_bridge_version", Json::Str("1.0.0".into()));
        }
        write_command(&fs, "r1", &doc);
        b.pump();
        assert_eq!(
            read_result(&fs, "r1")
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(Json::as_str),
            Some(codes::BRIDGE_VERSION_MISMATCH)
        );
    }

    #[test]
    fn transaction_ids_are_echoed_into_the_envelope() {
        let (fs, b) = setup();
        let mut doc = envelope("r1", "commit_candidate");
        if let Json::Obj(m) = &mut doc {
            m.insert(
                "payload",
                qjson::json_obj! { "transaction_id" => "tx-0001" },
            );
        }
        write_command(&fs, "r1", &doc);
        b.pump();
        assert_eq!(
            read_result(&fs, "r1")
                .get("transaction_id")
                .and_then(Json::as_str),
            Some("tx-0001")
        );
    }

    #[test]
    fn a_transaction_id_is_also_read_out_of_a_staged_plan() {
        let (fs, b) = setup();
        let mut doc = envelope("r1", "stage_candidate");
        if let Json::Obj(m) = &mut doc {
            m.insert(
                "payload",
                qjson::json_obj! { "plan" => qjson::json_obj!{ "transaction_id" => "tx-plan" } },
            );
        }
        write_command(&fs, "r1", &doc);
        b.pump();
        assert_eq!(
            read_result(&fs, "r1")
                .get("transaction_id")
                .and_then(Json::as_str),
            Some("tx-plan")
        );
    }

    #[test]
    fn the_heartbeat_it_writes_parses_as_online() {
        let (fs, b) = setup();
        b.write_heartbeat("online");
        let bytes = fs
            .read(Path::new("/ipc/heartbeat.json"))
            .expect("heartbeat");
        let doc = Json::parse(&String::from_utf8(bytes).expect("utf8")).expect("json");
        let hb = crate::heartbeat::Heartbeat::parse(&doc, qjson::time::unix_now()).expect("online");
        assert!(hb.commands_match_this_build());
        b.write_heartbeat("offline");
        let bytes = fs
            .read(Path::new("/ipc/heartbeat.json"))
            .expect("heartbeat");
        let doc = Json::parse(&String::from_utf8(bytes).expect("utf8")).expect("json");
        assert!(crate::heartbeat::Heartbeat::parse(&doc, qjson::time::unix_now()).is_err());
    }

    #[test]
    fn the_generated_scene_snapshot_verifies_against_the_recorded_hashes() {
        let snap = Snapshot::from_json(&scene_snapshot()).expect("parse");
        snap.verify().expect("hashes reproduce");
        // The scene is the same one the recorded fixtures describe, so the
        // digests must match the recorded vectors exactly.
        assert_eq!(snap.midi_hash.as_deref(), Some("fnv1a64:fac2c019eba29541"));
        assert_eq!(
            snap.note_selection_hash.as_deref(),
            Some("fnv1a64:5fc9d7c416e376f8")
        );
        assert_eq!(
            snap.tempo_map_hash.as_deref(),
            Some("fnv1a64:387bdc9bc9f1446a")
        );
        assert_eq!(
            snap.timesig_map_hash.as_deref(),
            Some("fnv1a64:0fa04d651f4255a8")
        );
        assert_eq!(
            snap.note_list_hash.as_deref(),
            Some("fnv1a64:8a202eb33de32f29")
        );
        assert_eq!(
            snap.snapshot_hash.as_deref(),
            Some("fnv1a64:3e7bf035037bcf4f")
        );
    }

    #[test]
    fn set_now_moves_the_validators_clock() {
        let (fs, b) = setup();
        b.set_now(qjson::time::unix_now() + 3600);
        write_command(&fs, "r1", &envelope("r1", "ping"));
        b.pump();
        assert_eq!(
            read_result(&fs, "r1")
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(Json::as_str),
            Some(codes::EXPIRED_REQUEST)
        );
    }
}
