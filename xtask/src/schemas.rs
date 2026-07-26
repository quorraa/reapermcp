//! `validate-schemas`: compiles every schema and checks every governed file.
//!
//! Fixture envelopes follow a `valid-*` / `invalid-*` naming convention, and
//! both directions are checked: a `valid-*` file must pass its schema, and an
//! `invalid-*` file must fail it. A negative fixture that quietly starts
//! passing is exactly as broken as a positive one that starts failing.
//!
//! The exception is the handful of negatives listed in [`SEMANTIC_NEGATIVES`],
//! which are well-formed envelopes that only the bridge can reject at request
//! time; those are checked as positives.

use crate::report::{count, Report};
use qjson::schema::Schema;
use qjson::Json;
use std::path::Path;

/// Collects every `*.json` file under `dir`, recursively, sorted.
pub fn json_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    collect(dir, &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "json") {
            out.push(p);
        }
    }
}

/// Which schema governs a fixture path, if any.
fn fixture_schema(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".command.json") {
        Some("ipc-request.schema.json")
    } else if name.ends_with(".result.json") {
        Some("ipc-result.schema.json")
    } else {
        None
    }
}

/// Negative fixtures whose invalidity is **semantic**, not structural.
///
/// An expired deadline and a wrong instance token are perfectly well-formed
/// envelopes; only the bridge can reject them, at request time. They are
/// listed here rather than being silently exempted by a looser rule, so
/// adding a genuinely structural negative fixture still gets checked.
const SEMANTIC_NEGATIVES: &[&str] = &["invalid-expired.command.json", "invalid-token.command.json"];

/// True when a fixture's name declares it as a structural negative case.
fn expects_failure(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("invalid-") && !SEMANTIC_NEGATIVES.contains(&n))
}

/// True when the fixture is a negative case the schema cannot detect.
fn is_semantic_negative(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| SEMANTIC_NEGATIVES.contains(&n))
}

/// Compiles every `schemas/*.schema.json` and validates every governed file.
pub fn validate_schemas(root: &Path) -> Report {
    let mut report = Report::new("validate-schemas");
    let schema_dir = root.join("schemas");

    let mut compiled: Vec<(String, Schema)> = Vec::new();
    for path in json_files(&schema_dir) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                report.problem(path.display().to_string(), e.to_string());
                continue;
            }
        };
        let doc = match Json::parse(&text) {
            Ok(d) => d,
            Err(e) => {
                report.problem(path.display().to_string(), e.to_string());
                continue;
            }
        };
        match Schema::compile(&doc) {
            Ok(s) => compiled.push((name, s)),
            Err(e) => {
                report.problem(path.display().to_string(), e.message);
            }
        }
    }
    report.note(format!("compiled {} schemas", compiled.len()));
    report.detail("schemas", count(compiled.len()));

    let find = |name: &str| compiled.iter().find(|(n, _)| n == name).map(|(_, s)| s);

    // --- knowledge ---
    let knowledge = root.join("knowledge");
    let mut checked = 0usize;
    for path in json_files(&knowledge) {
        let rel = path
            .strip_prefix(&knowledge)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let Some(name) = theory_kb::validate::schema_for(&rel) else {
            continue;
        };
        let Some(schema) = find(name) else {
            report.problem(name, "schema is missing from schemas/");
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            report.problem(path.display().to_string(), "unreadable");
            continue;
        };
        match Json::parse(&text) {
            Ok(doc) => {
                checked += 1;
                for v in schema.validate(&doc) {
                    report.problem(
                        format!("knowledge/{rel}{}", v.instance_path),
                        format!("{}: {} (against {name})", v.keyword, v.message),
                    );
                }
            }
            Err(e) => {
                report.problem(format!("knowledge/{rel}"), e.to_string());
            }
        }
    }
    report.note(format!(
        "checked {checked} knowledge files against their schema"
    ));
    report.detail("knowledge_files", count(checked));

    // --- fixtures ---
    let fixtures = root.join("fixtures");
    let mut positive = 0usize;
    let mut negative = 0usize;
    let mut semantic = 0usize;
    for path in json_files(&fixtures) {
        let Some(name) = fixture_schema(&path) else {
            continue;
        };
        let Some(schema) = find(name) else {
            report.problem(name, "schema is missing from schemas/");
            continue;
        };
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let negative_case = expects_failure(&path);
        let violations = match Json::parse(&text) {
            Ok(doc) => schema
                .validate(&doc)
                .into_iter()
                .map(|v| format!("{}{}: {}", v.instance_path, v.keyword, v.message))
                .collect::<Vec<_>>(),
            // A file that does not even parse counts as failing its schema,
            // which is the point of `invalid-truncated.command.json`.
            Err(e) => vec![e.to_string()],
        };
        if negative_case {
            negative += 1;
            if violations.is_empty() {
                report.problem(
                    rel,
                    format!("a negative fixture unexpectedly satisfies {name}"),
                );
            }
        } else if is_semantic_negative(&path) {
            // Well-formed on the wire; only the bridge can reject it.
            semantic += 1;
            for v in violations {
                report.problem(rel.clone(), format!("{v} (against {name})"));
            }
        } else {
            positive += 1;
            for v in violations {
                report.problem(rel.clone(), format!("{v} (against {name})"));
            }
        }
    }
    report.note(format!(
        "checked {positive} valid, {semantic} semantically-invalid and {negative} \
         structurally-invalid IPC envelopes"
    ));
    report.detail("valid_envelopes", count(positive));
    report.detail("semantic_negative_envelopes", count(semantic));
    report.detail("invalid_envelopes", count(negative));
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_routing() {
        assert_eq!(
            fixture_schema(Path::new("a/valid-ping.command.json")),
            Some("ipc-request.schema.json")
        );
        assert_eq!(
            fixture_schema(Path::new("a/valid-stage.result.json")),
            Some("ipc-result.schema.json")
        );
        assert_eq!(fixture_schema(Path::new("a/melody.json")), None);
        assert!(expects_failure(Path::new(
            "x/invalid-truncated.command.json"
        )));
        assert!(!expects_failure(Path::new("x/valid-token.command.json")));
        // Semantic negatives are structurally valid, so they are checked as
        // positives instead.
        assert!(!expects_failure(Path::new("x/invalid-token.command.json")));
        assert!(is_semantic_negative(Path::new(
            "x/invalid-token.command.json"
        )));
        assert!(!is_semantic_negative(Path::new(
            "x/invalid-truncated.command.json"
        )));
    }

    #[test]
    fn validate_schemas_passes_on_the_repository() {
        let r = validate_schemas(&crate::repo_root());
        assert!(r.ok, "{:#?}", r.problems);
    }
}
