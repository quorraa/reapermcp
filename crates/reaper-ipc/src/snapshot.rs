//! Parsing and independent verification of an `inspect_selection` result.
//!
//! The bridge sends six hashes with every snapshot. This module rebuilds the
//! canonical strings from the data that travelled alongside them and checks
//! that they reproduce. A hash that does not reproduce is
//! [`codes::SNAPSHOT_HASH_MISMATCH`][crate::error::codes::SNAPSHOT_HASH_MISMATCH]
//! — an error, not a warning, because a snapshot whose own hashes disagree
//! cannot be used as a staleness token.
//!
//! # What can and cannot be verified
//!
//! | hash | verifiable | why |
//! |---|---|---|
//! | `note_list_hash` | always | over the `notes` array that was sent |
//! | `tempo_map_hash` | always | over the `tempo_markers` array that was sent |
//! | `timesig_map_hash` | always | same markers |
//! | `snapshot_hash` | always | over fields that were all sent, including the other hashes verbatim |
//! | `midi_hash` | only when unfiltered | it covers the *whole* take; the client only sees the filtered notes |
//! | `note_selection_hash` | only when unfiltered | same |
//!
//! "Unfiltered" means `note_count == source_note_count`, in which case the
//! `notes` array carries every note in the take and its PPQ fields are enough
//! to rebuild the take-domain canonical strings.

use crate::error::{codes, IpcError};
use crate::hash::{self, ListNote, SnapshotFields, TakeNote, TempoMarker};
use qjson::Json;

/// One note from a snapshot's `notes` array.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotNote {
    /// 0-based position in the filtered array.
    pub index: i64,
    /// 0-based index in `MIDI_GetNote` order.
    pub source_index: i64,
    /// Start in take PPQ.
    pub start_ppq: f64,
    /// End in take PPQ.
    pub end_ppq: f64,
    /// Start in project quarter notes.
    pub start_qn: f64,
    /// End in project quarter notes.
    pub end_qn: f64,
    /// `end_qn - start_qn`.
    pub duration_qn: f64,
    /// `start_qn - item_position_qn`.
    pub item_relative_start_qn: f64,
    /// `end_qn - item_position_qn`.
    pub item_relative_end_qn: f64,
    /// Start in project seconds.
    pub start_seconds: f64,
    /// End in project seconds.
    pub end_seconds: f64,
    /// MIDI pitch, 0..=127.
    pub pitch: i64,
    /// MIDI velocity, 1..=127.
    pub velocity: i64,
    /// MIDI channel, 0..=15.
    pub channel: i64,
    /// Muted flag.
    pub muted: bool,
    /// Selected flag.
    pub selected: bool,
}

impl SnapshotNote {
    /// Parses one note object.
    pub fn from_json(v: &Json) -> Result<SnapshotNote, IpcError> {
        Ok(SnapshotNote {
            index: num_i(v, "index")?,
            source_index: num_i(v, "source_index")?,
            start_ppq: num_f(v, "start_ppq")?,
            end_ppq: num_f(v, "end_ppq")?,
            start_qn: num_f(v, "start_qn")?,
            end_qn: num_f(v, "end_qn")?,
            duration_qn: num_f(v, "duration_qn")?,
            item_relative_start_qn: num_f(v, "item_relative_start_qn")?,
            item_relative_end_qn: num_f(v, "item_relative_end_qn")?,
            start_seconds: num_f(v, "start_seconds")?,
            end_seconds: num_f(v, "end_seconds")?,
            pitch: num_i(v, "pitch")?,
            velocity: num_i(v, "velocity")?,
            channel: num_i(v, "channel")?,
            muted: flag(v, "muted"),
            selected: flag(v, "selected"),
        })
    }

    /// This note in the take-PPQ domain used by `midi_hash`.
    pub fn as_take_note(&self) -> TakeNote {
        TakeNote {
            start_ppq: self.start_ppq,
            end_ppq: self.end_ppq,
            channel: self.channel,
            pitch: self.pitch,
            velocity: self.velocity,
            muted: self.muted,
            selected: self.selected,
        }
    }

