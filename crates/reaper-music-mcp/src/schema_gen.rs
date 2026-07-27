//! Tool schemas: declared, compiled, and enforced in both directions.
//!
//! Every tool carries a JSON Schema 2020-12 `inputSchema` **and** an
//! `outputSchema`. Both are compiled with [`qjson::schema::Schema`] when the
//! registry is built, and both are enforced at call time: arguments that fail
//! the input schema never reach a tool, and a result that fails the output
//! schema is reported as [`OUTPUT_SCHEMA_VIOLATION`] rather than being handed
//! to the client.
//!
//! [`OUTPUT_SCHEMA_VIOLATION`]: crate::error::codes::OUTPUT_SCHEMA_VIOLATION
//!
//! # Why input schemas are closed and output schemas are too
//!
//! `additionalProperties: false` on the input side turns a typo in an argument
//! name into an immediate structured error instead of a silently ignored
//! setting — which, for a tool that writes into a user's project, is the
//! difference between "nothing happened" and "something you did not ask for
//! happened". The output side is closed for the same reason in reverse: a field
//! this server emits but never declared is a field no client can rely on.

use qjson::schema::{Schema, SchemaError};
use qjson::{json_obj, Json, JsonMap};

/// The JSON Schema dialect every schema in this crate declares.
pub const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// The pattern every server-issued identifier must match.
///
/// Identical to the bridge's request-id pattern, and deliberately unable to
/// express a path: no `/`, no `\`, no `..`, and a leading alphanumeric.
pub const ID_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$";

/// The style profile ids the knowledge bundle is required to define.
pub const PROFILE_IDS: &[&str] = &[
    "common_practice",
    "strict_counterpoint",
    "jazz_standard",
    "blues",
    "pop_rock",
    "neo_soul_rnb",
    "modal_ambient",
    "cinematic",
    "electronic_loop",
    "drum_and_bass",
];

/// Melody-extraction modes, per `IPC_WIRE.md` §6.3.
pub const EXTRACTION_MODES: &[&str] = &[
    "auto",
    "selected_notes",
    "highest_voice",
    "lowest_voice",
    "midi_channel",
    "monophonic_voice",
    "all_notes_as_harmony",
];

/// Note scopes.
pub const NOTE_SCOPES: &[&str] = &["selected_or_all", "selected_only", "all"];

/// Source modes.
pub const SOURCE_MODES: &[&str] = &["auto", "active_editor", "selected_item"];

/// Analysis strictness levels.
pub const STRICTNESS: &[&str] = &[
    "exploratory",
    "balanced",
    "conservative",
    "common_practice_strict",
];

/// Bass motion strategies.
pub const BASS_MOTIONS: &[&str] = &[
    "auto",
    "roots",
    "inversions",
    "stepwise",
    "pedal",
    "ostinato",
    "contrary_motion",
];

/// Loop intents.
pub const LOOP_INTENTS: &[&str] = &[
    "closed_tonic",
    "open_dominant",
    "modal_drone",
    "seamless_color",
    "transition_ready",
    "one_shot_ending",
];

/// Harmonic-grid modes.
pub const GRID_MODES: &[&str] = &["auto", "existing", "bars", "beats"];

/// Rule domains.
pub const RULE_DOMAINS: &[&str] = &[
    "melody",
    "harmony",
    "extensions",
    "voice_leading",
    "counterpoint",
    "arrangement",
    "looping",
];

/// Rule kinds.
pub const RULE_KINDS: &[&str] = &[
    "hard_integrity",
    "mathematical_invariant",
    "strong_theory_principle",
    "theory_default",
    "style_sensitive_preference",
    "arrangement_heuristic",
    "loop_integrity",
    "implementation_heuristic",
];

/// Voicing families.
pub const VOICING_FAMILIES: &[&str] = &[
    "close",
    "open",
    "drop2",
    "drop3",
    "shell",
    "rootless",
    "spread",
    "quartal",
    "quintal",
    "cluster",
    "power",
    "upper_structure",
    "pedal",
];

/// Arrangement roles.
pub const ARRANGEMENT_ROLES: &[&str] = &[
    "lead",
    "counterlead",
    "bass",
    "harmonic_bed",
    "pad",
    "comping",
    "pulse",
    "ostinato",
    "riff",
    "percussion",
    "impact",
    "transition",
    "texture",
    "ambience",
    "ornament",
    "ear_candy",
];

// ---------------------------------------------------------------------------
// Small schema constructors
// ---------------------------------------------------------------------------

/// `{"type": t}`.
pub fn typed(t: &str) -> Json {
    json_obj! { "type" => t }
}

/// `{"type": [t, "null"]}`.
pub fn nullable(t: &str) -> Json {
    json_obj! { "type" => Json::Arr(vec![Json::Str(t.to_string()), Json::Str("null".into())]) }
}

/// `{"type": "array", "items": items}`.
pub fn arr_of(items: Json) -> Json {
    json_obj! { "type" => "array", "items" => items }
}

/// `{"type": "string", "enum": [...]}`.
pub fn enum_of(values: &[&str]) -> Json {
    json_obj! {
        "type" => "string",
        "enum" => Json::Arr(values.iter().map(|v| Json::Str((*v).to_string())).collect()),
    }
}

/// A closed numeric range.
pub fn num_range(lo: f64, hi: f64) -> Json {
    json_obj! { "type" => "number", "minimum" => lo, "maximum" => hi }
}

/// A closed integer range.
pub fn int_range(lo: i64, hi: i64) -> Json {
    json_obj! { "type" => "integer", "minimum" => lo, "maximum" => hi }
}

/// A string constrained to [`ID_PATTERN`].
pub fn id_string() -> Json {
    json_obj! { "type" => "string", "pattern" => ID_PATTERN, "minLength" => 1, "maxLength" => 128 }
}

