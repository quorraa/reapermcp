//! The brief §28 "Looping" list, plus the six intents against compatible and
//! incompatible material.
//!
//! Several of these behaviours correspond to `test_id`s that `theory-kb` and
//! `music-analysis` already claim. They are tested here anyway — `theory-kb`
//! proves the *rule* fires given the facts; this crate has to prove it computes
//! the right facts from real material.

use loop_engine::prelude::*;
use loop_engine::testing;
use music_domain::prelude::*;
use theory_kb::prelude::RuleStatus;

// ---------------------------------------------------------------------------
// §28: detect a hanging note beyond loop end
// ---------------------------------------------------------------------------

#[test]
fn a_note_sounding_past_the_loop_end_is_reported_as_hanging() {
    let h = testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice");
    let report = h.report();
    assert_eq!(
        report.hanging_notes.len(),
        1,
        "the fixture's final G3 sustains a quarter past the loop end"
    );
    assert_eq!(report.tail_qn, BeatTime::from_quarters(1));
    let finding = report
        .findings
        .iter()
        .find(|f| f.code == "LOOP_HANGING_NOTE")
        .expect("a hanging note is a finding");
    assert_eq!(finding.severity, Severity::Major);
    assert!(!report.compatible);
}

#[test]
fn the_hanging_note_rule_fires_on_real_material() {
    let a = testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice").audit();
    assert_eq!(
        a.rule_status("looping.no_hanging_note_past_loop_end"),
        Some(RuleStatus::Applied)
    );
    assert!(a.rule_points() < 0.0);
}

#[test]
fn a_note_ending_exactly_on_the_loop_end_does_not_hang() {
    let h = testing::Harness::empty(LoopIntent::ClosedTonic, "common_practice").with_notes(vec![
        testing::note(
            0,
            60,
            BeatTime::from_quarters(12),
            BeatTime::from_quarters(4),
        ),
    ]);
    assert!(h.report().hanging_notes.is_empty());
    assert_eq!(h.report().tail_qn, BeatTime::ZERO);
}

// ---------------------------------------------------------------------------
// §28: correctly handle a note explicitly marked to carry
// ---------------------------------------------------------------------------

#[test]
fn a_note_marked_to_carry_is_crossing_but_never_hanging() {
    let h = testing::unmarked_pedal_overhang(LoopIntent::ModalDrone, "modal_ambient");
    let plain = h.report();
    assert_eq!(plain.hanging_notes, vec![0]);

    let marked = testing::pedal_loop(LoopIntent::ModalDrone, "modal_ambient").report();
    assert!(
        marked.hanging_notes.is_empty(),
        "an explicit carry is a decision, not a fault"
    );
    assert_eq!(marked.crossing_notes, vec![0]);
    assert!(marked
        .findings
        .iter()
        .any(|f| f.code == "LOOP_NOTE_CARRIED" && f.severity == Severity::Info));
}

#[test]
fn an_explicit_carry_bypasses_the_hanging_note_rule() {
    let a = testing::pedal_loop(LoopIntent::ModalDrone, "modal_ambient").audit();
    assert_eq!(a.carry_policy, "carry");
    assert_eq!(
        a.rule_status("looping.no_hanging_note_past_loop_end"),
        Some(RuleStatus::Bypassed),
        "{:?}",
        a.rule_explanation("looping.no_hanging_note_past_loop_end")
    );
}

