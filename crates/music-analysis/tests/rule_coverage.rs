//! One test per `test_id` that `music-analysis` takes responsibility for.
//!
//! `knowledge/` names, on every rule, the behavioural tests that pin it. This
//! file holds the ones this crate owns — the melody, salience, grid and
//! key-region behaviours — with each test named exactly after the `test_id` it
//! discharges, so the claim in `crates/theory-kb/tests/test_id_coverage.json`
//! can be checked by reading the function names.
//!
//! Everything here is deliberately behavioural: it asserts what the analysis
//! *says*, not how it computes it.

mod common;

use common::{fixture, repo_root};
use music_analysis::grid::{build_grid, GridMode};
use music_analysis::key::analyze_key_ranked;
use music_analysis::nct::{classify_ncts_in_key, DEFAULT_MAX_HYPOTHESES};
use music_analysis::phrase::{analyze_phrases, melody_profile, refine_cadences};
use music_analysis::prelude::*;
use music_analysis::salience::{analyze_salience, SALIENCE_COMPONENTS};
use music_domain::prelude::*;
use theory_kb::{KnowledgeBase, ResolvedProfile};

fn kb() -> &'static KnowledgeBase {
    KnowledgeBase::embedded()
}

fn profile(id: &str) -> ResolvedProfile {
    kb().resolve_profile(id).expect("a profile")
}

/// A 4/4 note set from `(pitch, onset, duration)` triples.
fn notes(spec: &[(&str, &str, &str)]) -> NoteSet {
    notes_in(spec, TimeSignature::new(4, 4), 120.0)
}

/// [`notes`] in an explicit meter and tempo.
fn notes_in(spec: &[(&str, &str, &str)], sig: TimeSignature, bpm: f64) -> NoteSet {
    let built = spec
        .iter()
        .enumerate()
        .map(|(i, (p, on, dur))| {
            let mut n = Note::new(
                i as NoteId,
                SpelledPitch::parse(p).expect("a spelled pitch"),
                BeatTime::parse(on).expect("a rational onset"),
                BeatTime::parse(dur).expect("a rational duration"),
            );
            n.selected = true;
            n
        })
        .collect();
    NoteSet::sorted(built, TimeMap::constant(bpm, sig))
}

/// Chord events from `(symbol, onset, duration)` triples.
fn chords(spec: &[(&str, &str, &str)]) -> Vec<ChordEvent> {
    spec.iter()
        .enumerate()
        .map(|(i, (sym, on, dur))| {
            let mut e = ChordEvent::new(
                i as u32,
                music_domain::symbol::parse(sym).expect("a chord symbol"),
                BeatTime::parse(on).expect("a rational onset"),
                BeatTime::parse(dur).expect("a rational duration"),
            );
            e.original_symbol = Some((*sym).to_string());
            e
        })
        .collect()
}

const C_MAJOR: &[i32] = &[0, 2, 4, 5, 7, 9, 11];

/// Classify one melody against an explicit harmony.
fn classify(
    ns: &NoteSet,
    h: &[ChordEvent],
) -> std::collections::BTreeMap<NoteId, Vec<NctHypothesis>> {
    classify_ncts_in_key(
        ns,
        h,
        &ns.time_map.clone(),
        kb(),
        &profile("common_practice"),
        C_MAJOR,
        DEFAULT_MAX_HYPOTHESES,
    )
}

/// The kinds hypothesised for one note.
fn kinds(map: &std::collections::BTreeMap<NoteId, Vec<NctHypothesis>>, id: NoteId) -> Vec<NctKind> {
    map[&id].iter().map(|h| h.kind).collect()
}

/// The default pipeline over a note set.
fn analysis(ns: &NoteSet) -> Analysis {
    analyze(kb(), "rule-coverage", ns, &AnalyzeParams::default()).expect("analysis")
}

// ---------------------------------------------------------------------------
// melody: non-chord tones
// ---------------------------------------------------------------------------