/// A nullable string constrained to [`ID_PATTERN`].
pub fn nullable_id_string() -> Json {
    json_obj! {
        "type" => Json::Arr(vec![Json::Str("string".into()), Json::Str("null".into())]),
        "maxLength" => 128,
    }
}

/// Attaches a `description` to a schema fragment.
pub fn described(schema: Json, description: &str) -> Json {
    match schema {
        Json::Obj(mut m) => {
            m.insert("description", Json::Str(description.to_string()));
            Json::Obj(m)
        }
        other => other,
    }
}

/// Builds an object schema.
pub fn object(props: Vec<(&str, Json)>, required: &[&str], additional: bool) -> Json {
    let mut p = JsonMap::new();
    for (k, v) in props {
        p.insert(k, v);
    }
    json_obj! {
        "type" => "object",
        "properties" => Json::Obj(p),
        "required" => Json::Arr(required.iter().map(|r| Json::Str((*r).to_string())).collect()),
        "additionalProperties" => additional,
    }
}

/// A top-level schema: an object plus the dialect declaration and a title.
pub fn root(title: &str, body: Json) -> Json {
    match body {
        Json::Obj(m) => {
            let mut out = JsonMap::new();
            out.insert("$schema", Json::Str(DIALECT.to_string()));
            out.insert("title", Json::Str(title.to_string()));
            for (k, v) in m.iter() {
                out.insert(k, v.clone());
            }
            Json::Obj(out)
        }
        other => other,
    }
}

/// The `{code, message, severity}` warning object every tool may return.
pub fn warning_schema() -> Json {
    object(
        vec![
            ("code", typed("string")),
            ("message", typed("string")),
            ("severity", typed("string")),
        ],
        &["code", "message"],
        true,
    )
}

/// The standard `warnings` array.
pub fn warnings_property() -> (&'static str, Json) {
    ("warnings", arr_of(warning_schema()))
}

/// The standard `ok` discriminator, always `true` on the success path.
pub fn ok_property() -> (&'static str, Json) {
    ("ok", json_obj! { "type" => "boolean", "const" => true })
}

/// Builds a tool output schema with the shared `ok` / `warnings` envelope.
pub fn output(title: &str, mut props: Vec<(&'static str, Json)>, required: &[&str]) -> Json {
    props.insert(0, ok_property());
    props.push(warnings_property());
    let mut req: Vec<&str> = vec!["ok"];
    req.extend_from_slice(required);
    req.push("warnings");
    root(title, object(props, &req, false))
}

// ---------------------------------------------------------------------------
// Tool specifications
// ---------------------------------------------------------------------------

/// One tool as the protocol describes it.
#[derive(Clone, Debug)]
pub struct ToolSpec {
    /// The stable tool name.
    pub name: &'static str,
    /// A short human title.
    pub title: &'static str,
    /// What the tool does, for the client model.
    pub description: &'static str,
    /// The declared `inputSchema`.
    pub input_schema: Json,
    /// The declared `outputSchema`.
    pub output_schema: Json,
    /// True when the tool cannot work without a live REAPER bridge.
    pub requires_bridge: bool,
    /// True when the tool mutates the REAPER project.
    pub mutates_project: bool,
}

impl ToolSpec {
    /// The `tools/list` entry.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "name" => self.name,
            "title" => self.title,
            "description" => self.description,
            "inputSchema" => self.input_schema.clone(),
            "outputSchema" => self.output_schema.clone(),
            "annotations" => json_obj! {
                "readOnlyHint" => !self.mutates_project,
                "destructiveHint" => false,
                "idempotentHint" => !self.mutates_project,
                "openWorldHint" => self.requires_bridge,
            },
        }
    }
}

/// A spec plus its compiled schemas.
#[derive(Debug)]
pub struct CompiledTool {
    /// The declaration.
    pub spec: ToolSpec,
    /// The compiled input schema.
    pub input: Schema,
    /// The compiled output schema.
    pub output: Schema,
}

/// Every tool, compiled.
#[derive(Debug)]
pub struct ToolRegistry {
    tools: Vec<CompiledTool>,
}

impl ToolRegistry {
    /// Compiles every declared schema.
    ///
    /// Fails only if a schema in this file is malformed, which a unit test
    /// makes impossible to ship.
    pub fn compile() -> Result<ToolRegistry, SchemaError> {
        let mut tools = Vec::new();
        for spec in tool_specs() {
            let input = Schema::compile(&spec.input_schema)?;
            let output = Schema::compile(&spec.output_schema)?;
            tools.push(CompiledTool {
                spec,
                input,
                output,
            });
        }
        Ok(ToolRegistry { tools })
    }

    /// Every tool, in declaration order.
    pub fn all(&self) -> &[CompiledTool] {
        &self.tools
    }

    /// Looks a tool up by name.
    pub fn get(&self, name: &str) -> Option<&CompiledTool> {
        self.tools.iter().find(|t| t.spec.name == name)
    }

    /// The `tools/list` payload.
    pub fn list_json(&self) -> Json {
        Json::Arr(self.tools.iter().map(|t| t.spec.to_json()).collect())
    }

    /// Every tool name, in declaration order.
    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|t| t.spec.name).collect()
    }
}

/// The melody-extraction argument object, shared by several tools.
fn melody_extraction_schema() -> Json {
    described(
        object(
            vec![
                ("mode", enum_of(EXTRACTION_MODES)),
                ("channel", int_range(0, 15)),
            ],
            &[],
            false,
        ),
        "How to pick the line to work on. `channel` is required when `mode` is `midi_channel`.",
    )
}

/// The harmonic-rhythm argument object.
fn harmonic_rhythm_schema() -> Json {
    described(
        object(
            vec![
                ("mode", enum_of(GRID_MODES)),
                ("value", num_range(0.0625, 64.0)),
            ],
            &[],
            false,
        ),
        "Harmonic grid. `value` is bars for `bars` and beats for `beats`; ignored otherwise.",
    )
}

