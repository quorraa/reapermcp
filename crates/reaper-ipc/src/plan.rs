//! The `EditPlan` wire form.
//!
//! [`music_domain::plan::EditPlan`] is the in-process representation; this
//! module is its representation *on the wire*. The two are not identical, and
//! the differences are deliberate rather than accidental — see below. Every
//! plan leaving this crate goes through [`plan_to_wire`], and every plan read
//! back in goes through [`plan_from_wire`].
//!
//! # Two documented divergences from `music-domain`
//!
//! **Quarter-note positions.** `music_domain::time::BeatTime::to_json` emits an
//! exact rational *string* (`"8"`, `"1/2"`, `"7/3"`), which is the right choice
//! in-process: loop boundaries must compare exactly. The wire, however,
//! specifies `start_qn` / `end_qn` as finite JSON **numbers** in
//! `[-100000, 1000000]`, and the bridge rejects a string outright
//! (`transactions.lua`'s `check_qn` tests `type(v) == "number"`). So the wire
//! form converts with `BeatTime::as_f64`, and reading converts back — which
//! `BeatTime::from_json` already supports, since it accepts numbers.
//!
//! **The precondition discriminator.** `music_domain::plan::Precondition`
//! serialises its discriminator under the key `kind`. The wire specifies
//! `type`, and the bridge reads `p.type`. [`plan_to_wire`] emits `type`;
//! [`plan_from_wire`] accepts either and normalises to what the domain parser
//! wants.
//!
//! **Ownership tags.** `music-domain` reads and writes `tags` as a JSON object
//! (`{"QLABS_ROLE": "harmonic_bed"}`). The wire specifies an array of
//! `["KEY", "VALUE"]` pairs, and the bridge also accepts
//! `{"key": …, "value": …}` objects — but not a bare map.
//! [`plan_to_wire`] emits pairs; [`plan_from_wire`] accepts all three shapes and
//! normalises to the map the domain parser wants.
//!
//! All three translations live here, at the boundary, rather than in
//! `music-domain`: the domain crate should not have to know what a REAPER
//! bridge expects.

use crate::error::{codes, IpcError};
use crate::limits;
use music_domain::plan::{EditOperation, EditPlan, ExpectedOutput, PlannedNote, Precondition};
use qjson::{Json, JsonMap};

/// The closed ownership-tag allowlist a plan may supply (§10).
///
/// Only `QLABS_ROLE` is genuinely useful to send — the rest are written by the
/// bridge itself — but the bridge accepts any of these and rejects everything
/// else with `INVALID_EDIT_PLAN`. That closure is what stops the tag channel
/// from becoming a general write primitive into REAPER object state.
pub const TAG_KEYS: &[&str] = &[
    "QLABS_CANDIDATE_ID",
    "QLABS_COMMITTED_AT",
    "QLABS_CREATED_AT",
    "QLABS_KNOWLEDGE_VERSION",
    "QLABS_OBJECT_KIND",
    "QLABS_OWNER",
    "QLABS_PLAN_ID",
    "QLABS_PREVIEW_MUTED",
    "QLABS_ROLE",
    "QLABS_SOURCE_SNAPSHOT",
    "QLABS_STATUS",
    "QLABS_TEMP_ID",
    "QLABS_TRANSACTION_ID",
    "QLABS_UNDO_LABEL",
];

/// True when `key` is on the ownership-tag allowlist.
pub fn is_allowed_tag_key(key: &str) -> bool {
    TAG_KEYS.contains(&key)
}

/// The kinds an [`ExpectedOutput`] may name.
pub const EXPECTED_OUTPUT_KINDS: &[&str] =
    &["folder_track", "midi_item", "region", "send", "track"];

fn qn(v: music_domain::time::BeatTime) -> Json {
    Json::Float(v.as_f64())
}

fn tags_json(tags: &[(String, String)]) -> Json {
    Json::Arr(
        tags.iter()
            .map(|(k, v)| Json::Arr(vec![Json::Str(k.clone()), Json::Str(v.clone())]))
            .collect(),
    )
}

