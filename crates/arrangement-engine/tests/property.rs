//! A property sweep across profiles, seeds, fixtures and controls.
//!
//! Nothing here checks a specific musical decision. These are the invariants
//! that must hold for *every* plan the engine can produce: MIDI bounds, strictly
//! positive durations, pitches inside the instrument profile, notes inside the
//! generated span, monophonic parts that never overlap, and byte-identical
//! output for identical input.

use arrangement_engine::density;
use arrangement_engine::patterns::realize_pattern;
use arrangement_engine::testing::{chords_from, harness, harness_seeded, FIXTURES, PROFILE_IDS};
use arrangement_engine::{ArrangementParams, ArrangementPlan};
use music_domain::prelude::*;
use theory_kb::KnowledgeBase;

fn kb() -> &'static KnowledgeBase {
    KnowledgeBase::embedded()
}

/// The seeds the sweep uses.
const SEEDS: &[u64] = &[0, 1, 7, 42, 1_000_003];

/// The role sets the sweep uses.
fn role_sets() -> Vec<Vec<ArrangementRole>> {
    vec![
        vec![ArrangementRole::Lead, ArrangementRole::Bass],
        vec![
            ArrangementRole::Bass,
            ArrangementRole::HarmonicBed,
            ArrangementRole::Pad,
        ],
        vec![
            ArrangementRole::Lead,
            ArrangementRole::Counterlead,
            ArrangementRole::Bass,
            ArrangementRole::Comping,
            ArrangementRole::Ostinato,
        ],
        ArrangementRole::all().to_vec(),
    ]
}

/// Asserts every invariant a finished plan must satisfy.
fn check(plan: &ArrangementPlan, label: &str) {
    for (i, part) in plan.parts.iter().enumerate() {
        let profile = kb()
            .instrument_profile(part.instrument_profile.as_deref().unwrap_or_default())
            .unwrap_or_else(|| panic!("{label}: {} has no instrument", part.role.id()));
        for n in &part.notes {
            assert!(
                (0..=127).contains(&n.midi),
                "{label}: {} wrote MIDI {}",
                part.role.id(),
                n.midi
            );
            assert!(
                profile.range.contains(n.midi),
                "{label}: {} wrote {} outside {}'s range",
                part.role.id(),
                n.midi,
                profile.id
            );
            assert!(
                n.duration.is_positive(),
                "{label}: {} wrote a non-positive duration",
                part.role.id()
            );
            assert!(
                n.onset >= plan.span.0 && n.onset < plan.span.1,
                "{label}: {} started outside the span",
                part.role.id()
            );
            assert!(
                n.end() <= plan.span.1,
                "{label}: {} sounded past the span",
                part.role.id()
            );
            assert!(
                (1..=127).contains(&n.velocity),
                "{label}: {} wrote velocity {}",
                part.role.id(),
                n.velocity
            );
            assert_eq!(
                n.pitch.midi(),
                n.midi,
                "{label}: {} spelled {} as {}",
                part.role.id(),
                n.midi,
                n.pitch.to_ascii()
            );
        }
        if !part.polyphonic {
            let mut sorted = part.notes.clone();
            sorted.sort_by_key(|a| a.onset);
            for w in sorted.windows(2) {
                assert!(
                    !w[0].overlaps(&w[1]),
                    "{label}: monophonic {} overlapped itself",
                    part.role.id()
                );
            }
        }
        let measured = density::max_polyphony(&part.notes);
        assert!(
            measured <= profile.polyphony.max(1) as usize,
            "{label}: {} sounded {measured} voices on {}",
            part.role.id(),
            profile.id
        );
        assert_eq!(plan.metrics[i].notes, part.notes.len());
    }
    assert_eq!(plan.metrics.len(), plan.parts.len());
    assert!(!plan.energy.is_empty(), "{label}: no energy curve");
    for (_, v) in &plan.energy {
        assert!((0.0..=1.0).contains(v), "{label}: energy out of range");
    }
    for component in SCORE_COMPONENTS {
        assert!(
            plan.score.contains(component),
            "{label}: the score vector is missing {component}"
        );
    }
    assert!(
        plan.score.total().is_finite(),
        "{label}: score is not finite"
    );
}

