//! The rule engine: the part of this crate that *executes* the knowledge
//! rather than displaying it.
//!
//! # Three-valued logic
//!
//! Predicates return [`Tri`], not `bool`. `Unknown` is a first-class answer and
//! means "the caller has not told us"; it is never quietly coerced to `false`.
//! A rule with any `Unknown` condition reports `not_applicable` rather than
//! firing, because guessing here would put invented reasoning into a user-facing
//! explanation.
//!
//! # Evaluation order for one rule
//!
//! Following `docs/THEORY_MODEL.md` §2:
//!
//! 1. the trigger event must equal the context event;
//! 2. every trigger selector must match a fact in the context;
//! 3. the active profile chain must contain one of the rule's `profiles`;
//! 4. the profile must not have `"disabled"` the rule;
//! 5. any `exception` evaluating `True` bypasses the rule — reported, not hidden;
//! 6. every `condition` must evaluate `True`;
//! 7. the rule applies, contributing `score_delta × multiplier` to one component.
//!
//! Steps 5 and 6 are deliberately in that order: an exception bypasses the rule
//! even when the conditions would have matched, and the trace shows the bypass
//! rather than silence.
//!
//! # Hard before soft
//!
//! [`RuleEngine::evaluate`] runs [`crate::model::RuleKind::is_hard`] kinds first.
//! A rule whose kind [`crate::model::RuleKind::rejects`] and whose conditions all
//! hold is reported
//! `Violated`, contributes no score at all, and lands in
//! [`RuleOutcome::hard_violations`] so the caller can discard the candidate
//! before any soft scoring is even attempted.
//!
//! # Facts
//!
//! Predicates take no arguments. Everything parametric is either encoded in the
//! predicate name or read from a named fact in [`RuleContext`]; the fact keys
//! are the constants in [`facts`].

use crate::error::KbError;
use crate::load::KnowledgeBase;
use crate::model::{RuleEvent, TheoryRule};
use crate::profile::ResolvedProfile;
use music_domain::candidate::{RuleApplication, RuleStatus};
use qjson::{Json, JsonMap};
use std::collections::BTreeMap;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// three-valued logic
// ---------------------------------------------------------------------------

/// A predicate's answer. `Unknown` means the context does not carry the fact.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tri {
    /// The predicate holds.
    True,
    /// The predicate demonstrably does not hold.
    False,
    /// The context does not know. Never treated as `False`.
    Unknown,
}

impl Tri {
    /// Lifts a definite boolean.
    pub fn known(b: bool) -> Tri {
        if b {
            Tri::True
        } else {
            Tri::False
        }
    }

    /// Kleene negation: `Unknown` negates to `Unknown`.
    ///
    /// Named `not` to read as the logic table it implements; `std::ops::Not`
    /// is implemented below and delegates here.
    #[allow(clippy::should_implement_trait)] // std::ops::Not is also implemented, below.
    pub fn not(self) -> Tri {
        match self {
            Tri::True => Tri::False,
            Tri::False => Tri::True,
            Tri::Unknown => Tri::Unknown,
        }
    }

    /// Kleene conjunction: `False` wins over `Unknown`.
    pub fn and(self, other: Tri) -> Tri {
        match (self, other) {
            (Tri::False, _) | (_, Tri::False) => Tri::False,
            (Tri::True, Tri::True) => Tri::True,
            _ => Tri::Unknown,
        }
    }

    /// Kleene disjunction: `True` wins over `Unknown`.
    pub fn or(self, other: Tri) -> Tri {
        match (self, other) {
            (Tri::True, _) | (_, Tri::True) => Tri::True,
            (Tri::False, Tri::False) => Tri::False,
            _ => Tri::Unknown,
        }
    }

    /// Falls back to `f` only when this answer is `Unknown`.
    pub fn or_else(self, f: impl FnOnce() -> Tri) -> Tri {
        match self {
            Tri::Unknown => f(),
            other => other,
        }
    }

    /// True only for [`Tri::True`].
    pub fn is_true(self) -> bool {
        self == Tri::True
    }

    /// True only for [`Tri::Unknown`].
    pub fn is_unknown(self) -> bool {
        self == Tri::Unknown
    }

    /// `"true"`, `"false"` or `"unknown"`.
    pub fn id(self) -> &'static str {
        match self {
            Tri::True => "true",
            Tri::False => "false",
            Tri::Unknown => "unknown",
        }
    }
}

impl std::ops::Not for Tri {
    type Output = Tri;

    fn not(self) -> Tri {
        Tri::not(self)
    }
}

impl From<bool> for Tri {
    fn from(b: bool) -> Tri {
        Tri::known(b)
    }
}

