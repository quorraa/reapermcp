//! `music-domain` — the shared musical vocabulary of the QLabs REAPER Music
//! Intelligence MCP server.
//!
//! Every other crate in the workspace speaks these types. The design rules that
//! matter most:
//!
//! * **Spelling is data.** `G#` and `A♭` sound alike and are different values,
//!   because they imply different harmonic roles and resolutions.
//! * **Time is exact.** [`time::BeatTime`] is a normalized rational number of
//!   quarter notes with exact `Eq`, `Ord` and `Hash`, so loop boundaries and
//!   chord spans compare exactly instead of nearly.
//! * **Chords are semantic.** A [`chord::ChordSpec`] records root, triad,
//!   seventh, extensions, additions, alterations, omissions and bass — never a
//!   bare pitch-class set — so `Cadd9` and `C9`, `C6` and `C13`, `Csus4` and
//!   `C11` stay distinct.
//! * **Determinism.** Nothing here iterates a `HashMap`; every serialization is
//!   insertion-ordered, and content hashes go through
//!   `qjson::Json::to_canonical_string`.
//!
//! # Modules
//!
//! | Module | Purpose |
//! |---|---|
//! | [`pitch`] | letters, accidentals, spelled pitches, spelling contexts |
//! | [`interval`] | diatonic intervals with quality derived from spelling |
//! | [`time`] | [`time::BeatTime`], time signatures, the tempo/meter map |
//! | [`scale`] | scale definitions and rooted scale instances |
//! | [`chord`] | the semantic chord model, voicings, chord events |
//! | [`symbol`] | the chord-symbol parser and its alias table |
//! | [`note`] | notes, voices, roles, NCT hypotheses, note sets |
//! | [`structure`] | phrases, motives, cadences, regions, sections, roles |
//! | [`candidate`] | score vectors, decision traces, candidates, loop reports |
//! | [`plan`] | edit plans, operations, preconditions, expected outputs |
//! | [`fixture`] | the REAPER-free fixture format loader |
//! | [`ids`] | identifier helpers built on `qjson::uuid` |
//! | [`error`] | [`error::DomainError`] and its stable codes |
//!
//! # Example
//!
//! ```
//! use music_domain::prelude::*;
//!
//! let spec = symbol::parse("Cmaj7").expect("a chord symbol");
//! assert_eq!(spec.pitch_classes(), vec![0, 4, 7, 11]);
//! assert_eq!(spec.render_ascii(), "Cmaj7");
//!
//! let c4 = SpelledPitch::parse("C4").expect("a pitch");
//! assert_eq!(c4.transpose(Interval::M3).to_ascii(), "E4");
//!
//! let bar = BeatTime::new(1, 3) + BeatTime::new(2, 3);
//! assert_eq!(bar, BeatTime::ONE);
//! ```

#![warn(missing_docs)]

pub mod candidate;
pub mod chord;
pub mod error;
pub mod fixture;
pub mod ids;
pub mod interval;
pub mod note;
pub mod pitch;
pub mod plan;
pub mod scale;
pub mod structure;
pub mod symbol;
pub mod time;

pub use error::DomainError;

/// The types most callers want in scope.
pub mod prelude {
    pub use crate::candidate::{
        Candidate, CandidateKind, DecisionTrace, LoopReport, Part, RuleApplication, RuleStatus,
        ScoreVector, Severity, Warning, SCORE_COMPONENTS,
    };
    pub use crate::chord::{
        ChordDegree, ChordEvent, ChordSpec, HarmonicFunction, SeventhQuality, TriadQuality,
        Voicing, VoicingFamily,
    };
    pub use crate::error::DomainError;
    pub use crate::fixture::Fixture;
    pub use crate::interval::{Interval, IntervalQuality, SignedInterval};
    pub use crate::note::{
        NctHypothesis, NctKind, Note, NoteId, NoteOrigin, NoteRole, NoteSet, VoiceId,
        VOICE_UNASSIGNED,
    };
    pub use crate::pitch::{Accidental, Letter, SpelledPitch, SpellingContext};
    pub use crate::plan::{EditOperation, EditPlan, ExpectedOutput, PlannedNote, Precondition};
    pub use crate::scale::{ScaleDef, ScaleInstance};
    pub use crate::structure::{
        ArrangementRole, CadenceKind, HarmonicRegion, KeyRegion, LoopIntent, MotionKind, Motive,
        MotiveOccurrence, MotiveTransform, Phrase, RelativeMotion, Section, VoiceLeadingConnection,
    };
    pub use crate::symbol;
    pub use crate::time::{BeatTime, MeterEvent, TempoEvent, TimeMap, TimeSignature, PPQ};
}