#[test]
fn every_profile_produces_a_valid_plan() {
    for profile in PROFILE_IDS {
        let h = harness("melodies/eight_bar_c_major", profile);
        for roles in role_sets() {
            let plan = h
                .arrange(
                    &ArrangementParams::default()
                        .with_profile(profile)
                        .with_roles(&roles),
                )
                .unwrap_or_else(|e| panic!("{profile}: {e}"));
            check(&plan, profile);
        }
    }
}

#[test]
fn every_seed_produces_a_valid_plan() {
    let h = harness("melodies/eight_bar_c_major", "jazz_standard");
    for seed in SEEDS {
        let plan = h
            .arrange(
                &ArrangementParams::default()
                    .with_profile("jazz_standard")
                    .with_roles(&role_sets()[2])
                    .with_seed(*seed),
            )
            .expect("a plan");
        check(&plan, &format!("seed {seed}"));
    }
}

#[test]
fn every_fixture_produces_a_valid_plan() {
    for fixture in FIXTURES {
        let h = harness(fixture, "cinematic");
        let plan = h
            .arrange(
                &ArrangementParams::default()
                    .with_profile("cinematic")
                    .with_roles(&role_sets()[2]),
            )
            .unwrap_or_else(|e| panic!("{fixture}: {e}"));
        check(&plan, fixture);
    }
}

#[test]
fn every_density_setting_produces_a_valid_plan() {
    let h = harness("melodies/eight_bar_c_major", "electronic_loop");
    for step in 0..=10 {
        let d = f64::from(step) / 10.0;
        let plan = h
            .arrange(
                &ArrangementParams::default()
                    .with_profile("electronic_loop")
                    .with_roles(&role_sets()[2])
                    .with_density(d),
            )
            .expect("a plan");
        check(&plan, &format!("density {d}"));
    }
}

#[test]
fn every_register_spread_produces_a_valid_plan() {
    let h = harness("melodies/eight_bar_c_major", "neo_soul_rnb");
    for step in 0..=10 {
        let s = f64::from(step) / 10.0;
        let plan = h
            .arrange(
                &ArrangementParams::default()
                    .with_profile("neo_soul_rnb")
                    .with_roles(&role_sets()[2])
                    .with_register_spread(s),
            )
            .expect("a plan");
        check(&plan, &format!("spread {s}"));
    }
}

#[test]
fn every_energy_level_produces_a_valid_plan() {
    let h = harness("melodies/eight_bar_c_major", "modal_ambient");
    for step in 0..=10 {
        let e = f64::from(step) / 10.0;
        let curve = [(BeatTime::ZERO, e), (BeatTime::from_quarters(32), e)];
        let plan = h
            .arrange(
                &ArrangementParams::default()
                    .with_profile("modal_ambient")
                    .with_roles(&role_sets()[2])
                    .with_energy_curve(&curve),
            )
            .expect("a plan");
        check(&plan, &format!("energy {e}"));
    }
}

#[test]
fn every_loop_intent_produces_a_valid_plan() {
    let h = harness("melodies/dorian_vamp_d", "modal_ambient");
    for intent in [
        LoopIntent::ClosedTonic,
        LoopIntent::OpenDominant,
        LoopIntent::ModalDrone,
        LoopIntent::SeamlessColor,
        LoopIntent::TransitionReady,
        LoopIntent::OneShotEnding,
    ] {
        let plan = h
            .arrange(
                &ArrangementParams::default()
                    .with_profile("modal_ambient")
                    .with_roles(&role_sets()[2])
                    .with_loop_intent(intent),
            )
            .expect("a plan");
        check(&plan, intent.id());
    }
}

