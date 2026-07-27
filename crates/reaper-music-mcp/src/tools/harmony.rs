//! `harmony.generate_candidates`, `harmony.reharmonize` and `voicing.generate`.
//!
//! All three return the same shape — a list of candidate summaries, each with a
//! `candidate://` URI — because all three produce the same thing: server-issued
//! candidates that can be explained, audited, arranged and staged.
//!
//! # Caching
//!
//! Generation is cached on the tuple the brief §30 names: snapshot, profile,
//! parameters, knowledge hash and seed. The snapshot component is the content
//! **hash**, so re-inspecting an unmodified project hits the cache and editing
//! a single note cannot. `cached` in the result says which happened.
//!
//! # Determinism
//!
//! The engines are deterministic given `(analysis, knowledge version, profile,
//! params, seed)`, and the seed defaults to a fixed constant rather than a
//! clock, so a call repeated with the same arguments returns byte-identical
//! candidates. That is asserted in `generation_is_deterministic`.

use super::{bool_or, f64_or, i64_or, opt_str, profile_of, require_id, require_snapshot, seed_of};
use crate::error::{codes, ToolError};
use crate::server::{CallContext, ServerCore};
use crate::store::{CandidateRecord, SessionStore, SnapshotRecord};
use harmony_engine::prelude::*;
use music_analysis::prelude::*;
use music_domain::prelude::*;
use qjson::{json_obj, Json};

/// The voicing families used when the caller does not name any.
pub const DEFAULT_VOICING_FAMILIES: &[&str] = &["close", "drop2", "shell"];

/// Builds the analysis a generation call works from.
///
/// Uses an explicit `analysis_id` when the caller supplies one — refusing it if
/// it belongs to a different snapshot, which would silently harmonize one piece
/// of music against another's reading — then any analysis already computed for
/// this snapshot, and finally runs one.
pub fn analysis_for(
    core: &ServerCore,
    record: &SnapshotRecord,
    args: &Json,
    ctx: &CallContext,
) -> Result<Analysis, ToolError> {
    if let Some(id) = opt_str(args, "analysis_id") {
        let analysis = core.with_store(|s| s.analysis(id, ctx.now).cloned())?;
        if analysis.snapshot_id != record.snapshot.snapshot_id {
            return Err(ToolError::with_details(
                codes::INVALID_ARGUMENT,
                "the analysis was computed for a different snapshot",
                json_obj! {
                    "analysis_id" => id,
                    "analysis_snapshot_id" => analysis.snapshot_id.clone(),
                    "requested_snapshot_id" => record.snapshot.snapshot_id.clone(),
                },
            )
            .remedy("analyze this snapshot, or omit analysis_id and let the server do it"));
        }
        return Ok(analysis);
    }
    let params = crate::tools::analysis::params_of(core, args)?;
    if let Some(existing) =
        core.with_store(|s| s.analysis_for_snapshot(&record.snapshot.snapshot_id, ctx.now).cloned())
    {
        if existing.profile_id == params.profile_id {
            return Ok(existing);
        }
    }
    crate::tools::analysis::analyze_record(core, record, &params, ctx)
}

