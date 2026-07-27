//! Every rejection the contract owes an external knowledge directory, driven
//! by deliberately-broken in-memory bundles built from the real one.
//!
//! Each test breaks exactly one thing, so a failure names the check that
//! regressed rather than "validation changed".

use qjson::{Json, JsonMap};
use std::collections::BTreeMap;
use theory_kb::error::KbError;
use theory_kb::load::KnowledgeBase;
use theory_kb::validate::{validate_report, ValidateOptions};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// A fresh copy of the real bundle's files, ready to be broken.
fn files() -> BTreeMap<String, String> {
    KnowledgeBase::embedded().files().clone()
}

/// Rewrites one file's `items` array, mapping each record.
///
/// Returning `None` from `f` drops the record.
fn edit_items(
    files: &mut BTreeMap<String, String>,
    path: &str,
    mut f: impl FnMut(JsonMap) -> Option<JsonMap>,
) {
    let text = files
        .get(path)
        .unwrap_or_else(|| panic!("{path} is absent"));
    let doc = Json::parse(text).expect("valid JSON");
    let root = doc.as_obj().expect("an object").clone();
    let items = doc.arr_field("items").expect("an items array").to_vec();
    let mut out = Vec::new();
    for item in items {
        let m = item.as_obj().expect("an object").clone();
        if let Some(new) = f(m) {
            out.push(Json::Obj(new));
        }
    }
    let mut root = root;
    root.insert("items", Json::Arr(out));
    files.insert(path.to_string(), Json::Obj(root).to_string());
}

/// Rewrites the single record whose `id` matches.
fn edit_item(
    files: &mut BTreeMap<String, String>,
    path: &str,
    id: &str,
    f: impl FnOnce(&mut JsonMap),
) {
    let mut applied = false;
    let mut f = Some(f);
    edit_items(files, path, |mut m| {
        if m.get("id").and_then(Json::as_str) == Some(id) {
            if let Some(func) = f.take() {
                func(&mut m);
                applied = true;
            }
        }
        Some(m)
    });
    assert!(applied, "{path} has no record with id '{id}'");
}

/// Rewrites a top-level field of a whole file (used for the manifest).
fn edit_root(files: &mut BTreeMap<String, String>, path: &str, f: impl FnOnce(&mut JsonMap)) {
    let text = files
        .get(path)
        .unwrap_or_else(|| panic!("{path} is absent"));
    let doc = Json::parse(text).expect("valid JSON");
    let mut root = doc.as_obj().expect("an object").clone();
    f(&mut root);
    files.insert(path.to_string(), Json::Obj(root).to_string());
}

/// Parses and validates, returning every problem. A parse failure counts as
/// one problem, because the loader stops there.
fn problems(files: BTreeMap<String, String>) -> Vec<KbError> {
    match KnowledgeBase::parse(files, "test-bundle") {
        Err(e) => vec![e],
        Ok(kb) => validate_report(
            &kb,
            &ValidateOptions {
                allow_pending_hash: true,
                ..ValidateOptions::default()
            },
        ),
    }
}

/// Asserts a specific error code is among the problems.
#[track_caller]
fn expect_code(problems: &[KbError], code: &str) {
    assert!(
        problems.iter().any(|e| e.code == code),
        "expected a {code}, got: {:#?}",
        problems.iter().map(|e| e.to_string()).collect::<Vec<_>>()
    );
}

/// The unmodified bundle must be clean, or every test below is meaningless.
#[test]
fn the_unmodified_bundle_has_no_problems() {
    assert!(problems(files()).is_empty());
}

// ---------------------------------------------------------------------------
// ids and references
// ---------------------------------------------------------------------------

#[test]
fn duplicate_rule_ids_are_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.blues_dominant_seventh_is_stable_tonic",
        |m| {
            m.insert(
                "id",
                Json::Str("harmony.dominant_seventh_resolves_down_fifth".into()),
            );
        },
    );
    expect_code(&problems(f), "KB_DUPLICATE_ID");
}