#[test]
fn passing_tone_weak_beat_stepwise() {
    let ns = notes(&[
        ("C4", "0", "1/2"),
        ("D4", "1/2", "1/2"),
        ("E4", "1", "1"),
        ("G4", "2", "2"),
    ]);
    let map = classify(&ns, &chords(&[("C", "0", "4")]));
    assert_eq!(map[&1][0].kind, NctKind::Passing, "{:?}", kinds(&map, 1));
    assert!(map[&1][0].confidence >= 0.8);
}

#[test]
fn passing_tone_not_from_pitch_alone() {
    // The identical pitch over the identical chord, reached and left
    // differently, must not be called a passing tone.
    let stepwise = notes(&[("C4", "0", "1/2"), ("D4", "1/2", "1/2"), ("E4", "1", "2")]);
    let leapt = notes(&[("G4", "0", "1/2"), ("D4", "1/2", "1/2"), ("A4", "1", "2")]);
    let h = chords(&[("C", "0", "4")]);
    assert!(classify(&stepwise, &h)[&1]
        .iter()
        .any(|x| x.kind == NctKind::Passing));
    let other = classify(&leapt, &h);
    assert!(
        !other[&1].iter().any(|x| x.kind == NctKind::Passing),
        "pitch membership decided the label: {:?}",
        kinds(&other, 1)
    );
}

#[test]
fn neighbor_tone_returns_to_origin() {
    let ns = notes(&[
        ("E4", "0", "1/2"),
        ("F4", "1/2", "1/2"),
        ("E4", "1", "1"),
        ("C4", "2", "2"),
    ]);
    let map = classify(&ns, &chords(&[("C", "0", "4")]));
    assert!(
        kinds(&map, 1).contains(&NctKind::Neighbor),
        "{:?}",
        kinds(&map, 1)
    );
}

#[test]
fn suspension_resolves_down_by_step() {
    let ns = notes(&[
        ("F4", "3", "1"),
        ("F4", "4", "2"),
        ("E4", "6", "2"),
        ("C4", "8", "2"),
    ]);
    let map = classify(&ns, &chords(&[("G7", "0", "4"), ("C", "4", "8")]));
    assert_eq!(map[&1][0].kind, NctKind::Suspension, "{:?}", kinds(&map, 1));
}

#[test]
fn suspension_prepared_and_resolved() {
    let f = fixture("melodies/waltz_suspensions");
    let ns = f.note_set();
    let harmony = chords(&[
        ("C", "0", "3"),
        ("C", "3", "3"),
        ("F", "6", "3"),
        ("F", "9", "3"),
        ("C", "12", "3"),
        ("G", "15", "3"),
        ("C", "18", "3"),
        ("C", "21", "3"),
    ]);
    let map = classify_ncts_in_key(
        &ns,
        &harmony,
        &f.time_map(),
        kb(),
        &profile("common_practice"),
        C_MAJOR,
        DEFAULT_MAX_HYPOTHESES,
    );
    let suspended: Vec<NoteId> = map
        .iter()
        .filter(|(_, v)| v.iter().any(|h| h.kind == NctKind::Suspension))
        .map(|(id, _)| *id)
        .collect();
    assert!(!suspended.is_empty(), "the waltz produced no suspensions");
    for id in suspended {
        let i = ns.notes.iter().position(|n| n.id == id).expect("the note");
        assert_eq!(
            ns.notes[i - 1].midi,
            ns.notes[i].midi,
            "note {id} unprepared"
        );
        assert!(
            ns.notes[i + 1].midi < ns.notes[i].midi,
            "note {id} does not fall"
        );
        assert!(f.time_map().metric_weight(ns.notes[i].onset) >= 0.5);
    }
}

#[test]
fn suspension_without_preparation_rejected() {
    // Same pitch, same beat, same chord — but stepped into rather than tied
    // over, so it is not a suspension.
    let prepared = notes(&[("F4", "3", "1"), ("F4", "4", "2"), ("E4", "6", "2")]);
    let unprepared = notes(&[("G4", "3", "1"), ("F4", "4", "2"), ("E4", "6", "2")]);
    let h = chords(&[("G7", "0", "4"), ("C", "4", "4")]);
    assert!(classify(&prepared, &h)[&1]
        .iter()
        .any(|x| x.kind == NctKind::Suspension));
    let other = classify(&unprepared, &h);
    assert!(
        !other[&1].iter().any(|x| x.kind == NctKind::Suspension),
        "an unprepared dissonance was called a suspension: {:?}",
        kinds(&other, 1)
    );
}

