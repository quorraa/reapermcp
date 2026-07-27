//! Every pattern in the catalogue, every role in the vocabulary.
//!
//! The brief requires that each of the twenty-eight catalogued patterns realise
//! without violating its instrument profile, and that all sixteen arrangement
//! roles be reachable. These tests walk the data rather than a hand-written
//! list, so a pattern added to `knowledge/` is covered the moment it lands.

use arrangement_engine::density;
use arrangement_engine::patterns::{self, realize_pattern};
use arrangement_engine::roles;
use arrangement_engine::testing::chords_from;
use arrangement_engine::ArrangementParams;
use music_domain::prelude::*;
use theory_kb::{ArrangementPattern, KnowledgeBase};

fn kb() -> &'static KnowledgeBase {
    KnowledgeBase::embedded()
}

fn tm() -> TimeMap {
    TimeMap::constant(120.0, TimeSignature::new(4, 4))
}

fn chords() -> Vec<ChordEvent> {
    chords_from(&["Cmaj7", "Am7", "Fmaj7", "G7"], 4)
}

fn span() -> (BeatTime, BeatTime) {
    (BeatTime::ZERO, BeatTime::from_quarters(16))
}

/// Realises a pattern on the instrument the engine would choose for it.
fn realise(p: &ArrangementPattern, density: f64, seed: u64) -> Vec<Note> {
    let role = ArrangementRole::parse(&p.role).expect("a known role");
    let inst = roles::select_instrument(kb(), p, role).expect("an instrument");
    realize_pattern(kb(), p, &chords(), &tm(), inst, density, seed).expect("a realisation")
}

#[test]
fn the_catalogue_has_the_patterns_the_brief_lists() {
    assert!(
        kb().arrangement_patterns().len() >= 24,
        "the brief requires at least 24 arrangement patterns"
    );
    assert!(
        kb().instrument_profiles().len() >= 12,
        "the brief requires at least 12 instrument profiles"
    );
}

#[test]
fn every_pattern_declares_every_field_the_brief_requires() {
    for p in kb().arrangement_patterns() {
        assert!(!p.role.is_empty(), "{} has no role", p.id);
        assert!(!p.register.is_empty(), "{} has no register", p.id);
        assert!(p.range.low_midi < p.range.high_midi, "{} has no range", p.id);
        assert!((0.0..=1.0).contains(&p.density), "{} density", p.id);
        assert!(!p.rhythmic_activity.is_empty(), "{} activity", p.id);
        assert!(!p.harmonic_responsibility.is_empty(), "{} harmony", p.id);
        assert!(!p.priority.is_empty(), "{} priority", p.id);
        assert!(p.polyphony >= 1, "{} polyphony", p.id);
        assert!(!p.articulation_tendency.is_empty(), "{} articulation", p.id);
        assert!(!p.note_length_tendency.is_empty(), "{} note length", p.id);
        assert!(!p.section_participation.is_empty(), "{} sections", p.id);
        assert!((0.0..=1.0).contains(&p.energy_contribution), "{} energy", p.id);
        assert!(!p.loop_behavior.is_empty(), "{} loop behaviour", p.id);
        assert!(!p.rhythm.onsets.is_empty(), "{} onsets", p.id);
        assert!(p.rhythm.grid_qn.is_positive(), "{} grid", p.id);
        assert!(p.rhythm.bar_length_qn.is_positive(), "{} bar length", p.id);
    }
}

#[test]
fn every_pattern_realises_inside_its_instrument_profile() {
    for p in kb().arrangement_patterns() {
        let role = ArrangementRole::parse(&p.role).expect("a known role");
        let inst = roles::select_instrument(kb(), p, role).expect("an instrument");
        let notes = realise(p, 0.7, 21);
        assert!(!notes.is_empty(), "{} realised nothing", p.id);
        for n in &notes {
            assert!(
                inst.range.contains(n.midi),
                "{} wrote {} outside {}'s {}..{}",
                p.id,
                n.midi,
                inst.id,
                inst.range.low_midi,
                inst.range.high_midi
            );
            assert!((0..=127).contains(&n.midi), "{} wrote MIDI {}", p.id, n.midi);
            assert!((1..=127).contains(&n.velocity), "{} velocity", p.id);
            assert!(n.duration.is_positive(), "{} duration", p.id);
            assert_eq!(n.pitch.midi(), n.midi, "{} spelling", p.id);
        }
    }
}