#[test]
fn a_carried_note_does_not_break_the_exact_length_invariant() {
    let h = testing::pedal_loop(LoopIntent::ModalDrone, "modal_ambient");
    let a = h.audit();
    assert!(
        a.length_exact,
        "material outside the span by request is not a length fault"
    );
    assert!(!a
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_LENGTH_MISMATCH"));
    assert!(a.report.compatible);
}

#[test]
fn every_documented_carry_mark_is_recognised() {
    for mark in loop_engine::CARRY_MARKS {
        let mut n = testing::note(
            0,
            60,
            BeatTime::from_quarters(15),
            BeatTime::from_quarters(3),
        );
        n.articulation = Some((*mark).to_string());
        let h = testing::Harness::empty(LoopIntent::SeamlessColor, "electronic_loop")
            .with_notes(vec![n]);
        assert!(
            h.report().hanging_notes.is_empty(),
            "the articulation {mark:?} must count as an explicit carry"
        );
    }
}

// ---------------------------------------------------------------------------
// §28: detect a pickup requiring special handling
// ---------------------------------------------------------------------------

#[test]
fn an_anacrusis_is_measured_and_reported() {
    let report =
        testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice").report();
    assert_eq!(report.pickup_qn, BeatTime::from_quarters(1));
    assert!(report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_PICKUP_PRESENT" && f.severity == Severity::Moderate));
}

#[test]
fn the_pickup_rule_fires_and_offers_both_documented_treatments() {
    let h = testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice");
    let a = h.audit();
    assert_eq!(
        a.rule_status("looping.pickup_wraps_or_duplicates"),
        Some(RuleStatus::Applied)
    );
    let repairs = suggest_repairs(
        a.report.intent.map(|_| h.kb).unwrap(),
        &h.profile,
        &h.input(),
        &a.report,
    );
    let ids: Vec<&str> = repairs.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"wrap_pickup_into_loop_tail"));
    assert!(ids.contains(&"duplicate_pickup_for_first_pass"));
}

#[test]
fn a_multi_note_anacrusis_is_measured_from_its_first_onset() {
    let h = testing::Harness::empty(LoopIntent::ClosedTonic, "common_practice").with_notes(vec![
        testing::note_at(0, 55, (-3, 2), (1, 2)),
        testing::note_at(1, 57, (-1, 1), (1, 1)),
        testing::note(2, 60, BeatTime::ZERO, BeatTime::from_quarters(4)),
    ]);
    assert_eq!(h.report().pickup_qn, BeatTime::new(3, 2));
}

#[test]
fn a_loop_with_no_anacrusis_reports_zero() {
    let h = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice");
    assert_eq!(h.report().pickup_qn, BeatTime::ZERO);
    assert!(!h
        .report()
        .findings
        .iter()
        .any(|f| f.code == "LOOP_PICKUP_PRESENT"));
}

// ---------------------------------------------------------------------------
// §28: V-to-I across the wrap is compatible with closed-tonic intent
// ---------------------------------------------------------------------------

#[test]
fn v_to_i_across_the_wrap_is_highly_compatible_with_closed_tonic() {
    let h = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice");
    let a = h.audit();
    assert_eq!(
        a.observation.final_function,
        Some(HarmonicFunction::Dominant)
    );
    assert_eq!(a.observation.first_function, Some(HarmonicFunction::Tonic));
    assert!(a.report.compatible);
    assert!(a.report.score > 0.9, "score was {}", a.report.score);
    assert!(a.report.harmonic_wrap.contains("authentic cadence"));
    assert_eq!(
        a.rule_status("looping.closed_tonic_prefers_dominant_to_tonic_wrap"),
        Some(RuleStatus::Applied)
    );
}

