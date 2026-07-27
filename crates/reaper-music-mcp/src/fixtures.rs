//! Running the real engine with no REAPER present.
//!
//! A fixture is a JSON file describing notes, chords and a tempo map. This
//! module turns one into exactly the same [`SnapshotRecord`] the bridge would
//! have produced — same hashes, same identifiers, same note ordering — so the
//! analysis, generation and staging paths run unchanged. That is what makes
//! `analyze-fixture` and `generate-fixture` genuine end-to-end exercises of the
//! engine rather than a separate code path that only looks similar.
//!
//! The synthetic snapshot's hashes are computed with [`reaper_ipc::hash`], the
//! same canonicalisation the Lua bridge uses, which means a plan built from a
//! fixture carries preconditions that would be checkable against a real
//! project.

use crate::error::{codes, ToolError};
use crate::server::{CallContext, ServerCore};
use crate::store::{ScopeEcho, SnapshotRecord};
use music_domain::fixture::Fixture;
use music_domain::prelude::*;
use qjson::{json_obj, Json};
use reaper_ipc::{hash, Snapshot};
use std::path::{Path, PathBuf};

/// The namespace synthetic fixture identifiers are derived in.
pub const FIXTURE_NAMESPACE: &str = "qlabs.mcp.fixture";

/// Loads a fixture from disk.
pub fn load(path: &Path) -> Result<Fixture, ToolError> {
    Fixture::from_path(path).map_err(|e| {
        ToolError::with_details(
            codes::INVALID_ARGUMENT,
            format!("{}: {}", path.display(), e.message),
            json_obj! { "path" => path.display().to_string(), "code" => e.code.clone() },
        )
    })
}

