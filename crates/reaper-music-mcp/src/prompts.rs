//! MCP prompt templates.
//!
//! Nine user-controlled prompts. Each one renders to a single user message that
//! tells the client model which semantic tools to call and in what order.
//!
//! # Prompts never bypass validation
//!
//! A prompt is text. It cannot call a tool, cannot supply a snapshot id and
//! cannot widen a schema — every instruction it produces still has to go
//! through `tools/call`, where the input schema is enforced exactly as it would
//! be for a hand-written call. Each template therefore says *what to ask for*,
//! and deliberately never says "skip the snapshot" or "stage without
//! confirmation": staging is always described as requiring the user's explicit
//! choice, which matches the tool layer's own refusal to stage anything but a
//! server-issued candidate id.

use crate::error::{codes, ToolError};
use crate::rpc::{rpc_codes, RpcError};
use qjson::{json_obj, Json};

/// One argument of a prompt template.
#[derive(Clone, Copy, Debug)]
pub struct PromptArg {
    /// The argument name.
    pub name: &'static str,
    /// What it means.
    pub description: &'static str,
    /// Whether the client must supply it.
    pub required: bool,
}

/// One prompt template.
#[derive(Clone, Debug)]
pub struct PromptSpec {
    /// The stable prompt name.
    pub name: &'static str,
    /// A short human title.
    pub title: &'static str,
    /// What the prompt is for.
    pub description: &'static str,
    /// The declared arguments.
    pub arguments: &'static [PromptArg],
    /// The template body. `{name}` placeholders are filled from the arguments.
    pub template: &'static str,
}

const STYLE_ARG: PromptArg = PromptArg {
    name: "style_profile",
    description: "A style profile id, for example jazz_standard or modal_ambient.",
    required: false,
};

const COUNT_ARG: PromptArg = PromptArg {
    name: "candidate_count",
    description: "How many candidates to ask for, 1 through 8.",
    required: false,
};

const FOCUS_ARG: PromptArg = PromptArg {
    name: "focus",
    description: "A free-text description of the sound the user is after.",
    required: false,
};

const CANDIDATE_ARG: PromptArg = PromptArg {
    name: "candidate_id",
    description: "A candidate id this server issued.",
    required: true,
};

const LOOP_ARG: PromptArg = PromptArg {
    name: "loop_intent",
    description: "closed_tonic, open_dominant, modal_drone, seamless_color, transition_ready or \
                  one_shot_ending.",
    required: false,
};

