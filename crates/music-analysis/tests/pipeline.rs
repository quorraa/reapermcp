//! End-to-end behaviour of the analysis pipeline over the committed corpus.
//!
//! These tests are written against the musical claims the product makes, not
//! against implementation details: they check that the melody extraction admits
//! what it assumed, that the key detector ranks modal and blues answers where
//! the material calls for them, that the grid does not collapse to one chord
//! per note, and that every fixture in the corpus analyses at all.

mod common;

use common::{analysis_notes, fixture, fixtures, has_own_notes, loop_span};
use music_analysis::grid::GridMode;
use music_analysis::key::analyze_key_ranked;
use music_analysis::nct::DEFAULT_MAX_HYPOTHESES;
use music_analysis::phrase::analyze_phrases;
use music_analysis::prelude::*;
use music_analysis::report::{AMBIGUOUS_KEY, HANGING_NOTE, PICKUP_BEFORE_LOOP};
use music_analysis::salience::analyze_salience;
use music_analysis::selection::{ExtractionMode, AMBIGUOUS_MELODY};
use music_analysis::voices::separate_voices;
use music_domain::prelude::*;
use theory_kb::KnowledgeBase;

fn kb() -> &'static KnowledgeBase {
    KnowledgeBase::embedded()
}

fn defaults() -> AnalyzeParams {
    AnalyzeParams::default()
}

fn run(id: &str, p: &AnalyzeParams) -> Analysis {
    let f = fixture(id);
    let notes = analysis_notes(&f);
    analyze(kb(), &format!("fixture:{id}"), &notes, p).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// Sounding pitch classes of a named collection on a named tonic.
fn collection(tonic: &str, scale_id: &str) -> Vec<i32> {
    let t = SpelledPitch::parse_class(tonic).expect("a spelled class");
    let pc = (t.0.natural_pc() + t.1 .0 as i32).rem_euclid(12);
    let def = kb().scale(scale_id).expect("a scale");
    let mut v: Vec<i32> = def
        .semitones
        .iter()
        .map(|s| (pc + s).rem_euclid(12))
        .collect();
    v.sort_unstable();
    v
}

/// The leading candidate's collection, sorted.
fn top_collection(a: &Analysis) -> Vec<i32> {
    let mut v = a.key.top_pcs(kb());
    v.sort_unstable();
    v
}

// ---------------------------------------------------------------------------
// the corpus
// ---------------------------------------------------------------------------

#[test]
fn the_corpus_is_eleven_fixtures() {
    assert_eq!(fixtures().len(), 11);
}

#[test]
fn every_fixture_analyses_without_error() {
    for f in fixtures() {
        let notes = analysis_notes(&f);
        let p = AnalyzeParams {
            extraction: if has_own_notes(&f) {
                ExtractionRequest::auto()
            } else {
                ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony)
            },
            loop_span: loop_span(&f),
            ..defaults()
        };
        let a = analyze(kb(), &f.id, &notes, &p).unwrap_or_else(|e| panic!("{}: {e}", f.id));
        assert!(!a.id.is_empty(), "{}", f.id);
        assert!(a.confidence > 0.0, "{}", f.id);
        assert!(!a.grid.slots.is_empty(), "{}", f.id);
    }
}

#[test]
fn every_fixture_produces_a_ranked_key_field() {
    for f in fixtures() {
        let notes = analysis_notes(&f);
        let k = analyze_key(kb(), &notes, &f.time_map(), None, None);
        assert!(!k.candidates.is_empty(), "{}", f.id);
        for w in k.candidates.windows(2) {
            assert!(w[0].score >= w[1].score, "{} is unsorted", f.id);
        }
        for c in &k.candidates {
            assert_eq!(c.evidence.len(), KEY_EVIDENCE.len(), "{}", f.id);
        }
    }
}

#[test]
fn every_fixtures_grid_is_coarser_than_its_note_list() {
    for f in fixtures() {
        let notes = analysis_notes(&f);
        if notes.notes.len() < 4 {
            continue;
        }
        let p = AnalyzeParams {
            extraction: if has_own_notes(&f) {
                ExtractionRequest::auto()
            } else {
                ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony)
            },
            ..defaults()
        };
        let a = analyze(kb(), &f.id, &notes, &p).expect("analysis");
        assert!(
            a.grid.slots.len() < notes.notes.len(),
            "{}: {} slots for {} notes",
            f.id,
            a.grid.slots.len(),
            notes.notes.len()
        );
    }
}

// ---------------------------------------------------------------------------
// melody extraction
// ---------------------------------------------------------------------------

#[test]
fn the_two_voice_fixture_is_not_silently_flattened() {
    let a = run("melodies/two_voice_polyphony", &defaults());
    assert_eq!(a.extraction.mode_used, ExtractionMode::HighestVoice);
    assert!(a.extraction.has_warning(AMBIGUOUS_MELODY));
    assert!(
        a.extraction.confidence < 0.5,
        "confidence is {}",
        a.extraction.confidence
    );
    assert!(a.extraction.voice_count >= 2);
    assert!(!a.extraction.accompaniment.notes.is_empty());
    assert!(
        a.extraction
            .assumptions
            .iter()
            .any(|s| s.contains("polyphonic")),
        "{:?}",
        a.extraction.assumptions
    );
}

