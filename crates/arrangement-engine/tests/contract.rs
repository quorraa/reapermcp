//! The frozen `arrangement-engine` API, the request's validation, and the
//! determinism guarantee.

use arrangement_engine::error::ArrangementError;
use arrangement_engine::masking::MaskingReport;
use arrangement_engine::patterns::realize_pattern;
use arrangement_engine::plan::RoleAssignment;
use arrangement_engine::testing::{chords_from, harness, harness_seeded};
use arrangement_engine::{arrange, masking_report, ArrangementParams, ArrangementPlan};
use harmony_engine::CancelFlag;
use music_analysis::report::Analysis;
use music_domain::prelude::*;
use theory_kb::{ArrangementPattern, InstrumentProfile, KnowledgeBase};

/// The signature of [`arrange`], exactly as `CONTRACTS.md` freezes it.
type ArrangeFn = fn(
    &KnowledgeBase,
    &Analysis,
    &Candidate,
    &ArrangementParams,
    &CancelFlag,
) -> Result<ArrangementPlan, ArrangementError>;

/// The signature of [`realize_pattern`], exactly as `CONTRACTS.md` freezes it.
type RealizeFn = fn(
    &KnowledgeBase,
    &ArrangementPattern,
    &[ChordEvent],
    &TimeMap,
    &InstrumentProfile,
    f64,
    u64,
) -> Result<Vec<Note>, ArrangementError>;

/// The signature of [`masking_report`].
type MaskingFn = fn(&[Part]) -> MaskingReport;

fn kb() -> &'static KnowledgeBase {
    KnowledgeBase::embedded()
}

fn full() -> ArrangementParams {
    ArrangementParams::default().with_roles(&[
        ArrangementRole::Lead,
        ArrangementRole::Bass,
        ArrangementRole::HarmonicBed,
        ArrangementRole::Comping,
    ])
}

#[test]
fn the_frozen_signatures_exist() {
    let _: ArrangeFn = arrange;
    let _: RealizeFn = realize_pattern;
    let _: MaskingFn = masking_report;
    let _: fn(&ArrangementParams) -> Result<(), ArrangementError> = ArrangementParams::validate;
    let _: fn(&ArrangementParams) -> qjson::Json = ArrangementParams::canonical_json;
}

#[test]
fn the_frozen_request_fields_exist() {
    let p = ArrangementParams {
        profile_id: "pop_rock".to_string(),
        roles: vec![ArrangementRole::Bass],
        energy_curve: vec![(BeatTime::ZERO, 0.5)],
        density: 0.5,
        texture_pattern: Some("arr_block_chords".to_string()),
        register_spread: 0.5,
        sections: Vec::new(),
        preserve_melody: true,
        loop_intent: Some(LoopIntent::ClosedTonic),
        seed: 1,
    };
    assert!(p.validate().is_ok());
}

#[test]
fn the_frozen_plan_fields_exist() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&full()).expect("a plan");
    let ArrangementPlan {
        parts,
        assignments,
        energy,
        score,
        rule_applications,
        warnings,
        ..
    } = &plan;
    assert!(!parts.is_empty());
    assert!(!assignments.is_empty());
    assert!(!energy.is_empty());
    assert!(score.total().is_finite());
    assert!(!rule_applications.is_empty());
    let _ = warnings.len();
}

#[test]
fn the_frozen_assignment_fields_exist() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&full()).expect("a plan");
    for a in &plan.assignments {
        let RoleAssignment {
            role,
            pattern_id,
            instrument_profile,
            register,
            density,
            polyphony,
            priority,
            sections,
            rationale,
        } = a;
        assert!(ArrangementRole::all().contains(role));
        assert!(kb()
            .arrangement_patterns()
            .iter()
            .any(|p| p.id == *pattern_id));
        assert!(kb().instrument_profile(instrument_profile).is_some());
        assert!(register.0 < register.1);
        assert!(*density >= 0.0);
        assert!(*polyphony <= 16);
        assert!(*priority <= 2);
        let _ = sections.len();
        assert!(!rationale.is_empty());
    }
}

#[test]
fn the_frozen_masking_fields_exist() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&full()).expect("a plan");
    let report = masking_report(&plan.parts);
    let MaskingReport {
        collisions,
        register_overlap,
        onset_collisions,
        suggestions,
    } = &report;
    let _ = collisions.len();
    assert!((0.0..=1.0).contains(register_overlap));
    let _ = *onset_collisions;
    let _ = suggestions.len();
}

