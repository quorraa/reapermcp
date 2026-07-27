//! The behaviours the brief's §28 list and `knowledge/rules/arrangement.json`
//! name by id.
//!
//! Every function here whose name matches a `test_id` in the knowledge base is
//! the behavioural test that discharges it. Nothing is asserted that is not
//! measured: "sparse profiles contain rests" means a measured rest ratio above
//! zero, "register separation reduces collisions" means a smaller measured
//! collision count.

use arrangement_engine::density::{self, PartMetrics};
use arrangement_engine::masking::masking_report;
use arrangement_engine::patterns::{self, realize_pattern};
use arrangement_engine::plan::{enforce, essential_layers, guide_tones_survive_without};
use arrangement_engine::roles;
use arrangement_engine::testing::{chords_from, harness, with_chords};
use arrangement_engine::ArrangementParams;
use music_domain::prelude::*;
use theory_kb::{ArrangementPattern, InstrumentProfile, KnowledgeBase};

fn kb() -> &'static KnowledgeBase {
    KnowledgeBase::embedded()
}

fn pattern(id: &str) -> &'static ArrangementPattern {
    kb()
        .arrangement_patterns()
        .iter()
        .find(|p| p.id == id)
        .unwrap_or_else(|| panic!("{id} is not in the catalogue"))
}

fn instrument(id: &str) -> &'static InstrumentProfile {
    kb()
        .instrument_profile(id)
        .unwrap_or_else(|| panic!("{id} is not an instrument profile"))
}

fn four_four() -> TimeMap {
    TimeMap::constant(120.0, TimeSignature::new(4, 4))
}

fn span_of(chords: &[ChordEvent]) -> (BeatTime, BeatTime) {
    (
        chords[0].onset,
        chords
            .iter()
            .map(ChordEvent::end)
            .fold(chords[0].onset, BeatTime::max),
    )
}

fn measure_pattern(id: &str, inst: &str, density: f64) -> (Vec<Note>, PartMetrics) {
    let chords = chords_from(&["C", "Am", "F", "G7"], 4);
    let notes = realize_pattern(
        kb(),
        pattern(id),
        &chords,
        &four_four(),
        instrument(inst),
        density,
        13,
    )
    .expect("a realisation");
    let metrics = density::measure(&notes, span_of(&chords));
    (notes, metrics)
}

fn full_request() -> ArrangementParams {
    ArrangementParams::default().with_roles(&[
        ArrangementRole::Lead,
        ArrangementRole::Bass,
        ArrangementRole::HarmonicBed,
        ArrangementRole::Pad,
        ArrangementRole::Comping,
        ArrangementRole::Counterlead,
    ])
}

// ---------------------------------------------------------------------------
// §28: bass remains within configured range
// ---------------------------------------------------------------------------

#[test]
fn bass_octave_folded_when_too_low() {
    let bass = instrument("bass");
    // A bass note two octaves under the profile's floor, and one above its
    // ceiling: both must come back inside, and both must keep their spelling.
    let mut notes: Vec<Note> = [4, 96]
        .iter()
        .enumerate()
        .map(|(i, midi)| {
            let mut n = Note::new(
                i as NoteId,
                SpelledPitch::from_midi(*midi, None),
                BeatTime::from_quarters(i as i64),
                BeatTime::from_quarters(1),
            );
            n.midi = *midi;
            n
        })
        .collect();
    let before: Vec<i32> = notes.iter().map(|n| n.midi.rem_euclid(12)).collect();
    enforce(
        &mut notes,
        bass,
        (BeatTime::ZERO, BeatTime::from_quarters(4)),
        1,
    );
    assert_eq!(notes.len(), 2);
    for (i, n) in notes.iter().enumerate() {
        assert!(
            bass.range.contains(n.midi),
            "{} is outside {}..{}",
            n.midi,
            bass.range.low_midi,
            bass.range.high_midi
        );
        assert_eq!(
            n.midi.rem_euclid(12),
            before[i],
            "folding changed the pitch class"
        );
        assert_eq!(n.pitch.midi(), n.midi, "the spelling drifted from the pitch");
    }
}

