//! `music.analyze_selection` — stages 1 to 4 of the pipeline over a snapshot.
//!
//! The tool returns a *reading*, not a verdict: ranked key candidates with the
//! evidence behind each one, phrase boundaries with confidences, several
//! non-chord-tone hypotheses where a note is genuinely ambiguous, and a
//! harmonic grid with the rationale for the rhythm it chose. Nothing here
//! collapses that into a single label, because the material often does not
//! support one.
//!
//! The full report is stored and served as `analysis://{id}`; the tool result
//! is a summary sized for a conversation.

use super::{opt_f64, opt_str, profile_of, require_snapshot};
use crate::error::{codes, ToolError};
use crate::server::{CallContext, ServerCore};
use crate::store::SnapshotRecord;
use music_analysis::prelude::*;
use music_domain::prelude::*;
use qjson::{json_obj, Json};

/// Builds [`AnalyzeParams`] from tool arguments.
pub fn params_of(core: &ServerCore, args: &Json) -> Result<AnalyzeParams, ToolError> {
    let profile_id = profile_of(core, args)?;

    let extraction = extraction_of(args)?;

    let tonal_center = match args.get("tonal_center") {
        Some(v @ Json::Obj(_)) => {
            let tonic = v.str_field("tonic").map_err(|e| {
                ToolError::with_details(
                    codes::INVALID_ARGUMENT,
                    format!("tonal_center.tonic: {e}"),
                    json_obj! { "argument" => "tonal_center.tonic" },
                )
            })?;
            let scale_id = v.str_field("scale_id").map_err(|e| {
                ToolError::with_details(
                    codes::INVALID_ARGUMENT,
                    format!("tonal_center.scale_id: {e}"),
                    json_obj! { "argument" => "tonal_center.scale_id" },
                )
            })?;
            if SpelledPitch::parse_class(tonic).is_none() {
                return Err(ToolError::with_details(
                    codes::INVALID_ARGUMENT,
                    format!("{tonic} is not a pitch class, for example C, F# or Bb"),
                    json_obj! { "argument" => "tonal_center.tonic", "value" => tonic },
                ));
            }
            if core.knowledge.kb().scale(scale_id).is_none() {
                return Err(ToolError::with_details(
                    codes::INVALID_ARGUMENT,
                    format!("{scale_id} is not a scale in this knowledge bundle"),
                    json_obj! { "argument" => "tonal_center.scale_id", "value" => scale_id },
                )
                .remedy("read theory://catalog for the scale ids this build knows"));
            }
            Some((tonic.to_string(), scale_id.to_string()))
        }
        _ => None,
    };

    let meter = match opt_str(args, "meter") {
        Some(s) => Some(TimeSignature::parse(s).ok_or_else(|| {
            ToolError::with_details(
                codes::INVALID_ARGUMENT,
                format!("{s} is not a time signature, for example 4/4 or 6/8"),
                json_obj! { "argument" => "meter", "value" => s },
            )
        })?),
        None => None,
    };

    let grid = grid_of(args)?;
    let strictness = match opt_str(args, "strictness") {
        Some(s) => Strictness::parse(s).ok_or_else(|| {
            ToolError::with_details(
                codes::INVALID_ARGUMENT,
                format!("{s} is not an analysis strictness"),
                json_obj! { "argument" => "strictness", "value" => s },
            )
        })?,
        None => Strictness::Balanced,
    };

    let loop_span = loop_span_of(args, "loop_span")?;

    Ok(AnalyzeParams {
        profile_id,
        extraction,
        tonal_center,
        meter,
        grid,
        strictness,
        loop_span,
    })
}

/// Reads a `melody_extraction` argument.
pub fn extraction_of(args: &Json) -> Result<ExtractionRequest, ToolError> {
    let Some(e) = args.get("melody_extraction") else {
        return Ok(ExtractionRequest::auto());
    };
    let mode_id = e.get("mode").and_then(Json::as_str).unwrap_or("auto");
    let mode = ExtractionMode::parse(mode_id).ok_or_else(|| {
        ToolError::with_details(
            codes::INVALID_ARGUMENT,
            format!("{mode_id} is not a melody-extraction mode"),
            json_obj! { "argument" => "melody_extraction.mode", "value" => mode_id },
        )
    })?;
    let channel = e.get("channel").and_then(Json::as_i64);
    if mode == ExtractionMode::MidiChannel && channel.is_none() {
        return Err(ToolError::with_details(
            codes::INVALID_ARGUMENT,
            "melody_extraction.channel is required when the mode is midi_channel",
            json_obj! { "argument" => "melody_extraction.channel" },
        ));
    }
    Ok(ExtractionRequest {
        mode,
        channel: channel.map(|c| c.clamp(0, 15) as u8),
    })
}

