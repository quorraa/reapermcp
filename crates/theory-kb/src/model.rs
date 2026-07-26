//! One Rust type per knowledge record, plus the closed enumerations the rule
//! files draw on.
//!
//! Every record type exposes `from_json` and `to_json`. Records whose JSON
//! shape is deliberately open-ended (chord qualities, voicing templates,
//! arrangement patterns, …) keep a `raw` copy of the original object so that
//! `to_json` is lossless and a future knowledge field is never silently
//! dropped by an older binary.
//!
//! Field-name choices follow the JSON exactly; there is no renaming layer.

use crate::error::KbError;
use music_domain::candidate::Severity;
use music_domain::scale::ScaleDef;
use music_domain::time::BeatTime;
use qjson::{Json, JsonMap};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// small JSON helpers
// ---------------------------------------------------------------------------

/// Reads a required string field.
pub(crate) fn req_str(v: &Json, path: &str, key: &str) -> Result<String, KbError> {
    v.str_field(key)
        .map(str::to_string)
        .map_err(|e| KbError::shape(format!("{path}.{key}"), e.message))
}

/// Reads a string field that may be JSON `null`, mapping `null` to `None`.
pub(crate) fn nullable_str(v: &Json, path: &str, key: &str) -> Result<Option<String>, KbError> {
    match v.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(s)) => Ok(Some(s.clone())),
        Some(other) => Err(KbError::shape(
            format!("{path}.{key}"),
            format!("expected string or null, found {}", other.type_name()),
        )),
    }
}

/// Reads a required array-of-strings field.
pub(crate) fn str_vec(v: &Json, path: &str, key: &str) -> Result<Vec<String>, KbError> {
    let arr = v
        .arr_field(key)
        .map_err(|e| KbError::shape(format!("{path}.{key}"), e.message))?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        match item.as_str() {
            Some(s) => out.push(s.to_string()),
            None => {
                return Err(KbError::shape(
                    format!("{path}.{key}[{i}]"),
                    format!("expected string, found {}", item.type_name()),
                ))
            }
        }
    }
    Ok(out)
}

/// Reads an array-of-strings field that may be absent, yielding an empty vec.
pub(crate) fn opt_str_vec(v: &Json, path: &str, key: &str) -> Result<Vec<String>, KbError> {
    match v.get(key) {
        None | Some(Json::Null) => Ok(Vec::new()),
        Some(_) => str_vec(v, path, key),
    }
}

/// Reads a required number field.
pub(crate) fn req_f64(v: &Json, path: &str, key: &str) -> Result<f64, KbError> {
    v.f64_field(key)
        .map_err(|e| KbError::shape(format!("{path}.{key}"), e.message))
}

/// Reads a required integer field.
pub(crate) fn req_i64(v: &Json, path: &str, key: &str) -> Result<i64, KbError> {
    v.i64_field(key)
        .map_err(|e| KbError::shape(format!("{path}.{key}"), e.message))
}

/// Reads a required boolean field.
pub(crate) fn req_bool(v: &Json, path: &str, key: &str) -> Result<bool, KbError> {
    v.bool_field(key)
        .map_err(|e| KbError::shape(format!("{path}.{key}"), e.message))
}

/// Reads an optional string field, defaulting to the empty string.
pub(crate) fn opt_str(v: &Json, key: &str) -> String {
    v.get(key)
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Wraps a string slice as a JSON array.
pub(crate) fn strs(items: &[String]) -> Json {
    Json::Arr(items.iter().map(|s| Json::Str(s.clone())).collect())
}

/// Appends every key of `raw` that the modelled serialiser did not already
/// emit, so a round trip through an older binary never loses data.
pub(crate) fn merge_raw_extras(out: &mut JsonMap, raw: &Json) {
    if let Some(obj) = raw.as_obj() {
        for (k, v) in obj.iter() {
            if !out.contains_key(k) {
                out.insert(k, v.clone());
            }
        }
    }
}

/// Parses a rational-string field such as `"1/2"` into exact musical time.
pub(crate) fn beat_field(v: &Json, path: &str, key: &str) -> Result<BeatTime, KbError> {
    let s = req_str(v, path, key)?;
    BeatTime::parse(&s).map_err(|e| KbError::shape(format!("{path}.{key}"), e.message))
}

// ---------------------------------------------------------------------------
// closed enumerations
// ---------------------------------------------------------------------------

/// Builds a closed enum with `id`, `parse` and `all`, so the data vocabulary
/// and the Rust vocabulary can never drift apart silently.
macro_rules! closed_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $( $(#[$vmeta:meta])* $variant:ident => $id:literal ),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name { $( $(#[$vmeta])* $variant ),+ }

        impl $name {
            /// The identifier used in `knowledge/`.
            pub fn id(self) -> &'static str {
                match self { $( $name::$variant => $id ),+ }
            }
            /// Parses the identifier used in `knowledge/`.
            pub fn parse(s: &str) -> Option<$name> {
                match s { $( $id => Some($name::$variant), )+ _ => None }
            }
            /// Every variant, in declaration order.
            pub fn all() -> &'static [$name] {
                &[ $( $name::$variant ),+ ]
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.id())
            }
        }
    };
}

closed_enum! {
    /// The seven rule domains, one per file under `knowledge/rules/`.
    RuleDomain {
        /// Melodic analysis and non-chord-tone classification.
        Melody => "melody",
        /// Chord choice, function and progression.
        Harmony => "harmony",
        /// Extensions, alterations and omissions.
        Extensions => "extensions",
        /// Voice leading between realised voicings.
        VoiceLeading => "voice_leading",
        /// Species-style independence and dissonance treatment.
        Counterpoint => "counterpoint",
        /// Part writing, register, density and masking.
        Arrangement => "arrangement",
        /// Loop-boundary correctness.
        Looping => "looping",
    }
}

closed_enum! {
    /// How binding a rule is. The kind decides *when* a rule is evaluated, not
    /// merely how much it scores.
    RuleKind {
        /// Product and data guarantees. Enforced before scoring.
        HardIntegrity => "hard_integrity",
        /// Arithmetic and representational facts. Enforced before scoring.
        MathematicalInvariant => "mathematical_invariant",
        /// Principles a competent musician in the declared profiles would not argue with.
        StrongTheoryPrinciple => "strong_theory_principle",
        /// Standard practice with known exceptions.
        TheoryDefault => "theory_default",
        /// Genuinely contradicted by other profiles.
        StyleSensitivePreference => "style_sensitive_preference",
        /// Register, density, doubling and masking guidance.
        ArrangementHeuristic => "arrangement_heuristic",
        /// Loop-boundary correctness. High weight, evaluated early.
        LoopIntegrity => "loop_integrity",
        /// Honest engineering judgement, carrying no citation by contract.
        ImplementationHeuristic => "implementation_heuristic",
    }
}

impl RuleKind {
    /// True for the kinds evaluated in the first tier, before any soft scoring.
    ///
    /// `loop_integrity` joins the two integrity kinds here because loop
    /// correctness has to be known before a loop candidate is ranked; it is
    /// nonetheless *scored* rather than rejected — see [`RuleKind::rejects`].
    pub fn is_hard(self) -> bool {
        matches!(
            self,
            RuleKind::HardIntegrity | RuleKind::MathematicalInvariant | RuleKind::LoopIntegrity
        )
    }

    /// True for the kinds whose failure rejects a candidate outright rather
    /// than pushing it down the ranking.
    ///
    /// Only `hard_integrity` and `mathematical_invariant` qualify. A
    /// `loop_integrity` rule with a positive delta (`+2.5` for a smooth wrap)
    /// would otherwise be nonsensically reported as a violation.
    pub fn rejects(self) -> bool {
        matches!(
            self,
            RuleKind::HardIntegrity | RuleKind::MathematicalInvariant
        )
    }
}

closed_enum! {
    /// The fourteen points in the pipeline at which a rule can be considered.
    RuleEvent {
        /// A chord symbol has been parsed into a `ChordSpec`.
        SymbolParsed => "symbol_parsed",
        /// A single generated note is about to be written into an edit plan.
        NoteEmitted => "note_emitted",
        /// One melody note is being analysed.
        MelodyNote => "melody_note",
        /// A complete melodic phrase has been segmented.
        MelodyPhrase => "melody_phrase",
        /// A non-chord-tone classification is being scored.
        NctHypothesis => "nct_hypothesis",
        /// The harmonic-rhythm grid is being chosen.
        HarmonicGrid => "harmonic_grid",
        /// A single chord candidate is being scored in one slot.
        ChordSelected => "chord_selected",
        /// Two adjacent chords are being scored together.
        ChordPair => "chord_pair",
        /// A complete candidate progression is being scored.
        ProgressionPath => "progression_path",
        /// A realised voicing for one chord is being scored.
        VoicingBuilt => "voicing_built",
        /// Two voices moving between two chords are being scored.
        VoicePairMotion => "voice_pair_motion",
        /// A complete generated part is being scored.
        PartGenerated => "part_generated",
        /// The whole multi-part plan is being scored.
        ArrangementPlan => "arrangement_plan",
        /// The wrap point of a loop is being audited.
        LoopBoundary => "loop_boundary",
    }
}