#[test]
fn bass_parts_stay_inside_their_configured_range() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&ArrangementParams::default().with_roles(&[ArrangementRole::Bass]))
        .expect("a plan");
    let part = plan.part(ArrangementRole::Bass).expect("a bass part");
    let profile = instrument(part.instrument_profile.as_deref().expect("an instrument"));
    assert!(!part.notes.is_empty());
    for n in &part.notes {
        assert!(profile.range.contains(n.midi), "{} escaped", n.midi);
    }
}

// ---------------------------------------------------------------------------
// §28: countermelody does not duplicate every melody onset
// ---------------------------------------------------------------------------

#[test]
fn countermelody_does_not_duplicate_every_melody_onset() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(
            &ArrangementParams::default()
                .with_roles(&[ArrangementRole::Lead, ArrangementRole::Counterlead]),
        )
        .expect("a plan");
    let lead: Vec<BeatTime> = plan
        .part(ArrangementRole::Lead)
        .expect("a lead")
        .notes
        .iter()
        .map(|n| n.onset)
        .collect();
    let counter = plan.part(ArrangementRole::Counterlead).expect("a counter");
    assert!(!counter.notes.is_empty());
    let shared = counter
        .notes
        .iter()
        .filter(|n| lead.contains(&n.onset))
        .count();
    assert!(
        shared < counter.notes.len(),
        "the counterlead shadowed every lead onset"
    );
}

#[test]
fn background_avoids_lead_onsets() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&full_request().with_roles(&[
            ArrangementRole::Lead,
            ArrangementRole::Comping,
            ArrangementRole::Ostinato,
            ArrangementRole::Texture,
            ArrangementRole::Pad,
        ]))
        .expect("a plan");
    let lead: Vec<BeatTime> = plan
        .part(ArrangementRole::Lead)
        .expect("a lead")
        .notes
        .iter()
        .map(|n| n.onset)
        .collect();
    let mut checked = 0usize;
    for part in &plan.parts {
        let assignment = plan.assignment(part.role).expect("an assignment");
        let p = pattern(&assignment.pattern_id);
        // The rule's own exceptions: a sustained texture and a part that
        // declares doubling are both allowed to sound with the lead.
        if assignment.priority == 0
            || part.notes.is_empty()
            || patterns::is_sustained(p)
            || patterns::allows_doubling(p)
        {
            continue;
        }
        checked += 1;
        let shared = part.notes.iter().filter(|n| lead.contains(&n.onset)).count();
        let ratio = shared as f64 / part.notes.len() as f64;
        assert!(
            ratio <= 0.5,
            "{} lands on {:.0}% of the lead's onsets",
            part.role.id(),
            ratio * 100.0
        );
    }
    assert!(checked > 0, "the request produced no part the rule applies to");
}

#[test]
fn offbeat_comping_reduces_collision() {
    let chords = chords_from(&["C", "Am", "F", "G7"], 4);
    let tm = four_four();
    let on_grid = realize_pattern(
        kb(),
        pattern("arr_block_chords"),
        &chords,
        &tm,
        instrument("piano_keys"),
        1.0,
        3,
    )
    .expect("a realisation");
    let offbeat = realize_pattern(
        kb(),
        pattern("arr_offbeat_comping"),
        &chords,
        &tm,
        instrument("guitar_comping"),
        1.0,
        3,
    )
    .expect("a realisation");
    // A lead that articulates on every beat, which is what a background part
    // has to keep out of the way of.
    let lead: Vec<BeatTime> = (0..16).map(BeatTime::from_quarters).collect();
    let hits = |notes: &[Note]| notes.iter().filter(|n| lead.contains(&n.onset)).count();
    assert!(hits(&on_grid) > 0, "the on-grid pattern should collide");
    assert_eq!(
        hits(&offbeat),
        0,
        "the offbeat pattern must never land on the beat"
    );
}

// ---------------------------------------------------------------------------
// §28: sparse profiles contain rests
// ---------------------------------------------------------------------------

