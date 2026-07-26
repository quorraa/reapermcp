//! The loop audit: pipeline stage 10 and brief §10.9.
//!
//! [`audit`] measures the seam, executes every `loop_boundary` rule in
//! `knowledge/rules/looping.json` through [`theory_kb::RuleEngine`], judges the
//! result against the caller's [`LoopIntent`], and reports. It never changes a
//! note — [`crate::boundary::apply_boundary_policy`] is the only mutating
//! function in the crate, and its policy is the caller's choice.
//!
//! # Facts and rules
//!
//! A rule whose selector or condition facts are never set silently never fires,
//! which would leave the audit looking as though it consulted the knowledge
//! base when it did nothing of the sort. [`boundary_context`] therefore sets
//! every fact the fourteen looping rules read, and
//! [`LoopAudit::rule_status`] lets a test assert that a named rule genuinely
//! reached `Applied` or `Bypassed` rather than `NotApplicable`.
//!
//! Two facts are deliberately **left unset**, and both are exceptions rather
//! than conditions, so leaving them unknown widens rather than narrows what
//! fires:
//!
//! * `role` — the wrap is a property of the whole loop, not of one part, so
//!   there is no honest value for "the part under examination". Setting it to
//!   `"bass"` would make `looping.bass_continuity_across_wrap` bypass itself
//!   through its own `role_is_bass` exception in exactly the case it exists to
//!   catch.
//! * `voicing_family` — no single voicing family describes a whole loop.

use crate::boundary::{
    self, carried_notes, crossing_notes, hanging_notes, occupied_length, pickup_length,
    tail_length, LoopSpan,
};
use crate::intent::{self, IntentFit, SeamIntegrity};
use crate::wrap::{self, KeyFrame, WrapObservation};
use music_analysis::report::Analysis;
use music_domain::prelude::*;
use theory_kb::prelude::*;
use theory_kb::rules::facts;
use theory_kb::RuleEvent;

/// Score at or above which a loop is reported as compatible with its intent.
pub const COMPATIBLE_THRESHOLD: f64 = 0.6;

/// The knowledge-base rule whose stated condition is the *satisfied* form of
/// its own invariant.
///
/// `looping.exact_length_is_preserved` is a `mathematical_invariant` carrying a
/// `-1000` effect, and its single condition is `loop_length_is_exact` — the
/// predicate that is true when the length is *correct*. The rule engine
/// therefore reports it as `Violated` precisely when nothing is wrong, and can
/// never report it when the length has actually drifted.
///
/// This crate still executes the rule with honest facts, because suppressing it
/// would hide the defect. It does not, however, let that inverted status reach
/// the report: the length invariant is checked here with exact [`BeatTime`]
/// equality, which is the guarantee the product actually needs. The knowledge
/// defect is recorded rather than worked around silently.
pub const INVERTED_LENGTH_RULE: &str = "looping.exact_length_is_preserved";

/// Everything the audit reads.
pub struct LoopInput<'a> {
    /// The loop region and what it is for.
    pub span: LoopSpan,
    /// Every note in scope, including any pickup and any overhang.
    pub notes: &'a NoteSet,
    /// The harmony over the loop.
    pub chords: &'a [ChordEvent],
    /// The arrangement layers, when the caller has them. Without parts the
    /// layer-removal question is unanswerable and is reported as such.
    pub parts: &'a [Part],
    /// The tempo and meter map.
    pub time_map: &'a TimeMap,
    /// A full analysis, when one was run. Its key reading wins over anything
    /// this crate could infer from the loop alone.
    pub analysis: Option<&'a Analysis>,
}

impl LoopInput<'_> {
    /// Checks the span before anything is measured.
    pub fn validate(&self) -> Result<(), crate::error::LoopError> {
        self.span.validate()
    }
}

/// The audit, plus everything it used to reach its verdict.
///
/// [`audit`] returns the frozen [`LoopReport`]; this is the same run with the
/// working shown, which is what the repair suggestions and the tests read.
pub struct LoopAudit {
    /// The frozen report.
    pub report: LoopReport,
    /// The tonal frame the wrap was judged in.
    pub key: KeyFrame,
    /// Every measurement taken at the seam.
    pub observation: WrapObservation,
    /// The integrity facts fed to the intent criteria.
    pub integrity: SeamIntegrity,
    /// How well the material serves the declared intent.
    pub fit: IntentFit,
    /// Every looping rule that was consulted.
    pub outcome: RuleOutcome,
    /// The facts the rules were evaluated against.
    pub context: RuleContext,
    /// The carry policy the material implies.
    pub carry_policy: &'static str,
    /// Notes that cross the seam because the caller asked them to.
    pub carried: Vec<NoteId>,
    /// True when the material occupies exactly the requested span.
    pub length_exact: bool,
}