#[cfg(test)]
mod contract_surface {
    //! Compile-time assertions that the frozen cross-crate API exists with the
    //! signatures the other workspace crates are written against. If one of
    //! these stops compiling, a downstream crate has just been broken.

    use crate::pitch::Letter;
    use crate::prelude::*;
    use qjson::Json;

    /// The return type of [`ChordSpec::chord_tones`], named so the signature
    /// assertion below stays readable.
    type ChordTones = Vec<(ChordDegree, (Letter, Accidental))>;

    #[test]
    fn pitch_and_interval_signatures() {
        let _: fn(char) -> Option<Letter> = Letter::from_char;
        let _: fn(Letter) -> char = Letter::as_char;
        let _: fn(Letter) -> i32 = Letter::natural_pc;
        let _: fn(Letter) -> i32 = Letter::diatonic_index;
        let _: fn(i32) -> Letter = Letter::from_diatonic_index;
        let _: fn(Letter, i32) -> (Letter, i32) = Letter::step;

        let _: fn(Accidental) -> String = Accidental::ascii;
        let _: fn(Accidental) -> String = Accidental::unicode;
        let _: fn(Accidental) -> bool = Accidental::is_common;

        let _: fn(Letter, Accidental, i32) -> SpelledPitch = SpelledPitch::new;
        let _: fn(SpelledPitch) -> i32 = SpelledPitch::midi;
        let _: fn(SpelledPitch) -> i32 = SpelledPitch::pitch_class;
        let _: fn(SpelledPitch) -> i32 = SpelledPitch::diatonic_step;
        let _: fn(&str) -> Option<SpelledPitch> = SpelledPitch::parse;
        let _: fn(&str) -> Option<(Letter, Accidental)> = SpelledPitch::parse_class;
        let _: fn(SpelledPitch) -> String = SpelledPitch::to_ascii;
        let _: fn(SpelledPitch) -> String = SpelledPitch::to_unicode;
        let _: fn(SpelledPitch) -> String = SpelledPitch::class_ascii;
        let _: fn(SpelledPitch, Interval) -> SpelledPitch = SpelledPitch::transpose;
        let _: fn(SpelledPitch, i32, &ScaleInstance) -> SpelledPitch =
            SpelledPitch::transpose_diatonic;
        let _: fn(i32, Option<&SpellingContext>) -> SpelledPitch = SpelledPitch::from_midi;
        let _: fn(SpelledPitch, Letter) -> Option<SpelledPitch> = SpelledPitch::enharmonic_respell;
        let _: fn(SpelledPitch) -> bool = SpelledPitch::is_valid_midi;

        let _: fn(i32, IntervalQuality) -> Option<Interval> = Interval::new;
        let _: fn(Interval) -> i32 = Interval::semitones;
        let _: fn(Interval) -> i32 = Interval::diatonic_steps;
        let _: fn(SpelledPitch, SpelledPitch) -> SignedInterval = Interval::between;
        let _: fn(i32) -> Interval = Interval::from_semitones_default;
        let _: fn(Interval) -> String = Interval::name;
        let _: fn(&str) -> Option<Interval> = Interval::parse;
        let _: fn(Interval) -> bool = Interval::is_perfect_consonance;
        let _: fn(Interval) -> bool = Interval::is_imperfect_consonance;
        let _: fn(Interval) -> bool = Interval::is_dissonant;
        let _: fn(Interval) -> Interval = Interval::inverted;
        let _: fn(Interval) -> Interval = Interval::simple;
    }