#[test]
fn sparse_profile_contains_rests() {
    let (notes, metrics) = measure_pattern("arr_density_breakdown", "pluck", 0.05);
    assert!(!notes.is_empty(), "a sparse part is still a part");
    assert!(
        metrics.rest_ratio > 0.0,
        "a sparse realisation sounded continuously"
    );
    let (_, dense) = measure_pattern("arr_density_breakdown", "pluck", 1.0);
    assert!(
        metrics.rest_ratio >= dense.rest_ratio,
        "sparser must not mean busier"
    );
}

#[test]
fn every_non_sustained_pattern_breathes() {
    let chords = chords_from(&["C", "Am", "F", "G7"], 4);
    let span = span_of(&chords);
    for p in kb().arrangement_patterns() {
        if patterns::is_sustained(p) {
            continue;
        }
        let role = ArrangementRole::parse(&p.role).expect("a known role");
        let inst = roles::select_instrument(kb(), p, role).expect("an instrument");
        let notes =
            realize_pattern(kb(), p, &chords, &four_four(), inst, 0.2, 5).expect("a realisation");
        let metrics = density::measure(&notes, span);
        assert!(
            metrics.rest_ratio > 0.0,
            "{} sounds continuously at low density",
            p.id
        );
    }
}

#[test]
fn strategic_silence_present() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let dip = [
        (BeatTime::ZERO, 0.9),
        (BeatTime::from_quarters(16), 0.0),
        (BeatTime::from_quarters(32), 0.9),
    ];
    let plan = h
        .arrange(&full_request().with_energy_curve(&dip))
        .expect("a plan");
    assert!(
        !plan.loop_plan.silence.is_empty(),
        "a dipping curve must produce a planned silence"
    );
    let (start, end) = plan.loop_plan.silence[0];
    assert!(start > plan.span.0, "silence must not open the loop");
    assert!(end <= plan.span.1);
    let background: Vec<&Part> = plan
        .parts
        .iter()
        .filter(|p| plan.assignment(p.role).map(|a| a.priority) == Some(2))
        .collect();
    assert!(!background.is_empty(), "the request included background roles");
    for part in background {
        assert!(
            part.notes.iter().all(|n| n.onset < start || n.onset >= end),
            "{} played through the planned silence",
            part.role.id()
        );
    }
}

// ---------------------------------------------------------------------------
// §28: pad profile produces sustained behaviour
// ---------------------------------------------------------------------------

#[test]
fn pad_produces_sustained_behavior() {
    let (notes, metrics) = measure_pattern("arr_sustained_pad", "pad", 0.5);
    assert!(!notes.is_empty());
    assert!(
        metrics.mean_duration >= 2.0,
        "pad notes average only {:.2} QN",
        metrics.mean_duration
    );
    assert!(
        metrics.rest_ratio < 0.2,
        "a pad should cover its span, rest ratio was {:.2}",
        metrics.rest_ratio
    );
    let (_, plucked) = measure_pattern("arr_arpeggio", "pluck", 0.5);
    assert!(
        metrics.mean_duration > plucked.mean_duration * 3.0,
        "the pad is not measurably more sustained than an arpeggio"
    );
}

#[test]
fn pad_ties_common_tones() {
    // C and Am share C and E; a pad must hold them rather than restrike them.
    let chords = chords_from(&["C", "Am"], 4);
    let notes = realize_pattern(
        kb(),
        pattern("arr_sustained_pad"),
        &chords,
        &four_four(),
        instrument("pad"),
        1.0,
        2,
    )
    .expect("a realisation");
    let boundary = BeatTime::from_quarters(4);
    let held: Vec<&Note> = notes
        .iter()
        .filter(|n| n.onset < boundary && n.end() > boundary)
        .collect();
    assert!(
        !held.is_empty(),
        "no pad voice was tied across the harmony change"
    );
    let common = [0, 4]; // C and E
    assert!(
        held.iter().all(|n| common.contains(&n.midi.rem_euclid(12))),
        "only a common tone may be tied across the change"
    );
}

// ---------------------------------------------------------------------------
// §28: rhythmic-comping pattern follows its grid
// ---------------------------------------------------------------------------

