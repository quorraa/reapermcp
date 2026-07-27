//! `candidate.explain` — the decision trace behind a candidate.
//!
//! Everything here is read out of the stored [`DecisionTrace`]; nothing is
//! composed. Rule ids are the ids the engine actually applied, and every source
//! id is resolved against the knowledge bundle before it is returned, so a
//! citation in the output always names a real entry in `theory://sources`.

use super::{require_id, str_or};
use crate::error::ToolError;
use crate::server::{CallContext, ServerCore};
use crate::store::CandidateRecord;
use qjson::{json_obj, Json};

/// How much of the trace to return.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Detail {
    /// Score summary, top rules and the headline explanation.
    Concise,
    /// Everything the trace holds.
    Detailed,
}

impl Detail {
    /// The stable identifier.
    pub fn id(self) -> &'static str {
        match self {
            Detail::Concise => "concise",
            Detail::Detailed => "detailed",
        }
    }

    /// Parses the identifier.
    pub fn parse(s: &str) -> Option<Detail> {
        match s {
            "concise" => Some(Detail::Concise),
            "detailed" => Some(Detail::Detailed),
            _ => None,
        }
    }

    /// How many rule applications to include.
    pub fn rule_limit(self) -> usize {
        match self {
            Detail::Concise => 8,
            Detail::Detailed => usize::MAX,
        }
    }
}

/// `candidate.explain`.
pub fn explain(core: &ServerCore, args: &Json, ctx: &CallContext) -> Result<Json, ToolError> {
    let id = require_id(args, "candidate_id")?;
    let record = core.with_store(|s| s.candidate(&id, ctx.now).cloned())?;
    let detail = Detail::parse(&str_or(args, "detail", "detailed")).unwrap_or(Detail::Detailed);
    Ok(body(core, &record, detail))
}

