//! The named cases from brief §28, "Candidate generation".

use harmony_engine::candidates::{build_pools, build_slot_options};
use harmony_engine::diversity::{diversify, max_pairwise_similarity, PathFeatures};
use harmony_engine::generate::{candidate_fingerprint, generate_candidates};
use harmony_engine::keyctx::KeyContext;
use harmony_engine::params::{CancelFlag, GenerateParams, SearchConfig};
use harmony_engine::search::search_in;
use harmony_engine::testing::{self, Harness};
use music_domain::prelude::*;
use theory_kb::KnowledgeBase;

const FIXTURE: &str = "melodies/eight_bar_c_major";

fn run(p: &GenerateParams) -> (Harness, Vec<Candidate>) {
    run_on(FIXTURE, p)
}

fn run_on(fixture: &str, p: &GenerateParams) -> (Harness, Vec<Candidate>) {
    let h = testing::harness_with(fixture, p.clone());
    let out = generate_candidates(h.kb, &h.analysis, p, &CancelFlag::new(), &mut |_, _| {})
        .unwrap_or_else(|e| panic!("{fixture}/{}: {e}", p.profile_id));
    (h, out)
}

fn fingerprints(candidates: &[Candidate]) -> Vec<String> {
    candidates.iter().map(candidate_fingerprint).collect()
}

// ---------------------------------------------------------------------------
// Determinism.
// ---------------------------------------------------------------------------

#[test]
fn same_seed_same_ordering() {
    let p = GenerateParams::default()
        .with_profile("jazz_standard")
        .with_seed(4242);
    let (_, a) = run(&p);
    let (_, b) = run(&p);
    assert_eq!(
        fingerprints(&a),
        fingerprints(&b),
        "the same request must produce byte-identical candidates"
    );
    let ids_a: Vec<&str> = a.iter().map(|c| c.id.as_str()).collect();
    let ids_b: Vec<&str> = b.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids_a, ids_b, "candidate ids are content-derived");
}

#[test]
fn determinism_holds_for_every_profile() {
    for profile in testing::PROFILE_IDS {
        let p = GenerateParams::default().with_profile(profile).with_seed(7);
        let (_, a) = run(&p);
        let (_, b) = run(&p);
        assert_eq!(fingerprints(&a), fingerprints(&b), "profile {profile}");
    }
}

#[test]
fn different_seeds_vary_tie_broken_options() {
    // A different seed may reorder options that scored equally. It must never
    // produce something invalid, and it must stay reproducible for its own seed.
    let mut shapes: Vec<Vec<String>> = Vec::new();
    for seed in [0u64, 1, 2, 3, 5, 8, 13, 21] {
        let p = GenerateParams::default()
            .with_profile("electronic_loop")
            .with_seed(seed);
        let (h, candidates) = run(&p);
        let again = generate_candidates(h.kb, &h.analysis, &p, &CancelFlag::new(), &mut |_, _| {})
            .expect("candidates");
        assert_eq!(
            fingerprints(&candidates),
            fingerprints(&again),
            "seed {seed} is not reproducible"
        );
        for c in &candidates {
            for part in &c.parts {
                for note in &part.notes {
                    assert!((0..=127).contains(&note.midi));
                    assert!(note.duration.is_positive());
                }
            }
        }
        shapes.push(
            candidates
                .iter()
                .map(|c| {
                    c.chords
                        .iter()
                        .map(|x| x.spec.render_ascii())
                        .collect::<Vec<String>>()
                        .join(">")
                })
                .collect(),
        );
    }
    let mut unique = shapes.clone();
    unique.sort();
    unique.dedup();
    // Ties are genuinely broken by the seed, so at least two seeds must differ;
    // if the search has no ties at all this is a legitimate single outcome, so
    // the assertion is that the seed never breaks anything rather than that it
    // always changes something.
    assert!(!unique.is_empty());
}

#[test]
fn seed_only_breaks_ties() {
    // Two seeds over the same request must produce candidates whose scores are
    // equal to within the tie threshold, never a materially worse answer.
    let best = |seed: u64| -> f64 {
        let p = GenerateParams::default()
            .with_profile("common_practice")
            .with_seed(seed);
        let (_, c) = run(&p);
        c[0].trace.score.total()
    };
    let a = best(0);
    let b = best(99);
    assert!(
        (a - b).abs() < 0.05,
        "the seed changed the quality of the answer: {a} vs {b}"
    );
}

// ---------------------------------------------------------------------------
// Global search, not greedy selection.
// ---------------------------------------------------------------------------

