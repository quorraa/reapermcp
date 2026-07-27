//! Building an [`EditPlan`] from a candidate.
//!
//! This is the safety-critical module. Everything it produces is
//! **additive**: new tracks inside a new folder, new muted MIDI items, and
//! optionally a region and one explicitly requested MIDI send. No operation in
//! the plan can touch the source item, the source take or any pre-existing
//! track, because the operation vocabulary has no way to express it.
//!
//! # The seven preconditions
//!
//! `IPC_WIRE.md` §6.4.3 evaluates a plan's `preconditions` inside
//! `transactions.lua` **before** the undo block opens, and `expected_project`
//! is only consulted on `inspect_selection`. Staleness protection therefore
//! lives entirely in the plan, and every plan this module builds carries all
//! seven:
//!
//! | Precondition | Rejects |
//! |---|---|
//! | `ProjectUuid` | the project was swapped |
//! | `StateChangeCount` | anything at all changed |
//! | `ItemGuidExists` | the source item was deleted |
//! | `TakeGuidExists` | the source take was deleted |
//! | `MidiHash` | a note was edited |
//! | `TempoMapHash` | the tempo map was edited |
//! | `ItemBounds` | the item was moved or resized |
//!
//! `StateChangeCount` is the strictest of the seven and, as `IPC_WIRE.md` §5
//! notes, it also increments on a benign selection change. It is included
//! anyway because staging writes into the user's project: a spurious
//! `PROJECT_CHANGED` costs one re-inspection, whereas a missed change costs
//! material written against a melody that no longer exists.
//!
//! # Wire conversion
//!
//! The plan leaves this module as a [`EditPlan`] value and crosses to the
//! bridge through [`reaper_ipc::plan::plan_to_wire`] — never
//! [`EditPlan::to_json`], whose precondition discriminator, `BeatTime`
//! encoding and tag shape all differ from what the Lua side reads.
//! [`EditPlan::to_json`] is what the `editplan://{id}` resource serves, where
//! the exact rational strings are the better representation.

use crate::error::ToolError;
use crate::store::{ScopeEcho, SnapshotRecord};
use music_domain::plan::{EditOperation, EditPlan, ExpectedOutput, PlannedNote, Precondition};
use music_domain::prelude::*;
use qjson::json_obj;
use reaper_ipc::limits;

/// The undo-label prefix the bridge requires. Any other prefix is refused.
pub const UNDO_PREFIX: &str = "QLabs MCP: ";

/// The default folder name for a staged candidate.
pub const DEFAULT_FOLDER_NAME: &str = "QLabs Candidate";

/// The default track-name prefix.
pub const DEFAULT_TRACK_PREFIX: &str = "QLabs ";

/// The ownership tag key a plan is allowed to supply.
pub const ROLE_TAG: &str = "QLABS_ROLE";

/// How the caller wants the candidate staged.
#[derive(Clone, Debug)]
pub struct StageOptions {
    /// Prefix for every generated track name.
    pub track_name_prefix: String,
    /// Name of the folder that holds this candidate.
    pub folder_name: String,
    /// Whether generated items start muted. Defaults to true: a preview must
    /// not change what the user hears until they commit.
    pub muted: bool,
    /// Whether to create a region spanning the staged material.
    pub create_region: bool,
    /// Whether to route the harmony track's MIDI to the source track.
    pub route_to_source_track: bool,
}

impl Default for StageOptions {
    fn default() -> Self {
        StageOptions {
            track_name_prefix: DEFAULT_TRACK_PREFIX.to_string(),
            folder_name: DEFAULT_FOLDER_NAME.to_string(),
            muted: true,
            create_region: false,
            route_to_source_track: false,
        }
    }
}

/// A plan plus what the caller needs to know about it.
#[derive(Clone, Debug)]
pub struct BuiltPlan {
    /// The plan itself.
    pub plan: EditPlan,
    /// The scope the plan was generated against; `stage_candidate` must echo it.
    pub scope: ScopeEcho,
    /// How many notes the plan writes.
    pub note_count: usize,
}

