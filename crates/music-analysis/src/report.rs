//! The full analysis report and the pipeline that produces it.
//!
//! [`analyze`] runs stages 1 to 4 in order and assembles an [`Analysis`], which
//! the MCP server caches and serves as `analysis://{id}`.
//!
//! # Determinism
//!
//! The same notes, the same [`AnalyzeParams`] and the same knowledge bundle
//! must produce byte-identical output, because that is what makes golden tests
//! — and reproducible harmonisation — possible. Three things follow:
//!
//! * the analysis **id** is derived with `uuid_from_name` over the snapshot id
//!   and the canonical JSON of the parameters, so the same request always
//!   yields the same id;
//! * nothing in the pipeline iterates a `HashMap`, and every score that reaches
//!   JSON is rounded to six decimals;
//! * [`Analysis::to_json`] deliberately **omits** `created_at` and
//!   `expires_at`. They are wall-clock lifecycle metadata, not analysis, and
//!   including them would make byte-equality impossible. They are reported by
//!   [`Analysis::summary_json`], which is the lifecycle view.

use crate::chord_detect::detect_chords;
use crate::error::AnalysisError;
use crate::grid::{build_grid, GridMode, HarmonicGrid};
use crate::key::{analyze_key_ranked, KeyAnalysis};
use crate::nct::classify_ncts_in_key;
use crate::phrase::{
    analyze_phrases, melody_profile, refine_cadences, MelodyProfile, PhraseAnalysis,
};
use crate::salience::{
    analyze_salience_with_threshold, SalienceReport, SalienceWeights, STRUCTURAL_THRESHOLD,
};
use crate::selection::{extract, Extraction, ExtractionRequest, AMBIGUOUS_MELODY};
use crate::util::{num, round6, warning_array};
use music_domain::ids;
use music_domain::prelude::*;
use qjson::time::{iso8601_from_unix, unix_now};
use qjson::{Json, JsonMap};
use std::collections::BTreeMap;
use theory_kb::KnowledgeBase;

/// How long a cached analysis stays valid, in seconds.
pub const ANALYSIS_TTL_SECONDS: i64 = 3600;

/// Warning raised when the leading key candidates are too close to choose
/// between.
pub const AMBIGUOUS_KEY: &str = "AMBIGUOUS_KEY";
/// Warning raised when chords were inferred from a single line.
pub const CHORDS_INFERRED_FROM_MELODY: &str = "CHORDS_INFERRED_FROM_MELODY";
/// Warning raised when a note sounds past the end of the loop.
pub const HANGING_NOTE: &str = "HANGING_NOTE";
/// Warning raised when a note begins before the loop starts.
pub const PICKUP_BEFORE_LOOP: &str = "PICKUP_BEFORE_LOOP";
/// Warning raised when a note sounds across the loop start.
pub const NOTE_CROSSES_LOOP_START: &str = "NOTE_CROSSES_LOOP_START";
/// Warning raised when the grid does not line up with the loop boundary.
pub const GRID_NOT_LOOP_ALIGNED: &str = "GRID_NOT_LOOP_ALIGNED";

/// How much benefit of the doubt the analysis gives itself.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Strictness {
    /// Keep every plausible reading; a wide key field and many hypotheses.
    Exploratory,
    /// The default.
    #[default]
    Balanced,
    /// Fewer readings, a higher bar for calling a note structural.
    Conservative,
    /// One reading wherever the theory admits one.
    CommonPracticeStrict,
}