/// Builds [`GenerateParams`] from tool arguments.
pub fn generate_params(core: &ServerCore, args: &Json) -> Result<GenerateParams, ToolError> {
    let defaults = GenerateParams::default();
    let bass_motion = match opt_str(args, "bass_motion") {
        Some(s) => BassMotion::parse(s).ok_or_else(|| bad_enum("bass_motion", s))?,
        None => defaults.bass_motion,
    };
    let loop_intent = match opt_str(args, "loop_intent") {
        Some(s) => Some(LoopIntent::parse(s).ok_or_else(|| bad_enum("loop_intent", s))?),
        None => defaults.loop_intent,
    };
    let strictness = match opt_str(args, "strictness") {
        Some(s) => Strictness::parse(s).ok_or_else(|| bad_enum("strictness", s))?,
        None => defaults.strictness,
    };
    let countermelody = match args.get("countermelody") {
        Some(c) => CountermelodyParams {
            enabled: c
                .get("enabled")
                .and_then(Json::as_bool)
                .unwrap_or(defaults.countermelody.enabled),
            density: c
                .get("density")
                .and_then(Json::as_f64)
                .unwrap_or(defaults.countermelody.density),
            role: match c.get("role").and_then(Json::as_str) {
                Some(r) => {
                    ArrangementRole::parse(r).ok_or_else(|| bad_enum("countermelody.role", r))?
                }
                None => defaults.countermelody.role,
            },
        },
        None => defaults.countermelody.clone(),
    };

    let params = GenerateParams {
        profile_id: profile_of(core, args)?,
        candidate_count: i64_or(args, "candidate_count", defaults.candidate_count as i64)
            .clamp(1, 8) as usize,
        preserve_melody: bool_or(args, "preserve_melody", defaults.preserve_melody),
        preserve_rhythm: bool_or(args, "preserve_rhythm", defaults.preserve_rhythm),
        grid: crate::tools::analysis::grid_of(args)?,
        complexity: f64_or(args, "complexity", defaults.complexity),
        chromaticism: f64_or(args, "chromaticism", defaults.chromaticism),
        extension_density: f64_or(args, "extension_density", defaults.extension_density),
        bass_motion,
        countermelody,
        loop_intent,
        strictness,
        seed: seed_of(args),
        voice_count: i64_or(args, "voice_count", defaults.voice_count as i64).clamp(2, 8) as usize,
    };
    params.validate()?;
    Ok(params)
}

fn bad_enum(argument: &str, value: &str) -> ToolError {
    ToolError::with_details(
        codes::INVALID_ARGUMENT,
        format!("{value} is not a valid {argument}"),
        json_obj! { "argument" => argument, "value" => value },
    )
}

/// One candidate as the tool result describes it.
pub fn candidate_summary(record: &CandidateRecord) -> Json {
    let c = &record.candidate;
    let mut rule_ids: Vec<String> = c
        .trace
        .rule_applications
        .iter()
        .map(|r| r.rule_id.clone())
        .collect();
    rule_ids.sort();
    rule_ids.dedup();

    json_obj! {
        "candidate_id" => c.id.clone(),
        "kind" => c.kind.id(),
        "label" => c.label.clone(),
        "strategy" => c.strategy.clone(),
        "resource_uri" => format!("candidate://{}", c.id),
        "trace_uri" => format!("candidate://{}/trace", c.id),
        "chord_count" => c.chords.len() as i64,
        "note_count" => c.parts.iter().map(|p| p.notes.len()).sum::<usize>() as i64,
        "chords" => Json::Arr(
            c.chords.iter().map(|e| Json::Str(e.spec.render_ascii())).collect()
        ),
        "confidence" => c.trace.confidence,
        "score_total" => c.trace.score.total(),
        "score_components" => Json::Arr(
            c.trace
                .score
                .components()
                .map(|(name, value)| json_obj! { "name" => name, "value" => value })
                .collect()
        ),
        "rule_ids" => Json::Arr(rule_ids.into_iter().map(Json::Str).collect()),
        "source_ids" => Json::Arr(
            c.trace.source_ids.iter().map(|s| Json::Str(s.clone())).collect()
        ),
        "loop" => match &c.loop_report {
            Some(r) => r.to_json(),
            None => Json::Null,
        },
        "explanation" => c.trace.explanation.clone(),
        "parts" => Json::Arr(
            c.parts
                .iter()
                .map(|p| Json::Str(format!("{}:{}", p.role.id(), p.name)))
                .collect()
        ),
    }
}

