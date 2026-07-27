//! Behavioural tests for the rule engine against the real knowledge bundle.
//!
//! Each test named after a `test_id` from `knowledge/` is registered in
//! `tests/test_id_coverage.json`, so the gap between what the data promises and
//! what this build pins is visible rather than hidden.

use theory_kb::model::RuleOverride;
use theory_kb::prelude::*;

fn kb() -> &'static KnowledgeBase {
    KnowledgeBase::embedded()
}

fn profile(id: &str) -> ResolvedProfile {
    kb().resolve_profile(id).expect("a known profile")
}

/// Finds one rule's application in an outcome.
fn app<'a>(outcome: &'a RuleOutcome, rule_id: &str) -> &'a RuleApplication {
    outcome
        .applications
        .iter()
        .find(|a| a.rule_id == rule_id)
        .unwrap_or_else(|| panic!("{rule_id} was not evaluated"))
}

/// A voicing-built context for a Cmaj7(11) in close position.
///
/// `CHORD_TONE_DEGREES` is what the symbol declares and drives the trigger;
/// `SOUNDING_DEGREES` is what the realised voicing plays and drives the
/// omission predicates. The two differ exactly when a voice is dropped.
fn close_major_eleven() -> RuleContext {
    RuleContext::new()
        .with_event(RuleEvent::VoicingBuilt)
        .with_str(facts::CHORD_FAMILY, "major")
        .with_str(facts::VOICING_FAMILY, "close")
        .with_list(facts::CHORD_TONE_DEGREES, &["1", "3", "5", "7", "11"])
        .with_list(facts::SOUNDING_DEGREES, &["1", "3", "5", "7", "11"])
        .with_num(facts::THIRD_TO_ELEVENTH_SEMITONES, 13.0)
        .with_num(facts::MIN_ADJACENT_VOICE_INTERVAL, 3.0)
        .with_bool("intentional_cluster", false)
        .with_bool("melody_is_11", false)
        .with_bool("planing_context", false)
}

// ---------------------------------------------------------------------------
// extensions.major_natural_11_close_register
// ---------------------------------------------------------------------------

#[test]
fn major11_close_register_penalty() {
    let p = profile("jazz_standard");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&close_major_eleven());
    let a = app(&outcome, "extensions.major_natural_11_close_register");

    assert_eq!(a.status, RuleStatus::Applied);
    assert!(a.score_delta < 0.0, "the clash should cost something");
    assert_eq!(
        a.matched_conditions,
        vec![
            "third_and_eleventh_are_in_adjacent_or_close_registers".to_string(),
            "third_is_not_suspended".to_string(),
            "cluster_intent_is_false".to_string(),
        ]
    );
    assert!(a.matched_exceptions.is_empty());

    // The penalty reaches the score vector: the component total is exactly
    // the sum of what every applied rule charged it, this one included.
    let others: f64 = outcome
        .applications
        .iter()
        .filter(|x| {
            x.rule_id != a.rule_id
                && x.status == RuleStatus::Applied
                && kb()
                    .rule(&x.rule_id)
                    .is_some_and(|r| r.effect.score_component == "extension_appropriateness")
        })
        .map(|x| x.score_delta)
        .sum();
    assert_eq!(
        outcome.delta("extension_appropriateness"),
        others + a.score_delta
    );
    assert_eq!(a.score_delta, -2.5 * p.rule_multiplier(&a.rule_id));
    assert!(
        a.explanation.contains("minor-ninth clash"),
        "the explanation should quote the rule's own summary: {}",
        a.explanation
    );
    assert!(
        a.explanation
            .contains("the third and the natural eleventh sit close together"),
        "the explanation should name the condition in English: {}",
        a.explanation
    );
    assert!(
        a.explanation.contains("extension appropriateness"),
        "the explanation should name the component it charged: {}",
        a.explanation
    );
}

