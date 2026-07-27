//! `doctor` — the installation self-check.
//!
//! Every check runs whether or not REAPER is present, and a check that cannot
//! be evaluated without the bridge reports `skipped` rather than `fail`. The
//! exit status therefore means "this installation is broken", never "REAPER is
//! not open".

use crate::config::ServerConfig;
use crate::server::ServerCore;
use qjson::{json_obj, Json};
use std::path::Path;

/// How a single check came out.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The check passed.
    Pass,
    /// The check found something the user should know about, but the
    /// installation still works.
    Warn,
    /// The installation is broken.
    Fail,
    /// The check needs a running bridge and there is not one.
    Skipped,
}

impl Status {
    /// The stable identifier.
    pub fn id(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Warn => "warn",
            Status::Fail => "fail",
            Status::Skipped => "skipped",
        }
    }

    /// The glyph used in the human-readable report.
    pub fn glyph(self) -> &'static str {
        match self {
            Status::Pass => "ok  ",
            Status::Warn => "warn",
            Status::Fail => "FAIL",
            Status::Skipped => "skip",
        }
    }
}

/// One check.
#[derive(Clone, Debug)]
pub struct Check {
    /// Stable check identifier.
    pub id: String,
    /// What was checked.
    pub title: String,
    /// The outcome.
    pub status: Status,
    /// What was found.
    pub detail: String,
    /// What to do about it, when there is something to do.
    pub remedy: Option<String>,
}

impl Check {
    fn new(id: &str, title: &str, status: Status, detail: impl Into<String>) -> Check {
        Check {
            id: id.to_string(),
            title: title.to_string(),
            status,
            detail: detail.into(),
            remedy: None,
        }
    }

    fn with_remedy(mut self, remedy: impl Into<String>) -> Check {
        self.remedy = Some(remedy.into());
        self
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "id" => self.id.clone(),
            "title" => self.title.clone(),
            "status" => self.status.id(),
            "detail" => self.detail.clone(),
            "remedy" => match &self.remedy {
                Some(r) => Json::Str(r.clone()),
                None => Json::Null,
            },
        }
    }
}

/// The whole report.
#[derive(Clone, Debug)]
pub struct Report {
    /// Every check, in run order.
    pub checks: Vec<Check>,
}

impl Report {
    /// True when nothing failed.
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|c| c.status != Status::Fail)
    }

    /// How many checks came out each way.
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let count = |s: Status| self.checks.iter().filter(|c| c.status == s).count();
        (
            count(Status::Pass),
            count(Status::Warn),
            count(Status::Fail),
            count(Status::Skipped),
        )
    }

    /// The `--json` form.
    pub fn to_json(&self) -> Json {
        let (pass, warn, fail, skipped) = self.counts();
        json_obj! {
            "ok" => self.ok(),
            "server_name" => crate::SERVER_NAME,
            "server_version" => crate::SERVER_VERSION,
            "mcp_protocol_version" => crate::MCP_PROTOCOL_VERSION,
            "ipc_protocol_version" => reaper_ipc::PROTOCOL_VERSION,
            "generated_at" => qjson::time::now_iso8601(),
            "summary" => json_obj! {
                "pass" => pass as i64,
                "warn" => warn as i64,
                "fail" => fail as i64,
                "skipped" => skipped as i64,
                "total" => self.checks.len() as i64,
            },
            "checks" => Json::Arr(self.checks.iter().map(Check::to_json).collect()),
        }
    }

    /// The human-readable form.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{} {} (MCP {}, IPC {})\n\n",
            crate::SERVER_NAME,
            crate::SERVER_VERSION,
            crate::MCP_PROTOCOL_VERSION,
            reaper_ipc::PROTOCOL_VERSION
        ));
        for c in &self.checks {
            out.push_str(&format!(
                "[{}] {}: {}\n",
                c.status.glyph(),
                c.title,
                c.detail
            ));
            if let Some(r) = &c.remedy {
                out.push_str(&format!("       -> {r}\n"));
            }
        }
        let (pass, warn, fail, skipped) = self.counts();
        out.push_str(&format!(
            "\n{pass} passed, {warn} warned, {fail} failed, {skipped} skipped — {}\n",
            if self.ok() {
                "installation looks healthy"
            } else {
                "installation is not usable"
            }
        ));
        out
    }
}

