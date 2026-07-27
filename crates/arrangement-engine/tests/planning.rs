//! The three planning levels, and the contrast levers.
//!
//! Phrase level: entrances, exits, answers, fills, cadential support, rest
//! placement, repetition variation. Section level: density, register, layer
//! count, texture, rhythmic activity, harmonic complexity, contrast, transition
//! preparation. Whole-loop level: long-term energy, the maximum-density point,
//! strategic silence, recurring roles, tonal return, open versus closed ending,
//! the loop transition.

use arrangement_engine::density::{self, PartMetrics};
use arrangement_engine::energy::{self, Ending};
use arrangement_engine::masking;
use arrangement_engine::patterns;
use arrangement_engine::phrase::PhraseFrame;
use arrangement_engine::sections;
use arrangement_engine::testing::{chords_from, harness};
use arrangement_engine::{ArrangementParams, ArrangementPlan};
use music_domain::prelude::*;
use theory_kb::KnowledgeBase;

fn kb() -> &'static KnowledgeBase {
    KnowledgeBase::embedded()
}

fn tm() -> TimeMap {
    TimeMap::constant(120.0, TimeSignature::new(4, 4))
}

fn wide_roles() -> Vec<ArrangementRole> {
    vec![
        ArrangementRole::Lead,
        ArrangementRole::Bass,
        ArrangementRole::HarmonicBed,
        ArrangementRole::Pad,
        ArrangementRole::Comping,
        ArrangementRole::Ostinato,
        ArrangementRole::Texture,
    ]
}

fn request() -> ArrangementParams {
    ArrangementParams::default().with_roles(&wide_roles())
}

fn two_sections() -> Vec<Section> {
    vec![
        Section {
            id: "verse".to_string(),
            start: BeatTime::ZERO,
            end: BeatTime::from_quarters(16),
            role: "verse".to_string(),
            energy: 0.2,
        },
        Section {
            id: "chorus".to_string(),
            start: BeatTime::from_quarters(16),
            end: BeatTime::from_quarters(32),
            role: "chorus".to_string(),
            energy: 0.95,
        },
    ]
}

fn slice(part: &Part, from: BeatTime, to: BeatTime) -> PartMetrics {
    let notes: Vec<Note> = part
        .notes
        .iter()
        .filter(|n| n.onset >= from && n.onset < to)
        .cloned()
        .collect();
    density::measure(&notes, (from, to))
}

// ---------------------------------------------------------------------------
// Phrase level
// ---------------------------------------------------------------------------

#[test]
fn the_phrase_frame_is_read_from_the_analysis() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let frame = PhraseFrame::from_analysis(&h.analysis, h.span());
    assert!(!frame.phrases.is_empty());
    for (s, e) in &frame.phrases {
        assert!(e > s);
        assert!(*s >= h.span().0 && *e <= h.span().1);
    }
    assert_eq!(
        frame.lead_onsets.len(),
        {
            let mut o: Vec<BeatTime> = h
                .analysis
                .extraction
                .melody
                .notes
                .iter()
                .map(|n| n.onset)
                .collect();
            o.sort();
            o.dedup();
            o.len()
        },
        "the frame must carry every distinct lead onset"
    );
}

#[test]
fn phrase_windows_tile_the_span_without_overlap() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let frame = PhraseFrame::from_analysis(&h.analysis, h.span());
    for w in frame.phrases.windows(2) {
        assert!(
            w[0].1 <= w[1].0,
            "phrases {:?} and {:?} overlap",
            w[0],
            w[1]
        );
    }
}

#[test]
fn structural_roles_enter_at_the_top_and_decorative_ones_may_not() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&request()).expect("a plan");
    for role in [ArrangementRole::Lead, ArrangementRole::Bass] {
        let part = plan.part(role).unwrap_or_else(|| panic!("{}", role.id()));
        let first = part.notes.iter().map(|n| n.onset).min().expect("notes");
        assert!(
            first < plan.span.0 + BeatTime::from_quarters(8),
            "{} entered late at {first}",
            role.id()
        );
    }
}