#[test]
fn every_pattern_honours_its_polyphony_and_that_of_its_instrument() {
    for p in kb().arrangement_patterns() {
        let role = ArrangementRole::parse(&p.role).expect("a known role");
        let inst = roles::select_instrument(kb(), p, role).expect("an instrument");
        let notes = realise(p, 1.0, 4);
        let measured = density::max_polyphony(&notes);
        assert!(
            measured <= p.polyphony.max(1) as usize,
            "{} sounded {} voices, declaring {}",
            p.id,
            measured,
            p.polyphony
        );
        assert!(
            measured <= inst.polyphony.max(1) as usize,
            "{} exceeded {}'s polyphony",
            p.id,
            inst.id
        );
    }
}

#[test]
fn every_pattern_respects_its_instruments_low_interval_limits() {
    for p in kb().arrangement_patterns() {
        let role = ArrangementRole::parse(&p.role).expect("a known role");
        let inst = roles::select_instrument(kb(), p, role).expect("an instrument");
        let notes = realise(p, 1.0, 8);
        let mut onsets: Vec<BeatTime> = notes.iter().map(|n| n.onset).collect();
        onsets.sort();
        onsets.dedup();
        for onset in onsets {
            let mut stack: Vec<i32> = notes
                .iter()
                .filter(|n| n.onset == onset)
                .map(|n| n.midi)
                .collect();
            stack.sort_unstable();
            stack.dedup();
            for w in stack.windows(2) {
                let limit = inst.min_spacing_at(w[0]).unwrap_or(1);
                assert!(
                    w[1] - w[0] >= limit,
                    "{} on {} stacked {} and {}, closer than the profile's {} semitones",
                    p.id,
                    inst.id,
                    w[0],
                    w[1],
                    limit
                );
            }
        }
    }
}

#[test]
fn every_pattern_stays_inside_the_span_it_was_given() {
    for p in kb().arrangement_patterns() {
        let notes = realise(p, 1.0, 6);
        for n in &notes {
            assert!(n.onset >= span().0, "{} started before the span", p.id);
            assert!(n.onset < span().1, "{} started after the span", p.id);
        }
    }
}

#[test]
fn every_pattern_sounds_only_the_harmony_it_is_responsible_for() {
    let chords = chords();
    for p in kb().arrangement_patterns() {
        let notes = realise(p, 1.0, 12);
        for n in &notes {
            let chord = patterns::chord_at(&chords, n.onset);
            let pcs = chord.spec.pitch_classes();
            assert!(
                pcs.contains(&n.midi.rem_euclid(12)),
                "{} sounded a pitch outside {} at {}",
                p.id,
                chord.spec.render_ascii(),
                n.onset
            );
        }
    }
}

#[test]
fn root_only_patterns_sound_only_roots() {
    let chords = chords();
    for p in kb()
        .arrangement_patterns()
        .iter()
        .filter(|p| p.harmonic_responsibility == "root_only")
    {
        let notes = realise(p, 1.0, 2);
        for n in &notes {
            let chord = patterns::chord_at(&chords, n.onset);
            assert_eq!(
                n.midi.rem_euclid(12),
                chord.spec.root_pc(),
                "{} sounded a non-root",
                p.id
            );
        }
    }
}

