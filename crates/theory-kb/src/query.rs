//! `theory.search`: deterministic retrieval across every knowledge collection.
//!
//! Scoring is pure token overlap with two boosts, computed over an ordered
//! vector of candidate documents. There is no randomness, no hashing whose
//! order could reach the output, and ties break by id, so the same query
//! against the same bundle always returns the same list in the same order.

use crate::load::KnowledgeBase;
use crate::model::{RuleDomain, RuleKind};
use qjson::{Json, JsonMap};

/// A search request.
#[derive(Clone, Debug, Default)]
pub struct SearchQuery {
    /// Free text. Tokenised on any non-alphanumeric character.
    pub text: String,
    /// Restrict to rules in these domains. Empty means no restriction.
    pub domains: Vec<RuleDomain>,
    /// Restrict to records active in this style profile.
    pub profile: Option<String>,
    /// Restrict to rules of these kinds. Empty means no restriction.
    pub kinds: Vec<RuleKind>,
    /// Restrict to records citing any of these source ids.
    pub sources: Vec<String>,
    /// Maximum hits to return. `0` means the default of 20.
    pub max_results: usize,
}

impl SearchQuery {
    /// A plain text query with default filters.
    pub fn text(text: &str) -> SearchQuery {
        SearchQuery {
            text: text.to_string(),
            ..SearchQuery::default()
        }
    }

    /// Reads the JSON form used by the MCP tool.
    pub fn from_json(v: &Json) -> SearchQuery {
        SearchQuery {
            text: v
                .get("text")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string(),
            domains: str_list(v, "domains")
                .iter()
                .filter_map(|d| RuleDomain::parse(d))
                .collect(),
            profile: v.get("profile").and_then(Json::as_str).map(str::to_string),
            kinds: str_list(v, "kinds")
                .iter()
                .filter_map(|k| RuleKind::parse(k))
                .collect(),
            sources: str_list(v, "sources"),
            max_results: v
                .get("max_results")
                .and_then(Json::as_i64)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(0),
        }
    }
}

/// Reads an optional array-of-strings field.
fn str_list(v: &Json, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Json::as_arr)
        .map(|a| {
            a.iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One search result.
#[derive(Clone, Debug)]
pub struct SearchHit {
    /// Which collection the record came from: `"rule"`, `"scale"`, …
    pub kind: &'static str,
    /// The record id.
    pub id: String,
    /// A short display title.
    pub title: String,
    /// One line describing the record.
    pub summary: String,
    /// The relevance score. Higher is better.
    pub score: f64,
    /// Source ids backing the record.
    pub source_ids: Vec<String>,
    /// The full record, for a client that wants more than the summary.
    pub detail: Json,
}

impl SearchHit {
    /// JSON form for the MCP tool result.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("kind", Json::Str(self.kind.to_string()));
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("title", Json::Str(self.title.clone()));
        m.insert("summary", Json::Str(self.summary.clone()));
        m.insert("score", Json::Float(self.score));
        m.insert(
            "source_ids",
            Json::Arr(
                self.source_ids
                    .iter()
                    .map(|s| Json::Str(s.clone()))
                    .collect(),
            ),
        );
        m.insert("detail", self.detail.clone());
        Json::Obj(m)
    }
}

/// The default result cap when the query does not set one.
pub const DEFAULT_MAX_RESULTS: usize = 20;

/// Weight of a token match in the id field.
const W_ID: f64 = 3.0;
/// Weight of a token match in the title or name field.
const W_TITLE: f64 = 2.0;
/// Weight of a token match in the summary field.
const W_SUMMARY: f64 = 2.0;
/// Weight of a token match in the rationale or long-form field.
const W_BODY: f64 = 1.0;
/// Weight of a token match in tags, aliases and other keyword lists.
const W_TAGS: f64 = 1.5;
/// Bonus when the whole query is exactly the record id.
const BOOST_EXACT_ID: f64 = 100.0;
/// Bonus when the whole query appears verbatim in any searched field.
const BOOST_PHRASE: f64 = 10.0;

/// One searchable document: the record reduced to weighted text.
struct Doc<'a> {
    kind: &'static str,
    id: &'a str,
    title: String,
    summary: String,
    body: String,
    tags: String,
    source_ids: Vec<String>,
    /// Style profiles this record is active in; empty means "all".
    profiles: Vec<String>,
    domain: Option<RuleDomain>,
    rule_kind: Option<RuleKind>,
    detail: Json,
}

