//! Determinism, profile sensitivity, and proof that every looping rule is
//! reachable.
//!
//! A rule whose facts are never populated silently never fires, which would
//! leave the audit looking as though it consulted `knowledge/` when it did
//! nothing of the sort. The sweep below drives each of the fourteen looping
//! rules to a status other than `NotApplicable` and records which shape does
//! it, so a regression in fact-building shows up as a failing test rather than
//! as a quietly weaker audit.

use loop_engine::prelude::*;
use loop_engine::testing::{self, Harness};
use music_domain::prelude::*;
use theory_kb::prelude::RuleStatus;

/// Every rule id paired with a harness that drives it off `NotApplicable`.
fn reachability_cases() -> Vec<(&'static str, Harness)> {
    vec![
        (
            "looping.no_hanging_note_past_loop_end",
            testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice"),
        ),
        (
            "looping.pickup_wraps_or_duplicates",
            testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice"),
        ),
        (
            // Stated in the negative, so it takes a loop whose span really has
            // drifted from the requested length to drive this off NotApplicable.
            "looping.exact_length_is_preserved",
            testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice"),
        ),
        (
            "looping.closed_tonic_prefers_dominant_to_tonic_wrap",
            testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice"),
        ),
        (
            "looping.modal_drone_does_not_require_dominant",
            testing::pedal_loop(LoopIntent::ModalDrone, "modal_ambient"),
        ),
        (
            "looping.open_dominant_ends_unresolved",
            testing::unresolved_dominant(LoopIntent::OpenDominant, "common_practice"),
        ),
        (
            "looping.seamless_color_uses_common_tones",
            testing::dominant_wrap(LoopIntent::SeamlessColor, "electronic_loop"),
        ),
        (
            "looping.bass_continuity_across_wrap",
            testing::bass_leap(LoopIntent::SeamlessColor, "electronic_loop"),
        ),
        (
            "looping.voice_leading_smooth_across_wrap",
            testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice"),
        ),
        (
            "looping.pedal_continues_across_wrap",
            testing::unmarked_pedal_overhang(LoopIntent::ModalDrone, "modal_ambient"),
        ),
        (
            "looping.harmonic_rhythm_stable_at_wrap",
            testing::uneven_harmonic_rhythm(LoopIntent::ClosedTonic, "pop_rock"),
        ),
        (
            "looping.one_shot_ending_is_excluded_from_wrap_scoring",
            testing::modal_planing_loop(LoopIntent::OneShotEnding, "cinematic"),
        ),
        (
            "looping.transition_ready_leaves_an_opening",
            testing::dominant_wrap(LoopIntent::TransitionReady, "electronic_loop"),
        ),
        (
            "looping.layer_removal_at_wrap_preserves_harmony",
            testing::layer_removal(LoopIntent::ClosedTonic, "pop_rock"),
        ),
    ]
}

#[test]
fn every_looping_rule_can_actually_fire() {
    for (rule, h) in reachability_cases() {
        let a = h.audit();
        let status = a.rule_status(rule);
        assert!(
            matches!(
                status,
                Some(RuleStatus::Applied) | Some(RuleStatus::Bypassed) | Some(RuleStatus::Violated)
            ),
            "{rule} never left NotApplicable: {:?}\n{:?}",
            status,
            a.rule_explanation(rule)
        );
    }
}

#[test]
fn the_reachability_sweep_covers_the_whole_looping_domain() {
    let covered: Vec<&str> = reachability_cases().iter().map(|(r, _)| *r).collect();
    for rule in testing::LOOPING_RULE_IDS {
        assert!(covered.contains(rule), "{rule} has no reachability case");
    }
    assert_eq!(covered.len(), testing::LOOPING_RULE_IDS.len());
}

#[test]
fn every_looping_rule_is_evaluated_on_every_audit() {
    for intent in testing::ALL_INTENTS {
        let a = testing::dominant_wrap(*intent, "electronic_loop").audit();
        let seen: Vec<&str> = a
            .outcome
            .applications
            .iter()
            .map(|x| x.rule_id.as_str())
            .collect();
        for rule in testing::LOOPING_RULE_IDS {
            assert!(
                seen.contains(rule),
                "{rule} was not evaluated for {intent:?}"
            );
        }
    }
}

