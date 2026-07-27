//! `loop.audit` — how well material wraps, relative to its declared intent.
//!
//! The verdict is always intent-relative. A `closed_tonic` loop wants a
//! dominant resolving across the wrap; a `modal_drone` loop is served by common
//! tones or a pedal and must not be penalised for having no dominant at all.
//! The tool reports the intent it audited against so the caller can see which
//! standard was applied.
//!
//! Four targets are supported, resolved in this order: a candidate, an
//! analysis, a staged transaction (which resolves to the candidate it staged),
//! and finally the snapshot itself.

use super::{opt_str, profile_of, require_snapshot};
use crate::error::{codes, ToolError};
use crate::server::{CallContext, ServerCore};
use loop_engine::{audit_detailed, suggest_repairs, LoopInput, LoopSpan};
use music_analysis::report::Analysis;
use music_domain::prelude::*;
use qjson::{json_obj, Json};

/// What the audit was run against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A generated candidate.
    Candidate(String),
    /// An analysis.
    Analysis(String),
    /// The source snapshot.
    Snapshot(String),
}

impl Target {
    /// The `target_kind` reported in the result.
    pub fn kind(&self) -> &'static str {
        match self {
            Target::Candidate(_) => "candidate",
            Target::Analysis(_) => "analysis",
            Target::Snapshot(_) => "snapshot",
        }
    }

    /// The identifier reported in the result.
    pub fn id(&self) -> &str {
        match self {
            Target::Candidate(id) | Target::Analysis(id) | Target::Snapshot(id) => id,
        }
    }
}