    #[test]
    fn time_signatures() {
        let _: fn(i64, i64) -> BeatTime = BeatTime::new;
        let _: fn(i64, i64) -> Option<BeatTime> = BeatTime::try_new;
        let _: fn(i64) -> BeatTime = BeatTime::from_quarters;
        let _: fn(f64) -> BeatTime = BeatTime::from_f64;
        let _: fn(f64, i64) -> BeatTime = BeatTime::from_f64_grid;
        let _: fn(BeatTime) -> f64 = BeatTime::as_f64;
        let _: fn(BeatTime) -> i64 = BeatTime::num;
        let _: fn(BeatTime) -> i64 = BeatTime::den;
        let _: fn(BeatTime) -> bool = BeatTime::is_zero;
        let _: fn(BeatTime) -> bool = BeatTime::is_positive;
        let _: fn(BeatTime, BeatTime) -> BeatTime = BeatTime::min;
        let _: fn(BeatTime, BeatTime) -> BeatTime = BeatTime::max;
        let _: fn(BeatTime) -> BeatTime = BeatTime::abs;
        let _: fn(BeatTime, BeatTime) -> BeatTime = BeatTime::rem_euclid;
        let _: fn(BeatTime, BeatTime) -> i64 = BeatTime::div_floor;
        let _: fn(BeatTime, i64, i64) -> BeatTime = BeatTime::scale;
        let _: fn(BeatTime) -> String = BeatTime::to_display;

        let _: fn(u16, u16) -> TimeSignature = TimeSignature::new;
        let _: fn(TimeSignature) -> BeatTime = TimeSignature::bar_length_qn;
        let _: fn(TimeSignature) -> BeatTime = TimeSignature::beat_unit_qn;
        let _: fn(TimeSignature) -> bool = TimeSignature::is_compound;
        let _: fn(TimeSignature, BeatTime) -> f64 = TimeSignature::metric_weight;
        let _: fn(&str) -> Option<TimeSignature> = TimeSignature::parse;
        let _: fn(TimeSignature) -> String = TimeSignature::to_display;

        let _: fn(f64, TimeSignature) -> TimeMap = TimeMap::constant;
        let _: fn(Vec<TempoEvent>, Vec<MeterEvent>) -> TimeMap = TimeMap::new;
        let _: fn(&TimeMap, BeatTime) -> TimeSignature = TimeMap::meter_at;
        let _: fn(&TimeMap, BeatTime) -> f64 = TimeMap::tempo_at;
        let _: fn(&TimeMap, BeatTime) -> i64 = TimeMap::bar_of;
        let _: fn(&TimeMap, i64) -> BeatTime = TimeMap::bar_start;
        let _: fn(&TimeMap, BeatTime) -> BeatTime = TimeMap::position_in_bar;
        let _: fn(&TimeMap, BeatTime) -> f64 = TimeMap::metric_weight;
        let _: fn(&TimeMap, BeatTime) -> f64 = TimeMap::qn_to_seconds;
        let _: fn(&TimeMap) -> String = TimeMap::hash_hex;
        let _: fn(&TimeMap, BeatTime, BeatTime) -> Vec<BeatTime> = TimeMap::bar_grid;
        let _: fn(&TimeMap, BeatTime, BeatTime, BeatTime) -> Vec<BeatTime> = TimeMap::beat_grid;
        assert_eq!(PPQ, 960);
    }