    /// This note in the project-QN domain used by `note_list_hash`.
    pub fn as_list_note(&self) -> ListNote {
        ListNote {
            start_qn: self.start_qn,
            end_qn: self.end_qn,
            pitch: self.pitch,
            velocity: self.velocity,
            channel: self.channel,
            muted: self.muted,
            selected: self.selected,
        }
    }
}

/// One entry of a snapshot's `tempo_markers` array.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotTempoMarker {
    /// REAPER's marker index.
    pub index: i64,
    /// Position in project seconds.
    pub time_seconds: f64,
    /// Position in project quarter notes.
    pub qn: f64,
    /// Tempo in BPM.
    pub bpm: f64,
    /// Time-signature numerator, 0 when the marker carries none.
    pub timesig_num: i64,
    /// Time-signature denominator, 0 when the marker carries none.
    pub timesig_den: i64,
    /// Linear tempo ramp flag.
    pub linear: bool,
    /// True on the single marker the bridge synthesises for a project with no
    /// explicit tempo or time-signature markers.
    pub synthetic: bool,
}

impl SnapshotTempoMarker {
    /// Parses one marker object.
    pub fn from_json(v: &Json) -> Result<SnapshotTempoMarker, IpcError> {
        Ok(SnapshotTempoMarker {
            index: num_i(v, "index")?,
            time_seconds: num_f(v, "time_seconds")?,
            qn: num_f(v, "qn")?,
            bpm: num_f(v, "bpm")?,
            timesig_num: num_i(v, "timesig_num")?,
            timesig_den: num_i(v, "timesig_den")?,
            linear: flag(v, "linear"),
            synthetic: flag(v, "synthetic"),
        })
    }

    /// This marker in the shape the canonical builders take.
    pub fn as_marker(&self) -> TempoMarker {
        TempoMarker {
            time_seconds: self.time_seconds,
            qn: self.qn,
            bpm: self.bpm,
            timesig_num: self.timesig_num,
            timesig_den: self.timesig_den,
            linear: self.linear,
        }
    }
}

/// A parsed `inspect_selection` result.
///
/// The raw JSON is retained so nothing is lost: callers that need a field this
/// struct does not model can read it from [`Snapshot::raw`].
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Fresh per-call snapshot id.
    pub snapshot_id: String,
    /// Persistent project UUID.
    pub project_uuid: Option<String>,
    /// Project state-change counter at inspection time.
    pub project_state_change_count: i64,
    /// Normalised track GUID.
    pub track_guid: Option<String>,
    /// Normalised item GUID.
    pub item_guid: Option<String>,
    /// Normalised take GUID.
    pub take_guid: Option<String>,
    /// Item position in project seconds.
    pub item_position_seconds: f64,
    /// Item length in project seconds.
    pub item_length_seconds: f64,
    /// Item position in project quarter notes.
    pub item_position_qn: f64,
    /// Item end in project quarter notes.
    pub item_end_qn: f64,
    /// Item length in project quarter notes.
    pub item_length_qn: f64,
    /// REAPER's `B_LOOPSRC` flag.
    pub is_loop_source: bool,
    /// Echo of the requested note scope.
    pub note_scope: Option<String>,
    /// Echo of the requested extraction mode.
    pub extraction_mode: Option<String>,
    /// Echo of the requested channel filter.
    pub extraction_channel: Option<i64>,
    /// `"active_editor"` or `"selected_item"`.
    pub resolved_by: Option<String>,
    /// Hash over the whole take.
    pub midi_hash: Option<String>,
    /// Selection hash over the whole take.
    pub note_selection_hash: Option<String>,
    /// Tempo-map hash.
    pub tempo_map_hash: Option<String>,
    /// Time-signature-map hash.
    pub timesig_map_hash: Option<String>,
    /// Hash over the filtered note list.
    pub note_list_hash: Option<String>,
    /// Hash over the whole snapshot.
    pub snapshot_hash: Option<String>,
    /// Number of notes in `notes`.
    pub note_count: i64,
    /// Notes in the take before filtering.
    pub source_note_count: i64,
    /// The filtered notes.
    pub notes: Vec<SnapshotNote>,
    /// The tempo / time-signature markers.
    pub tempo_markers: Vec<SnapshotTempoMarker>,
    /// Human-readable notes about the choices the bridge made.
    pub selection_assumptions: Vec<String>,
    /// `{code, message}` warnings.
    pub warnings: Vec<Json>,
    /// The unmodified result object.
    pub raw: Json,
}

