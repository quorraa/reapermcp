//! The in-memory session store.
//!
//! Everything the server hands a client an id for lives here: snapshots,
//! analyses, candidates, edit plans and staged transactions. The store is
//! **bounded** (a fixed number of entries per kind, oldest evicted first) and
//! **time-to-live'd** (an entry older than its TTL is refused with
//! [`crate::error::codes::EXPIRED_ID`] and swept). Nothing is written to disk;
//! killing the process discards every candidate.
//!
//! # Why ids are server-issued
//!
//! A client can only name an id the server minted, so a resource URI or a
//! `stage_candidate` call can never reach material this process did not
//! generate. Combined with the id pattern enforced in [`crate::resources`],
//! that is what stops an id from being usable as a filesystem path.
//!
//! # The candidate cache
//!
//! Generation is cached on the tuple
//! `(snapshot hash, profile, canonical params, knowledge hash, seed)`. The
//! snapshot **hash** rather than the snapshot **id** is the key component that
//! matters: two inspections of an unmodified project produce different ids but
//! the same hash, so the cache hits; one edit in REAPER changes the hash, so
//! the cache cannot hit and stale candidates can never be served for changed
//! material.

use crate::error::{codes, ToolError};
use music_analysis::report::Analysis;
use music_domain::plan::EditPlan;
use music_domain::prelude::*;
use qjson::{json_obj, Json};
use std::collections::BTreeMap;

/// How long a snapshot stays usable, in seconds.
pub const SNAPSHOT_TTL_SECONDS: i64 = 1800;
/// How long an analysis stays usable, in seconds.
pub const ANALYSIS_TTL_SECONDS: i64 = 3600;
/// How long a candidate stays usable, in seconds.
pub const CANDIDATE_TTL_SECONDS: i64 = 3600;
/// How long an edit plan stays usable, in seconds.
pub const PLAN_TTL_SECONDS: i64 = 3600;
/// How long a transaction record stays usable, in seconds.
pub const TRANSACTION_TTL_SECONDS: i64 = 86_400;

/// Maximum live snapshots.
pub const MAX_SNAPSHOTS: usize = 32;
/// Maximum live analyses.
pub const MAX_ANALYSES: usize = 32;
/// Maximum live candidates.
pub const MAX_CANDIDATES: usize = 128;
/// Maximum live edit plans.
pub const MAX_PLANS: usize = 64;
/// Maximum live transactions.
pub const MAX_TRANSACTIONS: usize = 64;
/// Maximum cached generation results.
pub const MAX_CACHE_ENTRIES: usize = 64;

/// The selection scope a snapshot was taken with.
///
/// Echoed back into `stage_candidate` so the bridge re-derives the snapshot the
/// same way it was originally derived. Sending a different scope produces a
/// spurious `STALE_SNAPSHOT`; see the binding notes in `CONTRACTS.md`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScopeEcho {
    /// `auto` | `active_editor` | `selected_item`.
    pub source_mode: String,
    /// `selected_or_all` | `selected_only` | `all`.
    pub note_scope: String,
    /// The melody-extraction mode.
    pub extraction_mode: String,
    /// The channel filter, when the mode is `midi_channel`.
    pub extraction_channel: Option<i64>,
}

impl ScopeEcho {
    /// The payload fragment `stage_candidate` must repeat.
    pub fn to_json(&self) -> Json {
        let mut extraction = qjson::JsonMap::new();
        extraction.insert("mode", Json::Str(self.extraction_mode.clone()));
        if let Some(c) = self.extraction_channel {
            extraction.insert("channel", Json::Int(c));
        }
        json_obj! {
            "source_mode" => self.source_mode.clone(),
            "note_scope" => self.note_scope.clone(),
            "melody_extraction" => Json::Obj(extraction),
        }
    }
}

