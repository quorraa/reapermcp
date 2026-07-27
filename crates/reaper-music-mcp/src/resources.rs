//! MCP resources and the URI parser that keeps them safe.
//!
//! Thirteen resources across six schemes. Six are concrete and appear in
//! `resources/list`; seven are templated and appear in
//! `resources/templates/list`.
//!
//! # A resource URI can never name a file
//!
//! [`ResourceUri::parse`] is a **closed** parser: the scheme must be one of six
//! literals, the path must match one of thirteen shapes, and every variable
//! segment must satisfy [`is_safe_segment`] — leading alphanumeric, then only
//! `A-Za-z0-9._-`, at most 128 bytes, and no `..` anywhere. That excludes `/`,
//! `\`, `%`, `~`, drive letters, absolute paths, UNC paths and every
//! percent-encoded escape of the same. There is no fallback branch that reads
//! from disk, so even a segment that satisfied the pattern could only ever
//! index the in-memory session store.
//!
//! Path-escape attempts are asserted explicitly in
//! `a_resource_uri_can_never_name_a_path`.

use crate::error::{codes, ToolError};
use crate::rpc::{rpc_codes, RpcError};
use crate::server::ServerCore;
use qjson::{json_obj, Json};
use theory_kb::RuleDomain;

/// The JSON-RPC error code MCP uses for "resource not found".
pub const RESOURCE_NOT_FOUND: i64 = -32002;

/// The MIME type every resource in this server serves.
pub const MIME_JSON: &str = "application/json";

/// The maximum length of a variable URI segment.
pub const MAX_SEGMENT: usize = 128;

/// True when `s` is usable as a variable segment of a resource URI.
///
/// Deliberately identical to the bridge's request-id pattern: one vocabulary
/// for "an identifier this system will accept", and one that cannot express a
/// path component.
pub fn is_safe_segment(s: &str) -> bool {
    if s.is_empty() || s.len() > MAX_SEGMENT || s.contains("..") {
        return false;
    }
    let Some(first) = s.chars().next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// Every resource this server serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceUri {
    /// `reaper://status`
    ReaperStatus,
    /// `reaper://project/current`
    ReaperProject,
    /// `reaper://selection/current`
    ReaperSelection,
    /// `theory://catalog`
    TheoryCatalog,
    /// `theory://sources`
    TheorySources,
    /// `theory://profiles`
    TheoryProfiles,
    /// `theory://profiles/{profile_id}`
    TheoryProfile(String),
    /// `theory://rules/{domain}`
    TheoryRules(String),
    /// `analysis://{analysis_id}`
    Analysis(String),
    /// `candidate://{candidate_id}`
    Candidate(String),
    /// `candidate://{candidate_id}/trace`
    CandidateTrace(String),
    /// `editplan://{plan_id}`
    EditPlan(String),
    /// `transaction://{transaction_id}`
    Transaction(String),
}

fn unknown(uri: &str, why: &str) -> ToolError {
    ToolError::with_details(
        codes::UNKNOWN_RESOURCE,
        format!("{uri} is not a resource this server serves"),
        json_obj! { "uri" => uri, "reason" => why },
    )
    .remedy("call resources/list and resources/templates/list for the served set")
}

impl ResourceUri {
    /// Parses a resource URI, rejecting everything not on the closed list.
    pub fn parse(uri: &str) -> Result<ResourceUri, ToolError> {
        if uri.len() > 512 {
            return Err(unknown(uri, "too long"));
        }
        let Some((scheme, rest)) = uri.split_once("://") else {
            return Err(unknown(uri, "no scheme"));
        };
        let segments: Vec<&str> = rest.split('/').collect();
        match (scheme, segments.as_slice()) {
            ("reaper", ["status"]) => Ok(ResourceUri::ReaperStatus),
            ("reaper", ["project", "current"]) => Ok(ResourceUri::ReaperProject),
            ("reaper", ["selection", "current"]) => Ok(ResourceUri::ReaperSelection),
            ("theory", ["catalog"]) => Ok(ResourceUri::TheoryCatalog),
            ("theory", ["sources"]) => Ok(ResourceUri::TheorySources),
            ("theory", ["profiles"]) => Ok(ResourceUri::TheoryProfiles),
            ("theory", ["profiles", id]) if is_safe_segment(id) => {
                Ok(ResourceUri::TheoryProfile((*id).to_string()))
            }
            ("theory", ["rules", domain]) if is_safe_segment(domain) => {
                Ok(ResourceUri::TheoryRules((*domain).to_string()))
            }
            ("analysis", [id]) if is_safe_segment(id) => {
                Ok(ResourceUri::Analysis((*id).to_string()))
            }
            ("candidate", [id]) if is_safe_segment(id) => {
                Ok(ResourceUri::Candidate((*id).to_string()))
            }
            ("candidate", [id, "trace"]) if is_safe_segment(id) => {
                Ok(ResourceUri::CandidateTrace((*id).to_string()))
            }
            ("editplan", [id]) if is_safe_segment(id) => {
                Ok(ResourceUri::EditPlan((*id).to_string()))
            }
            ("transaction", [id]) if is_safe_segment(id) => {
                Ok(ResourceUri::Transaction((*id).to_string()))
            }
            _ => Err(unknown(uri, "unrecognised shape or unsafe segment")),
        }
    }