/// Runs every check.
pub fn run(core: &ServerCore) -> Report {
    let mut checks = Vec::new();
    checks.push(check_version());
    checks.push(check_configuration(&core.config));
    checks.extend(check_ipc_directory(core));
    checks.push(check_instance_token(&core.config));
    checks.extend(check_knowledge(core));
    checks.push(check_schemas());
    checks.push(check_tool_surface(core));
    checks.extend(check_bridge(core));
    Report { checks }
}

fn check_version() -> Check {
    Check::new(
        "server_version",
        "MCP server version",
        Status::Pass,
        format!(
            "{} {} implementing MCP {}",
            crate::SERVER_NAME,
            crate::SERVER_VERSION,
            crate::MCP_PROTOCOL_VERSION
        ),
    )
}

fn check_configuration(config: &ServerConfig) -> Check {
    match (&config.ipc, &config.ipc_problem) {
        (Some(_), _) => Check::new(
            "configuration",
            "Configuration",
            Status::Pass,
            config.source.describe(),
        ),
        (None, Some(e)) => Check::new(
            "configuration",
            "Configuration",
            Status::Warn,
            format!("{}: {}", e.code, e.message),
        )
        .with_remedy("pass --config <REAPER>/Scripts/QLabs-Reaper-MCP/config.json"),
        (None, None) => Check::new(
            "configuration",
            "Configuration",
            Status::Warn,
            "no bridge configured; REAPER-free commands still work",
        )
        .with_remedy("pass --config or --ipc-dir to reach REAPER"),
    }
}

fn check_ipc_directory(core: &ServerCore) -> Vec<Check> {
    let Some(dir) = core.bridge.ipc_dir() else {
        return vec![Check::new(
            "ipc_directory",
            "IPC directory",
            Status::Skipped,
            "no IPC directory is configured",
        )];
    };
    let mut checks = Vec::new();
    if !dir.exists() {
        checks.push(
            Check::new(
                "ipc_directory",
                "IPC directory",
                Status::Warn,
                format!("{} does not exist yet", dir.display()),
            )
            .with_remedy("start the bridge once; it creates the whole tree"),
        );
        return checks;
    }
    checks.push(Check::new(
        "ipc_directory",
        "IPC directory",
        Status::Pass,
        dir.display().to_string(),
    ));

    checks.push(check_writable(
        &dir.join("commands"),
        "commands_writable",
        "Command directory",
    ));
    checks.push(check_readable(
        &dir.join("results"),
        "results_readable",
        "Result directory",
    ));
    checks.push(check_stale_commands(dir));
    checks
}

fn check_writable(path: &Path, id: &str, title: &str) -> Check {
    if let Err(e) = std::fs::create_dir_all(path) {
        return Check::new(
            id,
            title,
            Status::Fail,
            format!("{} cannot be created: {e}", path.display()),
        )
        .with_remedy("check the filesystem permissions on the REAPER resource path");
    }
    let probe = path.join(".qlabs-doctor-probe");
    match std::fs::write(&probe, b"probe") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Check::new(
                id,
                title,
                Status::Pass,
                format!("{} is writable", path.display()),
            )
        }
        Err(e) => Check::new(
            id,
            title,
            Status::Fail,
            format!("{} is not writable: {e}", path.display()),
        )
        .with_remedy("grant write access to the IPC directory"),
    }
}

fn check_readable(path: &Path, id: &str, title: &str) -> Check {
    match std::fs::read_dir(path) {
        Ok(_) => Check::new(
            id,
            title,
            Status::Pass,
            format!("{} is readable", path.display()),
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Check::new(
            id,
            title,
            Status::Warn,
            format!("{} does not exist yet", path.display()),
        )
        .with_remedy("start the bridge once; it creates the whole tree"),
        Err(e) => Check::new(
            id,
            title,
            Status::Fail,
            format!("{} is not readable: {e}", path.display()),
        ),
    }
}

fn check_stale_commands(dir: &Path) -> Check {
    let commands = dir.join("commands");
    let Ok(entries) = std::fs::read_dir(&commands) else {
        return Check::new(
            "stale_commands",
            "Stale commands",
            Status::Skipped,
            "the command directory could not be listed",
        );
    };
    let now = std::time::SystemTime::now();
    let mut stale = 0usize;
    let mut total = 0usize;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.ends_with(reaper_ipc::limits::SUFFIX_COMMAND) {
            continue;
        }
        total += 1;
        if let Ok(meta) = e.metadata() {
            if let Ok(modified) = meta.modified() {
                if now
                    .duration_since(modified)
                    .map(|d| d.as_secs() as i64 > reaper_ipc::limits::STALE_COMMAND_SECONDS)
                    .unwrap_or(false)
                {
                    stale += 1;
                }
            }
        }
    }
    if stale > 0 {
        Check::new(
            "stale_commands",
            "Stale commands",
            Status::Warn,
            format!("{stale} of {total} queued commands are older than the bridge's stale window"),
        )
        .with_remedy("start the bridge; it garbage-collects its own directories")
    } else {
        Check::new(
            "stale_commands",
            "Stale commands",
            Status::Pass,
            format!("{total} queued command file(s), none stale"),
        )
    }
}