/// A snapshot plus everything derived from it once.
#[derive(Clone, Debug)]
pub struct SnapshotRecord {
    /// The parsed, hash-verified snapshot.
    pub snapshot: reaper_ipc::Snapshot,
    /// The scope it was taken with.
    pub scope: ScopeEcho,
    /// The notes as the analysis engine sees them.
    pub notes: NoteSet,
    /// The raw result object, served as `reaper://selection/current`.
    pub raw: Json,
}

/// A generated candidate plus its provenance.
#[derive(Clone, Debug)]
pub struct CandidateRecord {
    /// The candidate itself.
    pub candidate: Candidate,
    /// The snapshot it was generated from.
    pub snapshot_id: String,
    /// The analysis it was generated from.
    pub analysis_id: String,
    /// The style profile used.
    pub profile_id: String,
    /// The knowledge bundle content hash at generation time.
    pub knowledge_hash: String,
    /// The tie-breaking seed.
    pub seed: u64,
}

/// An edit plan plus the scope it was generated against.
#[derive(Clone, Debug)]
pub struct PlanRecord {
    /// The plan.
    pub plan: EditPlan,
    /// The candidate it stages.
    pub candidate_id: String,
    /// The snapshot it was generated against.
    pub snapshot_id: String,
    /// The scope to echo when staging.
    pub scope: ScopeEcho,
}

/// A staged transaction as this server remembers it.
#[derive(Clone, Debug)]
pub struct TransactionRecord {
    /// The bridge-side transaction id.
    pub transaction_id: String,
    /// The plan that produced it.
    pub plan_id: String,
    /// The candidate that was staged.
    pub candidate_id: String,
    /// The snapshot the plan was built from.
    pub snapshot_id: String,
    /// `staged` | `committed` | `discarded` | `undone`.
    pub status: String,
    /// The undo label the bridge used.
    pub undo_label: String,
    /// The bridge's own `stage_candidate` result.
    pub stage_result: Json,
}

impl TransactionRecord {
    /// The `transaction://{id}` resource body.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "transaction_id" => self.transaction_id.clone(),
            "plan_id" => self.plan_id.clone(),
            "candidate_id" => self.candidate_id.clone(),
            "snapshot_id" => self.snapshot_id.clone(),
            "status" => self.status.clone(),
            "undo_label" => self.undo_label.clone(),
            "stage_result" => self.stage_result.clone(),
        }
    }
}

/// One stored value with its lifetime.
#[derive(Clone, Debug)]
struct Entry<T> {
    value: T,
    created_at: i64,
    expires_at: i64,
    /// Monotonic insertion counter, used for oldest-first eviction.
    sequence: u64,
}

/// A bounded, TTL'd collection of one kind of entry.
#[derive(Debug)]
struct Bucket<T> {
    kind: &'static str,
    entries: BTreeMap<String, Entry<T>>,
    capacity: usize,
    ttl: i64,
    next_sequence: u64,
}

impl<T> Bucket<T> {
    fn new(kind: &'static str, capacity: usize, ttl: i64) -> Bucket<T> {
        Bucket {
            kind,
            entries: BTreeMap::new(),
            capacity,
            ttl,
            next_sequence: 0,
        }
    }

    fn insert(&mut self, id: &str, value: T, now: i64) {
        self.sweep(now);
        if self.entries.len() >= self.capacity && !self.entries.contains_key(id) {
            self.evict_oldest();
        }
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.entries.insert(
            id.to_string(),
            Entry {
                value,
                created_at: now,
                expires_at: now + self.ttl,
                sequence,
            },
        );
    }