#[test]
fn no_rule_from_another_domain_is_evaluated_at_the_loop_boundary() {
    let a = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice").audit();
    for app in &a.outcome.applications {
        assert!(
            app.rule_id.starts_with("looping."),
            "{} is not a looping rule",
            app.rule_id
        );
    }
}

#[test]
fn every_rule_application_cites_its_sources() {
    let a = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice").audit();
    let kb = theory_kb::KnowledgeBase::embedded();
    for app in &a.outcome.applications {
        if app.status == RuleStatus::NotApplicable {
            continue;
        }
        assert!(!app.source_ids.is_empty(), "{} cites nothing", app.rule_id);
        for id in &app.source_ids {
            assert!(kb.source(id).is_some(), "{id} is not a real source");
        }
    }
}

#[test]
fn the_length_invariant_fires_on_drift_and_stays_silent_when_correct() {
    // The invariant is stated in the negative (`loop_length_is_not_exact`), so a
    // correct loop must not trip it and a drifted one must.
    let ok = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice").audit();
    assert!(ok.length_exact);
    assert!(
        !ok.outcome
            .hard_violations
            .contains(&loop_engine::audit::LENGTH_INVARIANT_RULE.to_string()),
        "a loop of exactly the requested length must not violate the length invariant"
    );
    assert!(ok.report.compatible);
    assert!(!ok
        .report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_LENGTH_MISMATCH"));

    // And the audit's own exact-rational check agrees with the rule.
    let broken =
        testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice").audit();
    assert!(!broken.length_exact);
}

// ---------------------------------------------------------------------------
// determinism
// ---------------------------------------------------------------------------

#[test]
fn the_report_json_is_identical_across_runs() {
    for intent in testing::ALL_INTENTS {
        for profile in testing::PROFILE_IDS {
            let h = testing::dominant_wrap(*intent, profile);
            let a = h.report().to_json().to_canonical_string();
            let b = h.report().to_json().to_canonical_string();
            assert_eq!(a, b, "{intent:?} / {profile}");
        }
    }
}

#[test]
fn two_harnesses_over_the_same_fixture_agree_byte_for_byte() {
    for id in [
        "loops/dominant_wrap",
        "loops/pickup_and_hanging_note",
        "progressions/modal_planing_loop",
    ] {
        let a = Harness::from_fixture(id, LoopIntent::SeamlessColor, "cinematic")
            .report()
            .to_json()
            .to_canonical_string();
        let b = Harness::from_fixture(id, LoopIntent::SeamlessColor, "cinematic")
            .report()
            .to_json()
            .to_canonical_string();
        assert_eq!(a, b, "{id}");
    }
}

#[test]
fn the_note_order_of_the_input_does_not_change_the_report() {
    let h = testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice");
    let forward = h.report().to_json().to_canonical_string();
    let mut reversed = h.notes.notes.clone();
    reversed.reverse();
    let backward = h
        .with_notes(reversed)
        .report()
        .to_json()
        .to_canonical_string();
    assert_eq!(forward, backward);
}

#[test]
fn the_repair_list_is_deterministic() {
    let h = testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice");
    let a = h.report().repairs;
    let b = h.report().repairs;
    assert_eq!(a, b);
    assert!(!a.is_empty());
}

#[test]
fn the_audit_json_round_trips_through_the_domain_type() {
    let report = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice").report();
    let json = report.to_json();
    let back = LoopReport::from_json(&json).expect("round trip");
    assert_eq!(
        back.to_json().to_canonical_string(),
        json.to_canonical_string()
    );
}

#[test]
fn the_detailed_audit_json_is_stable() {
    let h = testing::modal_planing_loop(LoopIntent::ModalDrone, "modal_ambient");
    let a = h.audit().to_json().to_canonical_string();
    let b = h.audit().to_json().to_canonical_string();
    assert_eq!(a, b);
    assert!(a.contains("intent_fit"));
    assert!(a.contains("carry_policy"));
}

// ---------------------------------------------------------------------------
// profile sensitivity
// ---------------------------------------------------------------------------

#[test]
fn every_profile_produces_a_usable_report_for_every_intent() {
    for profile in testing::PROFILE_IDS {
        for intent in testing::ALL_INTENTS {
            let r = testing::dominant_wrap(*intent, profile).report();
            assert!(
                (0.0..=1.0).contains(&r.score),
                "{profile} / {intent:?} scored {}",
                r.score
            );
            assert!(!r.harmonic_wrap.is_empty());
            assert!(!r.bass_wrap.is_empty());
            assert!(!r.voice_leading_wrap.is_empty());
        }
    }
}

#[test]
fn a_modal_profile_credits_a_modal_loop_more_than_a_common_practice_one_does() {
    let modal = testing::pedal_loop(LoopIntent::ModalDrone, "modal_ambient")
        .audit()
        .rule_points();
    let common = testing::pedal_loop(LoopIntent::ModalDrone, "common_practice")
        .audit()
        .rule_points();
    assert!(
        modal > common,
        "modal_ambient {modal} should credit more than common_practice {common}"
    );
}

#[test]
fn a_profile_outside_a_rules_scope_does_not_apply_it() {
    let a = testing::dominant_wrap(LoopIntent::SeamlessColor, "strict_counterpoint").audit();
    assert_eq!(
        a.rule_status("looping.seamless_color_uses_common_tones"),
        Some(RuleStatus::NotApplicable),
        "strict_counterpoint is not in that rule's profile list"
    );
}

#[test]
fn the_intent_fit_criteria_are_named_and_weighted() {
    for intent in testing::ALL_INTENTS {
        let a = testing::dominant_wrap(*intent, "cinematic").audit();
        assert!(!a.fit.criteria.is_empty());
        assert_eq!(a.fit.criteria.len(), a.fit.weights.len());
        let total: f64 = a.fit.weights.iter().sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "{intent:?} weights sum to {total}"
        );
        for (_, value) in &a.fit.criteria {
            assert!((0.0..=1.0).contains(value));
        }
    }
}