fn check_instance_token(config: &ServerConfig) -> Check {
    match &config.ipc {
        Some(ipc) if !ipc.instance_token.is_empty() => Check::new(
            "instance_token",
            "Installation token",
            Status::Pass,
            format!(
                "present, {} bytes (value not shown)",
                ipc.instance_token.len()
            ),
        ),
        Some(_) => Check::new(
            "instance_token",
            "Installation token",
            Status::Fail,
            "the configured token is empty",
        )
        .with_remedy("delete config.json and start the bridge so it mints a token"),
        None => Check::new(
            "instance_token",
            "Installation token",
            Status::Skipped,
            "no configuration to read a token from",
        ),
    }
}

fn check_knowledge(core: &ServerCore) -> Vec<Check> {
    let kb = core.knowledge.kb();
    let mut checks = Vec::new();

    let problems =
        theory_kb::validate::validate_report(kb, &theory_kb::validate::ValidateOptions::default());
    if problems.is_empty() {
        let counts = kb.counts();
        let summary = counts
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ");
        checks.push(Check::new(
            "knowledge_valid",
            "Knowledge validation",
            Status::Pass,
            format!("{} ({summary})", core.knowledge.origin()),
        ));
    } else {
        checks.push(
            Check::new(
                "knowledge_valid",
                "Knowledge validation",
                Status::Fail,
                format!(
                    "{} problem(s), first: {}",
                    problems.len(),
                    problems
                        .first()
                        .map(|p| format!("{}: {}", p.path, p.message))
                        .unwrap_or_default()
                ),
            )
            .with_remedy("run `cargo run -p xtask -- validate-knowledge` for the full report"),
        );
    }

    let hash = kb.content_hash();
    let manifest_hash = &kb.manifest().content_sha256;
    let status = if manifest_hash == theory_kb::PENDING_HASH {
        Status::Warn
    } else if manifest_hash == hash {
        Status::Pass
    } else {
        Status::Fail
    };
    checks.push(
        Check::new(
            "knowledge_hash",
            "Knowledge hash",
            status,
            format!(
                "version {}, recomputed {}, manifest {}",
                kb.version(),
                &hash[..16.min(hash.len())],
                &manifest_hash[..16.min(manifest_hash.len())]
            ),
        )
        .with_remedy("run `cargo run -p xtask -- stamp-manifest` after changing knowledge/"),
    );
    checks
}

fn check_schemas() -> Check {
    match crate::schema_gen::ToolRegistry::compile() {
        Ok(r) => Check::new(
            "tool_schemas",
            "Tool schemas",
            Status::Pass,
            format!("{} tools, input and output schemas compile", r.all().len()),
        ),
        Err(e) => Check::new("tool_schemas", "Tool schemas", Status::Fail, e.message)
            .with_remedy("this is a build defect; please report it"),
    }
}

fn check_tool_surface(core: &ServerCore) -> Check {
    let listed = core.tools.list_json().to_string();
    let leaked: Vec<&str> = crate::schema_gen::PROHIBITED_TOOL_NAMES
        .iter()
        .copied()
        .filter(|n| listed.contains(n))
        .collect();
    if leaked.is_empty() {
        Check::new(
            "tool_surface",
            "Prohibited tools",
            Status::Pass,
            format!(
                "none of the {} prohibited names appear in the tool surface",
                crate::schema_gen::PROHIBITED_TOOL_NAMES.len()
            ),
        )
    } else {
        Check::new(
            "tool_surface",
            "Prohibited tools",
            Status::Fail,
            format!("prohibited names are exposed: {}", leaked.join(", ")),
        )
    }
}