#[test]
fn an_answering_part_sounds_where_the_lead_does_not() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(
            &ArrangementParams::default()
                .with_roles(&[ArrangementRole::Lead, ArrangementRole::Counterlead]),
        )
        .expect("a plan");
    let counter = plan.part(ArrangementRole::Counterlead).expect("a counter");
    let assignment = plan
        .assignment(ArrangementRole::Counterlead)
        .expect("an assignment");
    let p = kb()
        .arrangement_patterns()
        .iter()
        .find(|p| p.id == assignment.pattern_id)
        .expect("the pattern");
    if p.rhythmic_activity != "phrase_gaps" {
        return;
    }
    let frame = PhraseFrame::from_analysis(&h.analysis, plan.span);
    let windows = frame.answer_windows();
    for n in &counter.notes {
        assert!(
            windows.iter().any(|(s, e)| n.onset >= *s && n.onset < *e),
            "an answering note at {} is outside every answer window",
            n.onset
        );
    }
}

#[test]
fn cadential_arrivals_are_known_to_the_frame() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let frame = PhraseFrame::from_analysis(&h.analysis, h.span());
    for c in &frame.cadences {
        assert!(frame.is_cadential(*c));
        assert!(*c >= h.span().0 && *c <= h.span().1);
    }
}

#[test]
fn repetition_variation_changes_the_repeat() {
    let frame = PhraseFrame::flat((BeatTime::ZERO, BeatTime::from_quarters(32)));
    let scales: Vec<f64> = (0..4).map(|i| frame.variation(i)).collect();
    let distinct = {
        let mut s: Vec<String> = scales.iter().map(|v| format!("{v:.3}")).collect();
        s.sort();
        s.dedup();
        s.len()
    };
    assert_eq!(
        distinct, 4,
        "four consecutive phrases must not be identical"
    );
}

#[test]
fn rest_placement_leaves_every_phrase_a_breath() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&request().with_density(0.3)).expect("a plan");
    let frame = PhraseFrame::from_analysis(&h.analysis, plan.span);
    for (i, part) in plan.parts.iter().enumerate() {
        let assignment = plan.assignment(part.role).expect("an assignment");
        let p = kb()
            .arrangement_patterns()
            .iter()
            .find(|p| p.id == assignment.pattern_id)
            .expect("the pattern");
        if patterns::is_sustained(p) || part.role == ArrangementRole::Lead {
            continue;
        }
        assert!(
            plan.metrics[i].rest_ratio > 0.0,
            "{} never stopped playing",
            part.role.id()
        );
        assert!(
            frame
                .phrases
                .iter()
                .all(|w| density::rest_ratio(&part.notes, *w) > 0.0),
            "{} filled a whole phrase",
            part.role.id()
        );
    }
}

// ---------------------------------------------------------------------------
// Section level
// ---------------------------------------------------------------------------

#[test]
fn sections_are_implied_when_none_are_given() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&request()).expect("a plan");
    assert!(!plan.sections.is_empty());
    assert_eq!(plan.sections[0].section.start, plan.span.0);
    assert_eq!(
        plan.sections.last().expect("a section").section.end,
        plan.span.1
    );
}

#[test]
fn a_given_section_plan_is_used_verbatim() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&request().with_sections(&two_sections()))
        .expect("a plan");
    assert_eq!(plan.sections.len(), 2);
    assert_eq!(plan.sections[0].section.id, "verse");
    assert_eq!(plan.sections[1].section.id, "chorus");
    assert_eq!(plan.sections[0].section.energy, 0.2);
    assert_eq!(plan.sections[1].section.energy, 0.95);
}

#[test]
fn section_density_follows_section_energy() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&request().with_sections(&two_sections()))
        .expect("a plan");
    let boundary = BeatTime::from_quarters(16);
    let mut busier = 0usize;
    for part in &plan.parts {
        let a = slice(part, plan.span.0, boundary);
        let b = slice(part, boundary, plan.span.1);
        if b.onsets_per_qn > a.onsets_per_qn {
            busier += 1;
        }
    }
    assert!(busier > 0, "the high-energy section is not busier anywhere");
}

#[test]
fn section_register_shift_moves_the_background() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&request().with_sections(&two_sections()))
        .expect("a plan");
    let boundary = BeatTime::from_quarters(16);
    let mut moved = 0usize;
    for part in &plan.parts {
        let a = slice(part, plan.span.0, boundary);
        let b = slice(part, boundary, plan.span.1);
        if a.notes > 0 && b.notes > 0 && (a.mean_midi - b.mean_midi).abs() >= 1.0 {
            moved += 1;
        }
    }
    assert!(moved > 0, "no part changed register between the sections");
}