/// The explanation body.
pub fn body(core: &ServerCore, record: &CandidateRecord, detail: Detail) -> Json {
    let kb = core.knowledge.kb();
    let trace = &record.candidate.trace;
    let profile = kb.resolve_profile(&record.profile_id).ok();

    let components: Vec<Json> = trace
        .score
        .components()
        .map(|(name, value)| {
            let weight = profile.as_ref().map(|p| p.weight(name)).unwrap_or(0.0);
            json_obj! {
                "name" => name,
                "value" => value,
                "weight" => weight,
                "weighted" => value * weight,
            }
        })
        .collect();

    let mut rules: Vec<&music_domain::candidate::RuleApplication> =
        trace.rule_applications.iter().collect();
    // Strongest influence first, then by id so the order never depends on a
    // hash iteration.
    rules.sort_by(|a, b| {
        b.score_delta
            .abs()
            .partial_cmp(&a.score_delta.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.rule_id.cmp(&b.rule_id))
    });

    let rules_json: Vec<Json> = rules
        .iter()
        .take(detail.rule_limit())
        .map(|r| {
            let known = kb.rule(&r.rule_id);
            json_obj! {
                "rule_id" => r.rule_id.clone(),
                "status" => r.status.id(),
                "score_delta" => r.score_delta,
                "score_component" => known
                    .map(|k| Json::Str(k.effect.score_component.clone()))
                    .unwrap_or(Json::Null),
                "domain" => known.map(|k| Json::Str(k.domain.id().to_string())).unwrap_or(Json::Null),
                "kind" => known.map(|k| Json::Str(k.kind.id().to_string())).unwrap_or(Json::Null),
                "summary" => known.map(|k| Json::Str(k.summary.clone())).unwrap_or(Json::Null),
                "explanation" => r.explanation.clone(),
                "matched_conditions" => Json::Arr(
                    r.matched_conditions.iter().map(|c| Json::Str(c.clone())).collect()
                ),
                "matched_exceptions" => Json::Arr(
                    r.matched_exceptions.iter().map(|c| Json::Str(c.clone())).collect()
                ),
                "source_ids" => Json::Arr(
                    r.source_ids.iter().map(|c| Json::Str(c.clone())).collect()
                ),
                "resolved" => known.is_some(),
            }
        })
        .collect();

    // Only source ids that resolve are returned: a citation a reader cannot
    // follow is worse than no citation.
    let mut source_ids: Vec<String> = trace.source_ids.clone();
    for r in &trace.rule_applications {
        source_ids.extend(r.source_ids.iter().cloned());
    }
    source_ids.sort();
    source_ids.dedup();
    let sources: Vec<Json> = source_ids
        .iter()
        .filter_map(|id| kb.source(id))
        .map(|s| {
            json_obj! {
                "source_id" => s.id.clone(),
                "title" => s.title.clone(),
                "authors" => Json::Arr(s.authors.iter().map(|a| Json::Str(a.clone())).collect()),
                "publisher" => s.publisher.clone(),
                "url" => s.url.clone(),
                "license" => s.license.clone(),
                "source_type" => s.source_type.clone(),
            }
        })
        .collect();

    let assumptions: Vec<Json> = match detail {
        Detail::Concise => trace
            .assumptions
            .iter()
            .take(5)
            .map(|a| Json::Str(a.clone()))
            .collect(),
        Detail::Detailed => trace
            .assumptions
            .iter()
            .map(|a| Json::Str(a.clone()))
            .collect(),
    };

    json_obj! {
        "ok" => true,
        "candidate_id" => record.candidate.id.clone(),
        "detail" => detail.id(),
        "label" => record.candidate.label.clone(),
        "strategy" => record.candidate.strategy.clone(),
        "snapshot_id" => record.snapshot_id.clone(),
        "analysis_id" => record.analysis_id.clone(),
        "profile_id" => record.profile_id.clone(),
        "seed" => record.seed as i64,
        "knowledge_version" => core.knowledge_version(),
        "confidence" => trace.confidence.clamp(0.0, 1.0),
        "explanation" => trace.explanation.clone(),
        "assumptions" => Json::Arr(assumptions),
        "score" => json_obj! {
            "total" => trace.score.total(),
            "components" => Json::Arr(components),
        },
        "rules" => Json::Arr(rules_json),
        "sources" => Json::Arr(sources),
        "rejected_alternatives" => Json::Arr(
            trace.rejected_alternatives.iter().map(|a| Json::Str(a.clone())).collect()
        ),
        "chords" => Json::Arr(
            record.candidate.chords.iter().map(|c| Json::Str(c.spec.render_ascii())).collect()
        ),
        "trace_uri" => format!("candidate://{}/trace", record.candidate.id),
        "warnings" => Json::Arr(trace.warnings.iter().map(|w| w.to_json()).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::snapshot_from_fixture;
    use std::sync::Arc;

    fn seeded() -> (Arc<ServerCore>, String) {
        let core = ServerCore::offline();
        let record = snapshot_from_fixture("melodies/eight_bar_c_major").unwrap();
        let snapshot_id = core.with_store(|s| s.put_snapshot(record, qjson::time::unix_now()));
        let generated = crate::tools::harmony::generate_candidates(
            &core,
            &json_obj! { "snapshot_id" => snapshot_id, "candidate_count" => 2, "seed" => 31 },
            &CallContext::detached(),
        )
        .unwrap();
        let id = generated.arr_field("candidates").unwrap()[0]
            .str_field("candidate_id")
            .unwrap()
            .to_string();
        (core, id)
    }

    #[test]
    fn explain_validates_against_its_declared_schema() {
        let (core, id) = seeded();
        let tool = core.tools.get("candidate.explain").unwrap();
        for detail in ["concise", "detailed"] {
            let body = explain(
                &core,
                &json_obj! { "candidate_id" => id.clone(), "detail" => detail },
                &CallContext::detached(),
            )
            .unwrap();
            assert!(tool.output.validate(&body).is_empty(), "{detail}: {body:?}");
        }
    }

    #[test]
    fn the_explanation_cites_real_rule_ids() {
        let (core, id) = seeded();
        let body = explain(
            &core,
            &json_obj! { "candidate_id" => id },
            &CallContext::detached(),
        )
        .unwrap();
        let rules = body.arr_field("rules").unwrap();
        assert!(!rules.is_empty(), "a trace must cite rules");
        for r in rules {
            let rule_id = r.str_field("rule_id").unwrap();
            assert!(
                core.knowledge.kb().rule(rule_id).is_some(),
                "{rule_id} does not exist in the bundle"
            );
            assert_eq!(r.get("resolved"), Some(&Json::Bool(true)));
        }
    }

    #[test]
    fn every_returned_source_id_resolves() {
        let (core, id) = seeded();
        let body = explain(
            &core,
            &json_obj! { "candidate_id" => id },
            &CallContext::detached(),
        )
        .unwrap();
        for s in body.arr_field("sources").unwrap() {
            let sid = s.str_field("source_id").unwrap();
            assert!(core.knowledge.kb().source(sid).is_some(), "{sid}");
            assert!(!s.str_field("title").unwrap().is_empty());
        }
    }

    #[test]
    fn the_score_reports_components_and_their_weights() {
        let (core, id) = seeded();
        let body = explain(
            &core,
            &json_obj! { "candidate_id" => id },
            &CallContext::detached(),
        )
        .unwrap();
        let components = body
            .get("score")
            .unwrap()
            .arr_field("components")
            .unwrap()
            .to_vec();
        assert!(!components.is_empty());
        for c in &components {
            assert!(c.get("weight").is_some());
            assert!(c.get("weighted").is_some());
        }
    }

    #[test]
    fn concise_returns_fewer_rules_than_detailed() {
        let (core, id) = seeded();
        let concise = explain(
            &core,
            &json_obj! { "candidate_id" => id.clone(), "detail" => "concise" },
            &CallContext::detached(),
        )
        .unwrap();
        let detailed = explain(
            &core,
            &json_obj! { "candidate_id" => id, "detail" => "detailed" },
            &CallContext::detached(),
        )
        .unwrap();
        assert!(
            concise.arr_field("rules").unwrap().len() <= detailed.arr_field("rules").unwrap().len()
        );
        assert!(concise.arr_field("rules").unwrap().len() <= Detail::Concise.rule_limit());
    }

    #[test]
    fn an_unknown_candidate_is_refused() {
        let core = ServerCore::offline();
        let e = explain(
            &core,
            &json_obj! { "candidate_id" => "nope" },
            &CallContext::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, crate::error::codes::UNKNOWN_ID);
    }

    #[test]
    fn an_expired_candidate_is_refused() {
        let (core, id) = seeded();
        let ctx = CallContext {
            now: qjson::time::unix_now() + crate::store::CANDIDATE_TTL_SECONDS + 5,
            ..CallContext::detached()
        };
        let e = explain(&core, &json_obj! { "candidate_id" => id }, &ctx).unwrap_err();
        assert_eq!(e.code, crate::error::codes::EXPIRED_ID);
    }

    #[test]
    fn detail_round_trips() {
        for d in [Detail::Concise, Detail::Detailed] {
            assert_eq!(Detail::parse(d.id()), Some(d));
        }
        assert_eq!(Detail::parse("verbose"), None);
    }

    #[test]
    fn rules_are_ordered_by_influence_deterministically() {
        let (core, id) = seeded();
        let a = explain(
            &core,
            &json_obj! { "candidate_id" => id.clone() },
            &CallContext::detached(),
        )
        .unwrap();
        let b = explain(
            &core,
            &json_obj! { "candidate_id" => id },
            &CallContext::detached(),
        )
        .unwrap();
        assert_eq!(a.to_canonical_string(), b.to_canonical_string());
        let deltas: Vec<f64> = a
            .arr_field("rules")
            .unwrap()
            .iter()
            .map(|r| r.f64_field("score_delta").unwrap().abs())
            .collect();
        for w in deltas.windows(2) {
            assert!(w[0] >= w[1] - 1e-9, "{deltas:?}");
        }
    }
}
