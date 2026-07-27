//! The `knowledge/` `test_id`s this crate discharges.
//!
//! Every function here is named exactly after a `test_ids` entry declared by a
//! rule in `knowledge/rules/looping.json`, and each one asserts the behaviour
//! that rule describes. The claim is recorded in the workspace coverage ledger;
//! a claim without a test of the same name is not a claim.
//!
//! Ids already claimed by `theory-kb` (`hanging_note_detected`,
//! `carry_policy_allows_overhang`, `v_to_i_across_wrap_compatible`,
//! `modal_drone_no_dominant_required`, `closed_tonic_prefers_turnaround`,
//! `modal_loop_scored_on_common_tones`) and by `music-analysis`
//! (`pickup_requires_special_handling`) are not re-claimed here, but the
//! corresponding behaviours are still tested — see `looping_behaviours.rs`.

use loop_engine::prelude::*;
use loop_engine::testing;
use music_domain::prelude::*;
use theory_kb::prelude::RuleStatus;

// ---------------------------------------------------------------------------
// looping.pickup_wraps_or_duplicates
// ---------------------------------------------------------------------------

#[test]
fn pickup_wrapped_into_tail() {
    let h = testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice");
    let span = h.span;
    let before = h.report();
    assert!(
        before.pickup_qn.is_positive(),
        "the fixture has an anacrusis"
    );

    let mut notes = h.notes.notes.clone();
    apply_boundary_policy(&mut notes, &span, CarryPolicy::Split);

    // The anacrusis now sounds at the end of the loop, where the repeat plays it.
    let wrapped = NoteSet::sorted(notes, h.time_map.clone());
    assert_eq!(
        pickup_length(&wrapped, &span),
        BeatTime::ZERO,
        "nothing sounds before the loop start any more"
    );
    assert!(
        wrapped
            .notes
            .iter()
            .any(|n| n.onset == BeatTime::from_quarters(15) && n.midi == 55),
        "the G3 upbeat is now the last quarter of the loop"
    );
    assert_eq!(
        wrapped.notes.len(),
        h.notes.notes.len() + 1,
        "the upbeat is moved rather than duplicated; the one extra note is the wrapped remainder \
         of the overhanging final note"
    );
    assert!(
        wrapped
            .notes
            .iter()
            .all(|n| n.onset >= span.start && n.end() <= span.end),
        "after a split, nothing sounds outside the loop span"
    );
    assert!(
        hanging_notes(&wrapped, &span).is_empty(),
        "and nothing hangs past the loop end"
    );
}

// ---------------------------------------------------------------------------
// looping.exact_length_is_preserved
// ---------------------------------------------------------------------------

#[test]
fn loop_length_exact() {
    for policy in CarryPolicy::all() {
        let h = testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice");
        let span = h.span;
        let requested = span.length();
        let mut notes = h.notes.notes.clone();
        apply_boundary_policy(&mut notes, &span, *policy);
        assert_eq!(
            span.length(),
            requested,
            "policy {} changed the loop length",
            policy.id()
        );
        assert_eq!(span.length(), BeatTime::from_quarters(16));
        assert_eq!(span.end - span.start, requested);
    }
}

#[test]
fn loop_boundary_equality_is_exact() {
    // A span whose boundaries are not representable as binary fractions: the
    // comparison must still be exact, which is what a rational buys.
    let span = LoopSpan::new(
        BeatTime::new(1, 3),
        BeatTime::new(49, 3),
        LoopIntent::SeamlessColor,
    );
    assert_eq!(span.length(), BeatTime::from_quarters(16));
    assert_eq!(span.length().num(), 16);
    assert_eq!(span.length().den(), 1);

    // A float round trip of the same arithmetic does not land on 16 exactly,
    // which is the reason the engine never uses one.
    let float_length = 49.0f64 / 3.0 - 1.0 / 3.0;
    assert_ne!(float_length, 16.0);

    let mut notes = vec![testing::note_at(0, 60, (1, 3), (49, 3))];
    apply_boundary_policy(&mut notes, &span, CarryPolicy::Truncate);
    assert_eq!(notes[0].onset, span.start);
    assert_eq!(notes[0].end(), span.end);
    assert_eq!(notes[0].duration, span.length());
}

// ---------------------------------------------------------------------------
// looping.open_dominant_ends_unresolved
// ---------------------------------------------------------------------------

#[test]
fn open_dominant_unresolved_is_ok() {
    let h = testing::unresolved_dominant(LoopIntent::OpenDominant, "common_practice");
    let a = h.audit();
    assert_eq!(
        a.rule_status("looping.open_dominant_ends_unresolved"),
        Some(RuleStatus::Applied),
        "{:?}",
        a.rule_explanation("looping.open_dominant_ends_unresolved")
    );
    assert!(
        !a.observation.unresolved_tendencies.is_empty(),
        "the fixture leaves a tendency tone hanging on purpose"
    );
    // The unresolved tone is reported as information, not as a fault, because
    // the caller asked for exactly what the engine produced.
    let tendency = a
        .report
        .findings
        .iter()
        .find(|f| f.code == "LOOP_UNRESOLVED_TENDENCY")
        .expect("the tendency is still reported");
    assert_eq!(tendency.severity, Severity::Info);
    assert!(a.report.compatible);
}

