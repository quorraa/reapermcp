//! Every rejection the loader owes an external knowledge directory.
//!
//! The point of this module is that a knowledge bundle either loads *whole* or
//! not at all. An unresolvable source reference, a rule scoped to a profile
//! that does not exist, or a predicate the engine cannot execute are all
//! startup failures here rather than rules that quietly never fire.
//!
//! The three authored schemas are compiled into the crate from `schemas/` so
//! validation needs no files on disk.

use crate::error::KbError;
use crate::load::{content_hash, KnowledgeBase, MANIFEST_FILE};
use crate::model::{RuleDomain, RuleKind, PENDING_HASH};
use crate::profile::ancestry;
use crate::rules::is_known_predicate;
use music_domain::candidate::SCORE_COMPONENTS;
use qjson::schema::Schema;
use qjson::Json;
use std::collections::{BTreeMap, BTreeSet};

/// The ten fixed style-profile ids.
pub const PROFILE_IDS: &[&str] = &[
    "blues",
    "cinematic",
    "common_practice",
    "drum_and_bass",
    "electronic_loop",
    "jazz_standard",
    "modal_ambient",
    "neo_soul_rnb",
    "pop_rock",
    "strict_counterpoint",
];

/// Sentinel values that stand for "no chord quality" or "any chord quality"
/// in `functions.json`; they are deliberately not ids.
const QUALITY_SENTINELS: &[&str] = &["none", "any"];

/// The minimum populated-knowledge floor from the product brief §7.
const MINIMUMS: &[(&str, usize)] = &[
    ("scales", 30),
    ("chord_qualities", 40),
    ("rules", 100),
    ("profiles", 10),
    ("voicing_templates", 30),
    ("progression_schemas", 25),
    ("arrangement_patterns", 24),
    ("instrument_profiles", 12),
];

const SOURCE_RECORD_SCHEMA: &str = include_str!("../../../schemas/source-record.schema.json");
const THEORY_RULE_SCHEMA: &str = include_str!("../../../schemas/theory-rule.schema.json");
const STYLE_PROFILE_SCHEMA: &str = include_str!("../../../schemas/style-profile.schema.json");

/// What to check.
#[derive(Clone, Debug)]
pub struct ValidateOptions {
    /// Accept the literal `"PENDING"` as `content_sha256`.
    ///
    /// True only for the embedded bundle, which `xtask stamp-manifest` stamps.
    /// An external `--knowledge-dir` with `"PENDING"` is rejected: unstamped
    /// knowledge has no provenance.
    pub allow_pending_hash: bool,
    /// Run the JSON Schema pass. Off only in tests that construct a record by
    /// hand and want the semantic checks alone.
    pub check_schemas: bool,
    /// Enforce the brief's minimum record counts.
    pub check_minimums: bool,
}

impl Default for ValidateOptions {
    fn default() -> Self {
        ValidateOptions {
            allow_pending_hash: false,
            check_schemas: true,
            check_minimums: true,
        }
    }
}

/// Validates the bundle, returning every problem found.
pub fn validate(kb: &KnowledgeBase, opts: &ValidateOptions) -> Result<(), Vec<KbError>> {
    let errors = validate_report(kb, opts);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validates the bundle, collecting every problem rather than stopping at the
/// first, so a broken bundle can be fixed in one pass.
pub fn validate_report(kb: &KnowledgeBase, opts: &ValidateOptions) -> Vec<KbError> {
    let mut errors = Vec::new();
    if opts.check_schemas {
        check_schemas(kb, &mut errors);
    }
    check_duplicate_ids(kb, &mut errors);
    check_rules(kb, &mut errors);
    check_profiles(kb, &mut errors);
    check_cross_references(kb, &mut errors);
    check_degrees(kb, &mut errors);
    check_scale_modes(kb, &mut errors);
    check_chord_symbols(kb, &mut errors);
    check_manifest(kb, opts, &mut errors);
    if opts.check_minimums {
        check_minimums(kb, &mut errors);
    }
    errors
}

// ---------------------------------------------------------------------------
// schemas
// ---------------------------------------------------------------------------

/// Compiles one of the crate's built-in schemas.
fn compile(name: &str, text: &str) -> Result<Schema, KbError> {
    let doc = Json::parse(text).map_err(|e| KbError::schema(name, e.to_string()))?;
    Schema::compile(&doc).map_err(|e| KbError::schema(name, e.message))
}

/// The three schemas that govern the knowledge bundle, by name.
pub fn builtin_schemas() -> Result<Vec<(&'static str, Schema)>, KbError> {
    Ok(vec![
        (
            "source-record.schema.json",
            compile("source-record.schema.json", SOURCE_RECORD_SCHEMA)?,
        ),
        (
            "theory-rule.schema.json",
            compile("theory-rule.schema.json", THEORY_RULE_SCHEMA)?,
        ),
        (
            "style-profile.schema.json",
            compile("style-profile.schema.json", STYLE_PROFILE_SCHEMA)?,
        ),
    ])
}

/// Which built-in schema governs a knowledge file, if any.
pub fn schema_for(path: &str) -> Option<&'static str> {
    if path == "sources.json" {
        Some("source-record.schema.json")
    } else if path.starts_with("rules/") {
        Some("theory-rule.schema.json")
    } else if path.starts_with("profiles/") {
        Some("style-profile.schema.json")
    } else {
        None
    }
}