/// Reads a `harmonic_rhythm` argument into a [`GridMode`].
pub fn grid_of(args: &Json) -> Result<GridMode, ToolError> {
    let Some(g) = args.get("harmonic_rhythm") else {
        return Ok(GridMode::Auto);
    };
    let mode = g.get("mode").and_then(Json::as_str).unwrap_or("auto");
    let value = g.get("value").and_then(Json::as_f64);
    match mode {
        "auto" => Ok(GridMode::Auto),
        "existing" => Ok(GridMode::Existing),
        "bars" | "beats" => {
            let v = value.filter(|v| v.is_finite() && *v > 0.0).ok_or_else(|| {
                ToolError::with_details(
                    codes::INVALID_ARGUMENT,
                    format!("harmonic_rhythm.value is required and must be positive for {mode}"),
                    json_obj! { "argument" => "harmonic_rhythm.value" },
                )
            })?;
            Ok(if mode == "bars" {
                GridMode::Bars(v)
            } else {
                GridMode::Beats(v)
            })
        }
        other => Err(ToolError::with_details(
            codes::INVALID_ARGUMENT,
            format!("{other} is not a harmonic-rhythm mode"),
            json_obj! { "argument" => "harmonic_rhythm.mode", "value" => other },
        )),
    }
}

/// Reads an optional `{start_qn, end_qn}` argument.
pub fn loop_span_of(args: &Json, key: &str) -> Result<Option<(BeatTime, BeatTime)>, ToolError> {
    let Some(v) = args.get(key) else {
        return Ok(None);
    };
    let (Some(start), Some(end)) = (
        v.get("start_qn").and_then(Json::as_f64),
        v.get("end_qn").and_then(Json::as_f64),
    ) else {
        return Err(ToolError::with_details(
            codes::INVALID_ARGUMENT,
            format!("{key} needs finite start_qn and end_qn"),
            json_obj! { "argument" => key },
        ));
    };
    if !(start.is_finite() && end.is_finite()) || end <= start {
        return Err(ToolError::with_details(
            codes::INVALID_ARGUMENT,
            format!("{key}.end_qn must be greater than {key}.start_qn"),
            json_obj! { "argument" => key, "start_qn" => start, "end_qn" => end },
        ));
    }
    Ok(Some((
        BeatTime::from_f64_grid(start, crate::convert::QN_GRID),
        BeatTime::from_f64_grid(end, crate::convert::QN_GRID),
    )))
}

/// Runs the analysis for a snapshot record, storing the report.
pub fn analyze_record(
    core: &ServerCore,
    record: &SnapshotRecord,
    params: &AnalyzeParams,
    ctx: &CallContext,
) -> Result<Analysis, ToolError> {
    ctx.check_cancelled()?;
    let analysis = analyze(
        core.knowledge.kb(),
        &record.snapshot.snapshot_id,
        &record.notes,
        params,
    )?;
    core.with_store(|s| s.put_analysis(analysis.clone(), ctx.now));
    Ok(analysis)
}

/// `music.analyze_selection`.
pub fn analyze_selection(
    core: &ServerCore,
    args: &Json,
    ctx: &CallContext,
) -> Result<Json, ToolError> {
    let record = require_snapshot(core, args, ctx)?;
    let params = params_of(core, args)?;
    ctx.report(0.2, "analyzing the selection");
    let analysis = analyze_record(core, &record, &params, ctx)?;
    ctx.report(1.0, "analysis stored");
    Ok(summary_body(core, &analysis))
}