impl LoopAudit {
    /// The status the named rule reached, or `None` when it was not consulted.
    pub fn rule_status(&self, rule_id: &str) -> Option<RuleStatus> {
        self.outcome
            .applications
            .iter()
            .find(|a| a.rule_id == rule_id)
            .map(|a| a.status)
    }

    /// The explanation the named rule produced.
    pub fn rule_explanation(&self, rule_id: &str) -> Option<&str> {
        self.outcome
            .applications
            .iter()
            .find(|a| a.rule_id == rule_id)
            .map(|a| a.explanation.as_str())
    }

    /// Every rule that reached `Applied`, in evaluation order.
    pub fn applied_rules(&self) -> Vec<&str> {
        self.outcome
            .applications
            .iter()
            .filter(|a| a.status == RuleStatus::Applied)
            .map(|a| a.rule_id.as_str())
            .collect()
    }

    /// The summed `loop_compatibility` delta, before clamping.
    pub fn rule_points(&self) -> f64 {
        self.outcome.delta("loop_compatibility")
    }

    /// JSON form of the working, for a decision trace.
    pub fn to_json(&self) -> qjson::Json {
        qjson::json_obj! {
            "report" => self.report.to_json(),
            "key" => qjson::json_obj! {
                "tonic" => self.key.label(),
                "scale_id" => self.key.scale_id.clone(),
                "is_modal" => self.key.is_modal,
                "confidence" => self.key.confidence,
                "source" => self.key.source,
            },
            "intent_fit" => self.fit.to_json(),
            "carry_policy" => self.carry_policy,
            "rules" => self.outcome.to_json(),
        }
    }
}

/// Builds the `loop_boundary` rule context.
///
/// Every fact the fourteen looping rules read is set here. See the module
/// documentation for the two that are deliberately left unset.
pub fn boundary_context(
    kb: &KnowledgeBase,
    key: &KeyFrame,
    span: &LoopSpan,
    o: &WrapObservation,
    i: &SeamIntegrity,
    carry_policy: &str,
    max_note_end: BeatTime,
    occupied: BeatTime,
    cadence_expected: bool,
) -> RuleContext {
    let mut rc = RuleContext::new();
    rc.set_event(RuleEvent::LoopBoundary);

    // --- selectors ---
    rc.set_str(facts::LOOP_INTENT, span.intent.id());
    rc.set_str(facts::KEY_MODE, &key.scale_id);
    if let Some(def) = kb.scale(&key.scale_id) {
        rc.set_str(facts::SCALE_FAMILY, &def.family);
    }
    rc.set_str(
        facts::KEY_CENTER_KIND,
        if key.is_modal { "modal" } else { "tonal" },
    );

    // --- harmony at the seam ---
    if let Some(f) = o.final_function {
        rc.set_str(facts::FINAL_CHORD_FUNCTION, wrap::function_id(f));
    }
    if let Some(f) = o.first_function {
        rc.set_str(facts::FIRST_CHORD_FUNCTION, wrap::function_id(f));
    }
    rc.set_bool("modal_center_is_active", key.is_modal);
    rc.set_bool("key_is_minor", key.key_context_id() == "minor");
    rc.set_bool("is_loop_wrap_boundary", true);

    // --- notes at the seam ---
    rc.set_str(facts::NOTE_CARRY_POLICY, carry_policy);
    rc.set_num(facts::MAX_NOTE_END_QN, max_note_end.as_f64());
    rc.set_num(facts::LOOP_END_QN, span.end.as_f64());
    rc.set_bool("pickup_is_present", i.pickup);

    // --- the length invariant ---
    // Both numbers come from exact rationals, so `==` on the f64 renderings is
    // exact for every value a `BeatTime` can hold at musical magnitudes; the
    // authoritative comparison is still made on `BeatTime` itself in `audit`.
    rc.set_num(facts::GENERATED_LENGTH_QN, occupied.as_f64());
    rc.set_num(facts::REQUESTED_LOOP_LENGTH_QN, span.length().as_f64());

    // --- bass and voice leading ---
    if let Some(iv) = o.bass_interval {
        rc.set_num(facts::WRAP_BASS_INTERVAL_SEMITONES, f64::from(iv));
    }
    rc.set_bool("common_tone_available", o.common_tone_available);
    rc.set_bool("common_tone_retained", o.common_tone_retained);
    rc.set_bool("stepwise_connection_available", o.stepwise_available);
    rc.set_bool(
        "tendency_tone_is_unresolved",
        !o.unresolved_tendencies.is_empty(),
    );

    // --- pedal ---
    rc.set_bool("pedal_point_is_active", o.pedal_active);
    rc.set_bool("pedal_continues_across_wrap", o.pedal_continues);

    // --- harmonic rhythm ---
    if let Some(b) = o.slot_before {
        rc.set_num(facts::SLOT_LENGTH_BEFORE_WRAP_QN, b.as_f64());
    }
    if let Some(a) = o.slot_after {
        rc.set_num(facts::SLOT_LENGTH_AFTER_WRAP_QN, a.as_f64());
    }
    rc.set_bool("cadence_is_expected_at_this_slot", cadence_expected);

    // --- layers ---
    if o.layers_known {
        rc.set_bool(
            "essential_chord_tone_only_in_this_layer",
            o.layer_removal.is_some(),
        );
    }

    // Clusters are a request-level choice this crate never makes on the
    // caller's behalf, so the exception is reported as absent rather than
    // assumed.
    rc.set_bool("intentional_cluster", false);
    rc
}