/// Every prompt, in `prompts/list` order.
pub fn prompt_specs() -> Vec<PromptSpec> {
    vec![
        PromptSpec {
            name: "analyze-selected-melody",
            title: "Analyze the selected melody",
            description:
                "Read the selected MIDI and report the key, phrases, structural notes and \
                 harmonic grid, without generating anything.",
            arguments: &[STYLE_ARG],
            template: "\
Analyze the melody I have selected in REAPER.

1. Call `reaper.status` and tell me if the bridge is not running.
2. Call `reaper.inspect_selection` and keep the snapshot id.
3. Call `music.analyze_selection` with that snapshot id{style_clause}.

Then summarise, in plain language: the ranked key and mode candidates with their confidence and \
the evidence behind them, the phrase structure and any pickup, which notes are structural, which \
are likely non-chord tones and why, the harmonic-rhythm the grid suggests, and the range and \
tessitura. Say clearly where the reading is ambiguous rather than picking one answer. Do not \
generate harmony and do not stage anything.",
        },
        PromptSpec {
            name: "harmonize-selected-melody",
            title: "Harmonize the selected melody",
            description: "The full workflow: status, inspect, analyze, generate several genuinely \
                 different harmonizations, explain the differences, stage only on request.",
            arguments: &[STYLE_ARG, COUNT_ARG, FOCUS_ARG],
            template: "\
Harmonize the melody I have selected in REAPER.{focus_clause}

1. Call `reaper.status`. If the bridge is offline, stop and tell me how to start it.
2. Call `reaper.inspect_selection`.
3. Call `music.analyze_selection` on the snapshot.
4. Call `harmony.generate_candidates` for {candidate_count_clause} candidates{style_clause}, with \
   `preserve_melody` and `preserve_rhythm` both true.
5. Call `candidate.explain` on each candidate.

Then compare them for me: name each strategy, quote the score components that separate them, and \
name the theory rules and sources each one leaned on. Say which one you would pick and why.

Do not call `reaper.stage_candidate` until I tell you which candidate I want. When I do, stage \
that one candidate id and nothing else.",
        },
        PromptSpec {
            name: "reharmonize-selected-region",
            title: "Reharmonize the selected region",
            description: "Reharmonize existing chords under explicit preservation constraints.",
            arguments: &[
                STYLE_ARG,
                COUNT_ARG,
                PromptArg {
                    name: "preserve",
                    description:
                        "What must survive: any of melody, bass, cadence, harmonic_rhythm.",
                    required: false,
                },
            ],
            template: "\
Reharmonize what I have selected in REAPER.

1. Call `reaper.inspect_selection`, then `music.analyze_selection`.
2. Call `harmony.reharmonize` on that snapshot{style_clause}, asking for \
   {candidate_count_clause} candidates.{preserve_clause}

Report what changed chord by chord: which chords were substituted, what transformation family \
each substitution came from, and what the melody note became over the new chord. Flag anything \
that weakened a cadence. Stage nothing until I choose.",
        },
        PromptSpec {
            name: "extend-selected-chords",
            title: "Add extensions to the selected chords",
            description: "Raise extension density and complexity on an existing progression while \
                 keeping its function.",
            arguments: &[STYLE_ARG, FOCUS_ARG],
            template: "\
Add extensions and colour to the chords I have selected, without changing their function.

1. Call `reaper.inspect_selection`, then `music.analyze_selection`.
2. Call `harmony.reharmonize` with `preserve_cadence` and `preserve_harmonic_rhythm` true and a \
   high `complexity`{style_clause}.{focus_clause}

For each chord, tell me which extension or alteration was added, why it is available over that \
melody note, and where an avoid tone was deliberately left out. Do not stage anything yet.",
        },
        PromptSpec {
            name: "create-smooth-voicings",
            title: "Create smooth voicings",
            description: "Re-voice a candidate for minimal motion and good spacing.",
            arguments: &[
                CANDIDATE_ARG,
                STYLE_ARG,
                PromptArg {
                    name: "voice_count",
                    description: "How many voices, 2 through 8.",
                    required: false,
                },
            ],
            template: "\
Re-voice candidate {candidate_id} for smooth voice leading.

Call `voicing.generate` on it{style_clause}, with `preserve_top` true so the melody stays on top, \
and ask for several voicing families so I can compare.

Report, per variant: total voice motion, the largest leap, any parallel perfect intervals, any \
crossings or overlaps, and any tendency tone left unresolved. Recommend one and say why. Stage \
only if I ask.",
        },
        PromptSpec {
            name: "arrange-selected-sketch",
            title: "Arrange the selected sketch",
            description: "Turn a candidate into role-based parts with an energy shape.",
            arguments: &[
                CANDIDATE_ARG,
                STYLE_ARG,
                PromptArg {
                    name: "roles",
                    description: "Comma-separated arrangement roles, for example bass,pad,comping.",
                    required: false,
                },
            ],
            template: "\
Arrange candidate {candidate_id} into parts.

Call `arrangement.generate` on it{style_clause}{roles_clause}, then read the masking report.

Tell me what each part does, what register it occupies, how dense it is, and where the \
arrangement uses silence rather than volume for contrast. Name any collision the masking report \
found and what you would change. Stage only the arrangement I approve.",
        },
        PromptSpec {
            name: "create-loopable-variants",
            title: "Create loopable variants",
            description:
                "Generate candidates that wrap cleanly for a given loop intent, and audit each.",
            arguments: &[STYLE_ARG, COUNT_ARG, LOOP_ARG],
            template: "\
Make the selected material loop cleanly.

1. Call `reaper.inspect_selection`, then `music.analyze_selection` with the loop span set to the \
   region I selected.
2. Call `harmony.generate_candidates` for {candidate_count_clause} candidates{style_clause}\
{loop_clause}.
3. Call `loop.audit` on each candidate.

For each one, report the harmonic wrap, the bass wrap, the voice-leading wrap, any hanging or \
crossing notes, and the pickup and tail. Remember that a modal drone loop does not need a \
dominant resolution — do not force one. Recommend the variant that wraps best and say what it \
gives up.",
        },
        PromptSpec {
            name: "audit-harmony-and-voice-leading",
            title: "Audit harmony and voice leading",
            description: "Critique existing material against a style profile without changing it.",
            arguments: &[STYLE_ARG],
            template: "\
Audit what I have selected. Change nothing.

1. Call `reaper.inspect_selection`, then `music.analyze_selection`{style_clause}.
2. Call `loop.audit` on the snapshot if the region is meant to loop.
3. Call `theory.search` for the rules behind anything you flag, so every criticism cites a rule \
   id and a source.

Report parallel perfect intervals, unresolved tendency tones, doubling and spacing problems, \
awkward leaps, and any harmony that does not fit the profile — and be explicit when something is \
only a problem under a strict profile and is idiomatic under this one. Do not generate or stage.",
        },
        PromptSpec {
            name: "explain-generated-candidate",
            title: "Explain a generated candidate",
            description: "Walk through one candidate's decision trace in detail.",
            arguments: &[
                CANDIDATE_ARG,
                PromptArg {
                    name: "detail",
                    description: "concise or detailed.",
                    required: false,
                },
            ],
            template: "\
Explain candidate {candidate_id}.

Call `candidate.explain` with `detail` set to {detail_clause}, then read \
`candidate://{candidate_id}/trace` for anything the summary left out.

Walk me through it: the key reading it assumed, the chord chosen for each grid slot and why, the \
score components and what each one contributed, the theory rules that fired and the sources \
behind them, the assumptions made where the material was ambiguous, and the alternatives that \
were rejected. Quote real rule ids and source ids — do not paraphrase them away.",
        },
    ]
}