impl From<Option<bool>> for Tri {
    fn from(b: Option<bool>) -> Tri {
        match b {
            Some(v) => Tri::known(v),
            None => Tri::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// fact key vocabulary
// ---------------------------------------------------------------------------

/// The named numeric and string facts the derived predicates read.
///
/// Engines that fill a [`RuleContext`] should use these constants rather than
/// string literals, so a renamed fact is a compile error instead of a predicate
/// that silently answers `Unknown` forever.
pub mod facts {
    // --- metric position ---
    /// Metric weight of the note's onset, `0.0..=1.0`.
    pub const METRIC_WEIGHT: &str = "metric_weight";

    // --- melody ---
    /// Signed semitone interval from the previous melody note.
    pub const MELODY_INTERVAL_SEMITONES: &str = "melody_interval_semitones";
    /// Structural salience of the melody note, `0.0..=1.0`.
    pub const MELODY_SALIENCE: &str = "melody_salience";
    /// Salience above which a note counts as structural. Defaults to `0.6`.
    pub const SALIENCE_THRESHOLD: &str = "salience_threshold";
    /// Note duration divided by the prevailing note value.
    pub const MELODY_DURATION_RATIO: &str = "melody_duration_ratio";
    /// The melody note's degree over the sounding chord, e.g. `"b9"`, `"11"`.
    pub const MELODY_CHORD_DEGREE: &str = "melody_chord_degree";

    // --- chord content ---
    /// Degrees actually sounding in the realised voicing, e.g. `["1","3","b7"]`.
    pub const SOUNDING_DEGREES: &str = "sounding_degrees";
    /// Degrees the chord symbol declares as chord tones.
    pub const CHORD_TONE_DEGREES: &str = "chord_tone_degrees";
    /// The chord's family id: `"major"`, `"dominant"`, `"sus"`, …
    pub const CHORD_FAMILY: &str = "chord_family";
    /// The chord's function class: `"tonic"`, `"predominant"`, `"dominant"`, …
    pub const FUNCTION_CLASS: &str = "function_class";
    /// The realised voicing family: `"close"`, `"quartal"`, `"rootless"`, …
    pub const VOICING_FAMILY: &str = "voicing_family";

    // --- register and spacing ---
    /// Lowest sounding MIDI pitch of the voicing or part.
    pub const MIN_SOUNDING_MIDI: &str = "min_sounding_midi";
    /// Highest sounding MIDI pitch of the voicing or part.
    pub const MAX_SOUNDING_MIDI: &str = "max_sounding_midi";
    /// Smallest interval between any adjacent voice pair, in semitones.
    pub const MIN_ADJACENT_VOICE_INTERVAL: &str = "min_adjacent_voice_interval";
    /// The active instrument profile's limit for the lowest voice pair.
    pub const INSTRUMENT_LOW_INTERVAL_LIMIT: &str = "instrument_low_interval_limit";
    /// Lowest playable MIDI pitch of the active instrument profile.
    pub const INSTRUMENT_LOW_MIDI: &str = "instrument_low_midi";
    /// Highest playable MIDI pitch of the active instrument profile.
    pub const INSTRUMENT_HIGH_MIDI: &str = "instrument_high_midi";
    /// Soprano-to-bass distance in semitones.
    pub const OUTER_VOICE_SPAN_SEMITONES: &str = "outer_voice_span_semitones";
    /// Distance from the chordal third up to the natural eleventh, in semitones.
    pub const THIRD_TO_ELEVENTH_SEMITONES: &str = "third_to_eleventh_semitones";

    // --- motion ---
    /// Relative motion of the voice pair: `"parallel"`, `"similar"`, `"contrary"`, `"oblique"`, `"static"`.
    pub const MOTION: &str = "motion";
    /// The interval the voice pair arrives at, in semitones.
    pub const ARRIVAL_INTERVAL_SEMITONES: &str = "arrival_interval_semitones";
    /// Signed semitone motion of the single voice under examination.
    pub const VOICE_MOTION_SEMITONES: &str = "voice_motion_semitones";
    /// Total semitone travel of the chosen path.
    pub const TOTAL_MOTION_SEMITONES: &str = "total_motion_semitones";
    /// Total semitone travel of the cheapest available path.
    pub const MIN_AVAILABLE_MOTION_SEMITONES: &str = "min_available_motion_semitones";
    /// Signed semitone motion from this chord's root to the next chord's root.
    pub const ROOT_MOTION_SEMITONES: &str = "root_motion_semitones";

    // --- key and request context ---
    /// The active key's mode id, e.g. `"major"`, `"aeolian"`, `"dorian"`.
    pub const KEY_MODE: &str = "key_mode";
    /// `"modal"` when analysis chose a modal centre, `"tonal"` otherwise.
    pub const KEY_CENTER_KIND: &str = "key_center_kind";
    /// Chord changes per bar.
    pub const CHORDS_PER_BAR: &str = "chords_per_bar";
    /// The request's complexity control, `0.0..=1.0`.
    pub const COMPLEXITY_TARGET: &str = "complexity_target";
    /// The request's chromaticism control, `0.0..=1.0`.
    pub const CHROMATICISM_TARGET: &str = "chromaticism_target";
    /// The request's extension-density control, `0.0..=1.0`.
    pub const EXTENSION_DENSITY: &str = "extension_density";
    /// The request's strictness setting.
    pub const STRICTNESS: &str = "strictness";

    // --- arrangement ---
    /// The part's arrangement role id.
    pub const ROLE: &str = "role";
    /// Onsets per bar written by this part.
    pub const PART_DENSITY: &str = "part_density";
    /// Onsets-per-bar budget from the arrangement pattern.
    pub const ROLE_DENSITY_TARGET: &str = "role_density_target";
    /// Fraction of this part's onsets that coincide with a foreground part's.
    pub const ONSET_COLLISION_RATIO: &str = "onset_collision_ratio";
    /// Semitones of tessitura shared with another part.
    pub const REGISTER_OVERLAP_SEMITONES: &str = "register_overlap_semitones";
    /// Maximum simultaneous notes this part writes.
    pub const PART_POLYPHONY: &str = "part_polyphony";
    /// Maximum simultaneous notes the instrument profile allows.
    pub const INSTRUMENT_POLYPHONY: &str = "instrument_polyphony";

    // --- loop ---
    /// Function class of the loop's last chord.
    pub const FINAL_CHORD_FUNCTION: &str = "final_chord_function";
    /// Function class of the loop's first chord.
    pub const FIRST_CHORD_FUNCTION: &str = "first_chord_function";
    /// `"carry"`, `"split"` or `"default"`.
    pub const NOTE_CARRY_POLICY: &str = "note_carry_policy";
    /// Signed bass interval across the wrap, in semitones.
    pub const WRAP_BASS_INTERVAL_SEMITONES: &str = "wrap_bass_interval_semitones";
    /// Generated span length in quarter notes.
    pub const GENERATED_LENGTH_QN: &str = "generated_length_qn";
    /// Requested loop length in quarter notes.
    pub const REQUESTED_LOOP_LENGTH_QN: &str = "requested_loop_length_qn";
    /// Harmonic slot length immediately before the wrap, in quarter notes.
    pub const SLOT_LENGTH_BEFORE_WRAP_QN: &str = "slot_length_before_wrap_qn";
    /// Harmonic slot length immediately after the wrap, in quarter notes.
    pub const SLOT_LENGTH_AFTER_WRAP_QN: &str = "slot_length_after_wrap_qn";
    /// Latest note end in the candidate, in quarter notes.
    pub const MAX_NOTE_END_QN: &str = "max_note_end_qn";
    /// The loop end point, in quarter notes.
    pub const LOOP_END_QN: &str = "loop_end_qn";

    // --- integrity ---
    /// Lowest MIDI number the candidate would write.
    pub const MIN_NOTE_MIDI: &str = "min_note_midi";
    /// Highest MIDI number the candidate would write.
    pub const MAX_NOTE_MIDI: &str = "max_note_midi";
    /// Shortest note duration the candidate would write, in quarter notes.
    pub const MIN_NOTE_DURATION_QN: &str = "min_note_duration_qn";

    // --- trigger selector facts ---
    /// Scale family of the active collection.
    pub const SCALE_FAMILY: &str = "scale_family";
    /// Cadence kind detected at this slot.
    pub const CADENCE_KIND: &str = "cadence_kind";
    /// The active loop intent.
    pub const LOOP_INTENT: &str = "loop_intent";
    /// The note's role.
    pub const NOTE_ROLE: &str = "note_role";
    /// Short interval name of the pair, e.g. `"P5"`.
    pub const INTERVAL: &str = "interval";
    /// Tags of the progression or cadence schema in play.
    pub const PROGRESSION_TAGS: &str = "progression_tags";
}

/// Salience above which a melody note counts as structural, when the caller
/// does not supply [`facts::SALIENCE_THRESHOLD`].
pub const DEFAULT_SALIENCE_THRESHOLD: f64 = 0.6;

// ---------------------------------------------------------------------------
// the context
// ---------------------------------------------------------------------------

/// Everything a predicate can inspect.
///
/// Facts are insertion-ordered so a serialised context is byte-stable. Engines
/// fill in what they know and leave the rest absent; an absent fact yields
/// [`Tri::Unknown`], never `false`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuleContext {
    event: Option<RuleEvent>,
    bools: Vec<(String, bool)>,
    nums: Vec<(String, f64)>,
    strs: Vec<(String, String)>,
    lists: Vec<(String, Vec<String>)>,
}

impl RuleContext {
    /// An empty context: every predicate answers `Unknown`.
    pub fn new() -> Self {
        RuleContext::default()
    }

    /// Asserts a predicate outright. This is the primary path: most of the
    /// vocabulary is a fact the analysis layer has already computed.
    pub fn set_bool(&mut self, predicate: &str, value: bool) {
        upsert(&mut self.bools, predicate, value);
    }

    /// Records a named numeric fact; see [`facts`].
    pub fn set_num(&mut self, key: &str, value: f64) {
        upsert(&mut self.nums, key, value);
    }

    /// Records a named string fact; see [`facts`].
    pub fn set_str(&mut self, key: &str, value: &str) {
        upsert(&mut self.strs, key, value.to_string());
    }

    /// Records a named list-of-strings fact, used by the degree and tag facts.
    pub fn set_list<S: AsRef<str>>(&mut self, key: &str, values: &[S]) {
        upsert(
            &mut self.lists,
            key,
            values.iter().map(|v| v.as_ref().to_string()).collect(),
        );
    }

    /// Reads an asserted predicate.
    pub fn get_bool(&self, predicate: &str) -> Option<bool> {
        self.bools
            .iter()
            .find(|(k, _)| k == predicate)
            .map(|(_, v)| *v)
    }

    /// Reads a numeric fact.
    pub fn get_num(&self, key: &str) -> Option<f64> {
        self.nums.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    /// Reads a string fact.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.strs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Reads a list fact.
    pub fn get_list(&self, key: &str) -> Option<&[String]> {
        self.lists
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_slice())
    }

    /// The pipeline event being processed.
    pub fn event(&self) -> Option<RuleEvent> {
        self.event
    }

    /// Sets the pipeline event being processed.
    pub fn set_event(&mut self, e: RuleEvent) {
        self.event = Some(e);
    }

    /// Builder form of [`RuleContext::set_event`].
    pub fn with_event(mut self, e: RuleEvent) -> Self {
        self.set_event(e);
        self
    }

    /// Builder form of [`RuleContext::set_bool`].
    pub fn with_bool(mut self, predicate: &str, value: bool) -> Self {
        self.set_bool(predicate, value);
        self
    }

    /// Builder form of [`RuleContext::set_num`].
    pub fn with_num(mut self, key: &str, value: f64) -> Self {
        self.set_num(key, value);
        self
    }

    /// Builder form of [`RuleContext::set_str`].
    pub fn with_str(mut self, key: &str, value: &str) -> Self {
        self.set_str(key, value);
        self
    }

    /// Builder form of [`RuleContext::set_list`].
    pub fn with_list<S: AsRef<str>>(mut self, key: &str, values: &[S]) -> Self {
        self.set_list(key, values);
        self
    }

    /// Evaluates one predicate against this context.
    ///
    /// Returns an error only for a predicate outside the closed vocabulary,
    /// which is a knowledge-data defect rather than a runtime condition.
    pub fn evaluate(&self, predicate: &str) -> Result<Tri, KbError> {
        evaluate_predicate(predicate, self)
    }

    /// JSON form, for embedding a context in a decision trace.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert(
            "event",
            match self.event {
                Some(e) => Json::Str(e.id().to_string()),
                None => Json::Null,
            },
        );
        let mut b = JsonMap::new();
        for (k, v) in &self.bools {
            b.insert(k.clone(), Json::Bool(*v));
        }
        m.insert("bools", Json::Obj(b));
        let mut n = JsonMap::new();
        for (k, v) in &self.nums {
            n.insert(k.clone(), Json::Float(*v));
        }
        m.insert("nums", Json::Obj(n));
        let mut s = JsonMap::new();
        for (k, v) in &self.strs {
            s.insert(k.clone(), Json::Str(v.clone()));
        }
        m.insert("strs", Json::Obj(s));
        let mut l = JsonMap::new();
        for (k, v) in &self.lists {
            l.insert(
                k.clone(),
                Json::Arr(v.iter().map(|x| Json::Str(x.clone())).collect()),
            );
        }
        m.insert("lists", Json::Obj(l));
        Json::Obj(m)
    }
}

/// Replaces an existing entry in place, preserving insertion order.
fn upsert<T>(store: &mut Vec<(String, T)>, key: &str, value: T) {
    if let Some(slot) = store.iter_mut().find(|(k, _)| k == key) {
        slot.1 = value;
    } else {
        store.push((key.to_string(), value));
    }
}

// ---------------------------------------------------------------------------
// the closed predicate vocabulary
// ---------------------------------------------------------------------------