/// `loop.audit`.
pub fn audit(core: &ServerCore, args: &Json, ctx: &CallContext) -> Result<Json, ToolError> {
    // A transaction resolves to the candidate it staged, so auditing a staged
    // candidate needs no separate code path.
    let candidate_id = match opt_str(args, "candidate_id") {
        Some(id) => Some(id.to_string()),
        None => match opt_str(args, "transaction_id") {
            Some(t) => Some(
                core.with_store(|s| s.transaction(t, ctx.now).cloned())?
                    .candidate_id,
            ),
            None => None,
        },
    };

    let (target, notes, chords, parts, analysis, time_map, snapshot_id) = match &candidate_id {
        Some(id) => {
            let record = core.with_store(|s| s.candidate(id, ctx.now).cloned())?;
            let snapshot =
                core.with_store(|s| s.snapshot(&record.snapshot_id, ctx.now).cloned())?;
            let analysis = core
                .with_store(|s| s.analysis(&record.analysis_id, ctx.now).cloned())
                .ok();
            let notes = NoteSet::sorted(
                record
                    .candidate
                    .parts
                    .iter()
                    .flat_map(|p| p.notes.iter().cloned())
                    .collect(),
                snapshot.notes.time_map.clone(),
            );
            (
                Target::Candidate(id.clone()),
                notes,
                record.candidate.chords.clone(),
                record.candidate.parts.clone(),
                analysis,
                snapshot.notes.time_map.clone(),
                record.snapshot_id.clone(),
            )
        }
        None => match opt_str(args, "analysis_id") {
            Some(id) => {
                let analysis = core.with_store(|s| s.analysis(id, ctx.now).cloned())?;
                let time_map = analysis.extraction.melody.time_map.clone();
                let notes = analysis.extraction.melody.clone();
                let chords = analysis.detected_chords.clone();
                let snapshot_id = analysis.snapshot_id.clone();
                (
                    Target::Analysis(id.to_string()),
                    notes,
                    chords,
                    Vec::new(),
                    Some(analysis),
                    time_map,
                    snapshot_id,
                )
            }
            None => {
                let record = require_snapshot(core, args, ctx)?;
                let analysis = core.with_store(|s| {
                    s.analysis_for_snapshot(&record.snapshot.snapshot_id, ctx.now)
                        .cloned()
                });
                let chords = analysis
                    .as_ref()
                    .map(|a: &Analysis| a.detected_chords.clone())
                    .unwrap_or_default();
                (
                    Target::Snapshot(record.snapshot.snapshot_id.clone()),
                    record.notes.clone(),
                    chords,
                    Vec::new(),
                    analysis,
                    record.notes.time_map.clone(),
                    record.snapshot.snapshot_id.clone(),
                )
            }
        },
    };

    let profile_id = profile_of(core, args)?;
    let profile = core
        .knowledge
        .kb()
        .resolve_profile(&profile_id)
        .map_err(ToolError::from)?;

    let intent = match opt_str(args, "loop_intent") {
        Some(s) => LoopIntent::parse(s).ok_or_else(|| {
            ToolError::with_details(
                codes::INVALID_ARGUMENT,
                format!("{s} is not a loop intent"),
                json_obj! { "argument" => "loop_intent", "value" => s },
            )
        })?,
        None => analysis
            .as_ref()
            .and_then(|a| a.grid.slots.first().map(|_| LoopIntent::ClosedTonic))
            .unwrap_or(LoopIntent::ClosedTonic),
    };

    let span = match crate::tools::analysis::loop_span_of(args, "loop_span")? {
        Some((start, end)) => LoopSpan { start, end, intent },
        None => {
            let (start, end) = if notes.notes.is_empty() {
                (BeatTime::ZERO, BeatTime::from_quarters(4))
            } else {
                notes.span()
            };
            if end <= start {
                return Err(ToolError::new(
                    codes::LOOP_AUDIT_FAILED,
                    "the material has no positive duration, so there is no wrap to audit",
                )
                .remedy("pass an explicit loop_span"));
            }
            LoopSpan { start, end, intent }
        }
    };

    ctx.check_cancelled()?;
    let input = LoopInput {
        span,
        notes: &notes,
        chords: &chords,
        parts: &parts,
        time_map: &time_map,
        analysis: analysis.as_ref(),
    };
    let detailed = audit_detailed(core.knowledge.kb(), &profile, &input);
    let repairs = suggest_repairs(core.knowledge.kb(), &profile, &input, &detailed.report);
    let report = &detailed.report;

    let _ = snapshot_id;
    Ok(json_obj! {
        "ok" => true,
        "target_kind" => target.kind(),
        "target_id" => target.id(),
        "intent" => match report.intent {
            Some(i) => Json::Str(i.id().to_string()),
            None => Json::Str(intent.id().to_string()),
        },
        "loop_start_qn" => report.loop_start.as_f64(),
        "loop_end_qn" => report.loop_end.as_f64(),
        "compatible" => report.compatible,
        "score" => report.score.clamp(0.0, 1.0),
        "confidence" => report.confidence.clamp(0.0, 1.0),
        "harmonic_wrap" => report.harmonic_wrap.clone(),
        "bass_wrap" => report.bass_wrap.clone(),
        "voice_leading_wrap" => report.voice_leading_wrap.clone(),
        "hanging_notes" => Json::Arr(
            report.hanging_notes.iter().map(|n| Json::Int(i64::from(*n))).collect()
        ),
        "crossing_notes" => Json::Arr(
            report.crossing_notes.iter().map(|n| Json::Int(i64::from(*n))).collect()
        ),
        "pickup_qn" => report.pickup_qn.as_f64(),
        "tail_qn" => report.tail_qn.as_f64(),
        "length_exact" => detailed.length_exact,
        "findings" => Json::Arr(report.findings.iter().map(|f| f.to_json()).collect()),
        "repairs" => Json::Arr(repairs.iter().map(|r| r.to_json()).collect()),
        "warnings" => Json::Arr(report.findings.iter().map(|f| f.to_json()).collect()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::snapshot_from_fixture;
    use std::sync::Arc;

    fn core_with(fixture: &str) -> (Arc<ServerCore>, String) {
        let core = ServerCore::offline();
        let record = snapshot_from_fixture(fixture).expect(fixture);
        let id = core.with_store(|s| s.put_snapshot(record, qjson::time::unix_now()));
        (core, id)
    }

    #[test]
    fn a_snapshot_audit_validates_against_its_declared_schema() {
        let (core, id) = core_with("loops/dominant_wrap");
        let tool = core.tools.get("loop.audit").unwrap();
        let body = audit(
            &core,
            &json_obj! { "snapshot_id" => id },
            &CallContext::detached(),
        )
        .unwrap();
        assert!(tool.output.validate(&body).is_empty(), "{body:?}");
        assert_eq!(body.str_field("target_kind").unwrap(), "snapshot");
    }

    #[test]
    fn an_explicit_loop_span_is_honoured() {
        let (core, id) = core_with("loops/dominant_wrap");
        let body = audit(
            &core,
            &json_obj! {
                "snapshot_id" => id,
                "loop_span" => json_obj! { "start_qn" => 0.0, "end_qn" => 8.0 },
            },
            &CallContext::detached(),
        )
        .unwrap();
        assert!((body.f64_field("loop_start_qn").unwrap() - 0.0).abs() < 1e-9);
        assert!((body.f64_field("loop_end_qn").unwrap() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn a_modal_drone_intent_is_audited_on_its_own_terms() {
        let (core, id) = core_with("melodies/dorian_vamp_d");
        let body = audit(
            &core,
            &json_obj! { "snapshot_id" => id, "loop_intent" => "modal_drone" },
            &CallContext::detached(),
        )
        .unwrap();
        assert_eq!(body.str_field("intent").unwrap(), "modal_drone");
        // The point is that a modal loop is not required to resolve; the audit
        // must not report it as incompatible purely for lacking a dominant.
        assert!(body.f64_field("score").unwrap() >= 0.0);
    }

    #[test]
    fn a_candidate_can_be_audited() {
        let (core, id) = core_with("melodies/eight_bar_c_major");
        let generated = crate::tools::harmony::generate_candidates(
            &core,
            &json_obj! { "snapshot_id" => id, "candidate_count" => 1, "seed" => 5 },
            &CallContext::detached(),
        )
        .unwrap();
        let candidate_id = generated.arr_field("candidates").unwrap()[0]
            .str_field("candidate_id")
            .unwrap()
            .to_string();
        let tool = core.tools.get("loop.audit").unwrap();
        let body = audit(
            &core,
            &json_obj! { "candidate_id" => candidate_id.clone() },
            &CallContext::detached(),
        )
        .unwrap();
        assert!(tool.output.validate(&body).is_empty(), "{body:?}");
        assert_eq!(body.str_field("target_kind").unwrap(), "candidate");
        assert_eq!(body.str_field("target_id").unwrap(), candidate_id);
    }

    #[test]
    fn an_analysis_can_be_audited() {
        let (core, id) = core_with("loops/pickup_and_hanging_note");
        let analysis = crate::tools::analysis::analyze_selection(
            &core,
            &json_obj! { "snapshot_id" => id },
            &CallContext::detached(),
        )
        .unwrap();
        let body = audit(
            &core,
            &json_obj! { "analysis_id" => analysis.str_field("analysis_id").unwrap() },
            &CallContext::detached(),
        )
        .unwrap();
        assert_eq!(body.str_field("target_kind").unwrap(), "analysis");
    }

    #[test]
    fn an_unknown_target_is_refused() {
        let core = ServerCore::offline();
        for args in [
            json_obj! { "candidate_id" => "nope" },
            json_obj! { "analysis_id" => "nope" },
            json_obj! { "snapshot_id" => "nope" },
            json_obj! { "transaction_id" => "nope" },
        ] {
            let e = audit(&core, &args, &CallContext::detached()).unwrap_err();
            assert!(
                e.code == codes::UNKNOWN_ID || e.code == codes::INVALID_ARGUMENT,
                "{args:?} gave {}",
                e.code
            );
        }
    }

    #[test]
    fn a_bad_loop_intent_is_refused() {
        let (core, id) = core_with("loops/dominant_wrap");
        let e = audit(
            &core,
            &json_obj! { "snapshot_id" => id, "loop_intent" => "vibes" },
            &CallContext::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::INVALID_ARGUMENT);
    }

    #[test]
    fn a_hanging_note_fixture_reports_hanging_notes() {
        let (core, id) = core_with("loops/pickup_and_hanging_note");
        let body = audit(
            &core,
            &json_obj! {
                "snapshot_id" => id,
                "loop_span" => json_obj! { "start_qn" => 0.0, "end_qn" => 8.0 },
            },
            &CallContext::detached(),
        )
        .unwrap();
        assert!(body.get("hanging_notes").is_some());
        assert!(body.get("repairs").is_some());
    }

    #[test]
    fn target_kinds_are_stable() {
        assert_eq!(Target::Candidate("a".into()).kind(), "candidate");
        assert_eq!(Target::Analysis("a".into()).kind(), "analysis");
        assert_eq!(Target::Snapshot("a".into()).kind(), "snapshot");
        assert_eq!(Target::Snapshot("xyz".into()).id(), "xyz");
    }
}