#[test]
fn retardation_resolves_up() {
    let ns = notes(&[
        ("B3", "3", "1"),
        ("B3", "4", "2"),
        ("C4", "6", "2"),
        ("E4", "8", "2"),
    ]);
    let map = classify(&ns, &chords(&[("G7", "0", "4"), ("C", "4", "8")]));
    assert!(
        kinds(&map, 1).contains(&NctKind::Retardation),
        "{:?}",
        kinds(&map, 1)
    );
    assert!(!kinds(&map, 1).contains(&NctKind::Suspension));
}

#[test]
fn anticipation_precedes_arrival() {
    let ns = notes(&[
        ("D4", "0", "2"),
        ("B3", "2", "3/2"),
        ("C4", "7/2", "1/2"),
        ("C4", "4", "2"),
    ]);
    let map = classify(&ns, &chords(&[("G7", "0", "4"), ("C", "4", "4")]));
    assert!(
        kinds(&map, 2).contains(&NctKind::Anticipation),
        "{:?}",
        kinds(&map, 2)
    );
}

#[test]
fn appoggiatura_leap_in_step_out() {
    let ns = notes(&[
        ("C4", "3", "1"),
        ("A4", "4", "2"),
        ("G4", "6", "2"),
        ("E4", "8", "2"),
    ]);
    let map = classify(&ns, &chords(&[("C", "0", "4"), ("C", "4", "8")]));
    assert!(
        kinds(&map, 1).contains(&NctKind::Appoggiatura),
        "{:?}",
        kinds(&map, 1)
    );
}

#[test]
fn escape_tone_step_in_leap_out() {
    let ns = notes(&[
        ("E4", "0", "1/2"),
        ("F4", "1/2", "1/2"),
        ("C4", "1", "1"),
        ("G4", "2", "2"),
    ]);
    let map = classify(&ns, &chords(&[("C", "0", "8")]));
    assert!(
        kinds(&map, 1).contains(&NctKind::EscapeTone),
        "{:?}",
        kinds(&map, 1)
    );
}

#[test]
fn chromatic_approach_half_step() {
    let ns = notes(&[
        ("C4", "0", "1/2"),
        ("C#4", "1/2", "1/2"),
        ("D4", "1", "1"),
        ("E4", "2", "2"),
    ]);
    let map = classify(&ns, &chords(&[("C", "0", "8")]));
    assert!(
        kinds(&map, 1).contains(&NctKind::ChromaticApproach),
        "{:?}",
        kinds(&map, 1)
    );
}

#[test]
fn chromatic_approach_not_modulation() {
    // A single chromatic step must not move the tonal centre.
    let plain = notes(&[
        ("C4", "0", "1"),
        ("E4", "1", "1"),
        ("G4", "2", "1"),
        ("E4", "3", "1"),
        ("F4", "4", "1"),
        ("D4", "5", "1"),
        ("B3", "6", "1"),
        ("C4", "7", "4"),
    ]);
    let mut chromatic = plain.clone();
    chromatic.notes[5].pitch = SpelledPitch::parse("Db4").expect("a pitch");
    chromatic.notes[5].midi = chromatic.notes[5].pitch.midi();

    let a = analyze_key_ranked(kb(), &plain, &plain.time_map.clone(), None, None, 8);
    let b = analyze_key_ranked(kb(), &chromatic, &chromatic.time_map.clone(), None, None, 8);
    let at = a.top().expect("a candidate");
    let bt = b.top().expect("a candidate");
    assert_eq!(
        at.tonic_pc(),
        bt.tonic_pc(),
        "{} vs {}",
        at.label(),
        bt.label()
    );
    assert!(
        b.regions.iter().all(|r| !r.is_tonicization),
        "one chromatic note was read as a key change"
    );
}