fn check_schemas(kb: &KnowledgeBase, errors: &mut Vec<KbError>) {
    let schemas = match builtin_schemas() {
        Ok(s) => s,
        Err(e) => {
            errors.push(e);
            return;
        }
    };
    for (path, doc) in kb.docs() {
        let Some(name) = schema_for(path) else {
            continue;
        };
        let Some((_, schema)) = schemas.iter().find(|(n, _)| *n == name) else {
            continue;
        };
        for v in schema.validate(doc) {
            errors.push(KbError::schema(
                format!("{path}{}", v.instance_path),
                format!("{}: {} (against {name})", v.keyword, v.message),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// ids
// ---------------------------------------------------------------------------

/// Reports every id that appears more than once in one collection.
fn check_unique<'a>(
    collection: &str,
    ids: impl Iterator<Item = &'a str>,
    errors: &mut Vec<KbError>,
) {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            errors.push(KbError::duplicate_id(
                collection,
                format!("id '{id}' appears more than once"),
            ));
        }
    }
}

fn check_duplicate_ids(kb: &KnowledgeBase, errors: &mut Vec<KbError>) {
    check_unique(
        "sources.json",
        kb.sources().iter().map(|s| s.id.as_str()),
        errors,
    );
    check_unique("rules/", kb.rules().iter().map(|r| r.id.as_str()), errors);
    check_unique(
        "profiles/",
        kb.profiles().iter().map(|p| p.id.as_str()),
        errors,
    );
    check_unique(
        "scales.json",
        kb.scales().iter().map(|s| s.id.as_str()),
        errors,
    );
    check_unique(
        "chord_qualities.json",
        kb.chord_qualities().iter().map(|q| q.id.as_str()),
        errors,
    );
    check_unique(
        "chord_symbols.json",
        kb.chord_symbols().iter().map(|s| s.id.as_str()),
        errors,
    );
    check_unique(
        "functions.json",
        kb.functions().iter().map(|f| f.id.as_str()),
        errors,
    );
    check_unique(
        "voicings.json",
        kb.voicing_templates().iter().map(|v| v.id.as_str()),
        errors,
    );
    check_unique(
        "progressions.json",
        kb.progressions().iter().map(|p| p.id.as_str()),
        errors,
    );
    check_unique(
        "cadences.json",
        kb.cadences().iter().map(|c| c.id()),
        errors,
    );
    check_unique(
        "arrangement_patterns.json",
        kb.arrangement_patterns().iter().map(|a| a.id.as_str()),
        errors,
    );
    check_unique(
        "instrument_profiles.json",
        kb.instrument_profiles().iter().map(|p| p.id.as_str()),
        errors,
    );
    check_unique(
        "modes.json",
        kb.modes().iter().map(|m| m.id.as_str()),
        errors,
    );
    check_unique(
        "intervals.json",
        kb.intervals().iter().map(|i| i.id.as_str()),
        errors,
    );
}

// ---------------------------------------------------------------------------
// rules
// ---------------------------------------------------------------------------

fn check_rules(kb: &KnowledgeBase, errors: &mut Vec<KbError>) {
    let source_ids: BTreeSet<&str> = kb.sources().iter().map(|s| s.id.as_str()).collect();
    let profile_ids: BTreeSet<&str> = kb.profiles().iter().map(|p| p.id.as_str()).collect();
    let components: BTreeSet<&str> = SCORE_COMPONENTS.iter().copied().collect();

    for rule in kb.rules() {
        let path = format!("rules/{}.json[{}]", rule.domain.id(), rule.id);

        // The id must be `<domain>.<name>`, and the domain must match.
        match rule.id.split_once('.') {
            Some((prefix, _)) if prefix == rule.domain.id() => {}
            _ => errors.push(KbError::inconsistent(
                &path,
                format!(
                    "rule id '{}' must start with its domain '{}.'",
                    rule.id,
                    rule.domain.id()
                ),
            )),
        }

        // Citations: non-empty if and only if the kind is not a heuristic.
        let is_heuristic = rule.kind == RuleKind::ImplementationHeuristic;
        if is_heuristic && !rule.source_refs.is_empty() {
            errors.push(KbError::inconsistent(
                &path,
                "an implementation_heuristic must carry no source_refs; it is engineering judgement, not theory",
            ));
        }
        if !is_heuristic && rule.source_refs.is_empty() {
            errors.push(KbError::inconsistent(
                &path,
                format!("a {} rule must cite at least one source", rule.kind.id()),
            ));
        }
        for r in &rule.source_refs {
            if !source_ids.contains(r.source_id.as_str()) {
                errors.push(KbError::unresolved_ref(
                    &path,
                    format!("source_refs names unknown source '{}'", r.source_id),
                ));
            }
        }

        for p in &rule.profiles {
            if !profile_ids.contains(p.as_str()) {
                errors.push(KbError::unresolved_ref(
                    &path,
                    format!("profiles names unknown style profile '{p}'"),
                ));
            }
        }

        if !components.contains(rule.effect.score_component.as_str()) {
            errors.push(KbError::unresolved_ref(
                &path,
                format!(
                    "effect.score_component '{}' is not one of the thirteen score components",
                    rule.effect.score_component
                ),
            ));
        }

        for p in rule.conditions.iter().chain(rule.exceptions.iter()) {
            if !is_known_predicate(p) {
                errors.push(KbError::unknown_predicate(
                    &path,
                    format!(
                        "predicate '{p}' is not implemented by this build; an unimplemented \
                         predicate is a startup failure, not a rule that never fires"
                    ),
                ));
            }
        }

        if rule.test_ids.is_empty() {
            errors.push(KbError::missing_test_ids(
                &path,
                "every rule must name at least one behavioural test; rules without tests drift",
            ));
        }

        if !(0.0..=1.0).contains(&rule.confidence) {
            errors.push(KbError::inconsistent(
                &path,
                format!("confidence {} is outside 0..=1", rule.confidence),
            ));
        }

        if rule.version == 0 {
            errors.push(KbError::inconsistent(&path, "version must start at 1"));
        }

        for d in rule
            .trigger
            .selectors
            .get("contains_degrees")
            .and_then(Json::as_arr)
            .unwrap_or(&[])
        {
            match d.as_str() {
                Some(s) if is_valid_degree(s) => {}
                Some(s) => errors.push(KbError::invalid_degree(
                    &path,
                    format!("trigger degree '{s}' does not match ^[b#]{{0,2}}\\d+$"),
                )),
                None => errors.push(KbError::shape(&path, "contains_degrees must hold strings")),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// profiles
// ---------------------------------------------------------------------------

fn check_profiles(kb: &KnowledgeBase, errors: &mut Vec<KbError>) {
    let rule_ids: BTreeSet<&str> = kb.rules().iter().map(|r| r.id.as_str()).collect();
    let quality_ids: BTreeSet<&str> = kb.chord_qualities().iter().map(|q| q.id.as_str()).collect();
    let components: BTreeSet<&str> = SCORE_COMPONENTS.iter().copied().collect();

    let present: BTreeSet<&str> = kb.profiles().iter().map(|p| p.id.as_str()).collect();
    for want in PROFILE_IDS {
        if !present.contains(want) {
            errors.push(KbError::missing_file(
                format!("profiles/{want}.json"),
                format!("required style profile '{want}' is missing"),
            ));
        }
    }

    for p in kb.profiles() {
        let path = format!("profiles/{}.json", p.id);

        if !PROFILE_IDS.contains(&p.id.as_str()) {
            errors.push(KbError::unknown_enum(
                &path,
                format!("'{}' is not one of the ten fixed profile ids", p.id),
            ));
        }

        // Parent chain: missing parents and cycles.
        if let Err(e) = ancestry(kb.profiles(), &p.id) {
            errors.push(e);
        }

        // Weights must cover exactly the thirteen components.
        let declared: BTreeSet<&str> = p.score_weights.keys().map(String::as_str).collect();
        for extra in declared.difference(&components) {
            errors.push(KbError::invalid_weights(
                &path,
                format!("score_weights names unknown component '{extra}'"),
            ));
        }
        for missing in components.difference(&declared) {
            errors.push(KbError::invalid_weights(
                &path,
                format!("score_weights is missing component '{missing}'"),
            ));
        }

        for (rule_id, _) in &p.rule_overrides {
            if !rule_ids.contains(rule_id.as_str()) {
                errors.push(KbError::unresolved_ref(
                    &path,
                    format!("rule_overrides names unknown rule '{rule_id}'"),
                ));
            }
        }

        if let Some(vocab) = p.raw.get("harmonic_vocabulary").and_then(Json::as_obj) {
            for (group, list) in vocab.iter() {
                for q in list.as_arr().unwrap_or(&[]).iter().filter_map(Json::as_str) {
                    if !quality_ids.contains(q) {
                        errors.push(KbError::unresolved_ref(
                            &path,
                            format!(
                                "harmonic_vocabulary.{group} names unknown chord quality '{q}'"
                            ),
                        ));
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// cross references
// ---------------------------------------------------------------------------

/// Reports a reference that does not resolve.
fn need<'a>(
    known: &BTreeSet<&str>,
    value: &'a str,
    path: &str,
    what: &str,
    errors: &mut Vec<KbError>,
) -> Option<&'a str> {
    if known.contains(value) {
        Some(value)
    } else {
        errors.push(KbError::unresolved_ref(
            path,
            format!("{what} '{value}' does not resolve"),
        ));
        None
    }
}

fn check_cross_references(kb: &KnowledgeBase, errors: &mut Vec<KbError>) {
    let sources: BTreeSet<&str> = kb.sources().iter().map(|s| s.id.as_str()).collect();
    let scales: BTreeSet<&str> = kb.scales().iter().map(|s| s.id.as_str()).collect();
    let qualities: BTreeSet<&str> = kb.chord_qualities().iter().map(|q| q.id.as_str()).collect();
    let profiles: BTreeSet<&str> = kb.profiles().iter().map(|p| p.id.as_str()).collect();

    let mut source_refs: Vec<(String, &Vec<String>)> = Vec::new();
    let mut style_refs: Vec<(String, &Vec<String>)> = Vec::new();

    for s in kb.scales() {
        let path = format!("scales.json[{}]", s.id);
        source_refs.push((path.clone(), &s.source_refs));
        if let Some(parent) = &s.parent {
            need(&scales, parent, &path, "parent scale", errors);
        }
        if let Some((parent, _)) = &s.mode_of {
            need(&scales, parent, &path, "mode_of parent scale", errors);
        }
        for c in &s.common_chords {
            need(&qualities, c, &path, "common_chords chord quality", errors);
        }
    }
    for q in kb.chord_qualities() {
        let path = format!("chord_qualities.json[{}]", q.id);
        source_refs.push((path.clone(), &q.source_refs));
        for s in &q.typical_scales {
            need(&scales, s, &path, "typical_scales scale", errors);
        }
    }
    for s in kb.chord_symbols() {
        let path = format!("chord_symbols.json[{}]", s.id);
        if let Some(q) = &s.quality_id {
            need(&qualities, q, &path, "quality_id chord quality", errors);
        }
    }
    for f in kb.functions() {
        let path = format!("functions.json[{}]", f.id);
        source_refs.push((path.clone(), &f.source_refs));
        for (field, value) in [
            ("triad_quality", &f.triad_quality),
            ("seventh_quality", &f.seventh_quality),
        ] {
            if !QUALITY_SENTINELS.contains(&value.as_str()) {
                need(
                    &qualities,
                    value,
                    &path,
                    &format!("{field} chord quality"),
                    errors,
                );
            }
        }
    }
    for m in kb.modes() {
        let path = format!("modes.json[{}]", m.id);
        source_refs.push((path.clone(), &m.source_refs));
        need(&scales, &m.scale_id, &path, "scale_id scale", errors);
        need(
            &qualities,
            &m.typical_tonic_chord,
            &path,
            "typical_tonic_chord chord quality",
            errors,
        );
        for c in &m.contrast_with {
            need(&scales, c, &path, "contrast_with scale", errors);
        }
    }
    for v in kb.voicing_templates() {
        let path = format!("voicings.json[{}]", v.id);
        source_refs.push((path.clone(), &v.source_refs));
        style_refs.push((path, &v.style_profiles));
    }
    for a in kb.arrangement_patterns() {
        let path = format!("arrangement_patterns.json[{}]", a.id);
        source_refs.push((path.clone(), &a.source_refs));
        style_refs.push((path, &a.style_profiles));
    }
    for p in kb.instrument_profiles() {
        source_refs.push((
            format!("instrument_profiles.json[{}]", p.id),
            &p.source_refs,
        ));
    }
    for i in kb.intervals() {
        source_refs.push((format!("intervals.json[{}]", i.id), &i.source_refs));
    }
    for (file, schema) in kb
        .progressions()
        .iter()
        .map(|p| ("progressions.json", p))
        .chain(kb.cadences().iter().map(|c| ("cadences.json", &c.schema)))
    {
        let path = format!("{file}[{}]", schema.id);
        source_refs.push((path.clone(), &schema.source_refs));
        style_refs.push((path.clone(), &schema.style_profiles));
        for s in &schema.applicable_scales {
            need(&scales, s, &path, "applicable_scales scale", errors);
        }
        for mc in &schema.melody_compatibility {
            if !QUALITY_SENTINELS.contains(&mc.quality.as_str()) {
                need(
                    &qualities,
                    &mc.quality,
                    &path,
                    "melody_compatibility quality",
                    errors,
                );
            }
        }
    }

    for (path, refs) in source_refs {
        for r in refs {
            need(&sources, r, &path, "source_refs source", errors);
        }
    }
    for (path, refs) in style_refs {
        for r in refs {
            need(&profiles, r, &path, "style_profiles profile", errors);
        }
    }
}

// ---------------------------------------------------------------------------
// degrees
// ---------------------------------------------------------------------------

/// The degree grammar `^[b#]{0,2}\d+$`, implemented directly so the validator
/// does not depend on a regex engine for a two-token language.
pub fn is_valid_degree(s: &str) -> bool {
    let mut chars = s.chars().peekable();
    let mut accidentals = 0;
    while matches!(chars.peek(), Some('b') | Some('#')) {
        chars.next();
        accidentals += 1;
        if accidentals > 2 {
            return false;
        }
    }
    let digits: String = chars.collect();
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

fn check_degrees(kb: &KnowledgeBase, errors: &mut Vec<KbError>) {
    let check = |path: &str, field: &str, values: &[String], errors: &mut Vec<KbError>| {
        for d in values {
            if !is_valid_degree(d) {
                errors.push(KbError::invalid_degree(
                    path,
                    format!("{field} degree '{d}' does not match ^[b#]{{0,2}}\\d+$"),
                ));
            }
        }
    };

    for s in kb.scales() {
        let path = format!("scales.json[{}]", s.id);
        check(&path, "degree_spelling", &s.degree_spelling, errors);
        check(
            &path,
            "characteristic_degrees",
            &s.characteristic_degrees,
            errors,
        );
        check(&path, "tension_degrees", &s.tension_degrees, errors);
    }
    for q in kb.chord_qualities() {
        let path = format!("chord_qualities.json[{}]", q.id);
        for d in q.all_degrees() {
            if !is_valid_degree(d) {
                errors.push(KbError::invalid_degree(
                    &path,
                    format!("degree '{d}' does not match ^[b#]{{0,2}}\\d+$"),
                ));
            }
        }
    }
    for s in kb.chord_symbols() {
        let path = format!("chord_symbols.json[{}]", s.id);
        check(&path, "degrees_added", &s.degrees_added, errors);
        check(&path, "degrees_altered", &s.degrees_altered, errors);
        check(&path, "degrees_omitted", &s.degrees_omitted, errors);
    }
    for v in kb.voicing_templates() {
        let path = format!("voicings.json[{}]", v.id);
        for d in v.all_degrees() {
            if !is_valid_degree(d) {
                errors.push(KbError::invalid_degree(
                    &path,
                    format!("degree '{d}' does not match ^[b#]{{0,2}}\\d+$"),
                ));
            }
        }
    }
    for (file, schema) in kb
        .progressions()
        .iter()
        .map(|p| ("progressions.json", p))
        .chain(kb.cadences().iter().map(|c| ("cadences.json", &c.schema)))
    {
        let path = format!("{file}[{}]", schema.id);
        for mc in &schema.melody_compatibility {
            check(&path, "chord_tones", &mc.chord_tones, errors);
            check(&path, "color_tones", &mc.color_tones, errors);
            check(&path, "avoid_degrees", &mc.avoid_degrees, errors);
        }
    }
    for f in kb.functions() {
        let path = format!("functions.json[{}]", f.id);
        for (field, value) in [
            ("scale_degree", &f.scale_degree),
            ("typical_bass_degree", &f.typical_bass_degree),
        ] {
            if let Some(d) = value {
                if !is_valid_degree(d) {
                    errors.push(KbError::invalid_degree(
                        &path,
                        format!("{field} '{d}' does not match ^[b#]{{0,2}}\\d+$"),
                    ));
                }
            }
        }
        check(&path, "tendency_tones", &f.tendency_tones, errors);
    }
}

// ---------------------------------------------------------------------------
// scale rotations and symbol tokens
// ---------------------------------------------------------------------------

/// Every modal entry must be an exact rotation of its declared parent; a
/// "mode" that is not a rotation is a different collection wearing a label.
fn check_scale_modes(kb: &KnowledgeBase, errors: &mut Vec<KbError>) {
    for s in kb.scales() {
        let Some((parent_id, index)) = &s.mode_of else {
            continue;
        };
        let path = format!("scales.json[{}]", s.id);
        let Some(parent) = kb.scale(parent_id) else {
            continue; // already reported as an unresolved reference
        };
        let n = parent.semitones.len();
        if n == 0 || *index >= n {
            errors.push(KbError::inconsistent(
                &path,
                format!("mode_of index {index} is outside '{parent_id}' ({n} degrees)"),
            ));
            continue;
        }
        let base = parent.semitones[*index];
        let rotated: Vec<i32> = (0..n)
            .map(|i| (parent.semitones[(index + i) % n] - base).rem_euclid(12))
            .collect();
        if rotated != s.semitones {
            errors.push(KbError::inconsistent(
                &path,
                format!(
                    "declared as mode {index} of '{parent_id}' but its semitones {:?} are not that rotation {:?}",
                    s.semitones, rotated
                ),
            ));
        }
    }
}

/// Parsing must be deterministic: every written token is unique across the
/// table and every precedence value is distinct, so a tie is a data defect
/// rather than an arbitrary winner.
fn check_chord_symbols(kb: &KnowledgeBase, errors: &mut Vec<KbError>) {
    let mut tokens: BTreeMap<&str, &str> = BTreeMap::new();
    let mut precedences: BTreeMap<i64, &str> = BTreeMap::new();
    for s in kb.chord_symbols() {
        for token in s.tokens() {
            if let Some(other) = tokens.insert(token, &s.id) {
                errors.push(KbError::duplicate_id(
                    format!("chord_symbols.json[{}]", s.id),
                    format!("token '{token}' is already claimed by '{other}'"),
                ));
            }
        }
        if let Some(other) = precedences.insert(s.precedence, &s.id) {
            errors.push(KbError::duplicate_id(
                format!("chord_symbols.json[{}]", s.id),
                format!(
                    "precedence {} is already used by '{other}'; a tie makes parsing arbitrary",
                    s.precedence
                ),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// manifest
// ---------------------------------------------------------------------------

fn check_manifest(kb: &KnowledgeBase, opts: &ValidateOptions, errors: &mut Vec<KbError>) {
    let m = kb.manifest();

    if m.content_sha256 == PENDING_HASH {
        if !opts.allow_pending_hash {
            errors.push(KbError::unstamped_manifest(
                MANIFEST_FILE,
                "content_sha256 is still \"PENDING\"; run `cargo run -p xtask -- stamp-manifest` \
                 before using this directory as an external knowledge source",
            ));
        }
    } else if m.content_sha256 != kb.content_hash() {
        errors.push(KbError::hash_mismatch(
            MANIFEST_FILE,
            format!(
                "content_sha256 says {} but the files hash to {}",
                m.content_sha256,
                kb.content_hash()
            ),
        ));
    }

    let actual = kb.counts();
    for (k, declared) in &m.counts {
        match actual.get(k) {
            Some(got) if got == declared => {}
            Some(got) => errors.push(KbError::count_mismatch(
                MANIFEST_FILE,
                format!("counts.{k} says {declared} but the bundle holds {got}"),
            )),
            None => errors.push(KbError::count_mismatch(
                MANIFEST_FILE,
                format!("counts.{k} is not a collection this build knows"),
            )),
        }
    }
    let by_domain = kb.rule_counts_by_domain();
    for (k, declared) in &m.rules_by_domain {
        match by_domain.get(k) {
            Some(got) if got == declared => {}
            Some(got) => errors.push(KbError::count_mismatch(
                MANIFEST_FILE,
                format!("rules_by_domain.{k} says {declared} but the bundle holds {got}"),
            )),
            None => errors.push(KbError::count_mismatch(
                MANIFEST_FILE,
                format!("rules_by_domain.{k} is not a rule domain"),
            )),
        }
    }
    if RuleDomain::all().len() != by_domain.len() {
        errors.push(KbError::inconsistent(
            "rules/",
            format!(
                "expected rules in all {} domains, found {}",
                RuleDomain::all().len(),
                by_domain.len()
            ),
        ));
    }

    let hashed: Vec<&str> = kb
        .docs()
        .keys()
        .filter(|p| p.as_str() != MANIFEST_FILE)
        .map(String::as_str)
        .collect();
    let declared: Vec<&str> = m.files.iter().map(String::as_str).collect();
    if hashed != declared {
        errors.push(KbError::inconsistent(
            MANIFEST_FILE,
            format!(
                "files lists {} entries but the bundle hashes {}: {:?} vs {:?}",
                declared.len(),
                hashed.len(),
                declared,
                hashed
            ),
        ));
    }
}

fn check_minimums(kb: &KnowledgeBase, errors: &mut Vec<KbError>) {
    let counts = kb.counts();
    for (k, floor) in MINIMUMS {
        let got = counts.get(*k).copied().unwrap_or(0);
        if got < *floor {
            errors.push(KbError::count_mismatch(
                MANIFEST_FILE,
                format!("the bundle must hold at least {floor} {k}, found {got}"),
            ));
        }
    }
}

/// Recomputes the manifest's `content_sha256` for the given bundle.
pub fn recomputed_hash(kb: &KnowledgeBase) -> String {
    content_hash(kb.docs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::KnowledgeBase;

    #[test]
    fn degree_grammar() {
        // The grammar is literally `^[b#]{0,2}\d+$`, so a mixed pair such as
        // "b#3" is accepted by it even though no one would write that.
        for ok in ["1", "3", "13", "b3", "#11", "bb7", "##4", "b#3"] {
            assert!(is_valid_degree(ok), "{ok} should be valid");
        }
        for bad in ["", "b", "#", "bbb7", "x", "3b", "-3", "1.5", "b3b"] {
            assert!(!is_valid_degree(bad), "{bad} should be invalid");
        }
    }

    #[test]
    fn the_three_schemas_compile() {
        let s = builtin_schemas().expect("schemas must compile");
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn schema_routing() {
        assert_eq!(
            schema_for("sources.json"),
            Some("source-record.schema.json")
        );
        assert_eq!(
            schema_for("rules/harmony.json"),
            Some("theory-rule.schema.json")
        );
        assert_eq!(
            schema_for("profiles/blues.json"),
            Some("style-profile.schema.json")
        );
        assert_eq!(schema_for("scales.json"), None);
    }

    #[test]
    fn the_real_bundle_validates_with_a_pending_hash_allowance() {
        let kb = KnowledgeBase::embedded();
        let errors = validate_report(
            kb,
            &ValidateOptions {
                allow_pending_hash: true,
                ..ValidateOptions::default()
            },
        );
        assert!(errors.is_empty(), "{:#?}", errors);
    }

    #[test]
    fn recomputed_hash_matches_the_loader() {
        let kb = KnowledgeBase::embedded();
        assert_eq!(recomputed_hash(kb), kb.content_hash());
    }

    #[test]
    fn profile_ids_are_the_ten_fixed_ones() {
        assert_eq!(PROFILE_IDS.len(), 10);
        let kb = KnowledgeBase::embedded();
        let mut got: Vec<&str> = kb.profiles().iter().map(|p| p.id.as_str()).collect();
        got.sort();
        assert_eq!(got, PROFILE_IDS);
    }
}
