//! Candidates, their scores, and the decision traces that explain them.
//!
//! Scores never collapse to one opaque number: a [`ScoreVector`] keeps every
//! named component in insertion order alongside the weighted total, so an
//! explanation can always name the components that decided the ranking.

use crate::chord::ChordEvent;
use crate::error::DomainError;
use crate::note::{Note, NoteId};
use crate::structure::{ArrangementRole, LoopIntent};
use crate::time::BeatTime;
use qjson::{json_obj, Json, JsonMap};

/// The score components every generation stage reports, in canonical order.
pub const SCORE_COMPONENTS: &[&str] = &[
    "melody_fit",
    "harmonic_coherence",
    "functional_or_modal_coherence",
    "voice_leading",
    "extension_appropriateness",
    "style_match",
    "phrase_direction",
    "bass_quality",
    "arrangement_clarity",
    "loop_compatibility",
    "complexity_target",
    "chromaticism_target",
    "candidate_diversity",
];

/// A named, insertion-ordered set of score components plus the weighted total.
///
/// The raw components are always retained; `total` is stored separately so a
/// caller can see both what was measured and how it was combined.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScoreVector {
    components: Vec<(String, f64)>,
    total: f64,
}

impl ScoreVector {
    /// An empty score vector.
    pub fn new() -> Self {
        ScoreVector::default()
    }

    /// Sets a component, keeping its original position if it already exists.
    pub fn set(&mut self, name: &str, value: f64) {
        if let Some(slot) = self.components.iter_mut().find(|(n, _)| n == name) {
            slot.1 = value;
        } else {
            self.components.push((name.to_string(), value));
        }
    }

    /// Adds `delta` to a component, creating it at the end if it is new.
    pub fn add(&mut self, name: &str, delta: f64) {
        let current = self.get(name);
        self.set(name, current + delta);
    }

    /// Reads a component; missing components read as 0.0.
    pub fn get(&self, name: &str) -> f64 {
        self.components
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| *v)
            .unwrap_or(0.0)
    }

    /// True when the component was explicitly set.
    pub fn contains(&self, name: &str) -> bool {
        self.components.iter().any(|(n, _)| n == name)
    }

    /// Number of components.
    pub fn len(&self) -> usize {
        self.components.len()
    }

    /// True when nothing has been scored yet.
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// The components in insertion order.
    pub fn components(&self) -> impl Iterator<Item = (&str, f64)> {
        self.components.iter().map(|(n, v)| (n.as_str(), *v))
    }

    /// The weighted total, as stored by the ranking stage.
    pub fn total(&self) -> f64 {
        self.total
    }

    /// Stores the weighted total.
    pub fn set_total(&mut self, t: f64) {
        self.total = t;
    }

    /// Computes and stores a total as the weighted sum of the components.
    ///
    /// Components without a weight contribute nothing; the sum is divided by
    /// the total weight actually used, so a partial profile still produces a
    /// comparable number. Returns the new total.
    pub fn recompute_total(&mut self, weights: &[(&str, f64)]) -> f64 {
        let mut sum = 0.0;
        let mut denom = 0.0;
        for (name, weight) in weights {
            if self.contains(name) {
                sum += self.get(name) * weight;
                denom += weight.abs();
            }
        }
        self.total = if denom > 0.0 { sum / denom } else { 0.0 };
        self.total
    }

    /// JSON form: `{"total": …, "components": {…}}`, components in insertion
    /// order.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        for (name, value) in &self.components {
            m.insert(name.clone(), Json::Float(*value));
        }
        json_obj! {
            "total" => self.total,
            "components" => Json::Obj(m),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<ScoreVector, DomainError> {
        let mut out = ScoreVector::new();
        out.set_total(v.opt_f64_field("total")?.unwrap_or(0.0));
        if let Some(Json::Obj(m)) = v.get("components") {
            for (name, value) in m.iter() {
                let value = value.as_f64().ok_or_else(|| {
                    DomainError::invalid_argument(format!(
                        "score component {name:?} is not a number"
                    ))
                })?;
                out.set(name, value);
            }
        }
        Ok(out)
    }
}