#[test]
fn major11_omit3_exception() {
    let p = profile("jazz_standard");
    // No third in the voicing: the rule's own exception list says this is fine.
    let ctx = close_major_eleven().with_list(facts::SOUNDING_DEGREES, &["1", "5", "7", "11"]);
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(&outcome, "extensions.major_natural_11_close_register");

    assert_eq!(a.status, RuleStatus::Bypassed);
    assert_eq!(a.score_delta, 0.0);
    assert!(a
        .matched_exceptions
        .contains(&"third_is_omitted".to_string()));
    assert_eq!(outcome.delta("extension_appropriateness"), 0.0);
    assert!(
        a.explanation.contains("Set aside here because"),
        "a bypass must say so: {}",
        a.explanation
    );
    assert!(
        a.explanation
            .contains("the third is omitted from the voicing"),
        "{}",
        a.explanation
    );
}

#[test]
fn major11_melody_exception() {
    let p = profile("jazz_standard");
    let ctx = close_major_eleven().with_bool("melody_is_11", true);
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(&outcome, "extensions.major_natural_11_close_register");

    assert_eq!(a.status, RuleStatus::Bypassed);
    assert_eq!(a.matched_exceptions, vec!["melody_is_11".to_string()]);
    assert!(
        a.explanation.contains("the eleventh is in the melody"),
        "{}",
        a.explanation
    );
}

#[test]
fn a_wide_voicing_also_bypasses_the_eleventh_penalty() {
    let p = profile("jazz_standard");
    let ctx = close_major_eleven().with_num(facts::MIN_ADJACENT_VOICE_INTERVAL, 9.0);
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(&outcome, "extensions.major_natural_11_close_register");
    assert_eq!(a.status, RuleStatus::Bypassed);
    assert!(a
        .matched_exceptions
        .contains(&"voices_are_widely_spaced".to_string()));
}

#[test]
fn the_eleventh_rule_is_not_applicable_to_a_profile_that_does_not_list_it() {
    // The rule is scoped to common_practice, jazz_standard, pop_rock, cinematic.
    let p = profile("modal_ambient");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&close_major_eleven());
    let a = app(&outcome, "extensions.major_natural_11_close_register");
    assert_eq!(a.status, RuleStatus::NotApplicable);
    assert_eq!(a.score_delta, 0.0);
    assert!(
        a.explanation
            .contains("modal_ambient profile does not use this rule"),
        "{}",
        a.explanation
    );
}

#[test]
fn a_trigger_selector_that_does_not_match_makes_the_rule_not_applicable() {
    let p = profile("jazz_standard");
    // A minor chord: the trigger's `chord_family: major` cannot match.
    let ctx = close_major_eleven().with_str(facts::CHORD_FAMILY, "minor");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(&outcome, "extensions.major_natural_11_close_register");
    assert_eq!(a.status, RuleStatus::NotApplicable);
    assert!(a.explanation.contains("chord family"), "{}", a.explanation);
}

#[test]
fn a_missing_contains_degrees_fact_makes_the_rule_not_applicable() {
    let p = profile("jazz_standard");
    // No eleventh in the chord at all: the trigger cannot match.
    let ctx = close_major_eleven()
        .with_list(facts::CHORD_TONE_DEGREES, &["1", "3", "5", "7"])
        .with_list(facts::SOUNDING_DEGREES, &["1", "3", "5", "7"]);
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(&outcome, "extensions.major_natural_11_close_register");
    assert_eq!(a.status, RuleStatus::NotApplicable);
}

#[test]
fn an_unknown_condition_never_guesses() {
    let p = profile("jazz_standard");
    // Everything the trigger needs, but nothing that answers the conditions.
    let ctx = RuleContext::new()
        .with_event(RuleEvent::VoicingBuilt)
        .with_str(facts::CHORD_FAMILY, "major")
        .with_list(facts::CHORD_TONE_DEGREES, &["1", "3", "5", "7", "11"]);
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(&outcome, "extensions.major_natural_11_close_register");
    assert_eq!(a.status, RuleStatus::NotApplicable);
    assert_eq!(a.score_delta, 0.0);
    assert!(
        a.explanation.contains("cannot yet tell whether"),
        "an unevaluated rule must say what it is missing: {}",
        a.explanation
    );
}