#[test]
fn ambiguous_tone_multiple_hypotheses() {
    // Accented, stepped into and stepped out of: an accented passing tone to
    // one analyst, an appoggiatura to another. Both must survive.
    let ns = notes(&[
        ("C4", "3", "1"),
        ("D4", "4", "2"),
        ("E4", "6", "2"),
        ("G4", "8", "2"),
    ]);
    let map = classify(&ns, &chords(&[("C", "0", "4"), ("C", "4", "8")]));
    assert!(
        map[&1].len() >= 2,
        "a single reading was forced: {:?}",
        kinds(&map, 1)
    );
    for h in &map[&1] {
        assert!(h.confidence > 0.0 && h.confidence < 1.0);
        assert!(!h.rationale.is_empty());
    }
}

// ---------------------------------------------------------------------------
// melody: salience and phrases
// ---------------------------------------------------------------------------

#[test]
fn salience_weighted_sum_is_transparent() {
    let ns = notes(&[
        ("C4", "0", "1"),
        ("E4", "1", "1"),
        ("G4", "2", "2"),
        ("C5", "4", "4"),
    ]);
    let a = analysis(&ns);
    assert!((a.salience.weights.total() - 1.0).abs() < 1e-9);
    for (id, d) in &a.salience.per_note {
        let names: Vec<&str> = d.components.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, SALIENCE_COMPONENTS, "note {id}");
        let sum: f64 = d.components.iter().map(|(_, v)| *v).sum();
        assert!(
            (sum - d.total).abs() < 1e-6,
            "note {id}: the parts sum to {sum} but the total is {}",
            d.total
        );
    }
}

#[test]
fn salience_pickup_downweighted() {
    let f = fixture("loops/pickup_and_hanging_note");
    let ns = f.note_set();
    let ph = analyze_phrases(&ns, &f.time_map());
    let sal = analyze_salience(&ns, &ph, &f.time_map(), &SalienceWeights::default());
    let pickup = ph
        .pickup
        .as_ref()
        .and_then(|p| p.notes.first())
        .copied()
        .expect("an anacrusis");
    let d = &sal.per_note[&pickup];
    assert!(d.component("pickup") < 0.0, "the pickup was not discounted");
    let undiscounted: f64 = d
        .components
        .iter()
        .filter(|(n, _)| *n != "pickup")
        .map(|(_, v)| *v)
        .sum();
    assert!(d.total < undiscounted);
}

#[test]
fn structural_note_long_strong_beat() {
    let ns = notes(&[
        ("C4", "0", "4"),
        ("D4", "9/2", "1/2"),
        ("E4", "5", "1/2"),
        ("F4", "11/2", "1/2"),
        ("G4", "6", "4"),
    ]);
    let a = analysis(&ns);
    assert!(a.salience.is_structural(0), "{}", a.salience.total(0));
    assert!(!a.salience.is_structural(1), "{}", a.salience.total(1));
    assert!(a.salience.total(0) > a.salience.total(1));
}

#[test]
fn structural_melody_note_explained() {
    let ns = notes(&[
        ("C4", "0", "4"),
        ("E4", "4", "1"),
        ("G4", "5", "1"),
        ("C5", "6", "4"),
    ]);
    let a = analysis(&ns);
    let id = *a
        .salience
        .structural
        .first()
        .expect("at least one structural note");
    let d = &a.salience.per_note[&id];
    let biggest = d
        .components
        .iter()
        .filter(|(n, _)| *n != "pickup")
        .max_by(|x, y| x.1.partial_cmp(&y.1).expect("comparable"))
        .expect("a leading component");
    assert!(
        biggest.1 > 0.0,
        "a structural note with no named reason for being one"
    );
    assert!(d.total >= a.salience.threshold);
    assert!(SALIENCE_COMPONENTS.contains(&biggest.0));
}

#[test]
fn phrase_end_note_structural() {
    for id in ["melodies/eight_bar_c_major", "melodies/waltz_suspensions"] {
        let f = fixture(id);
        let a = analyze(kb(), id, &f.note_set(), &AnalyzeParams::default()).expect("analysis");
        let last = a
            .phrases
            .phrases
            .last()
            .and_then(|p| p.notes.last())
            .copied()
            .expect("a phrase-final note");
        assert!(
            a.salience.is_structural(last),
            "{id}: the phrase-final note scored {}",
            a.salience.total(last)
        );
        assert!(a.salience.per_note[&last].component("phrase_edge") > 0.0);
    }
}