/// Every predicate this build understands, paired with the clause it
/// contributes to a human-readable explanation.
///
/// The clauses are written to slot into a sentence after "…because" or
/// "…here:", which is why they are lower-case and finite-verb phrases.
///
/// Sorted by id; [`RuleEngine::known_predicates`] depends on that ordering.
pub const PREDICATE_TABLE: &[(&str, &str)] = &[
    ("altered_tone_resolves_by_step", "every altered degree moves by step into the next chord"),
    ("bass_leaps_across_wrap", "the bass leaps more than a fifth across the loop seam"),
    ("bass_supplies_root", "another part is sounding the chord root in the bass register"),
    ("cadence_is_expected_at_this_slot", "phrase analysis marks this slot as a cadential arrival"),
    ("cadential_six_four_present", "a tonic-shaped chord is sounding over the dominant scale degree"),
    ("candidate_duplicates_existing_strategy", "this candidate repeats an already-selected candidate's root motion, functional path, modal source and bass contour"),
    ("chord_has_altered_tones", "the chord is sounding at least one altered degree"),
    ("chord_is_applied_dominant", "the chord is an applied dominant of something other than the tonic"),
    ("chord_is_borrowed_from_parallel_mode", "the chord is borrowed from the parallel mode"),
    ("chord_is_diatonic_to_key", "every chord tone belongs to the active key"),
    ("chord_is_in_root_position", "the chord is in root position"),
    ("chord_is_rootless_voicing", "the voicing is rootless"),
    ("chord_is_symmetric_collection", "the chord divides the octave evenly"),
    ("chord_is_tritone_substitute", "the chord is a tritone substitute for the expected dominant"),
    ("chord_symbol_is_ambiguous", "more than one reading of the chord symbol has equal precedence"),
    ("chordal_seventh_resolves_down_by_step", "the chordal seventh falls by step"),
    ("chromaticism_target_is_high", "the request asks for a high level of chromaticism"),
    ("cluster_intent_is_false", "the request did not ask for cluster voicings"),
    ("common_tone_available", "the two chords share at least one pitch class"),
    ("common_tone_retained", "a shared pitch class is held in the same voice"),
    ("complexity_target_is_high", "the request asks for high complexity"),
    ("contrast_is_dynamics_only", "the two sections differ only in velocity"),
    ("dissonance_is_on_strong_beat", "the dissonance falls on a metrically strong beat"),
    ("dissonance_is_prepared", "the dissonance is prepared as a consonance in the same voice"),
    ("doubling_is_allowed_for_role", "the arrangement pattern permits this doubling"),
    ("doubling_is_on_tendency_tone", "a doubled voice lands on a tendency tone"),
    ("duration_is_not_positive", "a note has a zero or negative duration"),
    ("essential_chord_tone_only_in_this_layer", "this layer holds the chord's last sounding third or seventh"),
    ("extension_density_is_high", "the request asks for dense extensions"),
    ("fifth_is_omitted", "the fifth is omitted from the voicing"),
    ("final_chord_is_dominant_function", "the loop's last chord is a dominant"),
    ("final_chord_is_tonic_function", "the loop's last chord is a tonic"),
    ("first_chord_is_tonic_function", "the loop's first chord is a tonic"),
    ("function_is_dominant", "the chord functions as a dominant"),
    ("function_is_predominant", "the chord functions as a predominant"),
    ("function_is_tonic", "the chord functions as a tonic"),
    ("guide_tones_present", "both guide tones are sounding"),
    ("harmonic_rhythm_changes_at_wrap", "the harmonic rhythm changes across the loop seam"),
    ("harmonic_rhythm_is_fast", "the harmony changes more than once per bar"),
    ("harmonic_rhythm_is_slow", "the harmony changes less than once per bar"),
    ("intentional_cluster", "clusters were asked for"),
    ("interval_is_perfect_fifth_or_octave", "the voices arrive on a perfect fifth or octave"),
    ("is_loop_wrap_boundary", "this pair spans the loop seam"),
    ("key_is_minor", "the key is minor"),
    ("leading_tone_resolves_up_by_step", "the leading tone rises a half step to the tonic"),
    ("leap_is_followed_by_step_in_opposite_direction", "the leap is recovered by a step in the opposite direction"),
    (
        "loop_length_is_not_exact",
        "the generated span differs from the requested loop length",
    ),
    ("melody_is_11", "the eleventh is in the melody"),
    ("melody_is_altered_tone", "the melody note is an altered degree of the chord"),
    ("melody_is_at_phrase_end", "the note ends its phrase"),
    ("melody_is_chord_tone", "the melody note is a chord tone"),
    ("melody_is_extension", "the melody note is an extension of the chord"),
    ("melody_is_pickup", "the note is a pickup into the phrase"),
    ("melody_is_repeated_pitch", "the melody repeats the previous pitch"),
    ("melody_leap_exceeds_fifth", "the melody leaps further than a fifth"),
    ("melody_material_would_be_altered", "the candidate would change melody material the caller asked to preserve"),
    ("melody_note_is_long", "the melody note is long for its context"),
    ("melody_note_is_on_strong_beat", "the melody note falls on a strong beat"),
    ("melody_note_is_structural", "the melody note is structural rather than decorative"),
    ("melody_outlines_augmented_or_tritone_interval", "the melody outlines an augmented interval or a tritone"),
    ("melody_reaches_registral_peak", "the note is the high point of its phrase"),
    ("midi_pitch_out_of_range", "a pitch falls outside the MIDI range"),
    ("modal_center_is_active", "the passage is centred on a mode rather than a major or minor key"),
    ("motion_is_contrary", "the voices move in contrary motion"),
    ("motion_is_oblique", "one voice holds while the other moves"),
    ("motion_is_parallel", "the voices move in parallel"),
    ("motion_is_similar", "the voices move in similar motion"),
    ("note_carry_policy_is_explicit", "the request set an explicit carry or split policy"),
    ("note_extends_past_loop_end", "a generated note sounds past the loop end"),
    ("note_is_approached_by_step", "the note is approached by step"),
    ("note_is_chromatic_to_active_scale", "the note is chromatic to the active scale"),
    ("note_is_left_by_step", "the note is left by step"),
    ("note_is_on_weak_beat", "the note falls on a weak beat"),
    ("note_outside_item_bounds", "a note falls outside the generated item"),
    ("note_returns_to_previous_pitch", "the line returns to the pitch it came from"),
    ("notes_overlap_in_monophonic_part", "two notes sound at once in a part declared monophonic"),
    ("omission_changes_semantic_identity", "the omission would change what the chord is, not merely what is played"),
    ("onsets_collide_with_higher_priority_role", "most of this part's onsets land on a foreground part's onsets"),
    ("outer_voice_span_exceeds_two_octaves", "the outer voices are more than two octaves apart"),
    ("outer_voices_involved", "the pair under examination is the soprano and the bass"),
    ("part_density_exceeds_role_target", "the part is busier than its role's density budget"),
    ("part_duplicates_melody_rhythm", "the part copies the melody's rhythm"),
    ("pedal_continues_across_wrap", "a pedal tone sounds on both sides of the seam"),
    ("pedal_point_is_active", "a pedal point is sounding under the harmony"),
    ("phrase_contains_no_rest", "the part sounds continuously for a whole phrase"),
    ("pickup_is_present", "material sounds before the loop's first downbeat"),
    ("polyphony_exceeds_profile_limit", "the part asks for more simultaneous notes than the instrument allows"),
    ("quartal_or_planing_context", "the context is quartal or planing"),
    ("register_is_high", "the voicing sits high"),
    ("register_is_low", "the voicing sits low"),
    ("register_overlaps_another_part", "the part's register overlaps another part's"),
    ("role_is_bass", "the part is the bass"),
    ("role_is_lead", "the part is the lead"),
    ("role_is_pad_or_sustained", "the part is a pad or other sustained texture"),
    ("root_is_omitted", "the root is omitted from the voicing"),
    ("root_motion_is_descending_fifth", "the roots move down a fifth"),
    ("root_motion_is_stepwise", "the roots move by step"),
    ("root_motion_is_third_related", "the roots are a third apart"),
    ("seventh_is_present", "a chordal seventh is sounding"),
    ("source_material_would_be_mutated", "the plan would modify the user's own material"),
    ("spacing_violates_instrument_low_interval_limit", "the spacing is tighter than the instrument's low interval limit"),
    ("stepwise_connection_available", "every voice could move by step or hold"),
    ("strictness_is_common_practice", "the request asked for strict common-practice treatment"),
    ("suspension_resolves_down_by_step", "the suspension falls by step"),
    ("target_chord_follows", "the expected destination chord actually follows"),
    ("tendency_tone_is_unresolved", "a tendency tone is left unresolved"),
    ("third_and_eleventh_are_in_adjacent_or_close_registers", "the third and the natural eleventh sit close together"),
    ("third_is_not_suspended", "the chord keeps a real third"),
    ("third_is_omitted", "the third is omitted from the voicing"),
    ("third_is_suspended", "the third is suspended"),
    ("total_motion_is_minimal", "the voices take the shortest available path"),
    ("voice_crossing_present", "a voice crosses a neighbouring voice"),
    ("voice_exceeds_instrument_range", "a pitch falls outside the part's instrument range"),
    ("voice_leap_is_large", "a single voice leaps more than an octave"),
    ("voice_overlap_present", "a voice overlaps a neighbouring voice's previous pitch"),
    ("voices_are_widely_spaced", "the voices are widely spaced"),
];

/// The lazily-materialised sorted name list backing
/// [`RuleEngine::known_predicates`].
fn predicate_names() -> &'static [&'static str] {
    static NAMES: OnceLock<Vec<&'static str>> = OnceLock::new();
    NAMES
        .get_or_init(|| PREDICATE_TABLE.iter().map(|(n, _)| *n).collect())
        .as_slice()
}

/// The human clause for a predicate, or the raw name when it is unknown.
pub fn predicate_phrase(name: &str) -> &str {
    PREDICATE_TABLE
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, p)| *p)
        .unwrap_or(name)
}

/// True when the predicate is part of the closed vocabulary.
pub fn is_known_predicate(name: &str) -> bool {
    PREDICATE_TABLE.iter().any(|(n, _)| *n == name)
}

// --- derivation helpers ------------------------------------------------------

fn num_cmp(ctx: &RuleContext, key: &str, f: impl FnOnce(f64) -> bool) -> Tri {
    match ctx.get_num(key) {
        Some(v) => Tri::known(f(v)),
        None => Tri::Unknown,
    }
}

fn num_pair(ctx: &RuleContext, a: &str, b: &str, f: impl FnOnce(f64, f64) -> bool) -> Tri {
    match (ctx.get_num(a), ctx.get_num(b)) {
        (Some(x), Some(y)) => Tri::known(f(x, y)),
        _ => Tri::Unknown,
    }
}

fn str_eq(ctx: &RuleContext, key: &str, want: &str) -> Tri {
    match ctx.get_str(key) {
        Some(v) => Tri::known(v == want),
        None => Tri::Unknown,
    }
}

fn str_in(ctx: &RuleContext, key: &str, wanted: &[&str]) -> Tri {
    match ctx.get_str(key) {
        Some(v) => Tri::known(wanted.contains(&v)),
        None => Tri::Unknown,
    }
}

/// Strips leading accidentals and reads the numeric part of a degree string.
fn degree_number(d: &str) -> Option<u32> {
    d.trim_start_matches(['b', '#']).parse().ok()
}

/// True when the degree carries an accidental, i.e. it is an alteration.
fn degree_is_altered(d: &str) -> bool {
    d.starts_with('b') || d.starts_with('#')
}

/// Whether any sounding degree has the given diatonic number.
fn has_degree(ctx: &RuleContext, n: u32) -> Tri {
    match ctx.get_list(facts::SOUNDING_DEGREES) {
        Some(ds) => Tri::known(ds.iter().any(|d| degree_number(d) == Some(n))),
        None => Tri::Unknown,
    }
}

/// The signed root motion reduced to a pitch-class distance.
fn root_motion_pc(ctx: &RuleContext) -> Option<i64> {
    ctx.get_num(facts::ROOT_MOTION_SEMITONES)
        .map(|s| (s.round() as i64).rem_euclid(12))
}

// --- the evaluator -----------------------------------------------------------

