//! Golden analyses of the committed fixture corpus.
//!
//! Every fixture is analysed with a fixed set of parameters and the canonical
//! JSON is compared byte-for-byte with the file committed under
//! `fixtures/expected/`. This is the crate's strongest determinism guarantee:
//! it pins not just "the same input gives the same output twice in one process"
//! but "the output has not changed since it was reviewed".
//!
//! Regenerate after an intentional change with:
//!
//! ```text
//! MUSIC_ANALYSIS_WRITE_GOLDENS=1 cargo test -p music-analysis --test golden
//! ```
//!
//! and **read the diff** before committing it.

mod common;

use common::{analysis_notes, fixtures, has_own_notes, loop_span, repo_root};
use music_analysis::prelude::*;
use music_analysis::selection::ExtractionMode;
use music_domain::prelude::*;
use theory_kb::KnowledgeBase;

/// The parameters every golden is produced with.
///
/// Deliberately *not* the fixture's `key_hint`: the point of the goldens is to
/// pin what the analysis infers, and handing it the answer would pin nothing.
fn params(f: &Fixture) -> AnalyzeParams {
    AnalyzeParams {
        profile_id: "common_practice".to_string(),
        extraction: if has_own_notes(f) {
            ExtractionRequest::auto()
        } else {
            ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony)
        },
        tonal_center: None,
        meter: None,
        grid: GridMode::Auto,
        strictness: Strictness::Balanced,
        loop_span: loop_span(f),
    }
}

/// `melodies/dorian_vamp_d` -> `melodies-dorian_vamp_d.json`.
fn golden_name(id: &str) -> String {
    format!("{}.json", id.replace('/', "-"))
}

/// The snapshot id a golden is produced against: fixed, so the analysis id is
/// pinned too.
fn snapshot_id(f: &Fixture) -> String {
    format!("fixture:{}", f.id)
}

#[test]
fn every_fixture_analyses_and_matches_its_golden() {
    let kb = KnowledgeBase::embedded();
    let dir = repo_root().join("fixtures/expected");
    let write = std::env::var("MUSIC_ANALYSIS_WRITE_GOLDENS").is_ok();
    if write {
        std::fs::create_dir_all(&dir).expect("the golden directory must be creatable");
    }

    let all = fixtures();
    assert_eq!(all.len(), 11, "the committed corpus is eleven fixtures");

    for f in &all {
        let notes = analysis_notes(f);
        let analysis = analyze(kb, &snapshot_id(f), &notes, &params(f))
            .unwrap_or_else(|e| panic!("{}: {e}", f.id));
        let text = format!("{}\n", analysis.to_json().to_canonical_string());
        let path = dir.join(golden_name(&f.id));

        if write {
            std::fs::write(&path, &text).expect("the golden must be writable");
            continue;
        }
        let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "{} is missing; regenerate with MUSIC_ANALYSIS_WRITE_GOLDENS=1",
                path.display()
            )
        });
        assert_eq!(
            on_disk, text,
            "the analysis of {} has changed; review the diff, then regenerate with \
             MUSIC_ANALYSIS_WRITE_GOLDENS=1",
            f.id
        );
    }
}

#[test]
fn goldens_are_valid_canonical_json() {
    let dir = repo_root().join("fixtures/expected");
    for f in fixtures() {
        let path = dir.join(golden_name(&f.id));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let parsed = qjson::Json::parse(text.trim_end()).unwrap_or_else(|e| {
            panic!("{} is not valid JSON: {e}", path.display());
        });
        assert_eq!(
            format!("{}\n", parsed.to_canonical_string()),
            text,
            "{} is not in canonical form",
            path.display()
        );
    }
}

#[test]
fn a_golden_run_is_reproducible_within_the_process() {
    let kb = KnowledgeBase::embedded();
    for f in fixtures() {
        let notes = analysis_notes(&f);
        let p = params(&f);
        let a = analyze(kb, &snapshot_id(&f), &notes, &p).expect("analysis");
        let b = analyze(kb, &snapshot_id(&f), &notes, &p).expect("analysis");
        assert_eq!(a.id, b.id, "{}", f.id);
        assert_eq!(
            a.to_json().to_canonical_string(),
            b.to_json().to_canonical_string(),
            "{}",
            f.id
        );
    }
}

#[test]
fn every_golden_names_the_knowledge_version_it_was_taken_against() {
    let kb = KnowledgeBase::embedded();
    let dir = repo_root().join("fixtures/expected");
    for f in fixtures() {
        let path = dir.join(golden_name(&f.id));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let parsed = qjson::Json::parse(text.trim_end()).expect("valid JSON");
        assert_eq!(
            parsed
                .get("knowledge_version")
                .and_then(qjson::Json::as_str),
            Some(kb.version()),
            "{} was taken against a different knowledge bundle",
            path.display()
        );
    }
}

#[test]
fn the_golden_directory_holds_exactly_the_corpus() {
    let dir = repo_root().join("fixtures/expected");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut on_disk: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".json"))
        .collect();
    on_disk.sort();
    let mut expected: Vec<String> = fixtures().iter().map(|f| golden_name(&f.id)).collect();
    expected.sort();
    assert_eq!(on_disk, expected);
}