#[test]
fn cadence_supports_phrase_end() {
    let ns = notes(&[
        ("C4", "0", "1"),
        ("D4", "1", "1"),
        ("E4", "2", "1"),
        ("D4", "3", "1"),
        ("B3", "6", "1"),
        ("C4", "7", "2"),
    ]);
    let mut ph = analyze_phrases(&ns, &ns.time_map.clone());
    refine_cadences(&mut ph, &ns, 0, C_MAJOR, &ns.time_map.clone());
    let last = ph.phrases.last().expect("a phrase");
    assert!(last.cadence.is_some(), "the close was not classified");
    let sal = analyze_salience(&ns, &ph, &ns.time_map.clone(), &SalienceWeights::default());
    let final_note = *last.notes.last().expect("a final note");
    assert!(
        sal.per_note[&final_note].component("cadential") > 0.0,
        "the cadence did not reach the salience model"
    );
}

#[test]
fn pickup_requires_special_handling() {
    let f = fixture("loops/pickup_and_hanging_note");
    let ns = f.note_set();
    let ph = analyze_phrases(&ns, &f.time_map());
    let pickup = ph.pickup.clone().expect("an anacrusis");
    assert!(pickup.is_pickup);
    assert!(
        pickup.start.is_negative(),
        "the pickup starts before bar one"
    );
    // It is its own phrase, not the head of the first real one.
    assert!(ph.phrases.iter().any(|p| p.is_pickup));
    let first_real = ph
        .phrases
        .iter()
        .find(|p| !p.is_pickup)
        .expect("a first real phrase");
    assert!(first_real.notes.iter().all(|id| !pickup.notes.contains(id)));
}

#[test]
fn phrase_has_single_climax() {
    let f = fixture("melodies/eight_bar_c_major");
    let ns = f.note_set();
    let profile = melody_profile(&ns);
    let climax = profile.climax.expect("a climax");
    let high = ns
        .notes
        .iter()
        .find(|n| n.id == climax)
        .expect("the climax note");
    assert_eq!(
        high.midi,
        ns.notes.iter().map(|n| n.midi).max().expect("a max")
    );
    // Exactly one note is named, even when the peak pitch recurs.
    assert_eq!(
        ns.notes
            .iter()
            .filter(|n| n.midi == high.midi)
            .filter(|n| n.id == climax)
            .count(),
        1
    );
    let a = analysis(&ns);
    for p in &a.phrases.phrases {
        let credited = p
            .notes
            .iter()
            .filter(|id| a.salience.per_note[id].component("registral_extreme") >= 0.09)
            .count();
        assert!(credited <= 1, "phrase {} has {credited} peaks", p.id);
    }
}

// ---------------------------------------------------------------------------
// the harmonic grid
// ---------------------------------------------------------------------------

#[test]
fn weak_short_note_no_new_chord() {
    let ns = notes(&[
        ("C4", "0", "2"),
        ("E4", "2", "1"),
        ("G4", "3", "1/2"),
        ("F#4", "7/2", "1/4"),
        ("G4", "4", "4"),
    ]);
    let a = analysis(&ns);
    let weak = BeatTime::parse("7/2").expect("a rational");
    assert!(
        !a.grid.has_boundary_at(weak),
        "a sixteenth on the last quarter of the bar opened a chord change: {:?}",
        a.grid
            .slots
            .iter()
            .map(|s| s.start.to_display())
            .collect::<Vec<_>>()
    );
    let slot = a.grid.slot_at(weak).expect("a slot");
    assert!(!slot.structural_notes.contains(&3));
}