/// The `prompts/list` payload.
pub fn list_json() -> Json {
    Json::Arr(
        prompt_specs()
            .iter()
            .map(|p| {
                json_obj! {
                    "name" => p.name,
                    "title" => p.title,
                    "description" => p.description,
                    "arguments" => Json::Arr(
                        p.arguments
                            .iter()
                            .map(|a| json_obj! {
                                "name" => a.name,
                                "description" => a.description,
                                "required" => a.required,
                            })
                            .collect(),
                    ),
                }
            })
            .collect(),
    )
}

/// Looks a prompt up by name.
pub fn get(name: &str) -> Option<PromptSpec> {
    prompt_specs().into_iter().find(|p| p.name == name)
}

/// Renders a prompt with the supplied arguments.
pub fn render(spec: &PromptSpec, args: &Json) -> Result<Json, ToolError> {
    let arg = |name: &str| args.get(name).and_then(Json::as_str).unwrap_or("").trim();

    for declared in spec.arguments {
        if declared.required && arg(declared.name).is_empty() {
            return Err(ToolError::with_details(
                codes::INVALID_ARGUMENTS,
                format!("prompt {} needs the {} argument", spec.name, declared.name),
                json_obj! { "prompt" => spec.name, "argument" => declared.name },
            ));
        }
    }

    let style = arg("style_profile");
    let count = arg("candidate_count");
    let focus = arg("focus");
    let candidate = arg("candidate_id");
    let preserve = arg("preserve");
    let roles = arg("roles");
    let detail = arg("detail");
    let loop_intent = arg("loop_intent");

    let style_clause = if style.is_empty() {
        " using the style profile that best fits what you hear".to_string()
    } else {
        format!(" with the `{style}` style profile")
    };
    let candidate_count_clause = if count.is_empty() {
        "three".to_string()
    } else {
        count.to_string()
    };
    let focus_clause = if focus.is_empty() {
        String::new()
    } else {
        format!(" I am after: {focus}.")
    };
    let preserve_clause = if preserve.is_empty() {
        String::new()
    } else {
        format!(" Preserve: {preserve}.")
    };
    let roles_clause = if roles.is_empty() {
        String::new()
    } else {
        format!(" with the roles {roles}")
    };
    let detail_clause = if detail.is_empty() {
        "detailed".to_string()
    } else {
        detail.to_string()
    };
    let loop_clause = if loop_intent.is_empty() {
        String::new()
    } else {
        format!(" and `loop_intent` set to `{loop_intent}`")
    };

    let text = spec
        .template
        .replace("{style_clause}", &style_clause)
        .replace("{candidate_count_clause}", &candidate_count_clause)
        .replace("{focus_clause}", &focus_clause)
        .replace("{preserve_clause}", &preserve_clause)
        .replace("{roles_clause}", &roles_clause)
        .replace("{detail_clause}", &detail_clause)
        .replace("{loop_clause}", &loop_clause)
        .replace("{candidate_id}", candidate);

    Ok(json_obj! {
        "description" => spec.description,
        "messages" => Json::Arr(vec![json_obj! {
            "role" => "user",
            "content" => json_obj! { "type" => "text", "text" => text },
        }]),
    })
}