#[test]
fn guide_tone_patterns_sound_only_guide_tones() {
    let chords = chords();
    for p in kb()
        .arrangement_patterns()
        .iter()
        .filter(|p| p.harmonic_responsibility == "guide_tones")
    {
        let notes = realise(p, 1.0, 2);
        for n in &notes {
            let chord = patterns::chord_at(&chords, n.onset);
            let wanted: Vec<i32> = chord
                .spec
                .guide_tones()
                .iter()
                .map(|d| (chord.spec.root_pc() + d.simple_semitones()).rem_euclid(12))
                .collect();
            if wanted.is_empty() {
                continue;
            }
            assert!(
                wanted.contains(&n.midi.rem_euclid(12)),
                "{} sounded {} which is not a guide tone of {}",
                p.id,
                n.midi.rem_euclid(12),
                chord.spec.render_ascii()
            );
        }
    }
}

#[test]
fn every_pattern_is_deterministic_under_the_same_seed() {
    for p in kb().arrangement_patterns() {
        let a = realise(p, 0.63, 99);
        let b = realise(p, 0.63, 99);
        assert_eq!(a.len(), b.len(), "{} changed length", p.id);
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(
                (x.id, x.midi, x.onset, x.duration, x.velocity, x.channel),
                (y.id, y.midi, y.onset, y.duration, y.velocity, y.channel),
                "{} is not deterministic",
                p.id
            );
        }
    }
}

#[test]
fn every_pattern_thins_when_asked_and_never_disappears() {
    for p in kb().arrangement_patterns() {
        let sparse = realise(p, 0.0, 3);
        let dense = realise(p, 1.0, 3);
        assert!(!sparse.is_empty(), "{} vanished at density 0", p.id);
        assert!(
            sparse.len() <= dense.len(),
            "{} wrote more notes at density 0 than at density 1",
            p.id
        );
    }
}

#[test]
fn sustained_patterns_are_measurably_longer_than_the_rest() {
    let mut sustained = Vec::new();
    let mut articulated = Vec::new();
    for p in kb().arrangement_patterns() {
        let notes = realise(p, 0.6, 5);
        let m = density::measure(&notes, span());
        if patterns::is_sustained(p) {
            sustained.push(m.mean_duration);
        } else {
            articulated.push(m.mean_duration);
        }
    }
    assert!(!sustained.is_empty() && !articulated.is_empty());
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    assert!(
        mean(&sustained) > mean(&articulated) * 1.5,
        "sustained patterns average {:.2} QN against {:.2}",
        mean(&sustained),
        mean(&articulated)
    );
}

#[test]
fn every_arrangement_role_has_a_pattern_pool() {
    for role in ArrangementRole::all() {
        let pool = roles::pattern_pool(kb(), *role);
        assert!(!pool.is_empty(), "{} has no pattern", role.id());
        for p in pool {
            assert!(
                p.role == role.id() || roles::substitutes(*role).contains(&p.role.as_str()),
                "{} drew {} from an unrelated role",
                role.id(),
                p.id
            );
        }
    }
}

#[test]
fn every_arrangement_role_realises_end_to_end() {
    let h = arrangement_engine::testing::harness("melodies/eight_bar_c_major", "cinematic");
    for role in ArrangementRole::all() {
        let plan = h
            .arrange(&ArrangementParams::default().with_roles(&[*role]))
            .unwrap_or_else(|e| panic!("{} failed: {e}", role.id()));
        assert_eq!(plan.assignments.len(), 1);
        let part = plan
            .part(*role)
            .unwrap_or_else(|| panic!("{} produced no part", role.id()));
        assert!(!part.notes.is_empty(), "{} produced no notes", role.id());
        let profile = kb()
            .instrument_profile(part.instrument_profile.as_deref().expect("an instrument"))
            .expect("a profile");
        for n in &part.notes {
            assert!(
                profile.range.contains(n.midi),
                "{} escaped {}'s range",
                role.id(),
                profile.id
            );
        }
    }
}

