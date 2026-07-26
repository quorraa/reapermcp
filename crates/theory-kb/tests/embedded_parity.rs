//! The embedded bundle must be exactly the tree on disk, and the predicate
//! vocabulary must be exactly the one the data uses.
//!
//! These are the two ways this crate could rot silently: a knowledge file
//! added without regenerating `embedded.rs`, and a predicate written into a
//! rule that the engine happens not to implement.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use theory_kb::embedded::EMBEDDED_FILES;
use theory_kb::prelude::*;

/// The repository root, found by walking up from this crate.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/theory-kb has two ancestors")
        .to_path_buf()
}

/// Every file under `knowledge/` on disk, keyed by forward-slash relative path.
fn disk_files() -> BTreeMap<String, String> {
    let root = repo_root().join("knowledge");
    let mut out = BTreeMap::new();
    theory_kb::load::read_tree(&root, &root, &mut out).expect("knowledge/ must be readable");
    out
}

#[test]
fn the_embedded_path_list_matches_the_disk_tree() {
    let disk: Vec<String> = disk_files().keys().cloned().collect();
    let embedded: Vec<String> = EMBEDDED_FILES
        .iter()
        .map(|(p, _)| (*p).to_string())
        .collect();
    assert_eq!(embedded, disk, "run `cargo run -p xtask -- regen-embedded`");
}

#[test]
fn the_embedded_contents_match_the_disk_tree_byte_for_byte() {
    let disk = disk_files();
    for (path, contents) in EMBEDDED_FILES {
        let on_disk = disk
            .get(*path)
            .unwrap_or_else(|| panic!("{path} is embedded but absent from knowledge/"));
        assert_eq!(on_disk, contents, "{path} differs between disk and binary");
    }
    assert_eq!(EMBEDDED_FILES.len(), disk.len());
}

#[test]
fn the_embedded_list_is_sorted_and_uses_forward_slashes() {
    for w in EMBEDDED_FILES.windows(2) {
        assert!(
            w[0].0 < w[1].0,
            "{} then {} is out of order",
            w[0].0,
            w[1].0
        );
    }
    for (p, _) in EMBEDDED_FILES {
        assert!(!p.contains('\\'), "{p} uses a backslash");
        assert!(!p.starts_with('/'), "{p} is absolute");
    }
}

#[test]
fn the_predicate_vocabulary_is_exactly_the_one_the_data_uses() {
    let kb = KnowledgeBase::embedded();
    let used = kb.predicates_used();
    let implemented: Vec<String> = RuleEngine::known_predicates()
        .iter()
        .map(|p| (*p).to_string())
        .collect();

    let unimplemented: Vec<&String> = used.iter().filter(|p| !implemented.contains(p)).collect();
    assert!(
        unimplemented.is_empty(),
        "predicates used by knowledge/ but not implemented: {unimplemented:?}"
    );

    let unused: Vec<&String> = implemented.iter().filter(|p| !used.contains(p)).collect();
    assert!(
        unused.is_empty(),
        "predicates implemented but never used by knowledge/: {unused:?}"
    );

    assert_eq!(
        used, implemented,
        "the two sets must be identical, in order"
    );
    assert_eq!(used.len(), 116);
}

#[test]
fn loading_the_directory_on_disk_agrees_with_the_embedded_bundle() {
    let kb = KnowledgeBase::load_dir(&repo_root().join("knowledge"))
        .expect("knowledge/ on disk must load and validate");
    let embedded = KnowledgeBase::embedded();
    assert_eq!(kb.content_hash(), embedded.content_hash());
    assert_eq!(kb.version(), embedded.version());
    assert_eq!(kb.counts(), embedded.counts());
    assert_eq!(kb.rules().len(), embedded.rules().len());
}

#[test]
fn every_rule_names_at_least_one_test_and_every_test_id_is_snake_case() {
    let kb = KnowledgeBase::embedded();
    for r in kb.rules() {
        assert!(!r.test_ids.is_empty(), "{} names no test", r.id);
        for t in &r.test_ids {
            assert!(
                t.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{t} is not snake_case"
            );
        }
    }
}

#[test]
fn every_rule_is_reachable_from_at_least_one_profile() {
    let kb = KnowledgeBase::embedded();
    let resolved: Vec<ResolvedProfile> = kb
        .profiles()
        .iter()
        .map(|p| kb.resolve_profile(&p.id).expect("resolves"))
        .collect();
    for r in kb.rules() {
        assert!(
            resolved.iter().any(|p| p.applies_to(r)),
            "{} is not active in any profile",
            r.id
        );
    }
}

#[test]
fn every_record_round_trips_through_json() {
    let kb = KnowledgeBase::embedded();
    // A round trip must preserve every field, including the ones this build
    // does not model and keeps only in `raw`.
    for r in kb.rules() {
        let back = TheoryRule::from_json(&r.to_json(), "round-trip").expect("re-reads");
        assert_eq!(&back, r, "{}", r.id);
    }
    for s in kb.sources() {
        let back =
            theory_kb::SourceRecord::from_json(&s.to_json(), "round-trip").expect("re-reads");
        assert_eq!(&back, s, "{}", s.id);
    }
    for q in kb.chord_qualities() {
        let json = q.to_json();
        let back = theory_kb::ChordQuality::from_json(&json, "round-trip").expect("re-reads");
        assert_eq!(back.to_json(), json, "{}", q.id);
    }
    for v in kb.voicing_templates() {
        let json = v.to_json();
        let back = theory_kb::VoicingTemplate::from_json(&json, "round-trip").expect("re-reads");
        assert_eq!(back.to_json(), json, "{}", v.id);
    }
    for a in kb.arrangement_patterns() {
        let json = a.to_json();
        let back = theory_kb::ArrangementPattern::from_json(&json, "round-trip").expect("re-reads");
        assert_eq!(back.to_json(), json, "{}", a.id);
    }
}

#[test]
fn the_catalog_survives_a_json_round_trip() {
    let kb = KnowledgeBase::embedded();
    let text = kb.catalog_json().to_string();
    let back = qjson::Json::parse(&text).expect("valid JSON");
    assert_eq!(back.to_string(), text);
}