impl Strictness {
    /// Stable identifier.
    pub fn id(self) -> &'static str {
        match self {
            Strictness::Exploratory => "exploratory",
            Strictness::Balanced => "balanced",
            Strictness::Conservative => "conservative",
            Strictness::CommonPracticeStrict => "common_practice_strict",
        }
    }

    /// Reads the wire form.
    pub fn parse(s: &str) -> Option<Strictness> {
        Strictness::all().iter().copied().find(|k| k.id() == s)
    }

    /// Every setting, in increasing order of strictness.
    pub fn all() -> &'static [Strictness] {
        &[
            Strictness::Exploratory,
            Strictness::Balanced,
            Strictness::Conservative,
            Strictness::CommonPracticeStrict,
        ]
    }

    /// The salience a note must reach to count as structural.
    pub fn salience_threshold(self) -> f64 {
        match self {
            Strictness::Exploratory => 0.45,
            Strictness::Balanced => STRUCTURAL_THRESHOLD,
            Strictness::Conservative => 0.60,
            Strictness::CommonPracticeStrict => 0.65,
        }
    }

    /// How many key hypotheses are reported.
    pub fn max_key_candidates(self) -> usize {
        match self {
            Strictness::Exploratory => 12,
            Strictness::Balanced => 8,
            Strictness::Conservative => 6,
            Strictness::CommonPracticeStrict => 5,
        }
    }

    /// How many readings one note may carry.
    pub fn max_nct_hypotheses(self) -> usize {
        match self {
            Strictness::Exploratory => 4,
            Strictness::Balanced => 3,
            Strictness::Conservative => 2,
            Strictness::CommonPracticeStrict => 1,
        }
    }
}

/// Everything the caller controls.
#[derive(Clone, Debug, PartialEq)]
pub struct AnalyzeParams {
    /// Style profile id, e.g. `"jazz_standard"`.
    pub profile_id: String,
    /// How to take the melody out of the selection.
    pub extraction: ExtractionRequest,
    /// A declared tonal centre as `(tonic, scale id)`, overriding inference.
    pub tonal_center: Option<(String, String)>,
    /// A meter override, when the host's map is wrong or absent.
    pub meter: Option<TimeSignature>,
    /// How to choose the harmonic grid.
    pub grid: GridMode,
    /// How much benefit of the doubt to give.
    pub strictness: Strictness,
    /// The loop region to audit against, when there is one.
    pub loop_span: Option<(BeatTime, BeatTime)>,
}

impl Default for AnalyzeParams {
    fn default() -> Self {
        AnalyzeParams {
            profile_id: "common_practice".to_string(),
            extraction: ExtractionRequest::auto(),
            tonal_center: None,
            meter: None,
            grid: GridMode::Auto,
            strictness: Strictness::Balanced,
            loop_span: None,
        }
    }
}

impl AnalyzeParams {
    /// Canonical JSON form. This is what the analysis id is derived from, so
    /// every field that changes the output must appear here.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("profile_id", Json::Str(self.profile_id.clone()));
        m.insert("extraction", self.extraction.to_json());
        m.insert(
            "tonal_center",
            match &self.tonal_center {
                Some((t, s)) => Json::Arr(vec![Json::Str(t.clone()), Json::Str(s.clone())]),
                None => Json::Null,
            },
        );
        m.insert(
            "meter",
            match self.meter {
                Some(s) => Json::Str(s.to_display()),
                None => Json::Null,
            },
        );
        m.insert("grid", self.grid.to_json());
        m.insert("strictness", Json::Str(self.strictness.id().to_string()));
        m.insert(
            "loop_span",
            match self.loop_span {
                Some((a, b)) => Json::Arr(vec![a.to_json(), b.to_json()]),
                None => Json::Null,
            },
        );
        Json::Obj(m)
    }

    /// The canonical string the analysis id hashes.
    pub fn canonical(&self) -> String {
        self.to_json().to_canonical_string()
    }
}