    /// The canonical text form.
    pub fn to_uri(&self) -> String {
        match self {
            ResourceUri::ReaperStatus => "reaper://status".into(),
            ResourceUri::ReaperProject => "reaper://project/current".into(),
            ResourceUri::ReaperSelection => "reaper://selection/current".into(),
            ResourceUri::TheoryCatalog => "theory://catalog".into(),
            ResourceUri::TheorySources => "theory://sources".into(),
            ResourceUri::TheoryProfiles => "theory://profiles".into(),
            ResourceUri::TheoryProfile(id) => format!("theory://profiles/{id}"),
            ResourceUri::TheoryRules(d) => format!("theory://rules/{d}"),
            ResourceUri::Analysis(id) => format!("analysis://{id}"),
            ResourceUri::Candidate(id) => format!("candidate://{id}"),
            ResourceUri::CandidateTrace(id) => format!("candidate://{id}/trace"),
            ResourceUri::EditPlan(id) => format!("editplan://{id}"),
            ResourceUri::Transaction(id) => format!("transaction://{id}"),
        }
    }
}

fn concrete_entry(uri: &str, name: &str, title: &str, description: &str) -> Json {
    json_obj! {
        "uri" => uri,
        "name" => name,
        "title" => title,
        "description" => description,
        "mimeType" => MIME_JSON,
    }
}

/// The concrete resources, for `resources/list`.
pub fn list_json() -> Json {
    Json::Arr(vec![
        concrete_entry(
            "reaper://status",
            "reaper-status",
            "REAPER bridge status",
            "Bridge liveness, REAPER version, project identity and this server's own version.",
        ),
        concrete_entry(
            "reaper://project/current",
            "reaper-project",
            "Current REAPER project",
            "Identity and play state of the project the bridge is attached to.",
        ),
        concrete_entry(
            "reaper://selection/current",
            "reaper-selection",
            "Current selection snapshot",
            "The most recent snapshot taken by reaper.inspect_selection, verbatim.",
        ),
        concrete_entry(
            "theory://catalog",
            "theory-catalog",
            "Theory catalog",
            "Knowledge version, manifest hash, domains, profiles, rule and source counts, \
             supported chord symbols and generation capabilities.",
        ),
        concrete_entry(
            "theory://sources",
            "theory-sources",
            "Theory sources",
            "The source registry every rule's citations resolve against.",
        ),
        concrete_entry(
            "theory://profiles",
            "theory-profiles",
            "Style profiles",
            "Every style profile, with its inheritance chain and score weights.",
        ),
    ])
}

fn template_entry(uri: &str, name: &str, title: &str, description: &str) -> Json {
    json_obj! {
        "uriTemplate" => uri,
        "name" => name,
        "title" => title,
        "description" => description,
        "mimeType" => MIME_JSON,
    }
}

/// The templated resources, for `resources/templates/list`.
pub fn templates_json() -> Json {
    Json::Arr(vec![
        template_entry(
            "theory://profiles/{profile_id}",
            "theory-profile",
            "One style profile",
            "The fully resolved profile: inheritance chain, normalized score weights, rule \
             overrides and every flattened field.",
        ),
        template_entry(
            "theory://rules/{domain}",
            "theory-rules",
            "Rules in one domain",
            "Every rule in a domain, with its trigger, conditions, effect, exceptions and \
             source citations.",
        ),
        template_entry(
            "analysis://{analysis_id}",
            "analysis",
            "A full analysis",
            "The complete analysis report behind an analysis id issued by \
             music.analyze_selection.",
        ),
        template_entry(
            "candidate://{candidate_id}",
            "candidate",
            "A generated candidate",
            "Chords, parts and provenance for a candidate id this server issued.",
        ),
        template_entry(
            "candidate://{candidate_id}/trace",
            "candidate-trace",
            "A candidate's decision trace",
            "Score components, applied rules, resolved sources, assumptions and rejected \
             alternatives.",
        ),
        template_entry(
            "editplan://{plan_id}",
            "edit-plan",
            "A staged edit plan",
            "The exact plan sent to REAPER, in the domain form with rational quarter notes.",
        ),
        template_entry(
            "transaction://{transaction_id}",
            "transaction",
            "A staged transaction",
            "The transaction record: status, undo label, and the bridge's staging result.",
        ),
    ])
}

