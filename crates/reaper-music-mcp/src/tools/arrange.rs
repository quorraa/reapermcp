//! `arrangement.generate` — role-based parts from a candidate.
//!
//! The result is itself a server-issued candidate, so an arrangement can be
//! explained, loop-audited and staged through exactly the same path as a
//! harmonization. That is deliberate: there is one staging code path, and
//! therefore one place where the seven preconditions are attached.

use super::{bool_or, f64_or, opt_str, profile_of, require_id, seed_of};
use crate::error::{codes, ToolError};
use crate::server::{CallContext, ServerCore};
use crate::store::CandidateRecord;
use arrangement_engine::ArrangementParams;
use music_domain::prelude::*;
use qjson::{json_obj, Json};

/// Builds [`ArrangementParams`] from tool arguments.
pub fn params_of(
    core: &ServerCore,
    args: &Json,
    fallback_profile: &str,
) -> Result<ArrangementParams, ToolError> {
    let with_profile = match args.clone() {
        Json::Obj(mut m) => {
            if m.get("style_profile").is_none() {
                m.insert("style_profile", Json::Str(fallback_profile.to_string()));
            }
            Json::Obj(m)
        }
        other => other,
    };
    let profile_id = profile_of(core, &with_profile)?;
    let defaults = ArrangementParams::default();

    let roles = match args.get("roles").and_then(Json::as_arr) {
        Some(a) if !a.is_empty() => {
            let mut out = Vec::new();
            for r in a {
                let id = r.as_str().unwrap_or("");
                out.push(ArrangementRole::parse(id).ok_or_else(|| {
                    ToolError::with_details(
                        codes::INVALID_ARGUMENT,
                        format!("{id} is not an arrangement role"),
                        json_obj! { "argument" => "roles", "value" => id },
                    )
                })?);
            }
            out
        }
        _ => defaults.roles.clone(),
    };

    let mut energy_curve = Vec::new();
    if let Some(points) = args.get("energy_curve").and_then(Json::as_arr) {
        for p in points {
            let (Some(qn), Some(v)) = (
                p.get("qn").and_then(Json::as_f64),
                p.get("value").and_then(Json::as_f64),
            ) else {
                return Err(ToolError::with_details(
                    codes::INVALID_ARGUMENT,
                    "each energy_curve point needs qn and value",
                    json_obj! { "argument" => "energy_curve" },
                ));
            };
            energy_curve.push((BeatTime::from_f64_grid(qn, crate::convert::QN_GRID), v));
        }
    }

    let mut sections = Vec::new();
    if let Some(list) = args.get("sections").and_then(Json::as_arr) {
        for s in list {
            let id = s.get("id").and_then(Json::as_str).unwrap_or("").to_string();
            let (Some(start), Some(end)) = (
                s.get("start_qn").and_then(Json::as_f64),
                s.get("end_qn").and_then(Json::as_f64),
            ) else {
                return Err(ToolError::with_details(
                    codes::INVALID_ARGUMENT,
                    "each section needs start_qn and end_qn",
                    json_obj! { "argument" => "sections" },
                ));
            };
            if end <= start {
                return Err(ToolError::with_details(
                    codes::INVALID_ARGUMENT,
                    format!("section {id} ends before it starts"),
                    json_obj! { "argument" => "sections", "section" => id.clone() },
                ));
            }
            sections.push(Section {
                id,
                start: BeatTime::from_f64_grid(start, crate::convert::QN_GRID),
                end: BeatTime::from_f64_grid(end, crate::convert::QN_GRID),
                role: s
                    .get("role")
                    .and_then(Json::as_str)
                    .unwrap_or("section")
                    .to_string(),
                energy: s.get("energy").and_then(Json::as_f64).unwrap_or(0.5),
            });
        }
    }

    let loop_intent = match opt_str(args, "loop_intent") {
        Some(s) => Some(LoopIntent::parse(s).ok_or_else(|| {
            ToolError::with_details(
                codes::INVALID_ARGUMENT,
                format!("{s} is not a loop intent"),
                json_obj! { "argument" => "loop_intent", "value" => s },
            )
        })?),
        None => defaults.loop_intent,
    };

    let texture_pattern = match opt_str(args, "texture_pattern") {
        Some(p) => {
            if core
                .knowledge
                .kb()
                .arrangement_patterns()
                .iter()
                .all(|x| x.id != p)
            {
                return Err(ToolError::with_details(
                    codes::INVALID_ARGUMENT,
                    format!("{p} is not an arrangement pattern in this knowledge bundle"),
                    json_obj! { "argument" => "texture_pattern", "value" => p },
                )
                .remedy("read theory://catalog for the pattern ids this build knows"));
            }
            Some(p.to_string())
        }
        None => defaults.texture_pattern.clone(),
    };

    let params = ArrangementParams {
        profile_id,
        roles,
        energy_curve,
        density: f64_or(args, "density", defaults.density),
        texture_pattern,
        register_spread: f64_or(args, "register_spread", defaults.register_spread),
        sections,
        preserve_melody: bool_or(args, "preserve_melody", defaults.preserve_melody),
        loop_intent,
        seed: seed_of(args),
    };
    params.validate()?;
    Ok(params)
}

