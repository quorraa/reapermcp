//! [`BridgeClient`] — the Rust half of the file IPC.
//!
//! # Write discipline
//!
//! A request is written to `<ipc-dir>/commands/<id>.tmp`, flushed and
//! `sync_all()`ed, then renamed to `<id>.command.json` **in the same
//! directory**, so the rename is same-filesystem and therefore atomic. The
//! bridge only ever lists names ending in exactly `.command.json`, so a partial
//! write is invisible to it. The temp file is never staged in a system temp
//! directory.
//!
//! # What the client may touch
//!
//! Writes go only into `commands/`. Deletes go only into `results/` (plus its
//! own unclaimed command file). `processing/`, `failed/` and `logs/` are
//! bridge-owned and are never read as protocol data, never written, never
//! renamed within, never deleted from. `heartbeat.json`, `bridge.lock` and
//! `config.json` are read-only.
//!
//! # The only value that reaches a path
//!
//! `request_id` is minted here with [`qjson::uuid`] and validated against the
//! wire pattern before any path is constructed. No caller-supplied path or id
//! is ever accepted.

use crate::cancel::CancelFlag;
use crate::config::IpcConfig;
use crate::envelope::{ExpectedProject, RequestEnvelope, ResultEnvelope};
use crate::error::{codes, IpcError};
use crate::fs::{Filesystem, RealFs};
use crate::heartbeat::Heartbeat;
use crate::limits;
use crate::snapshot::Snapshot;
use music_domain::plan::EditPlan;
use qjson::uuid::UuidGen;
use qjson::{Json, JsonMap};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A hook invoked immediately before each poll of `results/`.
///
/// Exists so tests can drive a [`crate::testing::FakeBridge`] in lockstep with
/// the polling loop, making round-trip, timeout and cancellation tests fully
/// deterministic instead of dependent on thread scheduling.
pub type PollHook = Arc<dyn Fn(u32) + Send + Sync>;

/// The client. One instance per bridge installation.
///
/// Cheap to share: everything inside is behind an [`Arc`], and the type is
/// `Send + Sync`, so the MCP server can hold one and call it from a request
/// thread while a heartbeat thread reads liveness.
pub struct BridgeClient {
    cfg: IpcConfig,
    fs: Arc<dyn Filesystem>,
    ids: Mutex<UuidGen>,
    poll_hook: Option<PollHook>,
}

impl std::fmt::Debug for BridgeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeClient")
            .field("ipc_dir", &self.cfg.ipc_dir)
            .field("request_timeout", &self.cfg.request_timeout)
            .field("poll_interval", &self.cfg.poll_interval)
            .finish_non_exhaustive()
    }
}

impl BridgeClient {
    /// Builds a client over the real filesystem.
    ///
    /// Validates the configuration and creates `<ipc-dir>/commands/` if it is
    /// missing — the bridge creates the whole tree at startup, but the MCP
    /// server may run first.
    pub fn new(cfg: IpcConfig) -> Result<BridgeClient, IpcError> {
        BridgeClient::with_fs(cfg, Arc::new(RealFs))
    }

    /// Builds a client over a supplied [`Filesystem`].
    pub fn with_fs(cfg: IpcConfig, fs: Arc<dyn Filesystem>) -> Result<BridgeClient, IpcError> {
        cfg.validate()?;
        let commands = cfg.commands_dir();
        fs.create_dir_all(&commands)
            .map_err(|e| IpcError::io("could not create the commands directory", &commands, &e))?;
        Ok(BridgeClient {
            cfg,
            fs,
            ids: Mutex::new(UuidGen::process_unique()),
            poll_hook: None,
        })
    }

    /// Installs a hook run before every poll. Test seam; see [`PollHook`].
    pub fn with_poll_hook(mut self, hook: PollHook) -> BridgeClient {
        self.poll_hook = Some(hook);
        self
    }

    /// The configuration this client was built with.
    pub fn config(&self) -> &IpcConfig {
        &self.cfg
    }

    // ---- liveness --------------------------------------------------------

    /// Reads `heartbeat.json`, computes its age and validates the protocol
    /// version.
    ///
    /// A missing or unparseable file is
    /// [`BRIDGE_OFFLINE`][codes::BRIDGE_OFFLINE]; a bridge speaking a different
    /// protocol is [`IPC_PROTOCOL_MISMATCH`][codes::IPC_PROTOCOL_MISMATCH].
    pub fn heartbeat(&self) -> Result<Heartbeat, IpcError> {
        let path = self.cfg.heartbeat_path();
        let bytes = self.fs.read(&path).map_err(|e| {
            IpcError::with_details(
                codes::BRIDGE_OFFLINE,
                format!("heartbeat.json could not be read: {e}"),
                qjson::json_obj! { "path" => path.display().to_string() },
            )
        })?;
        let text = String::from_utf8(bytes).map_err(|_| {
            IpcError::new(codes::BRIDGE_OFFLINE, "heartbeat.json is not valid UTF-8")
        })?;
        let doc = Json::parse(&text).map_err(|e| {
            IpcError::new(
                codes::BRIDGE_OFFLINE,
                format!("heartbeat.json is not valid JSON: {e}"),
            )
        })?;
        Heartbeat::parse(&doc, qjson::time::unix_now())
    }

    /// True when the bridge is online by the §11 liveness rule.
    ///
    /// Deliberately swallows the reason. Call [`BridgeClient::heartbeat`] when
    /// you need to tell the user *why*.
    pub fn is_online(&self) -> bool {
        self.heartbeat().is_ok()
    }