#[test]
fn rhythmic_comping_follows_its_grid() {
    let tm = four_four();
    for id in [
        "arr_offbeat_comping",
        "arr_afterbeat_comping",
        "arr_block_chords",
        "arr_broken_chords",
        "arr_alberti",
    ] {
        let p = pattern(id);
        let (notes, _) = measure_pattern(id, "piano_keys", 1.0);
        assert!(!notes.is_empty(), "{id} wrote nothing");
        for n in &notes {
            let in_bar = tm.position_in_bar(n.onset);
            assert!(
                p.rhythm.onsets.contains(&in_bar),
                "{id} placed a note at {in_bar} within the bar, off its declared grid"
            );
        }
    }
}

#[test]
fn every_catalogued_grid_is_respected() {
    let tm = four_four();
    let chords = chords_from(&["C", "Am", "F", "G7"], 4);
    for p in kb().arrangement_patterns() {
        if p.rhythm.sustain == "full_slot" {
            // These deliberately add an onset at each harmonic slot boundary.
            continue;
        }
        let role = ArrangementRole::parse(&p.role).expect("a known role");
        let inst = roles::select_instrument(kb(), p, role).expect("an instrument");
        let notes =
            realize_pattern(kb(), p, &chords, &tm, inst, 1.0, 1).expect("a realisation");
        for n in &notes {
            let in_bar = tm.position_in_bar(n.onset);
            assert!(
                p.rhythm.onsets.contains(&in_bar),
                "{} placed a note at {in_bar}, off its declared grid",
                p.id
            );
        }
    }
}

// ---------------------------------------------------------------------------
// §28: density settings change actual note density
// ---------------------------------------------------------------------------

#[test]
fn density_setting_changes_note_density() {
    let (_, sparse) = measure_pattern("arr_block_chords", "piano_keys", 0.05);
    let (_, dense) = measure_pattern("arr_block_chords", "piano_keys", 1.0);
    assert!(
        dense.onsets > sparse.onsets,
        "density did not change the onset count: {} vs {}",
        sparse.onsets,
        dense.onsets
    );
    assert!(dense.onsets_per_qn > sparse.onsets_per_qn);

    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let quiet = h
        .arrange(&full_request().with_density(0.05))
        .expect("a plan");
    let busy = h.arrange(&full_request().with_density(1.0)).expect("a plan");
    let count = |p: &arrangement_engine::ArrangementPlan| p.notes().len();
    assert!(
        count(&busy) > count(&quiet),
        "the plan-level density control did nothing: {} vs {}",
        count(&quiet),
        count(&busy)
    );
}

#[test]
fn density_is_monotone_across_the_whole_range() {
    let mut previous = 0usize;
    for step in 0..=10 {
        let d = f64::from(step) / 10.0;
        let (_, m) = measure_pattern("arr_broken_chords", "piano_keys", d);
        assert!(
            m.onsets >= previous,
            "density {d} produced fewer onsets than the step below it"
        );
        previous = m.onsets;
    }
}

// ---------------------------------------------------------------------------
// §28: register separation reduces collisions
// ---------------------------------------------------------------------------

#[test]
fn register_separation_reduces_collisions() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&full_request().with_register_spread(1.0))
        .expect("a plan");
    assert!(
        plan.masking_after.count() < plan.masking_before.count(),
        "separation did not reduce collisions: {} -> {}",
        plan.masking_before.count(),
        plan.masking_after.count()
    );
    assert!(plan.masking_after.register_overlap <= plan.masking_before.register_overlap);
}

#[test]
fn wider_spread_separates_further() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let narrow = h
        .arrange(&full_request().with_register_spread(0.0))
        .expect("a plan");
    let wide = h
        .arrange(&full_request().with_register_spread(1.0))
        .expect("a plan");
    assert_eq!(
        narrow.masking_before.count(),
        narrow.masking_after.count(),
        "zero spread must leave the catalogue windows alone"
    );
    assert!(
        wide.masking_after.count() < narrow.masking_after.count(),
        "a wider spread did not separate further: {} vs {}",
        narrow.masking_after.count(),
        wide.masking_after.count()
    );
}

