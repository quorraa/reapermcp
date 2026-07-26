//! `validate-knowledge`, `stamp-manifest` and `regen-embedded`.

use crate::report::{count, strings, Report};
use qjson::Json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use theory_kb::load::{content_hash, read_tree, KnowledgeBase, MANIFEST_FILE};
use theory_kb::validate::{validate_report, ValidateOptions};

/// Reads every file under `dir`, keyed by forward-slash relative path.
pub fn read_bundle(dir: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut files = BTreeMap::new();
    read_tree(dir, dir, &mut files).map_err(|e| e.to_string())?;
    Ok(files)
}

/// Full validation of the on-disk bundle and of the embedded bundle.
///
/// The on-disk copy is allowed to carry a `"PENDING"` hash only when it is the
/// repository's own `knowledge/`, which `stamp-manifest` is responsible for.
/// An explicit `--knowledge-dir` is held to the external-directory rules and
/// is rejected while unstamped.
pub fn validate_knowledge(root: &Path, knowledge_dir: Option<&Path>) -> Report {
    let mut report = Report::new("validate-knowledge");
    let is_external = knowledge_dir.is_some();
    let dir: PathBuf = knowledge_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join("knowledge"));

    // --- on disk ---
    match read_bundle(&dir) {
        Err(e) => {
            report.problem(dir.display().to_string(), e);
        }
        Ok(files) => match KnowledgeBase::parse(files, &dir.display().to_string()) {
            Err(e) => {
                report.problem_json(e.to_json());
            }
            Ok(kb) => {
                let errors = validate_report(
                    &kb,
                    &ValidateOptions {
                        allow_pending_hash: !is_external,
                        ..ValidateOptions::default()
                    },
                );
                for e in &errors {
                    report.problem_json(e.to_json());
                }
                report.note(format!(
                    "on disk:  {} v{} ({} rules, {} profiles, {} sources)",
                    dir.display(),
                    kb.version(),
                    kb.rules().len(),
                    kb.profiles().len(),
                    kb.sources().len()
                ));
                report.detail("disk_content_sha256", Json::Str(kb.content_hash().into()));
                report.detail(
                    "manifest_content_sha256",
                    Json::Str(kb.manifest().content_sha256.clone()),
                );
                let mut counts = qjson::JsonMap::new();
                for (k, v) in kb.counts() {
                    counts.insert(k, count(v));
                }
                report.detail("counts", Json::Obj(counts));
                let mut by_domain = qjson::JsonMap::new();
                for (k, v) in kb.rule_counts_by_domain() {
                    by_domain.insert(k, count(v));
                }
                report.detail("rules_by_domain", Json::Obj(by_domain));
                report.detail("test_ids", count(kb.test_ids().len()));
                report.detail("predicates_used", count(kb.predicates_used().len()));
            }
        },
    }

    // --- embedded ---
    match KnowledgeBase::try_embedded() {
        Err(e) => {
            report.problem_json(e.to_json());
        }
        Ok(kb) => {
            report.note(format!(
                "embedded: v{} ({} rules, {} predicates used of {} implemented)",
                kb.version(),
                kb.rules().len(),
                kb.predicates_used().len(),
                theory_kb::RuleEngine::known_predicates().len()
            ));
            report.detail(
                "embedded_content_sha256",
                Json::Str(kb.content_hash().into()),
            );

            // A predicate in the data this build cannot execute is a hard error.
            let implemented: Vec<&str> = theory_kb::RuleEngine::known_predicates().to_vec();
            let missing: Vec<String> = kb
                .predicates_used()
                .into_iter()
                .filter(|p| !implemented.contains(&p.as_str()))
                .collect();
            for p in &missing {
                report.problem(
                    "knowledge/rules",
                    format!("predicate '{p}' is used by the data but not implemented"),
                );
            }
            let unused: Vec<String> = implemented
                .iter()
                .filter(|p| !kb.predicates_used().iter().any(|u| u == *p))
                .map(|p| (*p).to_string())
                .collect();
            if !unused.is_empty() {
                report.detail("predicates_implemented_but_unused", strings(&unused));
            }
        }
    }
    report
}

