//! `validate-fixtures`: loads every corpus fixture through the shared loader.
//!
//! `xtask` depends only on `qjson` and `theory-kb`, so the loader is reached
//! through `theory_kb::music_domain`, which re-exports the domain crate for
//! exactly this reason.

use crate::report::{count, Report};
use std::path::Path;
use theory_kb::music_domain::fixture::Fixture;

/// The fixture corpus directories, excluding the golden-output and recorded-IPC
/// trees, which are not fixtures in this format.
const CORPUS_DIRS: &[&str] = &["melodies", "progressions", "loops"];

/// Loads and validates every fixture, checking the golden outputs are wired up.
pub fn validate_fixtures(root: &Path) -> Report {
    let mut report = Report::new("validate-fixtures");
    let fixtures = root.join("fixtures");
    let mut loaded = 0usize;
    let mut notes = 0usize;
    let mut chords = 0usize;

    for dir in CORPUS_DIRS {
        let d = fixtures.join(dir);
        if !d.is_dir() {
            report.problem(d.display().to_string(), "fixture directory is missing");
            continue;
        }
        for path in crate::schemas::json_files(&d) {
            let rel = path.strip_prefix(root).unwrap_or(&path).display().to_string();
            match Fixture::from_path(&path) {
                Ok(f) => {
                    if let Err(e) = f.validate() {
                        report.problem(rel.clone(), format!("[{}] {}", e.code, e.message));
                        continue;
                    }
                    // The id must match the file's place in the corpus, so a
                    // golden output can be found from the fixture alone.
                    let expected_id = format!(
                        "{dir}/{}",
                        path.file_stem().and_then(|s| s.to_str()).unwrap_or_default()
                    );
                    if f.id != expected_id {
                        report.problem(
                            rel.clone(),
                            format!("declares id '{}' but sits at '{expected_id}'", f.id),
                        );
                    }
                    let ns = f.note_set();
                    notes += ns.notes.len();
                    chords += f.chord_events().len();
                    // Exercise the derived views so a broken time map is caught
                    // here rather than in an engine three crates away.
                    let tm = f.time_map();
                    if tm.tempos.is_empty() || tm.meters.is_empty() {
                        report.problem(rel.clone(), "the fixture produced an empty time map");
                    }
                    if ns.notes.is_empty() && f.chord_events().is_empty() {
                        report.problem(rel.clone(), "the fixture has neither notes nor chords");
                    }
                    loaded += 1;
                }
                Err(e) => {
                    report.problem(rel, format!("[{}] {}", e.code, e.message));
                }
            }
        }
    }

    report.note(format!(
        "loaded {loaded} fixtures ({notes} notes, {chords} chords)"
    ));
    report.detail("fixtures", count(loaded));
    report.detail("notes", count(notes));
    report.detail("chords", count(chords));
    if loaded == 0 {
        report.problem("fixtures/", "the corpus is empty");
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_fixtures_passes_on_the_repository() {
        let r = validate_fixtures(&crate::repo_root());
        assert!(r.ok, "{:#?}", r.problems);
    }

    #[test]
    fn every_corpus_directory_holds_at_least_one_fixture() {
        let root = crate::repo_root();
        for d in CORPUS_DIRS {
            let files = crate::schemas::json_files(&root.join("fixtures").join(d));
            assert!(!files.is_empty(), "fixtures/{d} is empty");
        }
    }
}