#[test]
fn section_layer_budget_follows_energy() {
    let plans = sections::plan_sections(
        &two_sections(),
        (BeatTime::ZERO, BeatTime::from_quarters(32)),
        &tm(),
        &[],
        7,
        0.5,
    );
    assert!(plans[1].layer_budget >= plans[0].layer_budget);
    assert!(plans[1].harmonic_complexity > plans[0].harmonic_complexity);
    assert!(plans[1].rhythmic_activity > plans[0].rhythmic_activity);
}

#[test]
fn harmonic_complexity_changes_the_voicing_width() {
    let h = harness("melodies/eight_bar_c_major", "jazz_standard");
    let plan = h
        .arrange(
            &ArrangementParams::default()
                .with_profile("jazz_standard")
                .with_roles(&[ArrangementRole::HarmonicBed])
                .with_sections(&two_sections()),
        )
        .expect("a plan");
    let part = plan.part(ArrangementRole::HarmonicBed).expect("a bed");
    let boundary = BeatTime::from_quarters(16);
    let voices = |from: BeatTime, to: BeatTime| {
        let notes: Vec<Note> = part
            .notes
            .iter()
            .filter(|n| n.onset >= from && n.onset < to)
            .cloned()
            .collect();
        density::max_polyphony(&notes)
    };
    assert!(
        voices(boundary, plan.span.1) >= voices(plan.span.0, boundary),
        "the higher-energy section spelled less of the chord"
    );
}

#[test]
fn every_section_but_the_last_prepares_a_transition() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&request().with_sections(&two_sections()))
        .expect("a plan");
    assert!(plan.sections[0].transition_prep);
    assert!(!plan.sections[1].transition_prep);
}

#[test]
fn adjacent_sections_never_differ_only_in_velocity() {
    for energies in [(0.5, 0.5), (0.2, 0.9), (0.9, 0.2), (0.0, 0.0), (1.0, 1.0)] {
        let sections = vec![
            Section {
                id: "a".to_string(),
                start: BeatTime::ZERO,
                end: BeatTime::from_quarters(16),
                role: "verse".to_string(),
                energy: energies.0,
            },
            Section {
                id: "b".to_string(),
                start: BeatTime::from_quarters(16),
                end: BeatTime::from_quarters(32),
                role: "chorus".to_string(),
                energy: energies.1,
            },
        ];
        let plans = sections::plan_sections(
            &sections,
            (BeatTime::ZERO, BeatTime::from_quarters(32)),
            &tm(),
            &[],
            6,
            0.5,
        );
        let differs = plans[0].register_shift != plans[1].register_shift
            || (plans[0].density - plans[1].density).abs() >= 0.05
            || (plans[0].note_length_scale - plans[1].note_length_scale).abs() >= 0.1
            || plans[0].layer_budget != plans[1].layer_budget;
        assert!(
            differs,
            "sections with energies {energies:?} differ only in velocity"
        );
    }
}

// ---------------------------------------------------------------------------
// Whole-loop level
// ---------------------------------------------------------------------------

#[test]
fn long_term_energy_is_reported() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let rising = [(BeatTime::ZERO, 0.1), (BeatTime::from_quarters(32), 0.9)];
    let falling = [(BeatTime::ZERO, 0.9), (BeatTime::from_quarters(32), 0.1)];
    let up = h
        .arrange(&request().with_energy_curve(&rising))
        .expect("a plan");
    let down = h
        .arrange(&request().with_energy_curve(&falling))
        .expect("a plan");
    assert!(up.loop_plan.long_term_energy > 0.5);
    assert!(down.loop_plan.long_term_energy < -0.5);
}

#[test]
fn the_maximum_density_point_is_where_the_energy_peaks() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let curve = [
        (BeatTime::ZERO, 0.1),
        (BeatTime::from_quarters(20), 1.0),
        (BeatTime::from_quarters(32), 0.1),
    ];
    let plan = h
        .arrange(&request().with_energy_curve(&curve))
        .expect("a plan");
    assert_eq!(plan.loop_plan.max_density_qn, BeatTime::from_quarters(20));
    assert!(plan.loop_plan.max_density_energy > 0.9);
}

#[test]
fn recurring_roles_are_the_ones_that_hold_the_loop() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&request()).expect("a plan");
    assert!(plan
        .loop_plan
        .recurring_roles
        .contains(&ArrangementRole::Bass));
    assert!(plan
        .loop_plan
        .recurring_roles
        .contains(&ArrangementRole::Lead));
    assert!(!plan
        .loop_plan
        .recurring_roles
        .contains(&ArrangementRole::Texture));
}