    /// Reads `bridge.lock`, for diagnostics only. Never written or deleted.
    pub fn read_lock(&self) -> Result<Json, IpcError> {
        let path = self.cfg.lock_path();
        let bytes = self
            .fs
            .read(&path)
            .map_err(|e| IpcError::io("could not read bridge.lock", &path, &e))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| IpcError::new(codes::BRIDGE_OFFLINE, "bridge.lock is not valid UTF-8"))?;
        Json::parse(&text)
            .map_err(|e| IpcError::new(codes::BRIDGE_OFFLINE, format!("bridge.lock: {e}")))
    }

    // ---- the call --------------------------------------------------------

    /// Writes a command and waits for its result.
    ///
    /// Sequence: liveness check, envelope build and local validation, size
    /// check, `<id>.tmp` → fsync → rename to `<id>.command.json`, then poll
    /// `results/<id>.result.json` with bounded exponential backoff until the
    /// deadline.
    ///
    /// The backoff starts at a tenth of [`IpcConfig::poll_interval`] (floor
    /// 1 ms) and doubles up to that interval, so a fast reply is picked up
    /// almost immediately while a slow one costs a handful of syscalls per
    /// second rather than a spin. Each iteration checks `cancel` first.
    ///
    /// On timeout or cancellation the command file is removed if it is still
    /// unclaimed, and a structured
    /// [`IPC_TIMEOUT`][codes::IPC_TIMEOUT] / [`IPC_CANCELLED`][codes::IPC_CANCELLED]
    /// is returned.
    pub fn call(
        &self,
        command: &str,
        payload: Json,
        expected: Option<&ExpectedProject>,
        cancel: &CancelFlag,
    ) -> Result<Json, IpcError> {
        if cancel.is_cancelled() {
            return Err(cancelled("call was cancelled before it started"));
        }

        // §14.2 — no command file is written when the bridge is not there.
        self.heartbeat()?;

        let request_id = self.mint_request_id()?;
        let now = qjson::time::unix_now();
        let timeout_secs = self.cfg.request_timeout.as_secs().max(1) as i64;
        let envelope = RequestEnvelope::build(
            &request_id,
            &self.cfg.instance_token,
            command,
            payload,
            expected,
            now,
            now + timeout_secs,
        )?;

        let bytes = envelope.to_bytes();
        if bytes.len() > self.cfg.max_request_bytes {
            // §14.5 — fail locally rather than writing a file the bridge will
            // only reject.
            return Err(IpcError::with_details(
                codes::PAYLOAD_TOO_LARGE,
                format!(
                    "request is {} bytes; limit is {}",
                    bytes.len(),
                    self.cfg.max_request_bytes
                ),
                qjson::json_obj! { "size" => bytes.len(), "limit" => self.cfg.max_request_bytes },
            ));
        }

        self.publish_command(&request_id, &bytes)?;
        self.await_result(&request_id, cancel)
    }

    /// Mints a fresh `request_id` and checks it does not collide with a command
    /// file already on disk.
    fn mint_request_id(&self) -> Result<String, IpcError> {
        for _ in 0..8 {
            let id = {
                let g = self.ids.lock().map_err(|_| {
                    IpcError::new(codes::INTERNAL_BRIDGE_ERROR, "id generator lock poisoned")
                })?;
                g.next()
            };
            crate::protocol::validate_request_id(&id)?;
            let path = self.cfg.command_path(&id)?;
            if !self.fs.exists(&path) {
                return Ok(id);
            }
        }
        Err(IpcError::new(
            codes::INTERNAL_BRIDGE_ERROR,
            "could not mint an unused request id",
        ))
    }

    /// `<id>.tmp` → fsync → rename to `<id>.command.json`.
    fn publish_command(&self, id: &str, bytes: &[u8]) -> Result<(), IpcError> {
        let dir = self.cfg.commands_dir();
        self.fs
            .create_dir_all(&dir)
            .map_err(|e| IpcError::io("could not create the commands directory", &dir, &e))?;

        let tmp = self.cfg.command_tmp_path(id)?;
        let final_path = self.cfg.command_path(id)?;

        self.fs
            .write_sync(&tmp, bytes)
            .map_err(|e| IpcError::io("could not write the command file", &tmp, &e))?;

        if let Err(e) = self.fs.rename(&tmp, &final_path) {
            // Leave nothing half-written behind; the bridge's GC would sweep a
            // stray `.tmp` eventually, but not for a minute.
            let _ = self.fs.remove_file(&tmp);
            return Err(IpcError::io(
                "could not publish the command file",
                &final_path,
                &e,
            ));
        }
        Ok(())
    }

    /// Polls `results/<id>.result.json` until it appears, the caller cancels,
    /// or the deadline passes.
    fn await_result(&self, id: &str, cancel: &CancelFlag) -> Result<Json, IpcError> {
        let cap = self.cfg.poll_interval;
        let mut wait = (cap / 10).max(Duration::from_millis(1)).min(cap);
        let started = Instant::now();
        let deadline = started + self.cfg.request_timeout;
        let mut attempt: u32 = 0;

        loop {
            if let Some(hook) = &self.poll_hook {
                hook(attempt);
            }

            if let Some(value) = self.take_result(id)? {
                return value;
            }

            if cancel.is_cancelled() {
                self.cleanup_unclaimed(id);
                return Err(cancelled(format!(
                    "call {id} was cancelled after {} ms",
                    started.elapsed().as_millis()
                )));
            }

            let now = Instant::now();
            if now >= deadline {
                break;
            }
            std::thread::sleep(wait.min(deadline - now));
            wait = (wait * 2).min(cap);
            attempt = attempt.saturating_add(1);
        }

        // One last look: the bridge may have landed the result inside the final
        // sleep, and losing it would be worse than being a microsecond late.
        if let Some(hook) = &self.poll_hook {
            hook(attempt);
        }
        if let Some(value) = self.take_result(id)? {
            return value;
        }

        self.cleanup_unclaimed(id);
        Err(IpcError::with_details(
            codes::IPC_TIMEOUT,
            format!(
                "no result for {id} within {} ms",
                self.cfg.request_timeout.as_millis()
            ),
            qjson::json_obj! {
                "request_id" => id,
                "timeout_ms" => self.cfg.request_timeout.as_millis() as i64,
                "polls" => attempt as i64,
            },
        ))
    }

    /// Reads, deletes and interprets `<id>.result.json` if it exists.
    ///
    /// The outer `Result` is a failure to *read* the result; the inner one is
    /// the call's own outcome. Only `<id>.result.json` is ever opened — never a
    /// `.tmp`, and never another request's result.
    #[allow(clippy::type_complexity)] // the nesting is the point: read failure vs call outcome.
    fn take_result(&self, id: &str) -> Result<Option<Result<Json, IpcError>>, IpcError> {
        let path = self.cfg.result_path(id)?;
        if !self.fs.exists(&path) {
            return Ok(None);
        }

        let len = match self.fs.file_len(&path) {
            Ok(n) => n,
            // Vanished between the check and the stat: treat as not-yet-there.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(IpcError::io("could not stat the result file", &path, &e)),
        };

        if len as usize > self.cfg.max_result_bytes {
            self.delete_result(&path);
            return Err(IpcError::with_details(
                codes::RESULT_TOO_LARGE,
                format!(
                    "result is {len} bytes; limit is {}",
                    self.cfg.max_result_bytes
                ),
                qjson::json_obj! { "size" => len as i64, "limit" => self.cfg.max_result_bytes },
            ));
        }

        let bytes = match self.fs.read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(IpcError::io("could not read the result file", &path, &e)),
        };

        // §2 — the result is ours now; remove it whatever it turns out to say.
        self.delete_result(&path);

        Ok(Some(self.interpret_result(id, &bytes)))
    }

    fn interpret_result(&self, id: &str, bytes: &[u8]) -> Result<Json, IpcError> {
        let text = String::from_utf8(bytes.to_vec()).map_err(|_| {
            IpcError::new(
                codes::INTERNAL_BRIDGE_ERROR,
                "result file is not valid UTF-8",
            )
        })?;
        let doc = Json::parse(&text).map_err(|e| {
            IpcError::new(
                codes::INTERNAL_BRIDGE_ERROR,
                format!("result file is not valid JSON: {e}"),
            )
        })?;
        let envelope = ResultEnvelope::parse(&doc)?;
        if envelope.request_id != id {
            return Err(IpcError::with_details(
                codes::INTERNAL_BRIDGE_ERROR,
                format!(
                    "result for {id} carries request_id {:?}",
                    envelope.request_id
                ),
                qjson::json_obj! { "expected" => id, "received" => envelope.request_id.clone() },
            ));
        }
        envelope.into_outcome()
    }

    fn delete_result(&self, path: &Path) {
        // Uncollected results are garbage-collected by the bridge after
        // STALE_RESULT_SECONDS, so a failure here is not worth surfacing.
        let _ = self.fs.remove_file(path);
    }

    /// Removes this call's own command file if the bridge has not claimed it.
    ///
    /// Only `commands/<id>.command.json` — never anything in `processing/`. If
    /// the bridge already claimed the file, the rename moved it away and this
    /// is a no-op.
    fn cleanup_unclaimed(&self, id: &str) {
        if let Ok(path) = self.cfg.command_path(id) {
            if self.fs.exists(&path) {
                let _ = self.fs.remove_file(&path);
            }
        }
    }

    /// Reads and deletes a result the client already gave up on.
    ///
    /// Returns `true` when one was found. The wire spec requires the client to
    /// tolerate a late result: read it, discard it, delete it.
    pub fn discard_result(&self, request_id: &str) -> Result<bool, IpcError> {
        let path = self.cfg.result_path(request_id)?;
        if !self.fs.exists(&path) {
            return Ok(false);
        }
        self.delete_result(&path);
        Ok(true)
    }

    // ---- commands --------------------------------------------------------

    /// `ping` — liveness with a round trip.
    pub fn ping(&self) -> Result<Json, IpcError> {
        self.call("ping", Json::Null, None, &CancelFlag::new())
    }

    /// `status` — the bridge's full self-report.
    pub fn status(&self) -> Result<Json, IpcError> {
        self.call("status", Json::Null, None, &CancelFlag::new())
    }

    /// `inspect_selection` — builds a snapshot of the current MIDI selection.
    pub fn inspect_selection(&self, payload: Json) -> Result<Json, IpcError> {
        self.call("inspect_selection", payload, None, &CancelFlag::new())
    }

    /// `inspect_selection`, parsed and hash-verified.
    ///
    /// Every hash that can be recomputed from the data the bridge sent is
    /// recomputed. A hash that does not reproduce fails the call with
    /// [`SNAPSHOT_HASH_MISMATCH`][codes::SNAPSHOT_HASH_MISMATCH] rather than
    /// handing back a snapshot that cannot be trusted as a staleness token.
    pub fn inspect_snapshot(
        &self,
        payload: Json,
        expected: Option<&ExpectedProject>,
        cancel: &CancelFlag,
    ) -> Result<Snapshot, IpcError> {
        let value = self.call("inspect_selection", payload, expected, cancel)?;
        let snapshot = Snapshot::from_json(&value)?;
        snapshot.verify()?;
        Ok(snapshot)
    }

    /// `stage_candidate` — writes the plan into the project as a muted preview.
    ///
    /// `payload` may carry `verify_snapshot`, `source_mode`, `note_scope` and
    /// `melody_extraction`; `plan` is inserted from `plan` and overwrites any
    /// key of that name the caller supplied.
    pub fn stage_candidate(&self, plan: &EditPlan, payload: Json) -> Result<Json, IpcError> {
        preflight_plan(plan)?;
        let mut map = match payload {
            Json::Null => JsonMap::new(),
            Json::Obj(m) => m,
            other => {
                return Err(IpcError::with_details(
                    codes::MALFORMED_REQUEST,
                    "stage_candidate payload must be a JSON object",
                    qjson::json_obj! { "type" => other.type_name() },
                ))
            }
        };
        map.insert("plan", crate::plan::plan_to_wire(plan));
        self.call("stage_candidate", Json::Obj(map), None, &CancelFlag::new())
    }

    /// `commit_candidate` — flips a staged transaction's objects to committed.
    pub fn commit_candidate(&self, transaction_id: &str) -> Result<Json, IpcError> {
        self.transaction_call("commit_candidate", transaction_id)
    }

    /// `discard_candidate` — deletes only the objects this transaction owns.
    pub fn discard_candidate(&self, transaction_id: &str) -> Result<Json, IpcError> {
        self.transaction_call("discard_candidate", transaction_id)
    }

    /// `undo_last_generation` — undoes at most one owned undo entry.
    pub fn undo_last_generation(&self, transaction_id: &str) -> Result<Json, IpcError> {
        self.transaction_call("undo_last_generation", transaction_id)
    }

    fn transaction_call(&self, command: &str, transaction_id: &str) -> Result<Json, IpcError> {
        if !crate::protocol::is_safe_id(transaction_id, limits::MAX_ID_LEN) {
            return Err(IpcError::with_details(
                codes::MALFORMED_REQUEST,
                format!("transaction_id must match {}", crate::protocol::ID_PATTERN),
                qjson::json_obj! { "transaction_id" => transaction_id },
            ));
        }
        self.call(
            command,
            qjson::json_obj! { "transaction_id" => transaction_id },
            None,
            &CancelFlag::new(),
        )
    }
}

