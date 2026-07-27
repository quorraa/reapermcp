//! The brief §33 acceptance workflow, end to end, through the real binary.
//!
//! REAPER itself cannot run here, so [`reaper_ipc::testing::FakeBridge`] stands
//! in for it — over the **real** filesystem, in a scratch directory, driven by
//! its own pump thread. Everything else is genuine: the server is the released
//! binary talking JSON-RPC over pipes, the command files are written and
//! claimed through the atomic `.tmp` → rename dance, and the edit plan crosses
//! the boundary in its wire form.
//!
//! What that buys is that the staging path is really exercised. The fake bridge
//! evaluates `base_snapshot_hash` exactly the way `transactions.lua` does, so
//! `a_stale_snapshot_is_rejected` fails for the same reason the real bridge
//! would fail: the source material moved under the plan.
//!
//! Steps 1–3 of §33 (open REAPER, run the Lua script, select an item) are what
//! the fake bridge substitutes for; steps 4–16 are executed below.

use qjson::{json_obj, Json};
use reaper_ipc::testing::{FakeBridge, Response};
use reaper_ipc::{codes, IpcError, RealFs};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const BIN: &str = env!("CARGO_BIN_EXE_qlabs-reaper-music-mcp");
const TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// A unique scratch directory per test.
fn scratch(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qlabs-mcp-e2e-{tag}-{}-{n}-{}",
        std::process::id(),
        qjson::time::unix_now()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn fixtures_dir() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root")
        .join("fixtures")
}

/// The snapshot a fixture stands in for, in the exact shape the bridge returns.
fn fixture_snapshot(id: &str) -> Json {
    let path = fixtures_dir().join(format!("{id}.json"));
    let fixture = music_domain::fixture::Fixture::from_path(&path).expect("fixture loads");
    reaper_music_mcp::fixtures::snapshot_record(&fixture)
        .expect("snapshot builds")
        .raw
}

// ---------------------------------------------------------------------------
// The stand-in project
// ---------------------------------------------------------------------------

/// What the fake bridge believes the project currently contains.
#[derive(Debug, Default)]
struct ProjectState {
    /// The snapshot `inspect_selection` returns.
    snapshot: Json,
    /// Transactions staged so far, newest last.
    staged: Vec<String>,
    /// Transactions that have been committed.
    committed: Vec<String>,
    /// Transactions that have been discarded.
    discarded: Vec<String>,
    /// Every plan the bridge was asked to stage, in wire form.
    plans: Vec<Json>,
    /// The last owned undo label.
    last_undo_label: Option<String>,
    /// The project's state-change counter.
    state_change_count: i64,
}

impl ProjectState {
    fn snapshot_hash(&self) -> String {
        self.snapshot
            .str_field("snapshot_hash")
            .unwrap_or("")
            .to_string()
    }
}

type SharedProject = Arc<Mutex<ProjectState>>;

fn staging_result(plan: &Json, project: &mut ProjectState) -> Json {
    let transaction_id = plan.str_field("transaction_id").unwrap_or("").to_string();
    let mut tracks = Vec::new();
    let mut items = Vec::new();
    let mut regions = Vec::new();
    let mut sends = Vec::new();
    let mut note_count = 0i64;
    let mut pending: Vec<(String, f64, f64)> = Vec::new();

    for op in plan.arr_field("operations").unwrap_or(&[]) {
        match op.str_field("op").unwrap_or("") {
            kind @ ("create_folder_track" | "create_track") => {
                let temp_id = op.str_field("temp_id").unwrap_or("").to_string();
                tracks.push(json_obj! {
                    "temp_id" => temp_id.clone(),
                    "kind" => kind,
                    "guid" => format!("fake-track-{temp_id}"),
                    "name" => op.str_field("name").unwrap_or("").to_string(),
                });
            }
            "create_midi_item" => {
                let temp_id = op.str_field("temp_id").unwrap_or("").to_string();
                pending.push((
                    temp_id,
                    op.f64_field("start_qn").unwrap_or(0.0),
                    op.f64_field("end_qn").unwrap_or(0.0),
                ));
            }
            "insert_notes" => {
                let item = op.str_field("item").unwrap_or("").to_string();
                let n = op.arr_field("notes").map(<[Json]>::len).unwrap_or(0) as i64;
                note_count += n;
                if let Some((temp_id, start, end)) =
                    pending.iter().find(|(t, _, _)| *t == item).cloned()
                {
                    items.push(json_obj! {
                        "temp_id" => temp_id.clone(),
                        "guid" => format!("fake-item-{temp_id}"),
                        "take_guid" => format!("fake-take-{temp_id}"),
                        "note_count" => n,
                        "start_qn" => start,
                        "end_qn" => end,
                    });
                }
            }
            "create_region" => regions.push(json_obj! {
                "temp_id" => op.str_field("temp_id").unwrap_or("r0").to_string(),
                "marker_index" => 0,
                "name" => op.str_field("name").unwrap_or("").to_string(),
            }),
            "create_midi_send" => sends.push(json_obj! {
                "temp_id" => op.str_field("temp_id").unwrap_or("s0").to_string(),
                "send_index" => 0,
                "to_track_guid" => op.str_field("to_track_guid").unwrap_or("").to_string(),
            }),
            _ => {}
        }
    }

    project.state_change_count += 1;
    project.staged.push(transaction_id.clone());
    project.plans.push(plan.clone());
    project.last_undo_label = plan.str_field("undo_label").ok().map(str::to_string);

    json_obj! {
        "transaction_id" => transaction_id,
        "candidate_id" => plan.str_field("candidate_id").unwrap_or("").to_string(),
        "plan_id" => plan.str_field("plan_id").unwrap_or("").to_string(),
        "undo_label" => plan.str_field("undo_label").unwrap_or("").to_string(),
        "status" => "preview",
        "tracks" => Json::Arr(tracks),
        "items" => Json::Arr(items),
        "regions" => Json::Arr(regions),
        "sends" => Json::Arr(sends),
        "note_count" => note_count,
        "project_state_change_count" => project.state_change_count,
        "warnings" => Json::Arr(Vec::new()),
    }
}

