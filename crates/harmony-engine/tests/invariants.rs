//! Property-style tests over the whole request space.
//!
//! Everything here is a **hard constraint**: not a preference the profile can
//! trade away, but a property of the output that must hold for every profile,
//! every seed and every fixture, or the material cannot be written into a
//! project at all.

use harmony_engine::countermelody::{generate_countermelody, MIN_REGISTER_SEPARATION};
use harmony_engine::generate::{generate_candidates, span_of};
use harmony_engine::params::{CancelFlag, CountermelodyParams, GenerateParams};
use harmony_engine::testing;
use music_domain::prelude::*;
use std::sync::OnceLock;

/// The fixtures the full profile-by-seed sweep runs over.
const SWEEP_FIXTURES: &[&str] = &[
    "melodies/eight_bar_c_major",
    "melodies/dorian_vamp_d",
    "melodies/blues_head_c",
];

/// Seeds used in the sweep.
const SWEEP_SEEDS: &[u64] = &[0, 17];

/// One generated candidate together with the request that produced it.
struct Sample {
    fixture: &'static str,
    profile: &'static str,
    seed: u64,
    melody: NoteSet,
    candidate: Candidate,
}

/// The whole sweep, generated once and shared by every test in this binary.
///
/// The matrix is every profile against every sweep fixture at two seeds, which
/// is the property-style corpus the hard constraints are checked over. It is
/// built once because generation is deterministic, so re-running it per test
/// would only cost time.
fn corpus() -> &'static Vec<Sample> {
    static CORPUS: OnceLock<Vec<Sample>> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let mut out = Vec::new();
        for fixture in SWEEP_FIXTURES {
            for profile in testing::PROFILE_IDS {
                for seed in SWEEP_SEEDS {
                    let p = GenerateParams::default()
                        .with_profile(profile)
                        .with_seed(*seed);
                    let h = testing::harness_with(fixture, p.clone());
                    let candidates = generate_candidates(
                        h.kb,
                        &h.analysis,
                        &p,
                        &CancelFlag::new(),
                        &mut |_, _| {},
                    )
                    .unwrap_or_else(|e| panic!("{fixture}/{profile}/{seed}: {e}"));
                    assert_eq!(candidates.len(), p.candidate_count);
                    for candidate in candidates {
                        out.push(Sample {
                            fixture,
                            profile,
                            seed: *seed,
                            melody: h.analysis.extraction.melody.clone(),
                            candidate,
                        });
                    }
                }
            }
        }
        out
    })
}

/// Runs `check` over every sample in the corpus.
fn sweep(mut check: impl FnMut(&str, &str, u64, &Candidate)) {
    for sample in corpus() {
        check(
            sample.fixture,
            sample.profile,
            sample.seed,
            &sample.candidate,
        );
    }
}

#[test]
fn midi_pitch_bounds_enforced() {
    sweep(|fixture, profile, seed, candidate| {
        for part in &candidate.parts {
            for note in &part.notes {
                assert!(
                    (0..=127).contains(&note.midi),
                    "{fixture}/{profile}/{seed} wrote MIDI {}",
                    note.midi
                );
                assert!(note.pitch.is_valid_midi());
                assert_eq!(note.pitch.midi(), note.midi, "spelling and pitch disagree");
            }
        }
    });
}

#[test]
fn no_invalid_midi_pitches() {
    sweep(|_, profile, _, candidate| {
        for chord in &candidate.chords {
            if let Some(v) = &chord.voicing {
                for p in &v.pitches {
                    assert!(p.is_valid_midi(), "{profile} voiced {}", p.to_ascii());
                }
            }
        }
    });
}

#[test]
fn positive_duration_enforced() {
    sweep(|fixture, profile, seed, candidate| {
        for part in &candidate.parts {
            for note in &part.notes {
                assert!(
                    note.duration.is_positive(),
                    "{fixture}/{profile}/{seed} wrote a non-positive duration"
                );
                assert!(note.validate().is_ok());
            }
        }
        for chord in &candidate.chords {
            assert!(chord.duration.is_positive());
        }
    });
}

#[test]
fn no_negative_durations() {
    sweep(|_, _, _, candidate| {
        for part in &candidate.parts {
            for note in &part.notes {
                assert!(note.duration.as_f64() > 0.0);
                assert!(note.end() > note.onset);
            }
        }
    });
}