/// Lower-cases and splits on any non-alphanumeric character.
fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// True when `haystack` contains `needle` as a whole token sequence.
fn contains_ci(haystack: &str, needle: &str) -> bool {
    !needle.is_empty() && haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Counts weighted token hits of `tokens` in one field.
fn field_score(field: &str, tokens: &[String], weight: f64) -> f64 {
    if tokens.is_empty() || field.is_empty() {
        return 0.0;
    }
    let field_tokens = tokenize(field);
    let mut score = 0.0;
    for t in tokens {
        if field_tokens.iter().any(|f| f == t) {
            score += weight;
        } else if t.len() >= 4 && field_tokens.iter().any(|f| f.contains(t.as_str())) {
            // Partial credit for a stem match, so "dominant" finds
            // "dominant7_flat9" without a stemmer.
            score += weight * 0.5;
        }
    }
    score
}

impl KnowledgeBase {
    /// Deterministic search across every collection.
    ///
    /// Ordering is by descending score then ascending id, so equal-scoring
    /// records always come back in the same order.
    pub fn search(&self, q: &SearchQuery) -> Vec<SearchHit> {
        let tokens = tokenize(&q.text);
        let phrase = q.text.trim();
        let limit = if q.max_results == 0 {
            DEFAULT_MAX_RESULTS
        } else {
            q.max_results
        };

        let mut hits: Vec<SearchHit> = Vec::new();
        for doc in self.documents() {
            if !passes_filters(&doc, q) {
                continue;
            }
            let mut score = field_score(doc.id, &tokens, W_ID)
                + field_score(&doc.title, &tokens, W_TITLE)
                + field_score(&doc.summary, &tokens, W_SUMMARY)
                + field_score(&doc.body, &tokens, W_BODY)
                + field_score(&doc.tags, &tokens, W_TAGS);

            if !phrase.is_empty() {
                if doc.id.eq_ignore_ascii_case(phrase) {
                    score += BOOST_EXACT_ID;
                } else if contains_ci(doc.id, phrase)
                    || contains_ci(&doc.title, phrase)
                    || contains_ci(&doc.summary, phrase)
                    || contains_ci(&doc.body, phrase)
                {
                    score += BOOST_PHRASE;
                }
            }

            // A filtered browse with no text is a legitimate query: everything
            // that survives the filters is returned, ordered by id.
            if score <= 0.0 && !tokens.is_empty() {
                continue;
            }

            hits.push(SearchHit {
                kind: doc.kind,
                id: doc.id.to_string(),
                title: doc.title.clone(),
                summary: doc.summary.clone(),
                score,
                source_ids: doc.source_ids.clone(),
                detail: doc.detail.clone(),
            });
        }

        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
                .then_with(|| a.kind.cmp(b.kind))
        });
        hits.truncate(limit);
        hits
    }

    /// Builds the searchable view of every collection, in a fixed order.
    fn documents(&self) -> Vec<Doc<'_>> {
        let mut docs: Vec<Doc<'_>> = Vec::new();

        for r in self.rules() {
            docs.push(Doc {
                kind: "rule",
                id: &r.id,
                title: r.summary.clone(),
                summary: r.summary.clone(),
                body: r.rationale.clone(),
                tags: format!(
                    "{} {} {} {} {}",
                    r.domain.id(),
                    r.kind.id(),
                    r.trigger.event.id(),
                    r.effect.score_component,
                    r.conditions.join(" ")
                ),
                source_ids: r.source_ids(),
                profiles: r.profiles.clone(),
                domain: Some(r.domain),
                rule_kind: Some(r.kind),
                detail: r.to_json(),
            });
        }
        for s in self.sources() {
            docs.push(Doc {
                kind: "source",
                id: &s.id,
                title: s.title.clone(),
                summary: format!("{} — {}", s.publisher, s.source_type),
                body: s.notes.clone(),
                tags: s
                    .relevant_sections
                    .iter()
                    .map(|r| format!("{} {}", r.locator, r.topics.join(" ")))
                    .collect::<Vec<_>>()
                    .join(" "),
                source_ids: vec![s.id.clone()],
                profiles: Vec::new(),
                domain: None,
                rule_kind: None,
                detail: s.to_json(),
            });
        }
        for s in self.scales() {
            docs.push(Doc {
                kind: "scale",
                id: &s.id,
                title: s.name.clone(),
                summary: format!("{} collection: {}", s.family, s.degree_spelling.join(" ")),
                body: s.characteristic_degrees.join(" "),
                tags: format!("{} {}", s.aliases.join(" "), s.family),
                source_ids: s.source_refs.clone(),
                profiles: Vec::new(),
                domain: None,
                rule_kind: None,
                detail: s.to_json(),
            });
        }
        for q in self.chord_qualities() {
            docs.push(Doc {
                kind: "chord_quality",
                id: &q.id,
                title: q.name.clone(),
                summary: format!("{} chord: {}", q.family, q.degrees.join(" ")),
                body: q.notes.clone(),
                tags: format!(
                    "{} {} {}",
                    q.family,
                    q.symbol_examples.join(" "),
                    q.typical_function
                ),
                source_ids: q.source_refs.clone(),
                profiles: Vec::new(),
                domain: None,
                rule_kind: None,
                detail: q.to_json(),
            });
        }
        for v in self.voicing_templates() {
            docs.push(Doc {
                kind: "voicing_template",
                id: &v.id,
                title: v.name.clone(),
                summary: format!("{} voicing in {} voices", v.family, v.voice_count),
                body: v.notes.clone(),
                tags: format!("{} {}", v.family, v.degree_layout.join(" ")),
                source_ids: v.source_refs.clone(),
                profiles: v.style_profiles.clone(),
                domain: None,
                rule_kind: None,
                detail: v.to_json(),
            });
        }
        for p in self.progressions() {
            docs.push(Doc {
                kind: "progression",
                id: &p.id,
                title: p.name.clone(),
                summary: p.degrees.join(" - "),
                body: format!("{} {}", p.voice_leading_notes, p.bass_implications),
                tags: format!("{} {}", p.tags.join(" "), p.function_path.join(" ")),
                source_ids: p.source_refs.clone(),
                profiles: p.style_profiles.clone(),
                domain: None,
                rule_kind: None,
                detail: p.to_json(),
            });
        }
        for c in self.cadences() {
            docs.push(Doc {
                kind: "cadence",
                id: &c.schema.id,
                title: c.schema.name.clone(),
                summary: c.schema.degrees.join(" - "),
                body: format!(
                    "{} {}",
                    c.schema.voice_leading_notes, c.schema.bass_implications
                ),
                tags: format!("{} {}", c.cadence_kind, c.schema.tags.join(" ")),
                source_ids: c.schema.source_refs.clone(),
                profiles: c.schema.style_profiles.clone(),
                domain: None,
                rule_kind: None,
                detail: c.to_json(),
            });
        }
        for a in self.arrangement_patterns() {
            docs.push(Doc {
                kind: "arrangement_pattern",
                id: &a.id,
                title: a.name.clone(),
                summary: format!("{} part, {} register", a.role, a.register),
                body: a.notes.clone(),
                tags: format!("{} {} {}", a.role, a.rhythmic_activity, a.loop_behavior),
                source_ids: a.source_refs.clone(),
                profiles: a.style_profiles.clone(),
                domain: None,
                rule_kind: None,
                detail: a.to_json(),
            });
        }
        for p in self.instrument_profiles() {
            docs.push(Doc {
                kind: "instrument_profile",
                id: &p.id,
                title: p.name.clone(),
                summary: format!(
                    "MIDI {}..{}, polyphony {}",
                    p.range.low_midi, p.range.high_midi, p.polyphony
                ),
                body: p.notes.clone(),
                tags: p.typical_roles.join(" "),
                source_ids: p.source_refs.clone(),
                profiles: Vec::new(),
                domain: None,
                rule_kind: None,
                detail: p.to_json(),
            });
        }
        for f in self.functions() {
            docs.push(Doc {
                kind: "function",
                id: &f.id,
                title: format!("{} ({})", f.roman, f.key_context),
                summary: format!("{} function, {} category", f.function_class, f.category),
                body: f.notes.clone(),
                tags: format!("{} {}", f.function_class, f.typical_resolutions.join(" ")),
                source_ids: f.source_refs.clone(),
                profiles: Vec::new(),
                domain: None,
                rule_kind: None,
                detail: f.to_json(),
            });
        }
        for m in self.modes() {
            docs.push(Doc {
                kind: "mode",
                id: &m.id,
                title: m.scale_id.clone(),
                summary: m.character.clone(),
                body: format!("{} {}", m.typical_cadential_gesture, m.notes),
                tags: format!("{} {}", m.mode_family, m.characteristic_degree),
                source_ids: m.source_refs.clone(),
                profiles: Vec::new(),
                domain: None,
                rule_kind: None,
                detail: m.to_json(),
            });
        }
        for i in self.intervals() {
            docs.push(Doc {
                kind: "interval",
                id: &i.id,
                title: i.name.clone(),
                summary: format!("{} — {} semitones", i.short_name, i.semitones),
                body: i.typical_treatment.clone(),
                tags: format!("{} {}", i.quality, i.consonance_class),
                source_ids: i.source_refs.clone(),
                profiles: Vec::new(),
                domain: None,
                rule_kind: None,
                detail: i.to_json(),
            });
        }
        for p in self.profiles() {
            docs.push(Doc {
                kind: "profile",
                id: &p.id,
                title: p.name.clone(),
                summary: p.summary.clone(),
                body: p.notes.clone(),
                tags: String::new(),
                source_ids: Vec::new(),
                profiles: vec![p.id.clone()],
                domain: None,
                rule_kind: None,
                detail: p.to_json(),
            });
        }
        for s in self.chord_symbols() {
            docs.push(Doc {
                kind: "chord_symbol",
                id: &s.id,
                title: s.written.clone(),
                summary: format!("{} token rendering as {}", s.token_kind, s.canonical_ascii),
                body: s.notes.clone(),
                tags: format!(
                    "{} {} {}",
                    s.ascii_variants.join(" "),
                    s.unicode_variants.join(" "),
                    s.quality_id.clone().unwrap_or_default()
                ),
                source_ids: Vec::new(),
                profiles: Vec::new(),
                domain: None,
                rule_kind: None,
                detail: s.to_json(),
            });
        }
        docs
    }
}