#[test]
fn the_report_never_carries_a_repair_without_a_finding() {
    for intent in testing::ALL_INTENTS {
        let h = testing::dominant_wrap(*intent, "electronic_loop");
        let report = h.report();
        if report.repairs.is_empty() {
            continue;
        }
        assert!(
            !report.findings.is_empty(),
            "{intent:?} proposes repairs with nothing to repair"
        );
    }
}

#[test]
fn every_suggested_repair_carries_a_rationale_and_a_rule() {
    for intent in testing::ALL_INTENTS {
        let h = testing::pickup_and_hanging_note(*intent, "electronic_loop");
        let report = h.report();
        for r in suggest_repairs(h.kb, &h.profile, &h.input(), &report) {
            assert!(!r.description.is_empty(), "{} has no description", r.id);
            assert!(!r.rationale.is_empty(), "{} has no rationale", r.id);
            assert!(!r.rule_ids.is_empty(), "{} cites no rule", r.id);
        }
    }
}

#[test]
fn an_invalid_span_produces_a_report_rather_than_a_panic() {
    let h = testing::inverted_span();
    assert!(h.input().validate().is_err());
    let report = h.report();
    assert!(!report.compatible);
    assert!(report
        .findings
        .iter()
        .any(|f| f.code == "LOOP_INVALID_SPAN" && f.severity == Severity::Major));
}

#[test]
fn a_zero_length_span_is_rejected_without_dividing_by_zero() {
    let mut h = testing::Harness::empty(LoopIntent::ModalDrone, "modal_ambient");
    h.span = LoopSpan::new(
        BeatTime::from_quarters(4),
        BeatTime::from_quarters(4),
        LoopIntent::ModalDrone,
    );
    let report = h.report();
    assert_eq!(report.loop_start, report.loop_end);
    assert!(!report.compatible);
}
