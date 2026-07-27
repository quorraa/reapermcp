//! Edit plans: the only description of a change to a REAPER project.
//!
//! An [`EditPlan`] is data, not behaviour. It crosses the IPC boundary as JSON,
//! carries the preconditions that make committing it safe, and states what the
//! bridge is expected to produce. Every type here round-trips through
//! [`to_json`](EditPlan::to_json) / [`from_json`](EditPlan::from_json).

use crate::error::DomainError;
use crate::time::BeatTime;
use qjson::{json_obj, Json, JsonMap};

/// A note as it will be written into a MIDI item.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedNote {
    /// Start position in quarter notes.
    pub start_qn: BeatTime,
    /// End position in quarter notes.
    pub end_qn: BeatTime,
    /// Sounding MIDI pitch.
    pub pitch: i32,
    /// MIDI velocity.
    pub velocity: u8,
    /// MIDI channel.
    pub channel: u8,
    /// Mute state.
    pub muted: bool,
    /// Spelling to record alongside the sounding pitch, e.g. `"Ab4"`.
    pub spelling: String,
}

impl PlannedNote {
    /// Builds a planned note with velocity 96 on channel 0.
    pub fn new(start_qn: BeatTime, end_qn: BeatTime, pitch: i32, spelling: &str) -> PlannedNote {
        PlannedNote {
            start_qn,
            end_qn,
            pitch,
            velocity: 96,
            channel: 0,
            muted: false,
            spelling: spelling.to_string(),
        }
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "start_qn" => self.start_qn.to_json(),
            "end_qn" => self.end_qn.to_json(),
            "pitch" => self.pitch as i64,
            "velocity" => self.velocity as i64,
            "channel" => self.channel as i64,
            "muted" => self.muted,
            "spelling" => self.spelling.clone(),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<PlannedNote, DomainError> {
        Ok(PlannedNote {
            start_qn: BeatTime::from_json(v.field("start_qn")?)?,
            end_qn: BeatTime::from_json(v.field("end_qn")?)?,
            pitch: v.i64_field("pitch")? as i32,
            velocity: v.opt_i64_field("velocity")?.unwrap_or(96).clamp(0, 127) as u8,
            channel: v.opt_i64_field("channel")?.unwrap_or(0).clamp(0, 15) as u8,
            muted: v.opt_bool_field("muted")?.unwrap_or(false),
            spelling: v.opt_str_field("spelling")?.unwrap_or("").to_string(),
        })
    }

    /// Checks range and ordering invariants.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.end_qn <= self.start_qn {
            return Err(DomainError::invalid_edit_plan(format!(
                "planned note at {} has non-positive length",
                self.start_qn
            )));
        }
        if !(0..=127).contains(&self.pitch) {
            return Err(DomainError::invalid_edit_plan(format!(
                "planned note pitch {} is out of range",
                self.pitch
            )));
        }
        if self.velocity == 0 || self.velocity > 127 {
            return Err(DomainError::invalid_edit_plan(format!(
                "planned note velocity {} is out of range",
                self.velocity
            )));
        }
        if self.channel > 15 {
            return Err(DomainError::invalid_edit_plan(format!(
                "planned note channel {} is out of range",
                self.channel
            )));
        }
        Ok(())
    }
}