fn note_json(n: &PlannedNote) -> Json {
    qjson::json_obj! {
        "start_qn" => qn(n.start_qn),
        "end_qn" => qn(n.end_qn),
        "pitch" => n.pitch as i64,
        "velocity" => n.velocity as i64,
        "channel" => n.channel as i64,
        "muted" => n.muted,
        "spelling" => n.spelling.clone(),
    }
}

/// The wire form of one edit operation, discriminated on `op`.
pub fn operation_to_wire(op: &EditOperation) -> Json {
    match op {
        EditOperation::CreateFolderTrack {
            temp_id,
            name,
            tags,
        } => qjson::json_obj! {
            "op" => "create_folder_track",
            "temp_id" => temp_id.clone(),
            "name" => name.clone(),
            "tags" => tags_json(tags),
        },
        EditOperation::CreateTrack {
            temp_id,
            parent,
            name,
            tags,
        } => {
            let mut m = JsonMap::new();
            m.insert("op", Json::Str("create_track".into()));
            m.insert("temp_id", Json::Str(temp_id.clone()));
            if let Some(p) = parent {
                m.insert("parent", Json::Str(p.clone()));
            }
            m.insert("name", Json::Str(name.clone()));
            m.insert("tags", tags_json(tags));
            Json::Obj(m)
        }
        EditOperation::CreateMidiItem {
            temp_id,
            track,
            start_qn,
            end_qn,
            tags,
            muted,
        } => {
            qjson::json_obj! {
                "op" => "create_midi_item",
                "temp_id" => temp_id.clone(),
                "track" => track.clone(),
                "start_qn" => qn(*start_qn),
                "end_qn" => qn(*end_qn),
                "muted" => *muted,
                "tags" => tags_json(tags),
            }
        }
        EditOperation::InsertNotes { item, notes } => qjson::json_obj! {
            "op" => "insert_notes",
            "item" => item.clone(),
            "notes" => Json::Arr(notes.iter().map(note_json).collect()),
        },
        EditOperation::SetTrackMute { track, muted } => qjson::json_obj! {
            "op" => "set_track_mute",
            "track" => track.clone(),
            "muted" => *muted,
        },
        EditOperation::CreateRegion {
            temp_id,
            name,
            start_qn,
            end_qn,
        } => wire_with_temp_id(
            qjson::json_obj! {
                "op" => "create_region",
                "name" => name.clone(),
                "start_qn" => qn(*start_qn),
                "end_qn" => qn(*end_qn),
            },
            temp_id,
        ),
        EditOperation::CreateMidiSend {
            temp_id,
            from_track,
            to_track_guid,
        } => wire_with_temp_id(
            qjson::json_obj! {
                "op" => "create_midi_send",
                "from_track" => from_track.clone(),
                "to_track_guid" => to_track_guid.clone(),
            },
            temp_id,
        ),
    }
}

/// Adds `temp_id` to a wire operation, omitting the key when there is none.
///
/// The bridge only reports objects it recorded under a `temp_id`, and it
/// rejects an empty one, so the key is written only when it carries a value.
fn wire_with_temp_id(mut obj: Json, temp_id: &str) -> Json {
    if !temp_id.is_empty() {
        if let Some(m) = obj.as_obj_mut() {
            m.insert("temp_id".to_string(), Json::Str(temp_id.to_string()));
        }
    }
    obj
}

/// The wire form of one precondition, discriminated on `type`.
pub fn precondition_to_wire(p: &Precondition) -> Json {
    match p {
        Precondition::ProjectUuid(v) => {
            qjson::json_obj! { "type" => "project_uuid", "value" => v.clone() }
        }
        Precondition::StateChangeCount(v) => {
            qjson::json_obj! { "type" => "state_change_count", "value" => *v }
        }
        Precondition::ItemGuidExists(v) => {
            qjson::json_obj! { "type" => "item_guid_exists", "value" => v.clone() }
        }
        Precondition::TakeGuidExists(v) => {
            qjson::json_obj! { "type" => "take_guid_exists", "value" => v.clone() }
        }
        Precondition::MidiHash(v) => {
            qjson::json_obj! { "type" => "midi_hash", "value" => v.clone() }
        }
        Precondition::TempoMapHash(v) => {
            qjson::json_obj! { "type" => "tempo_map_hash", "value" => v.clone() }
        }
        Precondition::ItemBounds { start_qn, end_qn } => qjson::json_obj! {
            "type" => "item_bounds",
            "start_qn" => *start_qn,
            "end_qn" => *end_qn,
        },
    }
}