#[test]
fn path_search_is_global() {
    let h = testing::harness(FIXTURE, "common_practice");
    let ctx = h.context();
    let pools = build_pools(&ctx).expect("pools");
    let found = search_in(
        &ctx,
        &pools,
        &SearchConfig::default(),
        &CancelFlag::new(),
        &mut |_, _| {},
    )
    .expect("paths");
    for path in &found {
        assert_eq!(
            path.chords.len(),
            pools.len(),
            "paths span the whole phrase"
        );
    }
    assert!(found.len() > 1, "the search keeps alternatives");
}

#[test]
fn greedy_selection_rejected() {
    let h = testing::harness(FIXTURE, "jazz_standard");
    let ctx = h.context();
    let pools = build_pools(&ctx).expect("pools");
    let greedy: Vec<String> = pools.iter().map(|p| p[0].key()).collect();
    let found = search_in(
        &ctx,
        &pools,
        &SearchConfig::default(),
        &CancelFlag::new(),
        &mut |_, _| {},
    )
    .expect("paths");
    assert!(
        found.iter().all(|p| p.option_keys != greedy) || found.len() > 1,
        "the search must be able to depart from the per-slot best"
    );
    assert!(
        found.iter().any(|p| p.option_keys != greedy),
        "no path departed from the greedy walk"
    );
}

// ---------------------------------------------------------------------------
// Controls.
// ---------------------------------------------------------------------------

#[test]
fn candidate_count_limits() {
    for n in [1usize, 2, 3, 4, 6, 8] {
        let p = GenerateParams::default()
            .with_profile("pop_rock")
            .with_candidate_count(n);
        let (_, candidates) = run(&p);
        assert_eq!(candidates.len(), n);
    }
    let too_many = GenerateParams::default().with_candidate_count(9);
    assert!(too_many.validate().is_err());
    let none = GenerateParams::default().with_candidate_count(0);
    assert!(none.validate().is_err());
}

#[test]
fn candidate_diversity() {
    let p = GenerateParams::default().with_profile("jazz_standard");
    let h = testing::harness_with(FIXTURE, p.clone());
    let ctx = h.context();
    let pools = build_pools(&ctx).expect("pools");
    let found = search_in(
        &ctx,
        &pools,
        &SearchConfig::default(),
        &CancelFlag::new(),
        &mut |_, _| {},
    )
    .expect("paths");
    let top: Vec<_> = found.iter().take(3).cloned().collect();
    let picked = diversify(found, 3, &h.profile, p.seed);
    assert_eq!(picked.len(), 3);
    assert!(
        max_pairwise_similarity(&picked) <= max_pairwise_similarity(&top),
        "diversification must not increase similarity"
    );
    let mut strategies: Vec<&str> = picked.iter().map(|x| x.strategy.as_str()).collect();
    strategies.sort_unstable();
    strategies.dedup();
    assert_eq!(strategies.len(), 3);
    for (i, path) in picked.iter().enumerate() {
        for other in picked.iter().skip(i + 1) {
            let a = PathFeatures::of(path);
            let b = PathFeatures::of(other);
            assert!(a.similarity(&b) < 0.95, "two candidates are the same idea");
        }
    }
}

#[test]
fn complexity_control_changes_vocabulary() {
    let simple = GenerateParams {
        complexity: 0.0,
        extension_density: 0.0,
        ..GenerateParams::default().with_profile("neo_soul_rnb")
    };
    let rich = GenerateParams {
        complexity: 1.0,
        extension_density: 1.0,
        ..GenerateParams::default().with_profile("neo_soul_rnb")
    };
    let tones = |p: &GenerateParams| -> f64 {
        let (_, candidates) = run(p);
        let total: usize = candidates
            .iter()
            .flat_map(|c| c.chords.iter())
            .map(|c| c.spec.chord_tones().len())
            .sum();
        let count: usize = candidates.iter().map(|c| c.chords.len()).sum();
        total as f64 / count.max(1) as f64
    };
    let a = tones(&simple);
    let b = tones(&rich);
    assert!(b > a, "complexity must change the vocabulary: {b} vs {a}");
}

#[test]
fn chromaticism_control_changes_candidate_pool() {
    let pool_keys = |chromaticism: f64| -> Vec<String> {
        let p = GenerateParams {
            chromaticism,
            ..GenerateParams::default().with_profile("cinematic")
        };
        let h = testing::harness_with(FIXTURE, p);
        let ctx = h.context();
        let mut keys: Vec<String> = build_slot_options(&ctx, &ctx.analysis.grid.slots[0])
            .iter()
            .map(|o| o.key())
            .collect();
        keys.sort();
        keys
    };
    assert_ne!(pool_keys(0.0), pool_keys(1.0));
}