// ---------------------------------------------------------------------------
// sources
// ---------------------------------------------------------------------------

/// One chapter or section of a source, with the topics it backs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelevantSection {
    /// A chapter or section title. Page numbers are forbidden by policy.
    pub locator: String,
    /// Topics this section is cited for.
    pub topics: Vec<String>,
}

impl RelevantSection {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<RelevantSection, KbError> {
        Ok(RelevantSection {
            locator: req_str(v, path, "locator")?,
            topics: str_vec(v, path, "topics")?,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("locator", Json::Str(self.locator.clone()));
        m.insert("topics", strs(&self.topics));
        Json::Obj(m)
    }
}

/// A bibliographic entry from `knowledge/sources.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRecord {
    /// Stable identifier cited by rules and other records.
    pub id: String,
    /// Work title.
    pub title: String,
    /// Authors; empty is an honest statement of uncertainty.
    pub authors: Vec<String>,
    /// Publisher or hosting project.
    pub publisher: String,
    /// Canonical URL.
    pub url: String,
    /// Licence identifier, or `"unverified-reference-only"`.
    pub license: String,
    /// ISO date the source was consulted.
    pub accessed_at: String,
    /// One of `official_docs`, `open_textbook`, `academic_paper`, `course_material`.
    pub source_type: String,
    /// The sections actually cited.
    pub relevant_sections: Vec<RelevantSection>,
    /// How the bundle uses the source.
    pub usage: String,
    /// Licensing, interpretation and verification notes.
    pub notes: String,
}

impl SourceRecord {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<SourceRecord, KbError> {
        let id = req_str(v, path, "id")?;
        let p = format!("{path}[{id}]");
        let mut relevant_sections = Vec::new();
        for (i, s) in v
            .arr_field("relevant_sections")
            .map_err(|e| KbError::shape(format!("{p}.relevant_sections"), e.message))?
            .iter()
            .enumerate()
        {
            relevant_sections.push(RelevantSection::from_json(
                s,
                &format!("{p}.relevant_sections[{i}]"),
            )?);
        }
        Ok(SourceRecord {
            title: req_str(v, &p, "title")?,
            authors: str_vec(v, &p, "authors")?,
            publisher: req_str(v, &p, "publisher")?,
            url: req_str(v, &p, "url")?,
            license: req_str(v, &p, "license")?,
            accessed_at: req_str(v, &p, "accessed_at")?,
            source_type: req_str(v, &p, "source_type")?,
            relevant_sections,
            usage: req_str(v, &p, "usage")?,
            notes: req_str(v, &p, "notes")?,
            id,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("title", Json::Str(self.title.clone()));
        m.insert("authors", strs(&self.authors));
        m.insert("publisher", Json::Str(self.publisher.clone()));
        m.insert("url", Json::Str(self.url.clone()));
        m.insert("license", Json::Str(self.license.clone()));
        m.insert("accessed_at", Json::Str(self.accessed_at.clone()));
        m.insert("source_type", Json::Str(self.source_type.clone()));
        m.insert(
            "relevant_sections",
            Json::Arr(self.relevant_sections.iter().map(|s| s.to_json()).collect()),
        );
        m.insert("usage", Json::Str(self.usage.clone()));
        m.insert("notes", Json::Str(self.notes.clone()));
        Json::Obj(m)
    }
}

// ---------------------------------------------------------------------------
// rules
// ---------------------------------------------------------------------------

/// A citation from a rule to one section of one source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRef {
    /// The `id` of a record in `knowledge/sources.json`.
    pub source_id: String,
    /// The chapter or section title within that source.
    pub locator: String,
}

impl SourceRef {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<SourceRef, KbError> {
        Ok(SourceRef {
            source_id: req_str(v, path, "source_id")?,
            locator: req_str(v, path, "locator")?,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("source_id", Json::Str(self.source_id.clone()));
        m.insert("locator", Json::Str(self.locator.clone()));
        Json::Obj(m)
    }
}

/// What a rule does to the score vector when it applies.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleEffect {
    /// Added to the named component, after the profile multiplier.
    pub score_delta: f64,
    /// How loudly the rule reports itself in a trace.
    pub severity: Severity,
    /// Exactly one of the thirteen `SCORE_COMPONENTS`.
    pub score_component: String,
}

impl RuleEffect {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<RuleEffect, KbError> {
        let sev = req_str(v, path, "severity")?;
        Ok(RuleEffect {
            score_delta: req_f64(v, path, "score_delta")?,
            severity: Severity::parse(&sev).ok_or_else(|| {
                KbError::unknown_enum(
                    format!("{path}.severity"),
                    format!("'{sev}' is not one of info, minor, moderate, major"),
                )
            })?,
            score_component: req_str(v, path, "score_component")?,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("score_delta", Json::Float(self.score_delta));
        m.insert("severity", Json::Str(self.severity.id().to_string()));
        m.insert("score_component", Json::Str(self.score_component.clone()));
        Json::Obj(m)
    }
}

/// The pipeline point plus any narrowing selectors.
///
/// Selectors are kept as a raw [`JsonMap`] on purpose: the permitted keys are
/// fixed by `schemas/theory-rule.schema.json`, but their *values* are an open
/// vocabulary shared with the arrangement and voicing catalogues, so modelling
/// each one as a Rust enum would make the loader reject data the schema
/// accepts.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleTrigger {
    /// The engine event this rule listens for.
    pub event: RuleEvent,
    /// Every trigger key except `event`, in file order.
    pub selectors: JsonMap,
}

impl RuleTrigger {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<RuleTrigger, KbError> {
        let obj = v
            .as_obj()
            .ok_or_else(|| KbError::shape(path, "trigger must be an object"))?;
        let raw_event = req_str(v, path, "event")?;
        let event = RuleEvent::parse(&raw_event).ok_or_else(|| {
            KbError::unknown_enum(
                format!("{path}.event"),
                format!("'{raw_event}' is not one of the fourteen trigger events"),
            )
        })?;
        let mut selectors = JsonMap::new();
        for (k, val) in obj.iter() {
            if k != "event" {
                selectors.insert(k, val.clone());
            }
        }
        Ok(RuleTrigger { event, selectors })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("event", Json::Str(self.event.id().to_string()));
        for (k, v) in self.selectors.iter() {
            m.insert(k, v.clone());
        }
        Json::Obj(m)
    }
}

/// One executable theory rule.
#[derive(Clone, Debug, PartialEq)]
pub struct TheoryRule {
    /// `<domain>.<snake_case_name>`, unique across every rule file.
    pub id: String,
    /// The domain, which must match both the id prefix and the file name.
    pub domain: RuleDomain,
    /// How binding the rule is.
    pub kind: RuleKind,
    /// One sentence of original wording.
    pub summary: String,
    /// When the rule is considered.
    pub trigger: RuleTrigger,
    /// Predicates that must all hold.
    pub conditions: Vec<String>,
    /// What applying the rule does.
    pub effect: RuleEffect,
    /// Style profiles in which the rule is active at all.
    pub profiles: Vec<String>,
    /// Predicates any one of which bypasses the rule.
    pub exceptions: Vec<String>,
    /// Citations; empty if and only if the kind is `implementation_heuristic`.
    pub source_refs: Vec<SourceRef>,
    /// How confident the authors are, 0..=1.
    pub confidence: f64,
    /// Why the rule exists and where it stops applying.
    pub rationale: String,
    /// Names of the automated tests that pin this rule's behaviour.
    pub test_ids: Vec<String>,
    /// Incremented whenever the rule's meaning changes.
    pub version: u32,
}

impl TheoryRule {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<TheoryRule, KbError> {
        let id = req_str(v, path, "id")?;
        let p = format!("{path}[{id}]");
        let raw_domain = req_str(v, &p, "domain")?;
        let domain = RuleDomain::parse(&raw_domain).ok_or_else(|| {
            KbError::unknown_enum(
                format!("{p}.domain"),
                format!("'{raw_domain}' is not one of the seven rule domains"),
            )
        })?;
        let raw_kind = req_str(v, &p, "kind")?;
        let kind = RuleKind::parse(&raw_kind).ok_or_else(|| {
            KbError::unknown_enum(
                format!("{p}.kind"),
                format!("'{raw_kind}' is not one of the eight rule kinds"),
            )
        })?;
        let mut source_refs = Vec::new();
        for (i, s) in v
            .arr_field("source_refs")
            .map_err(|e| KbError::shape(format!("{p}.source_refs"), e.message))?
            .iter()
            .enumerate()
        {
            source_refs.push(SourceRef::from_json(s, &format!("{p}.source_refs[{i}]"))?);
        }
        let version = req_i64(v, &p, "version")?;
        Ok(TheoryRule {
            domain,
            kind,
            summary: req_str(v, &p, "summary")?,
            trigger: RuleTrigger::from_json(
                v.field("trigger")
                    .map_err(|e| KbError::shape(format!("{p}.trigger"), e.message))?,
                &format!("{p}.trigger"),
            )?,
            conditions: str_vec(v, &p, "conditions")?,
            effect: RuleEffect::from_json(
                v.field("effect")
                    .map_err(|e| KbError::shape(format!("{p}.effect"), e.message))?,
                &format!("{p}.effect"),
            )?,
            profiles: str_vec(v, &p, "profiles")?,
            exceptions: str_vec(v, &p, "exceptions")?,
            source_refs,
            confidence: req_f64(v, &p, "confidence")?,
            rationale: req_str(v, &p, "rationale")?,
            test_ids: str_vec(v, &p, "test_ids")?,
            version: u32::try_from(version).map_err(|_| {
                KbError::shape(format!("{p}.version"), format!("{version} is out of range"))
            })?,
            id,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("domain", Json::Str(self.domain.id().to_string()));
        m.insert("kind", Json::Str(self.kind.id().to_string()));
        m.insert("summary", Json::Str(self.summary.clone()));
        m.insert("trigger", self.trigger.to_json());
        m.insert("conditions", strs(&self.conditions));
        m.insert("effect", self.effect.to_json());
        m.insert("profiles", strs(&self.profiles));
        m.insert("exceptions", strs(&self.exceptions));
        m.insert(
            "source_refs",
            Json::Arr(self.source_refs.iter().map(|s| s.to_json()).collect()),
        );
        m.insert("confidence", Json::Float(self.confidence));
        m.insert("rationale", Json::Str(self.rationale.clone()));
        m.insert("test_ids", strs(&self.test_ids));
        m.insert("version", Json::Int(i64::from(self.version)));
        Json::Obj(m)
    }

    /// The source ids this rule cites, in file order.
    pub fn source_ids(&self) -> Vec<String> {
        self.source_refs
            .iter()
            .map(|s| s.source_id.clone())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// profiles
// ---------------------------------------------------------------------------

/// A profile's opinion about one rule.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RuleOverride {
    /// Multiply the rule's `score_delta` by this non-negative factor. A
    /// multiplier of `0.0` still reports the rule as `applied`.
    Multiplier(f64),
    /// Switch the rule off entirely; it reports as `not_applicable`.
    Disabled,
}

impl RuleOverride {
    /// Reads the JSON form: a number, or the literal string `"disabled"`.
    pub fn from_json(v: &Json, path: &str) -> Result<RuleOverride, KbError> {
        match v {
            Json::Str(s) if s == "disabled" => Ok(RuleOverride::Disabled),
            Json::Int(_) | Json::Float(_) => {
                let n = v.as_f64().unwrap_or(0.0);
                if n < 0.0 {
                    Err(KbError::shape(
                        path,
                        "a rule multiplier must be non-negative",
                    ))
                } else {
                    Ok(RuleOverride::Multiplier(n))
                }
            }
            other => Err(KbError::shape(
                path,
                format!(
                    "expected a non-negative number or \"disabled\", found {}",
                    other.type_name()
                ),
            )),
        }
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        match self {
            RuleOverride::Multiplier(m) => Json::Float(*m),
            RuleOverride::Disabled => Json::Str("disabled".to_string()),
        }
    }

    /// The factor to apply to a score delta. `Disabled` yields `0.0`, but a
    /// disabled rule never reaches scoring in the first place.
    pub fn multiplier(&self) -> f64 {
        match self {
            RuleOverride::Multiplier(m) => *m,
            RuleOverride::Disabled => 0.0,
        }
    }
}

/// One style profile as authored in `knowledge/profiles/<id>.json`.
///
/// Only the fields the engine reasons about structurally are lifted out; every
/// other declared field stays reachable through [`StyleProfile::raw`] and
/// through the flattened `fields` map on a resolved profile.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleProfile {
    /// One of the ten fixed profile ids.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The profile this one inherits from, or `None` for a root.
    pub parent: Option<String>,
    /// One paragraph describing the grammar this profile encodes.
    pub summary: String,
    /// Non-negative weight per score component, before normalisation.
    pub score_weights: BTreeMap<String, f64>,
    /// Per-rule multipliers and disables declared by this profile alone.
    pub rule_overrides: BTreeMap<String, RuleOverride>,
    /// Editorial notes.
    pub notes: String,
    /// The complete original object.
    pub raw: Json,
}

impl StyleProfile {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<StyleProfile, KbError> {
        let id = req_str(v, path, "id")?;
        let p = format!("{path}[{id}]");
        let mut score_weights = BTreeMap::new();
        let weights = v
            .obj_field("score_weights")
            .map_err(|e| KbError::shape(format!("{p}.score_weights"), e.message))?;
        for (k, val) in weights.iter() {
            let n = val.as_f64().ok_or_else(|| {
                KbError::shape(
                    format!("{p}.score_weights.{k}"),
                    format!("expected a number, found {}", val.type_name()),
                )
            })?;
            if n < 0.0 {
                return Err(KbError::invalid_weights(
                    format!("{p}.score_weights.{k}"),
                    "score weights must be non-negative",
                ));
            }
            score_weights.insert(k.to_string(), n);
        }
        let mut rule_overrides = BTreeMap::new();
        let overrides = v
            .obj_field("rule_overrides")
            .map_err(|e| KbError::shape(format!("{p}.rule_overrides"), e.message))?;
        for (k, val) in overrides.iter() {
            rule_overrides.insert(
                k.to_string(),
                RuleOverride::from_json(val, &format!("{p}.rule_overrides.{k}"))?,
            );
        }
        Ok(StyleProfile {
            name: req_str(v, &p, "name")?,
            parent: nullable_str(v, &p, "parent")?,
            summary: req_str(v, &p, "summary")?,
            score_weights,
            rule_overrides,
            notes: opt_str(v, "notes"),
            raw: v.clone(),
            id,
        })
    }