/// The countermelody argument object.
fn countermelody_schema() -> Json {
    object(
        vec![
            ("enabled", typed("boolean")),
            ("density", num_range(0.0, 1.0)),
            ("role", enum_of(ARRANGEMENT_ROLES)),
        ],
        &[],
        false,
    )
}

/// The shared candidate-summary object generation tools return.
fn candidate_summary_schema() -> Json {
    object(
        vec![
            ("candidate_id", id_string()),
            ("kind", typed("string")),
            ("label", typed("string")),
            ("strategy", typed("string")),
            ("resource_uri", typed("string")),
            ("trace_uri", typed("string")),
            ("chord_count", int_range(0, 100_000)),
            ("note_count", int_range(0, 1_000_000)),
            ("chords", arr_of(typed("string"))),
            ("confidence", num_range(0.0, 1.0)),
            ("score_total", typed("number")),
            (
                "score_components",
                arr_of(object(
                    vec![("name", typed("string")), ("value", typed("number"))],
                    &["name", "value"],
                    false,
                )),
            ),
            ("rule_ids", arr_of(typed("string"))),
            ("source_ids", arr_of(typed("string"))),
            (
                "loop",
                json_obj! {
                    "type" => Json::Arr(vec![Json::Str("object".into()), Json::Str("null".into())]),
                },
            ),
            ("explanation", typed("string")),
            ("parts", arr_of(typed("string"))),
        ],
        &[
            "candidate_id",
            "kind",
            "label",
            "strategy",
            "resource_uri",
            "trace_uri",
            "chord_count",
            "note_count",
            "chords",
            "confidence",
            "score_total",
            "score_components",
            "rule_ids",
            "source_ids",
            "loop",
            "explanation",
            "parts",
        ],
        false,
    )
}

/// The output schema shared by `harmony.generate_candidates`,
/// `harmony.reharmonize` and `voicing.generate`.
fn candidate_list_output(title: &str) -> Json {
    output(
        title,
        vec![
            ("snapshot_id", nullable_id_string()),
            ("analysis_id", nullable_id_string()),
            ("profile_id", typed("string")),
            ("seed", int_range(0, i64::MAX)),
            ("cached", typed("boolean")),
            ("candidate_count", int_range(0, 64)),
            ("candidates", arr_of(candidate_summary_schema())),
            ("knowledge_version", typed("string")),
        ],
        &[
            "snapshot_id",
            "analysis_id",
            "profile_id",
            "seed",
            "cached",
            "candidate_count",
            "candidates",
            "knowledge_version",
        ],
    )
}