#[test]
fn duplicate_scale_ids_are_rejected() {
    let mut f = files();
    edit_item(&mut f, "scales.json", "dorian", |m| {
        m.insert("id", Json::Str("major".into()));
    });
    expect_code(&problems(f), "KB_DUPLICATE_ID");
}

#[test]
fn an_unresolved_source_reference_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.dominant_seventh_resolves_down_fifth",
        |m| {
            m.insert(
                "source_refs",
                Json::Arr(vec![Json::Obj(
                    [
                        ("source_id".to_string(), Json::Str("no-such-book".into())),
                        ("locator".to_string(), Json::Str("Chapter 1".into())),
                    ]
                    .into_iter()
                    .collect(),
                )]),
            );
        },
    );
    let p = problems(f);
    expect_code(&p, "KB_UNRESOLVED_REF");
    assert!(p.iter().any(|e| e.message.contains("no-such-book")));
}

#[test]
fn a_rule_naming_an_unknown_profile_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.dominant_seventh_resolves_down_fifth",
        |m| {
            m.insert("profiles", Json::Arr(vec![Json::Str("bebop_polka".into())]));
        },
    );
    let p = problems(f);
    expect_code(&p, "KB_UNRESOLVED_REF");
    assert!(p.iter().any(|e| e.message.contains("bebop_polka")));
}

#[test]
fn a_rule_naming_an_unknown_score_component_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.dominant_seventh_resolves_down_fifth",
        |m| {
            let mut effect = m.get("effect").and_then(Json::as_obj).cloned().unwrap();
            effect.insert("score_component", Json::Str("vibes".into()));
            m.insert("effect", Json::Obj(effect));
        },
    );
    let p = problems(f);
    expect_code(&p, "KB_UNRESOLVED_REF");
    assert!(p.iter().any(|e| e.message.contains("vibes")));
}

#[test]
fn a_record_naming_an_unknown_chord_quality_is_rejected() {
    let mut f = files();
    edit_item(&mut f, "scales.json", "major", |m| {
        m.insert(
            "common_chords",
            Json::Arr(vec![Json::Str("hypermajor_ninth".into())]),
        );
    });
    let p = problems(f);
    expect_code(&p, "KB_UNRESOLVED_REF");
    assert!(p.iter().any(|e| e.message.contains("hypermajor_ninth")));
}

#[test]
fn a_record_naming_an_unknown_scale_is_rejected() {
    let mut f = files();
    edit_item(&mut f, "modes.json", "mode_lydian", |m| {
        m.insert("scale_id", Json::Str("hyperlydian".into()));
    });
    expect_code(&problems(f), "KB_UNRESOLVED_REF");
}

#[test]
fn a_missing_knowledge_file_is_rejected() {
    let mut f = files();
    f.remove("scales.json");
    expect_code(&problems(f), "KB_MISSING_FILE");
}

// ---------------------------------------------------------------------------
// profiles
// ---------------------------------------------------------------------------

#[test]
fn a_profile_inheritance_cycle_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "profiles/common_practice.json",
        "common_practice",
        |m| {
            m.insert("parent", Json::Str("jazz_standard".into()));
        },
    );
    let p = problems(f);
    expect_code(&p, "KB_PROFILE_CYCLE");
    assert!(p
        .iter()
        .any(|e| e.message.contains("jazz_standard") && e.message.contains("->")));
}

#[test]
fn a_self_parenting_profile_is_rejected() {
    let mut f = files();
    edit_item(&mut f, "profiles/blues.json", "blues", |m| {
        m.insert("parent", Json::Str("blues".into()));
    });
    expect_code(&problems(f), "KB_PROFILE_CYCLE");
}

#[test]
fn a_missing_profile_parent_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "profiles/jazz_standard.json",
        "jazz_standard",
        |m| {
            m.insert("parent", Json::Str("ragtime".into()));
        },
    );
    let p = problems(f);
    expect_code(&p, "KB_MISSING_PARENT");
    assert!(p.iter().any(|e| e.message.contains("ragtime")));
}

#[test]
fn a_missing_required_profile_is_rejected() {
    let mut f = files();
    f.remove("profiles/blues.json");
    let p = problems(f);
    expect_code(&p, "KB_MISSING_FILE");
    assert!(p.iter().any(|e| e.message.contains("blues")));
}

