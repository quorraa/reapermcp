//! The `test_ids` coverage ledger.
//!
//! Every rule in `knowledge/` names the behavioural tests that pin it. Most of
//! those tests belong to crates that do not exist yet — the harmony, voice-
//! leading, arrangement and loop engines — so this crate cannot honestly claim
//! them. Rather than pretending, the gap is written down.
//!
//! `tests/test_id_coverage.json` records, per crate, which `test_id`s are
//! actually implemented. This test keeps that ledger honest in three ways:
//!
//! * `implemented ∪ pending` must be **exactly** the set of `test_id`s the
//!   knowledge base declares, so a new rule cannot add a test id that quietly
//!   goes unnoticed;
//! * `by_crate["theory-kb"]` must be **exactly** the list below, so this crate
//!   cannot claim a test it does not have;
//! * later crates extend `by_crate` with their own entries, and the union must
//!   still equal `implemented`.
//!
//! Set `THEORY_KB_WRITE_COVERAGE=1` to rewrite the ledger after adding tests.

use qjson::{Json, JsonMap};
use std::path::{Path, PathBuf};
use theory_kb::prelude::*;

/// The `test_id`s this crate genuinely pins, each by a test of the same name in
/// `tests/rule_engine.rs`.
///
/// Everything here is a rule-firing behaviour — does the rule apply, is it
/// bypassed by its own exception, is it in scope for this profile, does a hard
/// rule reject — which is exactly what `theory-kb` owns. Test ids describing
/// *musical output* (a generated voicing, a chosen progression) belong to the
/// engines and stay in `pending`.
const THEORY_KB_TESTS: &[&str] = &[
    "accented_dissonance_allowed_elsewhere",
    "carry_policy_allows_overhang",
    "closed_tonic_prefers_turnaround",
    "dominant_resolves_down_fifth",
    "hanging_note_detected",
    "hidden_fifths_ignored_pop",
    "hidden_fifths_penalised_strict",
    "major11_close_register_penalty",
    "major11_melody_exception",
    "major11_omit3_exception",
    "midi_pitch_bounds_enforced",
    "modal_drone_no_dominant_required",
    "modal_loop_scored_on_common_tones",
    "no_invalid_midi_pitches",
    "preserve_melody_keeps_every_pitch",
    "strong_beat_consonance_strict",
    "v_to_i_across_wrap_compatible",
];

/// Where the ledger lives.
fn ledger_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/test_id_coverage.json")
}

/// Builds the ledger the current tree implies.
fn expected_ledger() -> Json {
    let kb = KnowledgeBase::embedded();
    let all = kb.test_ids();

    let mut implemented: Vec<String> = THEORY_KB_TESTS.iter().map(|s| (*s).to_string()).collect();
    implemented.sort();
    implemented.dedup();

    let pending: Vec<String> = all
        .iter()
        .filter(|t| !implemented.contains(t))
        .cloned()
        .collect();

    let mut by_crate = JsonMap::new();
    by_crate.insert("theory-kb", strings(&implemented));

    let mut m = JsonMap::new();
    m.insert("schema_version", Json::Str("1.0.0".into()));
    m.insert(
        "note",
        Json::Str(
            "Which knowledge test_ids have a behavioural test behind them. \
             Later crates add their own key under by_crate; implemented is the \
             union, and implemented + pending must equal every test_id in \
             knowledge/. Regenerate with THEORY_KB_WRITE_COVERAGE=1 cargo test \
             -p theory-kb --test test_id_coverage."
                .into(),
        ),
    );
    m.insert("knowledge_version", Json::Str(kb.version().to_string()));
    m.insert("total_test_ids", Json::Int(all.len() as i64));
    m.insert("implemented_count", Json::Int(implemented.len() as i64));
    m.insert(
        "coverage_percent",
        Json::Float(percent(implemented.len(), all.len())),
    );
    m.insert("by_crate", Json::Obj(by_crate));
    m.insert("implemented", strings(&implemented));
    m.insert("pending", strings(&pending));
    Json::Obj(m)
}

/// Coverage as a percentage rounded to one decimal place.
fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    (part as f64 / whole as f64 * 1000.0).round() / 10.0
}

/// Renders a string list.
fn strings(items: &[String]) -> Json {
    Json::Arr(items.iter().map(|s| Json::Str(s.clone())).collect())
}