fn bad(field: &str, what: &str) -> IpcError {
    IpcError::with_details(
        codes::INTERNAL_BRIDGE_ERROR,
        format!("snapshot field {field} {what}"),
        qjson::json_obj! { "field" => field },
    )
}

fn num_f(v: &Json, key: &str) -> Result<f64, IpcError> {
    v.get(key)
        .and_then(Json::as_f64)
        .ok_or_else(|| bad(key, "is missing or not a number"))
}

fn num_i(v: &Json, key: &str) -> Result<i64, IpcError> {
    match v.get(key) {
        Some(Json::Int(i)) => Ok(*i),
        Some(Json::Float(f)) if f.fract() == 0.0 => Ok(*f as i64),
        _ => Err(bad(key, "is missing or not an integer")),
    }
}

fn flag(v: &Json, key: &str) -> bool {
    v.get(key).and_then(Json::as_bool).unwrap_or(false)
}

/// Reads a field that may be absent or JSON `null`, which the protocol treats
/// identically.
fn opt_str(v: &Json, key: &str) -> Option<String> {
    match v.get(key) {
        Some(Json::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

fn opt_i(v: &Json, key: &str) -> Option<i64> {
    match v.get(key) {
        Some(Json::Int(i)) => Some(*i),
        Some(Json::Float(f)) if f.fract() == 0.0 => Some(*f as i64),
        _ => None,
    }
}

impl Snapshot {
    /// Parses an `inspect_selection` result object.
    pub fn from_json(v: &Json) -> Result<Snapshot, IpcError> {
        if v.as_obj().is_none() {
            return Err(bad("<root>", "is not a JSON object"));
        }
        let mut notes = Vec::new();
        if let Some(Json::Arr(items)) = v.get("notes") {
            notes.reserve(items.len());
            for n in items {
                notes.push(SnapshotNote::from_json(n)?);
            }
        }
        let mut tempo_markers = Vec::new();
        if let Some(Json::Arr(items)) = v.get("tempo_markers") {
            tempo_markers.reserve(items.len());
            for m in items {
                tempo_markers.push(SnapshotTempoMarker::from_json(m)?);
            }
        }
        let selection_assumptions = match v.get("selection_assumptions") {
            Some(Json::Arr(items)) => items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        };
        let warnings = match v.get("warnings") {
            Some(Json::Arr(items)) => items.clone(),
            _ => Vec::new(),
        };
        Ok(Snapshot {
            snapshot_id: opt_str(v, "snapshot_id").unwrap_or_default(),
            project_uuid: opt_str(v, "project_uuid"),
            project_state_change_count: opt_i(v, "project_state_change_count").unwrap_or(0),
            track_guid: opt_str(v, "track_guid"),
            item_guid: opt_str(v, "item_guid"),
            take_guid: opt_str(v, "take_guid"),
            item_position_seconds: num_f(v, "item_position_seconds")?,
            item_length_seconds: num_f(v, "item_length_seconds")?,
            item_position_qn: num_f(v, "item_position_qn")?,
            item_end_qn: num_f(v, "item_end_qn")?,
            item_length_qn: num_f(v, "item_length_qn")?,
            is_loop_source: flag(v, "is_loop_source"),
            note_scope: opt_str(v, "note_scope"),
            extraction_mode: opt_str(v, "extraction_mode"),
            extraction_channel: opt_i(v, "extraction_channel"),
            resolved_by: opt_str(v, "resolved_by"),
            midi_hash: opt_str(v, "midi_hash"),
            note_selection_hash: opt_str(v, "note_selection_hash"),
            tempo_map_hash: opt_str(v, "tempo_map_hash"),
            timesig_map_hash: opt_str(v, "timesig_map_hash"),
            note_list_hash: opt_str(v, "note_list_hash"),
            snapshot_hash: opt_str(v, "snapshot_hash"),
            note_count: num_i(v, "note_count")?,
            source_note_count: opt_i(v, "source_note_count").unwrap_or(-1),
            notes,
            tempo_markers,
            selection_assumptions,
            warnings,
            raw: v.clone(),
        })
    }

    /// True when the `notes` array covers the whole take, which is the
    /// condition under which `midi_hash` and `note_selection_hash` can be
    /// recomputed client-side.
    pub fn is_unfiltered(&self) -> bool {
        self.source_note_count >= 0
            && self.source_note_count == self.note_count
            && self.note_count as usize == self.notes.len()
    }

    /// The subset of fields `snapshot_hash` is computed over.
    ///
    /// The hash strings are taken verbatim from the snapshot, exactly as the
    /// bridge does — `snapshot_hash` is a hash *of the other hashes*, not a
    /// re-derivation of them.
    pub fn hash_fields(&self) -> SnapshotFields {
        SnapshotFields {
            project_uuid: self.project_uuid.clone(),
            track_guid: self.track_guid.clone(),
            item_guid: self.item_guid.clone(),
            take_guid: self.take_guid.clone(),
            item_position_seconds: self.item_position_seconds,
            item_length_seconds: self.item_length_seconds,
            item_position_qn: self.item_position_qn,
            item_length_qn: self.item_length_qn,
            is_loop_source: self.is_loop_source,
            note_scope: self.note_scope.clone(),
            extraction_mode: self.extraction_mode.clone(),
            extraction_channel: self.extraction_channel,
            midi_hash: self.midi_hash.clone(),
            tempo_map_hash: self.tempo_map_hash.clone(),
            timesig_map_hash: self.timesig_map_hash.clone(),
            note_selection_hash: self.note_selection_hash.clone(),
            note_list_hash: self.note_list_hash.clone(),
            note_count: self.note_count,
        }
    }

    /// Recomputes `snapshot_hash` from this snapshot's own fields.
    pub fn compute_snapshot_hash(&self) -> Result<String, IpcError> {
        Ok(hash::hash_canonical(&hash::snapshot_canonical(
            &self.hash_fields(),
        )?))
    }

    /// Independently verifies every hash that can be verified from the data the
    /// bridge sent.
    ///
    /// Returns the list of field names that were checked. A hash the bridge
    /// omitted is skipped rather than treated as a failure — the bridge may
    /// legitimately have no take to hash — but a hash that is present and does
    /// not reproduce is an error.
    pub fn verify(&self) -> Result<Vec<&'static str>, IpcError> {
        let mut checked = Vec::new();

        if let Some(expected) = &self.note_list_hash {
            let list: Vec<ListNote> = self.notes.iter().map(SnapshotNote::as_list_note).collect();
            hash::check(
                "note_list_hash",
                expected,
                &hash::note_list_canonical(&list)?,
            )?;
            checked.push("note_list_hash");
        }

        if !self.tempo_markers.is_empty() {
            let markers: Vec<TempoMarker> = self
                .tempo_markers
                .iter()
                .map(SnapshotTempoMarker::as_marker)
                .collect();
            if let Some(expected) = &self.tempo_map_hash {
                hash::check(
                    "tempo_map_hash",
                    expected,
                    &hash::tempo_canonical(&markers)?,
                )?;
                checked.push("tempo_map_hash");
            }
            if let Some(expected) = &self.timesig_map_hash {
                hash::check(
                    "timesig_map_hash",
                    expected,
                    &hash::timesig_canonical(&markers)?,
                )?;
                checked.push("timesig_map_hash");
            }
        }

        if self.is_unfiltered() {
            let take: Vec<TakeNote> = self.notes.iter().map(SnapshotNote::as_take_note).collect();
            if let Some(expected) = &self.midi_hash {
                hash::check("midi_hash", expected, &hash::midi_canonical(&take)?)?;
                checked.push("midi_hash");
            }
            if let Some(expected) = &self.note_selection_hash {
                hash::check(
                    "note_selection_hash",
                    expected,
                    &hash::selection_canonical(&take)?,
                )?;
                checked.push("note_selection_hash");
            }
        }

        if let Some(expected) = &self.snapshot_hash {
            hash::check(
                "snapshot_hash",
                expected,
                &hash::snapshot_canonical(&self.hash_fields())?,
            )?;
            checked.push("snapshot_hash");
        }

        Ok(checked)
    }

    /// The `expected_project` block that pins this snapshot for a later call.
    ///
    /// Deliberately omits `state_change_count`: it increments on any project
    /// edit including a selection change, so enforcing it makes requests fail
    /// for benign reasons. Callers that genuinely need "nothing may have
    /// happened" should set it themselves.
    pub fn expected_project(&self) -> crate::ExpectedProject {
        crate::ExpectedProject {
            project_uuid: self.project_uuid.clone(),
            state_change_count: None,
            source_item_guid: self.item_guid.clone(),
            source_take_guid: self.take_guid.clone(),
            midi_hash: self.midi_hash.clone(),
            tempo_map_hash: self.tempo_map_hash.clone(),
            snapshot_hash: self.snapshot_hash.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> Json {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mock-reaper/snapshots/four-note-melody.snapshot.json"
        );
        let text = std::fs::read_to_string(path).expect("snapshot fixture");
        Json::parse(&text).expect("parses")
    }

    #[test]
    fn parses_the_recorded_snapshot() {
        let s = Snapshot::from_json(&scene()).expect("parse");
        assert_eq!(s.snapshot_id, "00000000-0000-4000-8000-0000000000aa");
        assert_eq!(
            s.project_uuid.as_deref(),
            Some("00000000-0000-4000-8000-000000000001")
        );
        assert_eq!(s.note_count, 4);
        assert_eq!(s.source_note_count, 4);
        assert_eq!(s.notes.len(), 4);
        assert_eq!(s.tempo_markers.len(), 1);
        assert_eq!(s.note_scope.as_deref(), Some("all"));
        assert_eq!(s.extraction_mode.as_deref(), Some("auto"));
        assert_eq!(s.extraction_channel, None);
        assert_eq!(s.resolved_by.as_deref(), Some("active_editor"));
        assert!(!s.is_loop_source);
    }

    #[test]
    fn note_fields_round_trip_into_both_hash_domains() {
        let s = Snapshot::from_json(&scene()).expect("parse");
        let n = &s.notes[0];
        assert_eq!(n.pitch, 60);
        assert_eq!(n.velocity, 100);
        assert!(n.selected);
        assert_eq!(n.as_take_note().start_ppq, 0.0);
        assert_eq!(n.as_take_note().end_ppq, 960.0);
        assert_eq!(n.as_list_note().start_qn, 0.0);
        assert_eq!(n.as_list_note().end_qn, 1.0);
        assert!(!s.notes[3].selected);
    }

    #[test]
    fn the_synthetic_tempo_marker_is_flagged() {
        let s = Snapshot::from_json(&scene()).expect("parse");
        assert!(s.tempo_markers[0].synthetic);
        assert_eq!(s.tempo_markers[0].bpm, 120.0);
        assert_eq!(s.tempo_markers[0].timesig_num, 4);
        assert_eq!(s.tempo_markers[0].timesig_den, 4);
    }

    #[test]
    fn every_hash_in_the_recorded_snapshot_reproduces() {
        let s = Snapshot::from_json(&scene()).expect("parse");
        assert!(s.is_unfiltered());
        let checked = s.verify().expect("all hashes reproduce");
        assert_eq!(
            checked,
            vec![
                "note_list_hash",
                "tempo_map_hash",
                "timesig_map_hash",
                "midi_hash",
                "note_selection_hash",
                "snapshot_hash",
            ]
        );
    }

    #[test]
    fn recomputed_snapshot_hash_equals_the_recorded_one() {
        let s = Snapshot::from_json(&scene()).expect("parse");
        assert_eq!(
            s.compute_snapshot_hash().expect("compute"),
            "fnv1a64:3e7bf035037bcf4f"
        );
    }

    #[test]
    fn a_tampered_note_makes_verification_fail() {
        let mut doc = scene();
        if let Some(Json::Arr(notes)) = doc
            .as_obj()
            .and_then(|_| doc.get("notes"))
            .cloned()
            .as_mut()
        {
            notes[0] = {
                let mut n = notes[0].clone();
                if let Json::Obj(m) = &mut n {
                    m.insert("velocity", Json::Int(1));
                }
                n
            };
            if let Json::Obj(m) = &mut doc {
                m.insert("notes", Json::Arr(notes.clone()));
            }
        }
        let s = Snapshot::from_json(&doc).expect("parse");
        let err = s.verify().expect_err("tampered");
        assert_eq!(err.code, codes::SNAPSHOT_HASH_MISMATCH);
        assert_eq!(err.detail_str("field"), Some("note_list_hash"));
    }

    #[test]
    fn a_tampered_snapshot_hash_makes_verification_fail() {
        let mut doc = scene();
        if let Json::Obj(m) = &mut doc {
            m.insert(
                "snapshot_hash",
                Json::Str("fnv1a64:0000000000000000".into()),
            );
        }
        let s = Snapshot::from_json(&doc).expect("parse");
        let err = s.verify().expect_err("tampered");
        assert_eq!(err.code, codes::SNAPSHOT_HASH_MISMATCH);
        assert_eq!(err.detail_str("field"), Some("snapshot_hash"));
        assert_eq!(err.detail_str("actual"), Some("fnv1a64:3e7bf035037bcf4f"));
    }

    #[test]
    fn a_tampered_item_length_breaks_only_the_snapshot_hash() {
        let mut doc = scene();
        if let Json::Obj(m) = &mut doc {
            m.insert("item_length_qn", Json::Float(9.0));
        }
        let s = Snapshot::from_json(&doc).expect("parse");
        let err = s.verify().expect_err("tampered");
        assert_eq!(err.detail_str("field"), Some("snapshot_hash"));
    }

    #[test]
    fn a_filtered_snapshot_skips_the_take_domain_hashes() {
        let mut doc = scene();
        if let Json::Obj(m) = &mut doc {
            m.insert("source_note_count", Json::Int(9));
        }
        let s = Snapshot::from_json(&doc).expect("parse");
        assert!(!s.is_unfiltered());
        let checked = s.verify().expect("verifies what it can");
        assert!(!checked.contains(&"midi_hash"));
        assert!(!checked.contains(&"note_selection_hash"));
        assert!(checked.contains(&"note_list_hash"));
        assert!(checked.contains(&"snapshot_hash"));
    }

    #[test]
    fn expected_project_pins_content_not_the_state_counter() {
        let s = Snapshot::from_json(&scene()).expect("parse");
        let e = s.expected_project();
        assert_eq!(e.state_change_count, None);
        assert_eq!(e.snapshot_hash.as_deref(), Some("fnv1a64:3e7bf035037bcf4f"));
        assert_eq!(e.midi_hash.as_deref(), Some("fnv1a64:fac2c019eba29541"));
        assert_eq!(
            e.source_take_guid.as_deref(),
            Some("00000003-0003-4003-8003-000000000003")
        );
    }

    #[test]
    fn a_non_object_is_rejected() {
        let err = Snapshot::from_json(&Json::Int(1)).expect_err("not an object");
        assert_eq!(err.code, codes::INTERNAL_BRIDGE_ERROR);
    }

    #[test]
    fn a_missing_required_number_is_rejected() {
        let mut doc = scene();
        if let Json::Obj(m) = &mut doc {
            m.remove("item_position_qn");
        }
        let err = Snapshot::from_json(&doc).expect_err("missing field");
        assert_eq!(err.detail_str("field"), Some("item_position_qn"));
    }

    #[test]
    fn raw_json_is_preserved_for_fields_this_struct_does_not_model() {
        let s = Snapshot::from_json(&scene()).expect("parse");
        assert_eq!(
            s.raw.get("project_pointer").and_then(Json::as_str),
            Some("MOCK")
        );
        assert_eq!(
            s.raw.get("timestamp").and_then(Json::as_i64),
            Some(1785091879)
        );
    }
}