    fn evict_oldest(&mut self) {
        let oldest = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.sequence)
            .map(|(k, _)| k.clone());
        if let Some(k) = oldest {
            crate::log::debug(&format!("evicting the oldest {} {k}", self.kind));
            self.entries.remove(&k);
        }
    }

    fn sweep(&mut self, now: i64) -> usize {
        let expired: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.expires_at <= now)
            .map(|(k, _)| k.clone())
            .collect();
        for k in &expired {
            self.entries.remove(k);
        }
        expired.len()
    }

    fn get(&self, id: &str, now: i64) -> Result<&T, ToolError> {
        match self.entries.get(id) {
            Some(e) if e.expires_at > now => Ok(&e.value),
            Some(e) => Err(ToolError::with_details(
                codes::EXPIRED_ID,
                format!("{} {id} expired", self.kind),
                json_obj! {
                    "kind" => self.kind,
                    "id" => id,
                    "created_at" => qjson::time::iso8601_from_unix(e.created_at),
                    "expired_at" => qjson::time::iso8601_from_unix(e.expires_at),
                },
            )
            .remedy("take a fresh snapshot and regenerate")),
            None => Err(ToolError::with_details(
                codes::UNKNOWN_ID,
                format!("no {} with id {id}", self.kind),
                json_obj! { "kind" => self.kind, "id" => id },
            )
            .remedy("use an id this server issued in the current session")),
        }
    }

    fn contains(&self, id: &str, now: i64) -> bool {
        self.entries.get(id).is_some_and(|e| e.expires_at > now)
    }

    fn ids(&self, now: i64) -> Vec<String> {
        self.entries
            .iter()
            .filter(|(_, e)| e.expires_at > now)
            .map(|(k, _)| k.clone())
            .collect()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn set_status(&mut self, id: &str, f: impl FnOnce(&mut T)) -> bool {
        match self.entries.get_mut(id) {
            Some(e) => {
                f(&mut e.value);
                true
            }
            None => false,
        }
    }
}

/// Everything the current session remembers.
#[derive(Debug)]
pub struct SessionStore {
    snapshots: Bucket<SnapshotRecord>,
    analyses: Bucket<Analysis>,
    candidates: Bucket<CandidateRecord>,
    plans: Bucket<PlanRecord>,
    transactions: Bucket<TransactionRecord>,
    /// Cache key -> the candidate ids generated for it, in order.
    cache: BTreeMap<String, Entry<Vec<String>>>,
    cache_sequence: u64,
}

impl Default for SessionStore {
    fn default() -> Self {
        SessionStore::new()
    }
}

impl SessionStore {
    /// An empty store with the default bounds and TTLs.
    pub fn new() -> SessionStore {
        SessionStore {
            snapshots: Bucket::new("snapshot", MAX_SNAPSHOTS, SNAPSHOT_TTL_SECONDS),
            analyses: Bucket::new("analysis", MAX_ANALYSES, ANALYSIS_TTL_SECONDS),
            candidates: Bucket::new("candidate", MAX_CANDIDATES, CANDIDATE_TTL_SECONDS),
            plans: Bucket::new("edit plan", MAX_PLANS, PLAN_TTL_SECONDS),
            transactions: Bucket::new("transaction", MAX_TRANSACTIONS, TRANSACTION_TTL_SECONDS),
            cache: BTreeMap::new(),
            cache_sequence: 0,
        }
    }

    // ---- snapshots -------------------------------------------------------

    /// Stores a snapshot under its bridge-issued id.
    pub fn put_snapshot(&mut self, record: SnapshotRecord, now: i64) -> String {
        let id = record.snapshot.snapshot_id.clone();
        self.snapshots.insert(&id, record, now);
        id
    }

    /// Reads a snapshot.
    pub fn snapshot(&self, id: &str, now: i64) -> Result<&SnapshotRecord, ToolError> {
        self.snapshots.get(id, now)
    }

    /// The most recently inserted live snapshot, if any.
    pub fn latest_snapshot(&self, now: i64) -> Option<&SnapshotRecord> {
        self.snapshots
            .entries
            .values()
            .filter(|e| e.expires_at > now)
            .max_by_key(|e| e.sequence)
            .map(|e| &e.value)
    }

    // ---- analyses --------------------------------------------------------

    /// Stores an analysis under its engine-derived id.
    pub fn put_analysis(&mut self, analysis: Analysis, now: i64) -> String {
        let id = analysis.id.clone();
        self.analyses.insert(&id, analysis, now);
        id
    }