/// Reads a string array from the ledger.
fn read_strings(v: &Json, key: &str) -> Vec<String> {
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

#[test]
fn the_committed_ledger_is_current() {
    let expected = expected_ledger();
    let text = format!("{}\n", expected.to_string_pretty());

    if std::env::var("THEORY_KB_WRITE_COVERAGE").is_ok() {
        std::fs::write(ledger_path(), &text).expect("the ledger must be writable");
        return;
    }

    let on_disk = std::fs::read_to_string(ledger_path()).expect(
        "tests/test_id_coverage.json must exist; \
         regenerate with THEORY_KB_WRITE_COVERAGE=1",
    );
    assert_eq!(
        on_disk, text,
        "the coverage ledger has drifted; regenerate with \
         THEORY_KB_WRITE_COVERAGE=1 cargo test -p theory-kb --test test_id_coverage"
    );
}

#[test]
fn implemented_and_pending_together_are_every_declared_test_id() {
    let ledger =
        Json::parse(&std::fs::read_to_string(ledger_path()).expect("ledger")).expect("valid JSON");
    let implemented = read_strings(&ledger, "implemented");
    let pending = read_strings(&ledger, "pending");

    let mut union: Vec<String> = implemented.iter().chain(pending.iter()).cloned().collect();
    union.sort();
    let deduped_len = {
        let mut u = union.clone();
        u.dedup();
        u.len()
    };
    assert_eq!(
        deduped_len,
        union.len(),
        "a test id may not be both implemented and pending"
    );
    assert_eq!(
        union,
        KnowledgeBase::embedded().test_ids(),
        "the ledger must cover exactly the test ids the knowledge base declares"
    );
}

#[test]
fn every_claimed_test_id_is_actually_declared_by_a_rule() {
    let kb = KnowledgeBase::embedded();
    for t in THEORY_KB_TESTS {
        let owners: Vec<&str> = kb
            .rules()
            .iter()
            .filter(|r| r.test_ids.iter().any(|x| x == t))
            .map(|r| r.id.as_str())
            .collect();
        assert!(
            !owners.is_empty(),
            "{t} is claimed but no rule in knowledge/ names it"
        );
    }
}

#[test]
fn the_crate_breakdown_sums_to_the_implemented_list() {
    let ledger =
        Json::parse(&std::fs::read_to_string(ledger_path()).expect("ledger")).expect("valid JSON");
    let by_crate = ledger
        .get("by_crate")
        .and_then(Json::as_obj)
        .expect("by_crate must be an object");

    let mut union: Vec<String> = Vec::new();
    for (_, list) in by_crate.iter() {
        for id in list.as_arr().unwrap_or(&[]).iter().filter_map(Json::as_str) {
            if !union.iter().any(|x| x == id) {
                union.push(id.to_string());
            }
        }
    }
    union.sort();
    assert_eq!(union, read_strings(&ledger, "implemented"));

    let mine: Vec<String> = by_crate
        .get("theory-kb")
        .and_then(Json::as_arr)
        .expect("theory-kb must have an entry")
        .iter()
        .filter_map(Json::as_str)
        .map(str::to_string)
        .collect();
    let mut declared: Vec<String> = THEORY_KB_TESTS.iter().map(|s| (*s).to_string()).collect();
    declared.sort();
    assert_eq!(mine, declared);
}

#[test]
fn the_reported_numbers_are_arithmetically_honest() {
    let ledger =
        Json::parse(&std::fs::read_to_string(ledger_path()).expect("ledger")).expect("valid JSON");
    let total = ledger.get("total_test_ids").and_then(Json::as_i64).unwrap() as usize;
    let implemented_count = ledger
        .get("implemented_count")
        .and_then(Json::as_i64)
        .unwrap() as usize;
    let coverage = ledger
        .get("coverage_percent")
        .and_then(Json::as_f64)
        .unwrap();

    assert_eq!(total, KnowledgeBase::embedded().test_ids().len());
    assert_eq!(
        implemented_count,
        read_strings(&ledger, "implemented").len()
    );
    assert_eq!(
        total - implemented_count,
        read_strings(&ledger, "pending").len()
    );
    assert!((coverage - percent(implemented_count, total)).abs() < 1e-9);
    assert!(
        coverage > 0.0 && coverage < 100.0,
        "coverage is {coverage}%"
    );
}

#[test]
fn each_test_this_crate_claims_exists_as_a_rust_test() {
    // The claim is only meaningful if a function of the same name exists.
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/rule_engine.rs"))
            .expect("the behavioural test file must exist");
    for t in THEORY_KB_TESTS {
        assert!(
            source.contains(&format!("fn {t}()")),
            "{t} is claimed but tests/rule_engine.rs has no `fn {t}()`"
        );
    }
}
