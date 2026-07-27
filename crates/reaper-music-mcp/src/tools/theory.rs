//! `theory.search` — discovery over the embedded knowledge bundle.
//!
//! This tool is for explanation, not generation. It never touches REAPER, never
//! reads the session store and never returns anything that is not in the
//! bundle, so it works identically with or without a bridge.

use super::{i64_or, opt_str};
use crate::error::ToolError;
use crate::server::ServerCore;
use qjson::{json_obj, Json};
use theory_kb::SearchQuery;

/// The default number of hits returned when the caller does not ask.
pub const DEFAULT_MAX_RESULTS: i64 = 10;

/// `theory.search`.
pub fn search(core: &ServerCore, args: &Json) -> Result<Json, ToolError> {
    let kb = core.knowledge.kb();
    let text = opt_str(args, "query").unwrap_or("").trim().to_string();
    let max_results = i64_or(args, "max_results", DEFAULT_MAX_RESULTS).clamp(1, 100);

    // `SearchQuery::from_json` reads `text`; the MCP argument is called
    // `query`, which reads better in a tool schema. Translate here rather than
    // renaming a frozen API.
    let mut query_json = match args.clone() {
        Json::Obj(m) => m,
        _ => qjson::JsonMap::new(),
    };
    query_json.insert("text", Json::Str(text.clone()));
    query_json.insert("max_results", Json::Int(max_results));
    let query = SearchQuery::from_json(&Json::Obj(query_json));

    let hits = kb.search(&query);

    let mut source_ids: Vec<String> = Vec::new();
    for h in &hits {
        for s in &h.source_ids {
            if !source_ids.contains(s) {
                source_ids.push(s.clone());
            }
        }
    }
    source_ids.sort();

    let mut profiles: Vec<String> = Vec::new();
    for h in &hits {
        if h.kind == "profile" && !profiles.contains(&h.id) {
            profiles.push(h.id.clone());
        }
    }

    Ok(json_obj! {
        "ok" => true,
        "query" => text,
        "knowledge_version" => core.knowledge_version(),
        "knowledge_hash" => core.knowledge_hash(),
        "result_count" => hits.len() as i64,
        "hits" => Json::Arr(hits.iter().map(|h| json_obj! {
            "kind" => h.kind,
            "id" => h.id.clone(),
            "title" => h.title.clone(),
            "summary" => h.summary.clone(),
            "score" => h.score,
            "source_ids" => Json::Arr(h.source_ids.iter().map(|s| Json::Str(s.clone())).collect()),
            "detail" => match h.detail.clone() {
                v @ Json::Obj(_) => v,
                other => json_obj! { "value" => other },
            },
        }).collect()),
        "source_ids" => Json::Arr(source_ids.into_iter().map(Json::Str).collect()),
        "profiles" => Json::Arr(profiles.into_iter().map(Json::Str).collect()),
        "warnings" => super::no_warnings(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{CallContext, ServerCore};

    fn run(args: Json) -> Json {
        let core = ServerCore::offline();
        let r = crate::tools::call(&core, "theory.search", &args, &CallContext::detached());
        assert_eq!(r.get("isError"), Some(&Json::Bool(false)), "{r:?}");
        r.get("structuredContent").unwrap().clone()
    }

    #[test]
    fn a_search_returns_hits_with_sources() {
        let body = run(json_obj! { "query" => "tritone substitution" });
        assert!(body.i64_field("result_count").unwrap() > 0);
        let hits = body.arr_field("hits").unwrap();
        assert!(hits.iter().any(|h| !h
            .arr_field("source_ids")
            .unwrap()
            .is_empty()));
    }

    #[test]
    fn search_validates_against_its_declared_schema() {
        let core = ServerCore::offline();
        let tool = core.tools.get("theory.search").unwrap();
        for q in ["voice leading", "parallel fifths", "dorian", "zzzz-nothing"] {
            let body = search(&core, &json_obj! { "query" => q }).unwrap();
            assert!(tool.output.validate(&body).is_empty(), "{q}");
        }
    }

    #[test]
    fn max_results_is_honoured_and_clamped() {
        let body = run(json_obj! { "query" => "chord", "max_results" => 3 });
        assert!(body.arr_field("hits").unwrap().len() <= 3);
    }

    #[test]
    fn a_domain_filter_narrows_the_result() {
        let core = ServerCore::offline();
        let body = search(
            &core,
            &json_obj! {
                "query" => "motion",
                "domains" => Json::Arr(vec![Json::Str("voice_leading".into())]),
                "max_results" => 50,
            },
        )
        .unwrap();
        for hit in body.arr_field("hits").unwrap() {
            if hit.str_field("kind") == Ok("rule") {
                let id = hit.str_field("id").unwrap();
                let rule = core.knowledge.kb().rule(id).expect(id);
                assert_eq!(rule.domain, theory_kb::RuleDomain::VoiceLeading);
            }
        }
    }

    #[test]
    fn a_search_with_no_matches_is_not_an_error() {
        let body = run(json_obj! { "query" => "qqzzxx-not-a-concept" });
        assert_eq!(body.get("ok"), Some(&Json::Bool(true)));
    }

    #[test]
    fn the_knowledge_version_and_hash_are_reported() {
        let body = run(json_obj! { "query" => "cadence" });
        assert!(!body.str_field("knowledge_version").unwrap().is_empty());
        assert_eq!(body.str_field("knowledge_hash").unwrap().len(), 64);
    }

    #[test]
    fn results_are_deterministic() {
        let a = run(json_obj! { "query" => "dominant" });
        let b = run(json_obj! { "query" => "dominant" });
        assert_eq!(a.to_canonical_string(), b.to_canonical_string());
    }

    #[test]
    fn an_empty_query_is_rejected_by_the_schema() {
        let core = ServerCore::offline();
        let r = crate::tools::call(
            &core,
            "theory.search",
            &json_obj! { "query" => "" },
            &CallContext::detached(),
        );
        assert_eq!(r.get("isError"), Some(&Json::Bool(true)));
    }
}