/// The tool-sized summary of a full analysis.
pub fn summary_body(core: &ServerCore, a: &Analysis) -> Json {
    let key = json_obj! {
        "ambiguous" => a.key.ambiguous,
        "gap" => a.key.gap,
        "candidates" => Json::Arr(a.key.candidates.iter().map(|c| json_obj! {
            "label" => c.label(),
            "tonic" => c.label().split(' ').next().unwrap_or("").to_string(),
            "scale_id" => c.scale_id.clone(),
            "score" => c.score,
            "confidence" => c.confidence,
            "is_modal" => c.is_modal,
            "evidence" => Json::Arr(c.evidence.iter().map(|(n, v)| json_obj! {
                "name" => *n,
                "value" => *v,
            }).collect()),
        }).collect()),
        "regions" => Json::Arr(a.key.regions.iter().map(|r| json_obj! {
            "start_qn" => r.start.as_f64(),
            "end_qn" => r.end.as_f64(),
            "scale_id" => r.scale_id.clone(),
            "confidence" => r.confidence,
            "is_tonicization" => r.is_tonicization,
            "evidence" => Json::Arr(r.evidence.iter().map(|e| Json::Str(e.clone())).collect()),
        }).collect()),
    };

    let phrases = Json::Arr(
        a.phrases
            .phrases
            .iter()
            .map(|p| {
                json_obj! {
                    "id" => p.id as i64,
                    "start_qn" => p.start.as_f64(),
                    "end_qn" => p.end.as_f64(),
                    "note_count" => p.notes.len() as i64,
                    "cadence" => match p.cadence {
                        Some(c) => Json::Str(c.id().to_string()),
                        None => Json::Null,
                    },
                    "is_pickup" => p.is_pickup,
                    "confidence" => p.confidence,
                }
            })
            .collect(),
    );

    let motives = Json::Arr(
        a.phrases
            .motives
            .iter()
            .map(|m| {
                json_obj! {
                    "id" => m.id as i64,
                    "occurrences" => m.occurrences.len() as i64,
                    "salience" => m.salience,
                    "interval_profile" => Json::Arr(
                        m.interval_profile.iter().map(|i| Json::Int(i64::from(*i))).collect()
                    ),
                    "transforms" => Json::Arr(
                        m.occurrences
                            .iter()
                            .map(|o| Json::Str(format!("{:?}", o.transform).to_lowercase()))
                            .collect()
                    ),
                }
            })
            .collect(),
    );

    let ncts = Json::Arr(
        a.ncts
            .iter()
            .flat_map(|(id, hyps)| {
                hyps.iter().map(move |h| {
                    json_obj! {
                        "note_id" => i64::from(*id),
                        "kind" => h.kind.id(),
                        "confidence" => h.confidence,
                        "rationale" => h.rationale.clone(),
                    }
                })
            })
            .collect(),
    );

    let grid = json_obj! {
        "mode" => a.grid.mode_used.id(),
        "rationale" => a.grid.rationale.clone(),
        "slot_count" => a.grid.slots.len() as i64,
        "slots" => Json::Arr(a.grid.slots.iter().map(|s| json_obj! {
            "start_qn" => s.start.as_f64(),
            "end_qn" => s.end.as_f64(),
            "is_cadential" => s.is_cadential,
            "weight" => s.weight,
            "melody_notes" => Json::Arr(
                s.melody_notes.iter().map(|n| Json::Int(i64::from(*n))).collect()
            ),
        }).collect()),
    };

    let melody = json_obj! {
        "low" => a.melody.low.to_ascii(),
        "high" => a.melody.high.to_ascii(),
        "tessitura_low" => i64::from(a.melody.tessitura.0),
        "tessitura_high" => i64::from(a.melody.tessitura.1),
        "density" => a.melody.density,
        "direction_changes" => a.melody.direction_changes as i64,
        "repeated_pitches" => a.melody.repeated_pitches as i64,
        "mean_interval" => a.melody.mean_interval,
        "climax_note_id" => match a.melody.climax {
            Some(id) => Json::Int(i64::from(id)),
            None => Json::Null,
        },
        "leap_count" => a.melody.leaps.len() as i64,
    };

    let extraction = json_obj! {
        "mode_used" => a.extraction.mode_used.id(),
        "confidence" => a.extraction.confidence,
        "voice_count" => a.extraction.voice_count as i64,
        "melody_note_count" => a.extraction.melody.notes.len() as i64,
        "accompaniment_note_count" => a.extraction.accompaniment.notes.len() as i64,
        "assumptions" => Json::Arr(
            a.extraction.assumptions.iter().map(|s| Json::Str(s.clone())).collect()
        ),
    };

    let mut warnings: Vec<Json> = a.warnings.iter().map(|w| w.to_json()).collect();
    warnings.extend(a.extraction.warnings.iter().map(|w| w.to_json()));

    json_obj! {
        "ok" => true,
        "analysis_id" => a.id.clone(),
        "snapshot_id" => a.snapshot_id.clone(),
        "profile_id" => a.profile_id.clone(),
        "resource_uri" => format!("analysis://{}", a.id),
        "confidence" => a.confidence,
        "knowledge_version" => core.knowledge_version(),
        "key" => key,
        "phrases" => phrases,
        "motives" => motives,
        "structural_notes" => Json::Arr(
            a.salience.structural.iter().map(|n| Json::Int(i64::from(*n))).collect()
        ),
        "nct_hypotheses" => ncts,
        "grid" => grid,
        "melody" => melody,
        "extraction" => extraction,
        "detected_chords" => Json::Arr(
            a.detected_chords.iter().map(|c| Json::Str(c.spec.render_ascii())).collect()
        ),
        "loop_observations" => Json::Arr(a.loop_observations.iter().map(|w| w.to_json()).collect()),
        "warnings" => Json::Arr(warnings),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::snapshot_from_fixture;

    fn core_with_snapshot() -> (std::sync::Arc<ServerCore>, String) {
        let core = ServerCore::offline();
        let record = snapshot_from_fixture("melodies/eight_bar_c_major").expect("fixture");
        let id = core.with_store(|s| s.put_snapshot(record, qjson::time::unix_now()));
        (core, id)
    }

    #[test]
    fn analysis_runs_over_a_fixture_and_validates_against_its_schema() {
        let (core, id) = core_with_snapshot();
        let tool = core.tools.get("music.analyze_selection").unwrap();
        let body = analyze_selection(
            &core,
            &json_obj! { "snapshot_id" => id },
            &CallContext::detached(),
        )
        .unwrap();
        assert!(tool.output.validate(&body).is_empty(), "{body:?}");
        assert!(!body.arr_field("phrases").unwrap().is_empty());
        assert!(!body
            .get("key")
            .unwrap()
            .arr_field("candidates")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn the_key_reading_is_ranked_not_asserted() {
        let (core, id) = core_with_snapshot();
        let body = analyze_selection(
            &core,
            &json_obj! { "snapshot_id" => id },
            &CallContext::detached(),
        )
        .unwrap();
        let candidates = body.get("key").unwrap().arr_field("candidates").unwrap();
        assert!(candidates.len() > 1, "a ranked reading needs alternatives");
        for c in candidates {
            assert!(!c.arr_field("evidence").unwrap().is_empty());
        }
    }

    #[test]
    fn an_unknown_snapshot_id_is_refused() {
        let core = ServerCore::offline();
        let e = analyze_selection(
            &core,
            &json_obj! { "snapshot_id" => "nope" },
            &CallContext::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::UNKNOWN_ID);
    }

    #[test]
    fn an_expired_snapshot_id_is_refused() {
        let (core, id) = core_with_snapshot();
        let ctx = CallContext {
            now: qjson::time::unix_now() + crate::store::SNAPSHOT_TTL_SECONDS + 10,
            ..CallContext::detached()
        };
        let e = analyze_selection(&core, &json_obj! { "snapshot_id" => id }, &ctx).unwrap_err();
        assert_eq!(e.code, codes::EXPIRED_ID);
    }

    #[test]
    fn a_bad_tonal_center_is_refused() {
        let (core, id) = core_with_snapshot();
        for bad in [
            json_obj! { "tonic" => "H", "scale_id" => "major" },
            json_obj! { "tonic" => "C", "scale_id" => "not_a_scale" },
        ] {
            let e = analyze_selection(
                &core,
                &json_obj! { "snapshot_id" => id.clone(), "tonal_center" => bad },
                &CallContext::detached(),
            )
            .unwrap_err();
            assert_eq!(e.code, codes::INVALID_ARGUMENT);
        }
    }

    #[test]
    fn a_valid_tonal_center_hint_is_accepted() {
        let (core, id) = core_with_snapshot();
        let body = analyze_selection(
            &core,
            &json_obj! {
                "snapshot_id" => id,
                "tonal_center" => json_obj! { "tonic" => "C", "scale_id" => "major" },
            },
            &CallContext::detached(),
        )
        .unwrap();
        assert_eq!(body.get("ok"), Some(&Json::Bool(true)));
    }

    #[test]
    fn grid_arguments_are_parsed_and_validated() {
        assert_eq!(grid_of(&json_obj! {}).unwrap(), GridMode::Auto);
        assert_eq!(
            grid_of(&json_obj! { "harmonic_rhythm" => json_obj! { "mode" => "existing" } }).unwrap(),
            GridMode::Existing
        );
        assert_eq!(
            grid_of(
                &json_obj! { "harmonic_rhythm" => json_obj! { "mode" => "bars", "value" => 2.0 } }
            )
            .unwrap(),
            GridMode::Bars(2.0)
        );
        assert!(
            grid_of(&json_obj! { "harmonic_rhythm" => json_obj! { "mode" => "bars" } }).is_err()
        );
        assert!(
            grid_of(&json_obj! { "harmonic_rhythm" => json_obj! { "mode" => "hours" } }).is_err()
        );
    }

    #[test]
    fn a_loop_span_must_be_ordered() {
        assert!(loop_span_of(&json_obj! {}, "loop_span").unwrap().is_none());
        assert!(loop_span_of(
            &json_obj! { "loop_span" => json_obj! { "start_qn" => 0.0, "end_qn" => 32.0 } },
            "loop_span"
        )
        .unwrap()
        .is_some());
        assert!(loop_span_of(
            &json_obj! { "loop_span" => json_obj! { "start_qn" => 8.0, "end_qn" => 4.0 } },
            "loop_span"
        )
        .is_err());
    }

    #[test]
    fn extraction_arguments_are_validated() {
        assert_eq!(
            extraction_of(&json_obj! {}).unwrap().mode,
            ExtractionMode::Auto
        );
        assert!(extraction_of(
            &json_obj! { "melody_extraction" => json_obj! { "mode" => "midi_channel" } }
        )
        .is_err());
        let r = extraction_of(
            &json_obj! { "melody_extraction" => json_obj! { "mode" => "midi_channel", "channel" => 2 } },
        )
        .unwrap();
        assert_eq!(r.channel, Some(2));
    }

    #[test]
    fn the_analysis_is_stored_and_readable_as_a_resource() {
        let (core, id) = core_with_snapshot();
        let body = analyze_selection(
            &core,
            &json_obj! { "snapshot_id" => id },
            &CallContext::detached(),
        )
        .unwrap();
        let uri = body.str_field("resource_uri").unwrap();
        let resource = crate::resources::read(&core, uri).unwrap();
        assert_eq!(
            resource.str_field("id").unwrap(),
            body.str_field("analysis_id").unwrap()
        );
    }

    #[test]
    fn analysis_is_deterministic() {
        let (core, id) = core_with_snapshot();
        let a = analyze_selection(
            &core,
            &json_obj! { "snapshot_id" => id.clone() },
            &CallContext::detached(),
        )
        .unwrap();
        let b = analyze_selection(
            &core,
            &json_obj! { "snapshot_id" => id },
            &CallContext::detached(),
        )
        .unwrap();
        assert_eq!(a.to_canonical_string(), b.to_canonical_string());
    }

    #[test]
    fn a_cancelled_analysis_does_not_store_anything() {
        let (core, id) = core_with_snapshot();
        let ctx = CallContext::detached();
        ctx.cancel.cancel();
        let e = analyze_selection(&core, &json_obj! { "snapshot_id" => id }, &ctx).unwrap_err();
        assert_eq!(e.code, codes::CANCELLED);
        let stats = core.with_store(|s| s.stats());
        assert_eq!(stats.i64_field("analyses"), Ok(0));
    }
}