/// Whether a rule fired, and how.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RuleStatus {
    /// The rule matched and was applied.
    Applied,
    /// The rule matched but an exception bypassed it.
    Bypassed,
    /// The rule did not apply here.
    NotApplicable,
    /// The rule applied and the candidate broke it.
    Violated,
}

impl RuleStatus {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            RuleStatus::Applied => "applied",
            RuleStatus::Bypassed => "bypassed",
            RuleStatus::NotApplicable => "not_applicable",
            RuleStatus::Violated => "violated",
        }
    }

    /// Parses the identifier produced by [`RuleStatus::id`].
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "applied" => Some(RuleStatus::Applied),
            "bypassed" => Some(RuleStatus::Bypassed),
            "not_applicable" => Some(RuleStatus::NotApplicable),
            "violated" => Some(RuleStatus::Violated),
            _ => None,
        }
    }
}

/// One rule's contribution to a candidate.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleApplication {
    /// Rule identifier from the knowledge base.
    pub rule_id: String,
    /// Whether it fired.
    pub status: RuleStatus,
    /// Score change it caused.
    pub score_delta: f64,
    /// Conditions that matched.
    pub matched_conditions: Vec<String>,
    /// Exceptions that matched.
    pub matched_exceptions: Vec<String>,
    /// Sources backing the rule.
    pub source_ids: Vec<String>,
    /// Human-readable explanation.
    pub explanation: String,
}

impl RuleApplication {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "rule_id" => self.rule_id.clone(),
            "status" => self.status.id(),
            "score_delta" => self.score_delta,
            "matched_conditions" => str_array(&self.matched_conditions),
            "matched_exceptions" => str_array(&self.matched_exceptions),
            "source_ids" => str_array(&self.source_ids),
            "explanation" => self.explanation.clone(),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<RuleApplication, DomainError> {
        let s = v.str_field("status")?;
        let status = RuleStatus::parse(s)
            .ok_or_else(|| DomainError::invalid_argument(format!("bad rule status {s:?}")))?;
        Ok(RuleApplication {
            rule_id: v.str_field("rule_id")?.to_string(),
            status,
            score_delta: v.opt_f64_field("score_delta")?.unwrap_or(0.0),
            matched_conditions: string_list(v, "matched_conditions"),
            matched_exceptions: string_list(v, "matched_exceptions"),
            source_ids: string_list(v, "source_ids"),
            explanation: v.opt_str_field("explanation")?.unwrap_or("").to_string(),
        })
    }
}

/// How serious a warning is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Informational.
    Info,
    /// Minor issue.
    Minor,
    /// Moderate issue.
    Moderate,
    /// Major issue.
    Major,
}

impl Severity {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Minor => "minor",
            Severity::Moderate => "moderate",
            Severity::Major => "major",
        }
    }

    /// Parses the identifier produced by [`Severity::id`].
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "info" => Some(Severity::Info),
            "minor" => Some(Severity::Minor),
            "moderate" => Some(Severity::Moderate),
            "major" => Some(Severity::Major),
            _ => None,
        }
    }
}

/// A non-fatal finding attached to a candidate or a loop report.
#[derive(Clone, Debug, PartialEq)]
pub struct Warning {
    /// Stable code.
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// Severity.
    pub severity: Severity,
}