#[test]
fn score_weights_missing_a_component_are_rejected() {
    let mut f = files();
    edit_item(&mut f, "profiles/blues.json", "blues", |m| {
        let mut w = m
            .get("score_weights")
            .and_then(Json::as_obj)
            .cloned()
            .unwrap();
        w.remove("voice_leading");
        m.insert("score_weights", Json::Obj(w));
    });
    let p = problems(f);
    expect_code(&p, "KB_INVALID_WEIGHTS");
    assert!(p.iter().any(|e| e.message.contains("voice_leading")));
}

#[test]
fn score_weights_naming_an_unknown_component_are_rejected() {
    let mut f = files();
    edit_item(&mut f, "profiles/blues.json", "blues", |m| {
        let mut w = m
            .get("score_weights")
            .and_then(Json::as_obj)
            .cloned()
            .unwrap();
        w.remove("voice_leading");
        w.insert("swagger", Json::Float(1.0));
        m.insert("score_weights", Json::Obj(w));
    });
    let p = problems(f);
    expect_code(&p, "KB_INVALID_WEIGHTS");
    assert!(p.iter().any(|e| e.message.contains("swagger")));
}

#[test]
fn a_negative_score_weight_is_rejected() {
    let mut f = files();
    edit_item(&mut f, "profiles/blues.json", "blues", |m| {
        let mut w = m
            .get("score_weights")
            .and_then(Json::as_obj)
            .cloned()
            .unwrap();
        w.insert("voice_leading", Json::Float(-1.0));
        m.insert("score_weights", Json::Obj(w));
    });
    expect_code(&problems(f), "KB_INVALID_WEIGHTS");
}

#[test]
fn rule_overrides_naming_an_unknown_rule_are_rejected() {
    let mut f = files();
    edit_item(&mut f, "profiles/blues.json", "blues", |m| {
        let mut o = m
            .get("rule_overrides")
            .and_then(Json::as_obj)
            .cloned()
            .unwrap();
        o.insert("harmony.play_it_cool", Json::Float(2.0));
        m.insert("rule_overrides", Json::Obj(o));
    });
    let p = problems(f);
    expect_code(&p, "KB_UNRESOLVED_REF");
    assert!(p.iter().any(|e| e.message.contains("harmony.play_it_cool")));
}

#[test]
fn a_profile_naming_an_unknown_chord_quality_is_rejected() {
    let mut f = files();
    edit_item(&mut f, "profiles/blues.json", "blues", |m| {
        let mut v = m
            .get("harmonic_vocabulary")
            .and_then(Json::as_obj)
            .cloned()
            .unwrap();
        v.insert("core", Json::Arr(vec![Json::Str("shred_chord".into())]));
        m.insert("harmonic_vocabulary", Json::Obj(v));
    });
    let p = problems(f);
    expect_code(&p, "KB_UNRESOLVED_REF");
    assert!(p.iter().any(|e| e.message.contains("shred_chord")));
}

// ---------------------------------------------------------------------------
// closed vocabularies
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_rule_kind_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.blues_dominant_seventh_is_stable_tonic",
        |m| {
            m.insert("kind", Json::Str("vibes_based_guidance".into()));
        },
    );
    expect_code(&problems(f), "KB_UNKNOWN_ENUM");
}

#[test]
fn an_unknown_rule_domain_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.blues_dominant_seventh_is_stable_tonic",
        |m| {
            m.insert("domain", Json::Str("percussion".into()));
        },
    );
    expect_code(&problems(f), "KB_UNKNOWN_ENUM");
}

#[test]
fn an_unknown_trigger_event_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.blues_dominant_seventh_is_stable_tonic",
        |m| {
            let mut t = m.get("trigger").and_then(Json::as_obj).cloned().unwrap();
            t.insert("event", Json::Str("vibe_check".into()));
            m.insert("trigger", Json::Obj(t));
        },
    );
    expect_code(&problems(f), "KB_UNKNOWN_ENUM");
}