/// Every tool this server exposes, in `tools/list` order.
///
/// The list is closed. Brief §15's prohibited tools — `execute_lua`,
/// `execute_shell`, `run_reaper_action`, `write_arbitrary_midi`,
/// `delete_track_by_name`, `edit_project_chunk`, filesystem access and REAPER
/// passthrough — appear nowhere in it, and there is no dynamic registration
/// path that could add one.
pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "reaper.status",
            title: "REAPER status",
            description:
                "Reports whether the REAPER bridge is running and what project it is attached to, \
                 plus this server's version and knowledge bundle. Works with no bridge running: \
                 the reply then says so rather than failing.",
            input_schema: root(
                "reaper.status arguments",
                object(vec![], &[], false),
            ),
            output_schema: output(
                "reaper.status result",
                vec![
                    ("bridge_connected", typed("boolean")),
                    ("bridge_configured", typed("boolean")),
                    ("heartbeat_age_seconds", nullable("number")),
                    ("bridge_version", nullable("string")),
                    ("reaper_version", nullable("string")),
                    ("ipc_protocol_version", typed("string")),
                    ("mcp_protocol_version", typed("string")),
                    ("server_version", typed("string")),
                    ("active_project", typed("boolean")),
                    ("project_uuid", nullable("string")),
                    ("project_name", nullable("string")),
                    ("project_path", nullable("string")),
                    ("play_state", nullable("integer")),
                    ("selected_item_count", int_range(0, 1_000_000)),
                    ("active_midi_editor", typed("boolean")),
                    ("knowledge_version", typed("string")),
                    ("knowledge_hash", typed("string")),
                    ("ipc_dir", nullable("string")),
                    ("session", typed("object")),
                ],
                &[
                    "bridge_connected",
                    "bridge_configured",
                    "heartbeat_age_seconds",
                    "bridge_version",
                    "reaper_version",
                    "ipc_protocol_version",
                    "mcp_protocol_version",
                    "server_version",
                    "active_project",
                    "project_uuid",
                    "project_name",
                    "project_path",
                    "play_state",
                    "selected_item_count",
                    "active_midi_editor",
                    "knowledge_version",
                    "knowledge_hash",
                    "ipc_dir",
                    "session",
                ],
            ),
            requires_bridge: false,
            mutates_project: false,
        },
        ToolSpec {
            name: "reaper.inspect_selection",
            title: "Inspect the REAPER selection",
            description:
                "Takes an immutable snapshot of the selected MIDI material and returns a \
                 snapshot id every later tool refers to. Reads only; never modifies the project.",
            input_schema: root(
                "reaper.inspect_selection arguments",
                object(
                    vec![
                        ("source_mode", enum_of(SOURCE_MODES)),
                        ("note_scope", enum_of(NOTE_SCOPES)),
                        ("melody_extraction", melody_extraction_schema()),
                    ],
                    &[],
                    false,
                ),
            ),
            output_schema: output(
                "reaper.inspect_selection result",
                vec![
                    ("snapshot_id", id_string()),
                    ("resource_uri", typed("string")),
                    ("project_uuid", nullable("string")),
                    ("project_state_change_count", typed("integer")),
                    ("track_guid", nullable("string")),
                    ("item_guid", nullable("string")),
                    ("take_guid", nullable("string")),
                    ("item_start_qn", typed("number")),
                    ("item_end_qn", typed("number")),
                    ("item_length_qn", typed("number")),
                    ("is_loop_source", typed("boolean")),
                    ("midi_hash", nullable("string")),
                    ("tempo_map_hash", nullable("string")),
                    ("snapshot_hash", nullable("string")),
                    ("note_list_hash", nullable("string")),
                    ("note_count", int_range(0, 1_000_000)),
                    ("source_note_count", int_range(0, 1_000_000)),
                    ("notes", arr_of(typed("object"))),
                    ("tempo_bpm_at_start", typed("number")),
                    ("time_signature_at_start", typed("object")),
                    ("note_scope", typed("string")),
                    ("extraction_mode", typed("string")),
                    ("extraction_channel", nullable("integer")),
                    ("resolved_by", nullable("string")),
                    ("selection_assumptions", arr_of(typed("string"))),
                ],
                &[
                    "snapshot_id",
                    "resource_uri",
                    "project_uuid",
                    "project_state_change_count",
                    "track_guid",
                    "item_guid",
                    "take_guid",
                    "item_start_qn",
                    "item_end_qn",
                    "item_length_qn",
                    "is_loop_source",
                    "midi_hash",
                    "tempo_map_hash",
                    "snapshot_hash",
                    "note_list_hash",
                    "note_count",
                    "source_note_count",
                    "notes",
                    "tempo_bpm_at_start",
                    "time_signature_at_start",
                    "note_scope",
                    "extraction_mode",
                    "extraction_channel",
                    "resolved_by",
                    "selection_assumptions",
                ],
            ),
            requires_bridge: true,
            mutates_project: false,
        },
        ToolSpec {
            name: "theory.search",
            title: "Search the theory knowledge base",
            description:
                "Searches the embedded music-theory bundle for rules, scales, chord qualities, \
                 progressions, cadences, profiles and sources. For explanation and discovery; \
                 it is not the harmonization engine.",
            input_schema: root(
                "theory.search arguments",
                object(
                    vec![
                        ("query", json_obj! { "type" => "string", "minLength" => 1, "maxLength" => 512 }),
                        ("domains", arr_of(enum_of(RULE_DOMAINS))),
                        ("profile", enum_of(PROFILE_IDS)),
                        ("kinds", arr_of(enum_of(RULE_KINDS))),
                        ("sources", arr_of(typed("string"))),
                        ("max_results", int_range(1, 100)),
                    ],
                    &["query"],
                    false,
                ),
            ),
            output_schema: output(
                "theory.search result",
                vec![
                    ("query", typed("string")),
                    ("knowledge_version", typed("string")),
                    ("knowledge_hash", typed("string")),
                    ("result_count", int_range(0, 100)),
                    (
                        "hits",
                        arr_of(object(
                            vec![
                                ("kind", typed("string")),
                                ("id", typed("string")),
                                ("title", typed("string")),
                                ("summary", typed("string")),
                                ("score", typed("number")),
                                ("source_ids", arr_of(typed("string"))),
                                ("detail", typed("object")),
                            ],
                            &["kind", "id", "title", "summary", "score", "source_ids", "detail"],
                            false,
                        )),
                    ),
                    ("source_ids", arr_of(typed("string"))),
                    ("profiles", arr_of(typed("string"))),
                ],
                &[
                    "query",
                    "knowledge_version",
                    "knowledge_hash",
                    "result_count",
                    "hits",
                    "source_ids",
                    "profiles",
                ],
            ),
            requires_bridge: false,
            mutates_project: false,
        },
        ToolSpec {
            name: "music.analyze_selection",
            title: "Analyze the snapshot",
            description:
                "Runs melody, key, phrase, salience, non-chord-tone and harmonic-grid analysis \
                 over a snapshot and returns an analysis id plus a ranked reading. Never asserts \
                 a single key; it ranks candidates and reports the evidence.",
            input_schema: root(
                "music.analyze_selection arguments",
                object(
                    vec![
                        ("snapshot_id", id_string()),
                        ("style_profile", enum_of(PROFILE_IDS)),
                        ("melody_extraction", melody_extraction_schema()),
                        (
                            "tonal_center",
                            object(
                                vec![("tonic", typed("string")), ("scale_id", typed("string"))],
                                &["tonic", "scale_id"],
                                false,
                            ),
                        ),
                        (
                            "meter",
                            json_obj! { "type" => "string", "pattern" => "^[0-9]{1,2}/[0-9]{1,2}$" },
                        ),
                        ("harmonic_rhythm", harmonic_rhythm_schema()),
                        ("strictness", enum_of(STRICTNESS)),
                        (
                            "loop_span",
                            object(
                                vec![
                                    ("start_qn", num_range(-100_000.0, 1_000_000.0)),
                                    ("end_qn", num_range(-100_000.0, 1_000_000.0)),
                                ],
                                &["start_qn", "end_qn"],
                                false,
                            ),
                        ),
                    ],
                    &["snapshot_id"],
                    false,
                ),
            ),
            output_schema: output(
                "music.analyze_selection result",
                vec![
                    ("analysis_id", id_string()),
                    ("snapshot_id", id_string()),
                    ("profile_id", typed("string")),
                    ("resource_uri", typed("string")),
                    ("confidence", num_range(0.0, 1.0)),
                    ("knowledge_version", typed("string")),
                    ("key", typed("object")),
                    ("phrases", arr_of(typed("object"))),
                    ("motives", arr_of(typed("object"))),
                    ("structural_notes", arr_of(typed("integer"))),
                    ("nct_hypotheses", arr_of(typed("object"))),
                    ("grid", typed("object")),
                    ("melody", typed("object")),
                    ("extraction", typed("object")),
                    ("detected_chords", arr_of(typed("string"))),
                    ("loop_observations", arr_of(warning_schema())),
                ],
                &[
                    "analysis_id",
                    "snapshot_id",
                    "profile_id",
                    "resource_uri",
                    "confidence",
                    "knowledge_version",
                    "key",
                    "phrases",
                    "motives",
                    "structural_notes",
                    "nct_hypotheses",
                    "grid",
                    "melody",
                    "extraction",
                    "detected_chords",
                    "loop_observations",
                ],
            ),
            requires_bridge: false,
            mutates_project: false,
        },
        ToolSpec {
            name: "harmony.generate_candidates",
            title: "Generate harmonization candidates",
            description:
                "Generates genuinely different harmonizations of the analyzed melody, each with \
                 a full decision trace naming the theory rules and sources behind it. Preserves \
                 the melody and its timing unless told otherwise.",
            input_schema: root(
                "harmony.generate_candidates arguments",
                object(
                    vec![
                        ("snapshot_id", id_string()),
                        ("analysis_id", id_string()),
                        ("style_profile", enum_of(PROFILE_IDS)),
                        ("candidate_count", int_range(1, 8)),
                        ("preserve_melody", typed("boolean")),
                        ("preserve_rhythm", typed("boolean")),
                        ("harmonic_rhythm", harmonic_rhythm_schema()),
                        ("complexity", num_range(0.0, 1.0)),
                        ("chromaticism", num_range(0.0, 1.0)),
                        ("extension_density", num_range(0.0, 1.0)),
                        ("bass_motion", enum_of(BASS_MOTIONS)),
                        ("countermelody", countermelody_schema()),
                        ("loop_intent", enum_of(LOOP_INTENTS)),
                        ("strictness", enum_of(STRICTNESS)),
                        ("voice_count", int_range(2, 8)),
                        ("seed", int_range(0, i64::MAX)),
                    ],
                    &["snapshot_id"],
                    false,
                ),
            ),
            output_schema: candidate_list_output("harmony.generate_candidates result"),
            requires_bridge: false,
            mutates_project: false,
        },
        ToolSpec {
            name: "harmony.reharmonize",
            title: "Reharmonize an existing progression",
            description:
                "Reharmonizes the chords detected in a snapshot, or a generated candidate's \
                 chords, under explicit preservation constraints.",
            input_schema: root(
                "harmony.reharmonize arguments",
                object(
                    vec![
                        ("snapshot_id", id_string()),
                        ("analysis_id", id_string()),
                        ("candidate_id", id_string()),
                        ("style_profile", enum_of(PROFILE_IDS)),
                        ("candidate_count", int_range(1, 8)),
                        ("preserve_melody", typed("boolean")),
                        ("preserve_bass", typed("boolean")),
                        ("preserve_cadence", typed("boolean")),
                        ("preserve_harmonic_rhythm", typed("boolean")),
                        ("families", arr_of(typed("string"))),
                        ("complexity", num_range(0.0, 1.0)),
                        ("chromaticism", num_range(0.0, 1.0)),
                        ("seed", int_range(0, i64::MAX)),
                    ],
                    &[],
                    false,
                ),
            ),
            output_schema: candidate_list_output("harmony.reharmonize result"),
            requires_bridge: false,
            mutates_project: false,
        },
        ToolSpec {
            name: "voicing.generate",
            title: "Generate alternate voicings",
            description:
                "Re-voices a generated candidate's chords across a set of voicing families, \
                 keeping voice identity across time and reporting the voice-leading audit for \
                 each variant.",
            input_schema: root(
                "voicing.generate arguments",
                object(
                    vec![
                        ("candidate_id", id_string()),
                        ("style_profile", enum_of(PROFILE_IDS)),
                        ("voice_count", int_range(2, 8)),
                        ("families", arr_of(enum_of(VOICING_FAMILIES))),
                        ("low", int_range(0, 127)),
                        ("high", int_range(0, 127)),
                        ("preserve_top", typed("boolean")),
                        ("preserve_bass", typed("boolean")),
                        ("max_leap", int_range(1, 36)),
                        ("instrument_profile", typed("string")),
                        ("candidate_count", int_range(1, 8)),
                        ("seed", int_range(0, i64::MAX)),
                    ],
                    &["candidate_id"],
                    false,
                ),
            ),
            output_schema: candidate_list_output("voicing.generate result"),
            requires_bridge: false,
            mutates_project: false,
        },
        ToolSpec {
            name: "arrangement.generate",
            title: "Generate role-based parts",
            description:
                "Turns a harmonization candidate into role-based parts — bass, pad, comping, \
                 pulse and the rest — respecting instrument ranges, an energy curve and a \
                 masking budget.",
            input_schema: root(
                "arrangement.generate arguments",
                object(
                    vec![
                        ("candidate_id", id_string()),
                        ("style_profile", enum_of(PROFILE_IDS)),
                        ("roles", arr_of(enum_of(ARRANGEMENT_ROLES))),
                        (
                            "energy_curve",
                            arr_of(object(
                                vec![
                                    ("qn", num_range(-100_000.0, 1_000_000.0)),
                                    ("value", num_range(0.0, 1.0)),
                                ],
                                &["qn", "value"],
                                false,
                            )),
                        ),
                        ("density", num_range(0.0, 1.0)),
                        ("texture_pattern", typed("string")),
                        ("register_spread", num_range(0.0, 1.0)),
                        (
                            "sections",
                            arr_of(object(
                                vec![
                                    ("id", typed("string")),
                                    ("start_qn", num_range(-100_000.0, 1_000_000.0)),
                                    ("end_qn", num_range(-100_000.0, 1_000_000.0)),
                                    ("role", typed("string")),
                                    ("energy", num_range(0.0, 1.0)),
                                ],
                                &["id", "start_qn", "end_qn"],
                                false,
                            )),
                        ),
                        ("preserve_melody", typed("boolean")),
                        ("loop_intent", enum_of(LOOP_INTENTS)),
                        ("seed", int_range(0, i64::MAX)),
                    ],
                    &["candidate_id"],
                    false,
                ),
            ),
            output_schema: output(
                "arrangement.generate result",
                vec![
                    ("candidate_id", id_string()),
                    ("source_candidate_id", id_string()),
                    ("resource_uri", typed("string")),
                    ("trace_uri", typed("string")),
                    ("profile_id", typed("string")),
                    ("seed", int_range(0, i64::MAX)),
                    ("parts", arr_of(typed("object"))),
                    ("assignments", arr_of(typed("object"))),
                    ("energy", arr_of(typed("object"))),
                    ("masking", typed("object")),
                    ("score_total", typed("number")),
                    ("note_count", int_range(0, 1_000_000)),
                    ("knowledge_version", typed("string")),
                ],
                &[
                    "candidate_id",
                    "source_candidate_id",
                    "resource_uri",
                    "trace_uri",
                    "profile_id",
                    "seed",
                    "parts",
                    "assignments",
                    "energy",
                    "masking",
                    "score_total",
                    "note_count",
                    "knowledge_version",
                ],
            ),
            requires_bridge: false,
            mutates_project: false,
        },
        ToolSpec {
            name: "loop.audit",
            title: "Audit loop compatibility",
            description:
                "Audits the wrap point of a loop: harmonic, bass and voice-leading continuity, \
                 hanging notes, pickup and tail behaviour, and concrete repairs. Works on a \
                 snapshot, an analysis or a generated candidate.",
            input_schema: root(
                "loop.audit arguments",
                object(
                    vec![
                        ("snapshot_id", id_string()),
                        ("analysis_id", id_string()),
                        ("candidate_id", id_string()),
                        ("transaction_id", id_string()),
                        ("style_profile", enum_of(PROFILE_IDS)),
                        ("loop_intent", enum_of(LOOP_INTENTS)),
                        (
                            "loop_span",
                            object(
                                vec![
                                    ("start_qn", num_range(-100_000.0, 1_000_000.0)),
                                    ("end_qn", num_range(-100_000.0, 1_000_000.0)),
                                ],
                                &["start_qn", "end_qn"],
                                false,
                            ),
                        ),
                    ],
                    &[],
                    false,
                ),
            ),
            output_schema: output(
                "loop.audit result",
                vec![
                    ("target_kind", typed("string")),
                    ("target_id", nullable_id_string()),
                    ("intent", nullable("string")),
                    ("loop_start_qn", typed("number")),
                    ("loop_end_qn", typed("number")),
                    ("compatible", typed("boolean")),
                    ("score", num_range(0.0, 1.0)),
                    ("confidence", num_range(0.0, 1.0)),
                    ("harmonic_wrap", typed("string")),
                    ("bass_wrap", typed("string")),
                    ("voice_leading_wrap", typed("string")),
                    ("hanging_notes", arr_of(typed("integer"))),
                    ("crossing_notes", arr_of(typed("integer"))),
                    ("pickup_qn", typed("number")),
                    ("tail_qn", typed("number")),
                    ("length_exact", typed("boolean")),
                    ("findings", arr_of(warning_schema())),
                    ("repairs", arr_of(typed("object"))),
                ],
                &[
                    "target_kind",
                    "target_id",
                    "intent",
                    "loop_start_qn",
                    "loop_end_qn",
                    "compatible",
                    "score",
                    "confidence",
                    "harmonic_wrap",
                    "bass_wrap",
                    "voice_leading_wrap",
                    "hanging_notes",
                    "crossing_notes",
                    "pickup_qn",
                    "tail_qn",
                    "length_exact",
                    "findings",
                    "repairs",
                ],
            ),
            requires_bridge: false,
            mutates_project: false,
        },
        ToolSpec {
            name: "candidate.explain",
            title: "Explain a candidate",
            description:
                "Returns the decision trace behind a server-issued candidate: score components, \
                 the theory rules that fired, the sources behind them, the assumptions made and \
                 the alternatives rejected.",
            input_schema: root(
                "candidate.explain arguments",
                object(
                    vec![
                        ("candidate_id", id_string()),
                        ("detail", enum_of(&["concise", "detailed"])),
                    ],
                    &["candidate_id"],
                    false,
                ),
            ),
            output_schema: output(
                "candidate.explain result",
                vec![
                    ("candidate_id", id_string()),
                    ("detail", typed("string")),
                    ("label", typed("string")),
                    ("strategy", typed("string")),
                    ("snapshot_id", typed("string")),
                    ("analysis_id", typed("string")),
                    ("profile_id", typed("string")),
                    ("seed", typed("integer")),
                    ("knowledge_version", typed("string")),
                    ("confidence", num_range(0.0, 1.0)),
                    ("explanation", typed("string")),
                    ("assumptions", arr_of(typed("string"))),
                    ("score", typed("object")),
                    ("rules", arr_of(typed("object"))),
                    ("sources", arr_of(typed("object"))),
                    ("rejected_alternatives", arr_of(typed("string"))),
                    ("chords", arr_of(typed("string"))),
                    ("trace_uri", typed("string")),
                ],
                &[
                    "candidate_id",
                    "detail",
                    "label",
                    "strategy",
                    "snapshot_id",
                    "analysis_id",
                    "profile_id",
                    "seed",
                    "knowledge_version",
                    "confidence",
                    "explanation",
                    "assumptions",
                    "score",
                    "rules",
                    "sources",
                    "rejected_alternatives",
                    "chords",
                    "trace_uri",
                ],
            ),
            requires_bridge: false,
            mutates_project: false,
        },
        ToolSpec {
            name: "reaper.stage_candidate",
            title: "Stage a candidate into REAPER",
            description:
                "Writes a server-issued candidate into REAPER as new, muted, tagged tracks in \
                 their own folder. The source item is never touched. The plan carries the full \
                 precondition set, so material edited since the snapshot is rejected rather \
                 than overwritten.",
            input_schema: root(
                "reaper.stage_candidate arguments",
                object(
                    vec![
                        ("candidate_id", id_string()),
                        (
                            "track_name_prefix",
                            json_obj! { "type" => "string", "maxLength" => 64 },
                        ),
                        (
                            "folder_name",
                            json_obj! { "type" => "string", "maxLength" => 128 },
                        ),
                        ("muted", typed("boolean")),
                        ("create_region", typed("boolean")),
                        ("route_to_source_track", typed("boolean")),
                        ("verify_snapshot", typed("boolean")),
                    ],
                    &["candidate_id"],
                    false,
                ),
            ),
            output_schema: output(
                "reaper.stage_candidate result",
                vec![
                    ("transaction_id", id_string()),
                    ("plan_id", id_string()),
                    ("candidate_id", id_string()),
                    ("snapshot_id", id_string()),
                    ("undo_label", typed("string")),
                    ("status", typed("string")),
                    ("tracks", arr_of(typed("object"))),
                    ("items", arr_of(typed("object"))),
                    ("regions", arr_of(typed("object"))),
                    ("sends", arr_of(typed("object"))),
                    ("note_count", int_range(0, 1_000_000)),
                    ("project_state_change_count", typed("integer")),
                    ("scope_echoed", typed("object")),
                    ("precondition_kinds", arr_of(typed("string"))),
                    ("plan_uri", typed("string")),
                    ("transaction_uri", typed("string")),
                ],
                &[
                    "transaction_id",
                    "plan_id",
                    "candidate_id",
                    "snapshot_id",
                    "undo_label",
                    "status",
                    "tracks",
                    "items",
                    "regions",
                    "sends",
                    "note_count",
                    "project_state_change_count",
                    "scope_echoed",
                    "precondition_kinds",
                    "plan_uri",
                    "transaction_uri",
                ],
            ),
            requires_bridge: true,
            mutates_project: true,
        },
        ToolSpec {
            name: "reaper.commit_candidate",
            title: "Commit a staged candidate",
            description:
                "Flips a staged transaction's tagged objects from preview to committed and \
                 unmutes them. The source item is never merged into or deleted.",
            input_schema: root(
                "reaper.commit_candidate arguments",
                object(
                    vec![("transaction_id", id_string())],
                    &["transaction_id"],
                    false,
                ),
            ),
            output_schema: output(
                "reaper.commit_candidate result",
                vec![
                    ("transaction_id", id_string()),
                    ("status", typed("string")),
                    ("undo_label", typed("string")),
                    ("committed_tracks", typed("integer")),
                    ("committed_items", typed("integer")),
                    ("committed_takes", typed("integer")),
                    ("project_state_change_count", typed("integer")),
                ],
                &[
                    "transaction_id",
                    "status",
                    "undo_label",
                    "committed_tracks",
                    "committed_items",
                    "committed_takes",
                    "project_state_change_count",
                ],
            ),
            requires_bridge: true,
            mutates_project: true,
        },
        ToolSpec {
            name: "reaper.discard_candidate",
            title: "Discard a staged candidate",
            description:
                "Deletes only the objects carrying this transaction's QLabs ownership tags. \
                 Objects are never identified by track or item name.",
            input_schema: root(
                "reaper.discard_candidate arguments",
                object(
                    vec![("transaction_id", id_string())],
                    &["transaction_id"],
                    false,
                ),
            ),
            output_schema: output(
                "reaper.discard_candidate result",
                vec![
                    ("transaction_id", id_string()),
                    ("undo_label", typed("string")),
                    ("removed_items", typed("integer")),
                    ("removed_tracks", typed("integer")),
                    ("retained_tracks", typed("integer")),
                    ("project_state_change_count", typed("integer")),
                ],
                &[
                    "transaction_id",
                    "undo_label",
                    "removed_items",
                    "removed_tracks",
                    "retained_tracks",
                    "project_state_change_count",
                ],
            ),
            requires_bridge: true,
            mutates_project: true,
        },
        ToolSpec {
            name: "reaper.undo_last_generation",
            title: "Undo the last owned generation",
            description:
                "Undoes at most one undo entry, and only when it is this server's own last \
                 transaction. Anything else returns UNDO_NOT_OWNED; an unrelated REAPER action \
                 is never undone.",
            input_schema: root(
                "reaper.undo_last_generation arguments",
                object(vec![("transaction_id", id_string())], &[], false),
            ),
            output_schema: output(
                "reaper.undo_last_generation result",
                vec![
                    ("undone", typed("boolean")),
                    ("transaction_id", nullable_id_string()),
                    ("undo_label", nullable("string")),
                    ("kind", nullable("string")),
                    ("project_state_change_count", typed("integer")),
                ],
                &[
                    "undone",
                    "transaction_id",
                    "undo_label",
                    "kind",
                    "project_state_change_count",
                ],
            ),
            requires_bridge: true,
            mutates_project: true,
        },
    ]
}