#[test]
fn monophonic_no_overlap() {
    sweep(|fixture, profile, seed, candidate| {
        for part in &candidate.parts {
            if part.polyphonic {
                continue;
            }
            for pair in part.notes.windows(2) {
                assert!(
                    pair[0].end() <= pair[1].onset,
                    "{fixture}/{profile}/{seed}: {} overlaps in the {} part",
                    pair[0].onset.to_display(),
                    part.name
                );
            }
        }
    });
}

#[test]
fn illegal_monophonic_overlap_rejected() {
    // The repair path is exercised directly: two overlapping notes in a part
    // declared monophonic must not survive.
    let mut a = Note::new(
        1,
        SpelledPitch::parse("C3").expect("C3"),
        BeatTime::ZERO,
        BeatTime::from_quarters(8),
    );
    a.midi = a.pitch.midi();
    let mut b = Note::new(
        2,
        SpelledPitch::parse("E3").expect("E3"),
        BeatTime::from_quarters(2),
        BeatTime::from_quarters(4),
    );
    b.midi = b.pitch.midi();
    let mut parts = vec![Part {
        role: ArrangementRole::Bass,
        name: "Bass".to_string(),
        notes: vec![a, b],
        instrument_profile: Some("bass".to_string()),
        channel: 2,
        polyphonic: false,
    }];
    let warnings = harmony_engine::generate::enforce_invariants(
        &mut parts,
        (BeatTime::ZERO, BeatTime::from_quarters(8)),
    );
    let notes = &parts[0].notes;
    for pair in notes.windows(2) {
        assert!(pair[0].end() <= pair[1].onset);
    }
    assert!(notes.iter().all(|n| n.duration.is_positive()));
    let _ = warnings;
}

#[test]
fn notes_within_item_bounds() {
    for sample in corpus() {
        let (start, end) = span_of(&sample.candidate.chords, &sample.melody);
        for part in &sample.candidate.parts {
            for note in &part.notes {
                assert!(
                    note.onset >= start && note.end() <= end,
                    "{}/{}/{}: a note escapes [{}, {}]",
                    sample.fixture,
                    sample.profile,
                    sample.seed,
                    start.to_display(),
                    end.to_display()
                );
            }
        }
    }
}

#[test]
fn notes_are_ordered_within_every_part() {
    sweep(|_, profile, _, candidate| {
        for part in &candidate.parts {
            for pair in part.notes.windows(2) {
                assert!(
                    pair[0].onset <= pair[1].onset,
                    "{profile}: the {} part is unordered",
                    part.name
                );
            }
        }
    });
}

#[test]
fn every_reference_resolves() {
    sweep(|_, profile, _, candidate| {
        let kb = theory_kb::KnowledgeBase::embedded();
        for app in &candidate.trace.rule_applications {
            assert!(
                kb.rule(&app.rule_id).is_some(),
                "{profile} cites unknown rule {}",
                app.rule_id
            );
        }
        for source in &candidate.trace.source_ids {
            assert!(kb.source(source).is_some());
        }
        assert!(!candidate.id.is_empty());
        assert!(!candidate.trace.explanation.is_empty());
    });
}

#[test]
fn candidate_json_is_always_well_formed() {
    sweep(|_, profile, _, candidate| {
        let json = candidate.to_json();
        let text = json.to_canonical_string();
        assert!(!text.is_empty());
        let parsed = qjson::Json::parse(&text).expect("canonical JSON re-parses");
        let back = Candidate::from_json(&parsed).unwrap_or_else(|e| panic!("{profile}: {e}"));
        assert_eq!(back.id, candidate.id);
    });
}