/// The carry policy the material itself declares.
///
/// A note marked to carry is an explicit decision by whoever wrote it, and it
/// is what turns `looping.no_hanging_note_past_loop_end` from a fault into a
/// bypass.
fn implied_carry_policy(notes: &NoteSet, span: &LoopSpan) -> &'static str {
    if carried_notes(notes, span).is_empty() {
        "default"
    } else {
        "carry"
    }
}

/// A short description of the harmonic wrap.
fn harmonic_wrap_text(o: &WrapObservation) -> String {
    match (&o.final_symbol, &o.first_symbol) {
        (Some(last), Some(first)) => {
            let lf = o
                .final_function
                .map(|f| wrap::function_id(f))
                .unwrap_or("unclassified");
            let ff = o
                .first_function
                .map(|f| wrap::function_id(f))
                .unwrap_or("unclassified");
            let shape = if lf == "dominant" && ff == "tonic" {
                "an authentic cadence across the wrap"
            } else if lf == "tonic" && ff == "tonic" {
                "already closed before the wrap"
            } else if lf == "dominant" {
                "left open on the dominant"
            } else if o.common_tone_available {
                "connected by common tone rather than by function"
            } else {
                "a non-functional connection"
            };
            format!("{last} ({lf}) to {first} ({ff}): {shape}")
        }
        _ => "no harmony was supplied, so the wrap was judged on the notes alone".to_string(),
    }
}

/// A short description of the bass at the wrap.
fn bass_wrap_text(o: &WrapObservation) -> String {
    match (o.bass_end, o.bass_start, o.bass_interval) {
        (Some(a), Some(b), Some(iv)) => {
            let quality = if iv == 0 {
                "held"
            } else if iv.abs() <= 2 {
                "stepwise"
            } else if iv.abs() == 5 || iv.abs() == 7 {
                "by fifth"
            } else if iv.abs() <= 7 {
                "by leap within a fifth"
            } else {
                "by a leap wider than a fifth"
            };
            format!(
                "{} to {}, {iv:+} semitones, {quality}",
                SpelledPitch::from_midi(a, None).to_ascii(),
                SpelledPitch::from_midi(b, None).to_ascii()
            )
        }
        _ => "no bass material crosses the wrap".to_string(),
    }
}

/// A short description of the voice leading at the wrap.
fn voice_leading_wrap_text(o: &WrapObservation) -> String {
    if o.end_pitches.is_empty() || o.start_pitches.is_empty() {
        return "nothing sounds on both sides of the wrap".to_string();
    }
    let common = o.common_tone_pcs.len();
    format!(
        "{} voices into {}, total motion {} semitones, largest move {}, {} common pitch class{}{}",
        o.end_pitches.len(),
        o.start_pitches.len(),
        o.total_motion,
        o.max_leap,
        common,
        if common == 1 { "" } else { "es" },
        if o.common_tone_retained {
            ", one held in the same voice"
        } else {
            ""
        }
    )
}