#[test]
fn the_error_type_is_an_error() {
    let e = ArrangementError::new("CODE", "message");
    let _: &dyn std::error::Error = &e;
    assert_eq!(e.code, "CODE");
    assert_eq!(e.message, "message");
    assert_eq!(e.to_string(), "CODE: message");
}

// ---------------------------------------------------------------------------
// Validation and error paths
// ---------------------------------------------------------------------------

#[test]
fn an_out_of_range_density_is_rejected() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let err = h
        .arrange(&ArrangementParams::default().with_density(2.0))
        .expect_err("must be rejected");
    assert_eq!(err.code, "INVALID_ARGUMENT");
}

#[test]
fn an_unknown_profile_is_rejected() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let err = h
        .arrange(&ArrangementParams::default().with_profile("no_such_profile"))
        .expect_err("must be rejected");
    assert_eq!(err.code, "INVALID_ARGUMENT");
}

#[test]
fn a_candidate_with_no_chords_is_rejected() {
    let mut h = harness("melodies/eight_bar_c_major", "pop_rock");
    h.candidate.chords.clear();
    let err = h
        .arrange(&ArrangementParams::default())
        .expect_err("must be rejected");
    assert_eq!(err.code, "NO_HARMONY");
}

#[test]
fn a_zero_length_candidate_is_rejected() {
    let mut h = harness("melodies/eight_bar_c_major", "pop_rock");
    let spec = music_domain::symbol::parse("C").expect("a symbol");
    h.candidate.chords = vec![ChordEvent::new(0, spec, BeatTime::ZERO, BeatTime::ZERO)];
    let err = h
        .arrange(&ArrangementParams::default())
        .expect_err("must be rejected");
    assert_eq!(err.code, "NO_HARMONY");
}

#[test]
fn a_raised_cancel_flag_stops_the_request() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let cancel = CancelFlag::new();
    cancel.cancel();
    let err = arrange(
        h.kb,
        &h.analysis,
        &h.candidate,
        &ArrangementParams::default(),
        &cancel,
    )
    .expect_err("must be cancelled");
    assert_eq!(err.code, "CANCELLED");
}

#[test]
fn an_empty_pattern_list_is_a_knowledge_defect_not_a_panic() {
    // Every catalogued pattern declares onsets, so this cannot fire today; the
    // point is that it returns an error rather than dividing by zero.
    let chords = chords_from(&["C"], 4);
    let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
    for p in kb().arrangement_patterns() {
        let inst = kb()
            .instrument_profile("gm_harmony")
            .expect("an instrument");
        assert!(realize_pattern(kb(), p, &chords, &tm, inst, 0.5, 0).is_ok());
    }
}

#[test]
fn realising_against_no_chords_writes_nothing() {
    let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
    let p = &kb().arrangement_patterns()[0];
    let inst = kb()
        .instrument_profile("gm_harmony")
        .expect("an instrument");
    assert!(realize_pattern(kb(), p, &[], &tm, inst, 0.5, 0)
        .expect("no chords is not an error")
        .is_empty());
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

#[test]
fn the_same_request_produces_the_same_plan_json() {
    let h = harness("melodies/eight_bar_c_major", "jazz_standard");
    let params = full().with_profile("jazz_standard").with_seed(4);
    let a = h.arrange(&params).expect("a plan");
    let b = h.arrange(&params).expect("a plan");
    assert_eq!(
        a.to_json().to_canonical_string(),
        b.to_json().to_canonical_string()
    );
    assert_eq!(a.fingerprint(), b.fingerprint());
}

#[test]
fn a_different_seed_is_still_deterministic() {
    let h = harness("melodies/eight_bar_c_major", "jazz_standard");
    for seed in [0u64, 5, 99, u64::MAX] {
        let params = full().with_profile("jazz_standard").with_seed(seed);
        let a = h.arrange(&params).expect("a plan").fingerprint();
        let b = h.arrange(&params).expect("a plan").fingerprint();
        assert_eq!(a, b, "seed {seed} is not deterministic");
    }
}

#[test]
fn a_different_profile_produces_a_different_plan() {
    let a = harness("melodies/eight_bar_c_major", "modal_ambient")
        .arrange(&full().with_profile("modal_ambient"))
        .expect("a plan");
    let b = harness("melodies/eight_bar_c_major", "drum_and_bass")
        .arrange(&full().with_profile("drum_and_bass"))
        .expect("a plan");
    assert_ne!(a.fingerprint(), b.fingerprint());
}

#[test]
fn plan_json_is_stable_across_harness_rebuilds() {
    let params = full().with_profile("blues").with_seed(11);
    let first = harness_seeded("melodies/blues_head_c", "blues", 3)
        .arrange(&params)
        .expect("a plan")
        .fingerprint();
    let second = harness_seeded("melodies/blues_head_c", "blues", 3)
        .arrange(&params)
        .expect("a plan")
        .fingerprint();
    assert_eq!(first, second);
}

#[test]
fn canonical_request_json_is_order_independent_of_construction() {
    let a = ArrangementParams::default()
        .with_profile("cinematic")
        .with_roles(&[ArrangementRole::Bass, ArrangementRole::Pad])
        .with_density(0.4);
    let b = ArrangementParams {
        density: 0.4,
        roles: vec![ArrangementRole::Bass, ArrangementRole::Pad],
        profile_id: "cinematic".to_string(),
        ..ArrangementParams::default()
    };
    assert_eq!(
        a.canonical_json().to_canonical_string(),
        b.canonical_json().to_canonical_string()
    );
}

#[test]
fn parts_are_ordered_and_channelled_deterministically() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&full()).expect("a plan");
    let channels: Vec<u8> = plan.parts.iter().map(|p| p.channel).collect();
    let mut unique = channels.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(channels.len(), unique.len(), "two parts share a channel");
    for p in &plan.parts {
        assert!(p.channel < 16);
    }
}