    /// Writes the JSON form, preserving every authored field.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("name", Json::Str(self.name.clone()));
        m.insert(
            "parent",
            match &self.parent {
                Some(p) => Json::Str(p.clone()),
                None => Json::Null,
            },
        );
        m.insert("summary", Json::Str(self.summary.clone()));
        merge_raw_extras(&mut m, &self.raw);
        let mut w = JsonMap::new();
        for (k, v) in &self.score_weights {
            w.insert(k.clone(), Json::Float(*v));
        }
        m.insert("score_weights", Json::Obj(w));
        let mut o = JsonMap::new();
        for (k, v) in &self.rule_overrides {
            o.insert(k.clone(), v.to_json());
        }
        m.insert("rule_overrides", Json::Obj(o));
        m.insert("notes", Json::Str(self.notes.clone()));
        Json::Obj(m)
    }
}

// ---------------------------------------------------------------------------
// catalogue records
// ---------------------------------------------------------------------------

/// A scale or mode collection. Reuses the shared domain type verbatim.
pub type ScaleRecord = ScaleDef;

/// A low/high MIDI window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiRange {
    /// Lowest usable MIDI note.
    pub low_midi: i32,
    /// Highest usable MIDI note.
    pub high_midi: i32,
}

impl MidiRange {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<MidiRange, KbError> {
        Ok(MidiRange {
            low_midi: req_i64(v, path, "low_midi")? as i32,
            high_midi: req_i64(v, path, "high_midi")? as i32,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("low_midi", Json::Int(i64::from(self.low_midi)));
        m.insert("high_midi", Json::Int(i64::from(self.high_midi)));
        Json::Obj(m)
    }

    /// True when `midi` lies inside the window.
    pub fn contains(&self, midi: i32) -> bool {
        midi >= self.low_midi && midi <= self.high_midi
    }
}

/// The minimum spacing tolerated below a register boundary.
///
/// Low interval limits are data, not law: a sine sub, a string section and a
/// distorted guitar tolerate completely different intervals at the same pitch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpacingLimit {
    /// The boundary this entry governs: pitches below this MIDI number.
    pub below_midi: i32,
    /// The smallest adjacent-voice interval allowed there.
    pub min_semitones: i32,
}

impl SpacingLimit {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<SpacingLimit, KbError> {
        Ok(SpacingLimit {
            below_midi: req_i64(v, path, "below_midi")? as i32,
            min_semitones: req_i64(v, path, "min_semitones")? as i32,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("below_midi", Json::Int(i64::from(self.below_midi)));
        m.insert("min_semitones", Json::Int(i64::from(self.min_semitones)));
        Json::Obj(m)
    }
}

/// Reads a `[{below_midi, min_semitones}]` table.
fn spacing_table(v: &Json, path: &str, key: &str) -> Result<Vec<SpacingLimit>, KbError> {
    let mut out = Vec::new();
    if let Some(Json::Arr(items)) = v.get(key) {
        for (i, item) in items.iter().enumerate() {
            out.push(SpacingLimit::from_json(
                item,
                &format!("{path}.{key}[{i}]"),
            )?);
        }
    }
    Ok(out)
}

/// A semantic chord quality from `knowledge/chord_qualities.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct ChordQuality {
    /// Stable identifier, e.g. `"dominant7_flat9"`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Triad quality id, e.g. `"major"`, `"sus4"`.
    pub triad: String,
    /// Seventh quality id, or `"none"`.
    pub seventh: String,
    /// Core chord degrees.
    pub degrees: Vec<String>,
    /// Stacked extensions (9, 11, 13).
    pub extensions: Vec<String>,
    /// Alterations such as `"b9"`, `"#11"`.
    pub alterations: Vec<String>,
    /// Added tones that imply no seventh.
    pub added: Vec<String>,
    /// Degrees a voicing may drop without changing identity.
    pub omissible_degrees: Vec<String>,
    /// Whether the notation implies a chordal seventh.
    pub implies_seventh: bool,
    /// Whether the symbol said `alt`.
    pub alt_dominant: bool,
    /// Family id shared with `ChordSpec::family_id`.
    pub family: String,
    /// The function this quality usually carries.
    pub typical_function: String,
    /// Scale ids that fit this quality.
    pub typical_scales: Vec<String>,
    /// Written symbols that produce it.
    pub symbol_examples: Vec<String>,
    /// Editorial notes.
    pub notes: String,
    /// Source ids backing the record.
    pub source_refs: Vec<String>,
    /// The complete original object.
    pub raw: Json,
}

impl ChordQuality {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<ChordQuality, KbError> {
        let id = req_str(v, path, "id")?;
        let p = format!("{path}[{id}]");
        Ok(ChordQuality {
            name: req_str(v, &p, "name")?,
            triad: req_str(v, &p, "triad")?,
            seventh: req_str(v, &p, "seventh")?,
            degrees: str_vec(v, &p, "degrees")?,
            extensions: str_vec(v, &p, "extensions")?,
            alterations: str_vec(v, &p, "alterations")?,
            added: str_vec(v, &p, "added")?,
            omissible_degrees: str_vec(v, &p, "omissible_degrees")?,
            implies_seventh: req_bool(v, &p, "implies_seventh")?,
            alt_dominant: req_bool(v, &p, "alt_dominant")?,
            family: req_str(v, &p, "family")?,
            typical_function: req_str(v, &p, "typical_function")?,
            typical_scales: str_vec(v, &p, "typical_scales")?,
            symbol_examples: opt_str_vec(v, &p, "symbol_examples")?,
            notes: opt_str(v, "notes"),
            source_refs: opt_str_vec(v, &p, "source_refs")?,
            raw: v.clone(),
            id,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("name", Json::Str(self.name.clone()));
        m.insert("triad", Json::Str(self.triad.clone()));
        m.insert("seventh", Json::Str(self.seventh.clone()));
        m.insert("degrees", strs(&self.degrees));
        m.insert("extensions", strs(&self.extensions));
        m.insert("alterations", strs(&self.alterations));
        m.insert("added", strs(&self.added));
        m.insert("omissible_degrees", strs(&self.omissible_degrees));
        m.insert("implies_seventh", Json::Bool(self.implies_seventh));
        m.insert("alt_dominant", Json::Bool(self.alt_dominant));
        m.insert("family", Json::Str(self.family.clone()));
        m.insert("typical_function", Json::Str(self.typical_function.clone()));
        m.insert("typical_scales", strs(&self.typical_scales));
        m.insert("symbol_examples", strs(&self.symbol_examples));
        m.insert("notes", Json::Str(self.notes.clone()));
        m.insert("source_refs", strs(&self.source_refs));
        merge_raw_extras(&mut m, &self.raw);
        Json::Obj(m)
    }

    /// Every degree string the record declares, for degree-syntax validation.
    pub fn all_degrees(&self) -> Vec<&String> {
        self.degrees
            .iter()
            .chain(self.extensions.iter())
            .chain(self.alterations.iter())
            .chain(self.added.iter())
            .chain(self.omissible_degrees.iter())
            .collect()
    }
}

/// A parser token from `knowledge/chord_symbols.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct ChordSymbolAlias {
    /// Stable identifier, e.g. `"sym_maj13"`.
    pub id: String,
    /// What kind of token this is (`"quality"`, `"alteration"`, …).
    pub token_kind: String,
    /// The canonical written suffix.
    pub written: String,
    /// Accepted ASCII spellings.
    pub ascii_variants: Vec<String>,
    /// Accepted Unicode spellings.
    pub unicode_variants: Vec<String>,
    /// The chord quality this token selects, when it selects one.
    pub quality_id: Option<String>,
    /// Degrees the token adds.
    pub degrees_added: Vec<String>,
    /// Degrees the token alters.
    pub degrees_altered: Vec<String>,
    /// Degrees the token removes.
    pub degrees_omitted: Vec<String>,
    /// Whether the token replaces the third (sus identity).
    pub replaces_third: bool,
    /// Whether the token implies a chordal seventh.
    pub implies_seventh: bool,
    /// Higher wins; every value in the table is distinct so parsing is total.
    pub precedence: i64,
    /// Canonical ASCII render form.
    pub canonical_ascii: String,
    /// Canonical Unicode render form.
    pub canonical_unicode: String,
    /// Editorial notes.
    pub notes: String,
    /// The complete original object.
    pub raw: Json,
}

impl ChordSymbolAlias {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<ChordSymbolAlias, KbError> {
        let id = req_str(v, path, "id")?;
        let p = format!("{path}[{id}]");
        Ok(ChordSymbolAlias {
            token_kind: req_str(v, &p, "token_kind")?,
            written: req_str(v, &p, "written")?,
            ascii_variants: str_vec(v, &p, "ascii_variants")?,
            unicode_variants: str_vec(v, &p, "unicode_variants")?,
            quality_id: nullable_str(v, &p, "quality_id")?,
            degrees_added: str_vec(v, &p, "degrees_added")?,
            degrees_altered: str_vec(v, &p, "degrees_altered")?,
            degrees_omitted: str_vec(v, &p, "degrees_omitted")?,
            replaces_third: req_bool(v, &p, "replaces_third")?,
            implies_seventh: req_bool(v, &p, "implies_seventh")?,
            precedence: req_i64(v, &p, "precedence")?,
            canonical_ascii: req_str(v, &p, "canonical_ascii")?,
            canonical_unicode: req_str(v, &p, "canonical_unicode")?,
            notes: opt_str(v, "notes"),
            raw: v.clone(),
            id,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("token_kind", Json::Str(self.token_kind.clone()));
        m.insert("written", Json::Str(self.written.clone()));
        m.insert("ascii_variants", strs(&self.ascii_variants));
        m.insert("unicode_variants", strs(&self.unicode_variants));
        m.insert(
            "quality_id",
            match &self.quality_id {
                Some(q) => Json::Str(q.clone()),
                None => Json::Null,
            },
        );
        m.insert("degrees_added", strs(&self.degrees_added));
        m.insert("degrees_altered", strs(&self.degrees_altered));
        m.insert("degrees_omitted", strs(&self.degrees_omitted));
        m.insert("replaces_third", Json::Bool(self.replaces_third));
        m.insert("implies_seventh", Json::Bool(self.implies_seventh));
        m.insert("precedence", Json::Int(self.precedence));
        m.insert("canonical_ascii", Json::Str(self.canonical_ascii.clone()));
        m.insert(
            "canonical_unicode",
            Json::Str(self.canonical_unicode.clone()),
        );
        m.insert("notes", Json::Str(self.notes.clone()));
        merge_raw_extras(&mut m, &self.raw);
        Json::Obj(m)
    }

    /// Every spelling that should map to this token.
    pub fn tokens(&self) -> Vec<&str> {
        let mut out = vec![self.written.as_str()];
        out.extend(self.ascii_variants.iter().map(String::as_str));
        out.extend(self.unicode_variants.iter().map(String::as_str));
        out
    }
}

/// A functional entry from `knowledge/functions.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionEntry {
    /// Stable identifier, e.g. `"neapolitan"`.
    pub id: String,
    /// `"major"`, `"minor"`, `"modal"`, `"any"`.
    pub key_context: String,
    /// Roman-numeral label.
    pub roman: String,
    /// Root scale degree, or `None` where the entry is a shape rather than a degree.
    pub scale_degree: Option<String>,
    /// Chord-quality id of the triad, or `"any"`.
    pub triad_quality: String,
    /// Chord-quality id of the seventh chord, or `"none"`.
    pub seventh_quality: String,
    /// Tonic, predominant, dominant, applied, chromatic, modal, pedal, …
    pub function_class: String,
    /// Roman numerals this entry typically moves to.
    pub typical_resolutions: Vec<String>,
    /// `"diatonic"`, `"applied"`, `"mixture"`, `"chromatic"`, …
    pub category: String,
    /// Bass degree in its usual position.
    pub typical_bass_degree: Option<String>,
    /// Degrees with an obligatory resolution.
    pub tendency_tones: Vec<String>,
    /// Editorial notes.
    pub notes: String,
    /// Source ids backing the record.
    pub source_refs: Vec<String>,
    /// The complete original object.
    pub raw: Json,
}

impl FunctionEntry {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<FunctionEntry, KbError> {
        let id = req_str(v, path, "id")?;
        let p = format!("{path}[{id}]");
        Ok(FunctionEntry {
            key_context: req_str(v, &p, "key_context")?,
            roman: req_str(v, &p, "roman")?,
            scale_degree: nullable_str(v, &p, "scale_degree")?,
            triad_quality: req_str(v, &p, "triad_quality")?,
            seventh_quality: req_str(v, &p, "seventh_quality")?,
            function_class: req_str(v, &p, "function_class")?,
            typical_resolutions: str_vec(v, &p, "typical_resolutions")?,
            category: req_str(v, &p, "category")?,
            typical_bass_degree: nullable_str(v, &p, "typical_bass_degree")?,
            tendency_tones: str_vec(v, &p, "tendency_tones")?,
            notes: opt_str(v, "notes"),
            source_refs: opt_str_vec(v, &p, "source_refs")?,
            raw: v.clone(),
            id,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("key_context", Json::Str(self.key_context.clone()));
        m.insert("roman", Json::Str(self.roman.clone()));
        m.insert(
            "scale_degree",
            match &self.scale_degree {
                Some(d) => Json::Str(d.clone()),
                None => Json::Null,
            },
        );
        m.insert("triad_quality", Json::Str(self.triad_quality.clone()));
        m.insert("seventh_quality", Json::Str(self.seventh_quality.clone()));
        m.insert("function_class", Json::Str(self.function_class.clone()));
        m.insert("typical_resolutions", strs(&self.typical_resolutions));
        m.insert("category", Json::Str(self.category.clone()));
        m.insert(
            "typical_bass_degree",
            match &self.typical_bass_degree {
                Some(d) => Json::Str(d.clone()),
                None => Json::Null,
            },
        );
        m.insert("tendency_tones", strs(&self.tendency_tones));
        m.insert("notes", Json::Str(self.notes.clone()));
        m.insert("source_refs", strs(&self.source_refs));
        merge_raw_extras(&mut m, &self.raw);
        Json::Obj(m)
    }
}

/// A voicing template from `knowledge/voicings.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct VoicingTemplate {
    /// Stable identifier, e.g. `"rootless_a_major7"`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// One of the thirteen voicing families.
    pub family: String,
    /// How many voices the template writes.
    pub voice_count: i64,
    /// Degrees bottom-up.
    pub degree_layout: Vec<String>,
    /// Degrees that must be present.
    pub required_degrees: Vec<String>,
    /// Degrees that may be present.
    pub optional_degrees: Vec<String>,
    /// Degrees a realisation may drop.
    pub omissible_degrees: Vec<String>,
    /// Where the shape sits comfortably.
    pub register_hint: MidiRange,
    /// Minimum adjacent-voice spacing per register band.
    pub min_spacing_semitones_by_register: Vec<SpacingLimit>,
    /// Profiles that use the template.
    pub style_profiles: Vec<String>,
    /// Editorial notes.
    pub notes: String,
    /// Source ids backing the record.
    pub source_refs: Vec<String>,
    /// The complete original object.
    pub raw: Json,
}

impl VoicingTemplate {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<VoicingTemplate, KbError> {
        let id = req_str(v, path, "id")?;
        let p = format!("{path}[{id}]");
        Ok(VoicingTemplate {
            name: req_str(v, &p, "name")?,
            family: req_str(v, &p, "family")?,
            voice_count: req_i64(v, &p, "voice_count")?,
            degree_layout: str_vec(v, &p, "degree_layout")?,
            required_degrees: str_vec(v, &p, "required_degrees")?,
            optional_degrees: str_vec(v, &p, "optional_degrees")?,
            omissible_degrees: str_vec(v, &p, "omissible_degrees")?,
            register_hint: MidiRange::from_json(
                v.field("register_hint")
                    .map_err(|e| KbError::shape(format!("{p}.register_hint"), e.message))?,
                &format!("{p}.register_hint"),
            )?,
            min_spacing_semitones_by_register: spacing_table(
                v,
                &p,
                "min_spacing_semitones_by_register",
            )?,
            style_profiles: str_vec(v, &p, "style_profiles")?,
            notes: opt_str(v, "notes"),
            source_refs: opt_str_vec(v, &p, "source_refs")?,
            raw: v.clone(),
            id,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("name", Json::Str(self.name.clone()));
        m.insert("family", Json::Str(self.family.clone()));
        m.insert("voice_count", Json::Int(self.voice_count));
        m.insert("degree_layout", strs(&self.degree_layout));
        m.insert("required_degrees", strs(&self.required_degrees));
        m.insert("optional_degrees", strs(&self.optional_degrees));
        m.insert("omissible_degrees", strs(&self.omissible_degrees));
        m.insert("register_hint", self.register_hint.to_json());
        m.insert(
            "min_spacing_semitones_by_register",
            Json::Arr(
                self.min_spacing_semitones_by_register
                    .iter()
                    .map(SpacingLimit::to_json)
                    .collect(),
            ),
        );
        m.insert("style_profiles", strs(&self.style_profiles));
        m.insert("notes", Json::Str(self.notes.clone()));
        m.insert("source_refs", strs(&self.source_refs));
        merge_raw_extras(&mut m, &self.raw);
        Json::Obj(m)
    }

    /// Every degree string the record declares.
    pub fn all_degrees(&self) -> Vec<&String> {
        self.degree_layout
            .iter()
            .chain(self.required_degrees.iter())
            .chain(self.optional_degrees.iter())
            .chain(self.omissible_degrees.iter())
            .collect()
    }

    /// The smallest adjacent-voice interval this template tolerates at `midi`.
    pub fn min_spacing_at(&self, midi: i32) -> Option<i32> {
        self.min_spacing_semitones_by_register
            .iter()
            .find(|l| midi < l.below_midi)
            .map(|l| l.min_semitones)
    }
}

/// Which melody degrees fit one slot of a progression schema.
#[derive(Clone, Debug, PartialEq)]
pub struct MelodyCompatibility {
    /// Roman numeral of the slot.
    pub roman: String,
    /// Chord-quality id realised in the slot.
    pub quality: String,
    /// Scale degrees that are chord tones here.
    pub chord_tones: Vec<String>,
    /// Scale degrees that work as colour.
    pub color_tones: Vec<String>,
    /// Scale degrees to avoid as structural melody notes.
    pub avoid_degrees: Vec<String>,
    /// Zero-based slot index.
    pub slot: i64,
}

impl MelodyCompatibility {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<MelodyCompatibility, KbError> {
        Ok(MelodyCompatibility {
            roman: req_str(v, path, "roman")?,
            quality: req_str(v, path, "quality")?,
            chord_tones: str_vec(v, path, "chord_tones")?,
            color_tones: str_vec(v, path, "color_tones")?,
            avoid_degrees: str_vec(v, path, "avoid_degrees")?,
            slot: req_i64(v, path, "slot")?,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("roman", Json::Str(self.roman.clone()));
        m.insert("quality", Json::Str(self.quality.clone()));
        m.insert("chord_tones", strs(&self.chord_tones));
        m.insert("color_tones", strs(&self.color_tones));
        m.insert("avoid_degrees", strs(&self.avoid_degrees));
        m.insert("slot", Json::Int(self.slot));
        Json::Obj(m)
    }
}

/// A progression schema from `knowledge/progressions.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressionSchema {
    /// Stable identifier, e.g. `"prog_ii_v_i_major"`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Roman numerals, in order.
    pub degrees: Vec<String>,
    /// Function class per slot.
    pub function_path: Vec<String>,
    /// Scale ids the schema fits.
    pub applicable_scales: Vec<String>,
    /// Profiles that use the schema.
    pub style_profiles: Vec<String>,
    /// Melody guidance per slot.
    pub melody_compatibility: Vec<MelodyCompatibility>,
    /// What the bass usually does.
    pub bass_implications: String,
    /// What the voices usually do.
    pub voice_leading_notes: String,
    /// Where the schema wants to go next.
    pub expected_destination: String,
    /// Explanation wording with `{key}` and `{slotN}` placeholders.
    pub explanation_template: String,
    /// Tags matched by a rule trigger's `progression_tag` selector.
    pub tags: Vec<String>,
    /// Editorial notes.
    pub notes: String,
    /// Source ids backing the record.
    pub source_refs: Vec<String>,
    /// The complete original object.
    pub raw: Json,
}

impl ProgressionSchema {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<ProgressionSchema, KbError> {
        let id = req_str(v, path, "id")?;
        let p = format!("{path}[{id}]");
        let mut melody_compatibility = Vec::new();
        for (i, mc) in v
            .arr_field("melody_compatibility")
            .map_err(|e| KbError::shape(format!("{p}.melody_compatibility"), e.message))?
            .iter()
            .enumerate()
        {
            melody_compatibility.push(MelodyCompatibility::from_json(
                mc,
                &format!("{p}.melody_compatibility[{i}]"),
            )?);
        }
        Ok(ProgressionSchema {
            name: req_str(v, &p, "name")?,
            degrees: str_vec(v, &p, "degrees")?,
            function_path: str_vec(v, &p, "function_path")?,
            applicable_scales: str_vec(v, &p, "applicable_scales")?,
            style_profiles: str_vec(v, &p, "style_profiles")?,
            melody_compatibility,
            bass_implications: opt_str(v, "bass_implications"),
            voice_leading_notes: opt_str(v, "voice_leading_notes"),
            expected_destination: opt_str(v, "expected_destination"),
            explanation_template: req_str(v, &p, "explanation_template")?,
            tags: opt_str_vec(v, &p, "tags")?,
            notes: opt_str(v, "notes"),
            source_refs: opt_str_vec(v, &p, "source_refs")?,
            raw: v.clone(),
            id,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("name", Json::Str(self.name.clone()));
        m.insert("degrees", strs(&self.degrees));
        m.insert("function_path", strs(&self.function_path));
        m.insert("applicable_scales", strs(&self.applicable_scales));
        m.insert("style_profiles", strs(&self.style_profiles));
        m.insert(
            "melody_compatibility",
            Json::Arr(
                self.melody_compatibility
                    .iter()
                    .map(MelodyCompatibility::to_json)
                    .collect(),
            ),
        );
        m.insert(
            "bass_implications",
            Json::Str(self.bass_implications.clone()),
        );
        m.insert(
            "voice_leading_notes",
            Json::Str(self.voice_leading_notes.clone()),
        );
        m.insert(
            "expected_destination",
            Json::Str(self.expected_destination.clone()),
        );
        m.insert(
            "explanation_template",
            Json::Str(self.explanation_template.clone()),
        );
        m.insert("tags", strs(&self.tags));
        m.insert("notes", Json::Str(self.notes.clone()));
        m.insert("source_refs", strs(&self.source_refs));
        merge_raw_extras(&mut m, &self.raw);
        Json::Obj(m)
    }

    /// Fills `{key}` and `{slotN}` placeholders in the explanation template.
    pub fn explain(&self, key: &str, slots: &[String]) -> String {
        let mut out = self.explanation_template.replace("{key}", key);
        for (i, s) in slots.iter().enumerate() {
            out = out.replace(&format!("{{slot{i}}}"), s);
        }
        out
    }
}

/// A cadence schema: a progression schema plus its cadential identity.
#[derive(Clone, Debug, PartialEq)]
pub struct CadenceSchema {
    /// The shared progression fields.
    pub schema: ProgressionSchema,
    /// One of the eight `CadenceKind` ids.
    pub cadence_kind: String,
    /// How completely the cadence closes, 0..=1.
    pub closure_strength: f64,
}

impl CadenceSchema {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<CadenceSchema, KbError> {
        let schema = ProgressionSchema::from_json(v, path)?;
        let p = format!("{path}[{}]", schema.id);
        Ok(CadenceSchema {
            cadence_kind: req_str(v, &p, "cadence_kind")?,
            closure_strength: req_f64(v, &p, "closure_strength")?,
            schema,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = match self.schema.to_json() {
            Json::Obj(m) => m,
            _ => JsonMap::new(),
        };
        m.insert("cadence_kind", Json::Str(self.cadence_kind.clone()));
        m.insert("closure_strength", Json::Float(self.closure_strength));
        Json::Obj(m)
    }

    /// The record id.
    pub fn id(&self) -> &str {
        &self.schema.id
    }
}

/// The machine-usable rhythm block of an arrangement pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct RhythmPattern {
    /// One bar's length in quarter notes.
    pub bar_length_qn: BeatTime,
    /// The grid the onsets sit on.
    pub grid_qn: BeatTime,
    /// Onset offsets within one bar.
    pub onsets: Vec<BeatTime>,
    /// `"grid"`, `"to_next"`, `"full_slot"` or `"mixed"`.
    pub sustain: String,
    /// Velocity per onset, cycled if shorter.
    pub velocity_curve: Vec<i64>,
}

impl RhythmPattern {
    /// Reads the JSON form. Positions are exact rationals, never floats.
    pub fn from_json(v: &Json, path: &str) -> Result<RhythmPattern, KbError> {
        let mut onsets = Vec::new();
        for (i, o) in v
            .arr_field("onsets")
            .map_err(|e| KbError::shape(format!("{path}.onsets"), e.message))?
            .iter()
            .enumerate()
        {
            let s = o.as_str().ok_or_else(|| {
                KbError::shape(format!("{path}.onsets[{i}]"), "expected a rational string")
            })?;
            onsets.push(
                BeatTime::parse(s)
                    .map_err(|e| KbError::shape(format!("{path}.onsets[{i}]"), e.message))?,
            );
        }
        let mut velocity_curve = Vec::new();
        for (i, c) in v
            .arr_field("velocity_curve")
            .map_err(|e| KbError::shape(format!("{path}.velocity_curve"), e.message))?
            .iter()
            .enumerate()
        {
            velocity_curve.push(c.as_i64().ok_or_else(|| {
                KbError::shape(
                    format!("{path}.velocity_curve[{i}]"),
                    "expected an integer velocity",
                )
            })?);
        }
        Ok(RhythmPattern {
            bar_length_qn: beat_field(v, path, "bar_length_qn")?,
            grid_qn: beat_field(v, path, "grid_qn")?,
            onsets,
            sustain: req_str(v, path, "sustain")?,
            velocity_curve,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("bar_length_qn", Json::Str(self.bar_length_qn.to_display()));
        m.insert("grid_qn", Json::Str(self.grid_qn.to_display()));
        m.insert(
            "onsets",
            Json::Arr(
                self.onsets
                    .iter()
                    .map(|o| Json::Str(o.to_display()))
                    .collect(),
            ),
        );
        m.insert("sustain", Json::Str(self.sustain.clone()));
        m.insert(
            "velocity_curve",
            Json::Arr(self.velocity_curve.iter().map(|v| Json::Int(*v)).collect()),
        );
        Json::Obj(m)
    }
}

/// An accompaniment pattern from `knowledge/arrangement_patterns.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrangementPattern {
    /// Stable identifier, e.g. `"arr_chorale_voicing"`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// One of the sixteen arrangement roles.
    pub role: String,
    /// `"low"`, `"mid"`, `"high"`, …
    pub register: String,
    /// Usable MIDI window.
    pub range: MidiRange,
    /// Onsets-per-bar budget as a normalised control.
    pub density: f64,
    /// `"sustained"`, `"driving"`, `"syncopated"`, …
    pub rhythmic_activity: String,
    /// How much of the harmony this part carries.
    pub harmonic_responsibility: String,
    /// `"foreground"` or `"background"`.
    pub priority: String,
    /// Doubling intervals the pattern permits.
    pub allowed_doubling: Vec<String>,
    /// Simultaneous notes the pattern writes.
    pub polyphony: i64,
    /// Articulations the pattern tends to use.
    pub articulation_tendency: Vec<String>,
    /// `"grid"`, `"full_slot"`, `"short"`, …
    pub note_length_tendency: String,
    /// Sections the pattern participates in.
    pub section_participation: Vec<String>,
    /// How much energy the pattern adds, 0..=1.
    pub energy_contribution: f64,
    /// `"seamless"`, `"restart"`, `"tail"`, …
    pub loop_behavior: String,
    /// The machine-usable onset grid.
    pub rhythm: RhythmPattern,
    /// Profiles that use the pattern.
    pub style_profiles: Vec<String>,
    /// Editorial notes.
    pub notes: String,
    /// Source ids backing the record.
    pub source_refs: Vec<String>,
    /// The complete original object.
    pub raw: Json,
}

impl ArrangementPattern {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<ArrangementPattern, KbError> {
        let id = req_str(v, path, "id")?;
        let p = format!("{path}[{id}]");
        Ok(ArrangementPattern {
            name: req_str(v, &p, "name")?,
            role: req_str(v, &p, "role")?,
            register: req_str(v, &p, "register")?,
            range: MidiRange::from_json(
                v.field("range")
                    .map_err(|e| KbError::shape(format!("{p}.range"), e.message))?,
                &format!("{p}.range"),
            )?,
            density: req_f64(v, &p, "density")?,
            rhythmic_activity: req_str(v, &p, "rhythmic_activity")?,
            harmonic_responsibility: req_str(v, &p, "harmonic_responsibility")?,
            priority: req_str(v, &p, "priority")?,
            allowed_doubling: str_vec(v, &p, "allowed_doubling")?,
            polyphony: req_i64(v, &p, "polyphony")?,
            articulation_tendency: str_vec(v, &p, "articulation_tendency")?,
            note_length_tendency: req_str(v, &p, "note_length_tendency")?,
            section_participation: str_vec(v, &p, "section_participation")?,
            energy_contribution: req_f64(v, &p, "energy_contribution")?,
            loop_behavior: req_str(v, &p, "loop_behavior")?,
            rhythm: RhythmPattern::from_json(
                v.field("rhythm")
                    .map_err(|e| KbError::shape(format!("{p}.rhythm"), e.message))?,
                &format!("{p}.rhythm"),
            )?,
            style_profiles: str_vec(v, &p, "style_profiles")?,
            notes: opt_str(v, "notes"),
            source_refs: opt_str_vec(v, &p, "source_refs")?,
            raw: v.clone(),
            id,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("name", Json::Str(self.name.clone()));
        m.insert("role", Json::Str(self.role.clone()));
        m.insert("register", Json::Str(self.register.clone()));
        m.insert("range", self.range.to_json());
        m.insert("density", Json::Float(self.density));
        m.insert(
            "rhythmic_activity",
            Json::Str(self.rhythmic_activity.clone()),
        );
        m.insert(
            "harmonic_responsibility",
            Json::Str(self.harmonic_responsibility.clone()),
        );
        m.insert("priority", Json::Str(self.priority.clone()));
        m.insert("allowed_doubling", strs(&self.allowed_doubling));
        m.insert("polyphony", Json::Int(self.polyphony));
        m.insert("articulation_tendency", strs(&self.articulation_tendency));
        m.insert(
            "note_length_tendency",
            Json::Str(self.note_length_tendency.clone()),
        );
        m.insert("section_participation", strs(&self.section_participation));
        m.insert("energy_contribution", Json::Float(self.energy_contribution));
        m.insert("loop_behavior", Json::Str(self.loop_behavior.clone()));
        m.insert("rhythm", self.rhythm.to_json());
        m.insert("style_profiles", strs(&self.style_profiles));
        m.insert("notes", Json::Str(self.notes.clone()));
        m.insert("source_refs", strs(&self.source_refs));
        merge_raw_extras(&mut m, &self.raw);
        Json::Obj(m)
    }
}

/// A generic part profile from `knowledge/instrument_profiles.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct InstrumentProfile {
    /// Stable identifier, e.g. `"melody_lead"`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Playable MIDI window.
    pub range: MidiRange,
    /// The window where the part sounds best.
    pub comfortable_range: MidiRange,
    /// Maximum simultaneous notes.
    pub polyphony: i64,
    /// Minimum adjacent-voice spacing per register band.
    pub low_interval_limits: Vec<SpacingLimit>,
    /// Arrangement roles this profile suits.
    pub typical_roles: Vec<String>,
    /// Articulations the part supports.
    pub articulations: Vec<String>,
    /// Preferred onset density, 0..=1.
    pub density_preference: f64,
    /// Editorial notes.
    pub notes: String,
    /// Source ids backing the record.
    pub source_refs: Vec<String>,
    /// The complete original object.
    pub raw: Json,
}

impl InstrumentProfile {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<InstrumentProfile, KbError> {
        let id = req_str(v, path, "id")?;
        let p = format!("{path}[{id}]");
        Ok(InstrumentProfile {
            name: req_str(v, &p, "name")?,
            range: MidiRange::from_json(
                v.field("range")
                    .map_err(|e| KbError::shape(format!("{p}.range"), e.message))?,
                &format!("{p}.range"),
            )?,
            comfortable_range: MidiRange::from_json(
                v.field("comfortable_range")
                    .map_err(|e| KbError::shape(format!("{p}.comfortable_range"), e.message))?,
                &format!("{p}.comfortable_range"),
            )?,
            polyphony: req_i64(v, &p, "polyphony")?,
            low_interval_limits: spacing_table(v, &p, "low_interval_limits")?,
            typical_roles: str_vec(v, &p, "typical_roles")?,
            articulations: str_vec(v, &p, "articulations")?,
            density_preference: req_f64(v, &p, "density_preference")?,
            notes: opt_str(v, "notes"),
            source_refs: opt_str_vec(v, &p, "source_refs")?,
            raw: v.clone(),
            id,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("name", Json::Str(self.name.clone()));
        m.insert("range", self.range.to_json());
        m.insert("comfortable_range", self.comfortable_range.to_json());
        m.insert("polyphony", Json::Int(self.polyphony));
        m.insert(
            "low_interval_limits",
            Json::Arr(
                self.low_interval_limits
                    .iter()
                    .map(SpacingLimit::to_json)
                    .collect(),
            ),
        );
        m.insert("typical_roles", strs(&self.typical_roles));
        m.insert("articulations", strs(&self.articulations));
        m.insert("density_preference", Json::Float(self.density_preference));
        m.insert("notes", Json::Str(self.notes.clone()));
        m.insert("source_refs", strs(&self.source_refs));
        merge_raw_extras(&mut m, &self.raw);
        Json::Obj(m)
    }

    /// The smallest adjacent-voice interval this part tolerates at `midi`.
    pub fn min_spacing_at(&self, midi: i32) -> Option<i32> {
        self.low_interval_limits
            .iter()
            .find(|l| midi < l.below_midi)
            .map(|l| l.min_semitones)
    }
}

/// A modal *character* record from `knowledge/modes.json`.
///
/// Deliberately duplicates no interval data: the collection itself lives in
/// `scales.json` and is referenced by [`ModeRecord::scale_id`].
#[derive(Clone, Debug, PartialEq)]
pub struct ModeRecord {
    /// Stable identifier, e.g. `"mode_lydian"`.
    pub id: String,
    /// The scale id whose intervals this record describes.
    pub scale_id: String,
    /// `"diatonic"`, `"melodic_minor_mode"`, `"harmonic_minor_mode"`, …
    pub mode_family: String,
    /// Position on the bright-to-dark ordering within the family.
    pub brightness_rank: i64,
    /// The degree that gives the mode its identity.
    pub characteristic_degree: String,
    /// Chord-quality id of the usual tonic sonority.
    pub typical_tonic_chord: String,
    /// The cadential gesture that establishes the mode.
    pub typical_cadential_gesture: String,
    /// What the mode sounds like and why.
    pub character: String,
    /// Scale ids the mode is most usefully contrasted with.
    pub contrast_with: Vec<String>,
    /// Source ids backing the record.
    pub source_refs: Vec<String>,
    /// Editorial notes.
    pub notes: String,
    /// The complete original object.
    pub raw: Json,
}

impl ModeRecord {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<ModeRecord, KbError> {
        let id = req_str(v, path, "id")?;
        let p = format!("{path}[{id}]");
        Ok(ModeRecord {
            scale_id: req_str(v, &p, "scale_id")?,
            mode_family: req_str(v, &p, "mode_family")?,
            brightness_rank: req_i64(v, &p, "brightness_rank")?,
            characteristic_degree: req_str(v, &p, "characteristic_degree")?,
            typical_tonic_chord: req_str(v, &p, "typical_tonic_chord")?,
            typical_cadential_gesture: req_str(v, &p, "typical_cadential_gesture")?,
            character: req_str(v, &p, "character")?,
            contrast_with: str_vec(v, &p, "contrast_with")?,
            source_refs: opt_str_vec(v, &p, "source_refs")?,
            notes: opt_str(v, "notes"),
            raw: v.clone(),
            id,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("scale_id", Json::Str(self.scale_id.clone()));
        m.insert("mode_family", Json::Str(self.mode_family.clone()));
        m.insert("brightness_rank", Json::Int(self.brightness_rank));
        m.insert(
            "characteristic_degree",
            Json::Str(self.characteristic_degree.clone()),
        );
        m.insert(
            "typical_tonic_chord",
            Json::Str(self.typical_tonic_chord.clone()),
        );
        m.insert(
            "typical_cadential_gesture",
            Json::Str(self.typical_cadential_gesture.clone()),
        );
        m.insert("character", Json::Str(self.character.clone()));
        m.insert("contrast_with", strs(&self.contrast_with));
        m.insert("source_refs", strs(&self.source_refs));
        m.insert("notes", Json::Str(self.notes.clone()));
        merge_raw_extras(&mut m, &self.raw);
        Json::Obj(m)
    }
}

/// An interval record from `knowledge/intervals.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct IntervalRecord {
    /// Stable identifier, e.g. `"interval_perfect_5"`.
    pub id: String,
    /// Short name such as `"P5"`.
    pub short_name: String,
    /// Long name such as `"perfect fifth"`.
    pub name: String,
    /// Size in semitones; may be negative for notational artefacts.
    pub semitones: i64,
    /// Diatonic number 1..=15.
    pub diatonic_number: i64,
    /// `"perfect"`, `"major"`, `"minor"`, `"augmented"`, `"diminished"`.
    pub quality: String,
    /// `"perfect_consonance"`, `"imperfect_consonance"`, `"dissonance"`.
    pub consonance_class: String,
    /// Short name of the inversion.
    pub inversion: String,
    /// Whether the interval exceeds an octave.
    pub is_compound: bool,
    /// How the interval is usually handled.
    pub typical_treatment: String,
    /// Source ids backing the record.
    pub source_refs: Vec<String>,
    /// The complete original object.
    pub raw: Json,
}

impl IntervalRecord {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<IntervalRecord, KbError> {
        let id = req_str(v, path, "id")?;
        let p = format!("{path}[{id}]");
        Ok(IntervalRecord {
            short_name: req_str(v, &p, "short_name")?,
            name: req_str(v, &p, "name")?,
            semitones: req_i64(v, &p, "semitones")?,
            diatonic_number: req_i64(v, &p, "diatonic_number")?,
            quality: req_str(v, &p, "quality")?,
            consonance_class: req_str(v, &p, "consonance_class")?,
            inversion: req_str(v, &p, "inversion")?,
            is_compound: req_bool(v, &p, "is_compound")?,
            typical_treatment: req_str(v, &p, "typical_treatment")?,
            source_refs: opt_str_vec(v, &p, "source_refs")?,
            raw: v.clone(),
            id,
        })
    }

