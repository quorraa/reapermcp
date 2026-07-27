//! The command line.
//!
//! Eight subcommands. Seven of them work with no REAPER and no bridge running;
//! only `serve` expects a client on stdin.
//!
//! ```text
//! qlabs-reaper-music-mcp serve [--config PATH] [--ipc-dir PATH] [--knowledge-dir PATH]
//! qlabs-reaper-music-mcp doctor [--json] [--config PATH] [--ipc-dir PATH]
//! qlabs-reaper-music-mcp validate-knowledge [--json] [--knowledge-dir PATH]
//! qlabs-reaper-music-mcp list-profiles [--json]
//! qlabs-reaper-music-mcp analyze-fixture <file> [--profile ID] [--json]
//! qlabs-reaper-music-mcp generate-fixture <file> [--profile ID] [--count N] [--seed N] [--json]
//! qlabs-reaper-music-mcp print-mcp-config [--json]
//! qlabs-reaper-music-mcp version [--json]
//! ```
//!
//! # Where output goes
//!
//! Under `serve`, stdout is reserved for JSON-RPC and every subcommand writes
//! its human output to stdout only because it is *not* serving a client at the
//! time. `serve` itself writes nothing to stdout except protocol frames.

use crate::config::ServerConfig;
use crate::server::{Knowledge, Server, ServerCore};
use qjson::{json_obj, Json};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

/// A parsed command line.
#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    /// The subcommand.
    pub command: String,
    /// The positional argument, when the subcommand takes one.
    pub positional: Option<String>,
    /// `--config`.
    pub config: Option<PathBuf>,
    /// `--ipc-dir`.
    pub ipc_dir: Option<PathBuf>,
    /// `--knowledge-dir`.
    pub knowledge_dir: Option<PathBuf>,
    /// `--profile`.
    pub profile: Option<String>,
    /// `--count`.
    pub count: Option<i64>,
    /// `--seed`.
    pub seed: Option<u64>,
    /// `--json`.
    pub json: bool,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            command: "help".to_string(),
            positional: None,
            config: None,
            ipc_dir: None,
            knowledge_dir: None,
            profile: None,
            count: None,
            seed: None,
            json: false,
        }
    }
}

/// The usage text.
pub const USAGE: &str = "\
qlabs-reaper-music-mcp — QLabs REAPER Music Intelligence MCP server

USAGE:
  qlabs-reaper-music-mcp <command> [options]

COMMANDS:
  serve                      Speak MCP JSON-RPC 2.0 over stdio (the default for MCP hosts).
  doctor                     Check the installation and report what is wrong.
  validate-knowledge         Validate the music-theory bundle and print its counts.
  list-profiles              List the style profiles this build knows.
  analyze-fixture <file>     Run the real analysis engine over a fixture, no REAPER needed.
  generate-fixture <file>    Run the real generation engine over a fixture, no REAPER needed.
  print-mcp-config           Print a ready-to-paste MCP host configuration.
  version                    Print version and protocol information.

OPTIONS:
  --config PATH              The bridge's config.json.
  --ipc-dir PATH             The bridge's IPC directory (overrides config.json).
  --knowledge-dir PATH       Load knowledge from this directory instead of the embedded bundle.
  --profile ID               Style profile for the fixture commands.
  --count N                  Candidate count for generate-fixture (1..8).
  --seed N                   Tie-breaking seed for generate-fixture.
  --json                     Emit JSON instead of human-readable text.
  -h, --help                 Print this message.

ENVIRONMENT:
  QLABS_MCP_CONFIG           Same as --config.
  QLABS_MCP_IPC_DIR          Same as --ipc-dir.
  QLABS_MCP_KNOWLEDGE_DIR    Same as --knowledge-dir.
  QLABS_MCP_LOG              off | error | warn | info | debug | trace (default warn).
";