/// Builds the snapshot record a fixture stands in for.
///
/// Every identifier is derived from the fixture id with
/// [`qjson::uuid::uuid_from_name`], so the same fixture always produces the
/// same snapshot id — which is what makes fixture output byte-reproducible and
/// therefore usable as a golden test.
pub fn snapshot_record(fixture: &Fixture) -> Result<SnapshotRecord, ToolError> {
    let notes = fixture.note_set();
    let time_map = fixture.time_map();

    let (span_start, span_end) = if notes.notes.is_empty() {
        (BeatTime::ZERO, BeatTime::from_quarters(4))
    } else {
        notes.span()
    };
    let item_start = span_start.as_f64().min(0.0);
    let item_end = span_end.as_f64().max(item_start + 1.0);

    let take: Vec<hash::TakeNote> = notes
        .notes
        .iter()
        .map(|n| hash::TakeNote {
            start_ppq: n.onset.as_f64() * PPQ as f64,
            end_ppq: n.end().as_f64() * PPQ as f64,
            channel: i64::from(n.channel),
            pitch: i64::from(n.midi as i16),
            velocity: i64::from(n.velocity),
            muted: n.muted,
            selected: n.selected,
        })
        .collect();
    let list: Vec<hash::ListNote> = notes
        .notes
        .iter()
        .map(|n| hash::ListNote {
            start_qn: n.onset.as_f64(),
            end_qn: n.end().as_f64(),
            pitch: i64::from(n.midi as i16),
            velocity: i64::from(n.velocity),
            channel: i64::from(n.channel),
            muted: n.muted,
            selected: n.selected,
        })
        .collect();
    let markers: Vec<hash::TempoMarker> = time_map
        .tempos
        .iter()
        .map(|t| hash::TempoMarker {
            time_seconds: time_map.qn_to_seconds(t.qn),
            qn: t.qn.as_f64(),
            bpm: t.bpm,
            timesig_num: i64::from(time_map.meter_at(t.qn).numerator),
            timesig_den: i64::from(time_map.meter_at(t.qn).denominator),
            linear: t.linear,
        })
        .collect();

    let ipc = |e: reaper_ipc::IpcError| ToolError::from(e);
    let midi_hash = hash::hash_canonical(&hash::midi_canonical(&take).map_err(ipc)?);
    let selection_hash = hash::hash_canonical(&hash::selection_canonical(&take).map_err(ipc)?);
    let note_list_hash = hash::hash_canonical(&hash::note_list_canonical(&list).map_err(ipc)?);
    let tempo_hash = hash::hash_canonical(&hash::tempo_canonical(&markers).map_err(ipc)?);
    let timesig_hash = hash::hash_canonical(&hash::timesig_canonical(&markers).map_err(ipc)?);

    let project_uuid = qjson::uuid::uuid_from_name(FIXTURE_NAMESPACE, &format!("{}#p", fixture.id));
    let track_guid = qjson::uuid::uuid_from_name(FIXTURE_NAMESPACE, &format!("{}#tr", fixture.id));
    let item_guid = qjson::uuid::uuid_from_name(FIXTURE_NAMESPACE, &format!("{}#it", fixture.id));
    let take_guid = qjson::uuid::uuid_from_name(FIXTURE_NAMESPACE, &format!("{}#tk", fixture.id));
    let snapshot_id = qjson::uuid::uuid_from_name(FIXTURE_NAMESPACE, &format!("{}#sn", fixture.id));

    let fields = hash::SnapshotFields {
        project_uuid: Some(project_uuid.clone()),
        track_guid: Some(track_guid.clone()),
        item_guid: Some(item_guid.clone()),
        take_guid: Some(take_guid.clone()),
        item_position_seconds: 0.0,
        item_length_seconds: time_map.qn_to_seconds(BeatTime::from_f64(item_end - item_start)),
        item_position_qn: item_start,
        item_length_qn: item_end - item_start,
        is_loop_source: fixture.loop_region.is_some(),
        note_scope: Some("all".to_string()),
        extraction_mode: Some("auto".to_string()),
        extraction_channel: None,
        midi_hash: Some(midi_hash.clone()),
        tempo_map_hash: Some(tempo_hash.clone()),
        timesig_map_hash: Some(timesig_hash.clone()),
        note_selection_hash: Some(selection_hash.clone()),
        note_list_hash: Some(note_list_hash.clone()),
        note_count: notes.notes.len() as i64,
    };
    let snapshot_hash = hash::hash_canonical(&hash::snapshot_canonical(&fields).map_err(ipc)?);

    let note_json: Vec<Json> = notes
        .notes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            json_obj! {
                "index" => i as i64,
                "source_index" => i as i64,
                "start_ppq" => n.onset.as_f64() * PPQ as f64,
                "end_ppq" => n.end().as_f64() * PPQ as f64,
                "start_qn" => n.onset.as_f64(),
                "end_qn" => n.end().as_f64(),
                "duration_qn" => n.duration.as_f64(),
                "item_relative_start_qn" => n.onset.as_f64() - item_start,
                "item_relative_end_qn" => n.end().as_f64() - item_start,
                "start_seconds" => time_map.qn_to_seconds(n.onset),
                "end_seconds" => time_map.qn_to_seconds(n.end()),
                "pitch" => i64::from(n.midi as i16),
                "velocity" => i64::from(n.velocity),
                "channel" => i64::from(n.channel),
                "muted" => n.muted,
                "selected" => n.selected,
            }
        })
        .collect();

    let marker_json: Vec<Json> = markers
        .iter()
        .enumerate()
        .map(|(i, m)| {
            json_obj! {
                "index" => i as i64,
                "time_seconds" => m.time_seconds,
                "qn" => m.qn,
                "bpm" => m.bpm,
                "timesig_num" => m.timesig_num,
                "timesig_den" => m.timesig_den,
                "linear" => m.linear,
            }
        })
        .collect();

    let first_meter = time_map.meter_at(BeatTime::ZERO);
    let raw = json_obj! {
        "snapshot_id" => snapshot_id,
        "project_pointer" => "FIXTURE",
        "project_uuid" => project_uuid,
        "project_state_change_count" => 1,
        "track_guid" => track_guid,
        "item_guid" => item_guid,
        "take_guid" => take_guid,
        "item_position_seconds" => 0.0,
        "item_length_seconds" => fields.item_length_seconds,
        "item_position_qn" => item_start,
        "item_end_qn" => item_end,
        "item_length_qn" => item_end - item_start,
        "is_loop_source" => fields.is_loop_source,
        "note_scope" => "all",
        "extraction_mode" => "auto",
        "extraction_channel" => Json::Null,
        "resolved_by" => "selected_item",
        "midi_hash" => midi_hash,
        "note_selection_hash" => selection_hash,
        "tempo_map_hash" => tempo_hash,
        "timesig_map_hash" => timesig_hash,
        "note_list_hash" => note_list_hash,
        "snapshot_hash" => snapshot_hash,
        "note_count" => notes.notes.len() as i64,
        "source_note_count" => notes.notes.len() as i64,
        "notes" => Json::Arr(note_json),
        "tempo_markers" => Json::Arr(marker_json),
        "tempo_at_item_start" => time_map.tempo_at(BeatTime::ZERO),
        "time_signature_at_item_start" => json_obj! {
            "numerator" => i64::from(first_meter.numerator),
            "denominator" => i64::from(first_meter.denominator),
        },
        "timestamp" => 0,
        "timestamp_iso" => "1970-01-01T00:00:00Z",
        "bridge_version" => reaper_ipc::BRIDGE_VERSION,
        "reaper_version" => Json::Null,
        "selection_assumptions" => Json::Arr(vec![Json::Str(format!(
            "synthesised from fixture {}",
            fixture.id
        ))]),
        "warnings" => Json::Arr(Vec::new()),
    };

    let snapshot = Snapshot::from_json(&raw)?;
    snapshot.verify()?;
    let converted = crate::convert::note_set_of(&snapshot)?;

    Ok(SnapshotRecord {
        snapshot,
        scope: ScopeEcho {
            source_mode: "selected_item".to_string(),
            note_scope: "all".to_string(),
            extraction_mode: "auto".to_string(),
            extraction_channel: None,
        },
        notes: converted,
        raw,
    })
}