#[test]
fn a_returning_progression_reads_as_a_tonal_return() {
    let h = arrangement_engine::testing::with_chords(
        harness("melodies/eight_bar_c_major", "pop_rock"),
        chords_from(&["C", "F", "G", "C"], 8),
    );
    let plan = h.arrange(&request()).expect("a plan");
    assert!(plan.loop_plan.tonal_return);
    assert_eq!(plan.loop_plan.ending, Ending::Closed);
}

#[test]
fn a_dominant_ending_reads_as_open() {
    let h = arrangement_engine::testing::with_chords(
        harness("melodies/eight_bar_c_major", "pop_rock"),
        chords_from(&["C", "Am", "F", "G7"], 8),
    );
    let plan = h
        .arrange(&request().with_loop_intent(LoopIntent::OpenDominant))
        .expect("a plan");
    assert_eq!(plan.loop_plan.ending, Ending::Open);
    assert!(!plan.loop_plan.tonal_return);
}

#[test]
fn a_seamless_loop_announces_no_seam() {
    let h = harness("melodies/dorian_vamp_d", "modal_ambient");
    let seamless = h
        .arrange(
            &request()
                .with_profile("modal_ambient")
                .with_loop_intent(LoopIntent::SeamlessColor),
        )
        .expect("a plan");
    assert_eq!(seamless.loop_plan.transition_qn, None);
    let closed = h
        .arrange(
            &request()
                .with_profile("modal_ambient")
                .with_loop_intent(LoopIntent::ClosedTonic),
        )
        .expect("a plan");
    assert!(closed.loop_plan.transition_qn.is_some());
}

#[test]
fn the_energy_curve_is_sampled_per_bar() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&request()).expect("a plan");
    let bar = h.time_map().meter_at(plan.span.0).bar_length_qn();
    let bars = (plan.span.1 - plan.span.0).div_floor(bar);
    assert!(
        plan.energy.len() as i64 >= bars,
        "the curve was sampled {} times over {bars} bars",
        plan.energy.len()
    );
    assert_eq!(plan.energy[0].0, plan.span.0);
    assert_eq!(plan.energy.last().expect("a sample").0, plan.span.1);
}

#[test]
fn the_layer_budget_is_monotone_in_energy() {
    let mut previous = 0usize;
    for step in 0..=10 {
        let b = energy::layer_budget(f64::from(step) / 10.0, 8);
        assert!(b >= previous);
        previous = b;
    }
}

// ---------------------------------------------------------------------------
// The contrast levers
// ---------------------------------------------------------------------------

#[test]
fn register_is_a_lever() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let narrow = h
        .arrange(&request().with_register_spread(0.0))
        .expect("a plan");
    let wide = h
        .arrange(&request().with_register_spread(1.0))
        .expect("a plan");
    let windows = |p: &ArrangementPlan| -> Vec<(i32, i32)> {
        p.assignments.iter().map(|a| a.register).collect()
    };
    assert_ne!(
        windows(&narrow),
        windows(&wide),
        "register_spread did not move a single window"
    );
    let worst = |p: &ArrangementPlan| {
        let w = windows(p);
        let mut max = 0;
        for i in 0..w.len() {
            for j in (i + 1)..w.len() {
                max = max.max(masking::window_overlap(w[i], w[j]));
            }
        }
        max
    };
    assert!(
        worst(&wide) <= worst(&narrow),
        "a wider spread produced more register overlap"
    );
}

#[test]
fn rhythm_is_a_lever() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let base = ArrangementParams::default().with_roles(&[ArrangementRole::Comping]);
    let a = h
        .arrange(&base.clone().with_texture("arr_offbeat_comping"))
        .expect("a plan");
    let b = h
        .arrange(&base.with_texture("arr_alberti"))
        .expect("a plan");
    let onsets = |p: &ArrangementPlan| {
        let mut o: Vec<String> = p
            .part(ArrangementRole::Comping)
            .expect("a part")
            .notes
            .iter()
            .map(|n| n.onset.to_display())
            .collect();
        o.sort();
        o.dedup();
        o
    };
    assert_ne!(onsets(&a), onsets(&b), "the two textures share a rhythm");
}