/// Tool names this server must never expose (brief §15).
pub const PROHIBITED_TOOL_NAMES: &[&str] = &[
    "execute_lua",
    "execute_shell",
    "run_reaper_action",
    "write_arbitrary_midi",
    "delete_track_by_name",
    "edit_project_chunk",
    "read_file",
    "write_file",
    "list_directory",
    "reaper_api",
    "reaper_passthrough",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_schema_compiles() {
        ToolRegistry::compile().expect("every declared schema must compile");
    }

    #[test]
    fn the_tool_list_matches_the_frozen_surface() {
        let r = ToolRegistry::compile().unwrap();
        assert_eq!(
            r.names(),
            vec![
                "reaper.status",
                "reaper.inspect_selection",
                "theory.search",
                "music.analyze_selection",
                "harmony.generate_candidates",
                "harmony.reharmonize",
                "voicing.generate",
                "arrangement.generate",
                "loop.audit",
                "candidate.explain",
                "reaper.stage_candidate",
                "reaper.commit_candidate",
                "reaper.discard_candidate",
                "reaper.undo_last_generation",
            ]
        );
    }

    #[test]
    fn no_prohibited_tool_is_exposed() {
        let r = ToolRegistry::compile().unwrap();
        for bad in PROHIBITED_TOOL_NAMES {
            assert!(r.get(bad).is_none(), "{bad} must not be a tool");
        }
        let listed = r.list_json().to_string();
        for bad in PROHIBITED_TOOL_NAMES {
            assert!(!listed.contains(bad), "{bad} must not appear in tools/list");
        }
    }

    #[test]
    fn every_tool_declares_both_schemas() {
        let r = ToolRegistry::compile().unwrap();
        for t in r.all() {
            let entry = t.spec.to_json();
            assert!(entry.get("inputSchema").is_some(), "{}", t.spec.name);
            assert!(entry.get("outputSchema").is_some(), "{}", t.spec.name);
            assert_eq!(
                entry.get("inputSchema").unwrap().str_field("$schema"),
                Ok(DIALECT)
            );
            assert_eq!(
                entry.get("outputSchema").unwrap().str_field("$schema"),
                Ok(DIALECT)
            );
        }
    }

    #[test]
    fn input_schemas_are_closed() {
        let r = ToolRegistry::compile().unwrap();
        for t in r.all() {
            assert_eq!(
                t.spec.input_schema.get("additionalProperties"),
                Some(&Json::Bool(false)),
                "{} must reject unknown arguments",
                t.spec.name
            );
        }
    }

    #[test]
    fn candidate_count_is_bounded_to_eight() {
        let r = ToolRegistry::compile().unwrap();
        let t = r.get("harmony.generate_candidates").unwrap();
        for n in [0, 9, 100] {
            let v = json_obj! { "snapshot_id" => "s1", "candidate_count" => n };
            assert!(
                !t.input.validate(&v).is_empty(),
                "candidate_count {n} must be rejected"
            );
        }
        for n in [1, 3, 8] {
            let v = json_obj! { "snapshot_id" => "s1", "candidate_count" => n };
            assert!(
                t.input.validate(&v).is_empty(),
                "candidate_count {n} must be accepted"
            );
        }
    }

    #[test]
    fn normalized_controls_are_bounded_to_zero_one() {
        let r = ToolRegistry::compile().unwrap();
        let t = r.get("harmony.generate_candidates").unwrap();
        for key in ["complexity", "chromaticism", "extension_density"] {
            for bad in [-0.1, 1.1] {
                let mut m = JsonMap::new();
                m.insert("snapshot_id", Json::Str("s1".into()));
                m.insert(key, Json::Float(bad));
                assert!(
                    !t.input.validate(&Json::Obj(m)).is_empty(),
                    "{key} = {bad} must be rejected"
                );
            }
        }
    }

    #[test]
    fn unknown_profiles_are_rejected() {
        let r = ToolRegistry::compile().unwrap();
        let t = r.get("music.analyze_selection").unwrap();
        let v = json_obj! { "snapshot_id" => "s1", "style_profile" => "not_a_profile" };
        assert!(!t.input.validate(&v).is_empty());
        let v = json_obj! { "snapshot_id" => "s1", "style_profile" => "jazz_standard" };
        assert!(t.input.validate(&v).is_empty());
    }

    #[test]
    fn an_unknown_argument_is_rejected() {
        let r = ToolRegistry::compile().unwrap();
        let t = r.get("music.analyze_selection").unwrap();
        let v = json_obj! { "snapshot_id" => "s1", "styleprofile" => "jazz_standard" };
        assert!(!t.input.validate(&v).is_empty());
    }

    #[test]
    fn ids_cannot_name_a_path() {
        let r = ToolRegistry::compile().unwrap();
        let t = r.get("candidate.explain").unwrap();
        for bad in [
            "../../etc/passwd",
            "/etc/passwd",
            "a/b",
            "..",
            "",
            "C:\\Windows",
        ] {
            let v = json_obj! { "candidate_id" => bad };
            assert!(
                !t.input.validate(&v).is_empty(),
                "{bad:?} must not pass the id pattern"
            );
        }
    }

    #[test]
    fn missing_required_arguments_are_rejected() {
        let r = ToolRegistry::compile().unwrap();
        let t = r.get("theory.search").unwrap();
        assert!(!t.input.validate(&json_obj! {}).is_empty());
        assert!(t
            .input
            .validate(&json_obj! { "query" => "tritone" })
            .is_empty());
    }

    #[test]
    fn transaction_tools_require_a_transaction_id() {
        let r = ToolRegistry::compile().unwrap();
        for name in ["reaper.commit_candidate", "reaper.discard_candidate"] {
            let t = r.get(name).unwrap();
            assert!(!t.input.validate(&json_obj! {}).is_empty(), "{name}");
        }
        // Undo may be called with no argument at all.
        let undo = r.get("reaper.undo_last_generation").unwrap();
        assert!(undo.input.validate(&json_obj! {}).is_empty());
    }

    #[test]
    fn annotations_mark_the_mutating_tools() {
        let r = ToolRegistry::compile().unwrap();
        let mutating: Vec<&str> = r
            .all()
            .iter()
            .filter(|t| t.spec.mutates_project)
            .map(|t| t.spec.name)
            .collect();
        assert_eq!(
            mutating,
            vec![
                "reaper.stage_candidate",
                "reaper.commit_candidate",
                "reaper.discard_candidate",
                "reaper.undo_last_generation",
            ]
        );
    }

    #[test]
    fn profile_ids_match_the_frozen_list() {
        assert_eq!(PROFILE_IDS.len(), 10);
        let kb = theory_kb::KnowledgeBase::embedded();
        for id in PROFILE_IDS {
            assert!(kb.profile(id).is_some(), "{id} must exist in the bundle");
        }
    }

    #[test]
    fn descriptions_are_present_and_meaningful() {
        for spec in tool_specs() {
            assert!(spec.description.len() > 40, "{}", spec.name);
            assert!(!spec.title.is_empty(), "{}", spec.name);
        }
    }
}