/// `analyze-fixture <file>`: the real analysis pipeline, no REAPER.
pub fn analyze_fixture(core: &ServerCore, path: &Path, profile: &str) -> Result<Json, ToolError> {
    let fixture = load(path)?;
    let record = snapshot_record(&fixture)?;
    let ctx = CallContext::detached();
    let snapshot_id = core.with_store(|s| s.put_snapshot(record.clone(), ctx.now));

    let mut args = json_obj! { "snapshot_id" => snapshot_id.clone() };
    if let Json::Obj(m) = &mut args {
        m.insert("style_profile", Json::Str(profile.to_string()));
        if let Some(l) = &fixture.loop_region {
            m.insert(
                "loop_span",
                json_obj! {
                    "start_qn" => l.start_qn.as_f64(),
                    "end_qn" => l.end_qn.as_f64(),
                },
            );
        }
    }
    let body = crate::tools::analysis::analyze_selection(core, &args, &ctx)?;
    Ok(json_obj! {
        "fixture" => fixture.id.clone(),
        "title" => fixture.title.clone(),
        "profile" => profile,
        "snapshot_id" => snapshot_id,
        "note_count" => record.notes.notes.len() as i64,
        "analysis" => body,
    })
}

/// `generate-fixture <file>`: the real generation pipeline, no REAPER.
pub fn generate_fixture(
    core: &ServerCore,
    path: &Path,
    profile: &str,
    count: i64,
    seed: u64,
) -> Result<Json, ToolError> {
    let fixture = load(path)?;
    let record = snapshot_record(&fixture)?;
    let ctx = CallContext::detached();
    let snapshot_id = core.with_store(|s| s.put_snapshot(record.clone(), ctx.now));

    let args = json_obj! {
        "snapshot_id" => snapshot_id.clone(),
        "style_profile" => profile,
        "candidate_count" => count,
        "seed" => seed as i64,
        "preserve_melody" => true,
        "preserve_rhythm" => true,
    };
    let body = crate::tools::harmony::generate_candidates(core, &args, &ctx)?;
    Ok(json_obj! {
        "fixture" => fixture.id.clone(),
        "title" => fixture.title.clone(),
        "profile" => profile,
        "seed" => seed as i64,
        "snapshot_id" => snapshot_id,
        "generation" => body,
    })
}

/// The repository's `fixtures/` directory, derived from this crate's manifest.
///
/// Test-only: a shipped binary is given an explicit path and never guesses one.
#[cfg(test)]
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(|r| r.join("fixtures"))
        .unwrap_or_else(|| PathBuf::from("fixtures"))
}