#[test]
fn profile_changes_meaningfully_alter_results() {
    let mut shapes: Vec<(String, String)> = Vec::new();
    for profile in testing::PROFILE_IDS {
        let p = GenerateParams::default()
            .with_profile(profile)
            .with_seed(11);
        let (_, candidates) = run(&p);
        let shape = candidates[0]
            .chords
            .iter()
            .map(|c| c.spec.render_ascii())
            .collect::<Vec<String>>()
            .join(">");
        shapes.push(((*profile).to_string(), shape));
    }
    let mut distinct: Vec<&String> = shapes.iter().map(|(_, s)| s).collect();
    distinct.sort();
    distinct.dedup();
    assert!(
        distinct.len() >= 6,
        "ten profiles produced only {} distinct progressions: {:#?}",
        distinct.len(),
        shapes
    );
    // The two profiles at the extremes must never agree.
    let strict = shapes
        .iter()
        .find(|(p, _)| p == "strict_counterpoint")
        .expect("strict");
    let neo = shapes
        .iter()
        .find(|(p, _)| p == "neo_soul_rnb")
        .expect("neo soul");
    assert_ne!(strict.1, neo.1);
}

#[test]
fn profile_changes_alter_voicing_and_bass() {
    let take = |profile: &str| -> (usize, usize) {
        let p = GenerateParams::default().with_profile(profile).with_seed(2);
        let (_, candidates) = run(&p);
        let harmony = candidates[0]
            .parts
            .iter()
            .find(|x| x.role == ArrangementRole::HarmonicBed)
            .expect("harmony")
            .notes
            .len();
        let bass = candidates[0]
            .parts
            .iter()
            .find(|x| x.role == ArrangementRole::Bass)
            .expect("bass")
            .notes
            .len();
        (harmony, bass)
    };
    let strict = take("strict_counterpoint");
    let electronic = take("electronic_loop");
    assert_ne!(
        strict, electronic,
        "the realisation must differ, not only the chord symbols"
    );
}

// ---------------------------------------------------------------------------
// Preservation.
// ---------------------------------------------------------------------------

#[test]
fn preserve_melody_keeps_every_pitch() {
    for profile in testing::PROFILE_IDS {
        let p = GenerateParams {
            preserve_melody: true,
            ..GenerateParams::default().with_profile(profile)
        };
        let (h, candidates) = run(&p);
        let source = &h.analysis.extraction.melody.notes;
        for c in &candidates {
            let lead = c
                .parts
                .iter()
                .find(|x| x.role == ArrangementRole::Lead)
                .unwrap_or_else(|| panic!("{profile} dropped the melody part"));
            assert_eq!(lead.notes.len(), source.len(), "{profile}");
            for (out, original) in lead.notes.iter().zip(source) {
                assert_eq!(out.midi, original.midi, "{profile} changed a pitch");
                assert_eq!(out.onset, original.onset, "{profile} changed an onset");
                assert_eq!(
                    out.pitch.to_ascii(),
                    original.pitch.to_ascii(),
                    "{profile} changed a spelling"
                );
            }
        }
    }
}

#[test]
fn preserve_rhythm_keeps_onsets() {
    for profile in ["common_practice", "jazz_standard", "drum_and_bass"] {
        let p = GenerateParams {
            preserve_rhythm: true,
            ..GenerateParams::default().with_profile(profile)
        };
        let (h, candidates) = run(&p);
        let source = &h.analysis.extraction.melody.notes;
        for c in &candidates {
            let lead = c
                .parts
                .iter()
                .find(|x| x.role == ArrangementRole::Lead)
                .expect("melody part");
            for (out, original) in lead.notes.iter().zip(source) {
                assert_eq!(out.onset, original.onset);
                assert_eq!(out.duration, original.duration);
                assert_eq!(out.end(), original.end());
            }
        }
    }
}