/// One atomic edit the bridge will perform.
#[derive(Clone, Debug, PartialEq)]
pub enum EditOperation {
    /// Create a folder track to hold generated material.
    CreateFolderTrack {
        /// Plan-local identifier for the new track.
        temp_id: String,
        /// Track name.
        name: String,
        /// Key/value tags written into the track's notes.
        tags: Vec<(String, String)>,
    },
    /// Create a track, optionally inside a folder created earlier in the plan.
    CreateTrack {
        /// Plan-local identifier for the new track.
        temp_id: String,
        /// Parent folder's `temp_id`, when there is one.
        parent: Option<String>,
        /// Track name.
        name: String,
        /// Key/value tags.
        tags: Vec<(String, String)>,
    },
    /// Create an empty MIDI item on a track.
    CreateMidiItem {
        /// Plan-local identifier for the new item.
        temp_id: String,
        /// Target track's `temp_id`.
        track: String,
        /// Item start in quarter notes.
        start_qn: BeatTime,
        /// Item end in quarter notes.
        end_qn: BeatTime,
        /// Key/value tags.
        tags: Vec<(String, String)>,
        /// Whether the item starts muted.
        muted: bool,
    },
    /// Write notes into an item created earlier in the plan.
    InsertNotes {
        /// Target item's `temp_id`.
        item: String,
        /// Notes to write.
        notes: Vec<PlannedNote>,
    },
    /// Mute or unmute a track.
    SetTrackMute {
        /// Target track's `temp_id`.
        track: String,
        /// Desired mute state.
        muted: bool,
    },
    /// Create a project region.
    CreateRegion {
        /// Region name.
        name: String,
        /// Region start in quarter notes.
        start_qn: BeatTime,
        /// Region end in quarter notes.
        end_qn: BeatTime,
    },
    /// Route MIDI from a plan-created track to an existing track.
    CreateMidiSend {
        /// Source track's `temp_id`.
        from_track: String,
        /// Destination track GUID in the host project.
        to_track_guid: String,
    },
}