#[test]
fn the_full_sweep_of_profiles_and_seeds_holds() {
    for profile in PROFILE_IDS {
        for seed in [0u64, 7, 1_000_003] {
            let h = harness_seeded("melodies/eight_bar_c_major", profile, 7);
            let plan = h
                .arrange(
                    &ArrangementParams::default()
                        .with_profile(profile)
                        .with_roles(&role_sets()[3])
                        .with_seed(seed),
                )
                .unwrap_or_else(|e| panic!("{profile}/{seed}: {e}"));
            check(&plan, &format!("{profile}/{seed}"));
            assert!(
                plan.layer_count() >= 1,
                "{profile}/{seed} produced nothing at all"
            );
        }
    }
}

#[test]
fn every_pattern_and_instrument_pairing_is_safe() {
    let chords = chords_from(&["Cmaj7", "Am7", "Fmaj7", "G7"], 4);
    let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
    for p in kb().arrangement_patterns() {
        for inst in kb().instrument_profiles() {
            let notes =
                realize_pattern(kb(), p, &chords, &tm, inst, 0.8, 5).expect("a realisation");
            for n in &notes {
                assert!(
                    inst.range.contains(n.midi),
                    "{} on {} wrote {}",
                    p.id,
                    inst.id,
                    n.midi
                );
                assert!(n.duration.is_positive());
                assert!((0..=127).contains(&n.midi));
                assert!((1..=127).contains(&n.velocity));
            }
            assert!(
                density::max_polyphony(&notes) <= inst.polyphony.max(1) as usize,
                "{} on {} exceeded polyphony",
                p.id,
                inst.id
            );
        }
    }
}

#[test]
fn plans_are_deterministic_across_repeated_calls() {
    let h = harness("melodies/eight_bar_c_major", "blues");
    let params = ArrangementParams::default()
        .with_profile("blues")
        .with_roles(&role_sets()[2])
        .with_seed(31);
    let first = h.arrange(&params).expect("a plan").fingerprint();
    for _ in 0..4 {
        assert_eq!(
            h.arrange(&params).expect("a plan").fingerprint(),
            first,
            "the same request produced a different plan"
        );
    }
}

#[test]
fn different_seeds_never_break_an_invariant() {
    let h = harness("melodies/waltz_suspensions", "common_practice");
    for seed in SEEDS {
        let plan = h
            .arrange(
                &ArrangementParams::default()
                    .with_profile("common_practice")
                    .with_roles(&role_sets()[1])
                    .with_seed(*seed),
            )
            .expect("a plan");
        check(&plan, &format!("waltz seed {seed}"));
    }
}

#[test]
fn a_three_four_fixture_is_barred_correctly() {
    let h = harness("melodies/waltz_suspensions", "common_practice");
    let plan = h
        .arrange(
            &ArrangementParams::default()
                .with_profile("common_practice")
                .with_roles(&[ArrangementRole::HarmonicBed, ArrangementRole::Bass]),
        )
        .expect("a plan");
    let tm = h.time_map();
    let bar = tm.meter_at(plan.span.0).bar_length_qn();
    assert_eq!(bar, BeatTime::from_quarters(3), "the fixture is in 3/4");
    for part in &plan.parts {
        for n in &part.notes {
            assert!(
                tm.position_in_bar(n.onset) < bar,
                "{} placed a note outside the bar",
                part.role.id()
            );
        }
    }
}

#[test]
fn plan_json_round_trips_through_the_parts_it_describes() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&ArrangementParams::default().with_roles(&role_sets()[2]))
        .expect("a plan");
    let json = plan.to_json();
    let parts = json
        .get("parts")
        .and_then(qjson::Json::as_arr)
        .expect("parts");
    assert_eq!(parts.len(), plan.parts.len());
    for (i, v) in parts.iter().enumerate() {
        let decoded = Part::from_json(v).expect("a decodable part");
        assert_eq!(decoded.role, plan.parts[i].role);
        assert_eq!(decoded.notes.len(), plan.parts[i].notes.len());
    }
}