/// The complete result of stages 1 to 4.
#[derive(Clone, Debug)]
pub struct Analysis {
    /// Content-derived id: the same request always yields the same value.
    pub id: String,
    /// The snapshot this analysis was taken from.
    pub snapshot_id: String,
    /// The style profile used.
    pub profile_id: String,
    /// Stage 1.
    pub extraction: Extraction,
    /// Stage 2a.
    pub phrases: PhraseAnalysis,
    /// Stage 2b.
    pub salience: SalienceReport,
    /// Melodic statistics.
    pub melody: MelodyProfile,
    /// Stage 3.
    pub key: KeyAnalysis,
    /// Stage 4.
    pub grid: HarmonicGrid,
    /// Non-chord-tone hypotheses, by note id.
    pub ncts: BTreeMap<NoteId, Vec<NctHypothesis>>,
    /// Chords detected from the material.
    pub detected_chords: Vec<ChordEvent>,
    /// Findings about the loop region, when one was supplied.
    pub loop_observations: Vec<Warning>,
    /// Overall confidence, `0.0..=1.0`.
    pub confidence: f64,
    /// Every finding, including the extraction's.
    pub warnings: Vec<Warning>,
    /// The knowledge bundle version this was produced against.
    pub knowledge_version: String,
    /// Creation time, ISO-8601 UTC.
    pub created_at: String,
    /// Expiry time, ISO-8601 UTC.
    pub expires_at: String,
}