impl Warning {
    /// Builds a warning.
    pub fn new(code: impl Into<String>, message: impl Into<String>, severity: Severity) -> Warning {
        Warning {
            code: code.into(),
            message: message.into(),
            severity,
        }
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "code" => self.code.clone(),
            "message" => self.message.clone(),
            "severity" => self.severity.id(),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<Warning, DomainError> {
        let s = v.str_field("severity")?;
        let severity = Severity::parse(s)
            .ok_or_else(|| DomainError::invalid_argument(format!("bad severity {s:?}")))?;
        Ok(Warning {
            code: v.str_field("code")?.to_string(),
            message: v.opt_str_field("message")?.unwrap_or("").to_string(),
            severity,
        })
    }
}

/// The full record of why a candidate looks the way it does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecisionTrace {
    /// Candidate this trace explains.
    pub candidate_id: String,
    /// Analysis the candidate was generated from.
    pub analysis_id: String,
    /// Project snapshot the analysis was taken from.
    pub snapshot_id: String,
    /// Knowledge-base version.
    pub knowledge_version: String,
    /// Style profile identifier.
    pub profile_id: String,
    /// Random seed used.
    pub seed: u64,
    /// Assumptions the generator had to make.
    pub assumptions: Vec<String>,
    /// Overall confidence, `0.0..=1.0`.
    pub confidence: f64,
    /// Score components and total.
    pub score: ScoreVector,
    /// Every rule that was consulted.
    pub rule_applications: Vec<RuleApplication>,
    /// Non-fatal findings.
    pub warnings: Vec<Warning>,
    /// Sources backing the decisions.
    pub source_ids: Vec<String>,
    /// Concise musical explanation, generated from the structured decisions.
    pub explanation: String,
    /// Ids of alternatives that were rejected.
    pub rejected_alternatives: Vec<String>,
}

impl DecisionTrace {
    /// JSON form, matching the shape in the product brief.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "candidate_id" => self.candidate_id.clone(),
            "analysis_id" => self.analysis_id.clone(),
            "snapshot_id" => self.snapshot_id.clone(),
            "knowledge_version" => self.knowledge_version.clone(),
            "profile_id" => self.profile_id.clone(),
            "seed" => self.seed as i64,
            "assumptions" => str_array(&self.assumptions),
            "confidence" => self.confidence,
            "score" => self.score.to_json(),
            "rule_applications" => Json::Arr(self.rule_applications.iter().map(|r| r.to_json()).collect()),
            "warnings" => Json::Arr(self.warnings.iter().map(|w| w.to_json()).collect()),
            "source_ids" => str_array(&self.source_ids),
            "explanation" => self.explanation.clone(),
            "rejected_alternatives" => str_array(&self.rejected_alternatives),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<DecisionTrace, DomainError> {
        let mut rule_applications = Vec::new();
        if let Some(Json::Arr(a)) = v.get("rule_applications") {
            for r in a {
                rule_applications.push(RuleApplication::from_json(r)?);
            }
        }
        let mut warnings = Vec::new();
        if let Some(Json::Arr(a)) = v.get("warnings") {
            for w in a {
                warnings.push(Warning::from_json(w)?);
            }
        }
        let score = match v.get("score") {
            Some(s @ Json::Obj(_)) => ScoreVector::from_json(s)?,
            _ => ScoreVector::new(),
        };
        Ok(DecisionTrace {
            candidate_id: v.opt_str_field("candidate_id")?.unwrap_or("").to_string(),
            analysis_id: v.opt_str_field("analysis_id")?.unwrap_or("").to_string(),
            snapshot_id: v.opt_str_field("snapshot_id")?.unwrap_or("").to_string(),
            knowledge_version: v
                .opt_str_field("knowledge_version")?
                .unwrap_or("")
                .to_string(),
            profile_id: v.opt_str_field("profile_id")?.unwrap_or("").to_string(),
            seed: v.opt_i64_field("seed")?.unwrap_or(0).max(0) as u64,
            assumptions: string_list(v, "assumptions"),
            confidence: v.opt_f64_field("confidence")?.unwrap_or(0.0),
            score,
            rule_applications,
            warnings,
            source_ids: string_list(v, "source_ids"),
            explanation: v.opt_str_field("explanation")?.unwrap_or("").to_string(),
            rejected_alternatives: string_list(v, "rejected_alternatives"),
        })
    }
}