impl EditOperation {
    /// Stable operation identifier used in JSON.
    pub fn op_id(&self) -> &'static str {
        match self {
            EditOperation::CreateFolderTrack { .. } => "create_folder_track",
            EditOperation::CreateTrack { .. } => "create_track",
            EditOperation::CreateMidiItem { .. } => "create_midi_item",
            EditOperation::InsertNotes { .. } => "insert_notes",
            EditOperation::SetTrackMute { .. } => "set_track_mute",
            EditOperation::CreateRegion { .. } => "create_region",
            EditOperation::CreateMidiSend { .. } => "create_midi_send",
        }
    }

    /// JSON form; the variant is named by the `op` member.
    pub fn to_json(&self) -> Json {
        match self {
            EditOperation::CreateFolderTrack {
                temp_id,
                name,
                tags,
            } => json_obj! {
                "op" => self.op_id(),
                "temp_id" => temp_id.clone(),
                "name" => name.clone(),
                "tags" => tags_to_json(tags),
            },
            EditOperation::CreateTrack {
                temp_id,
                parent,
                name,
                tags,
            } => json_obj! {
                "op" => self.op_id(),
                "temp_id" => temp_id.clone(),
                "parent" => match parent { Some(p) => Json::Str(p.clone()), None => Json::Null },
                "name" => name.clone(),
                "tags" => tags_to_json(tags),
            },
            EditOperation::CreateMidiItem {
                temp_id,
                track,
                start_qn,
                end_qn,
                tags,
                muted,
            } => json_obj! {
                "op" => self.op_id(),
                "temp_id" => temp_id.clone(),
                "track" => track.clone(),
                "start_qn" => start_qn.to_json(),
                "end_qn" => end_qn.to_json(),
                "tags" => tags_to_json(tags),
                "muted" => *muted,
            },
            EditOperation::InsertNotes { item, notes } => json_obj! {
                "op" => self.op_id(),
                "item" => item.clone(),
                "notes" => Json::Arr(notes.iter().map(|n| n.to_json()).collect()),
            },
            EditOperation::SetTrackMute { track, muted } => json_obj! {
                "op" => self.op_id(),
                "track" => track.clone(),
                "muted" => *muted,
            },
            EditOperation::CreateRegion {
                name,
                start_qn,
                end_qn,
            } => json_obj! {
                "op" => self.op_id(),
                "name" => name.clone(),
                "start_qn" => start_qn.to_json(),
                "end_qn" => end_qn.to_json(),
            },
            EditOperation::CreateMidiSend {
                from_track,
                to_track_guid,
            } => json_obj! {
                "op" => self.op_id(),
                "from_track" => from_track.clone(),
                "to_track_guid" => to_track_guid.clone(),
            },
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<EditOperation, DomainError> {
        let op = v.str_field("op")?;
        Ok(match op {
            "create_folder_track" => EditOperation::CreateFolderTrack {
                temp_id: v.str_field("temp_id")?.to_string(),
                name: v.opt_str_field("name")?.unwrap_or("").to_string(),
                tags: tags_from_json(v)?,
            },
            "create_track" => EditOperation::CreateTrack {
                temp_id: v.str_field("temp_id")?.to_string(),
                parent: v.opt_str_field("parent")?.map(|s| s.to_string()),
                name: v.opt_str_field("name")?.unwrap_or("").to_string(),
                tags: tags_from_json(v)?,
            },
            "create_midi_item" => EditOperation::CreateMidiItem {
                temp_id: v.str_field("temp_id")?.to_string(),
                track: v.str_field("track")?.to_string(),
                start_qn: BeatTime::from_json(v.field("start_qn")?)?,
                end_qn: BeatTime::from_json(v.field("end_qn")?)?,
                tags: tags_from_json(v)?,
                muted: v.opt_bool_field("muted")?.unwrap_or(false),
            },
            "insert_notes" => {
                let mut notes = Vec::new();
                for n in v.arr_field("notes")? {
                    notes.push(PlannedNote::from_json(n)?);
                }
                EditOperation::InsertNotes {
                    item: v.str_field("item")?.to_string(),
                    notes,
                }
            }
            "set_track_mute" => EditOperation::SetTrackMute {
                track: v.str_field("track")?.to_string(),
                muted: v.bool_field("muted")?,
            },
            "create_region" => EditOperation::CreateRegion {
                name: v.opt_str_field("name")?.unwrap_or("").to_string(),
                start_qn: BeatTime::from_json(v.field("start_qn")?)?,
                end_qn: BeatTime::from_json(v.field("end_qn")?)?,
            },
            "create_midi_send" => EditOperation::CreateMidiSend {
                from_track: v.str_field("from_track")?.to_string(),
                to_track_guid: v.str_field("to_track_guid")?.to_string(),
            },
            other => {
                return Err(DomainError::invalid_edit_plan(format!(
                    "unknown edit operation {other:?}"
                )))
            }
        })
    }
}

/// Renders tags as a JSON object, preserving insertion order.
fn tags_to_json(tags: &[(String, String)]) -> Json {
    let mut m = JsonMap::new();
    for (k, v) in tags {
        m.insert(k.clone(), Json::Str(v.clone()));
    }
    Json::Obj(m)
}

/// Reads a tag object back into ordered pairs.
fn tags_from_json(v: &Json) -> Result<Vec<(String, String)>, DomainError> {
    let mut out = Vec::new();
    if let Some(Json::Obj(m)) = v.get("tags") {
        for (k, value) in m.iter() {
            let text = value.as_str().ok_or_else(|| {
                DomainError::invalid_edit_plan(format!("tag {k:?} is not a string"))
            })?;
            out.push((k.to_string(), text.to_string()));
        }
    }
    Ok(out)
}

/// A condition that must still hold when the plan is committed.
#[derive(Clone, Debug, PartialEq)]
pub enum Precondition {
    /// The project must still be this one.
    ProjectUuid(String),
    /// The project's state-change counter must match.
    StateChangeCount(i64),
    /// A media item must still exist.
    ItemGuidExists(String),
    /// A take must still exist.
    TakeGuidExists(String),
    /// The source MIDI must hash to this value.
    MidiHash(String),
    /// The tempo map must hash to this value.
    TempoMapHash(String),
    /// The source item must still have these bounds, in quarter notes.
    ItemBounds {
        /// Item start.
        start_qn: f64,
        /// Item end.
        end_qn: f64,
    },
}

impl Precondition {
    /// Stable kind identifier used in JSON.
    pub fn kind_id(&self) -> &'static str {
        match self {
            Precondition::ProjectUuid(_) => "project_uuid",
            Precondition::StateChangeCount(_) => "state_change_count",
            Precondition::ItemGuidExists(_) => "item_guid_exists",
            Precondition::TakeGuidExists(_) => "take_guid_exists",
            Precondition::MidiHash(_) => "midi_hash",
            Precondition::TempoMapHash(_) => "tempo_map_hash",
            Precondition::ItemBounds { .. } => "item_bounds",
        }
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        match self {
            Precondition::ProjectUuid(v)
            | Precondition::ItemGuidExists(v)
            | Precondition::TakeGuidExists(v)
            | Precondition::MidiHash(v)
            | Precondition::TempoMapHash(v) => json_obj! {
                "kind" => self.kind_id(),
                "value" => v.clone(),
            },
            Precondition::StateChangeCount(n) => json_obj! {
                "kind" => self.kind_id(),
                "value" => *n,
            },
            Precondition::ItemBounds { start_qn, end_qn } => json_obj! {
                "kind" => self.kind_id(),
                "start_qn" => *start_qn,
                "end_qn" => *end_qn,
            },
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<Precondition, DomainError> {
        let kind = v.str_field("kind")?;
        Ok(match kind {
            "project_uuid" => Precondition::ProjectUuid(v.str_field("value")?.to_string()),
            "state_change_count" => Precondition::StateChangeCount(v.i64_field("value")?),
            "item_guid_exists" => Precondition::ItemGuidExists(v.str_field("value")?.to_string()),
            "take_guid_exists" => Precondition::TakeGuidExists(v.str_field("value")?.to_string()),
            "midi_hash" => Precondition::MidiHash(v.str_field("value")?.to_string()),
            "tempo_map_hash" => Precondition::TempoMapHash(v.str_field("value")?.to_string()),
            "item_bounds" => Precondition::ItemBounds {
                start_qn: v.f64_field("start_qn")?,
                end_qn: v.f64_field("end_qn")?,
            },
            other => {
                return Err(DomainError::invalid_edit_plan(format!(
                    "unknown precondition {other:?}"
                )))
            }
        })
    }
}

/// What the bridge is expected to have produced for one plan-local id.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpectedOutput {
    /// Plan-local identifier from an earlier operation.
    pub temp_id: String,
    /// What it should be: `"track"`, `"item"`, `"region"`, …
    pub kind: String,
    /// Expected note count, for items.
    pub note_count: Option<usize>,
}

impl ExpectedOutput {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "temp_id" => self.temp_id.clone(),
            "kind" => self.kind.clone(),
            "note_count" => match self.note_count { Some(n) => Json::Int(n as i64), None => Json::Null },
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<ExpectedOutput, DomainError> {
        Ok(ExpectedOutput {
            temp_id: v.str_field("temp_id")?.to_string(),
            kind: v.str_field("kind")?.to_string(),
            note_count: v.opt_i64_field("note_count")?.map(|n| n.max(0) as usize),
        })
    }
}

/// A complete, atomic description of a change to the host project.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditPlan {
    /// Stable plan id.
    pub plan_id: String,
    /// Candidate this plan realizes.
    pub candidate_id: String,
    /// Transaction id used for undo ownership.
    pub transaction_id: String,
    /// Snapshot the plan was built against.
    pub base_snapshot_id: String,
    /// Hash of that snapshot.
    pub base_snapshot_hash: String,
    /// Project the plan belongs to.
    pub project_uuid: String,
    /// Knowledge-base version used.
    pub knowledge_version: String,
    /// Label for the host's undo stack.
    pub undo_label: String,
    /// Operations, in execution order.
    pub operations: Vec<EditOperation>,
    /// Conditions checked immediately before executing.
    pub preconditions: Vec<Precondition>,
    /// What the bridge should report back.
    pub expected_outputs: Vec<ExpectedOutput>,
}