/// Handles a `resources/read` request.
pub fn read_request(core: &ServerCore, params: &Json) -> Result<Json, RpcError> {
    let uri = params
        .get("uri")
        .and_then(Json::as_str)
        .ok_or_else(|| RpcError::new(rpc_codes::INVALID_PARAMS, "resources/read needs a uri"))?;
    match read(core, uri) {
        Ok(body) => Ok(json_obj! {
            "contents" => Json::Arr(vec![json_obj! {
                "uri" => uri,
                "mimeType" => MIME_JSON,
                "text" => body.to_string_pretty(),
            }]),
        }),
        Err(e) => Err(RpcError::with_data(
            RESOURCE_NOT_FOUND,
            e.message.clone(),
            e.to_json(),
        )),
    }
}

/// Reads one resource.
pub fn read(core: &ServerCore, uri: &str) -> Result<Json, ToolError> {
    let parsed = ResourceUri::parse(uri)?;
    let now = qjson::time::unix_now();
    let kb = core.knowledge.kb();
    match parsed {
        ResourceUri::ReaperStatus => Ok(crate::tools::reaper::status_body(core)),
        ResourceUri::ReaperProject => Ok(project_body(core)),
        ResourceUri::ReaperSelection => core.with_store(|s| match s.latest_snapshot(now) {
            Some(r) => Ok(r.raw.clone()),
            None => Err(ToolError::new(
                codes::UNKNOWN_ID,
                "no selection has been inspected in this session",
            )
            .remedy("call reaper.inspect_selection first")),
        }),
        ResourceUri::TheoryCatalog => Ok(catalog_body(core)),
        ResourceUri::TheorySources => Ok(json_obj! {
            "knowledge_version" => core.knowledge_version(),
            "count" => kb.sources().len() as i64,
            "sources" => Json::Arr(kb.sources().iter().map(|s| s.to_json()).collect()),
        }),
        ResourceUri::TheoryProfiles => Ok(json_obj! {
            "knowledge_version" => core.knowledge_version(),
            "count" => kb.profiles().len() as i64,
            "profiles" => Json::Arr(
                kb.profiles()
                    .iter()
                    .map(|p| json_obj! {
                        "id" => p.id.clone(),
                        "name" => p.name.clone(),
                        "parent" => match &p.parent {
                            Some(x) => Json::Str(x.clone()),
                            None => Json::Null,
                        },
                        "uri" => format!("theory://profiles/{}", p.id),
                    })
                    .collect(),
            ),
        }),
        ResourceUri::TheoryProfile(id) => profile_body(core, &id),
        ResourceUri::TheoryRules(domain) => {
            let d = RuleDomain::parse(&domain).ok_or_else(|| {
                ToolError::with_details(
                    codes::UNKNOWN_RESOURCE,
                    format!("{domain} is not a rule domain"),
                    json_obj! {
                        "domain" => domain.clone(),
                        "known" => Json::Arr(
                            crate::schema_gen::RULE_DOMAINS
                                .iter()
                                .map(|d| Json::Str((*d).to_string()))
                                .collect(),
                        ),
                    },
                )
            })?;
            let rules = kb.rules_in_domain(d);
            Ok(json_obj! {
                "domain" => domain,
                "knowledge_version" => core.knowledge_version(),
                "count" => rules.len() as i64,
                "rules" => Json::Arr(rules.iter().map(|r| r.to_json()).collect()),
            })
        }
        ResourceUri::Analysis(id) => core.with_store(|s| Ok(s.analysis(&id, now)?.to_json())),
        ResourceUri::Candidate(id) => core.with_store(|s| {
            let record = s.candidate(&id, now)?;
            let mut body = match record.candidate.to_json() {
                Json::Obj(m) => m,
                other => return Ok(other),
            };
            body.insert("snapshot_id", Json::Str(record.snapshot_id.clone()));
            body.insert("analysis_id", Json::Str(record.analysis_id.clone()));
            body.insert("profile_id", Json::Str(record.profile_id.clone()));
            body.insert("knowledge_hash", Json::Str(record.knowledge_hash.clone()));
            body.insert("seed", Json::Int(record.seed as i64));
            Ok(Json::Obj(body))
        }),
        ResourceUri::CandidateTrace(id) => {
            core.with_store(|s| Ok(s.candidate(&id, now)?.candidate.trace.to_json()))
        }
        ResourceUri::EditPlan(id) => core.with_store(|s| {
            let record = s.plan(&id, now)?;
            let mut body = match record.plan.to_json() {
                Json::Obj(m) => m,
                other => return Ok(other),
            };
            body.insert("scope", record.scope.to_json());
            body.insert(
                "precondition_kinds",
                Json::Arr(
                    crate::staging::precondition_kinds(&record.plan)
                        .into_iter()
                        .map(Json::Str)
                        .collect(),
                ),
            );
            Ok(Json::Obj(body))
        }),
        ResourceUri::Transaction(id) => core.with_store(|s| Ok(s.transaction(&id, now)?.to_json())),
    }
}