impl Analysis {
    /// The full analysis document.
    ///
    /// Deterministic: it carries no wall-clock time. See the module docs.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("snapshot_id", Json::Str(self.snapshot_id.clone()));
        m.insert("profile_id", Json::Str(self.profile_id.clone()));
        m.insert(
            "knowledge_version",
            Json::Str(self.knowledge_version.clone()),
        );
        m.insert("confidence", num(self.confidence));
        m.insert("extraction", self.extraction.to_json());
        m.insert("phrases", self.phrases.to_json());
        m.insert("salience", self.salience.to_json());
        m.insert("melody", self.melody.to_json());
        m.insert("key", self.key.to_json());
        m.insert("grid", self.grid.to_json());
        let mut ncts = JsonMap::new();
        for (id, v) in &self.ncts {
            ncts.insert(
                id.to_string(),
                Json::Arr(v.iter().map(NctHypothesis::to_json).collect()),
            );
        }
        m.insert("ncts", Json::Obj(ncts));
        m.insert(
            "detected_chords",
            Json::Arr(
                self.detected_chords
                    .iter()
                    .map(ChordEvent::to_json)
                    .collect(),
            ),
        );
        m.insert("loop_observations", warning_array(&self.loop_observations));
        m.insert("warnings", warning_array(&self.warnings));
        Json::Obj(m)
    }

    /// The short lifecycle view, which is where the timestamps live.
    pub fn summary_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("snapshot_id", Json::Str(self.snapshot_id.clone()));
        m.insert("profile_id", Json::Str(self.profile_id.clone()));
        m.insert(
            "knowledge_version",
            Json::Str(self.knowledge_version.clone()),
        );
        m.insert("created_at", Json::Str(self.created_at.clone()));
        m.insert("expires_at", Json::Str(self.expires_at.clone()));
        m.insert("confidence", num(self.confidence));
        m.insert(
            "key",
            match self.key.top() {
                Some(c) => {
                    let mut k = JsonMap::new();
                    k.insert("label", Json::Str(c.label()));
                    k.insert("scale_id", Json::Str(c.scale_id.clone()));
                    k.insert("confidence", num(c.confidence));
                    k.insert("is_modal", Json::Bool(c.is_modal));
                    k.insert("ambiguous", Json::Bool(self.key.ambiguous));
                    Json::Obj(k)
                }
                None => Json::Null,
            },
        );
        m.insert(
            "extraction_mode",
            Json::Str(self.extraction.mode_used.id().to_string()),
        );
        m.insert(
            "melody_notes",
            Json::Int(self.extraction.melody.len() as i64),
        );
        m.insert("phrase_count", Json::Int(self.phrases.phrases.len() as i64));
        m.insert("motive_count", Json::Int(self.phrases.motives.len() as i64));
        m.insert("slot_count", Json::Int(self.grid.slots.len() as i64));
        m.insert(
            "detected_chord_count",
            Json::Int(self.detected_chords.len() as i64),
        );
        m.insert("warnings", warning_array(&self.warnings));
        Json::Obj(m)
    }

    /// True when a warning with `code` is present.
    pub fn has_warning(&self, code: &str) -> bool {
        self.warnings.iter().any(|w| w.code == code)
            || self.loop_observations.iter().any(|w| w.code == code)
    }

    /// The hypotheses for one note.
    pub fn nct(&self, id: NoteId) -> &[NctHypothesis] {
        self.ncts.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Runs the analysis pipeline.
///
/// # Errors
///
/// Propagates [`crate::selection::extract`]'s errors, and returns
/// `KNOWLEDGE_INVALID` when `p.profile_id` names a profile the bundle does not
/// carry.
pub fn analyze(
    kb: &KnowledgeBase,
    snapshot_id: &str,
    notes: &NoteSet,
    p: &AnalyzeParams,
) -> Result<Analysis, AnalysisError> {
    let profile = kb.resolve_profile(&p.profile_id)?;

    // Stage 1.
    let extraction = extract(notes, &p.extraction)?;

    // The time map, with the caller's meter override applied.
    let tm = match p.meter {
        Some(sig) => TimeMap::new(
            notes.time_map.tempos.clone(),
            vec![MeterEvent {
                qn: BeatTime::ZERO,
                sig,
                measure: 0,
            }],
        ),
        None => notes.time_map.clone(),
    };

    // The line stages 2 and 4 describe. When the caller asked for no melody at
    // all, the skyline stands in so the material is still segmented.
    let line = if extraction.melody.is_empty() {
        let ids: Vec<NoteId> = notes.highest_line().iter().map(|n| n.id).collect();
        let picked: Vec<Note> = notes
            .notes
            .iter()
            .filter(|n| ids.contains(&n.id))
            .cloned()
            .collect();
        NoteSet::sorted(picked, tm.clone())
    } else {
        let mut l = extraction.melody.clone();
        l.time_map = tm.clone();
        l
    };

    // Stage 2.
    let mut phrases = analyze_phrases(&line, &tm);
    let melody = melody_profile(&line);

    // Stage 3 sees everything, because the bass and the verticals are evidence.
    let hint = p
        .tonal_center
        .as_ref()
        .map(|(t, s)| (t.as_str(), s.as_str()));
    let key = analyze_key_ranked(
        kb,
        notes,
        &tm,
        hint,
        None,
        p.strictness.max_key_candidates(),
    );
    let scale_pcs = key.top_pcs(kb);
    if let Some(top) = key.top() {
        refine_cadences(&mut phrases, &line, top.tonic_pc(), &scale_pcs, &tm);
    }

    // Stage 2b needs the cadences stage 3 supplied.
    let salience = analyze_salience_with_threshold(
        &line,
        &phrases,
        &tm,
        &SalienceWeights::default(),
        p.strictness.salience_threshold(),
    );

    // Stage 4.
    let grid = build_grid(&line, &tm, &phrases, &salience, p.grid.clone(), &profile);

    let detected_chords = detect_chords(notes, &grid, kb, &key);
    let ncts = classify_ncts_in_key(
        &line,
        &detected_chords,
        &tm,
        kb,
        &profile,
        &scale_pcs,
        p.strictness.max_nct_hypotheses(),
    );

    // Findings.
    let mut warnings = extraction.warnings.clone();
    if key.ambiguous && !key.candidates.is_empty() {
        warnings.push(Warning::new(
            AMBIGUOUS_KEY,
            format!(
                "the leading key candidates are within {:.3} of each other; the ranking is a \
                 preference, not a determination",
                key.gap
            ),
            Severity::Moderate,
        ));
    }
    if notes.max_polyphony() <= 1 && !detected_chords.is_empty() {
        warnings.push(Warning::new(
            CHORDS_INFERRED_FROM_MELODY,
            "the material is a single line, so the detected chords are what the melody implies \
             rather than harmony that was played",
            Severity::Info,
        ));
    }
    let loop_observations = audit_loop(notes, &grid, p.loop_span);

    let confidence = round6(overall_confidence(&extraction, &key, &phrases));

    let now = unix_now();
    Ok(Analysis {
        id: ids::analysis_id_from(&format!("{snapshot_id}|{}", p.canonical())),
        snapshot_id: snapshot_id.to_string(),
        profile_id: profile.id.clone(),
        extraction,
        phrases,
        salience,
        melody,
        key,
        grid,
        ncts,
        detected_chords,
        loop_observations,
        confidence,
        warnings,
        knowledge_version: kb.version().to_string(),
        created_at: iso8601_from_unix(now),
        expires_at: iso8601_from_unix(now + ANALYSIS_TTL_SECONDS),
    })
}

/// Blends the stage confidences into one number.
///
/// Extraction dominates: if we are not sure what the melody is, nothing built
/// on top of it deserves to look certain.
fn overall_confidence(e: &Extraction, k: &KeyAnalysis, ph: &PhraseAnalysis) -> f64 {
    let key_conf = k.top().map(|c| c.confidence).unwrap_or(0.0);
    let phrase_conf = if ph.phrases.is_empty() {
        0.5
    } else {
        ph.phrases.iter().map(|p| p.confidence).sum::<f64>() / ph.phrases.len() as f64
    };
    let blended = 0.5 * e.confidence + 0.3 * key_conf + 0.2 * phrase_conf;
    if e.has_warning(AMBIGUOUS_MELODY) {
        (blended * 0.8).clamp(0.0, 1.0)
    } else {
        blended.clamp(0.0, 1.0)
    }
}

/// Findings about the loop region.
fn audit_loop(
    notes: &NoteSet,
    grid: &HarmonicGrid,
    span: Option<(BeatTime, BeatTime)>,
) -> Vec<Warning> {
    let Some((start, end)) = span else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let hanging: Vec<NoteId> = notes
        .notes
        .iter()
        .filter(|n| n.onset < end && n.end() > end)
        .map(|n| n.id)
        .collect();
    if !hanging.is_empty() {
        out.push(Warning::new(
            HANGING_NOTE,
            format!(
                "{} note(s) sound past the loop end at {}: {}",
                hanging.len(),
                end.to_display(),
                join_ids(&hanging)
            ),
            Severity::Moderate,
        ));
    }
    let before: Vec<NoteId> = notes
        .notes
        .iter()
        .filter(|n| n.onset < start)
        .map(|n| n.id)
        .collect();
    if !before.is_empty() {
        out.push(Warning::new(
            PICKUP_BEFORE_LOOP,
            format!(
                "{} note(s) begin before the loop start at {}: {}",
                before.len(),
                start.to_display(),
                join_ids(&before)
            ),
            Severity::Info,
        ));
    }
    let crossing: Vec<NoteId> = notes
        .notes
        .iter()
        .filter(|n| n.onset < start && n.end() > start)
        .map(|n| n.id)
        .collect();
    if !crossing.is_empty() {
        out.push(Warning::new(
            NOTE_CROSSES_LOOP_START,
            format!(
                "{} note(s) sound across the loop start: {}",
                crossing.len(),
                join_ids(&crossing)
            ),
            Severity::Minor,
        ));
    }
    if let Some(last) = grid.slots.last() {
        if last.end != end && !grid.slots.is_empty() {
            out.push(Warning::new(
                GRID_NOT_LOOP_ALIGNED,
                format!(
                    "the harmonic grid ends at {} but the loop ends at {}",
                    last.end.to_display(),
                    end.to_display()
                ),
                Severity::Info,
            ));
        }
    }
    out
}

/// `"0, 3, 7"`.
fn join_ids(ids: &[NoteId]) -> String {
    ids.iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::ExtractionMode;
    use crate::testing::note_set;

    fn kb() -> &'static KnowledgeBase {
        KnowledgeBase::embedded()
    }

    fn eight_bar() -> NoteSet {
        note_set(&[
            ("C4", "0", "1"),
            ("E4", "1", "1"),
            ("G4", "2", "1"),
            ("E4", "3", "1"),
            ("F4", "4", "1"),
            ("A4", "5", "1"),
            ("G4", "6", "2"),
            ("D4", "8", "1"),
            ("F4", "9", "1"),
            ("A4", "10", "1"),
            ("F4", "11", "1"),
            ("E4", "12", "1"),
            ("G4", "13", "1"),
            ("C5", "14", "2"),
            ("B4", "16", "1"),
            ("A4", "17", "1"),
            ("G4", "18", "1"),
            ("F4", "19", "1"),
            ("E4", "20", "1"),
            ("D4", "21", "1"),
            ("C4", "22", "2"),
        ])
    }

    fn run(ns: &NoteSet, p: &AnalyzeParams) -> Analysis {
        analyze(kb(), "snapshot-1", ns, p).expect("analysis succeeds")
    }

    #[test]
    fn strictness_ids_round_trip() {
        for s in Strictness::all() {
            assert_eq!(Strictness::parse(s.id()), Some(*s));
        }
        assert_eq!(Strictness::parse("nope"), None);
        assert_eq!(Strictness::default(), Strictness::Balanced);
    }

    #[test]
    fn strictness_orders_the_thresholds() {
        let all = Strictness::all();
        for w in all.windows(2) {
            assert!(w[0].salience_threshold() <= w[1].salience_threshold());
            assert!(w[0].max_key_candidates() >= w[1].max_key_candidates());
            assert!(w[0].max_nct_hypotheses() >= w[1].max_nct_hypotheses());
        }
    }

    #[test]
    fn a_full_analysis_fills_in_every_stage() {
        let a = run(&eight_bar(), &AnalyzeParams::default());
        assert!(!a.extraction.melody.is_empty());
        assert!(!a.phrases.phrases.is_empty());
        assert!(!a.salience.per_note.is_empty());
        assert!(!a.key.candidates.is_empty());
        assert!(!a.grid.slots.is_empty());
        assert!(!a.ncts.is_empty());
        assert_eq!(a.knowledge_version, kb().version());
    }

    #[test]
    fn the_analysis_id_is_content_derived_and_stable() {
        let ns = eight_bar();
        let p = AnalyzeParams::default();
        let a = run(&ns, &p);
        let b = run(&ns, &p);
        assert_eq!(a.id, b.id);
        assert!(ids::is_uuid_shaped(&a.id), "{}", a.id);
    }

    #[test]
    fn different_params_yield_a_different_id() {
        let ns = eight_bar();
        let a = run(&ns, &AnalyzeParams::default());
        let b = run(
            &ns,
            &AnalyzeParams {
                grid: GridMode::Bars(2.0),
                ..AnalyzeParams::default()
            },
        );
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn a_different_snapshot_yields_a_different_id() {
        let ns = eight_bar();
        let p = AnalyzeParams::default();
        let a = analyze(kb(), "snapshot-1", &ns, &p).expect("ok");
        let b = analyze(kb(), "snapshot-2", &ns, &p).expect("ok");
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn analysis_json_is_byte_identical_across_runs() {
        let ns = eight_bar();
        let p = AnalyzeParams::default();
        let a = run(&ns, &p).to_json().to_canonical_string();
        let b = run(&ns, &p).to_json().to_canonical_string();
        assert_eq!(a, b);
    }

    #[test]
    fn analysis_json_carries_no_wall_clock() {
        let a = run(&eight_bar(), &AnalyzeParams::default());
        let text = a.to_json().to_canonical_string();
        assert!(!text.contains("created_at"));
        assert!(!text.contains("expires_at"));
        assert!(a.summary_json().get("created_at").is_some());
        assert!(a.summary_json().get("expires_at").is_some());
    }

    #[test]
    fn an_unknown_profile_is_rejected() {
        let e = analyze(
            kb(),
            "s",
            &eight_bar(),
            &AnalyzeParams {
                profile_id: "no_such_profile".to_string(),
                ..AnalyzeParams::default()
            },
        );
        assert_eq!(e.unwrap_err().code, "KNOWLEDGE_INVALID");
    }

    #[test]
    fn an_empty_selection_is_rejected() {
        let e = analyze(kb(), "s", &NoteSet::default(), &AnalyzeParams::default());
        assert_eq!(e.unwrap_err().code, "NO_MIDI_SOURCE");
    }

    #[test]
    fn strictness_changes_the_structural_set_and_the_field() {
        let ns = eight_bar();
        let loose = run(
            &ns,
            &AnalyzeParams {
                strictness: Strictness::Exploratory,
                ..AnalyzeParams::default()
            },
        );
        let tight = run(
            &ns,
            &AnalyzeParams {
                strictness: Strictness::CommonPracticeStrict,
                ..AnalyzeParams::default()
            },
        );
        assert!(loose.salience.structural.len() >= tight.salience.structural.len());
        assert!(loose.key.candidates.len() > tight.key.candidates.len());
        assert!(tight.ncts.values().all(|v| v.len() <= 1));
    }

    #[test]
    fn a_tonal_centre_override_leads_the_field() {
        let a = run(
            &eight_bar(),
            &AnalyzeParams {
                tonal_center: Some(("A".to_string(), "aeolian".to_string())),
                ..AnalyzeParams::default()
            },
        );
        let top = a.key.top().expect("a candidate");
        assert_eq!(top.scale_id, "aeolian");
        assert_eq!(top.tonic.0, Letter::A);
    }

    #[test]
    fn a_meter_override_changes_the_metric_reading() {
        let ns = eight_bar();
        let four = run(&ns, &AnalyzeParams::default());
        let three = run(
            &ns,
            &AnalyzeParams {
                meter: Some(TimeSignature::new(3, 4)),
                ..AnalyzeParams::default()
            },
        );
        assert_ne!(
            four.salience.to_json().to_canonical_string(),
            three.salience.to_json().to_canonical_string()
        );
    }

    #[test]
    fn the_grid_mode_reaches_the_report() {
        let a = run(
            &eight_bar(),
            &AnalyzeParams {
                grid: GridMode::Beats(1.0),
                ..AnalyzeParams::default()
            },
        );
        assert_eq!(a.grid.mode_used, GridMode::Beats(1.0));
    }

    #[test]
    fn a_monophonic_analysis_says_the_chords_were_inferred() {
        let a = run(&eight_bar(), &AnalyzeParams::default());
        assert!(a.has_warning(CHORDS_INFERRED_FROM_MELODY));
    }

    #[test]
    fn polyphonic_input_lowers_the_overall_confidence() {
        let mono = run(&eight_bar(), &AnalyzeParams::default());
        let poly = note_set(&[
            ("E5", "0", "1"),
            ("C4", "0", "2"),
            ("F5", "1", "1"),
            ("B3", "2", "2"),
            ("G5", "2", "2"),
            ("A3", "4", "2"),
            ("F5", "4", "2"),
        ]);
        let p = run(&poly, &AnalyzeParams::default());
        assert!(p.has_warning(AMBIGUOUS_MELODY));
        assert!(p.confidence < mono.confidence);
    }

    #[test]
    fn the_loop_audit_finds_a_hanging_note() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("E4", "1", "1"),
            ("G4", "2", "1"),
            ("C5", "3", "3"),
        ]);
        let a = run(
            &ns,
            &AnalyzeParams {
                loop_span: Some((BeatTime::ZERO, BeatTime::from_quarters(4))),
                ..AnalyzeParams::default()
            },
        );
        assert!(a.has_warning(HANGING_NOTE));
    }

    #[test]
    fn the_loop_audit_finds_a_pickup_before_the_loop() {
        let ns = note_set(&[
            ("G3", "-1", "1"),
            ("C4", "0", "2"),
            ("E4", "2", "2"),
            ("G4", "4", "4"),
        ]);
        let a = run(
            &ns,
            &AnalyzeParams {
                loop_span: Some((BeatTime::ZERO, BeatTime::from_quarters(8))),
                ..AnalyzeParams::default()
            },
        );
        assert!(a.has_warning(PICKUP_BEFORE_LOOP));
    }

    #[test]
    fn no_loop_span_means_no_loop_observations() {
        let a = run(&eight_bar(), &AnalyzeParams::default());
        assert!(a.loop_observations.is_empty());
    }

    #[test]
    fn all_notes_as_harmony_still_produces_a_grid_and_chords() {
        let poly = note_set(&[
            ("C3", "0", "4"),
            ("E4", "0", "4"),
            ("G4", "0", "4"),
            ("F3", "4", "4"),
            ("A4", "4", "4"),
            ("C5", "4", "4"),
        ]);
        let a = run(
            &poly,
            &AnalyzeParams {
                extraction: ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony),
                ..AnalyzeParams::default()
            },
        );
        assert!(a.extraction.melody.is_empty());
        assert!(!a.grid.slots.is_empty());
        assert_eq!(a.detected_chords.len(), 2);
    }

    #[test]
    fn the_summary_is_a_short_view_of_the_same_analysis() {
        let a = run(&eight_bar(), &AnalyzeParams::default());
        let s = a.summary_json();
        assert_eq!(s.get("id").and_then(Json::as_str), Some(a.id.as_str()));
        assert!(s.get("key").and_then(Json::as_obj).is_some());
        assert!(s.get("phrase_count").and_then(Json::as_i64).unwrap() > 0);
        assert!(s.to_string().len() < a.to_json().to_string().len());
    }

    #[test]
    fn params_json_is_canonical_and_complete() {
        let p = AnalyzeParams::default();
        let j = p.to_json();
        for key in [
            "profile_id",
            "extraction",
            "tonal_center",
            "meter",
            "grid",
            "strictness",
            "loop_span",
        ] {
            assert!(j.get(key).is_some(), "{key} is missing from the params");
        }
        assert_eq!(p.canonical(), p.to_json().to_canonical_string());
    }

    #[test]
    fn every_melody_note_receives_an_nct_entry() {
        let a = run(&eight_bar(), &AnalyzeParams::default());
        for n in &a.extraction.melody.notes {
            assert!(!a.nct(n.id).is_empty(), "note {} has no reading", n.id);
        }
    }

    #[test]
    fn a_phrase_ending_note_is_structural() {
        let a = run(&eight_bar(), &AnalyzeParams::default());
        let last = a
            .phrases
            .phrases
            .last()
            .and_then(|p| p.notes.last())
            .copied()
            .expect("a phrase-final note");
        assert!(
            a.salience.is_structural(last),
            "the phrase-final note scored {}",
            a.salience.total(last)
        );
    }

    #[test]
    fn the_profile_reaches_the_grid() {
        let ns = eight_bar();
        let modal = run(
            &ns,
            &AnalyzeParams {
                profile_id: "modal_ambient".to_string(),
                ..AnalyzeParams::default()
            },
        );
        let jazz = run(
            &ns,
            &AnalyzeParams {
                profile_id: "jazz_standard".to_string(),
                ..AnalyzeParams::default()
            },
        );
        assert_eq!(modal.profile_id, "modal_ambient");
        assert!(modal.grid.slots.len() <= jazz.grid.slots.len());
    }
}