#[test]
fn the_two_voice_fixture_top_line_is_the_upper_voice() {
    let a = run("melodies/two_voice_polyphony", &defaults());
    assert!(a.extraction.melody.notes.iter().all(|n| n.midi >= 62));
    assert!(a
        .extraction
        .accompaniment
        .notes
        .iter()
        .all(|n| n.midi <= 60));
}

#[test]
fn a_monophonic_fixture_is_extracted_with_confidence_and_no_ambiguity() {
    for id in [
        "melodies/eight_bar_c_major",
        "melodies/dorian_vamp_d",
        "melodies/waltz_suspensions",
        "melodies/blues_head_c",
        "melodies/chromatic_descent",
    ] {
        let a = run(id, &defaults());
        assert_eq!(
            a.extraction.mode_used,
            ExtractionMode::MonophonicVoice,
            "{id}"
        );
        assert!(!a.extraction.has_warning(AMBIGUOUS_MELODY), "{id}");
        assert!(a.extraction.confidence > 0.9, "{id}");
    }
}

#[test]
fn highest_and_lowest_voice_modes_split_the_two_voice_fixture() {
    let hi = run(
        "melodies/two_voice_polyphony",
        &AnalyzeParams {
            extraction: ExtractionRequest::mode(ExtractionMode::HighestVoice),
            ..defaults()
        },
    );
    let lo = run(
        "melodies/two_voice_polyphony",
        &AnalyzeParams {
            extraction: ExtractionRequest::mode(ExtractionMode::LowestVoice),
            ..defaults()
        },
    );
    assert!(hi.extraction.melody.notes.iter().all(|n| n.midi >= 62));
    assert!(lo.extraction.melody.notes.iter().all(|n| n.midi <= 60));
    assert_ne!(hi.melody.low.midi(), lo.melody.low.midi());
}

#[test]
fn midi_channel_mode_selects_only_that_channel() {
    let f = fixture("melodies/two_voice_polyphony");
    let mut notes = f.note_set();
    for n in notes.notes.iter_mut() {
        n.channel = if n.midi >= 62 { 0 } else { 5 };
    }
    let a = analyze(
        kb(),
        "channel-test",
        &notes,
        &AnalyzeParams {
            extraction: ExtractionRequest::channel(5),
            ..defaults()
        },
    )
    .expect("analysis");
    assert_eq!(a.extraction.mode_used, ExtractionMode::MidiChannel);
    assert!(a.extraction.melody.notes.iter().all(|n| n.channel == 5));
    assert!(a.extraction.melody.notes.iter().all(|n| n.midi < 62));
}

#[test]
fn all_notes_as_harmony_leaves_the_melody_empty_but_still_detects_chords() {
    let f = fixture("progressions/ii_v_i_c_major");
    let notes = analysis_notes(&f);
    let a = analyze(
        kb(),
        "harmony-test",
        &notes,
        &AnalyzeParams {
            extraction: ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony),
            ..defaults()
        },
    )
    .expect("analysis");
    assert!(a.extraction.melody.notes.is_empty());
    assert!(!a.detected_chords.is_empty());
}

#[test]
fn voice_separation_of_the_two_voice_fixture_is_deterministic() {
    let notes = fixture("melodies/two_voice_polyphony").note_set();
    let a = separate_voices(&notes, 8);
    let b = separate_voices(&notes, 8);
    assert_eq!(a, b);
    assert_eq!(a.len(), 2, "{a:?}");
    let mut all: Vec<NoteId> = a.iter().flatten().copied().collect();
    all.sort_unstable();
    let mut expected: Vec<NoteId> = notes.notes.iter().map(|n| n.id).collect();
    expected.sort_unstable();
    assert_eq!(all, expected);
}

#[test]
fn voice_separation_is_stable_under_a_larger_voice_budget() {
    let notes = fixture("melodies/two_voice_polyphony").note_set();
    assert_eq!(separate_voices(&notes, 4), separate_voices(&notes, 8));
}

// ---------------------------------------------------------------------------
// key and mode
// ---------------------------------------------------------------------------

#[test]
fn the_dorian_fixture_ranks_dorian_first() {
    let a = run("melodies/dorian_vamp_d", &defaults());
    let top = a.key.top().expect("a candidate");
    assert_eq!(top.scale_id, "dorian", "top is {}", top.label());
    assert_eq!(top.tonic.0, Letter::D);
    assert!(top.is_modal);
    assert!(!a.key.ambiguous, "gap is {}", a.key.gap);
}