impl EditPlan {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "plan_id" => self.plan_id.clone(),
            "candidate_id" => self.candidate_id.clone(),
            "transaction_id" => self.transaction_id.clone(),
            "base_snapshot_id" => self.base_snapshot_id.clone(),
            "base_snapshot_hash" => self.base_snapshot_hash.clone(),
            "project_uuid" => self.project_uuid.clone(),
            "knowledge_version" => self.knowledge_version.clone(),
            "undo_label" => self.undo_label.clone(),
            "operations" => Json::Arr(self.operations.iter().map(|o| o.to_json()).collect()),
            "preconditions" => Json::Arr(self.preconditions.iter().map(|p| p.to_json()).collect()),
            "expected_outputs" => Json::Arr(self.expected_outputs.iter().map(|e| e.to_json()).collect()),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<EditPlan, DomainError> {
        let mut operations = Vec::new();
        for o in v.arr_field("operations")? {
            operations.push(EditOperation::from_json(o)?);
        }
        let mut preconditions = Vec::new();
        if let Some(Json::Arr(a)) = v.get("preconditions") {
            for p in a {
                preconditions.push(Precondition::from_json(p)?);
            }
        }
        let mut expected_outputs = Vec::new();
        if let Some(Json::Arr(a)) = v.get("expected_outputs") {
            for e in a {
                expected_outputs.push(ExpectedOutput::from_json(e)?);
            }
        }
        Ok(EditPlan {
            plan_id: v.str_field("plan_id")?.to_string(),
            candidate_id: v.opt_str_field("candidate_id")?.unwrap_or("").to_string(),
            transaction_id: v.opt_str_field("transaction_id")?.unwrap_or("").to_string(),
            base_snapshot_id: v
                .opt_str_field("base_snapshot_id")?
                .unwrap_or("")
                .to_string(),
            base_snapshot_hash: v
                .opt_str_field("base_snapshot_hash")?
                .unwrap_or("")
                .to_string(),
            project_uuid: v.opt_str_field("project_uuid")?.unwrap_or("").to_string(),
            knowledge_version: v
                .opt_str_field("knowledge_version")?
                .unwrap_or("")
                .to_string(),
            undo_label: v.opt_str_field("undo_label")?.unwrap_or("").to_string(),
            operations,
            preconditions,
            expected_outputs,
        })
    }