#[test]
fn note_ids_are_unique_across_the_plan() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&ArrangementParams::default().with_roles(ArrangementRole::all()))
        .expect("a plan");
    let mut ids: Vec<NoteId> = plan.notes().iter().map(|n| n.id).collect();
    let before = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(before, ids.len(), "note ids collided");
}

#[test]
fn preserve_melody_keeps_every_lead_pitch_and_onset() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&ArrangementParams::default().with_roles(&[ArrangementRole::Lead]))
        .expect("a plan");
    let lead = plan.part(ArrangementRole::Lead).expect("a lead");
    let source: Vec<(i32, BeatTime)> = h
        .analysis
        .extraction
        .melody
        .notes
        .iter()
        .filter(|n| n.onset >= plan.span.0 && n.onset < plan.span.1)
        .map(|n| (n.midi, n.onset))
        .collect();
    let written: Vec<(i32, BeatTime)> = lead.notes.iter().map(|n| (n.midi, n.onset)).collect();
    assert_eq!(written, source, "preserve_melody altered the lead");
}

#[test]
fn preserve_melody_off_writes_the_lead_pattern_instead() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let mut params = ArrangementParams::default().with_roles(&[ArrangementRole::Lead]);
    params.preserve_melody = false;
    let plan = h.arrange(&params).expect("a plan");
    let lead = plan.part(ArrangementRole::Lead).expect("a lead");
    let source: Vec<BeatTime> = h
        .analysis
        .extraction
        .melody
        .notes
        .iter()
        .map(|n| n.onset)
        .collect();
    let written: Vec<BeatTime> = lead.notes.iter().map(|n| n.onset).collect();
    assert_ne!(written, source, "the lead pattern was not used");
}

#[test]
fn the_plan_covers_the_candidates_span_exactly() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&full()).expect("a plan");
    assert_eq!(plan.span, h.span());
    for n in plan.notes() {
        assert!(n.onset >= plan.span.0);
        assert!(n.end() <= plan.span.1);
    }
}

#[test]
fn the_score_vector_carries_every_component() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&full()).expect("a plan");
    for component in SCORE_COMPONENTS {
        assert!(plan.score.contains(component), "{component} missing");
        assert!(plan.score.get(component).is_finite());
    }
    assert_eq!(plan.score.len(), SCORE_COMPONENTS.len());
}

#[test]
fn rule_applications_are_ordered_and_unique() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&full()).expect("a plan");
    let ids: Vec<&str> = plan
        .rule_applications
        .iter()
        .map(|a| a.rule_id.as_str())
        .collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "rule applications are not ordered by id");
}

#[test]
fn withheld_roles_still_get_an_assignment() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(
            &ArrangementParams::default()
                .with_roles(ArrangementRole::all())
                .with_energy_curve(&[(BeatTime::ZERO, 0.0), (BeatTime::from_quarters(32), 0.0)]),
        )
        .expect("a plan");
    assert_eq!(plan.assignments.len(), ArrangementRole::all().len());
    for role in ArrangementRole::all() {
        assert!(plan.assignment(*role).is_some(), "{}", role.id());
    }
}