#[test]
fn an_unknown_severity_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.blues_dominant_seventh_is_stable_tonic",
        |m| {
            let mut e = m.get("effect").and_then(Json::as_obj).cloned().unwrap();
            e.insert("severity", Json::Str("catastrophic".into()));
            m.insert("effect", Json::Obj(e));
        },
    );
    expect_code(&problems(f), "KB_UNKNOWN_ENUM");
}

#[test]
fn an_unimplemented_predicate_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.blues_dominant_seventh_is_stable_tonic",
        |m| {
            m.insert(
                "conditions",
                Json::Arr(vec![Json::Str("chord_sounds_nice".into())]),
            );
        },
    );
    let p = problems(f);
    expect_code(&p, "KB_UNKNOWN_PREDICATE");
    assert!(p.iter().any(|e| e.message.contains("chord_sounds_nice")));
}

#[test]
fn an_unimplemented_predicate_in_an_exception_is_also_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.blues_dominant_seventh_is_stable_tonic",
        |m| {
            m.insert(
                "exceptions",
                Json::Arr(vec![Json::Str("the_producer_said_so".into())]),
            );
        },
    );
    expect_code(&problems(f), "KB_UNKNOWN_PREDICATE");
}

// ---------------------------------------------------------------------------
// per-record invariants
// ---------------------------------------------------------------------------

#[test]
fn an_empty_test_id_list_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.blues_dominant_seventh_is_stable_tonic",
        |m| {
            m.insert("test_ids", Json::Arr(vec![]));
        },
    );
    expect_code(&problems(f), "KB_MISSING_TEST_IDS");
}

#[test]
fn an_invalid_chord_degree_in_a_trigger_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/extensions.json",
        "extensions.major_natural_11_close_register",
        |m| {
            let mut t = m.get("trigger").and_then(Json::as_obj).cloned().unwrap();
            t.insert(
                "contains_degrees",
                Json::Arr(vec![Json::Str("three".into())]),
            );
            m.insert("trigger", Json::Obj(t));
        },
    );
    expect_code(&problems(f), "KB_INVALID_DEGREE");
}

#[test]
fn an_invalid_chord_degree_in_a_quality_is_rejected() {
    let mut f = files();
    edit_item(&mut f, "chord_qualities.json", "major_triad", |m| {
        m.insert(
            "degrees",
            Json::Arr(vec![
                Json::Str("1".into()),
                Json::Str("third".into()),
                Json::Str("5".into()),
            ]),
        );
    });
    let p = problems(f);
    expect_code(&p, "KB_INVALID_DEGREE");
    assert!(p.iter().any(|e| e.message.contains("third")));
}

#[test]
fn an_invalid_scale_degree_spelling_is_rejected() {
    let mut f = files();
    edit_item(&mut f, "scales.json", "major", |m| {
        m.insert("tension_degrees", Json::Arr(vec![Json::Str("bbb4".into())]));
    });
    expect_code(&problems(f), "KB_INVALID_DEGREE");
}

#[test]
fn a_heuristic_carrying_a_citation_is_rejected() {
    let mut f = files();
    let heuristic = KnowledgeBase::embedded()
        .rules()
        .iter()
        .find(|r| r.kind == theory_kb::RuleKind::ImplementationHeuristic)
        .expect("the bundle has honest heuristics")
        .clone();
    let path = format!("rules/{}.json", heuristic.domain.id());
    edit_item(&mut f, &path, &heuristic.id, |m| {
        m.insert(
            "source_refs",
            Json::Arr(vec![Json::Obj(
                [
                    ("source_id".to_string(), Json::Str("mt21c".into())),
                    ("locator".to_string(), Json::Str("Voice leading".into())),
                ]
                .into_iter()
                .collect(),
            )]),
        );
    });
    let p = problems(f);
    expect_code(&p, "KB_INCONSISTENT");
    assert!(p
        .iter()
        .any(|e| e.message.contains("engineering judgement")));
}

#[test]
fn a_sourced_rule_without_a_citation_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.blues_dominant_seventh_is_stable_tonic",
        |m| {
            m.insert("source_refs", Json::Arr(vec![]));
        },
    );
    let p = problems(f);
    expect_code(&p, "KB_INCONSISTENT");
    assert!(p.iter().any(|e| e.message.contains("must cite")));
}