/// Recomputes and rewrites `manifest.json`'s `content_sha256`.
///
/// # Exact byte layout of the hashed input
///
/// The stamped value is `SHA-256` over the concatenation, in **byte-wise
/// ascending relative-path order**, of the **UTF-8 canonical JSON**
/// (`qjson::Json::to_canonical_string`) of **every `*.json` file under the
/// knowledge root except `manifest.json`**. No separators, no path bytes, no
/// length prefixes, no trailing newline. The digest is rendered as 64
/// lower-case hex characters. See [`theory_kb::load::content_hash`], which is
/// the single implementation both this command and the loader use, so a
/// stamped manifest and a loaded bundle can never disagree.
///
/// The rewrite is a surgical replacement of the quoted value, so the rest of
/// the authored file — key order, indentation, comments in `notes` — survives
/// byte for byte.
pub fn stamp_manifest(root: &Path, knowledge_dir: Option<&Path>, check_only: bool) -> Report {
    let mut report = Report::new("stamp-manifest");
    let dir: PathBuf = knowledge_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join("knowledge"));

    let files = match read_bundle(&dir) {
        Ok(f) => f,
        Err(e) => {
            report.problem(dir.display().to_string(), e);
            return report;
        }
    };

    let mut docs = BTreeMap::new();
    for (path, text) in &files {
        if path.ends_with(".json") {
            match Json::parse(text) {
                Ok(d) => {
                    docs.insert(path.clone(), d);
                }
                Err(e) => {
                    report.problem(format!("{}/{path}", dir.display()), e.to_string());
                    return report;
                }
            }
        }
    }

    let hash = content_hash(&docs);
    report.detail("content_sha256", Json::Str(hash.clone()));
    report.detail(
        "hashed_files",
        strings(
            &docs
                .keys()
                .filter(|k| k.as_str() != MANIFEST_FILE)
                .cloned()
                .collect::<Vec<_>>(),
        ),
    );

    let manifest_path = dir.join(MANIFEST_FILE);
    let text = match std::fs::read_to_string(&manifest_path) {
        Ok(t) => t,
        Err(e) => {
            report.problem(manifest_path.display().to_string(), e.to_string());
            return report;
        }
    };
    let current = docs
        .get(MANIFEST_FILE)
        .and_then(|m| m.get("content_sha256"))
        .and_then(Json::as_str)
        .unwrap_or("");
    report.detail("previous_content_sha256", Json::Str(current.to_string()));

    if current == hash {
        report.note(format!("manifest already stamped with {hash}"));
        return report;
    }
    if check_only {
        report.problem(
            format!("{}/{MANIFEST_FILE}", dir.display()),
            format!(
                "content_sha256 is \"{current}\" but the files hash to {hash}; \
                 run `cargo run -p xtask -- stamp-manifest`"
            ),
        );
        return report;
    }

    match replace_hash_value(&text, &hash) {
        Ok(updated) => match std::fs::write(&manifest_path, updated) {
            Ok(()) => {
                report.note(format!("stamped {} with {hash}", manifest_path.display()));
            }
            Err(e) => {
                report.problem(manifest_path.display().to_string(), e.to_string());
            }
        },
        Err(e) => {
            report.problem(manifest_path.display().to_string(), e);
        }
    }
    report
}

/// Replaces the quoted value of `"content_sha256"` in raw manifest text,
/// leaving every other byte untouched.
pub fn replace_hash_value(text: &str, hash: &str) -> Result<String, String> {
    const KEY: &str = "\"content_sha256\"";
    let key_at = text
        .find(KEY)
        .ok_or_else(|| "the manifest has no content_sha256 field".to_string())?;
    let after_key = key_at + KEY.len();
    let colon = text[after_key..]
        .find(':')
        .ok_or_else(|| "content_sha256 is not followed by a colon".to_string())?
        + after_key;
    let open = text[colon + 1..]
        .find('"')
        .ok_or_else(|| "content_sha256 has no string value".to_string())?
        + colon
        + 1;
    let close = text[open + 1..]
        .find('"')
        .ok_or_else(|| "content_sha256's value is unterminated".to_string())?
        + open
        + 1;
    Ok(format!("{}{hash}{}", &text[..open + 1], &text[close..]))
}