/// The seven precondition kinds every plan must carry.
pub const REQUIRED_PRECONDITIONS: &[&str] = &[
    "project_uuid",
    "state_change_count",
    "item_guid_exists",
    "take_guid_exists",
    "midi_hash",
    "tempo_map_hash",
    "item_bounds",
];

/// True when `plan` carries every one of [`REQUIRED_PRECONDITIONS`].
pub fn has_all_preconditions(plan: &EditPlan) -> bool {
    REQUIRED_PRECONDITIONS
        .iter()
        .all(|want| plan.preconditions.iter().any(|p| p.kind_id() == *want))
}

/// Every precondition kind a plan carries, in plan order.
pub fn precondition_kinds(plan: &EditPlan) -> Vec<String> {
    plan.preconditions
        .iter()
        .map(|p| p.kind_id().to_string())
        .collect()
}

fn missing(field: &str) -> ToolError {
    ToolError::with_details(
        reaper_ipc::codes::STALE_SNAPSHOT,
        format!("the snapshot has no {field}, so a safe edit plan cannot be built"),
        json_obj! { "missing_field" => field },
    )
    .remedy("call reaper.inspect_selection again against a saved project")
}

/// Sanitises a user-supplied name for a REAPER object.
///
/// Control characters are stripped and the result is truncated to the bridge's
/// 200-byte limit at a character boundary. A name is cosmetic — it is never
/// used to identify an object — so silently cleaning it is safe.
pub fn clean_name(raw: &str, fallback: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string();
    let cleaned = if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    };
    let mut out = String::new();
    for c in cleaned.chars() {
        if out.len() + c.len_utf8() > 180 {
            break;
        }
        out.push(c);
    }
    out
}