#[test]
fn the_dorian_fixture_ranks_dorian_above_d_natural_minor() {
    let f = fixture("melodies/dorian_vamp_d");
    let k = analyze_key_ranked(kb(), &f.note_set(), &f.time_map(), None, None, 64);
    let dorian = k.rank_of(2, "dorian").expect("D dorian is in the field");
    let minor = k
        .rank_of(2, "natural_minor")
        .or_else(|| k.rank_of(2, "aeolian"))
        .expect("the D natural-minor collection is in the field");
    assert!(
        dorian < minor,
        "dorian ranked {dorian}, natural minor {minor}"
    );
}

#[test]
fn the_dorian_fixture_credits_the_natural_sixth() {
    let f = fixture("melodies/dorian_vamp_d");
    let k = analyze_key_ranked(kb(), &f.note_set(), &f.time_map(), None, None, 64);
    let dorian = k
        .candidates
        .iter()
        .find(|c| c.tonic_pc() == 2 && c.scale_id == "dorian")
        .expect("D dorian");
    let minor = k
        .candidates
        .iter()
        .find(|c| c.tonic_pc() == 2 && (c.scale_id == "natural_minor" || c.scale_id == "aeolian"))
        .expect("D natural minor");
    assert!(
        dorian.evidence_value("modal_characteristics")
            > minor.evidence_value("modal_characteristics"),
        "dorian {} vs minor {}",
        dorian.evidence_value("modal_characteristics"),
        minor.evidence_value("modal_characteristics")
    );
}

#[test]
fn the_blues_fixture_is_not_forced_into_major_or_minor() {
    let a = run("melodies/blues_head_c", &defaults());
    let top = a.key.top().expect("a candidate");
    assert!(
        !matches!(
            top.scale_id.as_str(),
            "major" | "ionian" | "natural_minor" | "aeolian" | "harmonic_minor" | "melodic_minor"
        ),
        "the blues head was labelled {}",
        top.label()
    );
    assert!(top.is_modal);
    assert_eq!(top.tonic.0, Letter::C);
}

#[test]
fn the_blues_fixture_leads_with_a_blues_collection() {
    let a = run("melodies/blues_head_c", &defaults());
    let top = a.key.top().expect("a candidate");
    assert_eq!(top.scale_id, "blues_minor", "top is {}", top.label());
}

#[test]
fn the_blues_fixture_keeps_the_flat_five() {
    let a = run("melodies/blues_head_c", &defaults());
    assert!(
        top_collection(&a).contains(&6),
        "the blue note was analysed away: {:?}",
        top_collection(&a)
    );
}

#[test]
fn the_diatonic_fixture_leads_with_c_major() {
    let a = run("melodies/eight_bar_c_major", &defaults());
    let top = a.key.top().expect("a candidate");
    assert_eq!(top.scale_id, "major", "top is {}", top.label());
    assert_eq!(top.tonic.0, Letter::C);
    assert!(!top.is_modal);
    assert!(!a.key.ambiguous);
}

#[test]
fn major_and_ionian_are_not_reported_as_two_answers() {
    let f = fixture("melodies/eight_bar_c_major");
    let k = analyze_key_ranked(kb(), &f.note_set(), &f.time_map(), None, None, 64);
    let c_major = k.rank_of(0, "major");
    let c_ionian = k.rank_of(0, "ionian");
    assert!(c_major.is_some());
    assert!(
        c_ionian.is_none(),
        "the same seven notes were reported twice"
    );
}

#[test]
fn the_waltz_fixture_leads_with_c_major() {
    let a = run("melodies/waltz_suspensions", &defaults());
    let top = a.key.top().expect("a candidate");
    assert_eq!(top.tonic.0, Letter::C, "top is {}", top.label());
    assert_eq!(top.scale_id, "major", "top is {}", top.label());
}

#[test]
fn the_two_voice_fixture_lands_on_the_c_major_collection() {
    let a = run("melodies/two_voice_polyphony", &defaults());
    assert_eq!(
        top_collection(&a),
        collection("C", "major"),
        "top is {}",
        a.key.top().expect("a candidate").label()
    );
}

#[test]
fn the_chromatic_fixture_is_reported_as_ambiguous() {
    let a = run("melodies/chromatic_descent", &defaults());
    assert!(a.key.ambiguous, "gap is {}", a.key.gap);
    assert!(a.has_warning(AMBIGUOUS_KEY));
}

#[test]
fn the_chromatic_fixture_is_not_fully_explained_by_one_collection() {
    let a = run("melodies/chromatic_descent", &defaults());
    let pcs = top_collection(&a);
    let outside: Vec<i32> = fixture("melodies/chromatic_descent")
        .note_set()
        .notes
        .iter()
        .map(|n| n.pitch_class())
        .filter(|pc| !pcs.contains(pc))
        .collect();
    assert!(
        !outside.is_empty(),
        "chromatic material was flattened into {:?}",
        pcs
    );
}