/// Parses a command line, excluding the program name.
pub fn parse(argv: &[String]) -> Result<Args, String> {
    let mut args = Args::default();
    let mut it = argv.iter();
    let mut saw_command = false;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => args.json = true,
            "-h" | "--help" => {
                args.command = "help".to_string();
                return Ok(args);
            }
            "--config" => args.config = Some(PathBuf::from(need(&mut it, "--config")?)),
            "--ipc-dir" => args.ipc_dir = Some(PathBuf::from(need(&mut it, "--ipc-dir")?)),
            "--knowledge-dir" => {
                args.knowledge_dir = Some(PathBuf::from(need(&mut it, "--knowledge-dir")?))
            }
            "--profile" => args.profile = Some(need(&mut it, "--profile")?),
            "--count" => {
                let v = need(&mut it, "--count")?;
                args.count = Some(
                    v.parse::<i64>()
                        .map_err(|_| format!("--count needs an integer, got {v:?}"))?,
                );
            }
            "--seed" => {
                let v = need(&mut it, "--seed")?;
                args.seed = Some(
                    v.parse::<u64>()
                        .map_err(|_| format!("--seed needs a non-negative integer, got {v:?}"))?,
                );
            }
            other if other.starts_with('-') => return Err(format!("unknown flag {other:?}")),
            other if !saw_command => {
                args.command = other.to_string();
                saw_command = true;
            }
            other if args.positional.is_none() => args.positional = Some(other.to_string()),
            other => return Err(format!("unexpected argument {other:?}")),
        }
    }
    if !saw_command {
        args.command = "help".to_string();
    }
    Ok(args)
}

fn need(it: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, String> {
    it.next()
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Builds the server core the command needs.
pub fn build_core(args: &Args) -> Result<Arc<ServerCore>, String> {
    let config = ServerConfig::resolve(
        args.config.as_deref(),
        args.ipc_dir.as_deref(),
        args.knowledge_dir.as_deref(),
    );
    let knowledge = match &config.knowledge_dir {
        Some(dir) => {
            let kb = theory_kb::KnowledgeBase::load_dir(dir)
                .map_err(|e| format!("{}: {}: {}", dir.display(), e.path, e.message))?;
            Knowledge::External(Box::new(kb))
        }
        None => Knowledge::Embedded,
    };
    let bridge = crate::bridge::Bridge::new(&config, None);
    Ok(ServerCore::new(config, bridge, knowledge))
}

/// Runs the command line, returning the process exit code.
///
/// `out` is where non-`serve` output goes. `serve` ignores it and writes
/// protocol frames to stdout through [`crate::rpc::Writer`].
pub fn run(argv: &[String], out: &mut dyn Write) -> i32 {
    let args = match parse(argv) {
        Ok(a) => a,
        Err(e) => {
            crate::log::error(&e);
            let _ = writeln!(out, "{e}\n\n{USAGE}");
            return 2;
        }
    };

    match args.command.as_str() {
        "help" => {
            let _ = write!(out, "{USAGE}");
            0
        }
        "version" => {
            emit(out, args.json, &version_json(), &version_text());
            0
        }
        "serve" => serve(&args),
        "doctor" => doctor(&args, out),
        "validate-knowledge" => validate_knowledge(&args, out),
        "list-profiles" => list_profiles(&args, out),
        "analyze-fixture" => analyze_fixture(&args, out),
        "generate-fixture" => generate_fixture(&args, out),
        "print-mcp-config" => {
            let config = mcp_config_json(&args);
            emit(out, true, &config, &config.to_string_pretty());
            0
        }
        other => {
            let _ = writeln!(out, "unknown command {other:?}\n\n{USAGE}");
            2
        }
    }
}

fn emit(out: &mut dyn Write, json: bool, value: &Json, text: &str) {
    if json {
        let _ = writeln!(out, "{}", value.to_string_pretty());
    } else {
        let _ = write!(out, "{text}");
    }
}

fn serve(args: &Args) -> i32 {
    let core = match build_core(args) {
        Ok(c) => c,
        Err(e) => {
            crate::log::error(&format!("could not start: {e}"));
            return 1;
        }
    };
    let writer = crate::rpc::Writer::stdout();
    let server = Server::new(core, writer);
    let stdin = std::io::stdin();
    server.serve(std::io::BufReader::new(stdin));
    0
}

fn doctor(args: &Args, out: &mut dyn Write) -> i32 {
    let core = match build_core(args) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(out, "knowledge could not be loaded: {e}");
            return 1;
        }
    };
    let report = crate::doctor::run(&core);
    emit(out, args.json, &report.to_json(), &report.to_text());
    i32::from(!report.ok())
}