/// The wire form of one expected output.
pub fn expected_output_to_wire(e: &ExpectedOutput) -> Json {
    let mut m = JsonMap::new();
    m.insert("temp_id", Json::Str(e.temp_id.clone()));
    m.insert("kind", Json::Str(e.kind.clone()));
    if let Some(n) = e.note_count {
        m.insert("note_count", Json::Int(n as i64));
    }
    Json::Obj(m)
}

/// The complete wire form of an edit plan.
pub fn plan_to_wire(plan: &EditPlan) -> Json {
    qjson::json_obj! {
        "plan_id" => plan.plan_id.clone(),
        "candidate_id" => plan.candidate_id.clone(),
        "transaction_id" => plan.transaction_id.clone(),
        "base_snapshot_id" => plan.base_snapshot_id.clone(),
        "base_snapshot_hash" => plan.base_snapshot_hash.clone(),
        "project_uuid" => plan.project_uuid.clone(),
        "knowledge_version" => plan.knowledge_version.clone(),
        "undo_label" => plan.undo_label.clone(),
        "operations" => Json::Arr(plan.operations.iter().map(operation_to_wire).collect()),
        "preconditions" => Json::Arr(plan.preconditions.iter().map(precondition_to_wire).collect()),
        "expected_outputs" => Json::Arr(plan.expected_outputs.iter().map(expected_output_to_wire).collect()),
    }
}

/// Reads a wire plan back into the domain type.
///
/// Accepts either wire spelling of the precondition discriminator (`type`, what
/// the bridge and the recorded fixtures use, or `kind`, what `music-domain`
/// writes) and either representation of a quarter-note position (a number, as
/// the wire specifies, or a rational string, as `music-domain` writes).
pub fn plan_from_wire(value: &Json) -> Result<EditPlan, IpcError> {
    let mut doc = value.clone();
    if let Json::Obj(m) = &mut doc {
        if let Some(Json::Arr(pres)) = m.get_mut("preconditions") {
            for p in pres.iter_mut() {
                normalize_precondition(p);
            }
        }
        if let Some(Json::Arr(ops)) = m.get_mut("operations") {
            for op in ops.iter_mut() {
                normalize_tags(op)?;
            }
        }
    }
    EditPlan::from_json(&doc).map_err(|e| {
        IpcError::with_details(
            codes::INVALID_EDIT_PLAN,
            format!("edit plan could not be read: {e}"),
            qjson::json_obj! { "domain_code" => e.code.clone() },
        )
    })
}

/// Renames a precondition's `type` discriminator to the `kind` the domain
/// parser expects, preserving everything else.
fn normalize_precondition(p: &mut Json) {
    let Json::Obj(m) = p else { return };
    if m.contains_key("kind") {
        return;
    }
    let Some(Json::Str(t)) = m.get("type").cloned() else {
        return;
    };
    m.insert("kind", Json::Str(t));
}

/// Rewrites an operation's wire `tags` array into the object the domain parser
/// expects, accepting `["KEY","VALUE"]` pairs and `{"key":…,"value":…}` objects.
///
/// A `tags` value that is already an object is left alone, so a plan produced by
/// `music-domain` itself round-trips unchanged.
fn normalize_tags(op: &mut Json) -> Result<(), IpcError> {
    let Json::Obj(m) = op else { return Ok(()) };
    let entries = match m.get("tags") {
        Some(Json::Arr(a)) => a.clone(),
        // Already an object, absent, or null: nothing to do.
        _ => return Ok(()),
    };
    let bad = |msg: &str| {
        IpcError::new(
            codes::INVALID_EDIT_PLAN,
            format!("tags entry is malformed: {msg}"),
        )
    };
    let mut out = JsonMap::new();
    for e in &entries {
        let (k, v) = match e {
            Json::Arr(pair) if pair.len() == 2 => (
                pair[0].as_str().ok_or_else(|| bad("key is not a string"))?,
                pair[1]
                    .as_str()
                    .ok_or_else(|| bad("value is not a string"))?,
            ),
            Json::Obj(o) => (
                o.get("key")
                    .and_then(Json::as_str)
                    .ok_or_else(|| bad("missing string `key`"))?,
                o.get("value")
                    .and_then(Json::as_str)
                    .ok_or_else(|| bad("missing string `value`"))?,
            ),
            _ => return Err(bad("expected a [key, value] pair or a {key, value} object")),
        };
        out.insert(k, Json::Str(v.to_string()));
    }
    m.insert("tags", Json::Obj(out));
    Ok(())
}