#[test]
fn a_user_hint_overrides_the_ranking_but_keeps_its_evidence() {
    let a = run(
        "melodies/dorian_vamp_d",
        &AnalyzeParams {
            tonal_center: Some(("Ab".to_string(), "lydian".to_string())),
            ..defaults()
        },
    );
    let top = a.key.top().expect("a candidate");
    assert_eq!(top.scale_id, "lydian");
    assert_eq!(top.tonic.0, Letter::A);
    assert!(top.evidence.iter().any(|(n, _)| *n == "user_hint"));
    assert!(
        top.evidence_value("pitch_distribution") < 0.5,
        "the hint must still be scored honestly"
    );
}

#[test]
fn a_fixtures_own_key_hint_is_a_plausible_candidate() {
    for f in fixtures() {
        let Some(hint) = &f.key_hint else { continue };
        let notes = analysis_notes(&f);
        let k = analyze_key_ranked(kb(), &notes, &f.time_map(), None, None, 96);
        let pc = (hint.tonic.0.natural_pc() + hint.tonic.1 .0 as i32).rem_euclid(12);
        let rank = k.rank_of(pc, &hint.scale_id);
        assert!(
            rank.is_some(),
            "{}: the declared key {}{} {} is not even a candidate",
            f.id,
            hint.tonic.0.as_char(),
            hint.tonic.1.ascii(),
            hint.scale_id
        );
    }
}

#[test]
fn long_material_produces_local_key_regions() {
    let a = run("melodies/dorian_vamp_d", &defaults());
    assert!(!a.key.regions.is_empty());
    for r in &a.key.regions {
        assert!(r.end > r.start);
        assert!(!r.evidence.is_empty());
        assert!(r.confidence >= 0.0);
    }
}

#[test]
fn a_local_window_can_be_narrowed() {
    let f = fixture("melodies/eight_bar_c_major");
    let wide = analyze_key(
        kb(),
        &f.note_set(),
        &f.time_map(),
        None,
        Some(BeatTime::from_quarters(16)),
    );
    let narrow = analyze_key(
        kb(),
        &f.note_set(),
        &f.time_map(),
        None,
        Some(BeatTime::from_quarters(4)),
    );
    assert!(narrow.regions.len() >= wide.regions.len());
}

// ---------------------------------------------------------------------------
// phrases, motives and salience
// ---------------------------------------------------------------------------

#[test]
fn the_eight_bar_fixture_segments_into_several_phrases() {
    let a = run("melodies/eight_bar_c_major", &defaults());
    assert!(
        a.phrases.phrases.len() >= 2,
        "{:?}",
        a.phrases.phrases.len()
    );
    let mut ids: Vec<NoteId> = a
        .phrases
        .phrases
        .iter()
        .flat_map(|p| p.notes.clone())
        .collect();
    ids.sort_unstable();
    let mut expected: Vec<NoteId> = a.extraction.melody.notes.iter().map(|n| n.id).collect();
    expected.sort_unstable();
    assert_eq!(ids, expected);
}

#[test]
fn phrase_boundaries_land_on_the_rests_of_the_loop_fixture() {
    let f = fixture("loops/pickup_and_hanging_note");
    let ns = f.note_set();
    let ph = analyze_phrases(&ns, &f.time_map());
    for (start, end) in &ph.gaps {
        assert!(end > start);
        assert!(
            ph.phrases.iter().any(|p| p.end <= *start),
            "no phrase closes at the rest starting {}",
            start.to_display()
        );
    }
}

#[test]
fn the_loop_fixture_pickup_is_detected() {
    let f = fixture("loops/pickup_and_hanging_note");
    let ns = f.note_set();
    let ph = analyze_phrases(&ns, &f.time_map());
    let pickup = ph.pickup.clone().expect("the anacrusis");
    assert!(pickup.is_pickup);
    assert!(pickup.start.is_negative());
}

#[test]
fn a_pickup_is_downweighted_in_the_salience_model() {
    let f = fixture("loops/pickup_and_hanging_note");
    let ns = f.note_set();
    let ph = analyze_phrases(&ns, &f.time_map());
    let sal = analyze_salience(&ns, &ph, &f.time_map(), &SalienceWeights::default());
    let pickup_id = ph
        .pickup
        .as_ref()
        .and_then(|p| p.notes.first())
        .copied()
        .expect("a pickup note");
    let detail = &sal.per_note[&pickup_id];
    assert!(detail.component("pickup") < 0.0);
    let raw: f64 = detail
        .components
        .iter()
        .filter(|(n, _)| *n != "pickup")
        .map(|(_, v)| *v)
        .sum();
    assert!(detail.total < raw);
}

#[test]
fn cadences_are_attached_to_phrases_once_the_key_is_known() {
    let a = run("melodies/eight_bar_c_major", &defaults());
    assert!(
        a.phrases.phrases.iter().any(|p| p.cadence.is_some()),
        "no phrase carries a cadence"
    );
}