    /// Total number of notes the plan writes.
    pub fn note_count(&self) -> usize {
        self.operations
            .iter()
            .map(|o| match o {
                EditOperation::InsertNotes { notes, .. } => notes.len(),
                _ => 0,
            })
            .sum()
    }

    /// SHA-256 of the canonical JSON form.
    pub fn hash_hex(&self) -> String {
        qjson::sha256::sha256_hex(self.to_json().to_canonical_string().as_bytes())
    }

    /// Checks internal consistency: every referenced `temp_id` must have been
    /// created earlier in the plan, and every planned note must be well formed.
    pub fn validate(&self) -> Result<(), DomainError> {
        let mut tracks: Vec<&str> = Vec::new();
        let mut items: Vec<&str> = Vec::new();
        for op in &self.operations {
            match op {
                EditOperation::CreateFolderTrack { temp_id, .. } => tracks.push(temp_id),
                EditOperation::CreateTrack {
                    temp_id, parent, ..
                } => {
                    if let Some(p) = parent {
                        if !tracks.contains(&p.as_str()) {
                            return Err(DomainError::invalid_edit_plan(format!(
                                "track {temp_id:?} names unknown parent {p:?}"
                            )));
                        }
                    }
                    tracks.push(temp_id);
                }
                EditOperation::CreateMidiItem { temp_id, track, .. } => {
                    if !tracks.contains(&track.as_str()) {
                        return Err(DomainError::invalid_edit_plan(format!(
                            "item {temp_id:?} names unknown track {track:?}"
                        )));
                    }
                    items.push(temp_id);
                }
                EditOperation::InsertNotes { item, notes } => {
                    if !items.contains(&item.as_str()) {
                        return Err(DomainError::invalid_edit_plan(format!(
                            "insert_notes names unknown item {item:?}"
                        )));
                    }
                    for n in notes {
                        n.validate()?;
                    }
                }
                EditOperation::SetTrackMute { track, .. } => {
                    if !tracks.contains(&track.as_str()) {
                        return Err(DomainError::invalid_edit_plan(format!(
                            "set_track_mute names unknown track {track:?}"
                        )));
                    }
                }
                EditOperation::CreateMidiSend { from_track, .. } => {
                    if !tracks.contains(&from_track.as_str()) {
                        return Err(DomainError::invalid_edit_plan(format!(
                            "create_midi_send names unknown track {from_track:?}"
                        )));
                    }
                }
                EditOperation::CreateRegion {
                    start_qn, end_qn, ..
                } => {
                    if end_qn <= start_qn {
                        return Err(DomainError::invalid_edit_plan(
                            "region end must be after its start",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qn(n: i64) -> BeatTime {
        BeatTime::from_quarters(n)
    }

    fn sample_plan() -> EditPlan {
        EditPlan {
            plan_id: "plan-1".to_string(),
            candidate_id: "cand-1".to_string(),
            transaction_id: "tx-1".to_string(),
            base_snapshot_id: "snap-1".to_string(),
            base_snapshot_hash: "abc".to_string(),
            project_uuid: "proj-1".to_string(),
            knowledge_version: "1.0.0".to_string(),
            undo_label: "QLabs: harmonize".to_string(),
            operations: vec![
                EditOperation::CreateFolderTrack {
                    temp_id: "folder".to_string(),
                    name: "QLabs".to_string(),
                    tags: vec![("qlabs_candidate".to_string(), "cand-1".to_string())],
                },
                EditOperation::CreateTrack {
                    temp_id: "track".to_string(),
                    parent: Some("folder".to_string()),
                    name: "Harmony".to_string(),
                    tags: vec![],
                },
                EditOperation::CreateMidiItem {
                    temp_id: "item".to_string(),
                    track: "track".to_string(),
                    start_qn: qn(0),
                    end_qn: qn(16),
                    tags: vec![("role".to_string(), "harmonic_bed".to_string())],
                    muted: false,
                },
                EditOperation::InsertNotes {
                    item: "item".to_string(),
                    notes: vec![
                        PlannedNote::new(qn(0), qn(4), 60, "C4"),
                        PlannedNote::new(qn(0), qn(4), 64, "E4"),
                    ],
                },
                EditOperation::SetTrackMute {
                    track: "track".to_string(),
                    muted: false,
                },
                EditOperation::CreateRegion {
                    name: "QLabs candidate".to_string(),
                    start_qn: qn(0),
                    end_qn: qn(16),
                },
                EditOperation::CreateMidiSend {
                    from_track: "track".to_string(),
                    to_track_guid: "{TRACK}".to_string(),
                },
            ],
            preconditions: vec![
                Precondition::ProjectUuid("proj-1".to_string()),
                Precondition::StateChangeCount(42),
                Precondition::ItemGuidExists("{ITEM}".to_string()),
                Precondition::TakeGuidExists("{TAKE}".to_string()),
                Precondition::MidiHash("deadbeef".to_string()),
                Precondition::TempoMapHash("cafe".to_string()),
                Precondition::ItemBounds {
                    start_qn: 0.0,
                    end_qn: 16.0,
                },
            ],
            expected_outputs: vec![ExpectedOutput {
                temp_id: "item".to_string(),
                kind: "item".to_string(),
                note_count: Some(2),
            }],
        }
    }

    #[test]
    fn planned_note_json_round_trip() {
        let mut n = PlannedNote::new(qn(0), qn(2), 67, "G4");
        n.velocity = 110;
        n.channel = 3;
        n.muted = true;
        assert_eq!(PlannedNote::from_json(&n.to_json()).unwrap(), n);
    }

    #[test]
    fn planned_note_validation() {
        assert!(PlannedNote::new(qn(0), qn(1), 60, "C4").validate().is_ok());
        let mut n = PlannedNote::new(qn(1), qn(1), 60, "C4");
        assert!(n.validate().is_err());
        n = PlannedNote::new(qn(0), qn(1), 200, "C4");
        assert!(n.validate().is_err());
        n = PlannedNote::new(qn(0), qn(1), 60, "C4");
        n.velocity = 0;
        assert!(n.validate().is_err());
        n = PlannedNote::new(qn(0), qn(1), 60, "C4");
        n.channel = 20;
        assert!(n.validate().is_err());
    }

    #[test]
    fn planned_note_keeps_spelling_next_to_the_sounding_pitch() {
        let n = PlannedNote::new(qn(0), qn(1), 68, "Ab4");
        assert_eq!(n.pitch, 68);
        assert_eq!(n.spelling, "Ab4");
        let json = n.to_json();
        assert_eq!(json.str_field("spelling").unwrap(), "Ab4");
        assert_eq!(json.str_field("start_qn").unwrap(), "0");
    }

    #[test]
    fn every_operation_round_trips() {
        for op in sample_plan().operations {
            let back = EditOperation::from_json(&op.to_json())
                .unwrap_or_else(|e| panic!("{}: {e}", op.op_id()));
            assert_eq!(back, op);
        }
    }

    #[test]
    fn operation_ids_are_distinct() {
        let ids: Vec<&str> = sample_plan().operations.iter().map(|o| o.op_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len());
    }

    #[test]
    fn unknown_operation_is_rejected() {
        let bad = json_obj! { "op" => "delete_everything" };
        let err = EditOperation::from_json(&bad).unwrap_err();
        assert_eq!(err.code, "INVALID_EDIT_PLAN");
    }

    #[test]
    fn every_precondition_round_trips() {
        for p in sample_plan().preconditions {
            let back = EditPrecheck::check(&p);
            assert_eq!(back, p);
        }
    }

    /// Helper so the precondition round trip reads clearly.
    struct EditPrecheck;
    impl EditPrecheck {
        fn check(p: &Precondition) -> Precondition {
            Precondition::from_json(&p.to_json()).expect("round trip")
        }
    }

    #[test]
    fn unknown_precondition_is_rejected() {
        let bad = json_obj! { "kind" => "phase_of_the_moon" };
        assert!(Precondition::from_json(&bad).is_err());
    }

    #[test]
    fn expected_output_round_trips() {
        let e = ExpectedOutput {
            temp_id: "item".to_string(),
            kind: "item".to_string(),
            note_count: Some(8),
        };
        assert_eq!(ExpectedOutput::from_json(&e.to_json()).unwrap(), e);
        let none = ExpectedOutput {
            note_count: None,
            ..e
        };
        assert_eq!(ExpectedOutput::from_json(&none.to_json()).unwrap(), none);
    }

    #[test]
    fn edit_plan_json_round_trip_and_hash() {
        let p = sample_plan();
        let back = EditPlan::from_json(&p.to_json()).expect("round trip");
        assert_eq!(back, p);
        assert_eq!(back.hash_hex(), p.hash_hex());
        assert_eq!(p.hash_hex().len(), 64);
        assert_eq!(p.note_count(), 2);
    }

    #[test]
    fn edit_plan_validation_accepts_a_consistent_plan() {
        assert!(sample_plan().validate().is_ok());
    }

    #[test]
    fn edit_plan_validation_catches_dangling_references() {
        let mut p = sample_plan();
        p.operations[3] = EditOperation::InsertNotes {
            item: "nope".to_string(),
            notes: vec![],
        };
        assert!(p.validate().is_err());
        let mut p = sample_plan();
        p.operations[2] = EditOperation::CreateMidiItem {
            temp_id: "item".to_string(),
            track: "ghost".to_string(),
            start_qn: qn(0),
            end_qn: qn(4),
            tags: vec![],
            muted: false,
        };
        assert!(p.validate().is_err());
        let mut p = sample_plan();
        p.operations[1] = EditOperation::CreateTrack {
            temp_id: "track".to_string(),
            parent: Some("ghost".to_string()),
            name: "x".to_string(),
            tags: vec![],
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn edit_plan_validation_catches_bad_regions_and_notes() {
        let mut p = sample_plan();
        p.operations[5] = EditOperation::CreateRegion {
            name: "bad".to_string(),
            start_qn: qn(8),
            end_qn: qn(8),
        };
        assert!(p.validate().is_err());
        let mut p = sample_plan();
        p.operations[3] = EditOperation::InsertNotes {
            item: "item".to_string(),
            notes: vec![PlannedNote::new(qn(4), qn(0), 60, "C4")],
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn tags_survive_the_round_trip_in_order() {
        let op = EditOperation::CreateTrack {
            temp_id: "t".to_string(),
            parent: None,
            name: "T".to_string(),
            tags: vec![
                ("b".to_string(), "2".to_string()),
                ("a".to_string(), "1".to_string()),
            ],
        };
        let back = EditOperation::from_json(&op.to_json()).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn missing_required_fields_are_reported() {
        let bad = json_obj! { "op" => "create_track" };
        assert!(EditOperation::from_json(&bad).is_err());
        let bad = json_obj! { "candidate_id" => "x" };
        assert!(EditPlan::from_json(&bad).is_err());
    }

    #[test]
    fn default_plan_is_empty_but_valid() {
        let p = EditPlan::default();
        assert!(p.validate().is_ok());
        assert_eq!(p.note_count(), 0);
    }
}