/// Evaluates one predicate name against a context.
///
/// Every predicate first honours an explicitly asserted boolean of the same
/// name — that is the fast path an analysis engine uses once it has computed
/// the fact. When no assertion is present, the predicates that can be *derived*
/// from named numeric, string or list facts are derived here; the rest answer
/// `Unknown`.
///
/// An unrecognised name is an error rather than a silent `Unknown`, so a
/// predicate appearing in `knowledge/` that this build cannot execute fails
/// validation at load time instead of never firing at run time.
pub fn evaluate_predicate(name: &str, ctx: &RuleContext) -> Result<Tri, KbError> {
    let asserted: Tri = ctx.get_bool(name).into();
    let t = match name {
        // ---- chord content ----
        "third_is_omitted" => asserted.or_else(|| has_degree(ctx, 3).not()),
        "fifth_is_omitted" => asserted.or_else(|| has_degree(ctx, 5).not()),
        "root_is_omitted" => asserted.or_else(|| has_degree(ctx, 1).not()),
        "seventh_is_present" => asserted.or_else(|| has_degree(ctx, 7)),
        "third_is_suspended" => asserted.or_else(|| {
            has_degree(ctx, 3)
                .not()
                .and(has_degree(ctx, 4).or(has_degree(ctx, 2)))
        }),
        // The documented exact logical negation of `third_is_suspended`.
        "third_is_not_suspended" => {
            asserted.or_else(|| evaluate_or_unknown("third_is_suspended", ctx).not())
        }
        "chord_has_altered_tones" => {
            asserted.or_else(|| match ctx.get_list(facts::SOUNDING_DEGREES) {
                Some(ds) => Tri::known(ds.iter().any(|d| degree_is_altered(d))),
                None => Tri::Unknown,
            })
        }
        "chord_is_rootless_voicing" => {
            asserted.or_else(|| str_eq(ctx, facts::VOICING_FAMILY, "rootless"))
        }
        "guide_tones_present" => asserted.or_else(|| {
            has_degree(ctx, 3)
                .or(has_degree(ctx, 4))
                .and(has_degree(ctx, 7))
        }),
        "chord_is_symmetric_collection" | "bass_supplies_root" | "doubling_is_on_tendency_tone" => {
            asserted
        }

        // ---- register and spacing ----
        "third_and_eleventh_are_in_adjacent_or_close_registers" => {
            asserted.or_else(|| num_cmp(ctx, facts::THIRD_TO_ELEVENTH_SEMITONES, |v| v <= 14.0))
        }
        "voices_are_widely_spaced" => {
            asserted.or_else(|| num_cmp(ctx, facts::MIN_ADJACENT_VOICE_INTERVAL, |v| v >= 7.0))
        }
        "spacing_violates_instrument_low_interval_limit" => asserted.or_else(|| {
            num_pair(
                ctx,
                facts::MIN_ADJACENT_VOICE_INTERVAL,
                facts::INSTRUMENT_LOW_INTERVAL_LIMIT,
                |got, limit| got < limit,
            )
        }),
        "register_is_low" => {
            asserted.or_else(|| num_cmp(ctx, facts::MAX_SOUNDING_MIDI, |v| v < 48.0))
        }
        "register_is_high" => {
            asserted.or_else(|| num_cmp(ctx, facts::MIN_SOUNDING_MIDI, |v| v > 79.0))
        }
        "outer_voice_span_exceeds_two_octaves" => {
            asserted.or_else(|| num_cmp(ctx, facts::OUTER_VOICE_SPAN_SEMITONES, |v| v > 24.0))
        }
        "voice_exceeds_instrument_range" => asserted.or_else(|| {
            num_pair(
                ctx,
                facts::MIN_SOUNDING_MIDI,
                facts::INSTRUMENT_LOW_MIDI,
                |lo, limit| lo < limit,
            )
            .or(num_pair(
                ctx,
                facts::MAX_SOUNDING_MIDI,
                facts::INSTRUMENT_HIGH_MIDI,
                |hi, limit| hi > limit,
            ))
        }),
        "voice_crossing_present" | "voice_overlap_present" => asserted,

        // ---- melody ----
        "melody_is_11" => asserted.or_else(|| str_eq(ctx, facts::MELODY_CHORD_DEGREE, "11")),
        "melody_is_chord_tone" => asserted.or_else(|| {
            match (
                ctx.get_str(facts::MELODY_CHORD_DEGREE),
                ctx.get_list(facts::CHORD_TONE_DEGREES),
            ) {
                (Some(d), Some(tones)) => Tri::known(tones.iter().any(|t| t == d)),
                _ => Tri::Unknown,
            }
        }),
        "melody_is_extension" => {
            asserted.or_else(|| match ctx.get_str(facts::MELODY_CHORD_DEGREE) {
                Some(d) => Tri::known(
                    !degree_is_altered(d)
                        && matches!(degree_number(d), Some(9) | Some(11) | Some(13)),
                ),
                None => Tri::Unknown,
            })
        }
        "melody_is_altered_tone" => {
            asserted.or_else(|| match ctx.get_str(facts::MELODY_CHORD_DEGREE) {
                Some(d) => Tri::known(degree_is_altered(d)),
                None => Tri::Unknown,
            })
        }
        "melody_note_is_structural" => asserted.or_else(|| {
            let threshold = ctx
                .get_num(facts::SALIENCE_THRESHOLD)
                .unwrap_or(DEFAULT_SALIENCE_THRESHOLD);
            num_cmp(ctx, facts::MELODY_SALIENCE, |v| v > threshold)
        }),
        "melody_note_is_on_strong_beat" => {
            asserted.or_else(|| num_cmp(ctx, facts::METRIC_WEIGHT, |v| v >= 0.75))
        }
        "melody_note_is_long" => {
            asserted.or_else(|| num_cmp(ctx, facts::MELODY_DURATION_RATIO, |v| v >= 2.0))
        }
        "melody_leap_exceeds_fifth" => {
            asserted.or_else(|| num_cmp(ctx, facts::MELODY_INTERVAL_SEMITONES, |v| v.abs() > 7.0))
        }
        "melody_is_repeated_pitch" => {
            asserted.or_else(|| num_cmp(ctx, facts::MELODY_INTERVAL_SEMITONES, |v| v == 0.0))
        }
        "leap_is_followed_by_step_in_opposite_direction"
        | "melody_is_at_phrase_end"
        | "melody_is_pickup"
        | "melody_outlines_augmented_or_tritone_interval"
        | "note_returns_to_previous_pitch"
        | "melody_reaches_registral_peak"
        | "melody_material_would_be_altered" => asserted,

        // ---- motion and voice leading ----
        "motion_is_parallel" => asserted.or_else(|| str_eq(ctx, facts::MOTION, "parallel")),
        "motion_is_similar" => asserted.or_else(|| str_eq(ctx, facts::MOTION, "similar")),
        "motion_is_contrary" => asserted.or_else(|| str_eq(ctx, facts::MOTION, "contrary")),
        "motion_is_oblique" => asserted.or_else(|| str_eq(ctx, facts::MOTION, "oblique")),
        "interval_is_perfect_fifth_or_octave" => asserted.or_else(|| {
            num_cmp(ctx, facts::ARRIVAL_INTERVAL_SEMITONES, |v| {
                let pc = (v.round() as i64).rem_euclid(12);
                pc == 0 || pc == 7
            })
        }),
        "voice_leap_is_large" => {
            asserted.or_else(|| num_cmp(ctx, facts::VOICE_MOTION_SEMITONES, |v| v.abs() > 12.0))
        }
        "total_motion_is_minimal" => asserted.or_else(|| {
            num_pair(
                ctx,
                facts::TOTAL_MOTION_SEMITONES,
                facts::MIN_AVAILABLE_MOTION_SEMITONES,
                |got, best| got <= best,
            )
        }),
        "outer_voices_involved"
        | "common_tone_available"
        | "common_tone_retained"
        | "stepwise_connection_available"
        | "leading_tone_resolves_up_by_step"
        | "chordal_seventh_resolves_down_by_step"
        | "altered_tone_resolves_by_step"
        | "tendency_tone_is_unresolved" => asserted,

        // ---- dissonance treatment ----
        "dissonance_is_on_strong_beat" => {
            asserted.or_else(|| num_cmp(ctx, facts::METRIC_WEIGHT, |v| v >= 0.5))
        }
        "note_is_on_weak_beat" => {
            asserted.or_else(|| num_cmp(ctx, facts::METRIC_WEIGHT, |v| v < 0.5))
        }
        "dissonance_is_prepared"
        | "suspension_resolves_down_by_step"
        | "note_is_approached_by_step"
        | "note_is_left_by_step"
        | "note_is_chromatic_to_active_scale" => asserted,

        // ---- function and progression ----
        "function_is_tonic" => asserted.or_else(|| str_eq(ctx, facts::FUNCTION_CLASS, "tonic")),
        "function_is_predominant" => {
            asserted.or_else(|| str_eq(ctx, facts::FUNCTION_CLASS, "predominant"))
        }
        "function_is_dominant" => {
            asserted.or_else(|| str_eq(ctx, facts::FUNCTION_CLASS, "dominant"))
        }
        "root_motion_is_descending_fifth" => asserted.or_else(|| match root_motion_pc(ctx) {
            Some(pc) => Tri::known(pc == 5),
            None => Tri::Unknown,
        }),
        "root_motion_is_stepwise" => asserted.or_else(|| match root_motion_pc(ctx) {
            Some(pc) => Tri::known(matches!(pc, 1 | 2 | 10 | 11)),
            None => Tri::Unknown,
        }),
        "root_motion_is_third_related" => asserted.or_else(|| match root_motion_pc(ctx) {
            Some(pc) => Tri::known(matches!(pc, 3 | 4 | 8 | 9)),
            None => Tri::Unknown,
        }),
        "chord_is_diatonic_to_key"
        | "chord_is_borrowed_from_parallel_mode"
        | "chord_is_applied_dominant"
        | "chord_is_tritone_substitute"
        | "target_chord_follows"
        | "cadence_is_expected_at_this_slot"
        | "cadential_six_four_present"
        | "chord_is_in_root_position" => asserted,

        // ---- context and user intent ----
        "key_is_minor" => asserted.or_else(|| {
            str_in(
                ctx,
                facts::KEY_MODE,
                &[
                    "minor",
                    "natural_minor",
                    "harmonic_minor",
                    "melodic_minor",
                    "aeolian",
                    "dorian",
                    "phrygian",
                    "locrian",
                ],
            )
        }),
        "modal_center_is_active" => {
            asserted.or_else(|| str_eq(ctx, facts::KEY_CENTER_KIND, "modal"))
        }
        "harmonic_rhythm_is_fast" => {
            asserted.or_else(|| num_cmp(ctx, facts::CHORDS_PER_BAR, |v| v > 1.0))
        }
        "harmonic_rhythm_is_slow" => {
            asserted.or_else(|| num_cmp(ctx, facts::CHORDS_PER_BAR, |v| v < 1.0))
        }
        // The documented exact negation of `intentional_cluster`.
        "cluster_intent_is_false" => {
            asserted.or_else(|| evaluate_or_unknown("intentional_cluster", ctx).not())
        }
        "intentional_cluster" => asserted,
        "quartal_or_planing_context" => asserted.or_else(|| {
            str_in(ctx, facts::VOICING_FAMILY, &["quartal", "quintal"])
                .or(ctx.get_bool("planing_context").into())
        }),
        "complexity_target_is_high" => {
            asserted.or_else(|| num_cmp(ctx, facts::COMPLEXITY_TARGET, |v| v >= 0.66))
        }
        "chromaticism_target_is_high" => {
            asserted.or_else(|| num_cmp(ctx, facts::CHROMATICISM_TARGET, |v| v >= 0.66))
        }
        "extension_density_is_high" => {
            asserted.or_else(|| num_cmp(ctx, facts::EXTENSION_DENSITY, |v| v >= 0.66))
        }
        "strictness_is_common_practice" => {
            asserted.or_else(|| str_eq(ctx, facts::STRICTNESS, "common_practice_strict"))
        }
        "pedal_point_is_active" => asserted,

        // ---- arrangement ----
        "role_is_bass" => asserted.or_else(|| str_eq(ctx, facts::ROLE, "bass")),
        "role_is_lead" => asserted.or_else(|| str_eq(ctx, facts::ROLE, "lead")),
        "role_is_pad_or_sustained" => {
            asserted.or_else(|| str_in(ctx, facts::ROLE, &["pad", "texture", "ambience"]))
        }
        "part_density_exceeds_role_target" => asserted.or_else(|| {
            num_pair(
                ctx,
                facts::PART_DENSITY,
                facts::ROLE_DENSITY_TARGET,
                |got, target| got > target,
            )
        }),
        "onsets_collide_with_higher_priority_role" => {
            asserted.or_else(|| num_cmp(ctx, facts::ONSET_COLLISION_RATIO, |v| v > 0.5))
        }
        "register_overlaps_another_part" => {
            asserted.or_else(|| num_cmp(ctx, facts::REGISTER_OVERLAP_SEMITONES, |v| v > 5.0))
        }
        "polyphony_exceeds_profile_limit" => asserted.or_else(|| {
            num_pair(
                ctx,
                facts::PART_POLYPHONY,
                facts::INSTRUMENT_POLYPHONY,
                |got, limit| got > limit,
            )
        }),
        "part_duplicates_melody_rhythm"
        | "phrase_contains_no_rest"
        | "doubling_is_allowed_for_role"
        | "essential_chord_tone_only_in_this_layer"
        | "contrast_is_dynamics_only" => asserted,

        // ---- loop ----
        "final_chord_is_dominant_function" => {
            asserted.or_else(|| str_eq(ctx, facts::FINAL_CHORD_FUNCTION, "dominant"))
        }
        "final_chord_is_tonic_function" => {
            asserted.or_else(|| str_eq(ctx, facts::FINAL_CHORD_FUNCTION, "tonic"))
        }
        "first_chord_is_tonic_function" => {
            asserted.or_else(|| str_eq(ctx, facts::FIRST_CHORD_FUNCTION, "tonic"))
        }
        "note_extends_past_loop_end" => asserted.or_else(|| {
            num_pair(
                ctx,
                facts::MAX_NOTE_END_QN,
                facts::LOOP_END_QN,
                |end, loop_end| end > loop_end,
            )
        }),
        "note_carry_policy_is_explicit" => {
            asserted.or_else(|| str_in(ctx, facts::NOTE_CARRY_POLICY, &["carry", "split"]))
        }
        "bass_leaps_across_wrap" => asserted
            .or_else(|| num_cmp(ctx, facts::WRAP_BASS_INTERVAL_SEMITONES, |v| v.abs() > 7.0)),
        "harmonic_rhythm_changes_at_wrap" => asserted.or_else(|| {
            num_pair(
                ctx,
                facts::SLOT_LENGTH_BEFORE_WRAP_QN,
                facts::SLOT_LENGTH_AFTER_WRAP_QN,
                |before, after| before != after,
            )
        }),
        // Exact rational comparison, never a float epsilon: both facts come
        // from `BeatTime`, whose values are exact.
        // Stated in the negative so the length invariant fires when the span has
        // drifted, rather than when it is correct. Exact rational comparison,
        // never a float epsilon: both facts come from `BeatTime`.
        "loop_length_is_not_exact" => asserted.or_else(|| {
            num_pair(
                ctx,
                facts::GENERATED_LENGTH_QN,
                facts::REQUESTED_LOOP_LENGTH_QN,
                |got, want| got != want,
            )
        }),
        "is_loop_wrap_boundary" | "pickup_is_present" | "pedal_continues_across_wrap" => asserted,

        // ---- integrity ----
        "midi_pitch_out_of_range" => asserted.or_else(|| {
            num_cmp(ctx, facts::MIN_NOTE_MIDI, |v| v < 0.0).or(num_cmp(
                ctx,
                facts::MAX_NOTE_MIDI,
                |v| v > 127.0,
            ))
        }),
        "duration_is_not_positive" => {
            asserted.or_else(|| num_cmp(ctx, facts::MIN_NOTE_DURATION_QN, |v| v <= 0.0))
        }
        "notes_overlap_in_monophonic_part"
        | "note_outside_item_bounds"
        | "source_material_would_be_mutated"
        | "chord_symbol_is_ambiguous"
        | "omission_changes_semantic_identity" => asserted,

        // ---- search and diversity ----
        "candidate_duplicates_existing_strategy" => asserted,

        other => {
            return Err(KbError::unknown_predicate(
                "",
                format!(
                    "'{other}' is not in the closed predicate vocabulary this build implements"
                ),
            ))
        }
    };
    Ok(t)
}