#[test]
fn the_dorian_fixture_closes_modally_rather_than_authentically() {
    let a = run("melodies/dorian_vamp_d", &defaults());
    let last = a.phrases.phrases.last().expect("a phrase");
    assert_eq!(
        last.cadence,
        Some(CadenceKind::Modal),
        "a mode with no leading tone cannot cadence authentically"
    );
}

#[test]
fn the_eight_bar_fixture_has_recurring_motives() {
    let a = run("melodies/eight_bar_c_major", &defaults());
    assert!(!a.phrases.motives.is_empty());
    for m in &a.phrases.motives {
        assert!(m.occurrences.len() >= 2);
        assert_eq!(m.interval_profile.len() + 1, m.rhythm_profile.len());
    }
}

#[test]
fn motive_occurrences_are_disjoint_across_the_corpus() {
    for f in fixtures() {
        let ns = f.note_set();
        if ns.notes.is_empty() {
            continue;
        }
        let ph = analyze_phrases(&ns, &f.time_map());
        let mut seen: Vec<NoteId> = Vec::new();
        for m in &ph.motives {
            for o in &m.occurrences {
                for id in &o.notes {
                    assert!(!seen.contains(id), "{}: note {id} appears twice", f.id);
                    seen.push(*id);
                }
            }
        }
    }
}

#[test]
fn the_melody_profile_describes_the_eight_bar_fixture() {
    let a = run("melodies/eight_bar_c_major", &defaults());
    assert_eq!(a.melody.low.to_ascii(), "C4");
    assert_eq!(a.melody.high.to_ascii(), "C5");
    assert!(a.melody.tessitura.0 <= a.melody.tessitura.1);
    assert!(a.melody.climax.is_some());
    assert!(a.melody.density > 0.0);
    assert!(a.melody.mean_interval > 0.0);
    assert_eq!(a.melody.contour.len(), a.extraction.melody.notes.len() - 1);
}

#[test]
fn salience_components_sum_to_the_total_for_every_fixture() {
    for f in fixtures() {
        let notes = analysis_notes(&f);
        if notes.notes.is_empty() {
            continue;
        }
        let p = AnalyzeParams {
            extraction: if has_own_notes(&f) {
                ExtractionRequest::auto()
            } else {
                ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony)
            },
            ..defaults()
        };
        let a = analyze(kb(), &f.id, &notes, &p).expect("analysis");
        for (id, d) in &a.salience.per_note {
            let sum: f64 = d.components.iter().map(|(_, v)| *v).sum();
            assert!(
                (sum - d.total).abs() < 1e-5,
                "{} note {id}: {sum} vs {}",
                f.id,
                d.total
            );
        }
    }
}

#[test]
fn a_phrase_ending_note_is_structural_in_the_corpus() {
    for id in ["melodies/eight_bar_c_major", "melodies/waltz_suspensions"] {
        let a = run(id, &defaults());
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
    }
}

// ---------------------------------------------------------------------------
// the harmonic grid
// ---------------------------------------------------------------------------

#[test]
fn the_grid_is_not_one_chord_per_note() {
    let a = run("melodies/eight_bar_c_major", &defaults());
    assert!(
        a.grid.slots.len() * 2 <= a.extraction.melody.notes.len(),
        "{} slots for {} notes",
        a.grid.slots.len(),
        a.extraction.melody.notes.len()
    );
}

#[test]
fn a_weak_short_note_does_not_open_a_slot_in_the_corpus() {
    let a = run("melodies/blues_head_c", &defaults());
    for n in &a.extraction.melody.notes {
        let short = n.duration <= BeatTime::new(1, 2);
        let weak = a.extraction.melody.time_map.metric_weight(n.onset) < 0.5;
        if short && weak {
            assert!(
                !a.grid.has_boundary_at(n.onset),
                "the short weak note {} at {} opened a chord change",
                n.id,
                n.onset.to_display()
            );
        }
    }
}

#[test]
fn every_grid_mode_produces_a_covering_grid() {
    for mode in [
        GridMode::Auto,
        GridMode::Existing,
        GridMode::Bars(1.0),
        GridMode::Bars(0.5),
        GridMode::Beats(2.0),
    ] {
        let a = run(
            "melodies/eight_bar_c_major",
            &AnalyzeParams {
                grid: mode.clone(),
                ..defaults()
            },
        );
        assert_eq!(a.grid.mode_used, mode);
        assert!(!a.grid.rationale.is_empty());
        let (start, end) = a.extraction.melody.span();
        assert_eq!(a.grid.slots.first().expect("a slot").start, start);
        assert_eq!(a.grid.slots.last().expect("a slot").end, end);
        for w in a.grid.slots.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
    }
}

#[test]
fn a_finer_requested_grid_produces_more_slots() {
    let bar = run(
        "melodies/eight_bar_c_major",
        &AnalyzeParams {
            grid: GridMode::Bars(1.0),
            ..defaults()
        },
    );
    let half = run(
        "melodies/eight_bar_c_major",
        &AnalyzeParams {
            grid: GridMode::Bars(0.5),
            ..defaults()
        },
    );
    assert!(half.grid.slots.len() > bar.grid.slots.len());
}