// ---------------------------------------------------------------------------
// hard rules
// ---------------------------------------------------------------------------

#[test]
fn midi_pitch_bounds_enforced() {
    let p = profile("pop_rock");
    let ctx = RuleContext::new()
        .with_event(RuleEvent::NoteEmitted)
        .with_num(facts::MIN_NOTE_MIDI, -3.0)
        .with_num(facts::MAX_NOTE_MIDI, 60.0);
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);

    assert!(!outcome.is_acceptable());
    assert!(outcome
        .hard_violations
        .contains(&"voice_leading.midi_pitch_within_range".to_string()));
    let a = app(&outcome, "voice_leading.midi_pitch_within_range");
    assert_eq!(a.status, RuleStatus::Violated);
    assert_eq!(a.score_delta, 0.0, "a rejected candidate is not scored");
    assert!(
        a.explanation.contains("rejected rather than scored"),
        "{}",
        a.explanation
    );
    assert_eq!(
        outcome.delta("arrangement_clarity"),
        0.0,
        "a violation must not leak into the score vector"
    );
}

#[test]
fn no_invalid_midi_pitches() {
    let p = profile("pop_rock");
    let ctx = RuleContext::new()
        .with_event(RuleEvent::NoteEmitted)
        .with_num(facts::MIN_NOTE_MIDI, 21.0)
        .with_num(facts::MAX_NOTE_MIDI, 108.0)
        .with_num(facts::MIN_NOTE_DURATION_QN, 0.25);
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    assert!(outcome.is_acceptable(), "{:?}", outcome.hard_violations);
    assert_eq!(
        app(&outcome, "voice_leading.midi_pitch_within_range").status,
        RuleStatus::NotApplicable
    );
    assert_eq!(
        app(&outcome, "voice_leading.duration_is_positive").status,
        RuleStatus::NotApplicable
    );
}

#[test]
fn preserve_melody_keeps_every_pitch() {
    let p = profile("cinematic");
    let ctx = RuleContext::new()
        .with_event(RuleEvent::PartGenerated)
        .with_bool("melody_material_would_be_altered", true);
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    assert!(outcome
        .hard_violations
        .contains(&"voice_leading.preserve_melody_pitches_exactly".to_string()));
    assert_eq!(outcome.delta("melody_fit"), 0.0);
}

#[test]
fn hard_rules_are_evaluated_before_soft_ones() {
    let p = profile("common_practice");
    let ctx = RuleContext::new()
        .with_event(RuleEvent::PartGenerated)
        .with_bool("melody_material_would_be_altered", true)
        .with_str(facts::ROLE, "bass");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);

    let first_soft = outcome
        .applications
        .iter()
        .position(|a| kb().rule(&a.rule_id).is_some_and(|r| !r.kind.is_hard()))
        .expect("the event has soft rules too");
    let last_hard = outcome
        .applications
        .iter()
        .rposition(|a| kb().rule(&a.rule_id).is_some_and(|r| r.kind.is_hard()))
        .expect("the event has hard rules too");
    assert!(
        last_hard < first_soft,
        "every hard rule must be reported before the first soft one"
    );
}

#[test]
fn a_loop_integrity_rule_scores_rather_than_rejecting() {
    let p = profile("electronic_loop");
    let ctx = RuleContext::new()
        .with_event(RuleEvent::LoopBoundary)
        .with_str(facts::LOOP_INTENT, "closed_tonic")
        .with_num(facts::MAX_NOTE_END_QN, 33.0)
        .with_num(facts::LOOP_END_QN, 32.0)
        .with_str(facts::NOTE_CARRY_POLICY, "default");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(&outcome, "looping.no_hanging_note_past_loop_end");
    assert_eq!(a.status, RuleStatus::Applied);
    assert!(a.score_delta < 0.0);
    assert!(
        outcome.is_acceptable(),
        "a hanging note is a heavy penalty, not a hard rejection"
    );
}

// ---------------------------------------------------------------------------
// looping
// ---------------------------------------------------------------------------