/// Evaluates a predicate that is known to be in the vocabulary, mapping the
/// impossible error case to `Unknown`. Used only for the two predicates
/// defined as exact negations of another.
fn evaluate_or_unknown(name: &str, ctx: &RuleContext) -> Tri {
    evaluate_predicate(name, ctx).unwrap_or(Tri::Unknown)
}

// ---------------------------------------------------------------------------
// the engine
// ---------------------------------------------------------------------------

/// The aggregate result of evaluating every rule that listens for one event.
#[derive(Clone, Debug, Default)]
pub struct RuleOutcome {
    /// One entry per rule whose trigger event matched, hard kinds first.
    pub applications: Vec<RuleApplication>,
    /// Ids of rules that failed a hard constraint. Non-empty means the caller
    /// must reject the candidate before any soft scoring is used.
    pub hard_violations: Vec<String>,
    /// Summed deltas per score component, after profile multipliers.
    pub deltas: BTreeMap<String, f64>,
    /// Distinct source ids behind the rules that actually informed the result,
    /// in first-seen order.
    pub source_ids: Vec<String>,
}

impl RuleOutcome {
    /// True when no hard constraint failed.
    pub fn is_acceptable(&self) -> bool {
        self.hard_violations.is_empty()
    }

    /// The delta accumulated for one component.
    pub fn delta(&self, component: &str) -> f64 {
        self.deltas.get(component).copied().unwrap_or(0.0)
    }

    /// Every application with the given status, in order.
    pub fn with_status(&self, status: RuleStatus) -> Vec<&RuleApplication> {
        self.applications
            .iter()
            .filter(|a| a.status == status)
            .collect()
    }

    /// JSON form for a decision trace.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert(
            "applications",
            Json::Arr(self.applications.iter().map(|a| a.to_json()).collect()),
        );
        m.insert(
            "hard_violations",
            Json::Arr(
                self.hard_violations
                    .iter()
                    .map(|v| Json::Str(v.clone()))
                    .collect(),
            ),
        );
        let mut d = JsonMap::new();
        for (k, v) in &self.deltas {
            d.insert(k.clone(), Json::Float(*v));
        }
        m.insert("deltas", Json::Obj(d));
        m.insert(
            "source_ids",
            Json::Arr(
                self.source_ids
                    .iter()
                    .map(|s| Json::Str(s.clone()))
                    .collect(),
            ),
        );
        Json::Obj(m)
    }
}

/// Executes the rule base for one profile.
pub struct RuleEngine<'k> {
    kb: &'k KnowledgeBase,
    profile: &'k ResolvedProfile,
}

impl<'k> RuleEngine<'k> {
    /// Binds the engine to a knowledge base and a resolved profile.
    pub fn new(kb: &'k KnowledgeBase, profile: &'k ResolvedProfile) -> Self {
        RuleEngine { kb, profile }
    }