/// `arrangement.generate`.
pub fn generate(core: &ServerCore, args: &Json, ctx: &CallContext) -> Result<Json, ToolError> {
    let source_id = require_id(args, "candidate_id")?;
    let source = core.with_store(|s| s.candidate(&source_id, ctx.now).cloned())?;
    let analysis = core.with_store(|s| s.analysis(&source.analysis_id, ctx.now).cloned())?;
    let params = params_of(core, args, &source.profile_id)?;

    ctx.check_cancelled()?;
    ctx.report(0.2, "arranging");
    let bridge = super::CancelBridge::new(&ctx.cancel);
    let plan = arrangement_engine::arrange(
        core.knowledge.kb(),
        &analysis,
        &source.candidate,
        &params,
        bridge.flag(),
    )?;
    ctx.check_cancelled()?;

    let id = qjson::uuid::uuid_from_name(
        "qlabs.mcp.arrangement",
        &format!(
            "{}|{}|{}",
            source.candidate.id, params.profile_id, params.seed
        ),
    );
    let mut trace = source.candidate.trace.clone();
    trace.candidate_id = id.clone();
    trace.score = plan.score.clone();
    trace.rule_applications = plan.rule_applications.clone();
    trace.warnings = plan.warnings.clone();
    trace.explanation = format!(
        "Arrangement of candidate {} in {}: {} layer(s) — {} — over {} note(s).",
        source.candidate.id,
        params.profile_id,
        plan.layer_count(),
        plan.parts
            .iter()
            .map(|p| p.role.id())
            .collect::<Vec<_>>()
            .join(", "),
        plan.notes().len()
    );

    let note_count: usize = plan.parts.iter().map(|p| p.notes.len()).sum();
    let candidate = Candidate {
        id: id.clone(),
        kind: CandidateKind::Arrangement,
        label: format!("Arrangement ({} layers)", plan.layer_count()),
        strategy: format!("arrangement_{}", params.profile_id),
        chords: source.candidate.chords.clone(),
        parts: plan.parts.clone(),
        trace,
        loop_report: source.candidate.loop_report.clone(),
        created_at: qjson::time::iso8601_from_unix(ctx.now),
        expires_at: qjson::time::iso8601_from_unix(ctx.now + crate::store::CANDIDATE_TTL_SECONDS),
    };
    let record = CandidateRecord {
        candidate,
        snapshot_id: source.snapshot_id.clone(),
        analysis_id: source.analysis_id.clone(),
        profile_id: params.profile_id.clone(),
        knowledge_hash: core.knowledge_hash(),
        seed: params.seed,
    };
    core.with_store(|s| s.put_candidate(record.clone(), ctx.now));

    ctx.report(1.0, "arrangement stored");
    Ok(json_obj! {
        "ok" => true,
        "candidate_id" => id.clone(),
        "source_candidate_id" => source_id,
        "resource_uri" => format!("candidate://{id}"),
        "trace_uri" => format!("candidate://{id}/trace"),
        "profile_id" => params.profile_id.clone(),
        "seed" => params.seed as i64,
        "parts" => Json::Arr(plan.parts.iter().map(|p| json_obj! {
            "role" => p.role.id(),
            "name" => p.name.clone(),
            "note_count" => p.notes.len() as i64,
            "low_pitch" => p.notes.iter().map(|n| n.midi).min().unwrap_or(0) as i64,
            "high_pitch" => p.notes.iter().map(|n| n.midi).max().unwrap_or(0) as i64,
            "instrument_profile" => match &p.instrument_profile {
                Some(i) => Json::Str(i.clone()),
                None => Json::Null,
            },
            "polyphonic" => p.polyphonic,
        }).collect()),
        "assignments" => Json::Arr(plan.assignments.iter().map(|a| a.to_json()).collect()),
        "energy" => Json::Arr(plan.energy.iter().map(|(qn, v)| json_obj! {
            "qn" => qn.as_f64(),
            "value" => *v,
        }).collect()),
        "masking" => json_obj! {
            "onset_collisions_before" => plan.masking_before.onset_collisions as i64,
            "onset_collisions_after" => plan.masking_after.onset_collisions as i64,
            "register_overlap_before" => plan.masking_before.register_overlap,
            "register_overlap_after" => plan.masking_after.register_overlap,
            "suggestions" => Json::Arr(
                plan.masking_after
                    .suggestions
                    .iter()
                    .map(|s| Json::Str(s.clone()))
                    .collect()
            ),
        },
        "score_total" => plan.score.total(),
        "note_count" => note_count as i64,
        "knowledge_version" => core.knowledge_version(),
        "warnings" => Json::Arr(plan.warnings.iter().map(|w| w.to_json()).collect()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::snapshot_from_fixture;

    fn seeded() -> (std::sync::Arc<ServerCore>, String) {
        let core = ServerCore::offline();
        let record = snapshot_from_fixture("melodies/eight_bar_c_major").unwrap();
        let snapshot_id = core.with_store(|s| s.put_snapshot(record, qjson::time::unix_now()));
        let generated = crate::tools::harmony::generate_candidates(
            &core,
            &json_obj! { "snapshot_id" => snapshot_id, "candidate_count" => 1, "seed" => 21 },
            &CallContext::detached(),
        )
        .unwrap();
        let candidate_id = generated.arr_field("candidates").unwrap()[0]
            .str_field("candidate_id")
            .unwrap()
            .to_string();
        (core, candidate_id)
    }

    #[test]
    fn arrangement_validates_against_its_declared_schema() {
        let (core, candidate_id) = seeded();
        let tool = core.tools.get("arrangement.generate").unwrap();
        let body = generate(
            &core,
            &json_obj! { "candidate_id" => candidate_id },
            &CallContext::detached(),
        )
        .unwrap();
        assert!(tool.output.validate(&body).is_empty(), "{body:?}");
    }

    #[test]
    fn an_arrangement_produces_several_roles() {
        let (core, candidate_id) = seeded();
        let body = generate(
            &core,
            &json_obj! {
                "candidate_id" => candidate_id,
                "roles" => Json::Arr(vec![
                    Json::Str("bass".into()),
                    Json::Str("pad".into()),
                    Json::Str("comping".into()),
                ]),
            },
            &CallContext::detached(),
        )
        .unwrap();
        let roles: Vec<String> = body
            .arr_field("parts")
            .unwrap()
            .iter()
            .map(|p| p.str_field("role").unwrap().to_string())
            .collect();
        assert!(roles.len() >= 2, "{roles:?}");
        assert!(body.i64_field("note_count").unwrap() > 0);
    }

    #[test]
    fn the_arrangement_is_itself_a_stageable_candidate() {
        let (core, candidate_id) = seeded();
        let body = generate(
            &core,
            &json_obj! { "candidate_id" => candidate_id },
            &CallContext::detached(),
        )
        .unwrap();
        let uri = body.str_field("resource_uri").unwrap();
        let resource = crate::resources::read(&core, uri).unwrap();
        assert_eq!(resource.str_field("kind").unwrap(), "arrangement");
    }

    #[test]
    fn an_unknown_role_is_refused() {
        let (core, candidate_id) = seeded();
        let e = generate(
            &core,
            &json_obj! {
                "candidate_id" => candidate_id,
                "roles" => Json::Arr(vec![Json::Str("kazoo".into())]),
            },
            &CallContext::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::INVALID_ARGUMENT);
    }

    #[test]
    fn an_unknown_texture_pattern_is_refused() {
        let (core, candidate_id) = seeded();
        let e = generate(
            &core,
            &json_obj! { "candidate_id" => candidate_id, "texture_pattern" => "not_a_pattern" },
            &CallContext::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::INVALID_ARGUMENT);
    }

    #[test]
    fn a_backwards_section_is_refused() {
        let (core, candidate_id) = seeded();
        let e = generate(
            &core,
            &json_obj! {
                "candidate_id" => candidate_id,
                "sections" => Json::Arr(vec![json_obj! {
                    "id" => "a", "start_qn" => 8.0, "end_qn" => 4.0,
                }]),
            },
            &CallContext::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::INVALID_ARGUMENT);
    }

    #[test]
    fn an_unknown_candidate_is_refused() {
        let core = ServerCore::offline();
        let e = generate(
            &core,
            &json_obj! { "candidate_id" => "nope" },
            &CallContext::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::UNKNOWN_ID);
    }

    #[test]
    fn arrangement_is_deterministic() {
        let (core, candidate_id) = seeded();
        let a = generate(
            &core,
            &json_obj! { "candidate_id" => candidate_id.clone() },
            &CallContext::detached(),
        )
        .unwrap();
        let b = generate(
            &core,
            &json_obj! { "candidate_id" => candidate_id },
            &CallContext::detached(),
        )
        .unwrap();
        assert_eq!(a.to_canonical_string(), b.to_canonical_string());
    }
}