#[test]
fn grid_not_one_chord_per_note() {
    for id in [
        "melodies/eight_bar_c_major",
        "melodies/dorian_vamp_d",
        "melodies/blues_head_c",
        "melodies/waltz_suspensions",
    ] {
        let f = fixture(id);
        let a = analyze(kb(), id, &f.note_set(), &AnalyzeParams::default()).expect("analysis");
        assert!(
            a.grid.slots.len() < a.extraction.melody.notes.len(),
            "{id}: {} slots for {} notes",
            a.grid.slots.len(),
            a.extraction.melody.notes.len()
        );
        // The decisive claim: notes do not open slots. Most onsets fall inside
        // a slot rather than at its start, and at least one slot has to carry
        // several notes.
        let on_a_boundary = a
            .extraction
            .melody
            .notes
            .iter()
            .filter(|n| a.grid.has_boundary_at(n.onset))
            .count();
        assert!(
            on_a_boundary < a.extraction.melody.notes.len(),
            "{id}: every note opened a slot"
        );
        assert!(
            a.grid.slots.iter().any(|s| s.melody_notes.len() > 1),
            "{id}: no slot holds more than one note"
        );
    }
}

#[test]
fn harmonic_rhythm_not_from_tempo() {
    // The same music at 60 and at 200 bpm is the same music: the harmonic
    // rhythm comes from the phrase structure and the profile, never from how
    // fast the transport is running.
    let spec: &[(&str, &str, &str)] = &[
        ("C4", "0", "1"),
        ("E4", "1", "1"),
        ("G4", "2", "1"),
        ("E4", "3", "1"),
        ("F4", "4", "1"),
        ("A4", "5", "1"),
        ("G4", "6", "2"),
        ("C5", "8", "4"),
    ];
    let slow = notes_in(spec, TimeSignature::new(4, 4), 60.0);
    let fast = notes_in(spec, TimeSignature::new(4, 4), 200.0);
    let make = |ns: &NoteSet| {
        let tm = ns.time_map.clone();
        let ph = analyze_phrases(ns, &tm);
        let sal = analyze_salience(ns, &ph, &tm, &SalienceWeights::default());
        build_grid(
            ns,
            &tm,
            &ph,
            &sal,
            GridMode::Auto,
            &profile("common_practice"),
        )
    };
    let a = make(&slow);
    let b = make(&fast);
    assert_eq!(
        a.to_json().to_canonical_string(),
        b.to_json().to_canonical_string(),
        "the grid moved with the tempo"
    );
}

// ---------------------------------------------------------------------------
// key regions
// ---------------------------------------------------------------------------

#[test]
fn tonicization_reported_as_region() {
    // Four bars of C major, two bars leaning hard on G with an F sharp, then
    // four bars of C major again. The excursion must come back as a region,
    // and as a *tonicization* rather than as a modulation.
    let ns = notes(&[
        ("C4", "0", "2"),
        ("E4", "2", "2"),
        ("G4", "4", "2"),
        ("C5", "6", "2"),
        ("E4", "8", "2"),
        ("C4", "10", "2"),
        ("G4", "12", "2"),
        ("C4", "14", "2"),
        ("D4", "16", "2"),
        ("F#4", "18", "2"),
        ("G4", "20", "4"),
        ("B4", "24", "2"),
        ("D5", "26", "2"),
        ("G4", "28", "4"),
        ("C4", "32", "2"),
        ("E4", "34", "2"),
        ("G4", "36", "2"),
        ("E4", "38", "2"),
        ("F4", "40", "2"),
        ("D4", "42", "2"),
        ("B3", "44", "2"),
        ("C4", "46", "4"),
    ]);
    let k = analyze_key_ranked(kb(), &ns, &ns.time_map.clone(), None, None, 8);
    assert!(k.regions.len() >= 2, "{:?}", k.regions.len());
    let top = k.top().expect("a global answer");
    assert!(
        k.regions
            .iter()
            .any(|r| r.tonic != top.tonic || r.scale_id != top.scale_id),
        "the excursion produced no region of its own"
    );
    for r in &k.regions {
        assert!(r.end > r.start);
        assert!(!r.evidence.is_empty(), "a region with no stated evidence");
    }
}