/// The shared result envelope for the three generation tools.
#[allow(clippy::too_many_arguments)] // Every field is a distinct provenance fact the result must carry.
fn candidate_list_body(
    core: &ServerCore,
    snapshot_id: Option<&str>,
    analysis_id: Option<&str>,
    profile_id: &str,
    seed: u64,
    cached: bool,
    records: &[CandidateRecord],
    warnings: Vec<Json>,
) -> Json {
    json_obj! {
        "ok" => true,
        "snapshot_id" => match snapshot_id {
            Some(s) => Json::Str(s.to_string()),
            None => Json::Null,
        },
        "analysis_id" => match analysis_id {
            Some(s) => Json::Str(s.to_string()),
            None => Json::Null,
        },
        "profile_id" => profile_id,
        "seed" => seed as i64,
        "cached" => cached,
        "candidate_count" => records.len() as i64,
        "candidates" => Json::Arr(records.iter().map(candidate_summary).collect()),
        "knowledge_version" => core.knowledge_version(),
        "warnings" => Json::Arr(warnings),
    }
}

/// `harmony.generate_candidates`.
pub fn generate_candidates(
    core: &ServerCore,
    args: &Json,
    ctx: &CallContext,
) -> Result<Json, ToolError> {
    let record = require_snapshot(core, args, ctx)?;
    let params = generate_params(core, args)?;
    ctx.check_cancelled()?;
    ctx.report(0.05, "reading the selection");
    let analysis = analysis_for(core, &record, args, ctx)?;

    let snapshot_hash = record
        .snapshot
        .snapshot_hash
        .clone()
        .unwrap_or_else(|| record.snapshot.snapshot_id.clone());
    let key = SessionStore::cache_key(
        &snapshot_hash,
        &params.profile_id,
        &params.canonical_json().to_canonical_string(),
        &core.knowledge_hash(),
        params.seed,
    );

    if let Some(ids) = core.with_store(|s| s.cached(&key, ctx.now)) {
        let records: Vec<CandidateRecord> =
            core.with_store(|s| ids.iter().filter_map(|id| s.candidate(id, ctx.now).ok().cloned()).collect());
        if records.len() == ids.len() {
            crate::log::debug("serving candidates from the generation cache");
            return Ok(candidate_list_body(
                core,
                Some(&record.snapshot.snapshot_id),
                Some(&analysis.id),
                &params.profile_id,
                params.seed,
                true,
                &records,
                Vec::new(),
            ));
        }
    }

    let bridge = super::CancelBridge::new(&ctx.cancel);
    ctx.report(0.15, "generating candidates");
    let mut progress = |fraction: f64, label: &str| {
        ctx.report(0.15 + 0.8 * fraction, label);
    };
    let candidates = harmony_engine::generate::generate_candidates(
        core.knowledge.kb(),
        &analysis,
        &params,
        bridge.flag(),
        &mut progress,
    )?;
    ctx.check_cancelled()?;

    let records = store_candidates(
        core,
        candidates,
        &record.snapshot.snapshot_id,
        &analysis.id,
        &params.profile_id,
        params.seed,
        ctx,
    );
    let ids: Vec<String> = records.iter().map(|r| r.candidate.id.clone()).collect();
    core.with_store(|s| s.cache(&key, ids, ctx.now));

    ctx.report(1.0, "candidates stored");
    Ok(candidate_list_body(
        core,
        Some(&record.snapshot.snapshot_id),
        Some(&analysis.id),
        &params.profile_id,
        params.seed,
        false,
        &records,
        Vec::new(),
    ))
}

fn store_candidates(
    core: &ServerCore,
    candidates: Vec<Candidate>,
    snapshot_id: &str,
    analysis_id: &str,
    profile_id: &str,
    seed: u64,
    ctx: &CallContext,
) -> Vec<CandidateRecord> {
    let knowledge_hash = core.knowledge_hash();
    let mut out = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let record = CandidateRecord {
            candidate,
            snapshot_id: snapshot_id.to_string(),
            analysis_id: analysis_id.to_string(),
            profile_id: profile_id.to_string(),
            knowledge_hash: knowledge_hash.clone(),
            seed,
        };
        core.with_store(|s| s.put_candidate(record.clone(), ctx.now));
        out.push(record);
    }
    out
}