fn cancelled(message: impl Into<String>) -> IpcError {
    IpcError::new(codes::IPC_CANCELLED, message)
}

/// Local structural checks on an [`EditPlan`] before it goes on the wire.
///
/// These duplicate a subset of the bridge's validation on purpose: a plan that
/// cannot possibly be accepted should fail here, where the error is cheap and
/// the message names the plan, rather than after a round trip.
pub fn preflight_plan(plan: &EditPlan) -> Result<(), IpcError> {
    let invalid = |m: String| {
        IpcError::with_details(
            codes::INVALID_EDIT_PLAN,
            m,
            qjson::json_obj! { "plan_id" => plan.plan_id.clone() },
        )
    };

    if !crate::protocol::is_safe_id(&plan.transaction_id, limits::MAX_ID_LEN) {
        return Err(invalid(format!(
            "transaction_id must match {}",
            crate::protocol::ID_PATTERN
        )));
    }
    if plan.plan_id.is_empty() || plan.plan_id.len() > limits::MAX_ID_LEN {
        return Err(invalid(format!(
            "plan_id must be 1..={} bytes",
            limits::MAX_ID_LEN
        )));
    }
    if plan.candidate_id.is_empty() || plan.candidate_id.len() > limits::MAX_ID_LEN {
        return Err(invalid(format!(
            "candidate_id must be 1..={} bytes",
            limits::MAX_ID_LEN
        )));
    }
    if plan.base_snapshot_id.is_empty() || plan.base_snapshot_id.len() > limits::MAX_ID_LEN {
        return Err(invalid(format!(
            "base_snapshot_id must be 1..={} bytes",
            limits::MAX_ID_LEN
        )));
    }
    if plan.base_snapshot_hash.is_empty() || plan.base_snapshot_hash.len() > limits::MAX_ID_LEN {
        return Err(invalid("base_snapshot_hash must be 1..=128 bytes".into()));
    }
    if plan.project_uuid.is_empty() {
        return Err(invalid("project_uuid is required".into()));
    }
    if !plan.undo_label.starts_with(limits::UNDO_LABEL_PREFIX) {
        // The bridge refuses to open an undo block with any other label, and
        // undo_last_generation refuses to undo an entry without it.
        return Err(invalid(format!(
            "undo_label must begin with {:?}",
            limits::UNDO_LABEL_PREFIX
        )));
    }
    if plan.undo_label.len() > limits::MAX_UNDO_LABEL_BYTES {
        return Err(invalid(format!(
            "undo_label must be at most {} bytes",
            limits::MAX_UNDO_LABEL_BYTES
        )));
    }
    if plan.undo_label.chars().any(|c| c.is_control()) {
        return Err(invalid(
            "undo_label must not contain control characters".into(),
        ));
    }
    if !is_valid_knowledge_version(&plan.knowledge_version) {
        return Err(IpcError::with_details(
            codes::KNOWLEDGE_INVALID,
            "knowledge_version must match ^[A-Za-z0-9._+-]{1,64}$",
            qjson::json_obj! { "knowledge_version" => plan.knowledge_version.clone() },
        ));
    }
    crate::plan::check_tag_keys(plan)?;
    if plan.operations.is_empty() {
        return Err(invalid("a plan must carry at least one operation".into()));
    }
    if plan.operations.len() > limits::MAX_OPERATIONS {
        return Err(invalid(format!(
            "a plan may carry at most {} operations",
            limits::MAX_OPERATIONS
        )));
    }
    if plan.preconditions.len() > limits::MAX_PRECONDITIONS {
        return Err(invalid(format!(
            "a plan may carry at most {} preconditions",
            limits::MAX_PRECONDITIONS
        )));
    }
    if plan.expected_outputs.len() > limits::MAX_EXPECTED_OUTPUTS {
        return Err(invalid(format!(
            "a plan may carry at most {} expected outputs",
            limits::MAX_EXPECTED_OUTPUTS
        )));
    }
    let notes: usize = plan
        .operations
        .iter()
        .map(|op| match op {
            music_domain::plan::EditOperation::InsertNotes { notes, .. } => notes.len(),
            _ => 0,
        })
        .sum();
    if notes > limits::MAX_GENERATED_NOTES {
        return Err(invalid(format!(
            "a plan may insert at most {} notes; this one inserts {notes}",
            limits::MAX_GENERATED_NOTES
        )));
    }
    Ok(())
}