fn validate_knowledge(args: &Args, out: &mut dyn Write) -> i32 {
    let core = match build_core(args) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(out, "knowledge could not be loaded: {e}");
            return 1;
        }
    };
    let kb = core.knowledge.kb();
    let problems =
        theory_kb::validate::validate_report(kb, &theory_kb::validate::ValidateOptions::default());
    let mut counts = qjson::JsonMap::new();
    for (k, v) in kb.counts() {
        counts.insert(k, Json::Int(v as i64));
    }
    let value = json_obj! {
        "ok" => problems.is_empty(),
        "origin" => core.knowledge.origin(),
        "knowledge_version" => kb.version(),
        "schema_version" => kb.schema_version(),
        "content_sha256" => kb.content_hash(),
        "counts" => Json::Obj(counts),
        "problems" => Json::Arr(problems.iter().map(|p| json_obj! {
            "code" => p.code.clone(),
            "path" => p.path.clone(),
            "message" => p.message.clone(),
        }).collect()),
    };
    let mut text = format!(
        "knowledge {} ({}), schema {}, sha256 {}\n",
        kb.version(),
        core.knowledge.origin(),
        kb.schema_version(),
        kb.content_hash()
    );
    for (k, v) in kb.counts() {
        text.push_str(&format!("  {k}: {v}\n"));
    }
    if problems.is_empty() {
        text.push_str("valid\n");
    } else {
        for p in &problems {
            text.push_str(&format!("  PROBLEM {} {}: {}\n", p.code, p.path, p.message));
        }
        text.push_str(&format!("{} problem(s)\n", problems.len()));
    }
    emit(out, args.json, &value, &text);
    i32::from(!problems.is_empty())
}

fn list_profiles(args: &Args, out: &mut dyn Write) -> i32 {
    let core = match build_core(args) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(out, "knowledge could not be loaded: {e}");
            return 1;
        }
    };
    let kb = core.knowledge.kb();
    let mut rows = Vec::new();
    let mut text = String::new();
    for p in kb.profiles() {
        let resolved = kb.resolve_profile(&p.id).ok();
        let chain = resolved
            .as_ref()
            .map(|r| r.chain.join(" <- "))
            .unwrap_or_default();
        text.push_str(&format!("{:<20} {}\n", p.id, p.name));
        if !chain.is_empty() {
            text.push_str(&format!("{:<20} inherits: {chain}\n", ""));
        }
        rows.push(json_obj! {
            "id" => p.id.clone(),
            "name" => p.name.clone(),
            "parent" => match &p.parent {
                Some(x) => Json::Str(x.clone()),
                None => Json::Null,
            },
            "chain" => Json::Arr(
                resolved
                    .as_ref()
                    .map(|r| r.chain.iter().map(|c| Json::Str(c.clone())).collect())
                    .unwrap_or_default()
            ),
            "uri" => format!("theory://profiles/{}", p.id),
        });
    }
    let count = rows.len();
    let value = json_obj! {
        "ok" => true,
        "count" => count as i64,
        "profiles" => Json::Arr(rows),
    };
    text.push_str(&format!("{count} profile(s)\n"));
    emit(out, args.json, &value, &text);
    0
}

fn analyze_fixture(args: &Args, out: &mut dyn Write) -> i32 {
    let Some(path) = &args.positional else {
        let _ = writeln!(out, "analyze-fixture needs a fixture file\n\n{USAGE}");
        return 2;
    };
    let core = match build_core(args) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(out, "knowledge could not be loaded: {e}");
            return 1;
        }
    };
    let profile = args
        .profile
        .clone()
        .unwrap_or_else(|| crate::tools::DEFAULT_PROFILE.to_string());
    match crate::fixtures::analyze_fixture(&core, std::path::Path::new(path), &profile) {
        Ok(value) => {
            let text = analyze_text(&value);
            emit(out, args.json, &value, &text);
            0
        }
        Err(e) => {
            let _ = writeln!(out, "{}: {}", e.code, e.message);
            1
        }
    }
}