/// Handles a `prompts/get` request.
pub fn get_request(params: &Json) -> Result<Json, RpcError> {
    let name = params
        .get("name")
        .and_then(Json::as_str)
        .ok_or_else(|| RpcError::new(rpc_codes::INVALID_PARAMS, "prompts/get needs a name"))?;
    let spec = get(name).ok_or_else(|| {
        RpcError::with_data(
            rpc_codes::INVALID_PARAMS,
            format!("unknown prompt {name:?}"),
            ToolError::with_details(
                codes::UNKNOWN_PROMPT,
                format!("unknown prompt {name}"),
                json_obj! { "prompt" => name },
            )
            .to_json(),
        )
    })?;
    let args = params.get("arguments").cloned().unwrap_or(Json::Null);
    render(&spec, &args)
        .map_err(|e| RpcError::with_data(rpc_codes::INVALID_PARAMS, e.message.clone(), e.to_json()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_set_is_the_frozen_nine() {
        let names: Vec<&str> = prompt_specs().iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            vec![
                "analyze-selected-melody",
                "harmonize-selected-melody",
                "reharmonize-selected-region",
                "extend-selected-chords",
                "create-smooth-voicings",
                "arrange-selected-sketch",
                "create-loopable-variants",
                "audit-harmony-and-voice-leading",
                "explain-generated-candidate",
            ]
        );
        assert_eq!(list_json().as_arr().unwrap().len(), 9);
    }

    #[test]
    fn every_prompt_renders_with_no_arguments_unless_one_is_required() {
        for spec in prompt_specs() {
            let has_required = spec.arguments.iter().any(|a| a.required);
            let rendered = render(&spec, &Json::Null);
            if has_required {
                assert!(rendered.is_err(), "{} must require its argument", spec.name);
            } else {
                let v = rendered.expect(spec.name);
                let text = v.arr_field("messages").unwrap()[0]
                    .get("content")
                    .unwrap()
                    .str_field("text")
                    .unwrap()
                    .to_string();
                assert!(
                    !text.contains('{'),
                    "{} left a placeholder: {text}",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn required_arguments_are_enforced() {
        let spec = get("explain-generated-candidate").unwrap();
        let e = render(&spec, &json_obj! {}).unwrap_err();
        assert_eq!(e.code, codes::INVALID_ARGUMENTS);
        let ok = render(&spec, &json_obj! { "candidate_id" => "cand-1" }).unwrap();
        let text = ok.arr_field("messages").unwrap()[0]
            .get("content")
            .unwrap()
            .str_field("text")
            .unwrap()
            .to_string();
        assert!(text.contains("cand-1"));
        assert!(!text.contains('{'));
    }

    #[test]
    fn the_harmonize_prompt_walks_the_acceptance_workflow() {
        let spec = get("harmonize-selected-melody").unwrap();
        let v = render(&spec, &json_obj! { "style_profile" => "jazz_standard" }).unwrap();
        let text = v.arr_field("messages").unwrap()[0]
            .get("content")
            .unwrap()
            .str_field("text")
            .unwrap()
            .to_string();
        for step in [
            "reaper.status",
            "reaper.inspect_selection",
            "music.analyze_selection",
            "harmony.generate_candidates",
            "candidate.explain",
        ] {
            assert!(text.contains(step), "missing {step}");
        }
        assert!(text.contains("jazz_standard"));
        assert!(
            text.contains("until I tell you"),
            "staging must wait for the user's choice"
        );
    }

    #[test]
    fn no_prompt_tells_the_client_to_bypass_validation() {
        for spec in prompt_specs() {
            let body = spec.template.to_ascii_lowercase();
            for forbidden in [
                "skip validation",
                "ignore the schema",
                "without a snapshot",
                "execute_lua",
                "run_reaper_action",
                "stage automatically",
                "stage without asking",
            ] {
                assert!(!body.contains(forbidden), "{}: {forbidden}", spec.name);
            }
        }
    }

    #[test]
    fn every_prompt_names_at_least_one_real_tool() {
        let tools = crate::schema_gen::ToolRegistry::compile().unwrap();
        let names = tools.names();
        for spec in prompt_specs() {
            assert!(
                names.iter().any(|n| spec.template.contains(n)),
                "{} names no tool",
                spec.name
            );
        }
    }

    #[test]
    fn get_request_rejects_an_unknown_prompt() {
        let e = get_request(&json_obj! { "name" => "nope" }).unwrap_err();
        assert_eq!(e.code, rpc_codes::INVALID_PARAMS);
        assert_eq!(
            e.data.unwrap().str_field("error_code").unwrap(),
            codes::UNKNOWN_PROMPT
        );
    }

    #[test]
    fn get_request_needs_a_name() {
        assert!(get_request(&json_obj! {}).is_err());
    }

    #[test]
    fn get_request_renders_a_known_prompt() {
        let v = get_request(&json_obj! {
            "name" => "analyze-selected-melody",
            "arguments" => json_obj! { "style_profile" => "modal_ambient" },
        })
        .unwrap();
        assert!(v.get("description").is_some());
        assert_eq!(v.arr_field("messages").unwrap().len(), 1);
    }

    #[test]
    fn the_loop_prompt_does_not_force_a_tonic_resolution() {
        let spec = get("create-loopable-variants").unwrap();
        assert!(spec
            .template
            .contains("does not need a dominant resolution"));
    }
}