/// What kind of thing a candidate proposes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CandidateKind {
    /// A new harmonization of a melody.
    Harmonization,
    /// A reharmonization of existing chords.
    Reharmonization,
    /// A voicing of existing chords.
    Voicing,
    /// A full arrangement.
    Arrangement,
}

impl CandidateKind {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            CandidateKind::Harmonization => "harmonization",
            CandidateKind::Reharmonization => "reharmonization",
            CandidateKind::Voicing => "voicing",
            CandidateKind::Arrangement => "arrangement",
        }
    }

    /// Parses the identifier produced by [`CandidateKind::id`].
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "harmonization" => Some(CandidateKind::Harmonization),
            "reharmonization" => Some(CandidateKind::Reharmonization),
            "voicing" => Some(CandidateKind::Voicing),
            "arrangement" => Some(CandidateKind::Arrangement),
            _ => None,
        }
    }

    /// Every variant, in declaration order.
    pub fn all() -> &'static [CandidateKind] {
        &[
            CandidateKind::Harmonization,
            CandidateKind::Reharmonization,
            CandidateKind::Voicing,
            CandidateKind::Arrangement,
        ]
    }
}

/// One generated instrumental part.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// Arrangement role.
    pub role: ArrangementRole,
    /// Display name.
    pub name: String,
    /// The notes of the part.
    pub notes: Vec<Note>,
    /// Instrument profile id from the knowledge base.
    pub instrument_profile: Option<String>,
    /// MIDI channel.
    pub channel: u8,
    /// True when the part may sound more than one note at a time.
    pub polyphonic: bool,
}

impl Part {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "role" => self.role.id(),
            "name" => self.name.clone(),
            "notes" => Json::Arr(self.notes.iter().map(|n| n.to_json()).collect()),
            "instrument_profile" => match &self.instrument_profile { Some(p) => Json::Str(p.clone()), None => Json::Null },
            "channel" => self.channel as i64,
            "polyphonic" => self.polyphonic,
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<Part, DomainError> {
        let r = v.str_field("role")?;
        let role = ArrangementRole::parse(r)
            .ok_or_else(|| DomainError::invalid_argument(format!("bad arrangement role {r:?}")))?;
        let mut notes = Vec::new();
        for n in v.arr_field("notes")? {
            notes.push(Note::from_json(n)?);
        }
        Ok(Part {
            role,
            name: v.opt_str_field("name")?.unwrap_or("").to_string(),
            notes,
            instrument_profile: v
                .opt_str_field("instrument_profile")?
                .map(|s| s.to_string()),
            channel: v.opt_i64_field("channel")?.unwrap_or(0).clamp(0, 15) as u8,
            polyphonic: v.opt_bool_field("polyphonic")?.unwrap_or(false),
        })
    }
}

/// What the loop audit found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoopReport {
    /// Declared loop intent.
    pub intent: Option<LoopIntent>,
    /// Loop start position.
    pub loop_start: BeatTime,
    /// Loop end position.
    pub loop_end: BeatTime,
    /// True when the material loops cleanly for its intent.
    pub compatible: bool,
    /// Loop compatibility score, `0.0..=1.0`.
    pub score: f64,
    /// How the harmony behaves across the wrap.
    pub harmonic_wrap: String,
    /// How the bass behaves across the wrap.
    pub bass_wrap: String,
    /// How the voice leading behaves across the wrap.
    pub voice_leading_wrap: String,
    /// Notes still sounding at the loop end.
    pub hanging_notes: Vec<NoteId>,
    /// Notes crossing the loop boundary.
    pub crossing_notes: Vec<NoteId>,
    /// Pickup length before the loop start.
    pub pickup_qn: BeatTime,
    /// Tail length after the loop end.
    pub tail_qn: BeatTime,
    /// Findings.
    pub findings: Vec<Warning>,
    /// Suggested repairs.
    pub repairs: Vec<String>,
    /// Confidence in the audit, `0.0..=1.0`.
    pub confidence: f64,
}