#[test]
fn masking_is_measured_not_asserted() {
    let notes = |midi: i32| {
        (0..4)
            .map(|i| {
                let mut n = Note::new(
                    i as NoteId,
                    SpelledPitch::from_midi(midi, None),
                    BeatTime::from_quarters(i),
                    BeatTime::from_quarters(1),
                );
                n.midi = midi;
                n
            })
            .collect::<Vec<Note>>()
    };
    let part = |name: &str, midi: i32| Part {
        role: ArrangementRole::HarmonicBed,
        name: name.to_string(),
        notes: notes(midi),
        instrument_profile: None,
        channel: 0,
        polyphonic: false,
    };
    let stacked = masking_report(&[part("a", 60), part("b", 61)]);
    let separated = masking_report(&[part("a", 40), part("b", 80)]);
    assert_eq!(stacked.count(), 4);
    assert_eq!(separated.count(), 0);
    assert!(!stacked.suggestions.is_empty());
}

// ---------------------------------------------------------------------------
// §28: requested roles produce distinct parts
// ---------------------------------------------------------------------------

#[test]
fn roles_produce_distinct_parts() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h.arrange(&full_request()).expect("a plan");
    assert!(plan.parts.len() >= 5, "most requested roles should sound");
    for i in 0..plan.parts.len() {
        for j in (i + 1)..plan.parts.len() {
            let (a, b) = (&plan.parts[i], &plan.parts[j]);
            assert_ne!(a.role, b.role);
            let onsets = |p: &Part| {
                let mut o: Vec<BeatTime> = p.notes.iter().map(|n| n.onset).collect();
                o.sort();
                o.dedup();
                o
            };
            let pitches = |p: &Part| {
                let mut m: Vec<i32> = p.notes.iter().map(|n| n.midi).collect();
                m.sort_unstable();
                m.dedup();
                m
            };
            assert!(
                onsets(a) != onsets(b) || pitches(a) != pitches(b),
                "{} and {} are the same part twice",
                a.role.id(),
                b.role.id()
            );
        }
    }
}

