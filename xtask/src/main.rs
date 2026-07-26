//! `xtask` — the repository's own build and verification commands.
//!
//! ```text
//! cargo run -p xtask -- validate-knowledge [--knowledge-dir PATH] [--json]
//! cargo run -p xtask -- stamp-manifest     [--knowledge-dir PATH] [--check] [--json]
//! cargo run -p xtask -- validate-schemas   [--json]
//! cargo run -p xtask -- validate-fixtures  [--json]
//! cargo run -p xtask -- regen-embedded     [--check] [--json]
//! cargo run -p xtask -- check-all          [--write] [--json]
//! ```
//!
//! Exit code is `0` on success and `1` with a report on stderr otherwise.
//!
//! `check-all` is a gate, so it runs `stamp-manifest` and `regen-embedded` in
//! **check** mode: it reports drift rather than silently rewriting the tree
//! underneath a CI run. Pass `--write` to let it fix what it finds.

mod fixtures;
mod knowledge;
mod report;
mod schemas;

use report::Report;
use std::path::{Path, PathBuf};

/// The repository root, derived from this crate's manifest directory so the
/// commands work from any working directory.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

/// Parsed command-line flags.
struct Args {
    command: String,
    json: bool,
    check: bool,
    write: bool,
    knowledge_dir: Option<PathBuf>,
}

/// Reads the command line, returning a usage message on error.
fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut args = Args {
        command: String::new(),
        json: false,
        check: false,
        write: false,
        knowledge_dir: None,
    };
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => args.json = true,
            "--check" => args.check = true,
            "--write" => args.write = true,
            "--knowledge-dir" => {
                let p = it
                    .next()
                    .ok_or_else(|| "--knowledge-dir needs a path".to_string())?;
                args.knowledge_dir = Some(PathBuf::from(p));
            }
            "-h" | "--help" => args.command = "help".to_string(),
            other if other.starts_with('-') => {
                return Err(format!("unknown flag '{other}'"));
            }
            other if args.command.is_empty() => args.command = other.to_string(),
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }
    if args.command.is_empty() {
        args.command = "help".to_string();
    }
    Ok(args)
}

const USAGE: &str = "\
xtask — QLabs REAPER Music Intelligence MCP build commands

  validate-knowledge   full theory-kb validation of knowledge/ on disk and of
                       the embedded bundle; prints counts
  stamp-manifest       recompute knowledge/manifest.json's content_sha256
  validate-schemas     compile every schemas/*.schema.json and check every
                       knowledge and fixture file governed by one
  validate-fixtures    load every fixture through music-domain's loader
  regen-embedded       regenerate crates/theory-kb/src/embedded.rs
  check-all            all of the above

Flags:
  --knowledge-dir PATH  operate on an external knowledge directory
  --check               report drift instead of rewriting (stamp/regen)
  --write               let check-all fix drift instead of reporting it
  --json                emit a machine-readable report
";

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("xtask: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    if args.command == "help" {
        println!("{USAGE}");
        return;
    }

    let root = repo_root();
    let dir = args.knowledge_dir.as_deref();
    let report = match args.command.as_str() {
        "validate-knowledge" => knowledge::validate_knowledge(&root, dir),
        "stamp-manifest" => knowledge::stamp_manifest(&root, dir, args.check),
        "validate-schemas" => schemas::validate_schemas(&root),
        "validate-fixtures" => fixtures::validate_fixtures(&root),
        "regen-embedded" => knowledge::regen_embedded(&root, args.check),
        "check-all" => check_all(&root, dir, !args.write),
        other => {
            eprintln!("xtask: unknown command '{other}'\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    std::process::exit(report.emit(args.json));
}

/// Runs every command and folds the results into one report.
fn check_all(root: &Path, knowledge_dir: Option<&Path>, check_only: bool) -> Report {
    let mut report = Report::new("check-all");
    report.absorb(knowledge::regen_embedded(root, check_only));
    report.absorb(knowledge::stamp_manifest(root, knowledge_dir, check_only));
    report.absorb(knowledge::validate_knowledge(root, knowledge_dir));
    report.absorb(schemas::validate_schemas(root));
    report.absorb(fixtures::validate_fixtures(root));
    if report.ok {
        report.note("check-all: knowledge, schemas and fixtures are consistent");
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn repo_root_holds_the_workspace_manifest() {
        assert!(repo_root().join("Cargo.toml").is_file());
        assert!(repo_root().join("knowledge/manifest.json").is_file());
    }

    #[test]
    fn args_default_to_help() {
        assert_eq!(parse_args(&argv(&[])).unwrap().command, "help");
        assert_eq!(parse_args(&argv(&["--help"])).unwrap().command, "help");
    }

    #[test]
    fn args_parse_flags_in_any_order() {
        let a = parse_args(&argv(&[
            "--json",
            "stamp-manifest",
            "--knowledge-dir",
            "/tmp/kb",
            "--check",
        ]))
        .unwrap();
        assert_eq!(a.command, "stamp-manifest");
        assert!(a.json && a.check && !a.write);
        assert_eq!(a.knowledge_dir, Some(PathBuf::from("/tmp/kb")));
    }

    #[test]
    fn args_reject_unknown_flags_and_extra_positionals() {
        assert!(parse_args(&argv(&["--nope"])).is_err());
        assert!(parse_args(&argv(&["a", "b"])).is_err());
        assert!(parse_args(&argv(&["--knowledge-dir"])).is_err());
    }

    #[test]
    fn check_all_passes_on_the_repository() {
        let r = check_all(&repo_root(), None, true);
        assert!(r.ok, "{:#?}", r.problems);
        assert_eq!(
            r.to_json().get("ok").and_then(qjson::Json::as_bool),
            Some(true)
        );
    }

    #[test]
    fn a_failing_report_exits_non_zero() {
        let mut r = Report::new("x");
        assert_eq!(r.emit(true), 0);
        r.problem("p", "m");
        assert_eq!(r.emit(true), 1);
    }
}