#[test]
fn the_grid_rationale_explains_the_choice() {
    let auto = run("melodies/eight_bar_c_major", &defaults());
    assert!(auto.grid.rationale.contains("common_practice"));
    assert!(auto.grid.rationale.contains("phrase boundaries"));

    let existing = run(
        "melodies/eight_bar_c_major",
        &AnalyzeParams {
            grid: GridMode::Existing,
            ..defaults()
        },
    );
    assert!(existing.grid.rationale.contains("detected"));

    let bars = run(
        "melodies/eight_bar_c_major",
        &AnalyzeParams {
            grid: GridMode::Bars(2.0),
            ..defaults()
        },
    );
    assert!(bars.grid.rationale.contains("as requested"));
}

#[test]
fn the_waltz_grid_follows_the_three_four_bar() {
    let a = run(
        "melodies/waltz_suspensions",
        &AnalyzeParams {
            grid: GridMode::Bars(1.0),
            ..defaults()
        },
    );
    for s in &a.grid.slots {
        assert_eq!(s.length(), BeatTime::from_quarters(3));
    }
}

#[test]
fn a_slow_profile_produces_a_coarser_grid_than_a_busy_one() {
    let modal = run(
        "melodies/dorian_vamp_d",
        &AnalyzeParams {
            profile_id: "modal_ambient".to_string(),
            ..defaults()
        },
    );
    let jazz = run(
        "melodies/dorian_vamp_d",
        &AnalyzeParams {
            profile_id: "jazz_standard".to_string(),
            ..defaults()
        },
    );
    assert!(
        modal.grid.slots.len() <= jazz.grid.slots.len(),
        "modal {} vs jazz {}",
        modal.grid.slots.len(),
        jazz.grid.slots.len()
    );
}

// ---------------------------------------------------------------------------
// non-chord tones
// ---------------------------------------------------------------------------

/// A plain common-practice reading of the waltz, one chord per 3/4 bar.
///
/// The fixture carries no chords of its own, and chords inferred from a single
/// line are circular for this purpose — a detector free to choose any chord
/// will choose one that contains the dissonance. Supplying the harmony is what
/// makes the question ("is that F a suspension?") answerable at all.
fn waltz_harmony() -> Vec<ChordEvent> {
    [
        ("C", 0),
        ("C", 3),
        ("F", 6),
        ("F", 9),
        ("C", 12),
        ("G", 15),
        ("C", 18),
        ("C", 21),
    ]
    .iter()
    .enumerate()
    .map(|(i, (sym, at))| {
        let mut e = ChordEvent::new(
            i as u32,
            music_domain::symbol::parse(sym).expect("a chord symbol"),
            BeatTime::from_quarters(*at),
            BeatTime::from_quarters(3),
        );
        e.original_symbol = Some((*sym).to_string());
        e
    })
    .collect()
}

#[test]
fn the_waltz_fixture_yields_suspensions() {
    let f = fixture("melodies/waltz_suspensions");
    let ns = f.note_set();
    let profile = kb().resolve_profile("common_practice").expect("a profile");
    let map = classify_ncts(&ns, &waltz_harmony(), &f.time_map(), kb(), &profile);
    let kinds: Vec<NctKind> = map.values().flatten().map(|h| h.kind).collect();
    assert!(
        kinds.contains(&NctKind::Suspension),
        "a fixture built on prepared dissonance produced {kinds:?}"
    );
}

#[test]
fn the_waltz_suspensions_are_prepared_accented_and_resolve_down() {
    let f = fixture("melodies/waltz_suspensions");
    let ns = f.note_set();
    let profile = kb().resolve_profile("common_practice").expect("a profile");
    let map = classify_ncts(&ns, &waltz_harmony(), &f.time_map(), kb(), &profile);
    for (id, hyps) in &map {
        if !hyps.iter().any(|h| h.kind == NctKind::Suspension) {
            continue;
        }
        let i = ns.notes.iter().position(|n| n.id == *id).expect("the note");
        let here = &ns.notes[i];
        assert_eq!(
            ns.notes[i - 1].midi,
            here.midi,
            "note {id} is called a suspension but was not prepared"
        );
        assert!(
            f.time_map().metric_weight(here.onset) >= 0.5,
            "note {id} is called a suspension on a weak beat"
        );
        assert!(
            ns.notes[i + 1].midi < here.midi,
            "note {id} is called a suspension but does not fall"
        );
    }
}

#[test]
fn the_waltz_fixture_distinguishes_suspension_from_accented_passing_tone() {
    let f = fixture("melodies/waltz_suspensions");
    let ns = f.note_set();
    let profile = kb().resolve_profile("common_practice").expect("a profile");
    let map = classify_ncts(&ns, &waltz_harmony(), &f.time_map(), kb(), &profile);
    // The F on the second beat of bar one is stepped into, not prepared, so it
    // is not a suspension however accented it is.
    let unprepared = ns
        .notes
        .iter()
        .find(|n| n.onset == BeatTime::ONE)
        .expect("the second note");
    let kinds: Vec<NctKind> = map[&unprepared.id].iter().map(|h| h.kind).collect();
    assert!(
        !kinds.contains(&NctKind::Suspension),
        "an unprepared dissonance was called a suspension: {kinds:?}"
    );
}