/// Builds the edit plan that stages `candidate` against `record`.
///
/// Fails rather than degrading when the snapshot is missing an identity or a
/// hash: a plan without preconditions would stage successfully against material
/// that had already changed, which is precisely the failure mode the product
/// exists to prevent.
pub fn build_plan(
    candidate: &Candidate,
    record: &SnapshotRecord,
    knowledge_version: &str,
    options: &StageOptions,
    ids: &qjson::uuid::UuidGen,
) -> Result<BuiltPlan, ToolError> {
    let snapshot = &record.snapshot;
    let project_uuid = snapshot
        .project_uuid
        .clone()
        .ok_or_else(|| missing("project_uuid"))?;
    let item_guid = snapshot
        .item_guid
        .clone()
        .ok_or_else(|| missing("item_guid"))?;
    let take_guid = snapshot
        .take_guid
        .clone()
        .ok_or_else(|| missing("take_guid"))?;
    let midi_hash = snapshot
        .midi_hash
        .clone()
        .ok_or_else(|| missing("midi_hash"))?;
    let tempo_map_hash = snapshot
        .tempo_map_hash
        .clone()
        .ok_or_else(|| missing("tempo_map_hash"))?;
    let base_snapshot_hash = snapshot
        .snapshot_hash
        .clone()
        .ok_or_else(|| missing("snapshot_hash"))?;

    let parts: Vec<&Part> = candidate
        .parts
        .iter()
        .filter(|p| !p.notes.is_empty())
        .collect();
    if parts.is_empty() {
        return Err(ToolError::invalid_argument(format!(
            "candidate {} has no notes to stage",
            candidate.id
        )));
    }
    if parts.len() > limits::MAX_TRACKS_PER_STAGE - 1 {
        return Err(ToolError::invalid_argument(format!(
            "candidate {} needs {} tracks; the bridge allows {}",
            candidate.id,
            parts.len() + 1,
            limits::MAX_TRACKS_PER_STAGE
        )));
    }

    let (item_start, item_end) = crate::convert::item_bounds(snapshot);
    let mut start_qn = item_start;
    let mut end_qn = item_end;
    for part in &parts {
        for n in &part.notes {
            start_qn = start_qn.min(n.onset.as_f64());
            end_qn = end_qn.max(n.end().as_f64());
        }
    }
    if !(start_qn.is_finite() && end_qn.is_finite()) {
        return Err(ToolError::internal(
            "the candidate spans a non-finite range of quarter notes",
        ));
    }
    if end_qn <= start_qn {
        end_qn = start_qn + 1.0;
    }
    if start_qn < -100_000.0 || end_qn > 1_000_000.0 {
        return Err(ToolError::invalid_argument(
            "the candidate lies outside the quarter-note range the bridge accepts",
        ));
    }
    let item_start_bt = BeatTime::from_f64_grid(start_qn, crate::convert::QN_GRID);
    let item_end_bt = BeatTime::from_f64_grid(end_qn, crate::convert::QN_GRID);

    let transaction_id = ids.next();
    let plan_id = ids.next();
    let short = transaction_id.chars().take(8).collect::<String>();
    let undo_label = format!("{UNDO_PREFIX}Stage candidate {short}");

    let folder_name = clean_name(&options.folder_name, DEFAULT_FOLDER_NAME);
    let prefix = clean_name(&options.track_name_prefix, DEFAULT_TRACK_PREFIX);

    let mut operations: Vec<EditOperation> = Vec::new();
    let mut expected: Vec<ExpectedOutput> = Vec::new();

    operations.push(EditOperation::CreateFolderTrack {
        temp_id: "f0".to_string(),
        name: folder_name.clone(),
        tags: vec![(ROLE_TAG.to_string(), "candidate_folder".to_string())],
    });
    expected.push(ExpectedOutput {
        temp_id: "f0".to_string(),
        kind: "folder_track".to_string(),
        note_count: None,
    });

    let mut total_notes = 0usize;
    let mut first_track: Option<String> = None;
    for (i, part) in parts.iter().enumerate() {
        let track_id = format!("t{i}");
        let item_id = format!("i{i}");
        if first_track.is_none() {
            first_track = Some(track_id.clone());
        }
        let role = part.role.id().to_string();
        let track_name = clean_name(
            &format!("{prefix}{}", part.name),
            &format!("{prefix}{role}"),
        );
        operations.push(EditOperation::CreateTrack {
            temp_id: track_id.clone(),
            parent: Some("f0".to_string()),
            name: track_name,
            tags: vec![(ROLE_TAG.to_string(), role)],
        });
        expected.push(ExpectedOutput {
            temp_id: track_id.clone(),
            kind: "track".to_string(),
            note_count: None,
        });

        operations.push(EditOperation::CreateMidiItem {
            temp_id: item_id.clone(),
            track: track_id.clone(),
            start_qn: item_start_bt,
            end_qn: item_end_bt,
            tags: Vec::new(),
            muted: options.muted,
        });

        let mut notes = Vec::with_capacity(part.notes.len());
        for n in &part.notes {
            let note = planned_note(n, item_start_bt, item_end_bt)?;
            notes.push(note);
        }
        if notes.len() > limits::MAX_NOTES_PER_ITEM {
            return Err(ToolError::invalid_argument(format!(
                "part {} has {} notes; the bridge allows {} per item",
                part.name,
                notes.len(),
                limits::MAX_NOTES_PER_ITEM
            )));
        }
        total_notes += notes.len();
        expected.push(ExpectedOutput {
            temp_id: item_id.clone(),
            kind: "midi_item".to_string(),
            note_count: Some(notes.len()),
        });
        operations.push(EditOperation::InsertNotes {
            item: item_id,
            notes,
        });
    }

    if total_notes > limits::MAX_GENERATED_NOTES {
        return Err(ToolError::invalid_argument(format!(
            "the candidate writes {total_notes} notes; the bridge allows {}",
            limits::MAX_GENERATED_NOTES
        )));
    }

    if options.create_region {
        operations.push(EditOperation::CreateRegion {
            temp_id: "r0".to_string(),
            name: folder_name.clone(),
            start_qn: item_start_bt,
            end_qn: item_end_bt,
        });
        expected.push(ExpectedOutput {
            temp_id: "r0".to_string(),
            kind: "region".to_string(),
            note_count: None,
        });
    }

    if options.route_to_source_track {
        let track_guid = snapshot.track_guid.clone().ok_or_else(|| {
            ToolError::invalid_argument(
                "route_to_source_track was requested but the snapshot has no track guid",
            )
        })?;
        let from = first_track.clone().ok_or_else(|| {
            ToolError::internal("route_to_source_track was requested with no generated track")
        })?;
        operations.push(EditOperation::CreateMidiSend {
            temp_id: "s0".to_string(),
            from_track: from,
            to_track_guid: track_guid,
        });
        expected.push(ExpectedOutput {
            temp_id: "s0".to_string(),
            kind: "send".to_string(),
            note_count: None,
        });
    }

    if operations.len() > limits::MAX_OPERATIONS {
        return Err(ToolError::invalid_argument(format!(
            "the plan needs {} operations; the bridge allows {}",
            operations.len(),
            limits::MAX_OPERATIONS
        )));
    }

    let preconditions = vec![
        Precondition::ProjectUuid(project_uuid.clone()),
        Precondition::StateChangeCount(snapshot.project_state_change_count),
        Precondition::ItemGuidExists(item_guid),
        Precondition::TakeGuidExists(take_guid),
        Precondition::MidiHash(midi_hash),
        Precondition::TempoMapHash(tempo_map_hash),
        Precondition::ItemBounds {
            start_qn: item_start,
            end_qn: item_end,
        },
    ];

    let plan = EditPlan {
        plan_id,
        candidate_id: candidate.id.clone(),
        transaction_id,
        base_snapshot_id: snapshot.snapshot_id.clone(),
        base_snapshot_hash,
        project_uuid,
        knowledge_version: knowledge_version.to_string(),
        undo_label,
        operations,
        preconditions,
        expected_outputs: expected,
    };

    plan.validate()?;
    reaper_ipc::client::preflight_plan(&plan)?;
    debug_assert!(has_all_preconditions(&plan));

    Ok(BuiltPlan {
        plan,
        scope: record.scope.clone(),
        note_count: total_notes,
    })
}