#[test]
fn density_is_a_lever() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    // The lead is preserved material, so it is deliberately excluded: the
    // density control governs what the engine writes, not what it was given.
    let accompaniment = request().with_roles(&[
        ArrangementRole::Bass,
        ArrangementRole::HarmonicBed,
        ArrangementRole::Comping,
        ArrangementRole::Ostinato,
    ]);
    let quiet = h
        .arrange(&accompaniment.clone().with_density(0.0))
        .expect("a plan");
    let busy = h.arrange(&accompaniment.with_density(1.0)).expect("a plan");
    assert!(
        busy.notes().len() > quiet.notes().len() * 2,
        "{} against {}",
        quiet.notes().len(),
        busy.notes().len()
    );
}

#[test]
fn texture_is_a_lever() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let base = ArrangementParams::default().with_roles(&[ArrangementRole::Texture]);
    let breakdown = h
        .arrange(&base.clone().with_texture("arr_density_breakdown"))
        .expect("a plan");
    let layered = h
        .arrange(&base.with_texture("arr_layered_pad_pluck"))
        .expect("a plan");
    assert_ne!(
        breakdown.notes().len(),
        layered.notes().len(),
        "two textures produced the same amount of material"
    );
}

#[test]
fn timbre_metadata_is_carried() {
    let h = harness("melodies/eight_bar_c_major", "cinematic");
    let plan = h
        .arrange(&request().with_profile("cinematic"))
        .expect("a plan");
    for part in &plan.parts {
        assert!(
            part.instrument_profile.is_some(),
            "{} carries no instrument",
            part.role.id()
        );
        assert!(
            part.notes.iter().all(|n| n.articulation.is_some()),
            "{} carries no articulation",
            part.role.id()
        );
    }
    let instruments: Vec<&str> = plan
        .parts
        .iter()
        .filter_map(|p| p.instrument_profile.as_deref())
        .collect();
    let distinct = {
        let mut v = instruments.clone();
        v.sort_unstable();
        v.dedup();
        v.len()
    };
    assert!(
        distinct > 1,
        "every part was written for the same instrument"
    );
}

#[test]
fn harmony_is_a_lever() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let simple = arrangement_engine::testing::with_chords(h.clone(), chords_from(&["C", "F"], 16));
    let rich = arrangement_engine::testing::with_chords(h, chords_from(&["Cmaj9", "Fmaj7#11"], 16));
    let pcs = |p: &ArrangementPlan| {
        let mut v: Vec<i32> = p.notes().iter().map(|n| n.midi.rem_euclid(12)).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let accompaniment = request().with_roles(&[
        ArrangementRole::HarmonicBed,
        ArrangementRole::Pad,
        ArrangementRole::Comping,
    ]);
    let a = simple.arrange(&accompaniment).expect("a plan");
    let b = rich.arrange(&accompaniment).expect("a plan");
    assert!(
        pcs(&b).len() > pcs(&a).len(),
        "richer harmony produced no more pitch classes: {:?} vs {:?}",
        pcs(&a),
        pcs(&b)
    );
}

#[test]
fn dynamics_are_a_lever_but_not_the_only_one() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&request().with_sections(&two_sections()))
        .expect("a plan");
    let boundary = BeatTime::from_quarters(16);
    let mut velocity_moved = false;
    let mut something_else_moved = false;
    for part in &plan.parts {
        let a = slice(part, plan.span.0, boundary);
        let b = slice(part, boundary, plan.span.1);
        if a.notes == 0 || b.notes == 0 {
            continue;
        }
        let c = density::contrast(&a, &b);
        if c.velocity.abs() >= 1.0 {
            velocity_moved = true;
        }
        if c.non_dynamic_dimensions() > 0 {
            something_else_moved = true;
        }
    }
    assert!(velocity_moved, "dynamics did not move at all");
    assert!(
        something_else_moved,
        "dynamics were the only thing that moved"
    );
}

#[test]
fn articulation_is_a_lever() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&request()).expect("a plan");
    let mut labels: Vec<String> = plan
        .parts
        .iter()
        .flat_map(|p| p.notes.iter())
        .filter_map(|n| n.articulation.clone())
        .collect();
    labels.sort();
    labels.dedup();
    assert!(
        labels.len() > 1,
        "every note carries the same articulation: {labels:?}"
    );
}