/// The `theory://profiles/{id}` body.
fn profile_body(core: &ServerCore, id: &str) -> Result<Json, ToolError> {
    let kb = core.knowledge.kb();
    let resolved = kb.resolve_profile(id).map_err(|_| {
        ToolError::with_details(
            codes::UNKNOWN_PROFILE,
            format!("no style profile {id}"),
            json_obj! { "profile_id" => id },
        )
        .remedy("read theory://profiles for the served set")
    })?;
    let raw = kb.profile(id).map(|p| p.raw.clone()).unwrap_or(Json::Null);
    let mut weights = qjson::JsonMap::new();
    for (k, v) in &resolved.weights {
        weights.insert(k.clone(), Json::Float(*v));
    }
    Ok(json_obj! {
        "id" => resolved.id.clone(),
        "chain" => Json::Arr(resolved.chain.iter().map(|c| Json::Str(c.clone())).collect()),
        "weights" => Json::Obj(weights),
        "fields" => Json::Obj(resolved.fields.clone()),
        "rule_overrides" => Json::Arr(
            resolved
                .overrides
                .iter()
                .map(|(k, v)| json_obj! {
                    "rule_id" => k.clone(),
                    "multiplier" => resolved.rule_multiplier(k),
                    "enabled" => !matches!(v, theory_kb::RuleOverride::Disabled),
                })
                .collect(),
        ),
        "raw" => raw,
    })
}

/// The `reaper://project/current` body.
fn project_body(core: &ServerCore) -> Json {
    match core.bridge.status() {
        Ok(status) => json_obj! {
            "bridge_connected" => true,
            "project_uuid" => status.get("project_uuid").cloned().unwrap_or(Json::Null),
            "project_name" => status.get("project_name").cloned().unwrap_or(Json::Null),
            "project_path" => status.get("project_path").cloned().unwrap_or(Json::Null),
            "active_project" => status.get("active_project").cloned().unwrap_or(Json::Bool(false)),
            "play_state" => status.get("play_state").cloned().unwrap_or(Json::Null),
            "selected_item_count" => status
                .get("selected_item_count")
                .cloned()
                .unwrap_or(Json::Int(0)),
            "active_midi_editor" => status
                .get("active_midi_editor")
                .cloned()
                .unwrap_or(Json::Bool(false)),
        },
        Err(e) => json_obj! {
            "bridge_connected" => false,
            "error_code" => e.code.clone(),
            "message" => e.message.clone(),
            "project_uuid" => Json::Null,
            "project_name" => Json::Null,
            "project_path" => Json::Null,
            "active_project" => false,
            "play_state" => Json::Null,
            "selected_item_count" => 0,
            "active_midi_editor" => false,
        },
    }
}