/// The precondition kinds a plan must carry before the bridge will stage it.
const REQUIRED_PRECONDITIONS: &[&str] = &[
    "project_uuid",
    "state_change_count",
    "item_guid_exists",
    "take_guid_exists",
    "midi_hash",
    "tempo_map_hash",
    "item_bounds",
];

fn build_bridge(ipc_dir: &std::path::Path, project: SharedProject) -> Arc<FakeBridge> {
    let responder_project = Arc::clone(&project);
    let bridge = FakeBridge::new(Arc::new(RealFs), ipc_dir, TOKEN).with_responder(Box::new(
        move |req| {
            let mut state = responder_project.lock().expect("project lock");
            match req.command.as_str() {
                "ping" => Response::Ok(json_obj! {
                    "pong" => true,
                    "bridge_version" => reaper_ipc::BRIDGE_VERSION,
                    "protocol_version" => reaper_ipc::PROTOCOL_VERSION,
                    "server_time" => qjson::time::now_iso8601(),
                    "uptime_seconds" => 12.0,
                }),
                "status" => Response::Ok(json_obj! {
                    "bridge_connected" => true,
                    "bridge_version" => reaper_ipc::BRIDGE_VERSION,
                    "bridge_schema_version" => "1",
                    "ipc_protocol_version" => reaper_ipc::PROTOCOL_VERSION,
                    "reaper_version" => "7.22/linux-x86_64",
                    "heartbeat_age_seconds" => 0,
                    "heartbeat_timestamp" => qjson::time::unix_now(),
                    "active_project" => true,
                    "project_uuid" => state.snapshot.str_field("project_uuid").unwrap_or("").to_string(),
                    "project_name" => "AcceptanceProject",
                    "project_path" => "/projects/acceptance.rpp",
                    "play_state" => 0,
                    "selected_item_count" => 1,
                    "active_midi_editor" => true,
                    "knowledge_version" => "1.0.0",
                    "uptime_seconds" => 12.0,
                    "requests_processed" => 1,
                    "requests_failed" => 0,
                    "ipc_dir" => "/ipc",
                    "commands" => Json::Arr(
                        reaper_ipc::COMMANDS.iter().map(|c| Json::Str((*c).to_string())).collect()
                    ),
                    "limits" => reaper_ipc::limits::limits_json(),
                }),
                "inspect_selection" => Response::Ok(state.snapshot.clone()),
                "stage_candidate" => {
                    let Some(plan) = req.payload.get("plan").cloned() else {
                        return Response::Err(IpcError::new(
                            codes::INVALID_EDIT_PLAN,
                            "stage_candidate needs a plan",
                        ));
                    };
                    // The bridge only ever stages a plan that says what it
                    // expects the project to look like.
                    let kinds: Vec<String> = plan
                        .arr_field("preconditions")
                        .unwrap_or(&[])
                        .iter()
                        .filter_map(|p| p.str_field("type").ok().map(str::to_string))
                        .collect();
                    for want in REQUIRED_PRECONDITIONS {
                        if !kinds.iter().any(|k| k == want) {
                            return Response::Err(IpcError::with_details(
                                codes::INVALID_EDIT_PLAN,
                                format!("the plan is missing the {want} precondition"),
                                json_obj! { "missing" => *want },
                            ));
                        }
                    }
                    // Staleness, evaluated the way transactions.lua does.
                    let base = plan.str_field("base_snapshot_hash").unwrap_or("");
                    let live = state.snapshot_hash();
                    if base != live {
                        return Response::Err(IpcError::with_details(
                            codes::STALE_SNAPSHOT,
                            "the source material changed since the plan was generated",
                            json_obj! {
                                "expected" => base,
                                "actual" => live,
                                "rebuilt_with" => json_obj! {
                                    "note_scope" => req.payload
                                        .str_field("note_scope").unwrap_or("").to_string(),
                                    "source_mode" => req.payload
                                        .str_field("source_mode").unwrap_or("").to_string(),
                                },
                                "rolled_back" => false,
                                "transaction_id" => plan.str_field("transaction_id")
                                    .unwrap_or("").to_string(),
                            },
                        ));
                    }
                    Response::Ok(staging_result(&plan, &mut state))
                }
                "commit_candidate" => {
                    let id = req.payload.str_field("transaction_id").unwrap_or("").to_string();
                    if !state.staged.contains(&id) {
                        return Response::Err(IpcError::new(
                            codes::TRANSACTION_NOT_FOUND,
                            format!("no staged objects for {id}"),
                        ));
                    }
                    state.state_change_count += 1;
                    state.committed.push(id.clone());
                    state.last_undo_label =
                        Some(format!("QLabs MCP: Commit candidate {}", &id[..8.min(id.len())]));
                    Response::Ok(json_obj! {
                        "transaction_id" => id,
                        "status" => "committed",
                        "undo_label" => state.last_undo_label.clone().unwrap_or_default(),
                        "committed_tracks" => 3,
                        "committed_items" => 2,
                        "committed_takes" => 2,
                        "inspected_tracks" => 3,
                        "inspected_items" => 2,
                        "inspected_takes" => 2,
                        "project_state_change_count" => state.state_change_count,
                    })
                }
                "discard_candidate" => {
                    let id = req.payload.str_field("transaction_id").unwrap_or("").to_string();
                    if !state.staged.contains(&id) {
                        return Response::Err(IpcError::new(
                            codes::TRANSACTION_NOT_FOUND,
                            format!("no staged objects for {id}"),
                        ));
                    }
                    state.state_change_count += 1;
                    state.staged.retain(|s| *s != id);
                    state.discarded.push(id.clone());
                    state.last_undo_label =
                        Some(format!("QLabs MCP: Discard candidate {}", &id[..8.min(id.len())]));
                    Response::Ok(json_obj! {
                        "transaction_id" => id,
                        "undo_label" => state.last_undo_label.clone().unwrap_or_default(),
                        "removed_items" => 2,
                        "removed_tracks" => 3,
                        "retained_tracks" => 0,
                        "warnings" => Json::Arr(Vec::new()),
                        "project_state_change_count" => state.state_change_count,
                    })
                }
                "undo_last_generation" => {
                    let id = req.payload.str_field("transaction_id").unwrap_or("").to_string();
                    let owned = state.staged.last().cloned().or_else(|| state.committed.last().cloned());
                    let label = state.last_undo_label.clone().unwrap_or_default();
                    if owned.as_deref() != Some(id.as_str()) || !label.starts_with("QLabs MCP: ") {
                        return Response::Err(IpcError::with_details(
                            codes::UNDO_NOT_OWNED,
                            "the top undo entry is not this MCP's last owned transaction",
                            json_obj! {
                                "top_undo_entry" => label,
                                "expected" => owned.unwrap_or_default(),
                            },
                        ));
                    }
                    state.state_change_count += 1;
                    Response::Ok(json_obj! {
                        "undone" => true,
                        "transaction_id" => id,
                        "undo_label" => label,
                        "kind" => "stage",
                        "project_state_change_count" => state.state_change_count,
                    })
                }
                other => Response::Err(IpcError::new(
                    codes::UNKNOWN_COMMAND,
                    format!("unknown command {other}"),
                )),
            }
        },
    ));
    Arc::new(bridge)
}