/// Loads a repository fixture by id, e.g. `melodies/eight_bar_c_major`.
#[cfg(test)]
pub fn snapshot_from_fixture(id: &str) -> Result<SnapshotRecord, ToolError> {
    let path = fixtures_dir().join(format!("{id}.json"));
    snapshot_record(&load(&path)?)
}

/// A `PathBuf` re-export so callers do not need the `std::path` import.
pub type FixturePath = PathBuf;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_repository_fixture_becomes_a_verified_snapshot() {
        let mut seen = 0;
        for dir in ["melodies", "progressions", "loops"] {
            let path = fixtures_dir().join(dir);
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().is_some_and(|e| e == "json") {
                    let fixture =
                        load(&p).unwrap_or_else(|e| panic!("{}: {}", p.display(), e.message));
                    let record = snapshot_record(&fixture).expect(&fixture.id);
                    record.snapshot.verify().expect(&fixture.id);
                    seen += 1;
                }
            }
        }
        assert!(seen >= 10, "expected the fixture corpus, saw {seen}");
    }

    #[test]
    fn the_synthetic_snapshot_carries_every_hash_staging_needs() {
        let record = snapshot_from_fixture("melodies/eight_bar_c_major").unwrap();
        let s = &record.snapshot;
        assert!(s.project_uuid.is_some());
        assert!(s.item_guid.is_some());
        assert!(s.take_guid.is_some());
        assert!(s.midi_hash.is_some());
        assert!(s.tempo_map_hash.is_some());
        assert!(s.snapshot_hash.is_some());
    }

    #[test]
    fn fixture_snapshots_are_reproducible() {
        let a = snapshot_from_fixture("melodies/eight_bar_c_major").unwrap();
        let b = snapshot_from_fixture("melodies/eight_bar_c_major").unwrap();
        assert_eq!(a.snapshot.snapshot_id, b.snapshot.snapshot_id);
        assert_eq!(a.snapshot.snapshot_hash, b.snapshot.snapshot_hash);
        assert_eq!(a.notes.hash_hex(), b.notes.hash_hex());
    }

    #[test]
    fn different_fixtures_hash_differently() {
        let a = snapshot_from_fixture("melodies/eight_bar_c_major").unwrap();
        let b = snapshot_from_fixture("melodies/dorian_vamp_d").unwrap();
        assert_ne!(a.snapshot.snapshot_hash, b.snapshot.snapshot_hash);
        assert_ne!(a.snapshot.project_uuid, b.snapshot.project_uuid);
    }

    #[test]
    fn analyze_fixture_runs_the_real_engine() {
        let core = ServerCore::offline();
        let path = fixtures_dir().join("melodies/eight_bar_c_major.json");
        let out = analyze_fixture(&core, &path, "common_practice").unwrap();
        let analysis = out.get("analysis").unwrap();
        assert!(
            analysis
                .get("key")
                .unwrap()
                .arr_field("candidates")
                .unwrap()
                .len()
                > 1
        );
        assert!(!analysis.arr_field("phrases").unwrap().is_empty());
    }

    #[test]
    fn generate_fixture_runs_the_real_engine() {
        let core = ServerCore::offline();
        let path = fixtures_dir().join("melodies/eight_bar_c_major.json");
        let out = generate_fixture(&core, &path, "jazz_standard", 3, 12_345).unwrap();
        let generation = out.get("generation").unwrap();
        assert_eq!(generation.i64_field("candidate_count"), Ok(3));
        for c in generation.arr_field("candidates").unwrap() {
            assert!(!c.arr_field("chords").unwrap().is_empty());
        }
    }

    #[test]
    fn generate_fixture_is_deterministic() {
        let path = fixtures_dir().join("melodies/eight_bar_c_major.json");
        let a = generate_fixture(&ServerCore::offline(), &path, "jazz_standard", 3, 7).unwrap();
        let b = generate_fixture(&ServerCore::offline(), &path, "jazz_standard", 3, 7).unwrap();
        assert_eq!(a.to_canonical_string(), b.to_canonical_string());
    }

    #[test]
    fn a_missing_fixture_is_a_structured_error() {
        let e = load(Path::new("/nonexistent/fixture.json")).unwrap_err();
        assert_eq!(e.code, codes::INVALID_ARGUMENT);
    }
}