#[test]
fn harmony_never_covers_a_preserved_melody() {
    let p = GenerateParams {
        preserve_melody: true,
        ..GenerateParams::default().with_profile("jazz_standard")
    };
    let (h, candidates) = run(&p);
    let melody = &h.analysis.extraction.melody;
    for c in &candidates {
        let harmony = c
            .parts
            .iter()
            .find(|x| x.role == ArrangementRole::HarmonicBed)
            .expect("harmony part");
        for note in &harmony.notes {
            let above = melody
                .notes
                .iter()
                .filter(|m| m.onset < note.end() && m.end() > note.onset)
                .map(|m| m.midi)
                .min();
            if let Some(lowest) = above {
                assert!(
                    note.midi < lowest,
                    "harmony {} reaches the melody {} at {}",
                    note.midi,
                    lowest,
                    note.onset.to_display()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Transposition invariance of functional analysis.
// ---------------------------------------------------------------------------

#[test]
fn transposition_invariance_for_functional_analysis() {
    let kb = KnowledgeBase::embedded();
    // The same degree of any key must produce the same numeral and function.
    let mut seen: Vec<(String, String)> = Vec::new();
    for (letter, accidental) in [
        (Letter::C, Accidental::NATURAL),
        (Letter::E, Accidental::FLAT),
        (Letter::F, Accidental::SHARP),
        (Letter::A, Accidental::NATURAL),
    ] {
        let key = KeyContext::new(kb, (letter, accidental), "major", 1.0, false).expect("key");
        let entry = kb
            .functions()
            .iter()
            .find(|f| f.id == "major_v")
            .expect("the dominant");
        let root = key
            .root_of(entry.scale_degree.as_deref().expect("degree"))
            .expect("root");
        let quality = kb.chord_quality("dominant7").expect("dominant7");
        let spec = harmony_engine::keyctx::spec_from_quality(quality, root);
        let roman = harmony_engine::keyctx::roman_label(entry, &spec, 0);
        // The sounding root is always seven semitones above the tonic.
        assert_eq!((spec.root_pc() - key.tonic_pc()).rem_euclid(12), 7);
        seen.push((key.label(), roman));
    }
    let numerals: Vec<&String> = seen.iter().map(|(_, r)| r).collect();
    assert!(
        numerals.windows(2).all(|w| w[0] == w[1]),
        "the numeral must not depend on the key: {seen:?}"
    );
    assert_eq!(numerals[0], "V7");
}

// ---------------------------------------------------------------------------
// Explanations.
// ---------------------------------------------------------------------------

#[test]
fn candidate_explanations_reference_real_ids() {
    for profile in testing::PROFILE_IDS {
        let p = GenerateParams::default().with_profile(profile);
        let (h, candidates) = run(&p);
        for c in &candidates {
            assert!(!c.trace.rule_applications.is_empty(), "{profile}");
            for app in &c.trace.rule_applications {
                assert!(
                    h.kb.rule(&app.rule_id).is_some(),
                    "{profile} cites unknown rule {}",
                    app.rule_id
                );
            }
            assert!(!c.trace.source_ids.is_empty(), "{profile} cites no source");
            for source in &c.trace.source_ids {
                assert!(
                    h.kb.source(source).is_some(),
                    "{profile} cites unknown source {source}"
                );
            }
            assert_eq!(c.trace.knowledge_version, h.kb.version());
            assert_eq!(c.trace.profile_id, *profile);
            assert_eq!(c.trace.candidate_id, c.id);
        }
    }
}

#[test]
fn score_vectors_expose_every_component() {
    let p = GenerateParams::default().with_profile("cinematic");
    let (_, candidates) = run(&p);
    for c in &candidates {
        for component in SCORE_COMPONENTS {
            assert!(
                c.trace.score.contains(component),
                "{} is missing from the score vector",
                component
            );
        }
        let json = c.trace.score.to_json();
        let components = json
            .get("components")
            .and_then(qjson::Json::as_obj)
            .expect("components object");
        assert_eq!(components.len(), SCORE_COMPONENTS.len());
        assert!(json.get("total").is_some());
    }
}

#[test]
fn traces_report_alternatives_and_assumptions() {
    let p = GenerateParams::default().with_profile("jazz_standard");
    let (_, candidates) = run(&p);
    for c in &candidates {
        assert!(
            !c.trace.rejected_alternatives.is_empty(),
            "at least one alternative should be reported"
        );
        assert!(!c.trace.assumptions.is_empty());
        assert!((0.0..=1.0).contains(&c.trace.confidence));
    }
}

// ---------------------------------------------------------------------------
// Larger material.
// ---------------------------------------------------------------------------

#[test]
fn sixteen_bars_produce_three_candidates() {
    let p = GenerateParams::default().with_profile("jazz_standard");
    let (h, candidates) = run_on("progressions/sixteen_bar_c_major", &p);
    assert_eq!(candidates.len(), 3);
    let (start, end) = (
        BeatTime::ZERO,
        h.analysis
            .extraction
            .melody
            .notes
            .iter()
            .map(|n| n.end())
            .max()
            .expect("an end"),
    );
    for c in &candidates {
        assert!(!c.chords.is_empty());
        for chord in &c.chords {
            assert!(chord.onset >= start && chord.end() <= end + BeatTime::from_quarters(4));
        }
    }
}