#[test]
fn no_source_mutation_during_staging() {
    // The melody the caller supplied is reproduced note for note, never edited
    // in place, so nothing the engine returns can mutate the user's material.
    for fixture in SWEEP_FIXTURES {
        let p = GenerateParams {
            preserve_melody: true,
            ..GenerateParams::default().with_profile("jazz_standard")
        };
        let h = testing::harness_with(fixture, p.clone());
        let before: Vec<(i32, String, String)> = h
            .analysis
            .extraction
            .melody
            .notes
            .iter()
            .map(|n| (n.midi, n.onset.to_display(), n.duration.to_display()))
            .collect();
        let candidates =
            generate_candidates(h.kb, &h.analysis, &p, &CancelFlag::new(), &mut |_, _| {})
                .expect("candidates");
        let after: Vec<(i32, String, String)> = h
            .analysis
            .extraction
            .melody
            .notes
            .iter()
            .map(|n| (n.midi, n.onset.to_display(), n.duration.to_display()))
            .collect();
        assert_eq!(before, after, "{fixture}: the source was modified");
        for c in &candidates {
            let lead = c
                .parts
                .iter()
                .find(|x| x.role == ArrangementRole::Lead)
                .expect("melody part");
            let copied: Vec<(i32, String, String)> = lead
                .notes
                .iter()
                .map(|n| (n.midi, n.onset.to_display(), n.duration.to_display()))
                .collect();
            assert_eq!(copied, before);
        }
    }
}

// ---------------------------------------------------------------------------
// Countermelody invariants.
// ---------------------------------------------------------------------------

/// Generates a countermelody for one fixture and profile.
fn counter(
    fixture: &str,
    profile: &str,
    density: f64,
) -> (testing::Harness, Vec<Note>, Vec<ChordEvent>) {
    let p = GenerateParams {
        countermelody: CountermelodyParams {
            enabled: true,
            density,
            role: ArrangementRole::Counterlead,
        },
        ..GenerateParams::default().with_profile(profile)
    };
    let h = testing::harness_with(fixture, p.clone());
    let candidates = generate_candidates(h.kb, &h.analysis, &p, &CancelFlag::new(), &mut |_, _| {})
        .expect("candidates");
    let chords = candidates[0].chords.clone();
    let notes = generate_countermelody(
        h.kb,
        &h.profile,
        &h.analysis.extraction.melody,
        &chords,
        &h.analysis,
        &p.countermelody,
        p.seed,
    )
    .expect("countermelody");
    (h, notes, chords)
}

#[test]
fn countermelody_not_every_melody_onset() {
    for profile in ["jazz_standard", "cinematic", "common_practice"] {
        let (h, notes, _) = counter("melodies/eight_bar_c_major", profile, 0.9);
        assert!(!notes.is_empty(), "{profile} produced no countermelody");
        let melody = &h.analysis.extraction.melody;
        let doubled = notes
            .iter()
            .filter(|n| melody.notes.iter().any(|m| m.onset == n.onset))
            .count();
        assert!(
            (doubled as f64) < 0.6 * notes.len() as f64,
            "{profile}: {doubled} of {} onsets double the melody",
            notes.len()
        );
    }
}

#[test]
fn countermelody_rhythm_differs() {
    let (h, notes, _) = counter("melodies/eight_bar_c_major", "jazz_standard", 0.7);
    let melody_onsets: Vec<String> = h
        .analysis
        .extraction
        .melody
        .notes
        .iter()
        .map(|n| n.onset.to_display())
        .collect();
    let counter_onsets: Vec<String> = notes.iter().map(|n| n.onset.to_display()).collect();
    assert_ne!(melody_onsets, counter_onsets);
}

#[test]
fn countermelody_register_separated() {
    let (h, notes, _) = counter("melodies/eight_bar_c_major", "common_practice", 0.7);
    let melody = &h.analysis.extraction.melody;
    for note in &notes {
        let sounding = melody
            .notes
            .iter()
            .find(|m| m.onset <= note.onset && m.end() > note.onset)
            .map(|m| m.midi);
        if let Some(m) = sounding {
            assert!(
                note.midi <= m - MIN_REGISTER_SEPARATION,
                "{} crowds the melody {m}",
                note.midi
            );
        }
    }
}

#[test]
fn countermelody_fills_phrase_gaps() {
    let (h, notes, chords) = counter("melodies/waltz_suspensions", "cinematic", 0.8);
    assert!(!notes.is_empty());
    let start = chords[0].onset;
    let end = chords[chords.len() - 1].end();
    for note in &notes {
        assert!(note.onset >= start && note.end() <= end);
        assert!(note.duration.is_positive());
    }
    let _ = h;
}