#[test]
fn every_melody_note_of_every_fixture_gets_a_reading() {
    for f in fixtures() {
        let notes = analysis_notes(&f);
        if notes.notes.is_empty() {
            continue;
        }
        let p = AnalyzeParams {
            extraction: if has_own_notes(&f) {
                ExtractionRequest::auto()
            } else {
                ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony)
            },
            ..defaults()
        };
        let a = analyze(kb(), &f.id, &notes, &p).expect("analysis");
        for n in &a.extraction.melody.notes {
            assert!(!a.nct(n.id).is_empty(), "{} note {}", f.id, n.id);
        }
        for v in a.ncts.values() {
            assert!(v.len() <= DEFAULT_MAX_HYPOTHESES);
            for h in v {
                assert!(h.confidence > 0.0 && h.confidence <= 1.0);
                assert!(!h.rationale.is_empty());
            }
        }
    }
}

#[test]
fn the_chromatic_fixture_produces_chromatic_readings() {
    let a = run("melodies/chromatic_descent", &defaults());
    let kinds: Vec<NctKind> = a.ncts.values().flatten().map(|h| h.kind).collect();
    assert!(
        kinds.iter().any(|k| matches!(
            k,
            NctKind::ChromaticApproach | NctKind::Passing | NctKind::Decorative
        )),
        "{kinds:?}"
    );
}

#[test]
fn strictness_narrows_the_readings() {
    let loose = run(
        "melodies/waltz_suspensions",
        &AnalyzeParams {
            strictness: Strictness::Exploratory,
            ..defaults()
        },
    );
    let tight = run(
        "melodies/waltz_suspensions",
        &AnalyzeParams {
            strictness: Strictness::CommonPracticeStrict,
            ..defaults()
        },
    );
    let loose_max = loose.ncts.values().map(Vec::len).max().unwrap_or(0);
    let tight_max = tight.ncts.values().map(Vec::len).max().unwrap_or(0);
    assert!(tight_max <= loose_max);
    assert_eq!(tight_max, 1);
    assert!(loose.key.candidates.len() > tight.key.candidates.len());
    assert!(loose.salience.structural.len() >= tight.salience.structural.len());
}

// ---------------------------------------------------------------------------
// chord detection
// ---------------------------------------------------------------------------

#[test]
fn the_ii_v_i_progression_is_detected_back() {
    let f = fixture("progressions/ii_v_i_c_major");
    let notes = analysis_notes(&f);
    let a = analyze(
        kb(),
        "ii-v-i",
        &notes,
        &AnalyzeParams {
            extraction: ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony),
            grid: GridMode::Bars(1.0),
            ..defaults()
        },
    )
    .expect("analysis");
    let symbols: Vec<String> = a
        .detected_chords
        .iter()
        .map(|c| c.spec.render_ascii())
        .collect();
    assert!(symbols.contains(&"Dm7".to_string()), "{symbols:?}");
    assert!(symbols.contains(&"G7".to_string()), "{symbols:?}");
    assert!(symbols.contains(&"Cmaj7".to_string()), "{symbols:?}");
}

#[test]
fn the_twelve_bar_blues_is_detected_as_dominant_sevenths() {
    let f = fixture("progressions/twelve_bar_blues_c");
    let notes = analysis_notes(&f);
    let a = analyze(
        kb(),
        "blues",
        &notes,
        &AnalyzeParams {
            extraction: ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony),
            grid: GridMode::Bars(1.0),
            profile_id: "blues".to_string(),
            ..defaults()
        },
    )
    .expect("analysis");
    assert_eq!(a.detected_chords.len(), 12);
    assert!(
        a.detected_chords
            .iter()
            .all(|c| c.spec.is_dominant_family()),
        "{:?}",
        a.detected_chords
            .iter()
            .map(|c| c.spec.render_ascii())
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_planing_loop_keeps_its_major_triads() {
    let f = fixture("progressions/modal_planing_loop");
    let notes = analysis_notes(&f);
    let a = analyze(
        kb(),
        "planing",
        &notes,
        &AnalyzeParams {
            extraction: ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony),
            grid: GridMode::Bars(1.0),
            profile_id: "modal_ambient".to_string(),
            ..defaults()
        },
    )
    .expect("analysis");
    assert_eq!(a.detected_chords.len(), 4);
    for c in &a.detected_chords {
        assert!(
            c.spec.is_major_family(),
            "{} is not a major triad",
            c.spec.render_ascii()
        );
    }
}