/// The whole scene: a scratch IPC directory, a pumped fake bridge, and the real
/// server binary talking to it over pipes.
struct Scene {
    dir: PathBuf,
    project: SharedProject,
    stop: Arc<AtomicBool>,
    pump: Option<std::thread::JoinHandle<()>>,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Scene {
    fn new(tag: &str, fixture: &str) -> Scene {
        let dir = scratch(tag);
        let ipc_dir = dir.join("ipc");
        std::fs::write(
            dir.join("config.json"),
            format!(
                "{{\"instance_token\":\"{TOKEN}\",\"ipc_dir\":null,\"poll_interval_ms\":10,\
                 \"heartbeat_interval_ms\":200,\"log_level\":\"off\",\"console_log\":false}}"
            ),
        )
        .expect("config.json");

        let project: SharedProject = Arc::new(Mutex::new(ProjectState {
            snapshot: fixture_snapshot(fixture),
            state_change_count: 1,
            ..ProjectState::default()
        }));
        let bridge = build_bridge(&ipc_dir, Arc::clone(&project));
        bridge.write_heartbeat("online");

        let stop = Arc::new(AtomicBool::new(false));
        let pump_stop = Arc::clone(&stop);
        let pump_bridge = Arc::clone(&bridge);
        let pump = std::thread::spawn(move || {
            let mut ticks = 0u64;
            while !pump_stop.load(Ordering::Relaxed) {
                pump_bridge.pump();
                ticks += 1;
                if ticks % 100 == 0 {
                    pump_bridge.write_heartbeat("online");
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            pump_bridge.write_heartbeat("offline");
        });

        let mut child = Command::new(BIN)
            .arg("serve")
            .arg("--config")
            .arg(dir.join("config.json"))
            .env("QLABS_MCP_LOG", "off")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the server binary must be runnable");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));

        let mut scene = Scene {
            dir,
            project,
            stop,
            pump: Some(pump),
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        scene.request(
            "initialize",
            json_obj! { "protocolVersion" => reaper_music_mcp::MCP_PROTOCOL_VERSION },
        );
        scene.notify("notifications/initialized", json_obj! {});
        scene
    }

    fn notify(&mut self, method: &str, params: Json) {
        let line = format!(
            "{}\n",
            json_obj! { "jsonrpc" => "2.0", "method" => method, "params" => params }
        );
        self.stdin.write_all(line.as_bytes()).expect("write");
        self.stdin.flush().expect("flush");
    }

    /// Prints one exchange when `QLABS_E2E_TRANSCRIPT` is set.
    fn transcribe(method: &str, params: &Json, response: &Json) {
        let label = match params.get("name").and_then(Json::as_str) {
            Some(tool) => format!("{method} {tool}"),
            None => match params.get("uri").and_then(Json::as_str) {
                Some(uri) => format!("{method} {uri}"),
                None => method.to_string(),
            },
        };
        let outcome = match response.get("result") {
            Some(r) => match r.get("isError") {
                Some(Json::Bool(true)) => format!(
                    "ERROR {}",
                    r.get("structuredContent")
                        .and_then(|s| s.str_field("error_code").ok())
                        .unwrap_or("?")
                ),
                _ => "ok".to_string(),
            },
            None => "rpc-error".to_string(),
        };
        let detail = response
            .get("result")
            .and_then(|r| r.get("structuredContent"))
            .map(|s| {
                let t = s.to_string();
                if t.len() > 220 {
                    format!("{}...", &t[..220])
                } else {
                    t
                }
            })
            .unwrap_or_default();
        eprintln!("--> {label}");
        eprintln!("<-- {outcome}  {detail}");
    }

    fn request(&mut self, method: &str, params: Json) -> Json {
        let params_echo = params.clone();
        let id = self.next_id;
        self.next_id += 1;
        let line = format!(
            "{}\n",
            json_obj! {
                "jsonrpc" => "2.0",
                "id" => id,
                "method" => method,
                "params" => params,
            }
        );
        self.stdin.write_all(line.as_bytes()).expect("write");
        self.stdin.flush().expect("flush");

        loop {
            let mut buf = String::new();
            let n = self.stdout.read_line(&mut buf).expect("read");
            assert_ne!(n, 0, "the server closed stdout while waiting for {method}");
            if buf.trim().is_empty() {
                continue;
            }
            let value = Json::parse(buf.trim())
                .unwrap_or_else(|e| panic!("stdout is not JSON-RPC ({e}): {buf}"));
            assert_eq!(value.str_field("jsonrpc"), Ok("2.0"), "{buf}");
            if value.get("id") == Some(&Json::Int(id)) {
                if std::env::var("QLABS_E2E_TRANSCRIPT").is_ok() {
                    Scene::transcribe(method, &params_echo, &value);
                }
                return value;
            }
            // A progress notification or an unrelated frame; keep reading.
            assert!(
                value.get("method").is_some(),
                "unexpected response for another id: {buf}"
            );
        }
    }

    fn result(&mut self, method: &str, params: Json) -> Json {
        let r = self.request(method, params);
        r.get("result")
            .unwrap_or_else(|| panic!("{method} failed: {r}"))
            .clone()
    }

    /// Calls a tool and asserts it succeeded, returning `structuredContent`.
    fn tool(&mut self, name: &str, arguments: Json) -> Json {
        let r = self.result(
            "tools/call",
            json_obj! { "name" => name, "arguments" => arguments },
        );
        assert_eq!(
            r.get("isError"),
            Some(&Json::Bool(false)),
            "{name} failed: {}",
            r.get("structuredContent").cloned().unwrap_or(Json::Null)
        );
        r.get("structuredContent").unwrap().clone()
    }

    /// Calls a tool and asserts it failed, returning the error payload.
    fn tool_error(&mut self, name: &str, arguments: Json) -> Json {
        let r = self.result(
            "tools/call",
            json_obj! { "name" => name, "arguments" => arguments },
        );
        assert_eq!(
            r.get("isError"),
            Some(&Json::Bool(true)),
            "{name} unexpectedly succeeded: {}",
            r.get("structuredContent").cloned().unwrap_or(Json::Null)
        );
        r.get("structuredContent").unwrap().clone()
    }

    fn read_resource(&mut self, uri: &str) -> Json {
        let r = self.result("resources/read", json_obj! { "uri" => uri });
        let text = r.arr_field("contents").unwrap()[0]
            .str_field("text")
            .unwrap()
            .to_string();
        Json::parse(&text).expect("resource body is JSON")
    }

    /// Replaces the project's material, as if the user had edited it.
    fn edit_project(&self, fixture: &str) {
        let mut state = self.project.lock().expect("project lock");
        state.snapshot = fixture_snapshot(fixture);
        state.state_change_count += 1;
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        // Closing stdin is the graceful-shutdown signal.
        let _ = self.stdin.write_all(b"");
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.stop.store(true, Ordering::Relaxed);
        if let Some(p) = self.pump.take() {
            let _ = p.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------------------
// §33 steps 4 to 8
// ---------------------------------------------------------------------------

#[test]
fn the_acceptance_workflow_runs_end_to_end() {
    let mut scene = Scene::new("workflow", "melodies/eight_bar_c_major");

    // 4. reaper.status
    let status = scene.tool("reaper.status", json_obj! {});
    assert_eq!(status.get("bridge_connected"), Some(&Json::Bool(true)));
    assert_eq!(status.get("active_project"), Some(&Json::Bool(true)));
    assert_eq!(
        status.str_field("reaper_version").unwrap(),
        "7.22/linux-x86_64"
    );

    // 5. reaper.inspect_selection
    let snapshot = scene.tool("reaper.inspect_selection", json_obj! {});
    let snapshot_id = snapshot.str_field("snapshot_id").unwrap().to_string();
    assert!(snapshot.i64_field("note_count").unwrap() > 0);
    assert!(snapshot
        .str_field("snapshot_hash")
        .unwrap()
        .starts_with("fnv1a64:"));

    // The snapshot is served verbatim as a resource.
    let selection = scene.read_resource("reaper://selection/current");
    assert_eq!(selection.str_field("snapshot_id").unwrap(), snapshot_id);

    // 6. music.analyze_selection
    let analysis = scene.tool(
        "music.analyze_selection",
        json_obj! { "snapshot_id" => snapshot_id.clone(), "style_profile" => "jazz_standard" },
    );
    let analysis_id = analysis.str_field("analysis_id").unwrap().to_string();
    assert!(
        analysis
            .get("key")
            .unwrap()
            .arr_field("candidates")
            .unwrap()
            .len()
            > 1,
        "the reading must be ranked, not asserted"
    );
    assert!(!analysis.arr_field("phrases").unwrap().is_empty());

    // 7 and 8. Three candidates, melody preserved, bass and countermelody,
    // loopable over the eight-bar region.
    let generation = scene.tool(
        "harmony.generate_candidates",
        json_obj! {
            "snapshot_id" => snapshot_id.clone(),
            "analysis_id" => analysis_id.clone(),
            "style_profile" => "jazz_standard",
            "candidate_count" => 3,
            "preserve_melody" => true,
            "preserve_rhythm" => true,
            "complexity" => 0.65,
            "chromaticism" => 0.35,
            "extension_density" => 0.55,
            "bass_motion" => "auto",
            "countermelody" => json_obj! { "enabled" => true, "density" => 0.25 },
            "loop_intent" => "closed_tonic",
            "strictness" => "balanced",
            "seed" => 12345,
        },
    );
    let candidates = generation.arr_field("candidates").unwrap().to_vec();
    assert_eq!(candidates.len(), 3);

    let strategies: Vec<String> = candidates
        .iter()
        .map(|c| c.str_field("strategy").unwrap().to_string())
        .collect();
    let mut unique = strategies.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        3,
        "three genuinely different candidates: {strategies:?}"
    );

    for c in &candidates {
        assert!(!c.arr_field("chords").unwrap().is_empty());
        assert!(!c.arr_field("score_components").unwrap().is_empty());
        assert!(!c.arr_field("rule_ids").unwrap().is_empty());
        let parts: Vec<String> = c
            .arr_field("parts")
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap_or("").to_string())
            .collect();
        assert!(
            parts.iter().any(|p| p.starts_with("lead:")),
            "the melody must survive as a part: {parts:?}"
        );
        assert!(
            parts.iter().any(|p| p.starts_with("bass:")),
            "bass was requested: {parts:?}"
        );
    }

    // Each candidate explains itself with real rule ids and resolvable sources.
    for c in &candidates {
        let id = c.str_field("candidate_id").unwrap().to_string();
        let explained = scene.tool(
            "candidate.explain",
            json_obj! { "candidate_id" => id.clone(), "detail" => "detailed" },
        );
        assert!(!explained.arr_field("rules").unwrap().is_empty());
        assert!(!explained
            .get("score")
            .unwrap()
            .arr_field("components")
            .unwrap()
            .is_empty());
        for s in explained.arr_field("sources").unwrap() {
            assert!(!s.str_field("title").unwrap().is_empty());
        }
        // And the trace is readable as a resource.
        let trace = scene.read_resource(&format!("candidate://{id}/trace"));
        assert!(trace.get("rule_applications").is_some());
    }

    // The loop is audited.
    for c in &candidates {
        let audit = scene.tool(
            "loop.audit",
            json_obj! {
                "candidate_id" => c.str_field("candidate_id").unwrap(),
                "loop_intent" => "closed_tonic",
                "loop_span" => json_obj! { "start_qn" => 0.0, "end_qn" => 32.0 },
            },
        );
        assert_eq!(audit.str_field("intent").unwrap(), "closed_tonic");
        assert!(audit.get("harmonic_wrap").is_some());
        assert!(audit.get("repairs").is_some());
    }

    // 9 and 10. Stage the chosen candidate.
    let chosen = candidates[0].str_field("candidate_id").unwrap().to_string();
    let staged = scene.tool(
        "reaper.stage_candidate",
        json_obj! { "candidate_id" => chosen.clone(), "create_region" => true },
    );
    let transaction_id = staged.str_field("transaction_id").unwrap().to_string();
    let plan_id = staged.str_field("plan_id").unwrap().to_string();
    assert_eq!(staged.str_field("status").unwrap(), "preview");
    assert!(staged
        .str_field("undo_label")
        .unwrap()
        .starts_with("QLabs MCP: "));
    assert!(staged.i64_field("note_count").unwrap() > 0);
    assert!(!staged.arr_field("tracks").unwrap().is_empty());
    assert!(!staged.arr_field("items").unwrap().is_empty());

    // The plan carried all seven preconditions and echoed the scope.
    let kinds: Vec<String> = staged
        .arr_field("precondition_kinds")
        .unwrap()
        .iter()
        .map(|k| k.as_str().unwrap_or("").to_string())
        .collect();
    for want in REQUIRED_PRECONDITIONS {
        assert!(kinds.contains(&want.to_string()), "missing {want}");
    }
    let echoed = staged.get("scope_echoed").unwrap();
    assert!(echoed.get("note_scope").is_some());
    assert!(echoed.get("melody_extraction").is_some());

    // 11. The original MIDI item is untouched: no operation in the plan names
    // the source take or item as a write target.
    let plan = scene.read_resource(&format!("editplan://{plan_id}"));
    let source_take = selection.str_field("take_guid").unwrap().to_string();
    let source_item = selection.str_field("item_guid").unwrap().to_string();
    for op in plan.arr_field("operations").unwrap() {
        let text = op.to_string();
        assert!(
            !text.contains(&source_take),
            "an op names the source take: {text}"
        );
        assert!(
            !text.contains(&source_item),
            "an op names the source item: {text}"
        );
        assert!(matches!(
            op.str_field("op").unwrap(),
            "create_folder_track"
                | "create_track"
                | "create_midi_item"
                | "insert_notes"
                | "set_track_mute"
                | "create_region"
                | "create_midi_send"
        ));
    }

    // 13. Discarding removes only that candidate.
    let discarded = scene.tool(
        "reaper.discard_candidate",
        json_obj! { "transaction_id" => transaction_id.clone() },
    );
    assert_eq!(
        discarded.str_field("transaction_id").unwrap(),
        transaction_id
    );
    assert!(discarded.i64_field("removed_tracks").unwrap() > 0);
    {
        let state = scene.project.lock().unwrap();
        assert!(state.discarded.contains(&transaction_id));
        assert!(!state.staged.contains(&transaction_id));
    }

    // 14. Staging again and committing preserves the generated tracks.
    let restaged = scene.tool(
        "reaper.stage_candidate",
        json_obj! { "candidate_id" => chosen.clone() },
    );
    let second_transaction = restaged.str_field("transaction_id").unwrap().to_string();
    assert_ne!(second_transaction, transaction_id, "a fresh transaction id");
    let committed = scene.tool(
        "reaper.commit_candidate",
        json_obj! { "transaction_id" => second_transaction.clone() },
    );
    assert_eq!(committed.str_field("status").unwrap(), "committed");
    assert!(committed.i64_field("committed_tracks").unwrap() > 0);

    // 15. Undo acts only on the owned transaction.
    let undone = scene.tool(
        "reaper.undo_last_generation",
        json_obj! { "transaction_id" => second_transaction.clone() },
    );
    assert_eq!(undone.get("undone"), Some(&Json::Bool(true)));
    assert!(undone
        .str_field("undo_label")
        .unwrap()
        .starts_with("QLabs MCP: "));

    // The transaction record is readable throughout.
    let record = scene.read_resource(&format!("transaction://{second_transaction}"));
    assert_eq!(record.str_field("candidate_id").unwrap(), chosen);
}

// ---------------------------------------------------------------------------
// §33 step 12: a stale snapshot is rejected
// ---------------------------------------------------------------------------

#[test]
fn a_stale_snapshot_is_rejected() {
    let mut scene = Scene::new("stale", "melodies/eight_bar_c_major");

    let snapshot = scene.tool("reaper.inspect_selection", json_obj! {});
    let snapshot_id = snapshot.str_field("snapshot_id").unwrap().to_string();
    let generation = scene.tool(
        "harmony.generate_candidates",
        json_obj! { "snapshot_id" => snapshot_id, "candidate_count" => 1, "seed" => 1 },
    );
    let candidate_id = generation.arr_field("candidates").unwrap()[0]
        .str_field("candidate_id")
        .unwrap()
        .to_string();

    // The user edits the source material after the candidate was generated.
    scene.edit_project("melodies/dorian_vamp_d");

    let failure = scene.tool_error(
        "reaper.stage_candidate",
        json_obj! { "candidate_id" => candidate_id.clone() },
    );
    assert_eq!(
        failure.str_field("error_code").unwrap(),
        codes::STALE_SNAPSHOT,
        "{failure}"
    );
    assert!(
        failure
            .str_field("remedy")
            .unwrap()
            .contains("inspect_selection"),
        "the remedy must tell the caller to re-inspect: {failure}"
    );
    // The details distinguish a genuine edit from a scope mismatch.
    let details = failure.get("details").unwrap();
    assert!(details.get("rebuilt_with").is_some(), "{details}");

    // Nothing was written.
    {
        let state = scene.project.lock().unwrap();
        assert!(state.staged.is_empty(), "a stale plan must stage nothing");
        assert!(state.plans.is_empty());
    }

    // Re-inspecting and regenerating against the new material works.
    let fresh = scene.tool("reaper.inspect_selection", json_obj! {});
    let fresh_id = fresh.str_field("snapshot_id").unwrap().to_string();
    let regenerated = scene.tool(
        "harmony.generate_candidates",
        json_obj! { "snapshot_id" => fresh_id, "candidate_count" => 1, "seed" => 1 },
    );
    let fresh_candidate = regenerated.arr_field("candidates").unwrap()[0]
        .str_field("candidate_id")
        .unwrap()
        .to_string();
    assert_ne!(
        fresh_candidate, candidate_id,
        "a changed snapshot regenerates"
    );
    let staged = scene.tool(
        "reaper.stage_candidate",
        json_obj! { "candidate_id" => fresh_candidate },
    );
    assert_eq!(staged.str_field("status").unwrap(), "preview");
}

#[test]
fn the_plan_the_bridge_receives_is_the_wire_form() {
    let mut scene = Scene::new("wire", "melodies/eight_bar_c_major");
    let snapshot = scene.tool("reaper.inspect_selection", json_obj! {});
    let generation = scene.tool(
        "harmony.generate_candidates",
        json_obj! {
            "snapshot_id" => snapshot.str_field("snapshot_id").unwrap(),
            "candidate_count" => 1,
            "seed" => 2,
        },
    );
    let candidate_id = generation.arr_field("candidates").unwrap()[0]
        .str_field("candidate_id")
        .unwrap()
        .to_string();
    scene.tool(
        "reaper.stage_candidate",
        json_obj! { "candidate_id" => candidate_id },
    );

    let state = scene.project.lock().unwrap();
    let plan = state.plans.first().expect("the bridge received a plan");

    // Preconditions are discriminated on `type`, never on `kind`.
    for p in plan.arr_field("preconditions").unwrap() {
        assert!(
            p.get("type").is_some(),
            "wire preconditions use `type`: {p}"
        );
        assert!(p.get("kind").is_none());
    }
    // Quarter notes are numbers, never rational strings.
    for op in plan.arr_field("operations").unwrap() {
        for key in ["start_qn", "end_qn"] {
            if let Some(v) = op.get(key) {
                assert!(
                    matches!(v, Json::Float(_) | Json::Int(_)),
                    "{key} must be a number on the wire, got {v}"
                );
            }
        }
        if let Some(notes) = op.get("notes").and_then(Json::as_arr) {
            for n in notes {
                assert!(matches!(
                    n.get("start_qn"),
                    Some(Json::Float(_) | Json::Int(_))
                ));
            }
        }
        // Tags are pairs, never a map.
        if let Some(tags) = op.get("tags") {
            assert!(matches!(tags, Json::Arr(_)), "tags must be pairs: {tags}");
            for t in tags.as_arr().unwrap() {
                assert!(matches!(t, Json::Arr(_)));
            }
        }
    }
    // And the undo label carries the prefix the bridge insists on.
    assert!(plan
        .str_field("undo_label")
        .unwrap()
        .starts_with("QLabs MCP: "));

    // The wire plan satisfies the published schema.
    let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("schemas/edit-plan.schema.json");
    let doc = Json::parse(&std::fs::read_to_string(schema_path).unwrap()).unwrap();
    let schema = qjson::schema::Schema::compile(&doc).unwrap();
    let violations = schema.validate(plan);
    assert!(
        violations.is_empty(),
        "{:?}",
        violations
            .iter()
            .map(|v| format!("{}{}: {}", v.instance_path, v.keyword, v.message))
            .collect::<Vec<_>>()
    );
}

#[test]
fn committing_a_transaction_this_session_did_not_stage_is_refused() {
    let mut scene = Scene::new("unowned", "melodies/eight_bar_c_major");
    for tool in ["reaper.commit_candidate", "reaper.discard_candidate"] {
        let failure = scene.tool_error(
            tool,
            json_obj! { "transaction_id" => "00000000-0000-4000-8000-00000000dead" },
        );
        assert_eq!(
            failure.str_field("error_code").unwrap(),
            "UNKNOWN_TRANSACTION",
            "{tool}"
        );
    }
}

#[test]
fn undo_refuses_when_the_last_owned_transaction_is_not_the_one_named() {
    let mut scene = Scene::new("undo", "melodies/eight_bar_c_major");
    let snapshot = scene.tool("reaper.inspect_selection", json_obj! {});
    let generation = scene.tool(
        "harmony.generate_candidates",
        json_obj! {
            "snapshot_id" => snapshot.str_field("snapshot_id").unwrap(),
            "candidate_count" => 2,
            "seed" => 4,
        },
    );
    let first = generation.arr_field("candidates").unwrap()[0]
        .str_field("candidate_id")
        .unwrap()
        .to_string();
    let second = generation.arr_field("candidates").unwrap()[1]
        .str_field("candidate_id")
        .unwrap()
        .to_string();

    let a = scene.tool(
        "reaper.stage_candidate",
        json_obj! { "candidate_id" => first },
    );
    scene.tool(
        "reaper.stage_candidate",
        json_obj! { "candidate_id" => second },
    );

    // The older transaction is no longer the top of the undo stack.
    let failure = scene.tool_error(
        "reaper.undo_last_generation",
        json_obj! { "transaction_id" => a.str_field("transaction_id").unwrap() },
    );
    assert_eq!(
        failure.str_field("error_code").unwrap(),
        codes::UNDO_NOT_OWNED
    );
    assert!(failure
        .get("details")
        .unwrap()
        .get("top_undo_entry")
        .is_some());
}

#[test]
fn an_offline_bridge_is_reported_rather_than_hanging() {
    let dir = scratch("offline");
    std::fs::write(
        dir.join("config.json"),
        format!("{{\"instance_token\":\"{TOKEN}\",\"ipc_dir\":null}}"),
    )
    .expect("config.json");

    let mut child = Command::new(BIN)
        .arg("serve")
        .arg("--config")
        .arg(dir.join("config.json"))
        .env("QLABS_MCP_LOG", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let started = std::time::Instant::now();
    for (id, message) in [
        (
            1,
            json_obj! { "protocolVersion" => reaper_music_mcp::MCP_PROTOCOL_VERSION },
        ),
        (2, json_obj! {}),
    ] {
        let method = if id == 1 { "initialize" } else { "tools/call" };
        let params = if id == 1 {
            message
        } else {
            json_obj! { "name" => "reaper.inspect_selection", "arguments" => json_obj! {} }
        };
        let line = format!(
            "{}\n",
            json_obj! { "jsonrpc" => "2.0", "id" => id, "method" => method, "params" => params }
        );
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.flush().unwrap();
        let mut buf = String::new();
        stdout.read_line(&mut buf).unwrap();
        let v = Json::parse(buf.trim()).unwrap();
        if id == 2 {
            let payload = v
                .get("result")
                .unwrap()
                .get("structuredContent")
                .unwrap()
                .clone();
            assert_eq!(
                payload.str_field("error_code").unwrap(),
                codes::BRIDGE_OFFLINE
            );
            assert!(payload.get("remedy").is_some());
        }
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "an offline bridge must fail fast, not wait out the request timeout"
    );

    drop(stdin);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