/// `harmony.reharmonize`.
pub fn reharmonize(core: &ServerCore, args: &Json, ctx: &CallContext) -> Result<Json, ToolError> {
    let profile_id = profile_of(core, args)?;
    let seed = seed_of(args);

    // The chords to work from come either from a generated candidate or from
    // what the analysis detected in the source material.
    let (record, analysis, existing) = match opt_str(args, "candidate_id") {
        Some(id) => {
            let cand = core.with_store(|s| s.candidate(id, ctx.now).cloned())?;
            let snapshot = core.with_store(|s| s.snapshot(&cand.snapshot_id, ctx.now).cloned())?;
            let analysis = core.with_store(|s| s.analysis(&cand.analysis_id, ctx.now).cloned())?;
            let chords = cand.candidate.chords.clone();
            (snapshot, analysis, chords)
        }
        None => {
            let record = require_snapshot(core, args, ctx)?;
            let analysis = analysis_for(core, &record, args, ctx)?;
            let chords = analysis.detected_chords.clone();
            (record, analysis, chords)
        }
    };

    if existing.is_empty() {
        return Err(ToolError::new(
            codes::INVALID_ARGUMENT,
            "there is no existing harmony to reharmonize",
        )
        .remedy(
            "generate a candidate first, or select material whose harmony the analysis can detect",
        ));
    }

    let defaults = harmony_engine::reharm::ReharmParams::default();
    let params = harmony_engine::reharm::ReharmParams {
        preserve_melody: bool_or(args, "preserve_melody", defaults.preserve_melody),
        preserve_bass: bool_or(args, "preserve_bass", defaults.preserve_bass),
        preserve_cadence: bool_or(args, "preserve_cadence", defaults.preserve_cadence),
        preserve_harmonic_rhythm: bool_or(
            args,
            "preserve_harmonic_rhythm",
            defaults.preserve_harmonic_rhythm,
        ),
        families: args
            .get("families")
            .and_then(Json::as_arr)
            .map(|a| {
                a.iter()
                    .filter_map(Json::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        complexity: f64_or(args, "complexity", defaults.complexity),
        chromaticism: f64_or(args, "chromaticism", defaults.chromaticism),
        profile_id: profile_id.clone(),
        candidate_count: i64_or(args, "candidate_count", defaults.candidate_count as i64)
            .clamp(1, 8) as usize,
        seed,
    };

    let bridge = super::CancelBridge::new(&ctx.cancel);
    ctx.report(0.3, "reharmonizing");
    let candidates = harmony_engine::reharm::reharmonize(
        core.knowledge.kb(),
        &analysis,
        &existing,
        &params,
        bridge.flag(),
    )?;
    ctx.check_cancelled()?;

    let records = store_candidates(
        core,
        candidates,
        &record.snapshot.snapshot_id,
        &analysis.id,
        &profile_id,
        seed,
        ctx,
    );
    ctx.report(1.0, "candidates stored");
    Ok(candidate_list_body(
        core,
        Some(&record.snapshot.snapshot_id),
        Some(&analysis.id),
        &profile_id,
        seed,
        false,
        &records,
        Vec::new(),
    ))
}

/// `voicing.generate`.
pub fn voicing_generate(
    core: &ServerCore,
    args: &Json,
    ctx: &CallContext,
) -> Result<Json, ToolError> {
    let candidate_id = require_id(args, "candidate_id")?;
    let source = core.with_store(|s| s.candidate(&candidate_id, ctx.now).cloned())?;
    if source.candidate.chords.is_empty() {
        return Err(ToolError::new(
            codes::INVALID_ARGUMENT,
            "the candidate carries no chords to voice",
        ));
    }
    let profile_id = profile_of(
        core,
        &match args.clone() {
            Json::Obj(mut m) => {
                if m.get("style_profile").is_none() {
                    m.insert("style_profile", Json::Str(source.profile_id.clone()));
                }
                Json::Obj(m)
            }
            other => other,
        },
    )?;
    let profile = core
        .knowledge
        .kb()
        .resolve_profile(&profile_id)
        .map_err(ToolError::from)?;

    let requested: Vec<String> = args
        .get("families")
        .and_then(Json::as_arr)
        .map(|a| {
            a.iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| {
            DEFAULT_VOICING_FAMILIES
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        });
    let wanted = i64_or(args, "candidate_count", requested.len() as i64).clamp(1, 8) as usize;
    let seed = seed_of(args);

    let melody = core
        .with_store(|s| s.analysis(&source.analysis_id, ctx.now).cloned())
        .ok()
        .map(|a| a.extraction.melody.clone());

    let defaults = VoicingParams::default();
    let mut records = Vec::new();
    for family_id in requested.iter().take(wanted) {
        ctx.check_cancelled()?;
        let family = VoicingFamily::parse(family_id).ok_or_else(|| bad_enum("families", family_id))?;
        let params = VoicingParams {
            voice_count: i64_or(args, "voice_count", defaults.voice_count as i64).clamp(2, 8)
                as usize,
            families: vec![family],
            low: i64_or(args, "low", i64::from(defaults.low)).clamp(0, 127) as i32,
            high: i64_or(args, "high", i64::from(defaults.high)).clamp(0, 127) as i32,
            preserve_top: bool_or(args, "preserve_top", true),
            preserve_bass: bool_or(args, "preserve_bass", defaults.preserve_bass),
            max_leap: i64_or(args, "max_leap", i64::from(defaults.max_leap)).clamp(1, 36) as i32,
            instrument_profile: opt_str(args, "instrument_profile")
                .map(str::to_string)
                .or_else(|| defaults.instrument_profile.clone()),
            seed,
        };
        params.validate()?;

        let voicings = voice_progression(
            core.knowledge.kb(),
            &profile,
            &source.candidate.chords,
            melody.as_ref(),
            &params,
        )?;
        let report =
            audit_voice_leading(core.knowledge.kb(), &profile, &voicings, &source.candidate.chords);

        let variant = build_voicing_candidate(&source, family, &voicings, &report, seed, ctx.now)?;
        let record = CandidateRecord {
            candidate: variant,
            snapshot_id: source.snapshot_id.clone(),
            analysis_id: source.analysis_id.clone(),
            profile_id: profile_id.clone(),
            knowledge_hash: core.knowledge_hash(),
            seed,
        };
        core.with_store(|s| s.put_candidate(record.clone(), ctx.now));
        records.push(record);
        ctx.report(
            records.len() as f64 / wanted.max(1) as f64,
            "voicing variants",
        );
    }

    Ok(candidate_list_body(
        core,
        Some(&source.snapshot_id),
        Some(&source.analysis_id),
        &profile_id,
        seed,
        false,
        &records,
        Vec::new(),
    ))
}

/// The first note id a voicing part uses.
const VOICING_ID_BASE: NoteId = 500_000;

/// Turns a set of voicings into a stageable candidate.
fn build_voicing_candidate(
    source: &CandidateRecord,
    family: VoicingFamily,
    voicings: &[Voicing],
    report: &VoiceLeadingReport,
    seed: u64,
    now: i64,
) -> Result<Candidate, ToolError> {
    let mut notes = Vec::new();
    let mut next_id = VOICING_ID_BASE;
    for (i, voicing) in voicings.iter().enumerate() {
        let Some(chord) = source.candidate.chords.get(i) else {
            break;
        };
        for (v, pitch) in voicing.pitches.iter().enumerate() {
            if !pitch.is_valid_midi() {
                continue;
            }
            let mut note = Note::new(next_id, *pitch, chord.onset, chord.duration);
            next_id += 1;
            note.midi = pitch.midi();
            note.velocity = 72;
            note.voice = voicing
                .voices
                .get(v)
                .copied()
                .unwrap_or(VoiceId(v as u16));
            note.role = NoteRole::Harmony;
            notes.push(note);
        }
    }
    if notes.is_empty() {
        return Err(ToolError::new(
            codes::GENERATION_FAILED,
            format!("the {} voicing produced no playable notes", family.id()),
        ));
    }

    let id = qjson::uuid::uuid_from_name(
        "qlabs.mcp.voicing",
        &format!("{}|{}|{seed}", source.candidate.id, family.id()),
    );
    let mut trace = source.candidate.trace.clone();
    trace.candidate_id = id.clone();
    trace.score = report.score.clone();
    trace.rule_applications = report.rule_applications.clone();
    trace.warnings = report.findings.clone();
    trace.explanation = format!(
        "{} voicing of candidate {}: {} semitones of total motion, largest leap {}, {} parallel \
         perfect interval(s), {} crossing(s), {} overlap(s).",
        family.id(),
        source.candidate.id,
        report.total_motion,
        report.max_leap,
        report.true_parallels().len(),
        report.crossings,
        report.overlaps
    );

    // A re-voicing keeps every non-harmony part of the source candidate — the
    // melody, bass and countermelody are not what is being changed — and
    // replaces only the harmonic bed.
    let mut parts: Vec<Part> = source
        .candidate
        .parts
        .iter()
        .filter(|p| p.role != ArrangementRole::HarmonicBed)
        .cloned()
        .collect();
    parts.push(Part {
        role: ArrangementRole::HarmonicBed,
        name: format!("Voicing {}", family.id()),
        notes,
        instrument_profile: Some("piano_keys".to_string()),
        channel: 0,
        polyphonic: true,
    });

    Ok(Candidate {
        id,
        kind: CandidateKind::Voicing,
        label: format!("{} voicing", family.id()),
        strategy: format!("voicing_{}", family.id()),
        chords: source.candidate.chords.clone(),
        parts,
        trace,
        loop_report: source.candidate.loop_report.clone(),
        created_at: qjson::time::iso8601_from_unix(now),
        expires_at: qjson::time::iso8601_from_unix(now + crate::store::CANDIDATE_TTL_SECONDS),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::snapshot_from_fixture;
    use std::sync::Arc;

    fn core_with_snapshot(fixture: &str) -> (Arc<ServerCore>, String) {
        let core = ServerCore::offline();
        let record = snapshot_from_fixture(fixture).expect(fixture);
        let id = core.with_store(|s| s.put_snapshot(record, qjson::time::unix_now()));
        (core, id)
    }

    fn generate(core: &ServerCore, id: &str, extra: &[(&str, Json)]) -> Json {
        let mut args = json_obj! { "snapshot_id" => id, "candidate_count" => 3, "seed" => 4242 };
        if let Json::Obj(m) = &mut args {
            for (k, v) in extra {
                m.insert(*k, v.clone());
            }
        }
        generate_candidates(core, &args, &CallContext::detached()).expect("generation")
    }

    #[test]
    fn generation_validates_against_its_declared_schema() {
        let (core, id) = core_with_snapshot("melodies/eight_bar_c_major");
        let tool = core.tools.get("harmony.generate_candidates").unwrap();
        let body = generate(&core, &id, &[]);
        assert!(tool.output.validate(&body).is_empty(), "{body:?}");
    }

    #[test]
    fn three_candidates_are_returned_with_uris_and_traces() {
        let (core, id) = core_with_snapshot("melodies/eight_bar_c_major");
        let body = generate(&core, &id, &[]);
        let candidates = body.arr_field("candidates").unwrap();
        assert_eq!(candidates.len(), 3);
        for c in candidates {
            assert!(c.str_field("resource_uri").unwrap().starts_with("candidate://"));
            assert!(c.str_field("trace_uri").unwrap().ends_with("/trace"));
            assert!(!c.arr_field("chords").unwrap().is_empty());
            assert!(!c.arr_field("score_components").unwrap().is_empty());
            assert!(!c.str_field("explanation").unwrap().is_empty());
        }
    }

    #[test]
    fn candidates_are_readable_as_resources() {
        let (core, id) = core_with_snapshot("melodies/eight_bar_c_major");
        let body = generate(&core, &id, &[]);
        for c in body.arr_field("candidates").unwrap() {
            let uri = c.str_field("resource_uri").unwrap();
            let resource = crate::resources::read(&core, uri).unwrap();
            assert_eq!(
                resource.str_field("id").unwrap(),
                c.str_field("candidate_id").unwrap()
            );
            let trace = crate::resources::read(&core, c.str_field("trace_uri").unwrap()).unwrap();
            assert!(trace.get("score").is_some());
        }
    }

    #[test]
    fn generation_is_deterministic() {
        let (core_a, id_a) = core_with_snapshot("melodies/eight_bar_c_major");
        let (core_b, id_b) = core_with_snapshot("melodies/eight_bar_c_major");
        let a = generate(&core_a, &id_a, &[]);
        let b = generate(&core_b, &id_b, &[]);
        assert_eq!(a.to_canonical_string(), b.to_canonical_string());
    }

    #[test]
    fn a_second_identical_call_is_served_from_the_cache() {
        let (core, id) = core_with_snapshot("melodies/eight_bar_c_major");
        let first = generate(&core, &id, &[]);
        assert_eq!(first.get("cached"), Some(&Json::Bool(false)));
        let second = generate(&core, &id, &[]);
        assert_eq!(second.get("cached"), Some(&Json::Bool(true)));
        assert_eq!(
            first.arr_field("candidates").unwrap().len(),
            second.arr_field("candidates").unwrap().len()
        );
    }

    #[test]
    fn a_different_seed_misses_the_cache() {
        let (core, id) = core_with_snapshot("melodies/eight_bar_c_major");
        generate(&core, &id, &[]);
        let other = generate(&core, &id, &[("seed", Json::Int(999))]);
        assert_eq!(other.get("cached"), Some(&Json::Bool(false)));
    }

    #[test]
    fn a_changed_snapshot_never_serves_cached_candidates() {
        let core = ServerCore::offline();
        let now = qjson::time::unix_now();
        let a = snapshot_from_fixture("melodies/eight_bar_c_major").unwrap();
        let b = snapshot_from_fixture("melodies/dorian_vamp_d").unwrap();
        assert_ne!(a.snapshot.snapshot_hash, b.snapshot.snapshot_hash);
        let id_a = core.with_store(|s| s.put_snapshot(a, now));
        let id_b = core.with_store(|s| s.put_snapshot(b, now));
        generate(&core, &id_a, &[]);
        let second = generate(&core, &id_b, &[]);
        assert_eq!(second.get("cached"), Some(&Json::Bool(false)));
    }

    #[test]
    fn candidates_use_genuinely_different_strategies() {
        let (core, id) = core_with_snapshot("melodies/eight_bar_c_major");
        let body = generate(&core, &id, &[("style_profile", Json::Str("jazz_standard".into()))]);
        let strategies: Vec<String> = body
            .arr_field("candidates")
            .unwrap()
            .iter()
            .map(|c| c.str_field("strategy").unwrap().to_string())
            .collect();
        let mut unique = strategies.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), strategies.len(), "{strategies:?}");
    }

    #[test]
    fn preserve_melody_keeps_every_source_pitch() {
        let (core, id) = core_with_snapshot("melodies/eight_bar_c_major");
        let record = core.with_store(|s| s.snapshot(&id, qjson::time::unix_now()).cloned()).unwrap();
        let body = generate(&core, &id, &[("preserve_melody", Json::Bool(true))]);
        let first = body.arr_field("candidates").unwrap()[0]
            .str_field("candidate_id")
            .unwrap()
            .to_string();
        let candidate = core
            .with_store(|s| s.candidate(&first, qjson::time::unix_now()).cloned())
            .unwrap();
        let lead = candidate
            .candidate
            .parts
            .iter()
            .find(|p| p.role == ArrangementRole::Lead)
            .expect("a lead part");
        for source_note in &record.notes.notes {
            assert!(
                lead.notes
                    .iter()
                    .any(|n| n.midi == source_note.midi && n.onset == source_note.onset),
                "melody note {} at {} was not preserved",
                source_note.midi,
                source_note.onset.to_display()
            );
        }
    }

    #[test]
    fn an_analysis_from_another_snapshot_is_refused() {
        let core = ServerCore::offline();
        let now = qjson::time::unix_now();
        let a = snapshot_from_fixture("melodies/eight_bar_c_major").unwrap();
        let b = snapshot_from_fixture("melodies/dorian_vamp_d").unwrap();
        let id_a = core.with_store(|s| s.put_snapshot(a, now));
        let id_b = core.with_store(|s| s.put_snapshot(b, now));
        let analysis_b = crate::tools::analysis::analyze_selection(
            &core,
            &json_obj! { "snapshot_id" => id_b },
            &CallContext::detached(),
        )
        .unwrap();
        let e = generate_candidates(
            &core,
            &json_obj! {
                "snapshot_id" => id_a,
                "analysis_id" => analysis_b.str_field("analysis_id").unwrap(),
            },
            &CallContext::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::INVALID_ARGUMENT);
    }

    #[test]
    fn a_cancelled_generation_returns_cancelled() {
        let (core, id) = core_with_snapshot("melodies/eight_bar_c_major");
        let ctx = CallContext::detached();
        ctx.cancel.cancel();
        let e = generate_candidates(&core, &json_obj! { "snapshot_id" => id }, &ctx).unwrap_err();
        assert_eq!(e.code, codes::CANCELLED);
    }

    #[test]
    fn cancellation_leaves_no_partial_candidates_behind() {
        let (core, id) = core_with_snapshot("melodies/eight_bar_c_major");
        let ctx = CallContext::detached();
        ctx.cancel.cancel();
        let _ = generate_candidates(&core, &json_obj! { "snapshot_id" => id }, &ctx);
        let stats = core.with_store(|s| s.stats());
        assert_eq!(stats.i64_field("candidates"), Ok(0));
        assert_eq!(stats.i64_field("cached_generations"), Ok(0));
    }

    #[test]
    fn reharmonize_validates_against_its_declared_schema() {
        let (core, id) = core_with_snapshot("progressions/ii_v_i_c_major");
        let tool = core.tools.get("harmony.reharmonize").unwrap();
        let generated = generate(&core, &id, &[]);
        let candidate_id = generated.arr_field("candidates").unwrap()[0]
            .str_field("candidate_id")
            .unwrap()
            .to_string();
        let body = reharmonize(
            &core,
            &json_obj! { "candidate_id" => candidate_id, "candidate_count" => 2 },
            &CallContext::detached(),
        )
        .unwrap();
        assert!(tool.output.validate(&body).is_empty(), "{body:?}");
        assert_eq!(body.i64_field("candidate_count"), Ok(2));
    }

    #[test]
    fn voicing_generate_validates_against_its_declared_schema() {
        let (core, id) = core_with_snapshot("melodies/eight_bar_c_major");
        let tool = core.tools.get("voicing.generate").unwrap();
        let generated = generate(&core, &id, &[]);
        let candidate_id = generated.arr_field("candidates").unwrap()[0]
            .str_field("candidate_id")
            .unwrap()
            .to_string();
        let body = voicing_generate(
            &core,
            &json_obj! { "candidate_id" => candidate_id },
            &CallContext::detached(),
        )
        .unwrap();
        assert!(tool.output.validate(&body).is_empty(), "{body:?}");
        assert!(!body.arr_field("candidates").unwrap().is_empty());
        for c in body.arr_field("candidates").unwrap() {
            assert_eq!(c.str_field("kind").unwrap(), "voicing");
            assert!(c.i64_field("note_count").unwrap() > 0);
        }
    }

    #[test]
    fn voicing_generate_refuses_an_unknown_candidate() {
        let core = ServerCore::offline();
        let e = voicing_generate(
            &core,
            &json_obj! { "candidate_id" => "nope" },
            &CallContext::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::UNKNOWN_ID);
    }

    #[test]
    fn generate_params_reject_a_bad_enum() {
        let core = ServerCore::offline();
        let e = generate_params(&core, &json_obj! { "bass_motion" => "sideways" }).unwrap_err();
        assert_eq!(e.code, codes::INVALID_ARGUMENT);
    }

    #[test]
    fn generate_params_take_their_defaults() {
        let core = ServerCore::offline();
        let p = generate_params(&core, &json_obj! {}).unwrap();
        assert_eq!(p.profile_id, super::super::DEFAULT_PROFILE);
        assert_eq!(p.seed, super::super::DEFAULT_SEED);
        assert!(p.preserve_melody);
    }
}