    /// Writes the JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("short_name", Json::Str(self.short_name.clone()));
        m.insert("name", Json::Str(self.name.clone()));
        m.insert("semitones", Json::Int(self.semitones));
        m.insert("diatonic_number", Json::Int(self.diatonic_number));
        m.insert("quality", Json::Str(self.quality.clone()));
        m.insert("consonance_class", Json::Str(self.consonance_class.clone()));
        m.insert("inversion", Json::Str(self.inversion.clone()));
        m.insert("is_compound", Json::Bool(self.is_compound));
        m.insert(
            "typical_treatment",
            Json::Str(self.typical_treatment.clone()),
        );
        m.insert("source_refs", strs(&self.source_refs));
        merge_raw_extras(&mut m, &self.raw);
        Json::Obj(m)
    }
}

// ---------------------------------------------------------------------------
// manifest
// ---------------------------------------------------------------------------

/// The literal `content_sha256` value meaning "authored but not yet hashed".
pub const PENDING_HASH: &str = "PENDING";

/// `knowledge/manifest.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    /// Semantic version of the knowledge content.
    pub knowledge_version: String,
    /// Semantic version of the file format.
    pub schema_version: String,
    /// When the bundle was generated.
    pub generated_at: String,
    /// SHA-256 over the canonical JSON of every other knowledge file, or
    /// [`PENDING_HASH`] before `xtask stamp-manifest` has run.
    pub content_sha256: String,
    /// Declared record counts, checked against the loaded bundle.
    pub counts: BTreeMap<String, usize>,
    /// Declared per-domain rule counts.
    pub rules_by_domain: BTreeMap<String, usize>,
    /// Every hashed file, in sorted relative-path order.
    pub files: Vec<String>,
    /// Editorial notes.
    pub notes: String,
    /// The complete original object.
    pub raw: Json,
}

impl Manifest {
    /// Reads the JSON form.
    pub fn from_json(v: &Json, path: &str) -> Result<Manifest, KbError> {
        let counts = usize_map(v, path, "counts")?;
        let rules_by_domain = usize_map(v, path, "rules_by_domain").unwrap_or_default();
        Ok(Manifest {
            knowledge_version: req_str(v, path, "knowledge_version")?,
            schema_version: req_str(v, path, "schema_version")?,
            generated_at: req_str(v, path, "generated_at")?,
            content_sha256: req_str(v, path, "content_sha256")?,
            counts,
            rules_by_domain,
            files: str_vec(v, path, "files")?,
            notes: opt_str(v, "notes"),
            raw: v.clone(),
        })
    }