#[test]
fn a_closed_tonic_loop_that_never_reaches_the_tonic_scores_badly() {
    let h = testing::Harness::empty(LoopIntent::ClosedTonic, "common_practice").with_chords(&[
        ("Dm", 0, 4),
        ("Em", 4, 4),
        ("F", 8, 4),
        ("Am", 12, 4),
    ]);
    let a = h.audit();
    assert!(!a.report.compatible);
    assert!(a.fit.criterion("functional_wrap") < 0.6);
    assert!(a
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_INTENT_MISMATCH"));
}

#[test]
fn the_closed_tonic_rule_is_bypassed_when_a_modal_centre_is_active() {
    let h = testing::modal_planing_loop(LoopIntent::ClosedTonic, "cinematic");
    let a = h.audit();
    assert!(a.key.is_modal);
    assert_ne!(
        a.rule_status("looping.closed_tonic_prefers_dominant_to_tonic_wrap"),
        Some(RuleStatus::Applied),
        "a modal centre is the rule's own exception"
    );
}

// ---------------------------------------------------------------------------
// §28: do not require V-to-I for modal-drone intent
// ---------------------------------------------------------------------------

#[test]
fn a_modal_loop_with_no_dominant_anywhere_is_compatible_with_modal_drone() {
    let h = testing::modal_planing_loop(LoopIntent::ModalDrone, "modal_ambient");
    let a = h.audit();
    assert!(
        !h.chords.iter().any(|c| c.spec.is_dominant_family()),
        "the fixture contains no dominant-family chord at all"
    );
    assert_ne!(
        a.observation.final_function,
        Some(HarmonicFunction::Dominant)
    );
    assert!(a.report.compatible);
    assert!(a.report.score > 0.85, "score was {}", a.report.score);
}

#[test]
fn modal_drone_never_scores_the_functional_wrap() {
    let a = testing::dominant_wrap(LoopIntent::ModalDrone, "modal_ambient").audit();
    assert!(a
        .fit
        .criteria
        .iter()
        .all(|(n, _)| *n != "functional_wrap" && *n != "leaves_an_opening"));
    assert!(a.fit.criterion("non_functional_wrap") < 0.01);
}

#[test]
fn a_functional_loop_serves_modal_drone_worse_than_a_modal_one() {
    let modal = testing::modal_planing_loop(LoopIntent::ModalDrone, "modal_ambient")
        .audit()
        .fit
        .fit;
    let functional = testing::dominant_wrap(LoopIntent::ModalDrone, "modal_ambient")
        .audit()
        .fit
        .fit;
    assert!(
        modal > functional,
        "modal {modal} should beat functional {functional} for a drone"
    );
}

#[test]
fn a_modal_loop_is_scored_on_common_tones_and_pedal_continuity() {
    let a = testing::pedal_loop(LoopIntent::ModalDrone, "modal_ambient").audit();
    assert!(a.observation.common_tone_retained);
    assert!(a.observation.pedal_continues);
    assert_eq!(
        a.rule_status("looping.modal_drone_does_not_require_dominant"),
        Some(RuleStatus::Applied),
        "{:?}",
        a.rule_explanation("looping.modal_drone_does_not_require_dominant")
    );
    assert_eq!(a.fit.criterion("pedal_or_common_tone"), 1.0);
}

#[test]
fn the_same_modal_material_is_not_compatible_with_closed_tonic() {
    let modal_intent =
        testing::modal_planing_loop(LoopIntent::ModalDrone, "modal_ambient").report();
    let tonal_intent =
        testing::modal_planing_loop(LoopIntent::ClosedTonic, "modal_ambient").report();
    assert!(modal_intent.compatible);
    assert!(
        !tonal_intent.compatible,
        "the intent, not the material, is what changed"
    );
}

// ---------------------------------------------------------------------------
// §28: detect bass discontinuity / voice-leading discontinuity
// ---------------------------------------------------------------------------

#[test]
fn a_held_bass_is_reported_as_continuous() {
    let a = testing::voice_leading_leap(LoopIntent::SeamlessColor, "common_practice").audit();
    assert_eq!(a.observation.bass_interval, Some(0));
    assert!(a.report.bass_wrap.contains("held"));
}

#[test]
fn the_bass_wrap_text_names_both_sides_and_the_interval() {
    let a = testing::bass_leap(LoopIntent::SeamlessColor, "electronic_loop").audit();
    assert!(a.report.bass_wrap.contains("D3"));
    assert!(a.report.bass_wrap.contains("C2"));
    assert!(a.report.bass_wrap.contains("-14"));
}

#[test]
fn a_bass_leap_is_not_reported_as_a_voice_leading_fault() {
    let a = testing::bass_leap(LoopIntent::SeamlessColor, "electronic_loop").audit();
    assert!(a.observation.stepwise_available, "the upper voices connect");
    assert!(!a
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_VOICE_LEADING_DISCONTINUITY"));
}

#[test]
fn a_monophonic_wrap_is_not_called_a_voice_leading_fault_for_a_fourth() {
    let a = testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice").audit();
    assert_eq!(a.observation.max_leap, 5);
    assert!(!a
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_VOICE_LEADING_DISCONTINUITY"));
}

// ---------------------------------------------------------------------------
// intents against compatible and incompatible material
// ---------------------------------------------------------------------------

#[test]
fn open_dominant_prefers_material_that_ends_unresolved() {
    let open = testing::unresolved_dominant(LoopIntent::OpenDominant, "jazz_standard")
        .audit()
        .fit
        .fit;
    let closed = testing::Harness::empty(LoopIntent::OpenDominant, "jazz_standard")
        .with_chords(&[("Dm7", 0, 4), ("G7", 4, 4), ("Cmaj7", 8, 8)])
        .audit()
        .fit
        .fit;
    assert!(open > closed, "open {open} vs closed {closed}");
}

#[test]
fn seamless_color_prefers_shared_pitch_classes() {
    let shared = testing::Harness::empty(LoopIntent::SeamlessColor, "electronic_loop")
        .with_chords(&[("Cmaj7", 0, 8), ("Am7", 8, 8)])
        .audit();
    let disjoint = testing::Harness::empty(LoopIntent::SeamlessColor, "electronic_loop")
        .with_chords(&[("C", 0, 8), ("F#", 8, 8)])
        .audit();
    assert!(shared.observation.common_tone_available);
    assert!(!disjoint.observation.common_tone_available);
    assert!(shared.fit.fit > disjoint.fit.fit);
}

#[test]
fn transition_ready_is_hurt_by_a_closing_tonic() {
    let closing = testing::Harness::empty(LoopIntent::TransitionReady, "electronic_loop")
        .with_chords(&[("F", 0, 4), ("G", 4, 4), ("Am", 8, 4), ("C", 12, 4)])
        .audit();
    assert_eq!(
        closing.observation.final_function,
        Some(HarmonicFunction::Tonic)
    );
    assert!(closing.fit.criterion("leaves_an_opening") < 0.3);
    assert!(!closing.report.compatible);
}

#[test]
fn one_shot_ending_wants_the_tonic_and_a_dominant_ending_fails_it() {
    let stinger = testing::modal_planing_loop(LoopIntent::OneShotEnding, "cinematic").report();
    let unfinished = testing::dominant_wrap(LoopIntent::OneShotEnding, "cinematic").report();
    assert!(stinger.compatible);
    assert!(
        !unfinished.compatible,
        "a loop ending on V is not a one-shot ending"
    );
}

#[test]
fn every_intent_gives_a_bounded_score_on_every_fixture() {
    for intent in testing::ALL_INTENTS {
        for h in [
            testing::dominant_wrap(*intent, "common_practice"),
            testing::modal_planing_loop(*intent, "modal_ambient"),
            testing::pickup_and_hanging_note(*intent, "electronic_loop"),
        ] {
            let r = h.report();
            assert!((0.0..=1.0).contains(&r.score), "{:?}", r.score);
            assert!((0.0..=1.0).contains(&r.confidence));
            assert_eq!(r.intent, Some(*intent));
            assert_eq!(r.loop_start, h.span.start);
            assert_eq!(r.loop_end, h.span.end);
        }
    }
}

#[test]
fn the_verdict_changes_with_the_intent_and_not_with_the_material() {
    let scores: Vec<f64> = testing::ALL_INTENTS
        .iter()
        .map(|i| {
            testing::dominant_wrap(*i, "common_practice")
                .audit()
                .fit
                .fit
        })
        .collect();
    let min = scores.iter().cloned().fold(f64::MAX, f64::min);
    let max = scores.iter().cloned().fold(f64::MIN, f64::max);
    assert!(
        max - min > 0.3,
        "the six intents must genuinely disagree about one loop: {scores:?}"
    );
}

// ---------------------------------------------------------------------------
// unresolved tendencies, harmonic rhythm, layers, percussion
// ---------------------------------------------------------------------------

#[test]
fn a_resolving_leading_tone_leaves_no_unresolved_tendency() {
    let a = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice").audit();
    assert!(
        a.observation.unresolved_tendencies.is_empty(),
        "{:?}",
        a.observation.unresolved_tendencies
    );
}

#[test]
fn an_unresolved_leading_tone_is_named_in_the_finding() {
    let a = testing::unresolved_dominant(LoopIntent::ClosedTonic, "common_practice").audit();
    assert!(a
        .observation
        .unresolved_tendencies
        .iter()
        .any(|t| t.contains("leading tone") || t.contains("chordal seventh")));
}

#[test]
fn an_unresolved_tendency_is_only_a_fault_when_the_intent_wanted_resolution() {
    let closed = testing::unresolved_dominant(LoopIntent::ClosedTonic, "common_practice").report();
    let open = testing::unresolved_dominant(LoopIntent::OpenDominant, "common_practice").report();
    let severity = |r: &LoopReport| {
        r.findings
            .iter()
            .find(|f| f.code == "LOOP_UNRESOLVED_TENDENCY")
            .map(|f| f.severity)
    };
    assert_eq!(severity(&closed), Some(Severity::Minor));
    assert_eq!(severity(&open), Some(Severity::Info));
}

#[test]
fn percussion_phase_metadata_is_reported_when_percussion_exists() {
    let a = testing::aligned_percussion(LoopIntent::SeamlessColor, "drum_and_bass").audit();
    let phase = a.observation.percussion.as_ref().expect("percussion phase");
    assert_eq!(phase.hits, 16);
    assert_eq!(phase.period_qn, BeatTime::from_quarters(1));
    assert_eq!(phase.phase_offset_qn, BeatTime::ZERO);
    assert!(phase.aligned);
    assert!(a
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_PERCUSSION_PHASE" && f.severity == Severity::Info));
}

#[test]
fn a_percussion_pattern_off_the_loop_point_is_reported_as_out_of_phase() {
    let a = testing::offset_percussion(LoopIntent::SeamlessColor, "drum_and_bass").audit();
    let phase = a.observation.percussion.as_ref().expect("percussion phase");
    assert_eq!(phase.phase_offset_qn, BeatTime::new(1, 2));
    assert!(!phase.aligned);
    assert!(a
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_PERCUSSION_PHASE" && f.severity == Severity::Minor));
}

#[test]
fn material_without_percussion_reports_no_phase_metadata() {
    let a = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice").audit();
    assert!(a.observation.percussion.is_none());
    assert!(!a
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_PERCUSSION_PHASE"));
}

#[test]
fn without_parts_the_layer_question_is_reported_as_unanswerable() {
    let a = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice").audit();
    assert!(!a.observation.layers_known);
    assert!(a
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_LAYERS_UNKNOWN" && f.severity == Severity::Info));
}

// ---------------------------------------------------------------------------
// key reading
// ---------------------------------------------------------------------------

#[test]
fn a_functional_progression_reads_as_a_key_and_a_planing_one_as_a_mode() {
    let functional = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice").audit();
    assert!(!functional.key.is_modal);
    assert_eq!(functional.key.tonic.0, Letter::C);

    let modal = testing::modal_planing_loop(LoopIntent::ModalDrone, "modal_ambient").audit();
    assert!(modal.key.is_modal);
}

#[test]
fn the_key_source_is_reported_so_a_caller_can_judge_the_reading() {
    assert_eq!(
        testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice")
            .audit()
            .key
            .source,
        "chords"
    );
    assert_eq!(
        testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice")
            .audit()
            .key
            .source,
        "notes"
    );
    assert_eq!(
        testing::Harness::empty(LoopIntent::ClosedTonic, "common_practice")
            .audit()
            .key
            .source,
        "default"
    );
}

#[test]
fn an_empty_loop_reports_that_it_has_no_material() {
    let report = testing::Harness::empty(LoopIntent::ClosedTonic, "common_practice").report();
    assert!(report.findings.iter().any(|f| f.code == "LOOP_NO_MATERIAL"));
    assert!(
        report.confidence < 0.7,
        "confidence was {}",
        report.confidence
    );
}

#[test]
fn confidence_rises_with_the_evidence_available() {
    let none = testing::Harness::empty(LoopIntent::ClosedTonic, "common_practice")
        .report()
        .confidence;
    let chords = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice")
        .report()
        .confidence;
    let both = testing::pedal_loop(LoopIntent::ModalDrone, "modal_ambient")
        .report()
        .confidence;
    assert!(none < chords);
    assert!(chords < both);
}