#[test]
fn a_rule_id_that_disagrees_with_its_domain_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.blues_dominant_seventh_is_stable_tonic",
        |m| {
            m.insert("id", Json::Str("melody.tonic_is_stable".into()));
        },
    );
    let p = problems(f);
    expect_code(&p, "KB_INCONSISTENT");
    assert!(p.iter().any(|e| e.message.contains("must start with")));
}

#[test]
fn a_mode_that_is_not_a_rotation_of_its_parent_is_rejected() {
    let mut f = files();
    edit_item(&mut f, "scales.json", "dorian", |m| {
        m.insert(
            "mode_of",
            Json::Arr(vec![Json::Str("major".into()), Json::Int(3)]),
        );
    });
    let p = problems(f);
    expect_code(&p, "KB_INCONSISTENT");
    assert!(p.iter().any(|e| e.message.contains("not that rotation")));
}

#[test]
fn a_duplicate_chord_symbol_token_is_rejected() {
    let mut f = files();
    edit_item(&mut f, "chord_symbols.json", "sym_maj13", |m| {
        m.insert("written", Json::Str("m7".into()));
    });
    let p = problems(f);
    expect_code(&p, "KB_DUPLICATE_ID");
    assert!(p.iter().any(|e| e.message.contains("token")));
}

#[test]
fn a_duplicate_chord_symbol_precedence_is_rejected() {
    let mut f = files();
    let first = KnowledgeBase::embedded().chord_symbols()[0].precedence;
    edit_item(&mut f, "chord_symbols.json", "sym_maj9", |m| {
        m.insert("precedence", Json::Int(first));
    });
    let p = problems(f);
    expect_code(&p, "KB_DUPLICATE_ID");
    assert!(p.iter().any(|e| e.message.contains("precedence")));
}

// ---------------------------------------------------------------------------
// schema
// ---------------------------------------------------------------------------

#[test]
fn a_schema_violation_is_rejected() {
    let mut f = files();
    edit_item(
        &mut f,
        "rules/harmony.json",
        "harmony.blues_dominant_seventh_is_stable_tonic",
        |m| {
            // `summary` has a minLength of 10 in theory-rule.schema.json.
            m.insert("summary", Json::Str("short".into()));
        },
    );
    expect_code(&problems(f), "KB_SCHEMA");
}

#[test]
fn a_source_record_schema_violation_is_rejected() {
    let mut f = files();
    edit_item(&mut f, "sources.json", "mt21c", |m| {
        // `url` must match `^https?://`.
        m.insert("url", Json::Str("ftp://example.invalid".into()));
    });
    expect_code(&problems(f), "KB_SCHEMA");
}

#[test]
fn a_profile_schema_violation_is_rejected() {
    let mut f = files();
    edit_item(&mut f, "profiles/blues.json", "blues", |m| {
        // `unit` fields are bounded to 0..=1.
        m.insert("modal_tolerance", Json::Float(4.0));
    });
    expect_code(&problems(f), "KB_SCHEMA");
}

// ---------------------------------------------------------------------------
// manifest
// ---------------------------------------------------------------------------

#[test]
fn a_manifest_hash_mismatch_is_rejected() {
    let mut f = files();
    edit_root(&mut f, "manifest.json", |m| {
        m.insert("content_sha256", Json::Str("0".repeat(64)));
    });
    let p = problems(f);
    expect_code(&p, "KB_HASH_MISMATCH");
    assert!(p
        .iter()
        .any(|e| e.message.contains("but the files hash to")));
}

#[test]
fn a_manifest_count_mismatch_is_rejected() {
    let mut f = files();
    edit_root(&mut f, "manifest.json", |m| {
        let mut c = m.get("counts").and_then(Json::as_obj).cloned().unwrap();
        c.insert("rules", Json::Int(999));
        m.insert("counts", Json::Obj(c));
    });
    let p = problems(f);
    expect_code(&p, "KB_COUNT_MISMATCH");
    assert!(p.iter().any(|e| e.message.contains("counts.rules")));
}