#[test]
fn substituted_roles_say_so() {
    let h = arrangement_engine::testing::harness("melodies/eight_bar_c_major", "electronic_loop");
    for role in [
        ArrangementRole::Pulse,
        ArrangementRole::Percussion,
        ArrangementRole::Ornament,
        ArrangementRole::EarCandy,
    ] {
        let plan = h
            .arrange(&ArrangementParams::default().with_roles(&[role]))
            .expect("a plan");
        let assignment = plan.assignment(role).expect("an assignment");
        assert!(
            assignment.rationale.contains("substituted"),
            "{} borrowed a pattern without saying so",
            role.id()
        );
        assert!(plan
            .warnings
            .iter()
            .any(|w| w.code == "ROLE_SUBSTITUTED" && w.message.contains(role.id())));
    }
}

#[test]
fn catalogued_roles_are_not_substituted() {
    let h = arrangement_engine::testing::harness("melodies/eight_bar_c_major", "jazz_standard");
    for role in ArrangementRole::all() {
        if !roles::patterns_for_role(kb(), *role).is_empty() {
            let plan = h
                .arrange(&ArrangementParams::default().with_roles(&[*role]))
                .expect("a plan");
            let assignment = plan.assignment(*role).expect("an assignment");
            let chosen = kb()
                .arrangement_patterns()
                .iter()
                .find(|p| p.id == assignment.pattern_id)
                .expect("the chosen pattern");
            assert_eq!(chosen.role, role.id(), "{} was substituted", role.id());
        }
    }
}

#[test]
fn a_forced_texture_is_honoured_end_to_end() {
    let h = arrangement_engine::testing::harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(
            &ArrangementParams::default()
                .with_roles(&[ArrangementRole::HarmonicBed])
                .with_texture("arr_chorale_voicing"),
        )
        .expect("a plan");
    assert_eq!(
        plan.assignment(ArrangementRole::HarmonicBed)
            .expect("an assignment")
            .pattern_id,
        "arr_chorale_voicing"
    );
}

#[test]
fn the_texture_lever_changes_the_output() {
    let h = arrangement_engine::testing::harness("melodies/eight_bar_c_major", "pop_rock");
    let base = ArrangementParams::default().with_roles(&[ArrangementRole::HarmonicBed]);
    let chorale = h
        .arrange(&base.clone().with_texture("arr_chorale_voicing"))
        .expect("a plan");
    let blocks = h
        .arrange(&base.with_texture("arr_block_chords"))
        .expect("a plan");
    let onsets = |p: &arrangement_engine::ArrangementPlan| {
        p.metrics_for(ArrangementRole::HarmonicBed)
            .expect("metrics")
            .onsets
    };
    assert!(
        onsets(&blocks) > onsets(&chorale),
        "the texture lever changed nothing: {} vs {}",
        onsets(&chorale),
        onsets(&blocks)
    );
}

#[test]
fn every_instrument_profile_is_usable() {
    let chords = chords();
    for inst in kb().instrument_profiles() {
        assert!(inst.range.low_midi < inst.range.high_midi, "{}", inst.id);
        assert!(inst.polyphony >= 1, "{}", inst.id);
        // A general-purpose harmony pattern must be playable on anything.
        let p = kb()
            .arrangement_patterns()
            .iter()
            .find(|p| p.id == "arr_block_chords")
            .expect("a pattern");
        let notes =
            realize_pattern(kb(), p, &chords, &tm(), inst, 0.8, 1).expect("a realisation");
        assert!(!notes.is_empty(), "{} played nothing", inst.id);
        for n in &notes {
            assert!(inst.range.contains(n.midi), "{} escaped its range", inst.id);
        }
    }
}

#[test]
fn instrument_profiles_declare_their_own_spacing() {
    let mut seen: Vec<Vec<(i32, i32)>> = Vec::new();
    for inst in kb().instrument_profiles() {
        let table: Vec<(i32, i32)> = inst
            .low_interval_limits
            .iter()
            .map(|l| (l.below_midi, l.min_semitones))
            .collect();
        seen.push(table);
    }
    let distinct: usize = {
        let mut s = seen.clone();
        s.sort();
        s.dedup();
        s.len()
    };
    assert!(
        distinct > 1,
        "every instrument declares the same spacing table, so the limit is universal after all"
    );
}