fn check_bridge(core: &ServerCore) -> Vec<Check> {
    if !core.bridge.is_configured() {
        return vec![
            Check::new(
                "bridge_heartbeat",
                "Bridge heartbeat",
                Status::Skipped,
                "no bridge is configured",
            ),
            Check::new(
                "bridge_version",
                "Bridge version",
                Status::Skipped,
                "no bridge is configured",
            ),
            Check::new(
                "reaper_version",
                "REAPER version",
                Status::Skipped,
                "no bridge is configured",
            ),
        ];
    }
    match core.bridge.heartbeat() {
        Ok(h) => {
            let version_status = if h.bridge_version == reaper_ipc::BRIDGE_VERSION {
                Status::Pass
            } else {
                Status::Warn
            };
            vec![
                Check::new(
                    "bridge_heartbeat",
                    "Bridge heartbeat",
                    Status::Pass,
                    format!("{:.1}s old", h.age.as_secs_f64()),
                ),
                Check::new(
                    "bridge_version",
                    "Bridge version",
                    version_status,
                    format!(
                        "bridge {}, this build expects {}",
                        h.bridge_version,
                        reaper_ipc::BRIDGE_VERSION
                    ),
                ),
                Check::new(
                    "reaper_version",
                    "REAPER version",
                    Status::Pass,
                    h.reaper_version.clone(),
                ),
            ]
        }
        Err(e) => vec![
            Check::new(
                "bridge_heartbeat",
                "Bridge heartbeat",
                Status::Warn,
                format!("{}: {}", e.code, e.message),
            )
            .with_remedy("open REAPER and run the QLabs_Reaper_MCP_Bridge.lua action"),
            Check::new(
                "bridge_version",
                "Bridge version",
                Status::Skipped,
                "the bridge is not answering",
            ),
            Check::new(
                "reaper_version",
                "REAPER version",
                Status::Skipped,
                "the bridge is not answering",
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_passes_with_no_bridge_present() {
        let core = ServerCore::offline();
        let report = run(&core);
        assert!(report.ok(), "{}", report.to_text());
        let (pass, _warn, fail, skipped) = report.counts();
        assert!(pass > 0);
        assert_eq!(fail, 0);
        assert!(skipped > 0, "bridge checks must be skipped, not failed");
    }

    #[test]
    fn the_json_form_carries_every_check() {
        let core = ServerCore::offline();
        let report = run(&core);
        let v = report.to_json();
        assert_eq!(v.get("ok"), Some(&Json::Bool(true)));
        assert_eq!(v.arr_field("checks").unwrap().len(), report.checks.len());
        assert_eq!(
            v.get("summary").unwrap().i64_field("total"),
            Ok(report.checks.len() as i64)
        );
        assert_eq!(
            v.str_field("mcp_protocol_version").unwrap(),
            crate::MCP_PROTOCOL_VERSION
        );
    }

    #[test]
    fn the_brief_checklist_is_covered() {
        let core = ServerCore::offline();
        let report = run(&core);
        let ids: Vec<&str> = report.checks.iter().map(|c| c.id.as_str()).collect();
        for required in [
            "server_version",
            "configuration",
            "instance_token",
            "knowledge_valid",
            "knowledge_hash",
            "bridge_heartbeat",
            "bridge_version",
            "reaper_version",
            "tool_schemas",
        ] {
            assert!(ids.contains(&required), "doctor is missing {required}");
        }
    }

    #[test]
    fn the_text_form_is_readable_and_names_the_outcome() {
        let core = ServerCore::offline();
        let text = run(&core).to_text();
        assert!(text.contains(crate::SERVER_NAME));
        assert!(text.contains("installation looks healthy"));
    }

    #[test]
    fn the_token_value_is_never_printed() {
        let dir = std::env::temp_dir().join(format!("qlabs-doctor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.json"),
            r#"{"instance_token":"cafebabecafebabecafebabecafebabe"}"#,
        )
        .unwrap();
        let config = ServerConfig::resolve(Some(&dir.join("config.json")), None, None);
        let bridge = crate::bridge::Bridge::new(&config, None);
        let core = ServerCore::new(config, bridge, crate::server::Knowledge::Embedded);
        let report = run(&core);
        assert!(!report.to_text().contains("cafebabe"));
        assert!(!report.to_json().to_string().contains("cafebabe"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn statuses_have_stable_identifiers() {
        for s in [Status::Pass, Status::Warn, Status::Fail, Status::Skipped] {
            assert!(!s.id().is_empty());
            assert_eq!(s.glyph().len(), 4);
        }
    }
}