    #[test]
    fn chord_and_symbol_signatures() {
        let _: fn(&str) -> Option<ChordDegree> = ChordDegree::parse;
        let _: fn(ChordDegree) -> String = ChordDegree::to_string;
        let _: fn(ChordDegree) -> i32 = ChordDegree::semitones_from_root;
        let _: fn(ChordDegree) -> i32 = ChordDegree::simple_semitones;
        let _: fn(ChordDegree) -> Interval = ChordDegree::interval;

        let _: fn(&ChordSpec) -> ChordTones = ChordSpec::chord_tones;
        let _: fn(&ChordSpec) -> Vec<i32> = ChordSpec::pitch_classes;
        let _: fn(&ChordSpec) -> Vec<ChordDegree> = ChordSpec::guide_tones;
        let _: fn(&ChordSpec) -> bool = ChordSpec::is_dominant_family;
        let _: fn(&ChordSpec) -> bool = ChordSpec::is_major_family;
        let _: fn(&ChordSpec) -> bool = ChordSpec::is_minor_family;
        let _: fn(&ChordSpec) -> bool = ChordSpec::is_diminished_family;
        let _: fn(&ChordSpec) -> bool = ChordSpec::is_suspended;
        let _: fn(&ChordSpec, u8) -> bool = ChordSpec::has_degree;
        let _: fn(&ChordSpec, i32) -> Option<ChordDegree> = ChordSpec::degree_of_pc;
        let _: fn(&ChordSpec) -> i32 = ChordSpec::root_pc;
        let _: fn(&ChordSpec) -> i32 = ChordSpec::bass_pc;
        let _: fn(&ChordSpec, Interval) -> ChordSpec = ChordSpec::transpose;
        let _: fn(&ChordSpec) -> String = ChordSpec::render_ascii;
        let _: fn(&ChordSpec) -> String = ChordSpec::render_unicode;
        let _: fn(&ChordSpec) -> &'static str = ChordSpec::family_id;
        let _: fn(&str) -> Result<ChordSpec, symbol::SymbolError> = symbol::parse;

        let _: fn(HarmonicFunction) -> &'static str = HarmonicFunction::id;
        let _: fn(&str) -> Option<HarmonicFunction> = HarmonicFunction::parse;
        let _: fn(VoicingFamily) -> &'static str = VoicingFamily::id;
        let _: fn(&str) -> Option<VoicingFamily> = VoicingFamily::parse;
        let _: fn() -> &'static [VoicingFamily] = VoicingFamily::all;
        let _: fn(SeventhQuality) -> &'static str = SeventhQuality::id;
        let _: fn(TriadQuality) -> &'static str = TriadQuality::id;
        let _: fn(&Voicing) -> Json = Voicing::to_json;
        let _: fn(&ChordEvent) -> Json = ChordEvent::to_json;
        let _: fn(&Json) -> Result<ChordEvent, DomainError> = ChordEvent::from_json;
    }