    /// The closed predicate vocabulary this build understands, sorted.
    pub fn known_predicates() -> &'static [&'static str] {
        predicate_names()
    }

    /// The profile this engine scores with.
    pub fn profile(&self) -> &ResolvedProfile {
        self.profile
    }

    /// Evaluates every rule whose trigger event matches the context's event.
    ///
    /// Hard kinds run first so a caller can inspect
    /// [`RuleOutcome::hard_violations`] and reject the candidate without ever
    /// looking at the soft scores.
    pub fn evaluate(&self, ctx: &RuleContext) -> RuleOutcome {
        let mut outcome = RuleOutcome::default();
        let Some(event) = ctx.event() else {
            return outcome;
        };
        let matching: Vec<&TheoryRule> = self
            .kb
            .rules()
            .iter()
            .filter(|r| r.trigger.event == event)
            .collect();
        let ordered = matching
            .iter()
            .filter(|r| r.kind.is_hard())
            .chain(matching.iter().filter(|r| !r.kind.is_hard()));

        for rule in ordered {
            let app = self.evaluate_rule(rule, ctx);
            match app.status {
                RuleStatus::Violated => outcome.hard_violations.push(app.rule_id.clone()),
                RuleStatus::Applied => {
                    *outcome
                        .deltas
                        .entry(rule.effect.score_component.clone())
                        .or_insert(0.0) += app.score_delta;
                }
                RuleStatus::Bypassed | RuleStatus::NotApplicable => {}
            }
            if app.status != RuleStatus::NotApplicable {
                for s in &app.source_ids {
                    if !outcome.source_ids.iter().any(|e| e == s) {
                        outcome.source_ids.push(s.clone());
                    }
                }
            }
            outcome.applications.push(app);
        }
        outcome
    }

    /// Evaluates a single rule, producing the trace entry a candidate carries.
    pub fn evaluate_rule(&self, rule: &TheoryRule, ctx: &RuleContext) -> RuleApplication {
        let source_ids = rule.source_ids();

        // 1 + 2: the trigger.
        if ctx.event() != Some(rule.trigger.event) {
            return self.not_applicable(
                rule,
                Vec::new(),
                format!(
                    "{} This is only weighed at the {} stage.",
                    rule.summary,
                    rule.trigger.event.id().replace('_', " ")
                ),
            );
        }
        if let Some(reason) = self.unmatched_selector(rule, ctx) {
            return self.not_applicable(rule, Vec::new(), format!("{} {reason}", rule.summary));
        }

        // 3: profile scope.
        if !self.profile.applies_to(rule) {
            return self.not_applicable(
                rule,
                Vec::new(),
                format!(
                    "The {} profile does not use this rule: {}",
                    self.profile.id, rule.summary
                ),
            );
        }

        // 4: an explicit disable short-circuits.
        if !self.profile.is_rule_enabled(&rule.id) {
            return self.not_applicable(
                rule,
                Vec::new(),
                format!(
                    "The {} profile switches this rule off: {} {}",
                    self.profile.id, rule.summary, rule.rationale
                ),
            );
        }

        // 5: exceptions bypass even when the conditions would have matched.
        let (matched_exceptions, _unknown_exceptions) = self.partition(&rule.exceptions, ctx);
        let (matched_conditions, condition_state) = self.conditions(&rule.conditions, ctx);
        if !matched_exceptions.is_empty() {
            let explanation = format!(
                "{} Set aside here because {}.{}",
                rule.summary,
                join_phrases(&matched_exceptions),
                if rule.rationale.is_empty() {
                    String::new()
                } else {
                    format!(" {}", rule.rationale)
                }
            );
            return RuleApplication {
                rule_id: rule.id.clone(),
                status: RuleStatus::Bypassed,
                score_delta: 0.0,
                matched_conditions,
                matched_exceptions,
                source_ids,
                explanation,
            };
        }

        // 6: every condition must hold.
        match condition_state {
            ConditionState::Failed(ref failed) => {
                let explanation = format!(
                    "{} Not in play here: {} does not hold.",
                    rule.summary,
                    join_phrases(std::slice::from_ref(failed))
                );
                return self.not_applicable(rule, matched_conditions, explanation);
            }
            ConditionState::Unknown(ref unknown) => {
                let explanation = format!(
                    "{} Not evaluated: the analysis cannot yet tell whether {}.",
                    rule.summary,
                    join_phrases(unknown)
                );
                return self.not_applicable(rule, matched_conditions, explanation);
            }
            ConditionState::AllTrue => {}
        }

        // 7: apply.
        let multiplier = self.profile.rule_multiplier(&rule.id);
        let because = if matched_conditions.is_empty() {
            String::new()
        } else {
            format!(" Here, {}.", join_phrases(&matched_conditions))
        };

        if rule.kind.rejects() && rule.effect.score_delta < 0.0 {
            let explanation = format!(
                "{}{} That is a hard {} constraint, so this candidate is rejected rather than scored. {}",
                rule.summary,
                because,
                rule.kind.id().replace('_', " "),
                rule.rationale
            );
            return RuleApplication {
                rule_id: rule.id.clone(),
                status: RuleStatus::Violated,
                score_delta: 0.0,
                matched_conditions,
                matched_exceptions,
                source_ids,
                explanation,
            };
        }

        let delta = rule.effect.score_delta * multiplier;
        let effect_text = if delta > 0.0 {
            format!(
                " Credited {:+.2} to {}.",
                delta,
                rule.effect.score_component.replace('_', " ")
            )
        } else if delta < 0.0 {
            format!(
                " Charged {:+.2} against {}.",
                delta,
                rule.effect.score_component.replace('_', " ")
            )
        } else {
            format!(
                " The {} profile zeroes its weight, so it is recorded without effect.",
                self.profile.id
            )
        };
        RuleApplication {
            rule_id: rule.id.clone(),
            status: RuleStatus::Applied,
            score_delta: delta,
            matched_conditions,
            matched_exceptions,
            source_ids,
            explanation: format!("{}{}{}", rule.summary, because, effect_text),
        }
    }

    /// Builds a `not_applicable` application.
    fn not_applicable(
        &self,
        rule: &TheoryRule,
        matched_conditions: Vec<String>,
        explanation: String,
    ) -> RuleApplication {
        RuleApplication {
            rule_id: rule.id.clone(),
            status: RuleStatus::NotApplicable,
            score_delta: 0.0,
            matched_conditions,
            matched_exceptions: Vec::new(),
            source_ids: rule.source_ids(),
            explanation,
        }
    }

    /// Splits a predicate list into the ones that hold and the ones the
    /// context cannot answer.
    fn partition(&self, predicates: &[String], ctx: &RuleContext) -> (Vec<String>, Vec<String>) {
        let mut matched = Vec::new();
        let mut unknown = Vec::new();
        for p in predicates {
            match evaluate_predicate(p, ctx) {
                Ok(Tri::True) => matched.push(p.clone()),
                Ok(Tri::Unknown) => unknown.push(p.clone()),
                // An unimplemented predicate cannot reach here: `validate`
                // rejects the bundle before the engine ever runs.
                Ok(Tri::False) | Err(_) => {}
            }
        }
        (matched, unknown)
    }

    /// Evaluates the condition list, stopping at the first failure so the
    /// explanation names one concrete reason rather than a list.
    fn conditions(
        &self,
        predicates: &[String],
        ctx: &RuleContext,
    ) -> (Vec<String>, ConditionState) {
        let mut matched = Vec::new();
        let mut unknown = Vec::new();
        for p in predicates {
            match evaluate_predicate(p, ctx) {
                Ok(Tri::True) => matched.push(p.clone()),
                Ok(Tri::False) => return (matched, ConditionState::Failed(p.clone())),
                Ok(Tri::Unknown) => unknown.push(p.clone()),
                Err(_) => unknown.push(p.clone()),
            }
        }
        if unknown.is_empty() {
            (matched, ConditionState::AllTrue)
        } else {
            (matched, ConditionState::Unknown(unknown))
        }
    }

    /// Returns a human reason when a trigger selector does not match.
    fn unmatched_selector(&self, rule: &TheoryRule, ctx: &RuleContext) -> Option<String> {
        for (key, want) in rule.trigger.selectors.iter() {
            let matched = match key {
                // The trigger asks what the *chord* declares, not what the
                // voicing happens to sound: `C11` has a third that a voicing
                // may omit, and the omission is what the exceptions are for.
                "contains_degrees" => {
                    let wanted: Vec<&str> = want
                        .as_arr()
                        .unwrap_or(&[])
                        .iter()
                        .filter_map(Json::as_str)
                        .collect();
                    match ctx
                        .get_list(facts::CHORD_TONE_DEGREES)
                        .or_else(|| ctx.get_list(facts::SOUNDING_DEGREES))
                    {
                        Some(have) => wanted.iter().all(|w| have.iter().any(|h| h == w)),
                        None => false,
                    }
                }
                "progression_tag" => match (want.as_str(), ctx.get_list(facts::PROGRESSION_TAGS)) {
                    (Some(w), Some(tags)) => tags.iter().any(|t| t == w),
                    _ => false,
                },
                _ => match selector_fact(key) {
                    Some(fact_key) => match (want.as_str(), ctx.get_str(fact_key)) {
                        (Some("any"), Some(_)) => true,
                        (Some(w), Some(have)) => w == have,
                        _ => false,
                    },
                    None => false,
                },
            };
            if !matched {
                return Some(format!(
                    "It only applies where {} is {}.",
                    key.replace('_', " "),
                    match want {
                        Json::Str(s) => s.clone(),
                        other => other.to_string(),
                    }
                ));
            }
        }
        None
    }
}

/// Maps a trigger selector key to the context fact key it reads.
///
/// `None` for a selector this build does not know, which the schema's closed
/// `propertyNames` list makes unreachable for valid data; the caller then
/// treats the selector as unmatched rather than as satisfied.
fn selector_fact(selector: &str) -> Option<&'static str> {
    match selector {
        "chord_family" => Some(facts::CHORD_FAMILY),
        "function_class" => Some(facts::FUNCTION_CLASS),
        "role" => Some(facts::ROLE),
        "scale_family" => Some(facts::SCALE_FAMILY),
        "voicing_family" => Some(facts::VOICING_FAMILY),
        "cadence_kind" => Some(facts::CADENCE_KIND),
        "loop_intent" => Some(facts::LOOP_INTENT),
        "note_role" => Some(facts::NOTE_ROLE),
        "motion" => Some(facts::MOTION),
        "interval" => Some(facts::INTERVAL),
        _ => None,
    }
}

/// Whether all conditions held.
enum ConditionState {
    /// Every condition evaluated `True`.
    AllTrue,
    /// This condition evaluated `False`.
    Failed(String),
    /// These conditions could not be evaluated.
    Unknown(Vec<String>),
}