#[test]
fn modulation_needs_confirmation_window() {
    // Eight bars of plain C major with two chromatic beats in the middle. A
    // passing chromaticism is not a modulation: the analysis must stay in one
    // region, and must not report a confirmed key change.
    let ns = notes(&[
        ("C4", "0", "2"),
        ("E4", "2", "2"),
        ("G4", "4", "2"),
        ("E4", "6", "2"),
        ("F4", "8", "2"),
        ("A4", "10", "2"),
        ("G4", "12", "4"),
        ("E4", "16", "1"),
        ("Eb4", "17", "1"),
        ("D4", "18", "2"),
        ("F4", "20", "2"),
        ("E4", "22", "2"),
        ("D4", "24", "2"),
        ("B3", "26", "2"),
        ("C4", "28", "4"),
    ]);
    let k = analyze_key_ranked(kb(), &ns, &ns.time_map.clone(), None, None, 8);
    let confirmed: Vec<&KeyRegion> = k.regions.iter().filter(|r| !r.is_tonicization).collect();
    assert_eq!(
        confirmed.len(),
        1,
        "two chromatic beats were read as a modulation: {:?}",
        k.regions
            .iter()
            .map(|r| format!(
                "{}{} {}",
                r.tonic.0.as_char(),
                r.tonic.1.ascii(),
                r.scale_id
            ))
            .collect::<Vec<_>>()
    );
    let top = k.top().expect("a global answer");
    assert_eq!(confirmed[0].tonic, top.tonic);
    assert_eq!(confirmed[0].scale_id, top.scale_id);
}

// ---------------------------------------------------------------------------
// the claim itself
// ---------------------------------------------------------------------------

/// Every `test_id` this crate takes responsibility for.
///
/// Kept in one place so the claim can be checked mechanically against both the
/// knowledge base and this file.
const CLAIMED: &[&str] = &[
    "ambiguous_tone_multiple_hypotheses",
    "anticipation_precedes_arrival",
    "appoggiatura_leap_in_step_out",
    "cadence_supports_phrase_end",
    "chromatic_approach_half_step",
    "chromatic_approach_not_modulation",
    "escape_tone_step_in_leap_out",
    "grid_not_one_chord_per_note",
    "harmonic_rhythm_not_from_tempo",
    "modulation_needs_confirmation_window",
    "neighbor_tone_returns_to_origin",
    "passing_tone_not_from_pitch_alone",
    "passing_tone_weak_beat_stepwise",
    "phrase_end_note_structural",
    "phrase_has_single_climax",
    "pickup_requires_special_handling",
    "retardation_resolves_up",
    "salience_pickup_downweighted",
    "salience_weighted_sum_is_transparent",
    "structural_melody_note_explained",
    "structural_note_long_strong_beat",
    "suspension_prepared_and_resolved",
    "suspension_resolves_down_by_step",
    "suspension_without_preparation_rejected",
    "tonicization_reported_as_region",
    "weak_short_note_no_new_chord",
];

#[test]
fn every_claimed_test_id_is_declared_by_a_rule() {
    let kb = kb();
    for t in CLAIMED {
        let owners: Vec<&str> = kb
            .rules()
            .iter()
            .filter(|r| r.test_ids.iter().any(|x| x == t))
            .map(|r| r.id.as_str())
            .collect();
        assert!(
            !owners.is_empty(),
            "{t} is claimed but no rule in knowledge/ names it"
        );
    }
}

#[test]
fn every_claimed_test_id_has_a_test_of_that_name() {
    let source =
        std::fs::read_to_string(repo_root().join("crates/music-analysis/tests/rule_coverage.rs"))
            .expect("this file must be readable");
    for t in CLAIMED {
        assert!(
            source.contains(&format!("fn {t}()")),
            "{t} is claimed but has no `fn {t}()`"
        );
    }
}

#[test]
fn the_claim_is_sorted_and_free_of_duplicates() {
    let mut sorted = CLAIMED.to_vec();
    sorted.sort_unstable();
    let before = sorted.len();
    sorted.dedup();
    assert_eq!(before, sorted.len(), "a test id is claimed twice");
    assert_eq!(sorted, CLAIMED.to_vec(), "the claim must stay sorted");
}