/// Whether phrase analysis marks the final slot as a cadential arrival.
fn cadence_expected(analysis: Option<&Analysis>, span: &LoopSpan) -> bool {
    match analysis {
        Some(an) => an
            .grid
            .slots
            .iter()
            .filter(|s| s.start < span.end && s.end > span.start)
            .next_back()
            .map(|s| s.is_cadential)
            .unwrap_or(false),
        None => false,
    }
}

/// Audits a loop against its declared intent.
///
/// The verdict is always relative to the intent: a wrap that is excellent for
/// `closed_tonic` may be wrong for `modal_drone`, and neither reading is
/// promoted to a universal truth. Nothing is mutated and no repair is applied;
/// [`crate::suggest_repairs`] proposes, the caller disposes.
pub fn audit(kb: &KnowledgeBase, prof: &ResolvedProfile, input: &LoopInput<'_>) -> LoopReport {
    audit_detailed(kb, prof, input).report
}

/// [`audit`] with the working shown.
pub fn audit_detailed(
    kb: &KnowledgeBase,
    prof: &ResolvedProfile,
    input: &LoopInput<'_>,
) -> LoopAudit {
    let span = input.span;
    let mut findings: Vec<Warning> = Vec::new();

    if !span.is_valid() {
        findings.push(Warning::new(
            "LOOP_INVALID_SPAN",
            format!(
                "the loop end {} is not after the loop start {}, so there is no wrap to audit",
                span.end.to_display(),
                span.start.to_display()
            ),
            Severity::Major,
        ));
    }

    let key = wrap::infer_key(kb, input.notes, input.chords, input.time_map, input.analysis);
    let observation = wrap::observe(kb, &key, &span, input.notes, input.chords, input.parts);

    let hanging = hanging_notes(input.notes, &span);
    let crossing = crossing_notes(input.notes, &span);
    let carried = carried_notes(input.notes, &span);
    let pickup_qn = pickup_length(input.notes, &span);
    let tail_qn = tail_length(input.notes, &span);
    let occupied = occupied_length(input.notes, &span);

    // Exact rational equality, never a float tolerance: one tick of drift
    // compounds on every repeat.
    let length_exact = occupied == span.length();

    let integrity = SeamIntegrity {
        hanging: hanging.len(),
        crossing: crossing.len(),
        pickup: pickup_qn.is_positive(),
        length_exact,
        modal: key.is_modal,
    };

    let max_note_end = input
        .notes
        .notes
        .iter()
        .filter(|n| !n.muted && n.duration.is_positive())
        .map(|n| n.end())
        .max()
        .unwrap_or(span.end);
    let carry_policy = implied_carry_policy(input.notes, &span);

    let context = boundary_context(
        kb,
        &key,
        &span,
        &observation,
        &integrity,
        carry_policy,
        max_note_end,
        occupied,
        cadence_expected(input.analysis, &span),
    );
    let engine = RuleEngine::new(kb, prof);
    let outcome = engine.evaluate(&context);

    let fit = intent::evaluate(span.intent, &observation, &integrity);

    // Rule deltas are clamped so a single rule cannot swamp the intent
    // criteria; the raw sum stays visible on `LoopAudit::rule_points`.
    let rule_points = outcome.delta("loop_compatibility").clamp(-16.0, 12.0);
    let score = (0.15 + 0.72 * fit.fit + rule_points / 40.0).clamp(0.0, 1.0);

    let one_shot = span.intent == LoopIntent::OneShotEnding;

    // --- findings ---
    if !hanging.is_empty() {
        findings.push(Warning::new(
            "LOOP_HANGING_NOTE",
            format!(
                "{} note{} still sounding {} past the loop end; on the first repeat {} audible as \
                 a stuck voice",
                hanging.len(),
                if hanging.len() == 1 { " is" } else { "s are" },
                tail_qn.to_display(),
                if hanging.len() == 1 {
                    "it is"
                } else {
                    "they are"
                }
            ),
            if one_shot {
                Severity::Info
            } else {
                Severity::Major
            },
        ));
    }
    if !carried.is_empty() {
        findings.push(Warning::new(
            "LOOP_NOTE_CARRIED",
            format!(
                "{} note{} marked to carry across the boundary, which is a decision rather than a \
                 fault",
                carried.len(),
                if carried.len() == 1 { " is" } else { "s are" }
            ),
            Severity::Info,
        ));
    }
    if integrity.pickup {
        findings.push(Warning::new(
            "LOOP_PICKUP_PRESENT",
            format!(
                "{} of anacrusis sounds before the loop start; it must be wrapped into the tail or \
                 duplicated, never dropped",
                pickup_qn.to_display()
            ),
            Severity::Moderate,
        ));
    }
    if tail_qn.is_positive() && hanging.is_empty() && carried.is_empty() {
        findings.push(Warning::new(
            "LOOP_TAIL_PRESENT",
            format!("{} of material sounds after the loop end", tail_qn.to_display()),
            Severity::Info,
        ));
    }
    if !length_exact {
        findings.push(Warning::new(
            "LOOP_LENGTH_MISMATCH",
            format!(
                "the material occupies {} but the loop asks for {}; the difference is exact, not a \
                 rounding artefact",
                occupied.to_display(),
                span.length().to_display()
            ),
            Severity::Moderate,
        ));
    }
    if observation.bass_leaps {
        findings.push(Warning::new(
            "LOOP_BASS_DISCONTINUITY",
            format!(
                "the bass moves {} across the wrap, which is where a listener locates the loop \
                 point",
                bass_wrap_text(&observation)
            ),
            Severity::Moderate,
        ));
    }
    if !observation.end_pitches.is_empty()
        && !observation.start_pitches.is_empty()
        && !observation.stepwise_available
        && observation.max_leap > 4
    {
        findings.push(Warning::new(
            "LOOP_VOICE_LEADING_DISCONTINUITY",
            format!(
                "no voice-leading connection across the wrap keeps every voice within a step; the \
                 cheapest one still leaps {} semitones",
                observation.max_leap
            ),
            Severity::Moderate,
        ));
    }
    for t in &observation.unresolved_tendencies {
        findings.push(Warning::new(
            "LOOP_UNRESOLVED_TENDENCY",
            t.clone(),
            if span.intent == LoopIntent::OpenDominant || one_shot {
                Severity::Info
            } else {
                Severity::Minor
            },
        ));
    }
    if observation.harmonic_rhythm_changes {
        findings.push(Warning::new(
            "LOOP_HARMONIC_RHYTHM_CHANGE",
            format!(
                "the last slot lasts {} but the first lasts {}, so the loop stumbles at the wrap",
                observation
                    .slot_before
                    .map(|b| b.to_display())
                    .unwrap_or_default(),
                observation
                    .slot_after
                    .map(|a| a.to_display())
                    .unwrap_or_default()
            ),
            Severity::Moderate,
        ));
    }
    if let Some(removal) = &observation.layer_removal {
        findings.push(Warning::new(
            "LOOP_LAYER_REMOVAL",
            format!(
                "the part {:?} is the only source of the {} at the wrap and does not sound at the \
                 loop start",
                removal.part_name, removal.degree
            ),
            Severity::Major,
        ));
    }
    if observation.pedal_active && !observation.pedal_continues {
        findings.push(Warning::new(
            "LOOP_PEDAL_RESTART",
            "a pedal or drone stops at the loop end and re-articulates, which throws away the one \
             advantage a drone has in a loop"
                .to_string(),
            Severity::Minor,
        ));
    }
    if let Some(p) = &observation.percussion {
        findings.push(Warning::new(
            "LOOP_PERCUSSION_PHASE",
            format!(
                "{} percussion onset{} repeating every {}, first hit {} after the loop start",
                p.hits,
                if p.hits == 1 { "" } else { "s" },
                p.period_qn.to_display(),
                p.phase_offset_qn.to_display()
            ),
            if p.aligned {
                Severity::Info
            } else {
                Severity::Minor
            },
        ));
    }
    if fit.fit < 0.5 {
        findings.push(Warning::new(
            "LOOP_INTENT_MISMATCH",
            format!("for {} intent, {}", span.intent.id(), fit.summary),
            Severity::Moderate,
        ));
    }
    if input.notes.notes.is_empty() && input.chords.is_empty() {
        findings.push(Warning::new(
            "LOOP_NO_MATERIAL",
            "the loop span contains neither notes nor chords, so every wrap observation is empty"
                .to_string(),
            Severity::Moderate,
        ));
    }
    if !observation.layers_known {
        findings.push(Warning::new(
            "LOOP_LAYERS_UNKNOWN",
            "no arrangement parts were supplied, so whether a layer removal costs an essential \
             chord tone at the wrap could not be decided"
                .to_string(),
            Severity::Info,
        ));
    }

    let compatible = if one_shot {
        // A stinger is meant to stop; it is judged on doing that well.
        fit.fit >= COMPATIBLE_THRESHOLD && length_exact
    } else {
        score >= COMPATIBLE_THRESHOLD && hanging.is_empty() && length_exact
    };

    let evidence = 0.4
        + if input.chords.is_empty() { 0.0 } else { 0.2 }
        + if input.notes.notes.is_empty() { 0.0 } else { 0.2 };
    let confidence = (evidence + 0.2 * key.confidence).clamp(0.0, 1.0);

    let mut report = LoopReport {
        intent: Some(span.intent),
        loop_start: span.start,
        loop_end: span.end,
        compatible,
        score,
        harmonic_wrap: harmonic_wrap_text(&observation),
        bass_wrap: bass_wrap_text(&observation),
        voice_leading_wrap: voice_leading_wrap_text(&observation),
        hanging_notes: hanging,
        crossing_notes: crossing,
        pickup_qn,
        tail_qn,
        findings,
        repairs: Vec::new(),
        confidence,
    };
    report.repairs = crate::repair::repair_ids(&report, input);

    LoopAudit {
        report,
        key,
        observation,
        integrity,
        fit,
        outcome,
        context,
        carry_policy,
        carried,
        length_exact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    #[test]
    fn every_looping_rule_is_consulted() {
        let h = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice");
        let audit = h.audit();
        let consulted: Vec<&str> = audit
            .outcome
            .applications
            .iter()
            .map(|a| a.rule_id.as_str())
            .collect();
        for rule in testing::LOOPING_RULE_IDS {
            assert!(
                consulted.contains(rule),
                "{rule} was never evaluated at the loop boundary"
            );
        }
    }

    #[test]
    fn the_context_sets_every_fact_the_looping_rules_read() {
        let h = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice");
        let audit = h.audit();
        for key in [
            facts::LOOP_INTENT,
            facts::KEY_CENTER_KIND,
            facts::FINAL_CHORD_FUNCTION,
            facts::FIRST_CHORD_FUNCTION,
            facts::NOTE_CARRY_POLICY,
        ] {
            assert!(
                audit.context.get_str(key).is_some(),
                "string fact {key} was never set"
            );
        }
        for key in [
            facts::MAX_NOTE_END_QN,
            facts::LOOP_END_QN,
            facts::GENERATED_LENGTH_QN,
            facts::REQUESTED_LOOP_LENGTH_QN,
            facts::WRAP_BASS_INTERVAL_SEMITONES,
            facts::SLOT_LENGTH_BEFORE_WRAP_QN,
            facts::SLOT_LENGTH_AFTER_WRAP_QN,
        ] {
            assert!(
                audit.context.get_num(key).is_some(),
                "numeric fact {key} was never set"
            );
        }
        for key in [
            "is_loop_wrap_boundary",
            "pickup_is_present",
            "common_tone_available",
            "common_tone_retained",
            "stepwise_connection_available",
            "tendency_tone_is_unresolved",
            "pedal_point_is_active",
            "pedal_continues_across_wrap",
            "modal_center_is_active",
            "intentional_cluster",
            "cadence_is_expected_at_this_slot",
        ] {
            assert!(
                audit.context.get_bool(key).is_some(),
                "predicate {key} was never asserted"
            );
        }
    }

    #[test]
    fn the_role_fact_is_left_unset_so_the_bass_rule_can_fire() {
        let h = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice");
        let audit = h.audit();
        assert_eq!(
            audit.context.get_str(facts::ROLE),
            None,
            "setting `role` would let looping.bass_continuity_across_wrap bypass itself"
        );
    }

    #[test]
    fn an_inverted_span_is_reported_rather_than_panicking() {
        let h = testing::inverted_span();
        let report = h.audit().report;
        assert!(!report.compatible);
        assert!(report
            .findings
            .iter()
            .any(|f| f.code == "LOOP_INVALID_SPAN"));
    }

    #[test]
    fn the_same_input_produces_the_same_report_json() {
        let h = testing::dominant_wrap(LoopIntent::ClosedTonic, "jazz_standard");
        let a = h.audit().report.to_json().to_canonical_string();
        let b = h.audit().report.to_json().to_canonical_string();
        assert_eq!(a, b);
    }
}