#[test]
fn silence_is_a_lever() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let flat = h
        .arrange(
            &request()
                .with_energy_curve(&[(BeatTime::ZERO, 0.6), (BeatTime::from_quarters(32), 0.6)]),
        )
        .expect("a plan");
    let dipping = h
        .arrange(&request().with_energy_curve(&[
            (BeatTime::ZERO, 0.9),
            (BeatTime::from_quarters(16), 0.0),
            (BeatTime::from_quarters(32), 0.9),
        ]))
        .expect("a plan");
    assert!(flat.loop_plan.silence.is_empty());
    assert!(!dipping.loop_plan.silence.is_empty());
    assert!(
        dipping.notes().len() < flat.notes().len(),
        "the planned silence removed nothing"
    );
}

#[test]
fn role_substitution_is_a_lever() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let a = h
        .arrange(&ArrangementParams::default().with_roles(&[ArrangementRole::Pulse]))
        .expect("a plan");
    let b = h
        .arrange(&ArrangementParams::default().with_roles(&[ArrangementRole::Percussion]))
        .expect("a plan");
    let pattern_of = |p: &ArrangementPlan, r: ArrangementRole| {
        p.assignment(r).expect("an assignment").pattern_id.clone()
    };
    assert!(!pattern_of(&a, ArrangementRole::Pulse).is_empty());
    assert!(!pattern_of(&b, ArrangementRole::Percussion).is_empty());
    assert!(!a.notes().is_empty(), "a substituted role produced nothing");
    assert!(!b.notes().is_empty(), "a substituted role produced nothing");
}

// ---------------------------------------------------------------------------
// Masking measurement
// ---------------------------------------------------------------------------

#[test]
fn masking_is_reported_before_and_after_separation() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&request().with_register_spread(0.9))
        .expect("a plan");
    assert!(plan.masking_before.count() > 0, "nothing to improve");
    assert!(plan.masking_after.count() <= plan.masking_before.count());
    assert!(!plan.masking_before.suggestions.is_empty());
    let json = plan.masking_after.to_json();
    assert!(json.get("collision_count").is_some());
}

#[test]
fn the_masking_report_names_real_levers() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&request().with_register_spread(0.0))
        .expect("a plan");
    let text = plan.masking_before.suggestions.join(" | ");
    let mentioned = [
        "register separation",
        "onset-density control",
        "note-length control",
        "role priority",
        "reduced doubling",
        "voice allocation",
        "contrary rhythmic activity",
    ]
    .iter()
    .filter(|lever| text.contains(**lever))
    .count();
    assert!(
        mentioned >= 4,
        "the report named only {mentioned} of the brief's levers: {text}"
    );
}

#[test]
fn collisions_are_located_in_time_and_pitch() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&request().with_register_spread(0.0))
        .expect("a plan");
    for (a, b, qn, semitones) in &plan.masking_before.collisions {
        assert!(*a < plan.parts.len() && *b < plan.parts.len());
        assert!(a < b, "collisions are recorded once, in index order");
        assert!(*qn >= plan.span.0 && *qn < plan.span.1);
        assert!(*semitones >= 0 && *semitones <= masking::MASKING_WINDOW_SEMITONES);
    }
}

#[test]
fn a_plan_records_why_it_withheld_a_role() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(
            &request()
                .with_energy_curve(&[(BeatTime::ZERO, 0.0), (BeatTime::from_quarters(32), 0.0)]),
        )
        .expect("a plan");
    assert!(plan.assignments.len() > plan.parts.len());
    assert!(plan.warnings.iter().any(|w| w.code == "ROLE_WITHHELD"));
    for a in &plan.assignments {
        if plan.part(a.role).is_none() {
            assert!(
                a.rationale.contains("withheld"),
                "{} was withheld without saying so",
                a.role.id()
            );
        }
    }
}

#[test]
fn the_plan_cites_real_rules() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&request()).expect("a plan");
    assert!(!plan.rule_applications.is_empty());
    for app in &plan.rule_applications {
        let rule = kb()
            .rule(&app.rule_id)
            .unwrap_or_else(|| panic!("{} is not a rule in the bundle", app.rule_id));
        assert!(!app.explanation.is_empty());
        for source in &app.source_ids {
            assert!(
                kb().source(source).is_some() || rule.kind.id() == "implementation_heuristic",
                "{} cites unknown source {source}",
                app.rule_id
            );
        }
    }
    let arrangement_rules = plan
        .rule_applications
        .iter()
        .filter(|a| a.rule_id.starts_with("arrangement."))
        .count();
    assert!(
        arrangement_rules >= 5,
        "only {arrangement_rules} arrangement rules were evaluated"
    );
}