    /// Writes the JSON form, preserving authored field order and extras.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert(
            "knowledge_version",
            Json::Str(self.knowledge_version.clone()),
        );
        m.insert("schema_version", Json::Str(self.schema_version.clone()));
        m.insert("generated_at", Json::Str(self.generated_at.clone()));
        m.insert("content_sha256", Json::Str(self.content_sha256.clone()));
        let mut c = JsonMap::new();
        for (k, v) in &self.counts {
            c.insert(k.clone(), Json::Int(*v as i64));
        }
        m.insert("counts", Json::Obj(c));
        let mut d = JsonMap::new();
        for (k, v) in &self.rules_by_domain {
            d.insert(k.clone(), Json::Int(*v as i64));
        }
        m.insert("rules_by_domain", Json::Obj(d));
        m.insert("files", strs(&self.files));
        m.insert("notes", Json::Str(self.notes.clone()));
        merge_raw_extras(&mut m, &self.raw);
        Json::Obj(m)
    }

    /// True when the manifest has not been stamped with a real hash yet.
    pub fn is_pending(&self) -> bool {
        self.content_sha256 == PENDING_HASH
    }
}

/// Reads an object of non-negative integers.
fn usize_map(v: &Json, path: &str, key: &str) -> Result<BTreeMap<String, usize>, KbError> {
    let obj = v
        .obj_field(key)
        .map_err(|e| KbError::shape(format!("{path}.{key}"), e.message))?;
    let mut out = BTreeMap::new();
    for (k, val) in obj.iter() {
        let n = val.as_i64().ok_or_else(|| {
            KbError::shape(
                format!("{path}.{key}.{k}"),
                format!("expected an integer, found {}", val.type_name()),
            )
        })?;
        out.insert(
            k.to_string(),
            usize::try_from(n)
                .map_err(|_| KbError::shape(format!("{path}.{key}.{k}"), "count is negative"))?,
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qjson::json_obj;

    #[test]
    fn rule_kind_hard_and_rejecting_tiers() {
        assert!(RuleKind::HardIntegrity.is_hard());
        assert!(RuleKind::MathematicalInvariant.is_hard());
        assert!(RuleKind::LoopIntegrity.is_hard());
        assert!(!RuleKind::TheoryDefault.is_hard());
        assert!(RuleKind::HardIntegrity.rejects());
        assert!(!RuleKind::LoopIntegrity.rejects());
        assert!(!RuleKind::StyleSensitivePreference.rejects());
    }

    #[test]
    fn closed_enums_round_trip_every_variant() {
        for k in RuleKind::all() {
            assert_eq!(RuleKind::parse(k.id()), Some(*k));
        }
        for d in RuleDomain::all() {
            assert_eq!(RuleDomain::parse(d.id()), Some(*d));
        }
        for e in RuleEvent::all() {
            assert_eq!(RuleEvent::parse(e.id()), Some(*e));
        }
        assert_eq!(RuleEvent::all().len(), 14);
        assert_eq!(RuleKind::all().len(), 8);
        assert_eq!(RuleDomain::all().len(), 7);
        assert_eq!(RuleEvent::parse("not_an_event"), None);
    }

    #[test]
    fn rule_override_parses_both_forms() {
        assert_eq!(
            RuleOverride::from_json(&Json::Str("disabled".into()), "p").unwrap(),
            RuleOverride::Disabled
        );
        assert_eq!(
            RuleOverride::from_json(&Json::Float(1.5), "p").unwrap(),
            RuleOverride::Multiplier(1.5)
        );
        assert_eq!(
            RuleOverride::from_json(&Json::Int(0), "p").unwrap(),
            RuleOverride::Multiplier(0.0)
        );
        assert!(RuleOverride::from_json(&Json::Float(-1.0), "p").is_err());
        assert!(RuleOverride::from_json(&Json::Str("off".into()), "p").is_err());
        assert_eq!(RuleOverride::Disabled.multiplier(), 0.0);
    }

    #[test]
    fn trigger_keeps_selectors_as_raw_json() {
        let t = RuleTrigger::from_json(
            &json_obj! { "event" => "chord_selected", "chord_family" => "dominant" },
            "t",
        )
        .unwrap();
        assert_eq!(t.event, RuleEvent::ChordSelected);
        assert_eq!(t.selectors.len(), 1);
        assert_eq!(
            t.selectors.get("chord_family").and_then(Json::as_str),
            Some("dominant")
        );
        assert_eq!(
            t.to_json().to_string(),
            r#"{"event":"chord_selected","chord_family":"dominant"}"#
        );
    }

    #[test]
    fn trigger_rejects_unknown_event() {
        let e = RuleTrigger::from_json(&json_obj! { "event" => "banana" }, "t").unwrap_err();
        assert_eq!(e.code, "KB_UNKNOWN_ENUM");
    }

    #[test]
    fn nullable_str_maps_null_to_none() {
        let v = json_obj! { "parent" => Json::Null };
        assert_eq!(nullable_str(&v, "p", "parent").unwrap(), None);
        assert_eq!(nullable_str(&v, "p", "absent").unwrap(), None);
        let v2 = json_obj! { "parent" => "major" };
        assert_eq!(
            nullable_str(&v2, "p", "parent").unwrap(),
            Some("major".to_string())
        );
        let v3 = json_obj! { "parent" => 3 };
        assert!(nullable_str(&v3, "p", "parent").is_err());
    }

    #[test]
    fn midi_range_contains_is_inclusive() {
        let r = MidiRange {
            low_midi: 48,
            high_midi: 72,
        };
        assert!(r.contains(48) && r.contains(72) && !r.contains(47) && !r.contains(73));
    }

    #[test]
    fn merge_raw_extras_never_overwrites_modelled_fields() {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str("kept".into()));
        merge_raw_extras(
            &mut m,
            &json_obj! { "id" => "discarded", "future_field" => 7 },
        );
        assert_eq!(m.get("id").and_then(Json::as_str), Some("kept"));
        assert_eq!(m.get("future_field").and_then(Json::as_i64), Some(7));
    }

    #[test]
    fn manifest_pending_sentinel() {
        let v = json_obj! {
            "knowledge_version" => "1.0.0",
            "schema_version" => "1.0.0",
            "generated_at" => "2026-07-26T00:00:00Z",
            "content_sha256" => "PENDING",
            "counts" => json_obj! { "sources" => 9 },
            "files" => qjson::json_arr!["sources.json"],
        };
        let m = Manifest::from_json(&v, "manifest.json").unwrap();
        assert!(m.is_pending());
        assert_eq!(m.counts.get("sources"), Some(&9));
        assert!(m.rules_by_domain.is_empty());
    }

    #[test]
    fn progression_explain_fills_placeholders() {
        let p = ProgressionSchema {
            id: "x".into(),
            name: "x".into(),
            degrees: vec![],
            function_path: vec![],
            applicable_scales: vec![],
            style_profiles: vec![],
            melody_compatibility: vec![],
            bass_implications: String::new(),
            voice_leading_notes: String::new(),
            expected_destination: String::new(),
            explanation_template: "In {key}: {slot0} then {slot1}.".into(),
            tags: vec![],
            notes: String::new(),
            source_refs: vec![],
            raw: Json::Null,
        };
        assert_eq!(
            p.explain("C major", &["Dm7".into(), "G7".into()]),
            "In C major: Dm7 then G7."
        );
    }
}