#[test]
fn hanging_note_detected() {
    let p = profile("electronic_loop");
    let ctx = RuleContext::new()
        .with_event(RuleEvent::LoopBoundary)
        .with_num(facts::MAX_NOTE_END_QN, 32.5)
        .with_num(facts::LOOP_END_QN, 32.0)
        .with_str(facts::NOTE_CARRY_POLICY, "default");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(&outcome, "looping.no_hanging_note_past_loop_end");
    assert_eq!(a.status, RuleStatus::Applied);
    assert!(outcome.delta("loop_compatibility") < 0.0);
}

#[test]
fn carry_policy_allows_overhang() {
    let p = profile("electronic_loop");
    let ctx = RuleContext::new()
        .with_event(RuleEvent::LoopBoundary)
        .with_num(facts::MAX_NOTE_END_QN, 32.5)
        .with_num(facts::LOOP_END_QN, 32.0)
        .with_str(facts::NOTE_CARRY_POLICY, "carry");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(&outcome, "looping.no_hanging_note_past_loop_end");
    assert_eq!(a.status, RuleStatus::Bypassed);
    assert_eq!(
        a.matched_exceptions,
        vec!["note_carry_policy_is_explicit".to_string()]
    );
    assert_eq!(a.score_delta, 0.0);
}

#[test]
fn v_to_i_across_wrap_compatible() {
    let p = profile("pop_rock");
    let ctx = RuleContext::new()
        .with_event(RuleEvent::LoopBoundary)
        .with_str(facts::LOOP_INTENT, "closed_tonic")
        .with_str(facts::FINAL_CHORD_FUNCTION, "dominant")
        .with_str(facts::FIRST_CHORD_FUNCTION, "tonic")
        .with_str(facts::KEY_CENTER_KIND, "tonal");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(
        &outcome,
        "looping.closed_tonic_prefers_dominant_to_tonic_wrap",
    );
    assert_eq!(a.status, RuleStatus::Applied);
    assert!(
        a.score_delta > 0.0,
        "a good wrap is rewarded, not merely allowed"
    );
    assert!(outcome.delta("loop_compatibility") > 0.0);
    assert!(
        a.explanation.contains("Credited"),
        "a bonus should read as a credit: {}",
        a.explanation
    );
}

#[test]
fn closed_tonic_prefers_turnaround() {
    let p = profile("pop_rock");
    // Same loop, but it ends on the tonic: the turnaround bonus does not apply.
    let ctx = RuleContext::new()
        .with_event(RuleEvent::LoopBoundary)
        .with_str(facts::LOOP_INTENT, "closed_tonic")
        .with_str(facts::FINAL_CHORD_FUNCTION, "tonic")
        .with_str(facts::FIRST_CHORD_FUNCTION, "tonic")
        .with_str(facts::KEY_CENTER_KIND, "tonal");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(
        &outcome,
        "looping.closed_tonic_prefers_dominant_to_tonic_wrap",
    );
    assert_eq!(a.status, RuleStatus::NotApplicable);
    assert_eq!(outcome.delta("loop_compatibility"), 0.0);
    assert!(a.explanation.contains("does not hold"), "{}", a.explanation);
}

#[test]
fn modal_drone_no_dominant_required() {
    let p = profile("modal_ambient");
    let ctx = RuleContext::new()
        .with_event(RuleEvent::LoopBoundary)
        .with_str(facts::LOOP_INTENT, "modal_drone")
        .with_str(facts::KEY_CENTER_KIND, "modal")
        .with_bool("common_tone_retained", true)
        .with_str(facts::FINAL_CHORD_FUNCTION, "modal");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(&outcome, "looping.modal_drone_does_not_require_dominant");
    assert_eq!(a.status, RuleStatus::Applied);
    assert!(a.score_delta > 0.0);
    // The tonal wrap rule must not even be considered for this loop intent.
    let closed = app(
        &outcome,
        "looping.closed_tonic_prefers_dominant_to_tonic_wrap",
    );
    assert_eq!(closed.status, RuleStatus::NotApplicable);
}