    #[test]
    fn note_and_structure_signatures() {
        let _: fn(NoteId, SpelledPitch, BeatTime, BeatTime) -> Note = Note::new;
        let _: fn(&Note) -> BeatTime = Note::end;
        let _: fn(&Note, &Note) -> bool = Note::overlaps;
        let _: fn(&Note, BeatTime) -> bool = Note::contains;
        let _: fn(&Note) -> Result<(), DomainError> = Note::validate;

        let _: fn(Vec<Note>, TimeMap) -> NoteSet = NoteSet::sorted;
        let _: fn(&NoteSet) -> (BeatTime, BeatTime) = NoteSet::span;
        let _: fn(&NoteSet) -> bool = NoteSet::is_monophonic;
        let _: fn(&NoteSet, BeatTime) -> usize = NoteSet::polyphony_at;
        let _: fn(&NoteSet) -> usize = NoteSet::max_polyphony;
        let _: for<'a> fn(&'a NoteSet, BeatTime) -> Vec<&'a Note> = NoteSet::sounding_at;
        let _: for<'a> fn(&'a NoteSet, VoiceId) -> Vec<&'a Note> = NoteSet::by_voice;
        let _: for<'a> fn(&'a NoteSet) -> Vec<&'a Note> = NoteSet::highest_line;
        let _: for<'a> fn(&'a NoteSet) -> Vec<&'a Note> = NoteSet::lowest_line;
        let _: fn(&NoteSet) -> String = NoteSet::hash_hex;
        let _: fn(&NoteSet) -> [f64; 12] = NoteSet::pitch_class_durations;

        let _: fn(NctKind) -> &'static str = NctKind::id;
        let _: fn(&str) -> Option<NctKind> = NctKind::parse;
        let _: fn(NoteRole) -> &'static str = NoteRole::id;
        let _: fn(ArrangementRole) -> &'static str = ArrangementRole::id;
        let _: fn() -> &'static [ArrangementRole] = ArrangementRole::all;
        let _: fn(LoopIntent) -> &'static str = LoopIntent::id;
        let _: fn(&Phrase) -> Json = Phrase::to_json;
        let _: fn(&Motive) -> Json = Motive::to_json;
        let _: fn(&MotiveOccurrence) -> Json = MotiveOccurrence::to_json;
        let _: fn(&KeyRegion) -> Json = KeyRegion::to_json;
        let _: fn(&HarmonicRegion) -> Json = HarmonicRegion::to_json;
        let _: fn(&Section) -> Json = Section::to_json;
        let _: fn(&VoiceLeadingConnection) -> Json = VoiceLeadingConnection::to_json;
        let _: fn(MotiveTransform) -> &'static str = MotiveTransform::id;
        let _: fn(CadenceKind) -> &'static str = CadenceKind::id;
        let _: fn(MotionKind) -> &'static str = MotionKind::id;
        let _: fn(RelativeMotion) -> &'static str = RelativeMotion::id;
        let _: fn(&NctHypothesis) -> Json = NctHypothesis::to_json;
        let _: fn(&NoteOrigin) -> Json = NoteOrigin::to_json;
        assert_eq!(VOICE_UNASSIGNED, VoiceId(u16::MAX));
    }

    #[test]
    fn candidate_plan_and_fixture_signatures() {
        let _: fn() -> ScoreVector = ScoreVector::new;
        let _: fn(&mut ScoreVector, &str, f64) = ScoreVector::set;
        let _: fn(&mut ScoreVector, &str, f64) = ScoreVector::add;
        let _: fn(&ScoreVector, &str) -> f64 = ScoreVector::get;
        let _: fn(&ScoreVector) -> f64 = ScoreVector::total;
        let _: fn(&mut ScoreVector, f64) = ScoreVector::set_total;
        let _: fn(&ScoreVector) -> Json = ScoreVector::to_json;
        assert_eq!(SCORE_COMPONENTS.len(), 13);

        let _: fn(&DecisionTrace) -> Json = DecisionTrace::to_json;
        let _: fn(&Json) -> Result<DecisionTrace, DomainError> = DecisionTrace::from_json;
        let _: fn(&Candidate) -> Json = Candidate::to_json;
        let _: fn(&LoopReport) -> Json = LoopReport::to_json;
        let _: fn(&Part) -> Json = Part::to_json;
        let _: fn(&RuleApplication) -> Json = RuleApplication::to_json;
        let _: fn(&Warning) -> Json = Warning::to_json;
        let _: fn(CandidateKind) -> &'static str = CandidateKind::id;
        let _: fn(RuleStatus) -> &'static str = RuleStatus::id;
        let _: fn(Severity) -> &'static str = Severity::id;

        let _: fn(&EditPlan) -> Json = EditPlan::to_json;
        let _: fn(&Json) -> Result<EditPlan, DomainError> = EditPlan::from_json;
        let _: fn(&EditOperation) -> Json = EditOperation::to_json;
        let _: fn(&Json) -> Result<EditOperation, DomainError> = EditOperation::from_json;
        let _: fn(&PlannedNote) -> Json = PlannedNote::to_json;
        let _: fn(&Json) -> Result<PlannedNote, DomainError> = PlannedNote::from_json;
        let _: fn(&Precondition) -> Json = Precondition::to_json;
        let _: fn(&Json) -> Result<Precondition, DomainError> = Precondition::from_json;
        let _: fn(&ExpectedOutput) -> Json = ExpectedOutput::to_json;
        let _: fn(&Json) -> Result<ExpectedOutput, DomainError> = ExpectedOutput::from_json;

        let _: fn(&Json) -> Result<Fixture, DomainError> = Fixture::from_json;
        let _: fn(&std::path::Path) -> Result<Fixture, DomainError> = Fixture::from_path;
        let _: fn(&Fixture) -> NoteSet = Fixture::note_set;
        let _: fn(&Fixture) -> Vec<ChordEvent> = Fixture::chord_events;
        let _: fn(&Fixture) -> TimeMap = Fixture::time_map;

        let _: fn(ScaleDef, (Letter, Accidental)) -> ScaleInstance = ScaleInstance::new;
        let _: fn(&ScaleInstance) -> Vec<i32> = ScaleInstance::pitch_classes;
        let _: fn(&ScaleInstance) -> Vec<(Letter, Accidental)> = ScaleInstance::spelled_degrees;
        let _: fn(&ScaleInstance, i32) -> bool = ScaleInstance::contains_pc;
        let _: fn(&ScaleInstance, i32) -> Option<usize> = ScaleInstance::degree_of_pc;
        let _: fn(&ScaleInstance, i32) -> SpelledPitch = ScaleInstance::spell;
        let _: fn(&ScaleInstance) -> SpellingContext = ScaleInstance::spelling_context;
        let _: fn(&ScaleInstance, i32) -> i32 = ScaleInstance::nearest_degree;
    }

    #[test]
    fn error_signatures() {
        let e = DomainError::new("CODE", "message");
        let _: &dyn std::error::Error = &e;
        assert_eq!(e.code, "CODE");
        assert_eq!(e.message, "message");
    }
}