/// The `theory://catalog` body: the bundle's own catalog plus the generation
/// capabilities this build actually offers.
fn catalog_body(core: &ServerCore) -> Json {
    let kb = core.knowledge.kb();
    let mut body = match kb.catalog_json() {
        Json::Obj(m) => m,
        other => return other,
    };
    body.insert(
        "supported_chord_symbols",
        Json::Arr(
            kb.chord_symbols()
                .iter()
                .map(|a| Json::Str(a.symbol.clone()))
                .collect(),
        ),
    );
    body.insert(
        "generation_capabilities",
        json_obj! {
            "harmonization" => true,
            "reharmonization" => true,
            "voicing" => true,
            "arrangement" => true,
            "loop_audit" => true,
            "countermelody" => true,
            "bass" => true,
            "staging" => true,
            "max_candidates" => 8,
            "profiles" => Json::Arr(
                crate::schema_gen::PROFILE_IDS
                    .iter()
                    .map(|p| Json::Str((*p).to_string()))
                    .collect(),
            ),
            "voicing_families" => Json::Arr(
                crate::schema_gen::VOICING_FAMILIES
                    .iter()
                    .map(|p| Json::Str((*p).to_string()))
                    .collect(),
            ),
            "arrangement_roles" => Json::Arr(
                crate::schema_gen::ARRANGEMENT_ROLES
                    .iter()
                    .map(|p| Json::Str((*p).to_string()))
                    .collect(),
            ),
        },
    );
    body.insert(
        "server_version",
        Json::Str(crate::SERVER_VERSION.to_string()),
    );
    body.insert(
        "mcp_protocol_version",
        Json::Str(crate::MCP_PROTOCOL_VERSION.to_string()),
    );
    Json::Obj(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_concrete_uri_parses_back_to_itself() {
        for entry in list_json().as_arr().unwrap() {
            let uri = entry.str_field("uri").unwrap();
            let parsed = ResourceUri::parse(uri).expect(uri);
            assert_eq!(parsed.to_uri(), uri);
        }
    }

    #[test]
    fn the_served_set_is_the_frozen_thirteen() {
        let concrete = list_json().as_arr().unwrap().len();
        let templates = templates_json().as_arr().unwrap().len();
        assert_eq!(concrete, 6);
        assert_eq!(templates, 7);
        assert_eq!(concrete + templates, 13);
    }

    #[test]
    fn templated_uris_parse() {
        assert_eq!(
            ResourceUri::parse("theory://profiles/jazz_standard").unwrap(),
            ResourceUri::TheoryProfile("jazz_standard".into())
        );
        assert_eq!(
            ResourceUri::parse("theory://rules/voice_leading").unwrap(),
            ResourceUri::TheoryRules("voice_leading".into())
        );
        assert_eq!(
            ResourceUri::parse("candidate://abc-123/trace").unwrap(),
            ResourceUri::CandidateTrace("abc-123".into())
        );
        assert_eq!(
            ResourceUri::parse("editplan://p1").unwrap(),
            ResourceUri::EditPlan("p1".into())
        );
        assert_eq!(
            ResourceUri::parse("transaction://t1").unwrap(),
            ResourceUri::Transaction("t1".into())
        );
    }

    #[test]
    fn a_resource_uri_can_never_name_a_path() {
        let attempts = [
            "analysis://../../etc/passwd",
            "analysis://..",
            "analysis://../secrets",
            "analysis:///etc/passwd",
            "analysis://%2e%2e%2fetc%2fpasswd",
            "analysis://a/../../b",
            "analysis://C:\\Windows\\System32",
            "analysis://\\\\server\\share",
            "analysis://~/.ssh/id_rsa",
            "candidate://../../../root/.bashrc",
            "candidate://x/../trace",
            "editplan://./plan",
            "editplan://.hidden",
            "transaction://a%00b",
            "file:///etc/passwd",
            "http://example.com/",
            "theory://profiles/../../etc",
            "theory://rules/../catalog",
            "reaper://status/../../etc/passwd",
            "analysis://a/b/c",
        ];
        for uri in attempts {
            let e = ResourceUri::parse(uri).unwrap_err();
            assert_eq!(e.code, codes::UNKNOWN_RESOURCE, "{uri} must be refused");
        }
    }

    #[test]
    fn a_path_escape_is_refused_at_the_read_boundary_too() {
        let core = ServerCore::offline();
        for uri in [
            "analysis://../../etc/passwd",
            "editplan://../../../proc/self/environ",
            "candidate://..%2f..%2fetc",
        ] {
            assert!(read(&core, uri).is_err(), "{uri}");
        }
    }

    #[test]
    fn safe_segments_are_exactly_the_id_shape() {
        assert!(is_safe_segment("abc"));
        assert!(is_safe_segment("jazz_standard"));
        assert!(is_safe_segment("a-b.c_d"));
        assert!(!is_safe_segment(""));
        assert!(!is_safe_segment("."));
        assert!(!is_safe_segment("..a"));
        assert!(!is_safe_segment("a..b"));
        assert!(!is_safe_segment("-lead"));
        assert!(!is_safe_segment("a/b"));
        assert!(!is_safe_segment("a b"));
        assert!(!is_safe_segment(&"a".repeat(MAX_SEGMENT + 1)));
    }

    #[test]
    fn an_unknown_scheme_is_refused() {
        for uri in ["nope://x", "reaper:/status", "status", ""] {
            assert!(ResourceUri::parse(uri).is_err(), "{uri}");
        }
    }

    #[test]
    fn the_catalog_reports_what_the_brief_requires() {
        let core = ServerCore::offline();
        let body = read(&core, "theory://catalog").unwrap();
        for key in [
            "knowledge_version",
            "content_sha256",
            "counts",
            "rules_by_domain",
            "supported_chord_symbols",
            "generation_capabilities",
        ] {
            assert!(body.get(key).is_some(), "catalog must carry {key}");
        }
        assert!(!body
            .arr_field("supported_chord_symbols")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn sources_and_profiles_read() {
        let core = ServerCore::offline();
        let sources = read(&core, "theory://sources").unwrap();
        assert!(sources.i64_field("count").unwrap() > 0);
        let profiles = read(&core, "theory://profiles").unwrap();
        assert_eq!(profiles.i64_field("count").unwrap(), 10);
    }

    #[test]
    fn one_profile_reads_with_its_chain_and_weights() {
        let core = ServerCore::offline();
        let body = read(&core, "theory://profiles/jazz_standard").unwrap();
        assert_eq!(body.str_field("id").unwrap(), "jazz_standard");
        assert!(!body.arr_field("chain").unwrap().is_empty());
        assert_eq!(
            body.get("weights").unwrap().as_obj().unwrap().len(),
            music_domain::candidate::SCORE_COMPONENTS.len()
        );
    }

    #[test]
    fn an_unknown_profile_is_refused() {
        let core = ServerCore::offline();
        let e = read(&core, "theory://profiles/not_a_profile").unwrap_err();
        assert_eq!(e.code, codes::UNKNOWN_PROFILE);
    }

    #[test]
    fn every_rule_domain_reads() {
        let core = ServerCore::offline();
        let mut total = 0;
        for d in crate::schema_gen::RULE_DOMAINS {
            let body = read(&core, &format!("theory://rules/{d}")).unwrap();
            total += body.i64_field("count").unwrap();
        }
        assert!(total >= 100, "the bundle must carry 100+ rules, saw {total}");
    }

    #[test]
    fn an_unknown_rule_domain_is_refused() {
        let core = ServerCore::offline();
        assert!(read(&core, "theory://rules/nonsense").is_err());
    }

    #[test]
    fn an_unknown_analysis_id_is_refused() {
        let core = ServerCore::offline();
        let e = read(&core, "analysis://deadbeef").unwrap_err();
        assert_eq!(e.code, codes::UNKNOWN_ID);
    }

    #[test]
    fn selection_without_an_inspection_is_refused() {
        let core = ServerCore::offline();
        assert!(read(&core, "reaper://selection/current").is_err());
    }

    #[test]
    fn status_and_project_read_with_no_bridge() {
        let core = ServerCore::offline();
        let status = read(&core, "reaper://status").unwrap();
        assert_eq!(status.get("bridge_connected"), Some(&Json::Bool(false)));
        let project = read(&core, "reaper://project/current").unwrap();
        assert_eq!(project.get("bridge_connected"), Some(&Json::Bool(false)));
    }

    #[test]
    fn read_request_wraps_the_body_in_contents() {
        let core = ServerCore::offline();
        let r = read_request(&core, &json_obj! { "uri" => "theory://catalog" }).unwrap();
        let contents = r.arr_field("contents").unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].str_field("mimeType").unwrap(), MIME_JSON);
        assert!(Json::parse(contents[0].str_field("text").unwrap()).is_ok());
    }

    #[test]
    fn read_request_without_a_uri_is_invalid_params() {
        let core = ServerCore::offline();
        let e = read_request(&core, &json_obj! {}).unwrap_err();
        assert_eq!(e.code, rpc_codes::INVALID_PARAMS);
    }

    #[test]
    fn a_refused_read_uses_the_resource_not_found_code() {
        let core = ServerCore::offline();
        let e = read_request(&core, &json_obj! { "uri" => "analysis://../etc" }).unwrap_err();
        assert_eq!(e.code, RESOURCE_NOT_FOUND);
        assert!(e.data.is_some());
    }
}