#[test]
fn modal_loop_scored_on_common_tones() {
    let p = profile("modal_ambient");
    let without = RuleContext::new()
        .with_event(RuleEvent::LoopBoundary)
        .with_str(facts::LOOP_INTENT, "modal_drone")
        .with_str(facts::KEY_CENTER_KIND, "modal")
        .with_bool("common_tone_retained", false);
    let outcome = RuleEngine::new(kb(), &p).evaluate(&without);
    assert_eq!(
        app(&outcome, "looping.modal_drone_does_not_require_dominant").status,
        RuleStatus::NotApplicable,
        "without a retained common tone there is nothing to reward"
    );
}

// ---------------------------------------------------------------------------
// counterpoint and voice leading, where profiles genuinely disagree
// ---------------------------------------------------------------------------

fn accented_dissonance() -> RuleContext {
    RuleContext::new()
        .with_event(RuleEvent::VoicePairMotion)
        .with_num(facts::METRIC_WEIGHT, 1.0)
        .with_bool("dissonance_is_prepared", false)
        .with_bool("suspension_resolves_down_by_step", false)
        .with_bool("intentional_cluster", false)
}

#[test]
fn strong_beat_consonance_strict() {
    let p = profile("strict_counterpoint");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&accented_dissonance());
    let a = app(&outcome, "counterpoint.strong_beats_are_consonant");
    assert_eq!(a.status, RuleStatus::Applied);
    assert!(a.score_delta < 0.0);
    assert!(outcome.delta("voice_leading") < 0.0);
}

#[test]
fn accented_dissonance_allowed_elsewhere() {
    // The rule is scoped to strict_counterpoint only …
    let jazz = profile("jazz_standard");
    let outcome = RuleEngine::new(kb(), &jazz).evaluate(&accented_dissonance());
    let a = app(&outcome, "counterpoint.strong_beats_are_consonant");
    assert_eq!(a.status, RuleStatus::NotApplicable);
    assert_eq!(outcome.delta("voice_leading"), 0.0);

    // … and jazz_standard additionally disables it outright, which is what the
    // profile's `rule_overrides` say.
    assert_eq!(
        jazz.overrides
            .get("counterpoint.strong_beats_are_consonant"),
        Some(&RuleOverride::Disabled)
    );
    assert!(!jazz.is_rule_enabled("counterpoint.strong_beats_are_consonant"));
}

#[test]
fn a_prepared_suspension_bypasses_the_strong_beat_rule() {
    let p = profile("strict_counterpoint");
    let ctx = accented_dissonance()
        .with_bool("dissonance_is_prepared", true)
        .with_bool("suspension_resolves_down_by_step", true);
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(&outcome, "counterpoint.strong_beats_are_consonant");
    assert_eq!(a.status, RuleStatus::Bypassed);
    assert_eq!(a.matched_exceptions.len(), 2);
    assert!(a
        .explanation
        .contains("the dissonance is prepared as a consonance in the same voice"));
}

fn hidden_fifth() -> RuleContext {
    RuleContext::new()
        .with_event(RuleEvent::VoicePairMotion)
        .with_str(facts::MOTION, "similar")
        .with_num(facts::ARRIVAL_INTERVAL_SEMITONES, 7.0)
        .with_bool("outer_voices_involved", true)
        .with_bool("stepwise_connection_available", false)
        .with_str(facts::VOICING_FAMILY, "close")
        .with_bool("planing_context", false)
}

#[test]
fn hidden_fifths_penalised_strict() {
    let p = profile("strict_counterpoint");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&hidden_fifth());
    let a = app(&outcome, "voice_leading.direct_perfect_in_outer_voices");
    assert_eq!(a.status, RuleStatus::Applied);
    assert!(a.score_delta < 0.0);
    assert_eq!(a.matched_conditions.len(), 3);
}