#[test]
fn every_role_can_be_requested_on_its_own() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    for role in ArrangementRole::all() {
        let plan = h
            .arrange(&ArrangementParams::default().with_roles(&[*role]))
            .expect("a plan");
        let assignment = plan
            .assignment(*role)
            .unwrap_or_else(|| panic!("{} got no assignment", role.id()));
        assert!(!assignment.pattern_id.is_empty());
        assert!(!assignment.instrument_profile.is_empty());
        assert!(!assignment.rationale.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Knowledge-declared behaviours
// ---------------------------------------------------------------------------

#[test]
fn instrument_ranges_are_data_driven() {
    let chords = chords_from(&["C", "Am", "F", "G7"], 4);
    // The same pattern on two different instruments must respect two different
    // ranges — there is no universal limit anywhere in the engine.
    let p = pattern("arr_root_pulse_bass");
    let low = realize_pattern(
        kb(),
        p,
        &chords,
        &four_four(),
        instrument("sub_bass"),
        1.0,
        4,
    )
    .expect("a realisation");
    let high = realize_pattern(kb(), p, &chords, &four_four(), instrument("mallet"), 1.0, 4)
        .expect("a realisation");
    for n in &low {
        assert!(instrument("sub_bass").range.contains(n.midi));
    }
    for n in &high {
        assert!(instrument("mallet").range.contains(n.midi));
    }
    let low_max = low.iter().map(|n| n.midi).max().expect("notes");
    let high_min = high.iter().map(|n| n.midi).min().expect("notes");
    assert!(
        low_max < high_min,
        "the two instrument profiles produced the same register"
    );
}

#[test]
fn polyphony_limit_enforced() {
    // A six-voice pattern on a monophonic instrument must come out monophonic.
    let chords = chords_from(&["Cmaj9"], 8);
    let notes = realize_pattern(
        kb(),
        pattern("arr_sustained_pad"),
        &chords,
        &four_four(),
        instrument("synth_lead"),
        1.0,
        6,
    )
    .expect("a realisation");
    assert!(!notes.is_empty());
    assert_eq!(
        density::max_polyphony(&notes),
        1,
        "a monophonic instrument sounded a chord"
    );

    let h = harness("melodies/eight_bar_c_major", "jazz_standard");
    let plan = h.arrange(&full_request()).expect("a plan");
    for (i, part) in plan.parts.iter().enumerate() {
        let profile = instrument(part.instrument_profile.as_deref().expect("an instrument"));
        assert!(
            plan.metrics[i].max_polyphony <= profile.polyphony.max(1) as usize,
            "{} exceeded {}'s polyphony",
            part.role.id(),
            profile.id
        );
    }
}

#[test]
fn doubling_limited_by_role() {
    // A pattern that declares no doubling never writes two voices on the same
    // pitch class; one that declares an octave does.
    let chords = chords_from(&["C"], 8);
    let undoubled = realize_pattern(
        kb(),
        pattern("arr_offbeat_comping"),
        &chords,
        &four_four(),
        instrument("guitar_comping"),
        1.0,
        6,
    )
    .expect("a realisation");
    assert!(!undoubled.is_empty());
    for n in &undoubled {
        let simultaneous: Vec<i32> = undoubled
            .iter()
            .filter(|o| o.onset == n.onset)
            .map(|o| o.midi.rem_euclid(12))
            .collect();
        let mut unique = simultaneous.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            simultaneous.len(),
            unique.len(),
            "arr_offbeat_comping doubled a pitch class it never declared"
        );
    }

    let doubled = realize_pattern(
        kb(),
        pattern("arr_pedal_texture"),
        &chords,
        &four_four(),
        instrument("pad"),
        1.0,
        6,
    )
    .expect("a realisation");
    let first = doubled[0].onset;
    let stack: Vec<i32> = doubled
        .iter()
        .filter(|n| n.onset == first)
        .map(|n| n.midi)
        .collect();
    assert_eq!(stack.len(), 2, "the pedal declares two voices");
    assert_eq!(
        (stack[1] - stack[0]) % 12,
        0,
        "the declared doubling is an octave"
    );
}

#[test]
fn sub_bass_root_only() {
    let chords = chords_from(&["Cmaj7", "Am7", "Fmaj7", "G13"], 4);
    let notes = realize_pattern(
        kb(),
        pattern("arr_syncopated_sub_bass"),
        &chords,
        &four_four(),
        instrument("sub_bass"),
        1.0,
        6,
    )
    .expect("a realisation");
    assert!(!notes.is_empty());
    for n in &notes {
        let chord = patterns::chord_at(&chords, n.onset);
        assert_eq!(
            n.midi.rem_euclid(12),
            chord.spec.root_pc(),
            "the sub bass sounded something other than the root at {}",
            n.onset
        );
    }
    assert_eq!(density::max_polyphony(&notes), 1);
}

#[test]
fn energy_curve_changes_layer_count() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let flat = |v: f64| {
        [
            (BeatTime::ZERO, v),
            (BeatTime::from_quarters(32), v),
        ]
    };
    let quiet = h
        .arrange(&full_request().with_energy_curve(&flat(0.0)))
        .expect("a plan");
    let loud = h
        .arrange(&full_request().with_energy_curve(&flat(1.0)))
        .expect("a plan");
    assert!(
        loud.layer_count() > quiet.layer_count(),
        "the energy curve did not change the layer count: {} vs {}",
        quiet.layer_count(),
        loud.layer_count()
    );
    assert!(quiet.layer_count() >= 1, "something must still sound");
}

#[test]
fn foreground_priority_resolves_conflict() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(
            &full_request()
                .with_register_spread(1.0)
                .with_roles(&[ArrangementRole::Counterlead, ArrangementRole::Comping]),
        )
        .expect("a plan");
    let counter = plan
        .assignment(ArrangementRole::Counterlead)
        .expect("an assignment");
    let comping = plan
        .assignment(ArrangementRole::Comping)
        .expect("an assignment");
    assert_eq!(counter.priority, 0, "the counterlead pattern is foreground");
    assert!(
        comping.priority >= counter.priority,
        "the background part must not outrank the foreground one"
    );
    // The foreground part keeps the register the catalogue gave it; the other
    // one is the one that moved.
    let catalogue = {
        let p = pattern(&counter.pattern_id);
        let i = instrument(&counter.instrument_profile);
        roles::base_window(p, i)
    };
    let displacement = ((counter.register.0 + counter.register.1) / 2
        - (catalogue.0 + catalogue.1) / 2)
        .abs();
    assert!(
        displacement <= 12,
        "the foreground part was displaced by {displacement} semitones"
    );
}