impl LoopReport {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "intent" => match self.intent { Some(i) => Json::Str(i.id().to_string()), None => Json::Null },
            "loop_start" => self.loop_start.to_json(),
            "loop_end" => self.loop_end.to_json(),
            "compatible" => self.compatible,
            "score" => self.score,
            "harmonic_wrap" => self.harmonic_wrap.clone(),
            "bass_wrap" => self.bass_wrap.clone(),
            "voice_leading_wrap" => self.voice_leading_wrap.clone(),
            "hanging_notes" => Json::Arr(self.hanging_notes.iter().map(|n| Json::Int(*n as i64)).collect()),
            "crossing_notes" => Json::Arr(self.crossing_notes.iter().map(|n| Json::Int(*n as i64)).collect()),
            "pickup_qn" => self.pickup_qn.to_json(),
            "tail_qn" => self.tail_qn.to_json(),
            "findings" => Json::Arr(self.findings.iter().map(|f| f.to_json()).collect()),
            "repairs" => str_array(&self.repairs),
            "confidence" => self.confidence,
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<LoopReport, DomainError> {
        let intent =
            match v.get("intent") {
                Some(Json::Str(s)) => Some(LoopIntent::parse(s).ok_or_else(|| {
                    DomainError::invalid_argument(format!("bad loop intent {s:?}"))
                })?),
                _ => None,
            };
        let mut findings = Vec::new();
        if let Some(Json::Arr(a)) = v.get("findings") {
            for f in a {
                findings.push(Warning::from_json(f)?);
            }
        }
        let ids = |key: &str| -> Vec<NoteId> {
            match v.get(key) {
                Some(Json::Arr(a)) => a
                    .iter()
                    .filter_map(|x| x.as_i64())
                    .map(|n| n.clamp(0, u32::MAX as i64) as NoteId)
                    .collect(),
                _ => Vec::new(),
            }
        };
        Ok(LoopReport {
            intent,
            loop_start: BeatTime::from_json(v.field("loop_start")?)?,
            loop_end: BeatTime::from_json(v.field("loop_end")?)?,
            compatible: v.opt_bool_field("compatible")?.unwrap_or(false),
            score: v.opt_f64_field("score")?.unwrap_or(0.0),
            harmonic_wrap: v.opt_str_field("harmonic_wrap")?.unwrap_or("").to_string(),
            bass_wrap: v.opt_str_field("bass_wrap")?.unwrap_or("").to_string(),
            voice_leading_wrap: v
                .opt_str_field("voice_leading_wrap")?
                .unwrap_or("")
                .to_string(),
            hanging_notes: ids("hanging_notes"),
            crossing_notes: ids("crossing_notes"),
            pickup_qn: BeatTime::from_json(v.field("pickup_qn")?)?,
            tail_qn: BeatTime::from_json(v.field("tail_qn")?)?,
            findings,
            repairs: string_list(v, "repairs"),
            confidence: v.opt_f64_field("confidence")?.unwrap_or(0.0),
        })
    }
}

/// One generated musical option, with everything needed to explain and stage it.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// Stable id.
    pub id: String,
    /// What it proposes.
    pub kind: CandidateKind,
    /// Short human name, e.g. `"Functional & cadence-directed"`.
    pub label: String,
    /// Stable strategy id, e.g. `"functional"`.
    pub strategy: String,
    /// Chords it proposes.
    pub chords: Vec<ChordEvent>,
    /// Parts it proposes.
    pub parts: Vec<Part>,
    /// Why it looks the way it does.
    pub trace: DecisionTrace,
    /// Loop audit, when one was run.
    pub loop_report: Option<LoopReport>,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// ISO-8601 expiry timestamp.
    pub expires_at: String,
}