#[test]
fn the_dominant_wrap_loop_ends_on_the_dominant() {
    let f = fixture("loops/dominant_wrap");
    let notes = analysis_notes(&f);
    let a = analyze(
        kb(),
        "wrap",
        &notes,
        &AnalyzeParams {
            extraction: ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony),
            grid: GridMode::Bars(1.0),
            loop_span: loop_span(&f),
            ..defaults()
        },
    )
    .expect("analysis");
    let last = a.detected_chords.last().expect("a chord");
    assert_eq!(last.spec.render_ascii(), "G7");
    assert!(last.spec.is_dominant_family());
}

#[test]
fn detected_chords_carry_their_key_context_and_confidence() {
    let f = fixture("progressions/ii_v_i_c_major");
    let notes = analysis_notes(&f);
    let a = analyze(
        kb(),
        "ctx",
        &notes,
        &AnalyzeParams {
            extraction: ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony),
            grid: GridMode::Bars(1.0),
            ..defaults()
        },
    )
    .expect("analysis");
    for c in &a.detected_chords {
        assert_eq!(c.inference_source, "detected");
        assert!(c.confidence > 0.0);
        assert!(c.local_tonic.is_some());
    }
}

// ---------------------------------------------------------------------------
// loop observations
// ---------------------------------------------------------------------------

#[test]
fn the_hanging_note_fixture_reports_its_hanging_note() {
    let f = fixture("loops/pickup_and_hanging_note");
    let a = analyze(
        kb(),
        "hanging",
        &f.note_set(),
        &AnalyzeParams {
            loop_span: loop_span(&f),
            ..defaults()
        },
    )
    .expect("analysis");
    assert!(a.has_warning(HANGING_NOTE));
    assert!(a.has_warning(PICKUP_BEFORE_LOOP));
}

#[test]
fn no_loop_span_produces_no_loop_observations() {
    let a = run("loops/pickup_and_hanging_note", &defaults());
    assert!(a.loop_observations.is_empty());
}

// ---------------------------------------------------------------------------
// determinism
// ---------------------------------------------------------------------------

#[test]
fn analyses_are_byte_identical_across_runs() {
    for f in fixtures() {
        let notes = analysis_notes(&f);
        let p = AnalyzeParams {
            extraction: if has_own_notes(&f) {
                ExtractionRequest::auto()
            } else {
                ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony)
            },
            loop_span: loop_span(&f),
            ..defaults()
        };
        let a = analyze(kb(), &f.id, &notes, &p).expect("analysis");
        let b = analyze(kb(), &f.id, &notes, &p).expect("analysis");
        assert_eq!(a.id, b.id, "{}", f.id);
        assert_eq!(
            a.to_json().to_canonical_string(),
            b.to_json().to_canonical_string(),
            "{}",
            f.id
        );
    }
}

#[test]
fn the_analysis_id_depends_only_on_the_snapshot_and_the_parameters() {
    let f = fixture("melodies/eight_bar_c_major");
    let notes = f.note_set();
    let a = analyze(kb(), "snap", &notes, &defaults()).expect("analysis");
    let same = analyze(kb(), "snap", &notes, &defaults()).expect("analysis");
    let other_snapshot = analyze(kb(), "snap-2", &notes, &defaults()).expect("analysis");
    let other_params = analyze(
        kb(),
        "snap",
        &notes,
        &AnalyzeParams {
            strictness: Strictness::Conservative,
            ..defaults()
        },
    )
    .expect("analysis");
    assert_eq!(a.id, same.id);
    assert_ne!(a.id, other_snapshot.id);
    assert_ne!(a.id, other_params.id);
}

#[test]
fn note_order_in_the_source_does_not_change_the_analysis() {
    let f = fixture("melodies/two_voice_polyphony");
    let forward = f.note_set();
    let mut shuffled = forward.clone();
    shuffled.notes.reverse();
    let shuffled = NoteSet::sorted(shuffled.notes, shuffled.time_map);
    let a = analyze(kb(), "order", &forward, &defaults()).expect("analysis");
    let b = analyze(kb(), "order", &shuffled, &defaults()).expect("analysis");
    assert_eq!(
        a.to_json().to_canonical_string(),
        b.to_json().to_canonical_string()
    );
}

#[test]
fn the_summary_view_agrees_with_the_full_document() {
    for f in fixtures() {
        let notes = analysis_notes(&f);
        let p = AnalyzeParams {
            extraction: if has_own_notes(&f) {
                ExtractionRequest::auto()
            } else {
                ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony)
            },
            ..defaults()
        };
        let a = analyze(kb(), &f.id, &notes, &p).expect("analysis");
        let s = a.summary_json();
        assert_eq!(
            s.get("id").and_then(qjson::Json::as_str),
            Some(a.id.as_str())
        );
        assert_eq!(
            s.get("slot_count").and_then(qjson::Json::as_i64),
            Some(a.grid.slots.len() as i64)
        );
        assert_eq!(
            s.get("detected_chord_count").and_then(qjson::Json::as_i64),
            Some(a.detected_chords.len() as i64)
        );
    }
}