    /// Reads an analysis.
    pub fn analysis(&self, id: &str, now: i64) -> Result<&Analysis, ToolError> {
        self.analyses.get(id, now)
    }

    /// The newest live analysis derived from `snapshot_id`.
    pub fn analysis_for_snapshot(&self, snapshot_id: &str, now: i64) -> Option<&Analysis> {
        self.analyses
            .entries
            .values()
            .filter(|e| e.expires_at > now && e.value.snapshot_id == snapshot_id)
            .max_by_key(|e| e.sequence)
            .map(|e| &e.value)
    }

    // ---- candidates ------------------------------------------------------

    /// Stores a candidate.
    pub fn put_candidate(&mut self, record: CandidateRecord, now: i64) -> String {
        let id = record.candidate.id.clone();
        self.candidates.insert(&id, record, now);
        id
    }

    /// Reads a candidate.
    pub fn candidate(&self, id: &str, now: i64) -> Result<&CandidateRecord, ToolError> {
        self.candidates.get(id, now)
    }

    // ---- plans -----------------------------------------------------------

    /// Stores an edit plan.
    pub fn put_plan(&mut self, record: PlanRecord, now: i64) -> String {
        let id = record.plan.plan_id.clone();
        self.plans.insert(&id, record, now);
        id
    }

    /// Reads an edit plan.
    pub fn plan(&self, id: &str, now: i64) -> Result<&PlanRecord, ToolError> {
        self.plans.get(id, now)
    }

    // ---- transactions ----------------------------------------------------

    /// Stores a transaction record.
    pub fn put_transaction(&mut self, record: TransactionRecord, now: i64) -> String {
        let id = record.transaction_id.clone();
        self.transactions.insert(&id, record, now);
        id
    }

    /// Reads a transaction record.
    pub fn transaction(&self, id: &str, now: i64) -> Result<&TransactionRecord, ToolError> {
        self.transactions.get(id, now)
    }

    /// Updates a transaction's status. Returns false when the id is unknown.
    pub fn set_transaction_status(&mut self, id: &str, status: &str) -> bool {
        let status = status.to_string();
        self.transactions
            .set_status(id, move |r| r.status = status.clone())
    }

    /// Every live transaction id.
    pub fn transaction_ids(&self, now: i64) -> Vec<String> {
        self.transactions.ids(now)
    }

    /// The newest live transaction still in the `staged` state.
    pub fn latest_staged_transaction(&self, now: i64) -> Option<&TransactionRecord> {
        self.transactions
            .entries
            .values()
            .filter(|e| e.expires_at > now && e.value.status == "staged")
            .max_by_key(|e| e.sequence)
            .map(|e| &e.value)
    }

    // ---- the generation cache -------------------------------------------

    /// Builds a cache key from everything that may change the result.
    ///
    /// The snapshot **hash** is used, never the snapshot id: a re-inspection of
    /// an unmodified project must hit, and any edit must miss.
    pub fn cache_key(
        snapshot_hash: &str,
        profile_id: &str,
        params_canonical: &str,
        knowledge_hash: &str,
        seed: u64,
    ) -> String {
        let mut h = qjson::sha256::Sha256::new();
        for part in [
            snapshot_hash,
            profile_id,
            params_canonical,
            knowledge_hash,
            &seed.to_string(),
        ] {
            h.update(part.as_bytes());
            h.update(b"\x1f");
        }
        h.finish_hex()
    }

    /// Looks a generation result up, returning the candidate ids that are all
    /// still live. A partially expired entry is a miss.
    pub fn cached(&self, key: &str, now: i64) -> Option<Vec<String>> {
        let entry = self.cache.get(key)?;
        if entry.expires_at <= now {
            return None;
        }
        if entry
            .value
            .iter()
            .all(|id| self.candidates.contains(id, now))
        {
            Some(entry.value.clone())
        } else {
            None
        }
    }