impl Candidate {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "id" => self.id.clone(),
            "kind" => self.kind.id(),
            "label" => self.label.clone(),
            "strategy" => self.strategy.clone(),
            "chords" => Json::Arr(self.chords.iter().map(|c| c.to_json()).collect()),
            "parts" => Json::Arr(self.parts.iter().map(|p| p.to_json()).collect()),
            "trace" => self.trace.to_json(),
            "loop_report" => match &self.loop_report { Some(r) => r.to_json(), None => Json::Null },
            "created_at" => self.created_at.clone(),
            "expires_at" => self.expires_at.clone(),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<Candidate, DomainError> {
        let k = v.str_field("kind")?;
        let kind = CandidateKind::parse(k)
            .ok_or_else(|| DomainError::invalid_argument(format!("bad candidate kind {k:?}")))?;
        let mut chords = Vec::new();
        if let Some(Json::Arr(a)) = v.get("chords") {
            for c in a {
                chords.push(ChordEvent::from_json(c)?);
            }
        }
        let mut parts = Vec::new();
        if let Some(Json::Arr(a)) = v.get("parts") {
            for p in a {
                parts.push(Part::from_json(p)?);
            }
        }
        let loop_report = match v.get("loop_report") {
            Some(r @ Json::Obj(_)) => Some(LoopReport::from_json(r)?),
            _ => None,
        };
        let trace = match v.get("trace") {
            Some(t @ Json::Obj(_)) => DecisionTrace::from_json(t)?,
            _ => DecisionTrace::default(),
        };
        Ok(Candidate {
            id: v.str_field("id")?.to_string(),
            kind,
            label: v.opt_str_field("label")?.unwrap_or("").to_string(),
            strategy: v.opt_str_field("strategy")?.unwrap_or("").to_string(),
            chords,
            parts,
            trace,
            loop_report,
            created_at: v.opt_str_field("created_at")?.unwrap_or("").to_string(),
            expires_at: v.opt_str_field("expires_at")?.unwrap_or("").to_string(),
        })
    }

    /// SHA-256 of the canonical JSON form.
    pub fn hash_hex(&self) -> String {
        qjson::sha256::sha256_hex(self.to_json().to_canonical_string().as_bytes())
    }
}

/// Renders a string vector as a JSON array.
fn str_array(v: &[String]) -> Json {
    Json::Arr(v.iter().map(|s| Json::Str(s.clone())).collect())
}