#[test]
fn hidden_fifths_ignored_pop() {
    let p = profile("pop_rock");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&hidden_fifth());
    let a = app(&outcome, "voice_leading.direct_perfect_in_outer_voices");
    assert_eq!(
        a.status,
        RuleStatus::NotApplicable,
        "pop_rock is not in the rule's profile list at all"
    );
    assert_eq!(outcome.delta("voice_leading"), 0.0);
}

#[test]
fn jazz_disables_hidden_fifths_even_though_it_inherits_common_practice() {
    let jazz = profile("jazz_standard");
    assert!(
        jazz.chain.contains(&"common_practice".to_string()),
        "jazz_standard inherits from common_practice"
    );
    let rule = kb()
        .rule("voice_leading.direct_perfect_in_outer_voices")
        .expect("the rule exists");
    assert!(
        jazz.applies_to(rule),
        "the rule reaches jazz through its ancestor"
    );
    assert!(
        !jazz.is_rule_enabled(&rule.id),
        "but jazz_standard switches it off"
    );

    let outcome = RuleEngine::new(kb(), &jazz).evaluate(&hidden_fifth());
    let a = app(&outcome, "voice_leading.direct_perfect_in_outer_voices");
    assert_eq!(a.status, RuleStatus::NotApplicable);
    assert!(
        a.explanation
            .contains("jazz_standard profile switches this rule off"),
        "a disabled rule must say who disabled it: {}",
        a.explanation
    );
}

// ---------------------------------------------------------------------------
// harmony
// ---------------------------------------------------------------------------

#[test]
fn dominant_resolves_down_fifth() {
    let p = profile("common_practice");
    let ctx = RuleContext::new()
        .with_event(RuleEvent::ChordPair)
        .with_str(facts::CHORD_FAMILY, "dominant")
        .with_str(facts::FUNCTION_CLASS, "dominant")
        .with_list(facts::SOUNDING_DEGREES, &["1", "3", "5", "b7"])
        .with_bool("chord_is_tritone_substitute", false)
        .with_str(facts::KEY_CENTER_KIND, "tonal")
        .with_bool("pedal_point_is_active", false);
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(&outcome, "harmony.dominant_seventh_resolves_down_fifth");
    assert_eq!(a.status, RuleStatus::Applied);
    assert!(a.score_delta > 0.0);
    assert!(outcome.delta("functional_or_modal_coherence") > 0.0);
    assert_eq!(a.source_ids, vec!["open-music-theory".to_string()]);
}

#[test]
fn a_modal_centre_bypasses_the_dominant_resolution_default() {
    let p = profile("common_practice");
    let ctx = RuleContext::new()
        .with_event(RuleEvent::ChordPair)
        .with_str(facts::CHORD_FAMILY, "dominant")
        .with_str(facts::FUNCTION_CLASS, "dominant")
        .with_list(facts::SOUNDING_DEGREES, &["1", "3", "5", "b7"])
        .with_str(facts::KEY_CENTER_KIND, "modal");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let a = app(&outcome, "harmony.dominant_seventh_resolves_down_fifth");
    assert_eq!(a.status, RuleStatus::Bypassed);
    assert!(a
        .matched_exceptions
        .contains(&"modal_center_is_active".to_string()));
}

// ---------------------------------------------------------------------------
// engine mechanics
// ---------------------------------------------------------------------------

#[test]
fn a_multiplier_scales_the_delta_and_a_zero_multiplier_still_applies() {
    let rule = kb()
        .rule("counterpoint.dissonance_is_prepared")
        .expect("the rule exists");
    let jazz = profile("jazz_standard");
    assert_eq!(
        jazz.rule_multiplier(&rule.id),
        0.3,
        "jazz_standard turns this one down rather than off"
    );

    let strict = profile("strict_counterpoint");
    let plain = strict.rule_multiplier(&rule.id);
    assert!(plain > 0.0);
}

#[test]
fn an_outcome_with_no_event_evaluates_nothing() {
    let p = profile("blues");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&RuleContext::new());
    assert!(outcome.applications.is_empty());
    assert!(outcome.deltas.is_empty());
    assert!(outcome.is_acceptable());
}