/// Applies the non-textual filters.
fn passes_filters(doc: &Doc<'_>, q: &SearchQuery) -> bool {
    if !q.domains.is_empty() && !doc.domain.is_some_and(|d| q.domains.contains(&d)) {
        return false;
    }
    if !q.kinds.is_empty() && !doc.rule_kind.is_some_and(|k| q.kinds.contains(&k)) {
        return false;
    }
    if let Some(p) = &q.profile {
        // A record that declares no profiles is style-neutral and always shown.
        if !doc.profiles.is_empty() && !doc.profiles.iter().any(|x| x == p) {
            return false;
        }
    }
    if !q.sources.is_empty() && !doc.source_ids.iter().any(|s| q.sources.contains(s)) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb() -> &'static KnowledgeBase {
        KnowledgeBase::embedded()
    }

    #[test]
    fn tokenizer_splits_on_punctuation_and_case() {
        assert_eq!(
            tokenize("Voice-Leading: parallel 5ths!"),
            vec!["voice", "leading", "parallel", "5ths"]
        );
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn an_exact_id_ranks_first() {
        let hits = kb().search(&SearchQuery::text(
            "extensions.major_natural_11_close_register",
        ));
        assert_eq!(hits[0].id, "extensions.major_natural_11_close_register");
        assert_eq!(hits[0].kind, "rule");
        assert!(hits[0].score >= BOOST_EXACT_ID);
    }

    #[test]
    fn search_is_byte_identical_across_runs() {
        let q = SearchQuery::text("parallel fifths voice leading");
        let a = kb().search(&q);
        let b = kb().search(&q);
        let render = |h: &[SearchHit]| {
            h.iter()
                .map(|x| format!("{}:{}:{:.6}", x.kind, x.id, x.score))
                .collect::<Vec<_>>()
        };
        assert_eq!(render(&a), render(&b));
        assert!(!a.is_empty());
    }

    #[test]
    fn ties_break_by_id() {
        let hits = kb().search(&SearchQuery {
            text: "loop".into(),
            max_results: 50,
            ..SearchQuery::default()
        });
        for w in hits.windows(2) {
            if (w[0].score - w[1].score).abs() < f64::EPSILON {
                assert!(w[0].id <= w[1].id, "{} then {}", w[0].id, w[1].id);
            } else {
                assert!(w[0].score > w[1].score);
            }
        }
    }

    #[test]
    fn domain_filter_restricts_to_rules() {
        let hits = kb().search(&SearchQuery {
            text: "chord".into(),
            domains: vec![RuleDomain::Looping],
            max_results: 100,
            ..SearchQuery::default()
        });
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.kind == "rule"));
        assert!(hits.iter().all(|h| h.id.starts_with("looping.")));
    }

    #[test]
    fn kind_filter_restricts_to_rules() {
        let hits = kb().search(&SearchQuery {
            text: "chord".into(),
            kinds: vec![RuleKind::HardIntegrity],
            max_results: 100,
            ..SearchQuery::default()
        });
        assert!(!hits.is_empty());
        for h in &hits {
            assert_eq!(
                kb().rule(&h.id).map(|r| r.kind),
                Some(RuleKind::HardIntegrity)
            );
        }
    }

    #[test]
    fn profile_filter_keeps_style_neutral_records() {
        let hits = kb().search(&SearchQuery {
            text: "blues".into(),
            profile: Some("blues".into()),
            max_results: 100,
            ..SearchQuery::default()
        });
        assert!(!hits.is_empty());
        for h in &hits {
            if let Some(r) = kb().rule(&h.id) {
                assert!(r.profiles.iter().any(|p| p == "blues"), "{}", h.id);
            }
        }
    }

    #[test]
    fn profile_filter_excludes_rules_scoped_elsewhere() {
        let hits = kb().search(&SearchQuery {
            text: "counterpoint".into(),
            profile: Some("drum_and_bass".into()),
            max_results: 100,
            ..SearchQuery::default()
        });
        for h in &hits {
            if let Some(r) = kb().rule(&h.id) {
                assert!(r.profiles.iter().any(|p| p == "drum_and_bass"), "{}", h.id);
            }
        }
    }

    #[test]
    fn source_filter_restricts_to_citing_records() {
        let hits = kb().search(&SearchQuery {
            text: "cadence".into(),
            sources: vec!["mt21c".into()],
            max_results: 100,
            ..SearchQuery::default()
        });
        assert!(!hits.is_empty());
        assert!(hits
            .iter()
            .all(|h| h.source_ids.iter().any(|s| s == "mt21c")));
    }

    #[test]
    fn an_empty_text_with_filters_browses() {
        let hits = kb().search(&SearchQuery {
            text: String::new(),
            domains: vec![RuleDomain::Counterpoint],
            max_results: 100,
            ..SearchQuery::default()
        });
        assert_eq!(hits.len(), 13);
        assert!(hits.windows(2).all(|w| w[0].id < w[1].id));
    }

    #[test]
    fn max_results_is_respected_and_defaults() {
        let capped = kb().search(&SearchQuery {
            text: "chord".into(),
            max_results: 3,
            ..SearchQuery::default()
        });
        assert_eq!(capped.len(), 3);
        let defaulted = kb().search(&SearchQuery::text("chord"));
        assert!(defaulted.len() <= DEFAULT_MAX_RESULTS);
    }

    #[test]
    fn a_query_matching_nothing_returns_nothing() {
        let hits = kb().search(&SearchQuery::text("zzzzqqqxyzzy"));
        assert!(hits.is_empty());
    }

    #[test]
    fn search_reaches_every_collection() {
        let mut seen: Vec<&str> = kb()
            .search(&SearchQuery {
                text: String::new(),
                max_results: 10_000,
                ..SearchQuery::default()
            })
            .iter()
            .map(|h| h.kind)
            .collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 14, "{seen:?}");
    }

    #[test]
    fn hits_carry_the_full_record() {
        let hits = kb().search(&SearchQuery::text("lydian"));
        let scale = hits
            .iter()
            .find(|h| h.kind == "scale")
            .expect("a scale hit");
        assert_eq!(
            scale
                .detail
                .get("semitones")
                .and_then(Json::as_arr)
                .map(<[Json]>::len),
            Some(7)
        );
    }

    #[test]
    fn query_from_json_reads_every_filter() {
        let q = SearchQuery::from_json(
            &Json::parse(
                r#"{"text":"x","domains":["harmony","nope"],"kinds":["theory_default"],
                    "profile":"blues","sources":["mt21c"],"max_results":5}"#,
            )
            .unwrap(),
        );
        assert_eq!(q.text, "x");
        assert_eq!(q.domains, vec![RuleDomain::Harmony]);
        assert_eq!(q.kinds, vec![RuleKind::TheoryDefault]);
        assert_eq!(q.profile.as_deref(), Some("blues"));
        assert_eq!(q.sources, vec!["mt21c".to_string()]);
        assert_eq!(q.max_results, 5);
    }

    #[test]
    fn hit_json_is_complete() {
        let hits = kb().search(&SearchQuery::text("tritone substitute"));
        let j = hits[0].to_json();
        for key in [
            "kind",
            "id",
            "title",
            "summary",
            "score",
            "source_ids",
            "detail",
        ] {
            assert!(j.get(key).is_some(), "missing {key}");
        }
    }
}