/// The exact text of the generated `crates/theory-kb/src/embedded.rs`.
pub fn render_embedded(paths: &[String]) -> String {
    let mut out = String::new();
    out.push_str("//! Generated file - do not edit by hand.\n");
    out.push_str("//!\n");
    out.push_str("//! Regenerate with `cargo run -p xtask -- regen-embedded`. The list is checked\n");
    out.push_str("//! against `knowledge/` on disk by `tests/embedded_parity.rs`, so it can never\n");
    out.push_str("//! silently drift from the tree it was generated from.\n");
    out.push('\n');
    out.push_str("/// Every file under `knowledge/`, as `(relative path, contents)` pairs.\n");
    out.push_str("///\n");
    out.push_str("/// Paths use forward slashes and are sorted byte-wise ascending. The layout is\n");
    out.push_str("/// fixed by the generator, so `rustfmt` is asked to leave it alone.\n");
    out.push_str("#[rustfmt::skip]\n");
    out.push_str("pub static EMBEDDED_FILES: &[(&str, &str)] = &[\n");
    for p in paths {
        out.push_str(&format!(
            "    (\"{p}\", include_str!(\"../../../knowledge/{p}\")),\n"
        ));
    }
    out.push_str("];\n");
    out
}

/// Regenerates `crates/theory-kb/src/embedded.rs` from `knowledge/` on disk.
pub fn regen_embedded(root: &Path, check_only: bool) -> Report {
    let mut report = Report::new("regen-embedded");
    let dir = root.join("knowledge");
    let files = match read_bundle(&dir) {
        Ok(f) => f,
        Err(e) => {
            report.problem(dir.display().to_string(), e);
            return report;
        }
    };
    let paths: Vec<String> = files.keys().cloned().collect();
    for p in &paths {
        if p.contains('"') || p.contains('\\') {
            report.problem(p, "knowledge file names must not contain quotes or backslashes");
            return report;
        }
    }
    let wanted = render_embedded(&paths);
    let target = root.join("crates/theory-kb/src/embedded.rs");
    let existing = std::fs::read_to_string(&target).unwrap_or_default();

    report.detail("files", count(paths.len()));
    if existing == wanted {
        report.note(format!(
            "{} is current ({} files)",
            target.display(),
            paths.len()
        ));
        return report;
    }
    if check_only {
        report.problem(
            target.display().to_string(),
            "the embedded file list has drifted from knowledge/; \
             run `cargo run -p xtask -- regen-embedded`",
        );
        return report;
    }
    match std::fs::write(&target, wanted) {
        Ok(()) => {
            report.note(format!(
                "regenerated {} ({} files)",
                target.display(),
                paths.len()
            ));
        }
        Err(e) => {
            report.problem(target.display().to_string(), e.to_string());
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_replacement_is_surgical() {
        let text = "{\n  \"a\": 1,\n  \"content_sha256\": \"PENDING\",\n  \"b\": 2\n}\n";
        let out = replace_hash_value(text, "abc").unwrap();
        assert_eq!(
            out,
            "{\n  \"a\": 1,\n  \"content_sha256\": \"abc\",\n  \"b\": 2\n}\n"
        );
    }

    #[test]
    fn hash_replacement_is_idempotent() {
        let text = "{\"content_sha256\":\"old\"}";
        let once = replace_hash_value(text, "new").unwrap();
        assert_eq!(replace_hash_value(&once, "new").unwrap(), once);
    }

    #[test]
    fn hash_replacement_reports_a_missing_field() {
        assert!(replace_hash_value("{}", "x").is_err());
        assert!(replace_hash_value("{\"content_sha256\"", "x").is_err());
    }

    #[test]
    fn rendered_embedded_matches_the_committed_file() {
        let root = crate::repo_root();
        let files = read_bundle(&root.join("knowledge")).expect("knowledge/ must be readable");
        let paths: Vec<String> = files.keys().cloned().collect();
        let on_disk = std::fs::read_to_string(root.join("crates/theory-kb/src/embedded.rs"))
            .expect("embedded.rs must exist");
        assert_eq!(
            render_embedded(&paths),
            on_disk,
            "run `cargo run -p xtask -- regen-embedded`"
        );
    }

    #[test]
    fn validate_knowledge_passes_on_the_repository() {
        let r = validate_knowledge(&crate::repo_root(), None);
        assert!(r.ok, "{:#?}", r.problems);
    }

    #[test]
    fn stamp_manifest_check_mode_is_clean() {
        let r = stamp_manifest(&crate::repo_root(), None, true);
        assert!(r.ok, "{:#?}", r.problems);
    }

    #[test]
    fn regen_embedded_check_mode_is_clean() {
        let r = regen_embedded(&crate::repo_root(), true);
        assert!(r.ok, "{:#?}", r.problems);
    }
}