/// Reads an optional array-of-strings field.
fn string_list(v: &Json, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Json::Arr(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol;

    fn trace() -> DecisionTrace {
        let mut score = ScoreVector::new();
        score.set("melody_fit", 0.96);
        score.set("voice_leading", 0.82);
        score.set_total(0.84);
        DecisionTrace {
            candidate_id: "cand-1".to_string(),
            analysis_id: "an-1".to_string(),
            snapshot_id: "snap-1".to_string(),
            knowledge_version: "1.0.0".to_string(),
            profile_id: "jazz_standard".to_string(),
            seed: 12345,
            assumptions: vec!["highest voice is the melody".to_string()],
            confidence: 0.87,
            score,
            rule_applications: vec![RuleApplication {
                rule_id: "vl.parallel_fifths".to_string(),
                status: RuleStatus::Applied,
                score_delta: -0.1,
                matched_conditions: vec!["outer voices".to_string()],
                matched_exceptions: vec![],
                source_ids: vec!["src.fux".to_string()],
                explanation: "parallel fifths between soprano and bass".to_string(),
            }],
            warnings: vec![Warning::new("wide_leap", "octave leap", Severity::Minor)],
            source_ids: vec!["src.levine".to_string()],
            explanation: "cadence-directed ii-V-I".to_string(),
            rejected_alternatives: vec!["cand-2".to_string()],
        }
    }

    #[test]
    fn score_components_are_the_documented_set() {
        assert_eq!(SCORE_COMPONENTS.len(), 13);
        assert!(SCORE_COMPONENTS.contains(&"melody_fit"));
        assert!(SCORE_COMPONENTS.contains(&"candidate_diversity"));
        let mut sorted = SCORE_COMPONENTS.to_vec();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len());
    }

    #[test]
    fn score_vector_keeps_insertion_order() {
        let mut s = ScoreVector::new();
        s.set("voice_leading", 0.5);
        s.set("melody_fit", 0.9);
        s.set("voice_leading", 0.7);
        let names: Vec<&str> = s.components().map(|(n, _)| n).collect();
        assert_eq!(names, ["voice_leading", "melody_fit"]);
        assert_eq!(s.get("voice_leading"), 0.7);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn score_vector_add_and_missing_components() {
        let mut s = ScoreVector::new();
        s.add("style_match", 0.25);
        s.add("style_match", 0.25);
        assert_eq!(s.get("style_match"), 0.5);
        assert_eq!(s.get("nothing"), 0.0);
        assert!(!s.contains("nothing"));
        assert!(s.contains("style_match"));
        assert!(!s.is_empty());
        assert!(ScoreVector::new().is_empty());
    }

    #[test]
    fn score_vector_keeps_raw_components_next_to_the_total() {
        let mut s = ScoreVector::new();
        s.set("melody_fit", 1.0);
        s.set("voice_leading", 0.0);
        let total = s.recompute_total(&[("melody_fit", 2.0), ("voice_leading", 1.0)]);
        assert!((total - 2.0 / 3.0).abs() < 1e-12);
        assert_eq!(s.total(), total);
        // The raw components survive the aggregation.
        assert_eq!(s.get("melody_fit"), 1.0);
        assert_eq!(s.get("voice_leading"), 0.0);
        s.set_total(0.5);
        assert_eq!(s.total(), 0.5);
        assert_eq!(ScoreVector::new().recompute_total(&[]), 0.0);
    }

    #[test]
    fn score_vector_json_round_trip() {
        let mut s = ScoreVector::new();
        s.set("melody_fit", 0.96);
        s.set("style_match", 0.88);
        s.set_total(0.9);
        let json = s.to_json();
        assert_eq!(json.f64_field("total").unwrap(), 0.9);
        assert_eq!(
            json.field("components")
                .unwrap()
                .f64_field("melody_fit")
                .unwrap(),
            0.96
        );
        let back = ScoreVector::from_json(&json).expect("round trip");
        assert_eq!(back, s);
        let names: Vec<&str> = back.components().map(|(n, _)| n).collect();
        assert_eq!(names, ["melody_fit", "style_match"]);
    }

    #[test]
    fn rule_status_and_severity_ids() {
        for s in [
            RuleStatus::Applied,
            RuleStatus::Bypassed,
            RuleStatus::NotApplicable,
            RuleStatus::Violated,
        ] {
            assert_eq!(RuleStatus::parse(s.id()), Some(s));
        }
        for s in [
            Severity::Info,
            Severity::Minor,
            Severity::Moderate,
            Severity::Major,
        ] {
            assert_eq!(Severity::parse(s.id()), Some(s));
        }
        assert!(RuleStatus::parse("nope").is_none());
        assert!(Severity::parse("nope").is_none());
    }

    #[test]
    fn rule_application_json_round_trip() {
        let t = trace();
        let r = &t.rule_applications[0];
        assert_eq!(&RuleApplication::from_json(&r.to_json()).unwrap(), r);
    }

    #[test]
    fn warning_json_round_trip() {
        let w = Warning::new("hanging_note", "note crosses the loop", Severity::Moderate);
        assert_eq!(Warning::from_json(&w.to_json()).unwrap(), w);
        let bad = json_obj! { "code" => "x", "severity" => "catastrophic" };
        assert!(Warning::from_json(&bad).is_err());
    }

    #[test]
    fn decision_trace_json_round_trip() {
        let t = trace();
        let back = DecisionTrace::from_json(&t.to_json()).expect("round trip");
        assert_eq!(back, t);
    }

    #[test]
    fn decision_trace_json_shape_matches_the_brief() {
        let t = trace();
        let j = t.to_json();
        assert_eq!(j.str_field("profile_id").unwrap(), "jazz_standard");
        assert_eq!(j.i64_field("seed").unwrap(), 12345);
        assert!(j.field("score").unwrap().get("components").is_some());
        assert!(j.field("rule_applications").unwrap().as_arr().is_some());
    }

    #[test]
    fn candidate_kind_ids() {
        for k in CandidateKind::all() {
            assert_eq!(CandidateKind::parse(k.id()), Some(*k));
        }
        assert!(CandidateKind::parse("nope").is_none());
    }

    #[test]
    fn part_json_round_trip() {
        let p = Part {
            role: ArrangementRole::Bass,
            name: "Bass".to_string(),
            notes: vec![Note::new(
                0,
                crate::pitch::SpelledPitch::parse("C2").unwrap(),
                BeatTime::ZERO,
                BeatTime::ONE,
            )],
            instrument_profile: Some("upright_bass".to_string()),
            channel: 1,
            polyphonic: false,
        };
        assert_eq!(Part::from_json(&p.to_json()).unwrap(), p);
        let bad = json_obj! { "role" => "wizard", "notes" => Json::Arr(vec![]) };
        assert!(Part::from_json(&bad).is_err());
    }

    #[test]
    fn loop_report_json_round_trip() {
        let r = LoopReport {
            intent: Some(LoopIntent::ClosedTonic),
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::from_quarters(16),
            compatible: true,
            score: 0.91,
            harmonic_wrap: "V-I across the wrap".to_string(),
            bass_wrap: "descending fifth".to_string(),
            voice_leading_wrap: "two common tones".to_string(),
            hanging_notes: vec![7],
            crossing_notes: vec![8, 9],
            pickup_qn: BeatTime::new(1, 2),
            tail_qn: BeatTime::ZERO,
            findings: vec![Warning::new("tail", "tail beyond loop", Severity::Info)],
            repairs: vec!["shorten the last note".to_string()],
            confidence: 0.8,
        };
        assert_eq!(LoopReport::from_json(&r.to_json()).unwrap(), r);
        assert_eq!(
            LoopReport::from_json(&LoopReport::default().to_json()).unwrap(),
            LoopReport::default()
        );
    }

    #[test]
    fn candidate_json_round_trip_and_hash() {
        let c = Candidate {
            id: "cand-1".to_string(),
            kind: CandidateKind::Harmonization,
            label: "Functional & cadence-directed".to_string(),
            strategy: "functional".to_string(),
            chords: vec![ChordEvent::new(
                0,
                symbol::parse("Dm7").unwrap(),
                BeatTime::ZERO,
                BeatTime::from_quarters(4),
            )],
            parts: vec![],
            trace: trace(),
            loop_report: None,
            created_at: "2026-07-26T00:00:00Z".to_string(),
            expires_at: "2026-07-26T01:00:00Z".to_string(),
        };
        let back = Candidate::from_json(&c.to_json()).expect("round trip");
        assert_eq!(back.id, c.id);
        assert_eq!(back.kind, c.kind);
        assert_eq!(back.strategy, c.strategy);
        assert_eq!(back.chords.len(), 1);
        assert_eq!(back.chords[0].spec, c.chords[0].spec);
        assert_eq!(back.trace, c.trace);
        assert_eq!(back.hash_hex(), c.hash_hex());
        assert_eq!(c.hash_hex().len(), 64);
    }

    #[test]
    fn candidate_json_rejects_a_bad_kind() {
        let bad = json_obj! { "id" => "x", "kind" => "sorcery" };
        assert!(Candidate::from_json(&bad).is_err());
    }
}