#[test]
fn a_manifest_domain_count_mismatch_is_rejected() {
    let mut f = files();
    edit_root(&mut f, "manifest.json", |m| {
        let mut c = m
            .get("rules_by_domain")
            .and_then(Json::as_obj)
            .cloned()
            .unwrap();
        c.insert("harmony", Json::Int(1));
        m.insert("rules_by_domain", Json::Obj(c));
    });
    expect_code(&problems(f), "KB_COUNT_MISMATCH");
}

#[test]
fn a_manifest_file_list_mismatch_is_rejected() {
    let mut f = files();
    edit_root(&mut f, "manifest.json", |m| {
        m.insert("files", Json::Arr(vec![Json::Str("scales.json".into())]));
    });
    expect_code(&problems(f), "KB_INCONSISTENT");
}

#[test]
fn dropping_records_below_the_minimum_is_rejected() {
    let mut f = files();
    let mut kept = 0;
    edit_items(&mut f, "scales.json", |m| {
        kept += 1;
        if kept <= 5 {
            Some(m)
        } else {
            None
        }
    });
    let p = problems(f);
    expect_code(&p, "KB_COUNT_MISMATCH");
    assert!(p.iter().any(|e| e.message.contains("at least 30 scales")));
}

#[test]
fn a_pending_hash_is_accepted_for_the_embedded_bundle_only() {
    let mut f = files();
    edit_root(&mut f, "manifest.json", |m| {
        m.insert("content_sha256", Json::Str("PENDING".into()));
    });
    let kb = KnowledgeBase::parse(f, "test-bundle").expect("still parses");

    let embedded_rules = validate_report(
        &kb,
        &ValidateOptions {
            allow_pending_hash: true,
            ..ValidateOptions::default()
        },
    );
    assert!(
        !embedded_rules
            .iter()
            .any(|e| e.code == "KB_UNSTAMPED_MANIFEST"),
        "the embedded bundle may be unstamped while xtask is what stamps it"
    );

    let external_rules = validate_report(&kb, &ValidateOptions::default());
    expect_code(&external_rules, "KB_UNSTAMPED_MANIFEST");
}

#[test]
fn malformed_json_is_rejected() {
    let mut f = files();
    f.insert("scales.json".to_string(), "{ not json".to_string());
    expect_code(&problems(f), "KB_PARSE");
}

#[test]
fn a_bundle_with_no_manifest_is_rejected() {
    let mut f = files();
    f.remove("manifest.json");
    expect_code(&problems(f), "KB_MISSING_FILE");
}

// ---------------------------------------------------------------------------
// load_dir
// ---------------------------------------------------------------------------

/// Writes a bundle to a scratch directory and returns its path.
fn write_bundle(name: &str, files: &BTreeMap<String, String>) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "theory-kb-{}-{}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    for (rel, text) in files {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("create dirs");
        }
        std::fs::write(&p, text).expect("write file");
    }
    dir
}

#[test]
fn load_dir_accepts_a_stamped_bundle_and_rejects_an_unstamped_one() {
    let good = write_bundle("good", &files());
    let kb = KnowledgeBase::load_dir(&good).expect("a stamped bundle loads");
    assert_eq!(kb.rules().len(), 147);
    assert_eq!(kb.content_hash(), KnowledgeBase::embedded().content_hash());
    let _ = std::fs::remove_dir_all(&good);

    let mut broken = files();
    edit_root(&mut broken, "manifest.json", |m| {
        m.insert("content_sha256", Json::Str("PENDING".into()));
    });
    let bad = write_bundle("pending", &broken);
    let err = KnowledgeBase::load_dir(&bad).expect_err("an unstamped bundle is rejected");
    assert_eq!(err.code, "KB_UNSTAMPED_MANIFEST");
    let _ = std::fs::remove_dir_all(&bad);
}

#[test]
fn load_dir_reports_a_missing_directory() {
    let err = KnowledgeBase::load_dir(std::path::Path::new(
        "/nonexistent/knowledge/directory/for/tests",
    ))
    .expect_err("a missing directory is an error");
    assert_eq!(err.code, "KB_IO");
}