fn is_valid_knowledge_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemFs;
    use music_domain::plan::{EditOperation, PlannedNote};
    use music_domain::time::BeatTime;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn online_heartbeat() -> String {
        qjson::json_obj! {
            "protocol_version" => limits::PROTOCOL_VERSION,
            "bridge_version" => "1.0.0",
            "bridge_schema_version" => "1",
            "reaper_version" => "7.22/linux-x86_64",
            "pid_token" => "9f2c1a7b4e0d63558a1140fbb2c37e91",
            "project_uuid" => "00000000-0000-4000-8000-000000000001",
            "ipc_dir" => "/ipc",
            "status" => "online",
            "timestamp" => qjson::time::unix_now(),
            "stale_after_seconds" => 10,
        }
        .to_string()
    }

    fn client_with(fs: MemFs, timeout_ms: u64) -> (BridgeClient, MemFs) {
        fs.plant("/ipc/heartbeat.json", online_heartbeat());
        let cfg = IpcConfig::new("/ipc", TOKEN)
            .with_request_timeout(Duration::from_millis(timeout_ms))
            .with_poll_interval(Duration::from_millis(4));
        let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone())).expect("client");
        (client, fs)
    }

    fn valid_plan() -> EditPlan {
        EditPlan {
            plan_id: "plan-1".into(),
            candidate_id: "cand-1".into(),
            transaction_id: "tx-0001".into(),
            base_snapshot_id: "snap-1".into(),
            base_snapshot_hash: "fnv1a64:3e7bf035037bcf4f".into(),
            project_uuid: "00000000-0000-4000-8000-000000000001".into(),
            knowledge_version: "2026.07.1".into(),
            undo_label: "QLabs MCP: Stage candidate ab12cd34".into(),
            operations: vec![EditOperation::CreateTrack {
                temp_id: "t0".into(),
                parent: None,
                name: "Chords".into(),
                tags: vec![],
            }],
            preconditions: vec![],
            expected_outputs: vec![],
        }
    }

    #[test]
    fn new_creates_the_commands_directory() {
        let fs = MemFs::new();
        let (_client, fs) = client_with(fs, 20);
        assert!(fs.list_dir(Path::new("/ipc/commands")).is_ok());
    }

    #[test]
    fn with_fs_rejects_an_invalid_configuration() {
        let cfg = IpcConfig::new("", TOKEN);
        assert!(BridgeClient::with_fs(cfg, Arc::new(MemFs::new())).is_err());
    }

    #[test]
    fn heartbeat_reports_online_and_computes_age() {
        let (client, _fs) = client_with(MemFs::new(), 20);
        let hb = client.heartbeat().expect("online");
        assert_eq!(hb.bridge_version, "1.0.0");
        assert!(client.is_online());
    }

    #[test]
    fn a_missing_heartbeat_is_bridge_offline() {
        let fs = MemFs::new();
        fs.create_dir_all(Path::new("/ipc/commands"))
            .expect("mkdir");
        let cfg = IpcConfig::new("/ipc", TOKEN);
        let client = BridgeClient::with_fs(cfg, Arc::new(fs)).expect("client");
        let err = client.heartbeat().expect_err("offline");
        assert!(err.is_bridge_offline());
        assert!(!client.is_online());
    }

    #[test]
    fn an_unparseable_heartbeat_is_bridge_offline() {
        let fs = MemFs::new();
        fs.plant("/ipc/heartbeat.json", "{ not json");
        let cfg = IpcConfig::new("/ipc", TOKEN);
        let client = BridgeClient::with_fs(cfg, Arc::new(fs)).expect("client");
        assert!(client.heartbeat().expect_err("offline").is_bridge_offline());
    }

    #[test]
    fn call_refuses_to_write_when_the_bridge_is_offline() {
        let fs = MemFs::new();
        fs.plant("/ipc/heartbeat.json", "{\"status\":\"offline\"}");
        let cfg = IpcConfig::new("/ipc", TOKEN).with_request_timeout(Duration::from_millis(10));
        let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone())).expect("client");
        let err = client
            .call("ping", Json::Null, None, &CancelFlag::new())
            .expect_err("offline");
        assert!(err.is_bridge_offline());
        assert!(
            fs.list_dir(Path::new("/ipc/commands"))
                .expect("ls")
                .is_empty(),
            "no command file may be written when the bridge is offline"
        );
    }

    #[test]
    fn call_writes_a_command_file_through_tmp_and_rename() {
        let (client, fs) = client_with(MemFs::new(), 15);
        let before = fs.sync_count();
        let err = client
            .call("ping", Json::Null, None, &CancelFlag::new())
            .expect_err("no bridge to answer");
        assert!(err.is_timeout());
        assert!(fs.sync_count() > before, "the tmp file must be fsynced");
    }

    #[test]
    fn the_published_command_file_is_a_valid_envelope_named_after_its_id() {
        let fs = MemFs::new();
        // A hook that captures the directory the moment the first poll runs,
        // i.e. immediately after the rename and before timeout cleanup.
        /// `(file name, contents)` pairs seen in `commands/` during a poll.
        type Captured = Arc<Mutex<Vec<(String, Vec<u8>)>>>;
        let captured: Captured = Arc::new(Mutex::new(Vec::new()));
        let fs2 = fs.clone();
        let cap2 = captured.clone();
        fs.plant("/ipc/heartbeat.json", online_heartbeat());
        let cfg = IpcConfig::new("/ipc", TOKEN)
            .with_request_timeout(Duration::from_millis(10))
            .with_poll_interval(Duration::from_millis(2));
        let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone()))
            .expect("client")
            .with_poll_hook(Arc::new(move |_| {
                for name in fs2.list_dir(Path::new("/ipc/commands")).unwrap_or_default() {
                    if let Ok(bytes) = fs2.read(Path::new(&format!("/ipc/commands/{name}"))) {
                        cap2.lock().expect("lock").push((name, bytes));
                    }
                }
            }));
        let _ = client.call("ping", Json::Null, None, &CancelFlag::new());
        let files = captured.lock().expect("lock").clone();
        let (name, bytes) = files.first().expect("a command file").clone();
        assert!(name.ends_with(".command.json"), "{name}");
        assert!(!name.ends_with(".tmp"), "{name}");
        let stem = name.trim_end_matches(".command.json").to_string();
        let doc = Json::parse(&String::from_utf8(bytes).expect("utf8")).expect("json");
        assert_eq!(
            doc.get("request_id").and_then(Json::as_str),
            Some(stem.as_str())
        );
        assert_eq!(
            doc.get("protocol_version").and_then(Json::as_str),
            Some(limits::PROTOCOL_VERSION)
        );
        assert_eq!(doc.get("command").and_then(Json::as_str), Some("ping"));
        assert_eq!(
            doc.get("instance_token").and_then(Json::as_str),
            Some(TOKEN)
        );
        crate::protocol::validate_request_id(&stem).expect("minted id is path-safe");
    }

    #[test]
    fn no_tmp_file_survives_a_successful_publish() {
        let (client, fs) = client_with(MemFs::new(), 10);
        let _ = client.call("ping", Json::Null, None, &CancelFlag::new());
        for p in fs.paths() {
            assert!(
                !p.to_string_lossy().ends_with(".tmp"),
                "stray tmp file: {}",
                p.display()
            );
        }
    }

    #[test]
    fn a_timeout_returns_a_structured_error_and_cleans_up() {
        let (client, fs) = client_with(MemFs::new(), 12);
        let err = client
            .call("ping", Json::Null, None, &CancelFlag::new())
            .expect_err("timeout");
        assert_eq!(err.code, codes::IPC_TIMEOUT);
        assert!(err.is_timeout());
        assert!(err.details.get("timeout_ms").is_some());
        assert!(
            fs.list_dir(Path::new("/ipc/commands"))
                .expect("ls")
                .is_empty(),
            "an unclaimed command file must be cleaned up on timeout"
        );
    }

    #[test]
    fn a_timeout_leaves_a_claimed_command_file_alone() {
        let fs = MemFs::new();
        fs.plant("/ipc/heartbeat.json", online_heartbeat());
        fs.create_dir_all(Path::new("/ipc/processing"))
            .expect("mkdir");
        let fs2 = fs.clone();
        let cfg = IpcConfig::new("/ipc", TOKEN)
            .with_request_timeout(Duration::from_millis(12))
            .with_poll_interval(Duration::from_millis(2));
        let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone()))
            .expect("client")
            .with_poll_hook(Arc::new(move |attempt| {
                if attempt == 0 {
                    // Claim it the way bridge.lua does: one rename.
                    for name in fs2.list_dir(Path::new("/ipc/commands")).unwrap_or_default() {
                        let stem = name.trim_end_matches(".command.json");
                        let _ = fs2.rename(
                            Path::new(&format!("/ipc/commands/{name}")),
                            Path::new(&format!("/ipc/processing/{stem}.processing.json")),
                        );
                    }
                }
            }));
        let err = client
            .call("ping", Json::Null, None, &CancelFlag::new())
            .expect_err("timeout");
        assert!(err.is_timeout());
        assert_eq!(
            fs.list_dir(Path::new("/ipc/processing")).expect("ls").len(),
            1,
            "the claimed file must be left where the bridge put it"
        );
    }

    #[test]
    fn cancellation_before_the_call_writes_nothing() {
        let (client, fs) = client_with(MemFs::new(), 20);
        let cancel = CancelFlag::new();
        cancel.cancel();
        let err = client
            .call("ping", Json::Null, None, &cancel)
            .expect_err("cancelled");
        assert_eq!(err.code, codes::IPC_CANCELLED);
        assert!(fs
            .list_dir(Path::new("/ipc/commands"))
            .expect("ls")
            .is_empty());
    }

    #[test]
    fn cancellation_mid_poll_stops_early_and_cleans_up() {
        let fs = MemFs::new();
        fs.plant("/ipc/heartbeat.json", online_heartbeat());
        let cancel = CancelFlag::new();
        let c2 = cancel.clone();
        let cfg = IpcConfig::new("/ipc", TOKEN)
            // A long timeout the test must not actually wait for.
            .with_request_timeout(Duration::from_secs(30))
            .with_poll_interval(Duration::from_millis(2));
        let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone()))
            .expect("client")
            .with_poll_hook(Arc::new(move |attempt| {
                if attempt == 2 {
                    c2.cancel();
                }
            }));
        let started = Instant::now();
        let err = client
            .call("ping", Json::Null, None, &cancel)
            .expect_err("cancelled");
        assert_eq!(err.code, codes::IPC_CANCELLED);
        assert!(err.is_cancelled());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cancellation must not wait for the deadline"
        );
        assert!(fs
            .list_dir(Path::new("/ipc/commands"))
            .expect("ls")
            .is_empty());
    }

    #[test]
    fn an_oversized_request_fails_locally_without_writing() {
        let fs = MemFs::new();
        fs.plant("/ipc/heartbeat.json", online_heartbeat());
        let mut cfg = IpcConfig::new("/ipc", TOKEN).with_request_timeout(Duration::from_millis(10));
        cfg.max_request_bytes = 200;
        let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone())).expect("client");
        let payload = qjson::json_obj! { "blob" => "x".repeat(4096) };
        let err = client
            .call("ping", payload, None, &CancelFlag::new())
            .expect_err("too large");
        assert_eq!(err.code, codes::PAYLOAD_TOO_LARGE);
        assert!(
            fs.list_dir(Path::new("/ipc/commands"))
                .expect("ls")
                .is_empty(),
            "an oversized request must never reach the filesystem"
        );
    }

    #[test]
    fn an_unknown_command_fails_locally_without_writing() {
        let (client, fs) = client_with(MemFs::new(), 10);
        let err = client
            .call("execute_lua", Json::Null, None, &CancelFlag::new())
            .expect_err("unknown");
        assert_eq!(err.code, codes::UNKNOWN_COMMAND);
        assert!(fs
            .list_dir(Path::new("/ipc/commands"))
            .expect("ls")
            .is_empty());
    }

    #[test]
    fn a_non_object_payload_fails_locally() {
        let (client, _fs) = client_with(MemFs::new(), 10);
        let err = client
            .call(
                "ping",
                Json::Arr(vec![Json::Int(1)]),
                None,
                &CancelFlag::new(),
            )
            .expect_err("array payload");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn an_oversized_result_is_refused_and_deleted() {
        let fs = MemFs::new();
        fs.plant("/ipc/heartbeat.json", online_heartbeat());
        fs.create_dir_all(Path::new("/ipc/results")).expect("mkdir");
        let fs2 = fs.clone();
        let mut cfg = IpcConfig::new("/ipc", TOKEN)
            .with_request_timeout(Duration::from_millis(30))
            .with_poll_interval(Duration::from_millis(2));
        cfg.max_result_bytes = 64;
        let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone()))
            .expect("client")
            .with_poll_hook(Arc::new(move |attempt| {
                if attempt == 1 {
                    for name in fs2.list_dir(Path::new("/ipc/commands")).unwrap_or_default() {
                        let stem = name.trim_end_matches(".command.json");
                        fs2.plant(format!("/ipc/results/{stem}.result.json"), "x".repeat(4096));
                    }
                }
            }));
        let err = client
            .call("ping", Json::Null, None, &CancelFlag::new())
            .expect_err("too large");
        assert_eq!(err.code, codes::RESULT_TOO_LARGE);
        assert!(fs
            .list_dir(Path::new("/ipc/results"))
            .expect("ls")
            .is_empty());
    }

    #[test]
    fn a_result_for_the_wrong_request_id_is_an_internal_bridge_error() {
        let fs = MemFs::new();
        fs.plant("/ipc/heartbeat.json", online_heartbeat());
        fs.create_dir_all(Path::new("/ipc/results")).expect("mkdir");
        let fs2 = fs.clone();
        let cfg = IpcConfig::new("/ipc", TOKEN)
            .with_request_timeout(Duration::from_millis(30))
            .with_poll_interval(Duration::from_millis(2));
        let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone()))
            .expect("client")
            .with_poll_hook(Arc::new(move |attempt| {
                if attempt == 1 {
                    for name in fs2.list_dir(Path::new("/ipc/commands")).unwrap_or_default() {
                        let stem = name.trim_end_matches(".command.json");
                        let body = qjson::json_obj! {
                            "protocol_version" => limits::PROTOCOL_VERSION,
                            "request_id" => "somebody-elses-id",
                            "command" => "ping",
                            "ok" => true,
                            "result" => qjson::json_obj!{ "pong" => true },
                        }
                        .to_string();
                        fs2.plant(format!("/ipc/results/{stem}.result.json"), body);
                    }
                }
            }));
        let err = client
            .call("ping", Json::Null, None, &CancelFlag::new())
            .expect_err("wrong id");
        assert_eq!(err.code, codes::INTERNAL_BRIDGE_ERROR);
    }

    #[test]
    fn a_partial_tmp_result_is_never_read() {
        let fs = MemFs::new();
        fs.plant("/ipc/heartbeat.json", online_heartbeat());
        fs.create_dir_all(Path::new("/ipc/results")).expect("mkdir");
        let fs2 = fs.clone();
        let cfg = IpcConfig::new("/ipc", TOKEN)
            .with_request_timeout(Duration::from_millis(14))
            .with_poll_interval(Duration::from_millis(2));
        let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone()))
            .expect("client")
            .with_poll_hook(Arc::new(move |attempt| {
                if attempt == 1 {
                    for name in fs2.list_dir(Path::new("/ipc/commands")).unwrap_or_default() {
                        let stem = name.trim_end_matches(".command.json");
                        // A half-written result, in the bridge's own staging name.
                        fs2.plant(
                            format!("/ipc/results/{stem}.result.tmp"),
                            "{\"protocol_version\": \"qlabs-rea",
                        );
                    }
                }
            }));
        let err = client
            .call("ping", Json::Null, None, &CancelFlag::new())
            .expect_err("nothing complete to read");
        assert_eq!(err.code, codes::IPC_TIMEOUT);
        // The partial file is bridge-owned; the client must leave it alone.
        assert_eq!(fs.list_dir(Path::new("/ipc/results")).expect("ls").len(), 1);
    }

    #[test]
    fn discard_result_collects_a_late_result() {
        let fs = MemFs::new();
        fs.plant("/ipc/heartbeat.json", online_heartbeat());
        fs.plant("/ipc/results/late-one.result.json", "{}");
        let cfg = IpcConfig::new("/ipc", TOKEN);
        let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone())).expect("client");
        assert!(client.discard_result("late-one").expect("discard"));
        assert!(!client.discard_result("late-one").expect("discard"));
        assert!(fs
            .list_dir(Path::new("/ipc/results"))
            .expect("ls")
            .is_empty());
    }

    #[test]
    fn discard_result_refuses_an_escaping_id() {
        let (client, _fs) = client_with(MemFs::new(), 10);
        let err = client
            .discard_result("../../etc/passwd")
            .expect_err("escape");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn transaction_commands_reject_an_unsafe_transaction_id() {
        let (client, fs) = client_with(MemFs::new(), 10);
        for bad in ["../x", "a/b", "", ".."] {
            assert_eq!(
                client.commit_candidate(bad).expect_err("unsafe").code,
                codes::MALFORMED_REQUEST
            );
            assert_eq!(
                client.discard_candidate(bad).expect_err("unsafe").code,
                codes::MALFORMED_REQUEST
            );
            assert_eq!(
                client.undo_last_generation(bad).expect_err("unsafe").code,
                codes::MALFORMED_REQUEST
            );
        }
        assert!(fs
            .list_dir(Path::new("/ipc/commands"))
            .expect("ls")
            .is_empty());
    }

    #[test]
    fn preflight_accepts_a_well_formed_plan() {
        preflight_plan(&valid_plan()).expect("valid");
    }

    #[test]
    fn preflight_rejects_a_foreign_undo_label() {
        let mut p = valid_plan();
        p.undo_label = "Some other edit".into();
        let err = preflight_plan(&p).expect_err("foreign label");
        assert_eq!(err.code, codes::INVALID_EDIT_PLAN);
    }

    #[test]
    fn preflight_rejects_a_control_character_in_the_undo_label() {
        let mut p = valid_plan();
        p.undo_label = "QLabs MCP: a\nb".into();
        assert_eq!(
            preflight_plan(&p).expect_err("control").code,
            codes::INVALID_EDIT_PLAN
        );
    }

    #[test]
    fn preflight_rejects_a_bad_knowledge_version_with_its_own_code() {
        let mut p = valid_plan();
        p.knowledge_version = "not a version!".into();
        let err = preflight_plan(&p).expect_err("bad version");
        assert_eq!(err.code, codes::KNOWLEDGE_INVALID);
        p.knowledge_version = String::new();
        assert_eq!(
            preflight_plan(&p).expect_err("empty").code,
            codes::KNOWLEDGE_INVALID
        );
    }

    #[test]
    fn preflight_rejects_an_unsafe_transaction_id() {
        let mut p = valid_plan();
        p.transaction_id = "../escape".into();
        assert_eq!(
            preflight_plan(&p).expect_err("unsafe").code,
            codes::INVALID_EDIT_PLAN
        );
    }

    #[test]
    fn preflight_rejects_an_empty_operation_list() {
        let mut p = valid_plan();
        p.operations.clear();
        assert_eq!(
            preflight_plan(&p).expect_err("empty").code,
            codes::INVALID_EDIT_PLAN
        );
    }

    #[test]
    fn preflight_rejects_too_many_operations() {
        let mut p = valid_plan();
        p.operations = (0..=limits::MAX_OPERATIONS)
            .map(|i| EditOperation::SetTrackMute {
                track: format!("t{i}"),
                muted: true,
            })
            .collect();
        assert_eq!(
            preflight_plan(&p).expect_err("too many").code,
            codes::INVALID_EDIT_PLAN
        );
    }

    #[test]
    fn preflight_rejects_too_many_notes() {
        let note = PlannedNote {
            start_qn: BeatTime::ZERO,
            end_qn: BeatTime::from_quarters(1),
            pitch: 60,
            velocity: 90,
            channel: 0,
            muted: false,
            spelling: "C4".into(),
        };
        let mut p = valid_plan();
        p.operations.push(EditOperation::InsertNotes {
            item: "i0".into(),
            notes: vec![note; limits::MAX_GENERATED_NOTES + 1],
        });
        assert_eq!(
            preflight_plan(&p).expect_err("too many notes").code,
            codes::INVALID_EDIT_PLAN
        );
    }

    #[test]
    fn stage_candidate_preflights_before_touching_the_filesystem() {
        let (client, fs) = client_with(MemFs::new(), 10);
        let mut p = valid_plan();
        p.undo_label = "nope".into();
        let err = client
            .stage_candidate(&p, Json::Null)
            .expect_err("bad label");
        assert_eq!(err.code, codes::INVALID_EDIT_PLAN);
        assert!(fs
            .list_dir(Path::new("/ipc/commands"))
            .expect("ls")
            .is_empty());
    }

    #[test]
    fn stage_candidate_rejects_a_non_object_payload() {
        let (client, _fs) = client_with(MemFs::new(), 10);
        let err = client
            .stage_candidate(&valid_plan(), Json::Int(1))
            .expect_err("bad payload");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn the_client_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BridgeClient>();
        assert_send_sync::<CancelFlag>();
    }

    #[test]
    fn knowledge_version_pattern() {
        for ok in ["2026.07.1", "a", "A-Z_0.9+x", &"a".repeat(64)] {
            assert!(is_valid_knowledge_version(ok), "{ok}");
        }
        for bad in ["", "has space", "slash/", &"a".repeat(65)] {
            assert!(!is_valid_knowledge_version(bad), "{bad}");
        }
    }
}