/// Checks the plan's ownership tags against the closed allowlist.
///
/// Returns the offending key on the first violation.
pub fn check_tag_keys(plan: &EditPlan) -> Result<(), IpcError> {
    for op in &plan.operations {
        let tags = match op {
            EditOperation::CreateFolderTrack { tags, .. }
            | EditOperation::CreateTrack { tags, .. }
            | EditOperation::CreateMidiItem { tags, .. } => tags,
            _ => continue,
        };
        if tags.len() > limits::MAX_TAGS_PER_OBJECT {
            return Err(IpcError::with_details(
                codes::INVALID_EDIT_PLAN,
                format!(
                    "an object may carry at most {} tags",
                    limits::MAX_TAGS_PER_OBJECT
                ),
                qjson::json_obj! { "count" => tags.len() },
            ));
        }
        for (k, v) in tags {
            if !is_allowed_tag_key(k) {
                return Err(IpcError::with_details(
                    codes::INVALID_EDIT_PLAN,
                    format!("tag key {k:?} is not on the ownership-tag allowlist"),
                    qjson::json_obj! { "key" => k.clone() },
                ));
            }
            if v.len() > limits::MAX_STRING_LEN {
                return Err(IpcError::with_details(
                    codes::INVALID_EDIT_PLAN,
                    format!(
                        "tag {k:?} value is longer than {} bytes",
                        limits::MAX_STRING_LEN
                    ),
                    qjson::json_obj! { "key" => k.clone() },
                ));
            }
            if v.chars().any(char::is_control) {
                return Err(IpcError::with_details(
                    codes::INVALID_EDIT_PLAN,
                    format!("tag {k:?} value contains a control character"),
                    qjson::json_obj! { "key" => k.clone() },
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use music_domain::time::BeatTime;

    fn plan_fixture() -> Json {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mock-reaper/plans/valid-two-part-candidate.plan.json"
        );
        let text = std::fs::read_to_string(path).expect("plan fixture");
        Json::parse(&text).expect("parses")
    }

    #[test]
    fn the_recorded_plan_reads_through_the_wire_translator() {
        let plan = plan_from_wire(&plan_fixture()).expect("read");
        assert_eq!(plan.operations.len(), 9);
        assert_eq!(plan.preconditions.len(), 6);
        assert_eq!(plan.expected_outputs.len(), 5);
        assert_eq!(plan.undo_label, "QLabs MCP: Stage candidate 00000000");
    }

    #[test]
    fn the_domain_parser_alone_cannot_read_the_recorded_plan() {
        // This is the divergence the module documents: `music-domain` expects
        // `kind`, the wire uses `type`. If this ever starts succeeding, the two
        // sides have converged and the translator can be simplified.
        assert!(
            EditPlan::from_json(&plan_fixture()).is_err(),
            "music-domain now reads the wire form directly; revisit plan_from_wire"
        );
    }

    #[test]
    fn round_tripping_the_recorded_plan_preserves_it() {
        let plan = plan_from_wire(&plan_fixture()).expect("read");
        let wire = plan_to_wire(&plan);
        let again = plan_from_wire(&wire).expect("read back");
        assert_eq!(again.plan_id, plan.plan_id);
        assert_eq!(again.transaction_id, plan.transaction_id);
        assert_eq!(again.operations.len(), plan.operations.len());
        assert_eq!(again.preconditions.len(), plan.preconditions.len());
        assert_eq!(again.expected_outputs.len(), plan.expected_outputs.len());
    }

    #[test]
    fn quarter_note_positions_go_on_the_wire_as_numbers() {
        let plan = plan_from_wire(&plan_fixture()).expect("read");
        let wire = plan_to_wire(&plan);
        let ops = wire.get("operations").and_then(Json::as_arr).expect("ops");
        let item = ops
            .iter()
            .find(|o| o.get("op").and_then(Json::as_str) == Some("create_midi_item"))
            .expect("a midi item op");
        assert!(
            matches!(
                item.get("start_qn"),
                Some(Json::Float(_)) | Some(Json::Int(_))
            ),
            "start_qn must be a number, found {:?}",
            item.get("start_qn")
        );
        assert_eq!(item.get("start_qn").and_then(Json::as_f64), Some(0.0));
        assert_eq!(item.get("end_qn").and_then(Json::as_f64), Some(8.0));
    }

    #[test]
    fn the_domain_form_would_have_sent_strings() {
        // Demonstrates why the translation exists: `to_json` emits rationals.
        let plan = plan_from_wire(&plan_fixture()).expect("read");
        let domain = plan.to_json();
        let ops = domain
            .get("operations")
            .and_then(Json::as_arr)
            .expect("ops");
        let item = ops
            .iter()
            .find(|o| o.get("op").and_then(Json::as_str) == Some("create_midi_item"))
            .expect("a midi item op");
        assert!(
            matches!(item.get("start_qn"), Some(Json::Str(_))),
            "the domain form is a rational string; the bridge would reject it"
        );
    }

    #[test]
    fn note_positions_are_numbers_too() {
        let plan = plan_from_wire(&plan_fixture()).expect("read");
        let wire = plan_to_wire(&plan);
        let ops = wire.get("operations").and_then(Json::as_arr).expect("ops");
        let insert = ops
            .iter()
            .find(|o| o.get("op").and_then(Json::as_str) == Some("insert_notes"))
            .expect("an insert_notes op");
        let notes = insert.get("notes").and_then(Json::as_arr).expect("notes");
        for n in notes {
            assert!(n.get("start_qn").and_then(Json::as_f64).is_some());
            assert!(n.get("end_qn").and_then(Json::as_f64).is_some());
            assert!(matches!(n.get("pitch"), Some(Json::Int(_))));
        }
    }

    #[test]
    fn preconditions_use_the_wire_discriminator() {
        let plan = plan_from_wire(&plan_fixture()).expect("read");
        let wire = plan_to_wire(&plan);
        let pres = wire
            .get("preconditions")
            .and_then(Json::as_arr)
            .expect("preconditions");
        for p in pres {
            assert!(
                p.get("type").and_then(Json::as_str).is_some(),
                "every precondition needs a `type`, got {p:?}"
            );
            assert!(p.get("kind").is_none(), "`kind` must not go on the wire");
        }
        assert_eq!(
            pres[0].get("type").and_then(Json::as_str),
            Some("project_uuid")
        );
    }

    #[test]
    fn the_wire_precondition_set_matches_the_recorded_one() {
        let recorded = plan_fixture();
        let recorded_types: Vec<&str> = recorded
            .get("preconditions")
            .and_then(Json::as_arr)
            .expect("preconditions")
            .iter()
            .filter_map(|p| p.get("type").and_then(Json::as_str))
            .collect();
        let plan = plan_from_wire(&recorded).expect("read");
        let wire = plan_to_wire(&plan);
        let ours: Vec<&str> = wire
            .get("preconditions")
            .and_then(Json::as_arr)
            .expect("preconditions")
            .iter()
            .filter_map(|p| p.get("type").and_then(Json::as_str))
            .collect();
        assert_eq!(ours, recorded_types);
    }

    #[test]
    fn item_bounds_preconditions_carry_their_own_numbers() {
        let plan = plan_from_wire(&plan_fixture()).expect("read");
        let wire = plan_to_wire(&plan);
        let pres = wire
            .get("preconditions")
            .and_then(Json::as_arr)
            .expect("pres");
        let bounds = pres
            .iter()
            .find(|p| p.get("type").and_then(Json::as_str) == Some("item_bounds"))
            .expect("item_bounds");
        assert_eq!(bounds.get("start_qn").and_then(Json::as_f64), Some(0.0));
        assert_eq!(bounds.get("end_qn").and_then(Json::as_f64), Some(8.0));
        assert!(bounds.get("value").is_none());
    }

    #[test]
    fn expected_outputs_keep_the_kind_discriminator() {
        let plan = plan_from_wire(&plan_fixture()).expect("read");
        let wire = plan_to_wire(&plan);
        let outs = wire
            .get("expected_outputs")
            .and_then(Json::as_arr)
            .expect("expected_outputs");
        for o in outs {
            let kind = o.get("kind").and_then(Json::as_str).expect("kind");
            assert!(EXPECTED_OUTPUT_KINDS.contains(&kind), "{kind}");
            assert!(o.get("temp_id").and_then(Json::as_str).is_some());
        }
    }

    #[test]
    fn tags_serialise_as_key_value_pairs() {
        let plan = plan_from_wire(&plan_fixture()).expect("read");
        let wire = plan_to_wire(&plan);
        let ops = wire.get("operations").and_then(Json::as_arr).expect("ops");
        let folder = ops
            .iter()
            .find(|o| o.get("op").and_then(Json::as_str) == Some("create_folder_track"))
            .expect("folder op");
        let tags = folder.get("tags").and_then(Json::as_arr).expect("tags");
        let pair = tags[0].as_arr().expect("a pair");
        assert_eq!(pair.len(), 2);
        assert_eq!(pair[0].as_str(), Some("QLABS_ROLE"));
    }

    #[test]
    fn the_domain_form_would_have_sent_a_tag_map() {
        // The second reason the translator exists: `to_json` writes an object.
        let plan = plan_from_wire(&plan_fixture()).expect("read");
        let domain = plan.to_json();
        let ops = domain
            .get("operations")
            .and_then(Json::as_arr)
            .expect("ops");
        let folder = ops
            .iter()
            .find(|o| o.get("op").and_then(Json::as_str) == Some("create_folder_track"))
            .expect("folder op");
        assert!(
            matches!(folder.get("tags"), Some(Json::Obj(_))),
            "the domain form is a map; the wire wants pairs"
        );
    }

    #[test]
    fn tags_can_also_arrive_as_key_value_objects() {
        let mut op = qjson::json_obj! {
            "op" => "create_track",
            "temp_id" => "t0",
            "tags" => qjson::json_arr![qjson::json_obj!{ "key" => "QLABS_ROLE", "value" => "bass" }],
        };
        normalize_tags(&mut op).expect("normalised");
        assert_eq!(
            op.get("tags")
                .and_then(|t| t.get("QLABS_ROLE"))
                .and_then(Json::as_str),
            Some("bass")
        );
    }

    #[test]
    fn a_malformed_tag_entry_is_refused() {
        let mut op = qjson::json_obj! {
            "op" => "create_track",
            "tags" => qjson::json_arr![qjson::json_arr!["only-one"]],
        };
        assert_eq!(
            normalize_tags(&mut op).expect_err("malformed").code,
            codes::INVALID_EDIT_PLAN
        );
        let mut op = qjson::json_obj! { "op" => "create_track", "tags" => qjson::json_arr![1] };
        assert!(normalize_tags(&mut op).is_err());
    }

    #[test]
    fn an_operation_whose_tags_are_already_a_map_is_left_alone() {
        let mut op = qjson::json_obj! {
            "op" => "create_track",
            "tags" => qjson::json_obj!{ "QLABS_ROLE" => "pad" },
        };
        normalize_tags(&mut op).expect("no-op");
        assert_eq!(
            op.get("tags")
                .and_then(|t| t.get("QLABS_ROLE"))
                .and_then(Json::as_str),
            Some("pad")
        );
    }

    #[test]
    fn the_recorded_plan_uses_only_allowlisted_tag_keys() {
        let plan = plan_from_wire(&plan_fixture()).expect("read");
        check_tag_keys(&plan).expect("allowlisted");
    }

    #[test]
    fn a_tag_key_outside_the_allowlist_is_refused() {
        let mut plan = plan_from_wire(&plan_fixture()).expect("read");
        plan.operations.insert(
            0,
            EditOperation::CreateTrack {
                temp_id: "t9".into(),
                parent: None,
                name: "x".into(),
                tags: vec![("P_NAME".into(), "gotcha".into())],
            },
        );
        let err = check_tag_keys(&plan).expect_err("refused");
        assert_eq!(err.code, codes::INVALID_EDIT_PLAN);
        assert_eq!(err.detail_str("key"), Some("P_NAME"));
    }

    #[test]
    fn a_control_character_in_a_tag_value_is_refused() {
        let mut plan = plan_from_wire(&plan_fixture()).expect("read");
        plan.operations.insert(
            0,
            EditOperation::CreateTrack {
                temp_id: "t9".into(),
                parent: None,
                name: "x".into(),
                tags: vec![("QLABS_ROLE".into(), "a\nb".into())],
            },
        );
        assert_eq!(
            check_tag_keys(&plan).expect_err("refused").code,
            codes::INVALID_EDIT_PLAN
        );
    }

    #[test]
    fn too_many_tags_on_one_object_is_refused() {
        let mut plan = plan_from_wire(&plan_fixture()).expect("read");
        plan.operations.insert(
            0,
            EditOperation::CreateTrack {
                temp_id: "t9".into(),
                parent: None,
                name: "x".into(),
                tags: (0..=limits::MAX_TAGS_PER_OBJECT)
                    .map(|i| ("QLABS_ROLE".to_string(), format!("v{i}")))
                    .collect(),
            },
        );
        assert_eq!(
            check_tag_keys(&plan).expect_err("refused").code,
            codes::INVALID_EDIT_PLAN
        );
    }

    #[test]
    fn the_tag_allowlist_matches_the_recorded_bridge_manifest() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mock-reaper/bridge/error-codes.json"
        );
        let doc = Json::parse(&std::fs::read_to_string(path).expect("fixture")).expect("parses");
        let recorded: Vec<&str> = doc
            .arr_field("tag_keys")
            .expect("tag_keys")
            .iter()
            .filter_map(Json::as_str)
            .collect();
        assert_eq!(recorded, TAG_KEYS);
    }

    #[test]
    fn the_operation_allowlist_matches_the_recorded_bridge_manifest() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mock-reaper/bridge/error-codes.json"
        );
        let doc = Json::parse(&std::fs::read_to_string(path).expect("fixture")).expect("parses");
        let recorded: Vec<&str> = doc
            .arr_field("operations")
            .expect("operations")
            .iter()
            .filter_map(Json::as_str)
            .collect();
        let mut ours: Vec<String> = [
            EditOperation::CreateFolderTrack {
                temp_id: "a".into(),
                name: String::new(),
                tags: vec![],
            },
            EditOperation::CreateTrack {
                temp_id: "a".into(),
                parent: None,
                name: String::new(),
                tags: vec![],
            },
            EditOperation::CreateMidiItem {
                temp_id: "a".into(),
                track: "t".into(),
                start_qn: BeatTime::ZERO,
                end_qn: BeatTime::from_quarters(1),
                tags: vec![],
                muted: false,
            },
            EditOperation::InsertNotes {
                item: "i".into(),
                notes: vec![],
            },
            EditOperation::SetTrackMute {
                track: "t".into(),
                muted: false,
            },
            EditOperation::CreateRegion {
                temp_id: "r0".into(),
                name: String::new(),
                start_qn: BeatTime::ZERO,
                end_qn: BeatTime::from_quarters(1),
            },
            EditOperation::CreateMidiSend {
                temp_id: "s0".into(),
                from_track: "t".into(),
                to_track_guid: "g".into(),
            },
        ]
        .iter()
        .map(|op| {
            operation_to_wire(op)
                .get("op")
                .and_then(Json::as_str)
                .expect("op")
                .to_string()
        })
        .collect();
        ours.sort();
        assert_eq!(ours, recorded);
    }

    #[test]
    fn a_plan_that_is_not_an_object_is_refused() {
        let err = plan_from_wire(&Json::Arr(vec![])).expect_err("not a plan");
        assert_eq!(err.code, codes::INVALID_EDIT_PLAN);
    }

    #[test]
    fn a_precondition_that_already_uses_kind_is_left_alone() {
        let mut p = qjson::json_obj! { "kind" => "project_uuid", "value" => "x" };
        normalize_precondition(&mut p);
        assert_eq!(p.get("kind").and_then(Json::as_str), Some("project_uuid"));
        assert!(p.get("type").is_none());
    }
}