fn analyze_text(value: &Json) -> String {
    let analysis = value.get("analysis").cloned().unwrap_or(Json::Null);
    let mut text = format!(
        "fixture {} — {}\n",
        value.str_field("fixture").unwrap_or("?"),
        value.str_field("title").unwrap_or("")
    );
    text.push_str(&format!(
        "profile {}, {} notes, analysis {}\n",
        value.str_field("profile").unwrap_or("?"),
        value.i64_field("note_count").unwrap_or(0),
        analysis.str_field("analysis_id").unwrap_or("?")
    ));
    if let Some(key) = analysis.get("key") {
        text.push_str("key candidates:\n");
        for c in key
            .as_obj()
            .and_then(|_| key.arr_field("candidates").ok())
            .unwrap_or(&[])
        {
            text.push_str(&format!(
                "  {:<16} score {:.3} confidence {:.3}{}\n",
                c.str_field("label").unwrap_or("?"),
                c.f64_field("score").unwrap_or(0.0),
                c.f64_field("confidence").unwrap_or(0.0),
                if c.get("is_modal") == Some(&Json::Bool(true)) {
                    " (modal)"
                } else {
                    ""
                }
            ));
        }
    }
    text.push_str(&format!(
        "phrases {}, motives {}, grid slots {}, confidence {:.3}\n",
        analysis.arr_field("phrases").map(|p| p.len()).unwrap_or(0),
        analysis.arr_field("motives").map(|p| p.len()).unwrap_or(0),
        analysis
            .get("grid")
            .and_then(|g| g.i64_field("slot_count").ok())
            .unwrap_or(0),
        analysis.f64_field("confidence").unwrap_or(0.0),
    ));
    text
}

fn generate_fixture(args: &Args, out: &mut dyn Write) -> i32 {
    let Some(path) = &args.positional else {
        let _ = writeln!(out, "generate-fixture needs a fixture file\n\n{USAGE}");
        return 2;
    };
    let core = match build_core(args) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(out, "knowledge could not be loaded: {e}");
            return 1;
        }
    };
    let profile = args
        .profile
        .clone()
        .unwrap_or_else(|| crate::tools::DEFAULT_PROFILE.to_string());
    let count = args.count.unwrap_or(3).clamp(1, 8);
    let seed = args.seed.unwrap_or(crate::tools::DEFAULT_SEED);
    match crate::fixtures::generate_fixture(
        &core,
        std::path::Path::new(path),
        &profile,
        count,
        seed,
    ) {
        Ok(value) => {
            let text = generate_text(&value);
            emit(out, args.json, &value, &text);
            0
        }
        Err(e) => {
            let _ = writeln!(out, "{}: {}", e.code, e.message);
            1
        }
    }
}