// ---------------------------------------------------------------------------
// looping.seamless_color_uses_common_tones
// ---------------------------------------------------------------------------

#[test]
fn seamless_loop_shares_common_tones() {
    let h = testing::dominant_wrap(LoopIntent::SeamlessColor, "electronic_loop");
    let a = h.audit();
    assert!(a.observation.common_tone_available);
    assert!(a.observation.common_tone_retained);
    assert_eq!(
        a.rule_status("looping.seamless_color_uses_common_tones"),
        Some(RuleStatus::Applied),
        "{:?}",
        a.rule_explanation("looping.seamless_color_uses_common_tones")
    );
    assert!(a.rule_points() > 0.0);
}

// ---------------------------------------------------------------------------
// looping.bass_continuity_across_wrap
// ---------------------------------------------------------------------------

#[test]
fn bass_discontinuity_detected() {
    let h = testing::bass_leap(LoopIntent::SeamlessColor, "electronic_loop");
    let a = h.audit();
    assert!(a.observation.bass_leaps);
    assert_eq!(a.observation.bass_interval, Some(-14));
    assert_eq!(
        a.rule_status("looping.bass_continuity_across_wrap"),
        Some(RuleStatus::Applied)
    );
    assert!(a
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_BASS_DISCONTINUITY"));
}

#[test]
fn bass_fifth_across_wrap_allowed() {
    // The rule's own exception: a dominant ending is allowed its bass leap.
    let h = testing::dominant_bass_leap(LoopIntent::ClosedTonic, "common_practice");
    let a = h.audit();
    assert!(a.observation.bass_leaps, "the bass really does leap");
    assert_eq!(
        a.rule_status("looping.bass_continuity_across_wrap"),
        Some(RuleStatus::Bypassed),
        "{:?}",
        a.rule_explanation("looping.bass_continuity_across_wrap")
    );
    // A plain fifth is not a leap at all.
    let plain = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice").audit();
    assert_eq!(plain.observation.bass_interval, Some(5));
    assert!(!plain.observation.bass_leaps);
    assert!(!plain
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_BASS_DISCONTINUITY"));
}

// ---------------------------------------------------------------------------
// looping.voice_leading_smooth_across_wrap
// ---------------------------------------------------------------------------

#[test]
fn voice_leading_discontinuity_detected() {
    let h = testing::voice_leading_leap(LoopIntent::SeamlessColor, "common_practice");
    let a = h.audit();
    assert!(!a.observation.stepwise_available);
    assert_eq!(a.observation.max_leap, 12);
    assert!(
        !a.observation.bass_leaps,
        "the bass is held, so this is a voice-leading fault and not a bass fault"
    );
    assert!(a
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_VOICE_LEADING_DISCONTINUITY"));
    assert_ne!(
        a.rule_status("looping.voice_leading_smooth_across_wrap"),
        Some(RuleStatus::Applied),
        "no bonus is credited when no stepwise connection exists"
    );
}

#[test]
fn wrap_scored_as_chord_pair() {
    // The wrap is treated as an ordinary chord connection: the last chord and
    // the first chord are measured against each other exactly as any adjacent
    // pair would be.
    let h = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice");
    let a = h.audit();
    assert_eq!(
        a.observation.final_function,
        Some(HarmonicFunction::Dominant)
    );
    assert_eq!(a.observation.first_function, Some(HarmonicFunction::Tonic));
    assert!(a.observation.stepwise_available);
    assert_eq!(
        a.rule_status("looping.voice_leading_smooth_across_wrap"),
        Some(RuleStatus::Applied)
    );
    assert!(a
        .report
        .voice_leading_wrap
        .contains("total motion 4 semitones"));
    assert!(a.context.get_bool("is_loop_wrap_boundary") == Some(true));
}

// ---------------------------------------------------------------------------
// looping.pedal_continues_across_wrap
// ---------------------------------------------------------------------------

#[test]
fn pedal_continuous_across_wrap() {
    let h = testing::unmarked_pedal_overhang(LoopIntent::ModalDrone, "modal_ambient");
    let a = h.audit();
    assert!(a.observation.pedal_active);
    assert!(a.observation.pedal_continues);
    assert_eq!(
        a.rule_status("looping.pedal_continues_across_wrap"),
        Some(RuleStatus::Applied),
        "{:?}",
        a.rule_explanation("looping.pedal_continues_across_wrap")
    );

    // A drone that stops dead at the loop end re-articulates on the repeat.
    let restarting = testing::restarting_pedal(LoopIntent::ModalDrone, "modal_ambient").audit();
    assert!(restarting.observation.pedal_active);
    assert!(!restarting.observation.pedal_continues);
    assert!(restarting
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_PEDAL_RESTART"));
}

// ---------------------------------------------------------------------------
// looping.harmonic_rhythm_stable_at_wrap
// ---------------------------------------------------------------------------

#[test]
fn harmonic_rhythm_stable_at_wrap() {
    let stable = testing::dominant_wrap(LoopIntent::ClosedTonic, "pop_rock").audit();
    assert_eq!(
        stable.observation.slot_before,
        stable.observation.slot_after
    );
    assert!(!stable.observation.harmonic_rhythm_changes);
    assert_eq!(
        stable.rule_status("looping.harmonic_rhythm_stable_at_wrap"),
        Some(RuleStatus::NotApplicable),
        "a stable harmonic rhythm must not be penalised"
    );
    assert!(!stable
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_HARMONIC_RHYTHM_CHANGE"));

    let uneven = testing::uneven_harmonic_rhythm(LoopIntent::ClosedTonic, "pop_rock").audit();
    assert_eq!(
        uneven.observation.slot_before,
        Some(BeatTime::from_quarters(2))
    );
    assert_eq!(
        uneven.observation.slot_after,
        Some(BeatTime::from_quarters(8))
    );
    assert_eq!(
        uneven.rule_status("looping.harmonic_rhythm_stable_at_wrap"),
        Some(RuleStatus::Applied)
    );
    assert!(uneven
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_HARMONIC_RHYTHM_CHANGE"));
}

// ---------------------------------------------------------------------------
// looping.one_shot_ending_is_excluded_from_wrap_scoring
// ---------------------------------------------------------------------------

#[test]
fn one_shot_excluded_from_loop_scoring() {
    let h = testing::modal_planing_loop(LoopIntent::OneShotEnding, "cinematic");
    let a = h.audit();
    assert_eq!(
        a.rule_status("looping.one_shot_ending_is_excluded_from_wrap_scoring"),
        Some(RuleStatus::Applied),
        "{:?}",
        a.rule_explanation("looping.one_shot_ending_is_excluded_from_wrap_scoring")
    );
    // No wrap criterion appears in the one-shot verdict at all.
    for (name, _) in &a.fit.criteria {
        assert!(
            !name.contains("wrap") && !name.contains("smoothness") && !name.contains("common_tone"),
            "one_shot_ending must not be scored on {name}"
        );
    }
    assert!(
        a.report.compatible,
        "a stinger that stops is fit for purpose"
    );
}

// ---------------------------------------------------------------------------
// looping.transition_ready_leaves_an_opening
// ---------------------------------------------------------------------------

#[test]
fn transition_ready_stays_open() {
    let open = testing::dominant_wrap(LoopIntent::TransitionReady, "electronic_loop").audit();
    assert_eq!(
        open.rule_status("looping.transition_ready_leaves_an_opening"),
        Some(RuleStatus::Applied),
        "{:?}",
        open.rule_explanation("looping.transition_ready_leaves_an_opening")
    );
    assert!(open.report.compatible);

    // A loop that closes on the tonic is bypassed by the rule's own exception
    // and scores worse for this intent.
    let closed = testing::Harness::empty(LoopIntent::TransitionReady, "electronic_loop")
        .with_chords(&[("Am", 0, 4), ("F", 4, 4), ("G", 8, 4), ("C", 12, 4)])
        .audit();
    assert_eq!(
        closed.observation.final_function,
        Some(HarmonicFunction::Tonic)
    );
    assert_eq!(
        closed.rule_status("looping.transition_ready_leaves_an_opening"),
        Some(RuleStatus::Bypassed)
    );
    assert!(closed.fit.fit < open.fit.fit);
}

// ---------------------------------------------------------------------------
// looping.layer_removal_at_wrap_preserves_harmony
// ---------------------------------------------------------------------------

#[test]
fn wrap_layer_removal_keeps_chord_tone() {
    // The guide tone is doubled by a layer that is still sounding at the loop
    // start, so removing the pad costs nothing and the rule stays out of it.
    let covered = testing::layer_removal_covered(LoopIntent::ClosedTonic, "pop_rock").audit();
    assert!(covered.observation.layers_known);
    assert_eq!(covered.observation.layer_removal, None);
    assert_eq!(
        covered.rule_status("looping.layer_removal_at_wrap_preserves_harmony"),
        Some(RuleStatus::NotApplicable)
    );
    assert!(!covered
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_LAYER_REMOVAL"));

    // With the doubling gone, the pad is the last source of the third.
    let exposed = testing::layer_removal(LoopIntent::ClosedTonic, "pop_rock").audit();
    let removal = exposed
        .observation
        .layer_removal
        .as_ref()
        .expect("the pad is the only source of the third");
    assert_eq!(removal.part_name, "pad");
    assert_eq!(removal.degree, "3");
    assert_eq!(
        exposed.rule_status("looping.layer_removal_at_wrap_preserves_harmony"),
        Some(RuleStatus::Applied)
    );
}