/// Joins predicate clauses into readable English.
fn join_phrases(predicates: &[String]) -> String {
    let phrases: Vec<&str> = predicates.iter().map(|p| predicate_phrase(p)).collect();
    match phrases.len() {
        0 => String::new(),
        1 => phrases[0].to_string(),
        2 => format!("{} and {}", phrases[0], phrases[1]),
        n => format!("{}, and {}", phrases[..n - 1].join(", "), phrases[n - 1]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tri_kleene_tables() {
        assert_eq!(Tri::True.and(Tri::Unknown), Tri::Unknown);
        assert_eq!(Tri::False.and(Tri::Unknown), Tri::False);
        assert_eq!(Tri::True.or(Tri::Unknown), Tri::True);
        assert_eq!(Tri::False.or(Tri::Unknown), Tri::Unknown);
        assert_eq!(Tri::Unknown.not(), Tri::Unknown);
        assert_eq!(Tri::True.not(), Tri::False);
        assert_eq!(Tri::known(true), Tri::True);
        assert_eq!(Tri::from(None::<bool>), Tri::Unknown);
        assert_eq!(Tri::Unknown.or_else(|| Tri::True), Tri::True);
        assert_eq!(Tri::False.or_else(|| Tri::True), Tri::False);
        assert_eq!(Tri::Unknown.id(), "unknown");
        assert!(Tri::Unknown.is_unknown() && !Tri::Unknown.is_true());
    }

    #[test]
    fn the_vocabulary_is_sorted_and_unique() {
        let names = RuleEngine::known_predicates();
        assert_eq!(names.len(), PREDICATE_TABLE.len());
        for w in names.windows(2) {
            assert!(w[0] < w[1], "{} then {} is out of order", w[0], w[1]);
        }
    }

    #[test]
    fn every_predicate_has_a_human_phrase() {
        for (name, phrase) in PREDICATE_TABLE {
            assert!(!phrase.is_empty(), "{name} has no phrase");
            assert!(
                phrase.chars().next().is_some_and(|c| c.is_lowercase()),
                "{name}'s phrase should slot into a sentence"
            );
            assert!(
                !phrase.ends_with('.'),
                "{name}'s phrase should not end a sentence"
            );
        }
    }

    #[test]
    fn every_predicate_is_unknown_on_an_empty_context() {
        let ctx = RuleContext::new();
        for name in RuleEngine::known_predicates() {
            assert_eq!(
                evaluate_predicate(name, &ctx).unwrap(),
                Tri::Unknown,
                "{name} guessed on an empty context"
            );
        }
    }

    #[test]
    fn every_predicate_honours_an_explicit_assertion() {
        for name in RuleEngine::known_predicates() {
            let t = RuleContext::new().with_bool(name, true);
            assert_eq!(evaluate_predicate(name, &t).unwrap(), Tri::True, "{name}");
            let f = RuleContext::new().with_bool(name, false);
            assert_eq!(evaluate_predicate(name, &f).unwrap(), Tri::False, "{name}");
        }
    }

    #[test]
    fn an_unimplemented_predicate_is_an_error() {
        let e = evaluate_predicate("teleports_the_bass_player", &RuleContext::new()).unwrap_err();
        assert_eq!(e.code, "KB_UNKNOWN_PREDICATE");
    }

    #[test]
    fn context_upserts_preserve_insertion_order() {
        let mut ctx = RuleContext::new();
        ctx.set_num("a", 1.0);
        ctx.set_num("b", 2.0);
        ctx.set_num("a", 3.0);
        assert_eq!(ctx.get_num("a"), Some(3.0));
        assert_eq!(
            ctx.to_json().get("nums").unwrap().to_string(),
            r#"{"a":3.0,"b":2.0}"#
        );
    }

    #[test]
    fn degree_helpers() {
        assert_eq!(degree_number("b13"), Some(13));
        assert_eq!(degree_number("11"), Some(11));
        assert_eq!(degree_number("bb7"), Some(7));
        assert_eq!(degree_number("x"), None);
        assert!(degree_is_altered("#11") && !degree_is_altered("11"));
    }

    // --- derived predicates ---------------------------------------------

    fn t(name: &str, ctx: &RuleContext) -> Tri {
        evaluate_predicate(name, ctx).unwrap()
    }

    #[test]
    fn chord_content_predicates_derive_from_sounding_degrees() {
        let full = RuleContext::new().with_list(facts::SOUNDING_DEGREES, &["1", "3", "5", "b7"]);
        assert_eq!(t("third_is_omitted", &full), Tri::False);
        assert_eq!(t("fifth_is_omitted", &full), Tri::False);
        assert_eq!(t("root_is_omitted", &full), Tri::False);
        assert_eq!(t("seventh_is_present", &full), Tri::True);
        assert_eq!(t("guide_tones_present", &full), Tri::True);
        assert_eq!(t("chord_has_altered_tones", &full), Tri::True);
        assert_eq!(t("third_is_suspended", &full), Tri::False);
        assert_eq!(t("third_is_not_suspended", &full), Tri::True);

        let shell = RuleContext::new().with_list(facts::SOUNDING_DEGREES, &["3", "b7"]);
        assert_eq!(t("root_is_omitted", &shell), Tri::True);
        assert_eq!(t("fifth_is_omitted", &shell), Tri::True);

        let sus = RuleContext::new().with_list(facts::SOUNDING_DEGREES, &["1", "4", "5"]);
        assert_eq!(t("third_is_suspended", &sus), Tri::True);
        assert_eq!(t("third_is_not_suspended", &sus), Tri::False);
        assert_eq!(t("seventh_is_present", &sus), Tri::False);
        assert_eq!(t("chord_has_altered_tones", &sus), Tri::False);
    }

    #[test]
    fn cluster_intent_is_the_exact_negation_of_intentional_cluster() {
        let yes = RuleContext::new().with_bool("intentional_cluster", true);
        assert_eq!(t("cluster_intent_is_false", &yes), Tri::False);
        let no = RuleContext::new().with_bool("intentional_cluster", false);
        assert_eq!(t("cluster_intent_is_false", &no), Tri::True);
        assert_eq!(
            t("cluster_intent_is_false", &RuleContext::new()),
            Tri::Unknown
        );
    }

    #[test]
    fn register_predicates_use_documented_thresholds() {
        let low = RuleContext::new().with_num(facts::MAX_SOUNDING_MIDI, 47.0);
        assert_eq!(t("register_is_low", &low), Tri::True);
        let not_low = RuleContext::new().with_num(facts::MAX_SOUNDING_MIDI, 48.0);
        assert_eq!(t("register_is_low", &not_low), Tri::False);
        let high = RuleContext::new().with_num(facts::MIN_SOUNDING_MIDI, 80.0);
        assert_eq!(t("register_is_high", &high), Tri::True);
        let not_high = RuleContext::new().with_num(facts::MIN_SOUNDING_MIDI, 79.0);
        assert_eq!(t("register_is_high", &not_high), Tri::False);
    }

    #[test]
    fn spacing_predicates_consult_the_instrument_profile() {
        let ctx = RuleContext::new()
            .with_num(facts::MIN_ADJACENT_VOICE_INTERVAL, 4.0)
            .with_num(facts::INSTRUMENT_LOW_INTERVAL_LIMIT, 7.0);
        assert_eq!(
            t("spacing_violates_instrument_low_interval_limit", &ctx),
            Tri::True
        );
        assert_eq!(t("voices_are_widely_spaced", &ctx), Tri::False);

        let wide = RuleContext::new()
            .with_num(facts::MIN_ADJACENT_VOICE_INTERVAL, 7.0)
            .with_num(facts::INSTRUMENT_LOW_INTERVAL_LIMIT, 7.0);
        assert_eq!(t("voices_are_widely_spaced", &wide), Tri::True);
        assert_eq!(
            t("spacing_violates_instrument_low_interval_limit", &wide),
            Tri::False
        );

        // Only one half of the pair known: still Unknown, never a guess.
        let half = RuleContext::new().with_num(facts::MIN_ADJACENT_VOICE_INTERVAL, 4.0);
        assert_eq!(
            t("spacing_violates_instrument_low_interval_limit", &half),
            Tri::Unknown
        );
    }

    #[test]
    fn voice_range_predicate_checks_both_ends() {
        let below = RuleContext::new()
            .with_num(facts::MIN_SOUNDING_MIDI, 30.0)
            .with_num(facts::INSTRUMENT_LOW_MIDI, 40.0);
        assert_eq!(t("voice_exceeds_instrument_range", &below), Tri::True);
        let inside = RuleContext::new()
            .with_num(facts::MIN_SOUNDING_MIDI, 45.0)
            .with_num(facts::INSTRUMENT_LOW_MIDI, 40.0)
            .with_num(facts::MAX_SOUNDING_MIDI, 70.0)
            .with_num(facts::INSTRUMENT_HIGH_MIDI, 80.0);
        assert_eq!(t("voice_exceeds_instrument_range", &inside), Tri::False);
    }

    #[test]
    fn metric_weight_thresholds_are_distinct() {
        let strong = RuleContext::new().with_num(facts::METRIC_WEIGHT, 0.75);
        assert_eq!(t("melody_note_is_on_strong_beat", &strong), Tri::True);
        assert_eq!(t("note_is_on_weak_beat", &strong), Tri::False);
        assert_eq!(t("dissonance_is_on_strong_beat", &strong), Tri::True);

        let middling = RuleContext::new().with_num(facts::METRIC_WEIGHT, 0.6);
        assert_eq!(t("melody_note_is_on_strong_beat", &middling), Tri::False);
        assert_eq!(t("note_is_on_weak_beat", &middling), Tri::False);
        assert_eq!(t("dissonance_is_on_strong_beat", &middling), Tri::True);

        let weak = RuleContext::new().with_num(facts::METRIC_WEIGHT, 0.25);
        assert_eq!(t("note_is_on_weak_beat", &weak), Tri::True);
        assert_eq!(t("dissonance_is_on_strong_beat", &weak), Tri::False);
    }

    #[test]
    fn melody_degree_predicates() {
        let eleven = RuleContext::new()
            .with_str(facts::MELODY_CHORD_DEGREE, "11")
            .with_list(facts::CHORD_TONE_DEGREES, &["1", "3", "5", "7"]);
        assert_eq!(t("melody_is_11", &eleven), Tri::True);
        assert_eq!(t("melody_is_extension", &eleven), Tri::True);
        assert_eq!(t("melody_is_altered_tone", &eleven), Tri::False);
        assert_eq!(t("melody_is_chord_tone", &eleven), Tri::False);

        let flat_nine = RuleContext::new().with_str(facts::MELODY_CHORD_DEGREE, "b9");
        assert_eq!(t("melody_is_altered_tone", &flat_nine), Tri::True);
        assert_eq!(t("melody_is_extension", &flat_nine), Tri::False);
        assert_eq!(t("melody_is_11", &flat_nine), Tri::False);

        let third = RuleContext::new()
            .with_str(facts::MELODY_CHORD_DEGREE, "3")
            .with_list(facts::CHORD_TONE_DEGREES, &["1", "3", "5"]);
        assert_eq!(t("melody_is_chord_tone", &third), Tri::True);
    }

    #[test]
    fn melody_shape_predicates() {
        let leap = RuleContext::new().with_num(facts::MELODY_INTERVAL_SEMITONES, -9.0);
        assert_eq!(t("melody_leap_exceeds_fifth", &leap), Tri::True);
        assert_eq!(t("melody_is_repeated_pitch", &leap), Tri::False);
        let fifth = RuleContext::new().with_num(facts::MELODY_INTERVAL_SEMITONES, 7.0);
        assert_eq!(t("melody_leap_exceeds_fifth", &fifth), Tri::False);
        let same = RuleContext::new().with_num(facts::MELODY_INTERVAL_SEMITONES, 0.0);
        assert_eq!(t("melody_is_repeated_pitch", &same), Tri::True);

        let long = RuleContext::new().with_num(facts::MELODY_DURATION_RATIO, 2.0);
        assert_eq!(t("melody_note_is_long", &long), Tri::True);
        let short = RuleContext::new().with_num(facts::MELODY_DURATION_RATIO, 1.0);
        assert_eq!(t("melody_note_is_long", &short), Tri::False);
    }

    #[test]
    fn structural_salience_uses_the_supplied_threshold() {
        let default = RuleContext::new().with_num(facts::MELODY_SALIENCE, 0.7);
        assert_eq!(t("melody_note_is_structural", &default), Tri::True);
        let raised = RuleContext::new()
            .with_num(facts::MELODY_SALIENCE, 0.7)
            .with_num(facts::SALIENCE_THRESHOLD, 0.8);
        assert_eq!(t("melody_note_is_structural", &raised), Tri::False);
    }

    #[test]
    fn motion_predicates_read_one_shared_fact() {
        let ctx = RuleContext::new().with_str(facts::MOTION, "parallel");
        assert_eq!(t("motion_is_parallel", &ctx), Tri::True);
        assert_eq!(t("motion_is_similar", &ctx), Tri::False);
        assert_eq!(t("motion_is_contrary", &ctx), Tri::False);
        assert_eq!(t("motion_is_oblique", &ctx), Tri::False);
        let ctx = RuleContext::new().with_str(facts::MOTION, "oblique");
        assert_eq!(t("motion_is_oblique", &ctx), Tri::True);
    }

    #[test]
    fn perfect_interval_predicate_folds_compounds() {
        for s in [0.0, 7.0, 12.0, 19.0, 24.0] {
            let ctx = RuleContext::new().with_num(facts::ARRIVAL_INTERVAL_SEMITONES, s);
            assert_eq!(
                t("interval_is_perfect_fifth_or_octave", &ctx),
                Tri::True,
                "{s}"
            );
        }
        for s in [3.0, 4.0, 6.0, 10.0] {
            let ctx = RuleContext::new().with_num(facts::ARRIVAL_INTERVAL_SEMITONES, s);
            assert_eq!(
                t("interval_is_perfect_fifth_or_octave", &ctx),
                Tri::False,
                "{s}"
            );
        }
    }

    #[test]
    fn voice_leading_size_predicates() {
        let big = RuleContext::new().with_num(facts::VOICE_MOTION_SEMITONES, -13.0);
        assert_eq!(t("voice_leap_is_large", &big), Tri::True);
        let octave = RuleContext::new().with_num(facts::VOICE_MOTION_SEMITONES, 12.0);
        assert_eq!(t("voice_leap_is_large", &octave), Tri::False);
        let minimal = RuleContext::new()
            .with_num(facts::TOTAL_MOTION_SEMITONES, 5.0)
            .with_num(facts::MIN_AVAILABLE_MOTION_SEMITONES, 5.0);
        assert_eq!(t("total_motion_is_minimal", &minimal), Tri::True);
        let wasteful = RuleContext::new()
            .with_num(facts::TOTAL_MOTION_SEMITONES, 9.0)
            .with_num(facts::MIN_AVAILABLE_MOTION_SEMITONES, 5.0);
        assert_eq!(t("total_motion_is_minimal", &wasteful), Tri::False);
    }

    #[test]
    fn function_predicates_read_the_function_class() {
        let ctx = RuleContext::new().with_str(facts::FUNCTION_CLASS, "dominant");
        assert_eq!(t("function_is_dominant", &ctx), Tri::True);
        assert_eq!(t("function_is_tonic", &ctx), Tri::False);
        assert_eq!(t("function_is_predominant", &ctx), Tri::False);
        let ctx = RuleContext::new().with_str(facts::FUNCTION_CLASS, "predominant");
        assert_eq!(t("function_is_predominant", &ctx), Tri::True);
    }

    #[test]
    fn root_motion_predicates_partition_the_octave() {
        let down_fifth = RuleContext::new().with_num(facts::ROOT_MOTION_SEMITONES, -7.0);
        assert_eq!(t("root_motion_is_descending_fifth", &down_fifth), Tri::True);
        let up_fourth = RuleContext::new().with_num(facts::ROOT_MOTION_SEMITONES, 5.0);
        assert_eq!(t("root_motion_is_descending_fifth", &up_fourth), Tri::True);
        let step = RuleContext::new().with_num(facts::ROOT_MOTION_SEMITONES, 2.0);
        assert_eq!(t("root_motion_is_stepwise", &step), Tri::True);
        assert_eq!(t("root_motion_is_third_related", &step), Tri::False);
        let third = RuleContext::new().with_num(facts::ROOT_MOTION_SEMITONES, -4.0);
        assert_eq!(t("root_motion_is_third_related", &third), Tri::True);
        assert_eq!(t("root_motion_is_descending_fifth", &third), Tri::False);
    }

    #[test]
    fn key_and_modality_predicates() {
        for mode in ["aeolian", "dorian", "harmonic_minor", "minor"] {
            let ctx = RuleContext::new().with_str(facts::KEY_MODE, mode);
            assert_eq!(t("key_is_minor", &ctx), Tri::True, "{mode}");
        }
        let major = RuleContext::new().with_str(facts::KEY_MODE, "major");
        assert_eq!(t("key_is_minor", &major), Tri::False);
        let modal = RuleContext::new().with_str(facts::KEY_CENTER_KIND, "modal");
        assert_eq!(t("modal_center_is_active", &modal), Tri::True);
        let tonal = RuleContext::new().with_str(facts::KEY_CENTER_KIND, "tonal");
        assert_eq!(t("modal_center_is_active", &tonal), Tri::False);
    }

    #[test]
    fn harmonic_rhythm_predicates_are_not_complementary_at_one() {
        let one = RuleContext::new().with_num(facts::CHORDS_PER_BAR, 1.0);
        assert_eq!(t("harmonic_rhythm_is_fast", &one), Tri::False);
        assert_eq!(t("harmonic_rhythm_is_slow", &one), Tri::False);
        let fast = RuleContext::new().with_num(facts::CHORDS_PER_BAR, 2.0);
        assert_eq!(t("harmonic_rhythm_is_fast", &fast), Tri::True);
        let slow = RuleContext::new().with_num(facts::CHORDS_PER_BAR, 0.5);
        assert_eq!(t("harmonic_rhythm_is_slow", &slow), Tri::True);
    }

    #[test]
    fn control_targets_use_the_documented_066_threshold() {
        for (pred, key) in [
            ("complexity_target_is_high", facts::COMPLEXITY_TARGET),
            ("chromaticism_target_is_high", facts::CHROMATICISM_TARGET),
            ("extension_density_is_high", facts::EXTENSION_DENSITY),
        ] {
            assert_eq!(t(pred, &RuleContext::new().with_num(key, 0.66)), Tri::True);
            assert_eq!(t(pred, &RuleContext::new().with_num(key, 0.65)), Tri::False);
        }
    }

    #[test]
    fn quartal_context_accepts_either_evidence() {
        let quartal = RuleContext::new().with_str(facts::VOICING_FAMILY, "quartal");
        assert_eq!(t("quartal_or_planing_context", &quartal), Tri::True);
        let planing = RuleContext::new()
            .with_str(facts::VOICING_FAMILY, "close")
            .with_bool("planing_context", true);
        assert_eq!(t("quartal_or_planing_context", &planing), Tri::True);
        let neither = RuleContext::new()
            .with_str(facts::VOICING_FAMILY, "close")
            .with_bool("planing_context", false);
        assert_eq!(t("quartal_or_planing_context", &neither), Tri::False);
        // Family known but planing unknown: cannot conclude False.
        let partial = RuleContext::new().with_str(facts::VOICING_FAMILY, "close");
        assert_eq!(t("quartal_or_planing_context", &partial), Tri::Unknown);
    }

    #[test]
    fn rootless_and_strictness_predicates() {
        let ctx = RuleContext::new().with_str(facts::VOICING_FAMILY, "rootless");
        assert_eq!(t("chord_is_rootless_voicing", &ctx), Tri::True);
        let strict = RuleContext::new().with_str(facts::STRICTNESS, "common_practice_strict");
        assert_eq!(t("strictness_is_common_practice", &strict), Tri::True);
        let loose = RuleContext::new().with_str(facts::STRICTNESS, "relaxed");
        assert_eq!(t("strictness_is_common_practice", &loose), Tri::False);
    }

    #[test]
    fn arrangement_role_predicates() {
        let bass = RuleContext::new().with_str(facts::ROLE, "bass");
        assert_eq!(t("role_is_bass", &bass), Tri::True);
        assert_eq!(t("role_is_lead", &bass), Tri::False);
        assert_eq!(t("role_is_pad_or_sustained", &bass), Tri::False);
        for r in ["pad", "texture", "ambience"] {
            let ctx = RuleContext::new().with_str(facts::ROLE, r);
            assert_eq!(t("role_is_pad_or_sustained", &ctx), Tri::True, "{r}");
        }
    }

    #[test]
    fn arrangement_budget_predicates() {
        let busy = RuleContext::new()
            .with_num(facts::PART_DENSITY, 0.8)
            .with_num(facts::ROLE_DENSITY_TARGET, 0.4);
        assert_eq!(t("part_density_exceeds_role_target", &busy), Tri::True);
        let colliding = RuleContext::new().with_num(facts::ONSET_COLLISION_RATIO, 0.51);
        assert_eq!(
            t("onsets_collide_with_higher_priority_role", &colliding),
            Tri::True
        );
        let half = RuleContext::new().with_num(facts::ONSET_COLLISION_RATIO, 0.5);
        assert_eq!(
            t("onsets_collide_with_higher_priority_role", &half),
            Tri::False
        );
        let overlap = RuleContext::new().with_num(facts::REGISTER_OVERLAP_SEMITONES, 6.0);
        assert_eq!(t("register_overlaps_another_part", &overlap), Tri::True);
        let poly = RuleContext::new()
            .with_num(facts::PART_POLYPHONY, 4.0)
            .with_num(facts::INSTRUMENT_POLYPHONY, 1.0);
        assert_eq!(t("polyphony_exceeds_profile_limit", &poly), Tri::True);
    }

    #[test]
    fn loop_predicates() {
        let dom = RuleContext::new().with_str(facts::FINAL_CHORD_FUNCTION, "dominant");
        assert_eq!(t("final_chord_is_dominant_function", &dom), Tri::True);
        assert_eq!(t("final_chord_is_tonic_function", &dom), Tri::False);
        let first = RuleContext::new().with_str(facts::FIRST_CHORD_FUNCTION, "tonic");
        assert_eq!(t("first_chord_is_tonic_function", &first), Tri::True);

        let hanging = RuleContext::new()
            .with_num(facts::MAX_NOTE_END_QN, 32.5)
            .with_num(facts::LOOP_END_QN, 32.0);
        assert_eq!(t("note_extends_past_loop_end", &hanging), Tri::True);

        let carry = RuleContext::new().with_str(facts::NOTE_CARRY_POLICY, "split");
        assert_eq!(t("note_carry_policy_is_explicit", &carry), Tri::True);
        let default = RuleContext::new().with_str(facts::NOTE_CARRY_POLICY, "default");
        assert_eq!(t("note_carry_policy_is_explicit", &default), Tri::False);

        let leap = RuleContext::new().with_num(facts::WRAP_BASS_INTERVAL_SEMITONES, -8.0);
        assert_eq!(t("bass_leaps_across_wrap", &leap), Tri::True);

        let stable = RuleContext::new()
            .with_num(facts::SLOT_LENGTH_BEFORE_WRAP_QN, 2.0)
            .with_num(facts::SLOT_LENGTH_AFTER_WRAP_QN, 2.0);
        assert_eq!(t("harmonic_rhythm_changes_at_wrap", &stable), Tri::False);
    }

    #[test]
    fn loop_length_uses_exact_equality() {
        let exact = RuleContext::new()
            .with_num(facts::GENERATED_LENGTH_QN, 32.0)
            .with_num(facts::REQUESTED_LOOP_LENGTH_QN, 32.0);
        // Stated in the negative: the invariant must fire on drift, not on
        // correctness. A matching length is therefore False, not True.
        assert_eq!(t("loop_length_is_not_exact", &exact), Tri::False);
        let off = RuleContext::new()
            .with_num(facts::GENERATED_LENGTH_QN, 32.0 + f64::EPSILON * 32.0)
            .with_num(facts::REQUESTED_LOOP_LENGTH_QN, 32.0);
        assert_eq!(t("loop_length_is_not_exact", &off), Tri::True);
    }

    #[test]
    fn integrity_predicates() {
        let low = RuleContext::new().with_num(facts::MIN_NOTE_MIDI, -1.0);
        assert_eq!(t("midi_pitch_out_of_range", &low), Tri::True);
        let high = RuleContext::new().with_num(facts::MAX_NOTE_MIDI, 128.0);
        assert_eq!(t("midi_pitch_out_of_range", &high), Tri::True);
        let ok = RuleContext::new()
            .with_num(facts::MIN_NOTE_MIDI, 0.0)
            .with_num(facts::MAX_NOTE_MIDI, 127.0);
        assert_eq!(t("midi_pitch_out_of_range", &ok), Tri::False);
        let zero = RuleContext::new().with_num(facts::MIN_NOTE_DURATION_QN, 0.0);
        assert_eq!(t("duration_is_not_positive", &zero), Tri::True);
        let positive = RuleContext::new().with_num(facts::MIN_NOTE_DURATION_QN, 0.25);
        assert_eq!(t("duration_is_not_positive", &positive), Tri::False);
    }

    #[test]
    fn join_phrases_reads_as_english() {
        assert_eq!(join_phrases(&[]), "");
        assert_eq!(
            join_phrases(&["third_is_omitted".to_string()]),
            "the third is omitted from the voicing"
        );
        assert_eq!(
            join_phrases(&["role_is_bass".to_string(), "register_is_low".to_string()]),
            "the part is the bass and the voicing sits low"
        );
        let three = join_phrases(&[
            "role_is_bass".to_string(),
            "register_is_low".to_string(),
            "seventh_is_present".to_string(),
        ]);
        assert!(
            three.contains(", and a chordal seventh is sounding"),
            "{three}"
        );
    }

    #[test]
    fn predicate_phrase_falls_back_to_the_raw_name() {
        assert_eq!(predicate_phrase("not_a_predicate"), "not_a_predicate");
        assert!(is_known_predicate("melody_is_11"));
        assert!(!is_known_predicate("melody_is_12"));
    }
}