fn generate_text(value: &Json) -> String {
    let generation = value.get("generation").cloned().unwrap_or(Json::Null);
    let mut text = format!(
        "fixture {} — {}\nprofile {}, seed {}\n",
        value.str_field("fixture").unwrap_or("?"),
        value.str_field("title").unwrap_or(""),
        value.str_field("profile").unwrap_or("?"),
        value.i64_field("seed").unwrap_or(0),
    );
    for c in generation.arr_field("candidates").unwrap_or(&[]) {
        text.push_str(&format!(
            "\n{} [{}] score {:.3} confidence {:.3}\n  {}\n  rules: {}\n",
            c.str_field("label").unwrap_or("?"),
            c.str_field("strategy").unwrap_or("?"),
            c.f64_field("score_total").unwrap_or(0.0),
            c.f64_field("confidence").unwrap_or(0.0),
            c.arr_field("chords")
                .map(|a| a
                    .iter()
                    .filter_map(Json::as_str)
                    .collect::<Vec<_>>()
                    .join(" | "))
                .unwrap_or_default(),
            c.arr_field("rule_ids")
                .map(|a| a
                    .iter()
                    .filter_map(Json::as_str)
                    .take(6)
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default(),
        ));
    }
    text
}

/// The `version` JSON body.
pub fn version_json() -> Json {
    json_obj! {
        "name" => crate::SERVER_NAME,
        "version" => crate::SERVER_VERSION,
        "mcp_protocol_version" => crate::MCP_PROTOCOL_VERSION,
        "ipc_protocol_version" => reaper_ipc::PROTOCOL_VERSION,
        "bridge_version_expected" => reaper_ipc::BRIDGE_VERSION,
        "knowledge_version" => theory_kb::KnowledgeBase::embedded().version(),
        "knowledge_sha256" => theory_kb::KnowledgeBase::embedded().content_hash(),
        "tool_count" => crate::schema_gen::tool_specs().len() as i64,
        "resource_count" => 13,
        "prompt_count" => crate::prompts::prompt_specs().len() as i64,
    }
}

fn version_text() -> String {
    let kb = theory_kb::KnowledgeBase::embedded();
    format!(
        "{} {}\n  MCP protocol:  {}\n  IPC protocol:  {}\n  bridge:        {}\n  knowledge:     \
         {} ({})\n  surface:       {} tools, 13 resources, {} prompts\n",
        crate::SERVER_NAME,
        crate::SERVER_VERSION,
        crate::MCP_PROTOCOL_VERSION,
        reaper_ipc::PROTOCOL_VERSION,
        reaper_ipc::BRIDGE_VERSION,
        kb.version(),
        &kb.content_hash()[..16],
        crate::schema_gen::tool_specs().len(),
        crate::prompts::prompt_specs().len(),
    )
}

/// The `print-mcp-config` body: a generic MCP host configuration.
pub fn mcp_config_json(args: &Args) -> Json {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| crate::BINARY_NAME.to_string());
    let mut command_args = vec![Json::Str("serve".into())];
    if let Some(c) = &args.config {
        command_args.push(Json::Str("--config".into()));
        command_args.push(Json::Str(c.display().to_string()));
    } else {
        command_args.push(Json::Str("--config".into()));
        command_args.push(Json::Str(
            "<REAPER resource path>/Scripts/QLabs-Reaper-MCP/config.json".into(),
        ));
    }
    json_obj! {
        "mcpServers" => json_obj! {
            "qlabs-reaper-music" => json_obj! {
                "command" => exe,
                "args" => Json::Arr(command_args),
                "env" => json_obj! { "QLABS_MCP_LOG" => "warn" },
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(argv: &[&str]) -> (i32, String) {
        let owned: Vec<String> = argv.iter().map(|s| (*s).to_string()).collect();
        let mut buf: Vec<u8> = Vec::new();
        let code = run(&owned, &mut buf);
        (code, String::from_utf8(buf).expect("utf-8"))
    }

    #[test]
    fn parsing_reads_every_flag() {
        let argv: Vec<String> = [
            "generate-fixture",
            "a.json",
            "--profile",
            "blues",
            "--count",
            "4",
            "--seed",
            "9",
            "--json",
            "--config",
            "/c.json",
            "--ipc-dir",
            "/ipc",
            "--knowledge-dir",
            "/k",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let a = parse(&argv).unwrap();
        assert_eq!(a.command, "generate-fixture");
        assert_eq!(a.positional.as_deref(), Some("a.json"));
        assert_eq!(a.profile.as_deref(), Some("blues"));
        assert_eq!(a.count, Some(4));
        assert_eq!(a.seed, Some(9));
        assert!(a.json);
        assert_eq!(a.config, Some(PathBuf::from("/c.json")));
        assert_eq!(a.ipc_dir, Some(PathBuf::from("/ipc")));
        assert_eq!(a.knowledge_dir, Some(PathBuf::from("/k")));
    }

    #[test]
    fn an_unknown_flag_is_an_error() {
        assert!(parse(&["serve".to_string(), "--wat".to_string()]).is_err());
    }

    #[test]
    fn a_flag_without_a_value_is_an_error() {
        assert!(parse(&["doctor".to_string(), "--config".to_string()]).is_err());
        assert!(parse(&["generate-fixture".to_string(), "--count".to_string()]).is_err());
    }

    #[test]
    fn a_non_numeric_count_is_an_error() {
        let argv: Vec<String> = ["generate-fixture", "--count", "many"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert!(parse(&argv).is_err());
    }

    #[test]
    fn no_arguments_prints_usage() {
        let (code, text) = capture(&[]);
        assert_eq!(code, 0);
        assert!(text.contains("USAGE"));
    }

    #[test]
    fn version_reports_both_protocols() {
        let (code, text) = capture(&["version"]);
        assert_eq!(code, 0);
        assert!(text.contains(crate::MCP_PROTOCOL_VERSION));
        assert!(text.contains(reaper_ipc::PROTOCOL_VERSION));
        let (code, json) = capture(&["version", "--json"]);
        assert_eq!(code, 0);
        let v = Json::parse(&json).unwrap();
        assert_eq!(
            v.str_field("mcp_protocol_version").unwrap(),
            crate::MCP_PROTOCOL_VERSION
        );
        assert_eq!(v.i64_field("tool_count"), Ok(14));
        assert_eq!(v.i64_field("resource_count"), Ok(13));
        assert_eq!(v.i64_field("prompt_count"), Ok(9));
    }

    #[test]
    fn doctor_runs_without_a_bridge_and_exits_zero() {
        let (code, text) = capture(&["doctor"]);
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("installation looks healthy"));
        let (code, json) = capture(&["doctor", "--json"]);
        assert_eq!(code, 0);
        let v = Json::parse(&json).unwrap();
        assert_eq!(v.get("ok"), Some(&Json::Bool(true)));
    }

    #[test]
    fn validate_knowledge_passes_on_the_embedded_bundle() {
        let (code, text) = capture(&["validate-knowledge"]);
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("valid"));
        let (code, json) = capture(&["validate-knowledge", "--json"]);
        assert_eq!(code, 0);
        let v = Json::parse(&json).unwrap();
        assert_eq!(v.get("ok"), Some(&Json::Bool(true)));
        assert!(v.arr_field("problems").unwrap().is_empty());
    }

    #[test]
    fn list_profiles_reports_all_ten() {
        let (code, json) = capture(&["list-profiles", "--json"]);
        assert_eq!(code, 0);
        let v = Json::parse(&json).unwrap();
        assert_eq!(v.i64_field("count"), Ok(10));
        for id in crate::schema_gen::PROFILE_IDS {
            assert!(json.contains(id), "{id} missing");
        }
    }

    #[test]
    fn analyze_fixture_runs_the_engine() {
        let path = crate::fixtures::fixtures_dir().join("melodies/eight_bar_c_major.json");
        let (code, text) = capture(&["analyze-fixture", path.to_str().unwrap()]);
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("key candidates"));
    }

    #[test]
    fn generate_fixture_runs_the_engine() {
        let path = crate::fixtures::fixtures_dir().join("melodies/eight_bar_c_major.json");
        let (code, json) = capture(&[
            "generate-fixture",
            path.to_str().unwrap(),
            "--count",
            "2",
            "--seed",
            "3",
            "--json",
        ]);
        assert_eq!(code, 0, "{json}");
        let v = Json::parse(&json).unwrap();
        assert_eq!(
            v.get("generation").unwrap().i64_field("candidate_count"),
            Ok(2)
        );
    }

    #[test]
    fn a_missing_fixture_argument_is_a_usage_error() {
        let (code, text) = capture(&["analyze-fixture"]);
        assert_eq!(code, 2);
        assert!(text.contains("needs a fixture file"));
        let (code, _) = capture(&["generate-fixture"]);
        assert_eq!(code, 2);
    }

    #[test]
    fn a_nonexistent_fixture_is_a_clean_failure() {
        let (code, text) = capture(&["analyze-fixture", "/nonexistent/x.json"]);
        assert_eq!(code, 1);
        assert!(text.contains("INVALID_ARGUMENT"));
    }

    #[test]
    fn print_mcp_config_is_valid_json_naming_serve() {
        let (code, text) = capture(&["print-mcp-config"]);
        assert_eq!(code, 0);
        let v = Json::parse(&text).unwrap();
        let entry = v
            .get("mcpServers")
            .unwrap()
            .get("qlabs-reaper-music")
            .unwrap();
        assert!(entry.get("command").is_some());
        assert_eq!(
            entry.arr_field("args").unwrap()[0],
            Json::Str("serve".into())
        );
    }

    #[test]
    fn an_unknown_command_is_a_usage_error() {
        let (code, text) = capture(&["frobnicate"]);
        assert_eq!(code, 2);
        assert!(text.contains("unknown command"));
    }

    #[test]
    fn an_unreadable_knowledge_dir_fails_cleanly() {
        let (code, text) = capture(&["validate-knowledge", "--knowledge-dir", "/nonexistent"]);
        assert_eq!(code, 1);
        assert!(text.contains("knowledge could not be loaded"));
    }

    #[test]
    fn the_usage_text_documents_every_command() {
        for c in [
            "serve",
            "doctor",
            "validate-knowledge",
            "list-profiles",
            "analyze-fixture",
            "generate-fixture",
            "print-mcp-config",
            "version",
        ] {
            assert!(USAGE.contains(c), "{c} is undocumented");
        }
    }
}