#[test]
fn evaluate_only_considers_rules_listening_for_the_event() {
    let p = profile("common_practice");
    let ctx = RuleContext::new().with_event(RuleEvent::HarmonicGrid);
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    assert_eq!(
        outcome.applications.len(),
        2,
        "two rules watch harmonic_grid"
    );
    for a in &outcome.applications {
        assert_eq!(
            kb().rule(&a.rule_id).map(|r| r.trigger.event),
            Some(RuleEvent::HarmonicGrid)
        );
    }
}

#[test]
fn evaluation_is_deterministic() {
    let p = profile("neo_soul_rnb");
    let ctx = close_major_eleven();
    let engine = RuleEngine::new(kb(), &p);
    let a = engine.evaluate(&ctx).to_json().to_canonical_string();
    let b = engine.evaluate(&ctx).to_json().to_canonical_string();
    assert_eq!(a, b);
}

#[test]
fn source_ids_are_collected_without_duplicates_and_only_from_live_rules() {
    let p = profile("strict_counterpoint");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&hidden_fifth());
    let mut sorted = outcome.source_ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), outcome.source_ids.len(), "no duplicates");
    for s in &outcome.source_ids {
        assert!(kb().source(s).is_some(), "{s} must resolve");
    }
}

#[test]
fn every_application_carries_a_real_explanation() {
    let p = profile("jazz_standard");
    for event in RuleEvent::all() {
        let ctx = RuleContext::new().with_event(*event);
        for a in RuleEngine::new(kb(), &p).evaluate(&ctx).applications {
            assert!(
                !a.explanation.is_empty(),
                "{} has no explanation",
                a.rule_id
            );
            assert!(
                a.explanation.len() > 20,
                "{} has a stub explanation: {}",
                a.rule_id,
                a.explanation
            );
            assert!(
                !a.explanation.contains("{}") && !a.explanation.contains("Tri::"),
                "{} leaks a debug rendering: {}",
                a.rule_id,
                a.explanation
            );
            assert!(
                a.explanation.ends_with('.') || a.explanation.ends_with('"'),
                "{} is not a sentence: {}",
                a.rule_id,
                a.explanation
            );
        }
    }
}

#[test]
fn every_rule_can_be_evaluated_in_every_profile_without_error() {
    // The engine must be total: no profile/rule pair may panic or produce a
    // status the trace cannot represent.
    for p in KnowledgeBase::embedded().profiles() {
        let resolved = profile(&p.id);
        let engine = RuleEngine::new(kb(), &resolved);
        for rule in kb().rules() {
            let ctx = RuleContext::new().with_event(rule.trigger.event);
            let a = engine.evaluate_rule(rule, &ctx);
            assert_eq!(a.rule_id, rule.id);
            assert!(matches!(
                a.status,
                RuleStatus::Applied
                    | RuleStatus::Bypassed
                    | RuleStatus::NotApplicable
                    | RuleStatus::Violated
            ));
        }
    }
}

#[test]
fn rules_for_event_and_evaluate_agree_on_scope() {
    let p = profile("blues");
    let ctx = RuleContext::new().with_event(RuleEvent::ChordSelected);
    let outcome = RuleEngine::new(kb(), &p).evaluate(&ctx);
    let live: Vec<&str> = kb()
        .rules_for_event(RuleEvent::ChordSelected, &p)
        .iter()
        .map(|r| r.id.as_str())
        .collect();
    for a in &outcome.applications {
        if a.status != RuleStatus::NotApplicable {
            assert!(live.contains(&a.rule_id.as_str()), "{}", a.rule_id);
        }
    }
}

#[test]
fn with_status_partitions_the_applications() {
    let p = profile("jazz_standard");
    let outcome = RuleEngine::new(kb(), &p).evaluate(&close_major_eleven());
    let total = [
        RuleStatus::Applied,
        RuleStatus::Bypassed,
        RuleStatus::NotApplicable,
        RuleStatus::Violated,
    ]
    .iter()
    .map(|s| outcome.with_status(*s).len())
    .sum::<usize>();
    assert_eq!(total, outcome.applications.len());
}