#[test]
fn layer_removal_keeps_guide_tones() {
    let h = with_chords(
        harness("melodies/eight_bar_c_major", "jazz_standard"),
        chords_from(&["Dm7", "G7", "Cmaj7", "A7"], 4),
    );
    let plan = h.arrange(&full_request()).expect("a plan");
    let essential = essential_layers(&plan.parts, &h.candidate.chords);
    let removable: Vec<ArrangementRole> = plan
        .parts
        .iter()
        .map(|p| p.role)
        .filter(|r| !essential.contains(r))
        .collect();
    assert!(
        !removable.is_empty(),
        "every layer was essential, so nothing could ever be taken away"
    );
    for role in removable {
        assert!(
            guide_tones_survive_without(&plan.parts, &h.candidate.chords, role),
            "removing {} lost a guide tone",
            role.id()
        );
    }
}

#[test]
fn breakdown_preserves_harmony() {
    let chords = chords_from(&["Dm7", "G7", "Cmaj7", "A7"], 4);
    let notes = realize_pattern(
        kb(),
        pattern("arr_density_breakdown"),
        &chords,
        &four_four(),
        instrument("pluck"),
        0.05,
        6,
    )
    .expect("a realisation");
    assert!(!notes.is_empty(), "a breakdown still plays something");
    for n in &notes {
        let chord = patterns::chord_at(&chords, n.onset);
        assert!(
            chord.spec.pitch_classes().contains(&n.midi.rem_euclid(12)),
            "the breakdown sounded a pitch outside {}",
            chord.spec.render_ascii()
        );
    }
    // Every chord is still represented.
    for chord in &chords {
        assert!(
            notes
                .iter()
                .any(|n| n.onset >= chord.onset && n.onset < chord.end()),
            "{} was dropped entirely by the breakdown",
            chord.spec.render_ascii()
        );
    }
}

#[test]
fn contrast_uses_more_than_velocity() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let sections = [
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
            energy: 0.9,
        },
    ];
    let plan = h
        .arrange(&full_request().with_sections(&sections))
        .expect("a plan");
    let mut dimensions = 0usize;
    for part in &plan.parts {
        let a = density::measure(
            &part
                .notes
                .iter()
                .filter(|n| n.onset < BeatTime::from_quarters(16))
                .cloned()
                .collect::<Vec<Note>>(),
            (BeatTime::ZERO, BeatTime::from_quarters(16)),
        );
        let b = density::measure(
            &part
                .notes
                .iter()
                .filter(|n| n.onset >= BeatTime::from_quarters(16))
                .cloned()
                .collect::<Vec<Note>>(),
            (BeatTime::from_quarters(16), BeatTime::from_quarters(32)),
        );
        dimensions += density::contrast(&a, &b).non_dynamic_dimensions();
    }
    assert!(
        dimensions > 0,
        "the two sections differ only in how loud they are"
    );
    let plans = &plan.sections;
    assert_ne!(plans[0].register_shift, plans[1].register_shift);
    assert!((plans[0].density - plans[1].density).abs() > 0.05);
    assert!((plans[0].note_length_scale - plans[1].note_length_scale).abs() > 0.05);
}

#[test]
fn transition_fill_before_boundary() {
    let h = harness("melodies/eight_bar_c_major", "pop_rock");
    let plan = h
        .arrange(&ArrangementParams::default().with_roles(&[
            ArrangementRole::Bass,
            ArrangementRole::Transition,
        ]))
        .expect("a plan");
    let fill = plan.part(ArrangementRole::Transition).expect("a fill");
    assert!(!fill.notes.is_empty(), "the transition wrote nothing");
    let boundaries: Vec<BeatTime> = plan
        .sections
        .iter()
        .map(|s| s.section.end)
        .chain(plan.loop_plan.transition_qn)
        .collect();
    let bar = h.time_map().meter_at(plan.span.0).bar_length_qn();
    for n in &fill.notes {
        assert!(
            boundaries
                .iter()
                .any(|b| n.onset < *b && n.onset >= *b - bar),
            "a fill note at {} is not in the bar before any boundary",
            n.onset
        );
    }
}