    /// Records a generation result under `key`.
    pub fn cache(&mut self, key: &str, ids: Vec<String>, now: i64) {
        self.sweep_cache(now);
        if self.cache.len() >= MAX_CACHE_ENTRIES && !self.cache.contains_key(key) {
            let oldest = self
                .cache
                .iter()
                .min_by_key(|(_, e)| e.sequence)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.cache.remove(&k);
            }
        }
        let sequence = self.cache_sequence;
        self.cache_sequence += 1;
        self.cache.insert(
            key.to_string(),
            Entry {
                value: ids,
                created_at: now,
                expires_at: now + CANDIDATE_TTL_SECONDS,
                sequence,
            },
        );
    }

    fn sweep_cache(&mut self, now: i64) {
        let expired: Vec<String> = self
            .cache
            .iter()
            .filter(|(_, e)| e.expires_at <= now)
            .map(|(k, _)| k.clone())
            .collect();
        for k in expired {
            self.cache.remove(&k);
        }
    }

    // ---- maintenance -----------------------------------------------------

    /// Drops everything past its TTL. Returns how many entries went.
    pub fn sweep(&mut self, now: i64) -> usize {
        let mut n = 0;
        n += self.snapshots.sweep(now);
        n += self.analyses.sweep(now);
        n += self.candidates.sweep(now);
        n += self.plans.sweep(now);
        n += self.transactions.sweep(now);
        let before = self.cache.len();
        self.sweep_cache(now);
        n += before - self.cache.len();
        n
    }

    /// Live entry counts, for `reaper.status` and `doctor`.
    pub fn stats(&self) -> Json {
        json_obj! {
            "snapshots" => self.snapshots.len() as i64,
            "analyses" => self.analyses.len() as i64,
            "candidates" => self.candidates.len() as i64,
            "edit_plans" => self.plans.len() as i64,
            "transactions" => self.transactions.len() as i64,
            "cached_generations" => self.cache.len() as i64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str) -> CandidateRecord {
        let mut c = Candidate {
            id: id.to_string(),
            kind: CandidateKind::Harmonization,
            label: "test".into(),
            strategy: "functional".into(),
            chords: Vec::new(),
            parts: Vec::new(),
            trace: DecisionTrace::default(),
            loop_report: None,
            created_at: String::new(),
            expires_at: String::new(),
        };
        c.trace.candidate_id = id.to_string();
        CandidateRecord {
            candidate: c,
            snapshot_id: "s".into(),
            analysis_id: "a".into(),
            profile_id: "jazz_standard".into(),
            knowledge_hash: "hash".into(),
            seed: 7,
        }
    }

    #[test]
    fn unknown_id_is_reported_as_unknown() {
        let store = SessionStore::new();
        let e = store.candidate("nope", 0).unwrap_err();
        assert_eq!(e.code, codes::UNKNOWN_ID);
    }

    #[test]
    fn expired_id_is_reported_as_expired() {
        let mut store = SessionStore::new();
        store.put_candidate(candidate("c1"), 1_000);
        assert!(store.candidate("c1", 1_001).is_ok());
        let e = store
            .candidate("c1", 1_000 + CANDIDATE_TTL_SECONDS + 1)
            .unwrap_err();
        assert_eq!(e.code, codes::EXPIRED_ID);
    }

    #[test]
    fn the_bucket_is_bounded_and_evicts_oldest_first() {
        let mut store = SessionStore::new();
        for i in 0..(MAX_CANDIDATES + 4) {
            store.put_candidate(candidate(&format!("c{i}")), 1_000);
        }
        assert_eq!(store.candidates.len(), MAX_CANDIDATES);
        assert!(store.candidate("c0", 1_000).is_err());
        assert!(store
            .candidate(&format!("c{}", MAX_CANDIDATES + 3), 1_000)
            .is_ok());
    }

    #[test]
    fn sweep_removes_expired_entries() {
        let mut store = SessionStore::new();
        store.put_candidate(candidate("c1"), 0);
        let removed = store.sweep(CANDIDATE_TTL_SECONDS + 1);
        assert_eq!(removed, 1);
        assert_eq!(store.candidates.len(), 0);
    }

    #[test]
    fn the_cache_keys_on_the_snapshot_hash() {
        let a = SessionStore::cache_key("fnv1a64:aaaa", "jazz_standard", "{}", "kh", 1);
        let b = SessionStore::cache_key("fnv1a64:bbbb", "jazz_standard", "{}", "kh", 1);
        assert_ne!(a, b, "a changed snapshot must key differently");
        let c = SessionStore::cache_key("fnv1a64:aaaa", "jazz_standard", "{}", "kh", 1);
        assert_eq!(a, c, "the same inputs must key identically");
    }

    #[test]
    fn the_cache_is_not_served_across_a_changed_snapshot() {
        let mut store = SessionStore::new();
        store.put_candidate(candidate("c1"), 0);
        let key = SessionStore::cache_key("fnv1a64:aaaa", "p", "{}", "kh", 1);
        store.cache(&key, vec!["c1".into()], 0);
        assert_eq!(store.cached(&key, 1), Some(vec!["c1".to_string()]));
        let changed = SessionStore::cache_key("fnv1a64:bbbb", "p", "{}", "kh", 1);
        assert_eq!(store.cached(&changed, 1), None);
    }

    #[test]
    fn a_cache_entry_with_an_expired_candidate_is_a_miss() {
        let mut store = SessionStore::new();
        store.put_candidate(candidate("c1"), 0);
        let key = SessionStore::cache_key("h", "p", "{}", "kh", 1);
        store.cache(&key, vec!["c1".into()], 0);
        assert!(store.cached(&key, CANDIDATE_TTL_SECONDS + 1).is_none());
    }

    #[test]
    fn transaction_status_transitions() {
        let mut store = SessionStore::new();
        store.put_transaction(
            TransactionRecord {
                transaction_id: "t1".into(),
                plan_id: "p1".into(),
                candidate_id: "c1".into(),
                snapshot_id: "s1".into(),
                status: "staged".into(),
                undo_label: "QLabs MCP: Stage candidate abcdef12".into(),
                stage_result: Json::Null,
            },
            0,
        );
        assert!(store.latest_staged_transaction(1).is_some());
        assert!(store.set_transaction_status("t1", "committed"));
        assert_eq!(store.transaction("t1", 1).unwrap().status, "committed");
        assert!(store.latest_staged_transaction(1).is_none());
        assert!(!store.set_transaction_status("nope", "committed"));
    }

    #[test]
    fn scope_echo_json_shape() {
        let s = ScopeEcho {
            source_mode: "auto".into(),
            note_scope: "selected_or_all".into(),
            extraction_mode: "midi_channel".into(),
            extraction_channel: Some(3),
        };
        let v = s.to_json();
        assert_eq!(v.str_field("note_scope").unwrap(), "selected_or_all");
        assert_eq!(
            v.get("melody_extraction").unwrap().i64_field("channel"),
            Ok(3)
        );
    }

    #[test]
    fn stats_reports_every_bucket() {
        let store = SessionStore::new();
        let v = store.stats();
        for k in [
            "snapshots",
            "analyses",
            "candidates",
            "edit_plans",
            "transactions",
            "cached_generations",
        ] {
            assert_eq!(v.i64_field(k), Ok(0), "{k}");
        }
    }

    #[test]
    fn the_cache_is_bounded() {
        let mut store = SessionStore::new();
        store.put_candidate(candidate("c1"), 0);
        for i in 0..(MAX_CACHE_ENTRIES + 5) {
            store.cache(&format!("key{i}"), vec!["c1".into()], 0);
        }
        assert_eq!(store.cache.len(), MAX_CACHE_ENTRIES);
    }
}