/// Converts a generated note into a plan note, clamped into the item's bounds.
fn planned_note(n: &Note, start: BeatTime, end: BeatTime) -> Result<PlannedNote, ToolError> {
    if !(0..=127).contains(&n.midi) {
        return Err(ToolError::invalid_argument(format!(
            "generated note pitch {} is outside 0..=127",
            n.midi
        )));
    }
    let mut a = n.onset.max(start);
    let mut b = n.end().min(end);
    if b <= a {
        // A note the item cannot hold would be rejected by the bridge as
        // INVALID_EDIT_PLAN; nudge it to the smallest representable span
        // inside the item instead of dropping music the user asked for.
        a = a
            .min(end - BeatTime::new(1, crate::convert::QN_GRID))
            .max(start);
        b = a + BeatTime::new(1, crate::convert::QN_GRID);
    }
    Ok(PlannedNote {
        start_qn: a,
        end_qn: b,
        pitch: n.midi,
        velocity: n.velocity.clamp(1, 127),
        channel: n.channel.min(15),
        muted: n.muted,
        spelling: n.pitch.to_ascii(),
    })
}

/// The payload `stage_candidate` sends, with the scope echoed.
///
/// Per binding note 3 the bridge re-derives the snapshot to check staleness, so
/// omitting the scope produces a spurious `STALE_SNAPSHOT` rather than a
/// genuine one.
pub fn stage_payload(built: &BuiltPlan, verify_snapshot: bool) -> qjson::Json {
    let mut m = match built.scope.to_json() {
        qjson::Json::Obj(m) => m,
        _ => qjson::JsonMap::new(),
    };
    m.insert("verify_snapshot", qjson::Json::Bool(verify_snapshot));
    qjson::Json::Obj(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ScopeEcho;
    use qjson::Json;

    fn record() -> SnapshotRecord {
        let raw = reaper_ipc::testing::scene_snapshot();
        let snapshot = reaper_ipc::Snapshot::from_json(&raw).expect("scene");
        let notes = crate::convert::note_set_of(&snapshot).expect("notes");
        SnapshotRecord {
            snapshot,
            scope: ScopeEcho {
                source_mode: "auto".into(),
                note_scope: "selected_or_all".into(),
                extraction_mode: "auto".into(),
                extraction_channel: None,
            },
            notes,
            raw,
        }
    }

    fn candidate() -> Candidate {
        let mut notes = Vec::new();
        for i in 0..4u32 {
            let mut n = Note::new(
                1000 + i,
                SpelledPitch::parse("C3").unwrap(),
                BeatTime::from_quarters(i as i64),
                BeatTime::from_quarters(1),
            );
            n.midi = 48 + i as i32;
            n.velocity = 80;
            notes.push(n);
        }
        Candidate {
            id: "cand-1".into(),
            kind: CandidateKind::Harmonization,
            label: "Functional".into(),
            strategy: "functional".into(),
            chords: Vec::new(),
            parts: vec![Part {
                role: ArrangementRole::HarmonicBed,
                name: "Chords".into(),
                notes,
                instrument_profile: Some("piano_keys".into()),
                channel: 0,
                polyphonic: true,
            }],
            trace: DecisionTrace::default(),
            loop_report: None,
            created_at: String::new(),
            expires_at: String::new(),
        }
    }

    fn build() -> BuiltPlan {
        build_plan(
            &candidate(),
            &record(),
            "2026.07.1",
            &StageOptions::default(),
            &qjson::uuid::UuidGen::from_seed(7),
        )
        .expect("plan builds")
    }

    #[test]
    fn a_generated_plan_carries_all_seven_preconditions() {
        let built = build();
        assert!(has_all_preconditions(&built.plan));
        let kinds = precondition_kinds(&built.plan);
        for want in REQUIRED_PRECONDITIONS {
            assert!(kinds.contains(&want.to_string()), "missing {want}");
        }
        assert_eq!(kinds.len(), 7);
    }

    #[test]
    fn the_undo_label_uses_the_required_prefix() {
        let built = build();
        assert!(
            built.plan.undo_label.starts_with(UNDO_PREFIX),
            "{}",
            built.plan.undo_label
        );
        assert!(built.plan.undo_label.len() <= limits::MAX_UNDO_LABEL_BYTES);
    }

    #[test]
    fn the_plan_only_creates_and_never_edits_the_source() {
        let built = build();
        for op in &built.plan.operations {
            match op {
                EditOperation::CreateFolderTrack { .. }
                | EditOperation::CreateTrack { .. }
                | EditOperation::CreateMidiItem { .. }
                | EditOperation::InsertNotes { .. }
                | EditOperation::SetTrackMute { .. }
                | EditOperation::CreateRegion { .. }
                | EditOperation::CreateMidiSend { .. } => {}
            }
        }
        // The source take guid never appears as a write target.
        let wire = reaper_ipc::plan::plan_to_wire(&built.plan).to_string();
        let take = record().snapshot.take_guid.unwrap();
        let ops = built
            .plan
            .operations
            .iter()
            .map(|o| o.op_id())
            .collect::<Vec<_>>();
        assert!(ops.contains(&"create_folder_track"));
        assert!(!ops.contains(&"create_midi_send"));
        assert!(
            !wire.contains(&format!("\"item\":\"{take}\"")),
            "no operation may write into the source take"
        );
    }

    #[test]
    fn items_start_muted_by_default() {
        let built = build();
        let muted = built
            .plan
            .operations
            .iter()
            .any(|op| matches!(op, EditOperation::CreateMidiItem { muted, .. } if *muted));
        assert!(muted, "a preview must start muted");
    }

    #[test]
    fn notes_lie_within_their_item_bounds() {
        let built = build();
        let mut bounds = None;
        for op in &built.plan.operations {
            match op {
                EditOperation::CreateMidiItem {
                    start_qn, end_qn, ..
                } => {
                    bounds = Some((*start_qn, *end_qn));
                }
                EditOperation::InsertNotes { notes, .. } => {
                    let (a, b) = bounds.expect("an item precedes its notes");
                    for n in notes {
                        assert!(n.start_qn >= a && n.end_qn <= b, "{n:?} outside [{a}, {b}]");
                        assert!(n.end_qn > n.start_qn);
                    }
                }
                _ => {}
            }
        }
    }

    #[test]
    fn the_wire_form_uses_numbers_and_type_discriminators() {
        let built = build();
        let wire = reaper_ipc::plan::plan_to_wire(&built.plan);
        let pres = wire.arr_field("preconditions").unwrap();
        assert!(!pres.is_empty());
        for p in pres {
            assert!(p.get("type").is_some(), "the wire uses `type`, not `kind`");
            assert!(p.get("kind").is_none());
        }
        let ops = wire.arr_field("operations").unwrap();
        let item = ops
            .iter()
            .find(|o| o.str_field("op") == Ok("create_midi_item"))
            .unwrap();
        assert!(matches!(
            item.get("start_qn"),
            Some(Json::Float(_)) | Some(Json::Int(_))
        ));
    }

    #[test]
    fn the_domain_form_keeps_rational_strings() {
        let built = build();
        let domain = built.plan.to_json();
        let ops = domain.arr_field("operations").unwrap();
        let item = ops
            .iter()
            .find(|o| o.str_field("op") == Ok("create_midi_item"))
            .unwrap();
        assert!(
            matches!(item.get("start_qn"), Some(Json::Str(_))),
            "editplan:// serves exact rationals"
        );
    }

    #[test]
    fn a_missing_snapshot_hash_refuses_to_build_a_plan() {
        let mut r = record();
        r.snapshot.snapshot_hash = None;
        let e = build_plan(
            &candidate(),
            &r,
            "2026.07.1",
            &StageOptions::default(),
            &qjson::uuid::UuidGen::from_seed(1),
        )
        .unwrap_err();
        assert_eq!(e.code, reaper_ipc::codes::STALE_SNAPSHOT);
    }

    #[test]
    fn a_missing_midi_hash_refuses_to_build_a_plan() {
        let mut r = record();
        r.snapshot.midi_hash = None;
        assert!(build_plan(
            &candidate(),
            &r,
            "2026.07.1",
            &StageOptions::default(),
            &qjson::uuid::UuidGen::from_seed(1),
        )
        .is_err());
    }

    #[test]
    fn an_empty_candidate_is_rejected() {
        let mut c = candidate();
        c.parts.clear();
        assert!(build_plan(
            &c,
            &record(),
            "2026.07.1",
            &StageOptions::default(),
            &qjson::uuid::UuidGen::from_seed(1),
        )
        .is_err());
    }

    #[test]
    fn a_region_is_only_created_when_requested() {
        let opts = StageOptions {
            create_region: true,
            ..StageOptions::default()
        };
        let built = build_plan(
            &candidate(),
            &record(),
            "2026.07.1",
            &opts,
            &qjson::uuid::UuidGen::from_seed(3),
        )
        .unwrap();
        assert!(built
            .plan
            .operations
            .iter()
            .any(|o| o.op_id() == "create_region"));
        assert!(!build()
            .plan
            .operations
            .iter()
            .any(|o| o.op_id() == "create_region"));
    }

    #[test]
    fn a_send_is_only_created_when_requested() {
        let opts = StageOptions {
            route_to_source_track: true,
            ..StageOptions::default()
        };
        let built = build_plan(
            &candidate(),
            &record(),
            "2026.07.1",
            &opts,
            &qjson::uuid::UuidGen::from_seed(3),
        )
        .unwrap();
        assert!(built
            .plan
            .operations
            .iter()
            .any(|o| o.op_id() == "create_midi_send"));
    }

    #[test]
    fn the_payload_echoes_the_scope() {
        let built = build();
        let payload = stage_payload(&built, true);
        assert_eq!(payload.str_field("note_scope").unwrap(), "selected_or_all");
        assert_eq!(payload.str_field("source_mode").unwrap(), "auto");
        assert_eq!(
            payload
                .get("melody_extraction")
                .unwrap()
                .str_field("mode")
                .unwrap(),
            "auto"
        );
        assert_eq!(payload.get("verify_snapshot"), Some(&Json::Bool(true)));
    }

    #[test]
    fn only_the_role_tag_is_supplied_by_the_plan() {
        let built = build();
        for op in &built.plan.operations {
            let tags = match op {
                EditOperation::CreateFolderTrack { tags, .. }
                | EditOperation::CreateTrack { tags, .. }
                | EditOperation::CreateMidiItem { tags, .. } => tags.clone(),
                _ => Vec::new(),
            };
            for (k, _) in tags {
                assert_eq!(k, ROLE_TAG, "a plan may only supply QLABS_ROLE");
                assert!(reaper_ipc::plan::is_allowed_tag_key(&k));
            }
        }
    }

    #[test]
    fn names_are_cleaned_and_bounded() {
        assert_eq!(clean_name("  Nice Name \n", "fallback"), "Nice Name");
        assert_eq!(clean_name("\u{7}\u{1}", "fallback"), "fallback");
        assert!(clean_name(&"x".repeat(500), "f").len() <= 180);
    }

    #[test]
    fn plan_building_is_deterministic_for_a_fixed_id_generator() {
        let a = build_plan(
            &candidate(),
            &record(),
            "2026.07.1",
            &StageOptions::default(),
            &qjson::uuid::UuidGen::from_seed(11),
        )
        .unwrap();
        let b = build_plan(
            &candidate(),
            &record(),
            "2026.07.1",
            &StageOptions::default(),
            &qjson::uuid::UuidGen::from_seed(11),
        )
        .unwrap();
        assert_eq!(a.plan.hash_hex(), b.plan.hash_hex());
    }

    #[test]
    fn expected_outputs_describe_every_created_object() {
        let built = build();
        let kinds: Vec<&str> = built
            .plan
            .expected_outputs
            .iter()
            .map(|e| e.kind.as_str())
            .collect();
        assert!(kinds.contains(&"folder_track"));
        assert!(kinds.contains(&"track"));
        assert!(kinds.contains(&"midi_item"));
        let item = built
            .plan
            .expected_outputs
            .iter()
            .find(|e| e.kind == "midi_item")
            .unwrap();
        assert_eq!(item.note_count, Some(4));
    }

    /// Every object-creating operation must declare a `temp_id`.
    ///
    /// The bridge records a created object only under a `temp_id`, and reports
    /// only what it recorded. An operation without one still performs the edit
    /// in REAPER but vanishes from the staging result, so a client cannot see
    /// that the object exists. `create_region` and `create_midi_send` both used
    /// to omit it, which is why `regions[]` and `sends[]` were always empty.
    #[test]
    fn region_and_send_operations_declare_a_temp_id() {
        let options = StageOptions {
            create_region: true,
            route_to_source_track: true,
            ..StageOptions::default()
        };
        let built = build_plan(
            &candidate(),
            &record(),
            "2026.07.1",
            &options,
            &qjson::uuid::UuidGen::from_seed(7),
        )
        .expect("plan builds");

        let mut saw_region = false;
        let mut saw_send = false;
        for op in &built.plan.operations {
            match op {
                EditOperation::CreateRegion { temp_id, .. } => {
                    assert!(!temp_id.is_empty(), "create_region has no temp_id");
                    saw_region = true;
                }
                EditOperation::CreateMidiSend { temp_id, .. } => {
                    assert!(!temp_id.is_empty(), "create_midi_send has no temp_id");
                    saw_send = true;
                }
                _ => {}
            }
        }
        assert!(saw_region, "no create_region operation was emitted");
        assert!(saw_send, "no create_midi_send operation was emitted");

        // And the plan must say it expects them back, so a bridge that drops
        // one is caught rather than silently believed.
        let kinds: Vec<&str> = built
            .plan
            .expected_outputs
            .iter()
            .map(|e| e.kind.as_str())
            .collect();
        assert!(
            kinds.contains(&"region"),
            "region is not an expected output"
        );
        assert!(kinds.contains(&"send"), "send is not an expected output");
    }
}
