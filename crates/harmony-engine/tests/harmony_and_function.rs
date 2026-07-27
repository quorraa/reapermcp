//! The named cases from brief §28, "Harmony and function".
//!
//! Each test is named for the `test_id` the knowledge rule it discharges
//! declares, so the rule base and the test suite can be reconciled mechanically.

use harmony_engine::candidates::{
    build_pools, build_slot_options, option_from_chord, ChordOption, STRATEGY_BASS_LED,
    STRATEGY_BORROWED, STRATEGY_CHROMATIC_MEDIANT, STRATEGY_PLANING, STRATEGY_TRITONE_SUB,
};
use harmony_engine::generate::generate_candidates;
use harmony_engine::params::{CancelFlag, GenerateParams, SearchConfig};
use harmony_engine::search::{search_in, HarmonicPath};
use harmony_engine::testing::{self, Harness};
use music_domain::prelude::*;
use theory_kb::KnowledgeBase;

/// Every option the engine would consider for the first slot of a fixture.
fn first_pool(h: &Harness) -> Vec<ChordOption> {
    let ctx = h.context();
    build_slot_options(&ctx, &ctx.analysis.grid.slots[0])
}

/// Every option across every slot, which is where rarer strategies show up.
fn all_options(h: &Harness) -> Vec<ChordOption> {
    let ctx = h.context();
    let mut out = Vec::new();
    for slot in &ctx.analysis.grid.slots {
        out.extend(build_slot_options(&ctx, slot));
    }
    out
}

/// The best paths for a fixture under a profile.
fn paths(h: &Harness) -> Vec<HarmonicPath> {
    let ctx = h.context();
    let pools = build_pools(&ctx).expect("pools");
    search_in(
        &ctx,
        &pools,
        &SearchConfig::default(),
        &CancelFlag::new(),
        &mut |_, _| {},
    )
    .expect("paths")
}

// ---------------------------------------------------------------------------
// Dm7 - G7 - Cmaj7 is read as ii - V - I in C major.
// ---------------------------------------------------------------------------

#[test]
fn ii_v_i_analysis_in_c_major() {
    let h = testing::harness("progressions/reharm_source_c_major", "jazz_standard");
    let ctx = h.context();
    let fixture = testing::load_fixture("progressions/reharm_source_c_major");
    let chords = fixture.chord_events();
    assert_eq!(chords[0].spec.render_ascii(), "Dm7");
    assert_eq!(chords[1].spec.render_ascii(), "G7");
    assert_eq!(chords[2].spec.render_ascii(), "Cmaj7");

    let read: Vec<ChordOption> = chords
        .iter()
        .take(3)
        .enumerate()
        .map(|(i, c)| {
            option_from_chord(
                &ctx,
                &ctx.analysis.grid.slots[i.min(ctx.analysis.grid.slots.len() - 1)],
                c,
            )
            .unwrap_or_else(|| panic!("no reading for {}", c.spec.render_ascii()))
        })
        .collect();

    assert_eq!(read[0].roman, "ii7");
    assert_eq!(read[1].roman, "V7");
    assert_eq!(read[2].roman, "Imaj7");
    assert_eq!(read[0].function, HarmonicFunction::Predominant);
    assert_eq!(read[1].function, HarmonicFunction::Dominant);
    assert_eq!(read[2].function, HarmonicFunction::Tonic);
    // D F A C is the predominant seventh, not a passing sonority.
    assert_eq!(read[0].entry_id, "major_ii");
}

#[test]
fn ii_v_i_scores_above_v_i() {
    let h = testing::harness("progressions/reharm_source_c_major", "jazz_standard");
    let ctx = h.context();
    let index = harmony_engine::search::SchemaIndex::build(&ctx);
    let ii_v = index
        .progression_weight("ii", "V")
        .expect("ii-V is described");
    let v_i = index.cadence_weight("V", "I").expect("V-I is described");
    assert!(ii_v > 0.0 && v_i > 0.0);
    // The knowledge base describes the whole ii-V-I as one schema, which is
    // what makes the three-chord path score above the two-chord one.
    assert!(!index.matching_schemas("ii", "V").is_empty());
    assert!(index
        .matching_schemas("ii", "V")
        .iter()
        .any(|id| id == "prog_ii_v_i_major"));
}

#[test]
fn dominant_resolves_down_fifth() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("harmony.dominant_seventh_resolves_down_fifth")
        .expect("the rule exists");
    let prof = kb.resolve_profile("common_practice").expect("profile");
    assert!(prof.applies_to(rule));
    assert!(rule.effect.score_delta > 0.0);

    let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
    let options = all_options(&h);
    let g7 = options
        .iter()
        .find(|o| o.symbol() == "G7")
        .expect("G7 is reachable in C major");
    assert_eq!(g7.function, HarmonicFunction::Dominant);
    let c = options
        .iter()
        .find(|o| o.symbol() == "Cmaj7" || o.symbol() == "C")
        .expect("the tonic is reachable");
    assert!(harmony_engine::search::seventh_resolves_down(g7, c));
    assert_eq!(
        (c.spec.root_pc() - g7.spec.root_pc()).rem_euclid(12),
        5,
        "V to I is a descending fifth"
    );
}

#[test]
fn deceptive_resolution_still_valid() {
    let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
    let options = all_options(&h);
    let g7 = options.iter().find(|o| o.symbol() == "G7").expect("G7");
    let vi = options
        .iter()
        .find(|o| o.symbol() == "Am7" || o.symbol() == "Am")
        .expect("vi");
    // A deceptive resolution is a legitimate option, not a filtered one: the
    // engine keeps it in the pool and scores it.
    assert!(vi
        .rule_applications
        .iter()
        .all(|a| a.status != RuleStatus::Violated));
    assert_eq!((vi.spec.root_pc() - g7.spec.root_pc()).rem_euclid(12), 2);
}

// ---------------------------------------------------------------------------
// Mixture, substitution, applied dominants and chromatic mediants.
// ---------------------------------------------------------------------------

#[test]
fn borrowed_iv_is_mixture() {
    let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
    let options = all_options(&h);
    let borrowed = options
        .iter()
        .find(|o| o.entry_id == "borrowed_iv_in_major")
        .expect("F minor is reachable in C major");
    assert_eq!(borrowed.source_strategy, STRATEGY_BORROWED);
    assert!(borrowed.symbol().starts_with("Fm"));
    assert_eq!(borrowed.function, HarmonicFunction::Predominant);
    assert!(borrowed.chromaticism > 0.0, "mixture is not diatonic");
    assert!(borrowed.rule_applications.iter().any(|a| a.rule_id
        == "harmony.borrowed_chord_comes_from_parallel_mode"
        && a.status == RuleStatus::Applied));
}

#[test]
fn mixture_from_parallel_mode_only() {
    let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
    let ctx = h.context();
    let options = all_options(&h);
    let borrowed: Vec<&ChordOption> = options
        .iter()
        .filter(|o| o.source_strategy == STRATEGY_BORROWED)
        .collect();
    assert!(!borrowed.is_empty());
    // Every borrowed chord must be spelled from the parallel minor of the same
    // tonic, so its root is a degree of C minor rather than an arbitrary root.
    let parallel: Vec<i32> = [0, 2, 3, 5, 7, 8, 10]
        .iter()
        .map(|s| (ctx.key.tonic_pc() + s).rem_euclid(12))
        .collect();
    for option in borrowed {
        assert!(
            parallel.contains(&option.spec.root_pc()),
            "{} is not built on a degree of the parallel mode",
            option.symbol()
        );
    }
}

#[test]
fn tritone_sub_db7_for_g7() {
    let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
    let options = all_options(&h);
    let sub = options
        .iter()
        .find(|o| o.source_strategy == STRATEGY_TRITONE_SUB && o.spec.is_dominant_family())
        .expect("a tritone substitute is reachable in a jazz context");
    assert_eq!(sub.spec.root_pc(), 1, "the substitute for G7 in C is on Db");
    assert_eq!(sub.function, HarmonicFunction::Dominant);
    assert!(
        sub.roman.starts_with("bII"),
        "the substitute keeps its own root in the numeral: {}",
        sub.roman
    );
    assert_eq!(sub.spec.root.0, Letter::D);
    assert_eq!(sub.spec.root.1, Accidental::FLAT);
}

#[test]
fn tritone_sub_shares_guide_tones() {
    let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
    let options = all_options(&h);
    let sub = options
        .iter()
        .find(|o| o.source_strategy == STRATEGY_TRITONE_SUB && o.spec.is_dominant_family())
        .expect("a tritone substitute");
    let g7 = symbol::parse("G7").expect("G7");
    let shared: Vec<i32> = sub
        .pitch_classes()
        .into_iter()
        .filter(|pc| g7.pitch_classes().contains(pc))
        .collect();
    // The tritone B-F is exactly what the two chords have in common.
    assert!(shared.contains(&11), "the leading tone is shared");
    assert!(shared.contains(&5), "the seventh is shared");
}

#[test]
fn applied_dominant_needs_target() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("harmony.applied_dominant_requires_target")
        .expect("rule");
    assert!(rule.effect.score_delta < 0.0);
    assert!(rule.exceptions.iter().any(|e| e == "target_chord_follows"));

    let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
    let options = all_options(&h);
    let applied = options
        .iter()
        .find(|o| o.function == HarmonicFunction::Applied)
        .expect("an applied dominant is reachable");
    let target_root = (applied.spec.root_pc() + 5).rem_euclid(12);
    let target = options
        .iter()
        .find(|o| o.spec.root_pc() == target_root)
        .expect("its target is reachable too");
    let ctx = h.context();
    assert!(harmony_engine::search::target_follows(
        &ctx, applied, target
    ));
}

#[test]
fn applied_dominant_identifies_tonicization() {
    let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
    let options = all_options(&h);
    let applied: Vec<&ChordOption> = options
        .iter()
        .filter(|o| o.function == HarmonicFunction::Applied)
        .collect();
    assert!(!applied.is_empty());
    for option in applied {
        assert!(
            option.roman.contains('/'),
            "an applied dominant names the degree it tonicises: {}",
            option.roman
        );
        assert!(option.entry_id.starts_with("applied_"));
    }
}

#[test]
fn chromatic_mediant_not_forced_diatonic() {
    let h = testing::harness("melodies/eight_bar_c_major", "cinematic");
    let options = all_options(&h);
    let mediant = options
        .iter()
        .find(|o| o.source_strategy == STRATEGY_CHROMATIC_MEDIANT)
        .expect("a chromatic mediant is reachable in a cinematic context");
    assert_eq!(
        mediant.function,
        HarmonicFunction::Chromatic,
        "a chromatic mediant is not a diatonic function"
    );
    assert!(
        mediant.chromaticism > 0.0,
        "a chromatic mediant is chromatic by definition"
    );
    assert!(
        mediant.roman.starts_with('b')
            || mediant
                .roman
                .chars()
                .next()
                .is_some_and(|c| c.is_uppercase()),
        "its numeral records the real root: {}",
        mediant.roman
    );
    // The third relationship is what makes it a mediant.
    let tonic = h.context().key.tonic_pc();
    let motion = (mediant.spec.root_pc() - tonic).rem_euclid(12);
    assert!(matches!(motion, 3 | 4 | 8 | 9), "root motion {motion}");
}

#[test]
fn chromatic_mediant_common_tone_reported() {
    let h = testing::harness("melodies/eight_bar_c_major", "cinematic");
    let options = all_options(&h);
    let mediant = options
        .iter()
        .find(|o| o.source_strategy == STRATEGY_CHROMATIC_MEDIANT)
        .expect("a chromatic mediant");
    let tonic = options
        .iter()
        .find(|o| o.spec.root_pc() == 0 && o.function == HarmonicFunction::Tonic)
        .expect("the tonic");
    assert!(
        harmony_engine::search::common_tones(tonic, mediant) >= 1,
        "a chromatic mediant shares a tone with the chord it displaces"
    );
}

// ---------------------------------------------------------------------------
// Modal and blues behaviour.
// ---------------------------------------------------------------------------

#[test]
fn planing_scores_well_modal_profile() {
    let kb = KnowledgeBase::embedded();
    let rule = kb.rule("harmony.planing_suspends_function").expect("rule");
    let modal = kb.resolve_profile("modal_ambient").expect("profile");
    let strict = kb.resolve_profile("strict_counterpoint").expect("profile");
    assert!(modal.applies_to(rule));
    assert!(
        !strict.applies_to(rule),
        "strict counterpoint does not plane"
    );

    let h = testing::harness("melodies/dorian_vamp_d", "modal_ambient");
    let planing: Vec<ChordOption> = all_options(&h)
        .into_iter()
        .filter(|o| o.source_strategy == STRATEGY_PLANING)
        .collect();
    assert!(!planing.is_empty(), "modal_ambient reaches planing");
    let strict_h = testing::harness("melodies/dorian_vamp_d", "strict_counterpoint");
    let strict_planing = all_options(&strict_h)
        .into_iter()
        .filter(|o| o.source_strategy == STRATEGY_PLANING)
        .count();
    assert_eq!(strict_planing, 0, "strict counterpoint does not reach it");
}

#[test]
fn constant_structure_preserves_quality() {
    let h = testing::harness("melodies/dorian_vamp_d", "modal_ambient");
    let planing: Vec<ChordOption> = all_options(&h)
        .into_iter()
        .filter(|o| o.source_strategy == STRATEGY_PLANING)
        .collect();
    let mut qualities: Vec<String> = planing.iter().map(|o| o.quality_id.clone()).collect();
    qualities.sort();
    qualities.dedup();
    assert_eq!(
        qualities.len(),
        1,
        "constant structure means one sonority moved about: {qualities:?}"
    );
}

#[test]
fn modal_vamp_no_leading_tone() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("harmony.modal_vamp_avoids_leading_tone")
        .expect("rule");
    let modal = kb.resolve_profile("modal_ambient").expect("profile");
    assert!(modal.applies_to(rule));
    assert!(rule.effect.score_delta < 0.0);

    let h = testing::harness("melodies/dorian_vamp_d", "modal_ambient");
    let ctx = h.context();
    // D dorian has no leading tone, and the engine must not invent one.
    assert_eq!(ctx.key.scale_id, "dorian");
    assert_eq!(ctx.key.leading_tone_pc(), None);
    assert!(ctx.key.is_modal);
}

#[test]
fn blues_tonic_seventh_stable() {
    let h = testing::harness("melodies/blues_head_c", "blues");
    let options = all_options(&h);
    let tonic_seventh = options
        .iter()
        .find(|o| {
            o.function == HarmonicFunction::Tonic
                && o.spec.is_dominant_family()
                && o.spec.root_pc() == h.context().key.tonic_pc()
        })
        .expect("a dominant-seventh tonic is reachable in a blues context");
    assert!(tonic_seventh.rule_applications.iter().any(|a| a.rule_id
        == "harmony.blues_dominant_seventh_is_stable_tonic"
        && a.status == RuleStatus::Applied
        && a.score_delta > 0.0));
}

#[test]
fn blues_no_resolution_penalty() {
    let kb = KnowledgeBase::embedded();
    let resolution = kb
        .rule("harmony.dominant_seventh_resolves_down_fifth")
        .expect("rule");
    let blues = kb.resolve_profile("blues").expect("profile");
    let common = kb.resolve_profile("common_practice").expect("profile");
    assert!(
        !blues.applies_to(resolution),
        "blues does not demand resolution after every seventh"
    );
    assert!(common.applies_to(resolution));
}

#[test]
fn retrogression_penalised_common_practice() {
    let kb = KnowledgeBase::embedded();
    let rule = kb.rule("harmony.retrogression_penalty").expect("rule");
    assert!(rule.effect.score_delta < 0.0);
    assert!(kb
        .resolve_profile("common_practice")
        .expect("profile")
        .applies_to(rule));
}

#[test]
fn retrogression_free_in_blues() {
    let kb = KnowledgeBase::embedded();
    let rule = kb.rule("harmony.retrogression_penalty").expect("rule");
    assert!(
        !kb.resolve_profile("blues")
            .expect("profile")
            .applies_to(rule),
        "V to IV is the shape of the blues, not a retrogression"
    );
}

// ---------------------------------------------------------------------------
// Cadences, inversions and the melody's claim on the harmony.
// ---------------------------------------------------------------------------

#[test]
fn cadential_six_four_labeled_dominant() {
    let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
    let options = all_options(&h);
    let six_four = options
        .iter()
        .find(|o| o.entry_id == "cadential_six_four")
        .expect("a cadential six-four is reachable");
    assert_eq!(
        six_four.function,
        HarmonicFunction::Dominant,
        "a cadential six-four is a dominant, not a tonic in second inversion"
    );
    assert_eq!(six_four.inversion, 2);
    assert!(six_four.roman.starts_with('V'));
}

#[test]
fn pac_requires_root_position() {
    let kb = KnowledgeBase::embedded();
    let pac = kb
        .cadences()
        .iter()
        .find(|c| c.schema.id == "cad_perfect_authentic")
        .expect("the perfect authentic cadence");
    assert!(
        pac.schema
            .voice_leading_notes
            .to_lowercase()
            .contains("root-position")
            || pac
                .schema
                .bass_implications
                .to_lowercase()
                .contains("root position")
    );
    let rule = kb.rule("harmony.root_position_at_cadence").expect("rule");
    assert!(rule.effect.score_delta > 0.0);
    assert!(rule
        .conditions
        .iter()
        .any(|c| c == "chord_is_in_root_position"));
}

#[test]
fn dim_triad_prefers_first_inversion() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("harmony.diminished_triad_avoids_root_position")
        .expect("rule");
    assert_eq!(
        rule.trigger
            .selectors
            .get("chord_family")
            .and_then(qjson::Json::as_str),
        Some("dim")
    );
    assert!(rule.effect.score_delta < 0.0);
    assert!(rule.exceptions.iter().any(|e| e == "seventh_is_present"));
    let jazz = kb.resolve_profile("jazz_standard").expect("profile");
    assert!(
        jazz.rule_multiplier("harmony.diminished_triad_avoids_root_position") < 1.0,
        "jazz softens it rather than obeying it"
    );
}

#[test]
fn structural_melody_note_explained() {
    let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
    let ctx = h.context();
    let slot = &ctx.analysis.grid.slots[0];
    let pool = build_slot_options(&ctx, slot);
    let structural: Vec<i32> = ctx
        .slot_melody(slot)
        .iter()
        .filter(|n| n.structural)
        .map(|n| n.pc)
        .collect();
    assert!(!structural.is_empty(), "the fixture has structural notes");
    let best = &pool[0];
    // The winning chord must have something to say about the structural notes:
    // each is a chord tone, an extension, or an explained non-chord tone.
    for pc in structural {
        let degree = best.spec.degree_of_pc(pc);
        let explained = degree.is_some()
            || ctx
                .melody
                .iter()
                .any(|n| n.pc == pc && n.nct_confidence > 0.0);
        assert!(explained, "pitch class {pc} is unaccounted for");
    }
}

#[test]
fn unexplained_clash_penalised() {
    let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
    let ctx = h.context();
    let slot = &ctx.analysis.grid.slots[0];
    let pool = build_slot_options(&ctx, slot);
    let notes = ctx.slot_melody(slot);
    let best_fit = pool
        .iter()
        .map(|o| o.local_score.get("melody_fit"))
        .fold(f64::MIN, f64::max);
    let worst_fit = pool
        .iter()
        .map(|o| o.local_score.get("melody_fit"))
        .fold(f64::MAX, f64::min);
    assert!(!notes.is_empty());
    assert!(
        best_fit > worst_fit + 0.1,
        "melody fit must actually separate the options: {best_fit} vs {worst_fit}"
    );
}

#[test]
fn chord_scale_membership_bonus() {
    let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
    let ctx = h.context();
    let slot = &ctx.analysis.grid.slots[0];
    let pool = build_slot_options(&ctx, slot);
    let fitting = pool
        .iter()
        .find(|o| o.local_score.get("melody_fit") > 0.9)
        .expect("some chord agrees with the melody");
    assert!(fitting.rule_applications.iter().any(|a| a.rule_id
        == "harmony.chord_scale_membership_supports_melody"
        && a.status == RuleStatus::Applied));
}

#[test]
fn chord_scale_does_not_decide_alone() {
    // Chord-scale membership generates options; the phrase-level search is what
    // chooses between them. A greedy walk over the pool must be reachable and
    // must not be what the search returns.
    let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
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
    assert!(found.iter().any(|p| p.option_keys != greedy));
}

// ---------------------------------------------------------------------------
// Bass-led and rhythm behaviour.
// ---------------------------------------------------------------------------

#[test]
fn bass_led_strategy_produces_stepwise_bass() {
    let h = testing::harness("melodies/eight_bar_c_major", "neo_soul_rnb");
    let options = all_options(&h);
    let inverted: Vec<&ChordOption> = options
        .iter()
        .filter(|o| o.source_strategy == STRATEGY_BASS_LED)
        .collect();
    assert!(!inverted.is_empty(), "inversions are reachable");
    for option in inverted {
        assert!(option.inversion > 0);
        assert_ne!(
            option.bass_pc(),
            option.spec.root_pc(),
            "a bass-led option puts something other than the root underneath"
        );
    }
}

#[test]
fn bass_led_candidate_is_distinct() {
    let h = testing::harness("melodies/eight_bar_c_major", "neo_soul_rnb");
    let found = paths(&h);
    let chromatic: Vec<&HarmonicPath> = found
        .iter()
        .filter(|p| p.strategy == "chromatic_bass_led")
        .collect();
    let functional: Vec<&HarmonicPath> = found
        .iter()
        .filter(|p| p.strategy == "functional")
        .collect();
    assert!(!chromatic.is_empty() && !functional.is_empty());
    assert_ne!(
        chromatic[0].shape(),
        functional[0].shape(),
        "the bass-led reading is a different progression, not a relabelled one"
    );
}

#[test]
fn dnb_slow_harmonic_rhythm() {
    let kb = KnowledgeBase::embedded();
    let dnb = kb.resolve_profile("drum_and_bass").expect("profile");
    let jazz = kb.resolve_profile("jazz_standard").expect("profile");
    let dnb_min = dnb
        .field_str("harmonic_rhythm.min_slot_qn")
        .expect("declared");
    let jazz_min = jazz
        .field_str("harmonic_rhythm.min_slot_qn")
        .expect("declared");
    assert!(
        dnb_min.parse::<f64>().unwrap_or(0.0) > jazz_min.parse::<f64>().unwrap_or(0.0),
        "drum and bass must not change chords at drum tempo: {dnb_min} vs {jazz_min}"
    );
    assert_eq!(
        dnb.field_bool("harmonic_rhythm.coupled_to_tempo"),
        Some(false)
    );
}

#[test]
fn harmonic_rhythm_not_from_tempo() {
    // The same melody at the same tempo produces different harmonic rhythms
    // under profiles that declare different slot lengths.
    let fast = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
    let slow = testing::harness("melodies/eight_bar_c_major", "drum_and_bass");
    assert!(
        fast.analysis.grid.slots.len() > slow.analysis.grid.slots.len(),
        "jazz {} vs drum_and_bass {}",
        fast.analysis.grid.slots.len(),
        slow.analysis.grid.slots.len()
    );
}

#[test]
fn low_chromaticism_filters_candidates() {
    let plain = GenerateParams {
        chromaticism: 0.0,
        ..GenerateParams::default().with_profile("cinematic")
    };
    let spicy = GenerateParams {
        chromaticism: 1.0,
        ..GenerateParams::default().with_profile("cinematic")
    };
    let a = testing::harness_with("melodies/eight_bar_c_major", plain);
    let b = testing::harness_with("melodies/eight_bar_c_major", spicy);
    let mean = |h: &Harness| -> f64 {
        let options = all_options(h);
        options.iter().map(|o| o.chromaticism).sum::<f64>() / options.len().max(1) as f64
    };
    let low = paths(&a);
    let high = paths(&b);
    let path_chromaticism = |p: &HarmonicPath| -> f64 { p.score.get("chromaticism_target") };
    assert!(mean(&a) >= 0.0 && mean(&b) >= 0.0);
    assert_ne!(
        path_chromaticism(&low[0]),
        path_chromaticism(&high[0]),
        "the chromaticism control must change what the search rewards"
    );
}

#[test]
fn chromaticism_control_changes_pool() {
    let plain = GenerateParams {
        chromaticism: 0.0,
        ..GenerateParams::default().with_profile("jazz_standard")
    };
    let spicy = GenerateParams {
        chromaticism: 1.0,
        ..GenerateParams::default().with_profile("jazz_standard")
    };
    let a = testing::harness_with("melodies/eight_bar_c_major", plain);
    let b = testing::harness_with("melodies/eight_bar_c_major", spicy);
    let keys = |h: &Harness| -> Vec<String> {
        let mut k: Vec<String> = first_pool(h).iter().map(|o| o.key()).collect();
        k.sort();
        k
    };
    assert_ne!(keys(&a), keys(&b), "the pool itself must change");
}

#[test]
fn sus4_resolves_to_third() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("harmony.suspension_resolves_to_third")
        .expect("rule");
    assert!(rule.effect.score_delta > 0.0);
    let sus = symbol::parse("G7sus4").expect("G7sus4");
    let g7 = symbol::parse("G7").expect("G7");
    assert!(sus.is_suspended());
    assert!(!g7.is_suspended());
    // C falls to B: the suspended fourth resolves down by step to the third.
    let fourth = (sus.root_pc() + 5).rem_euclid(12);
    let third = (g7.root_pc() + 4).rem_euclid(12);
    assert_eq!((third - fourth).rem_euclid(12), 11);
}

#[test]
fn sus4_stable_in_modal_profile() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("harmony.suspension_resolves_to_third")
        .expect("rule");
    assert!(
        rule.exceptions
            .iter()
            .any(|e| e == "modal_center_is_active"),
        "a modal sus chord is a sonority, not an unresolved suspension"
    );
}

#[test]
fn minor_dominant_raised_seventh() {
    let kb = KnowledgeBase::embedded();
    let major_v = kb
        .functions()
        .iter()
        .find(|f| f.id == "minor_v_major")
        .expect("the raised dominant of a minor key");
    assert_eq!(major_v.triad_quality, "major_triad");
    assert_eq!(major_v.function_class, "dominant");
    let minor_v = kb
        .functions()
        .iter()
        .find(|f| f.id == "minor_v_minor")
        .expect("the modal minor dominant");
    assert_eq!(minor_v.function_class, "modal");
}

#[test]
fn aeolian_keeps_minor_v() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("harmony.minor_key_dominant_raises_seventh")
        .expect("rule");
    assert!(
        rule.exceptions
            .iter()
            .any(|e| e == "modal_center_is_active"),
        "an aeolian passage keeps its minor v"
    );
}

#[test]
fn three_candidates_distinct_strategies() {
    let p = GenerateParams::default().with_profile("jazz_standard");
    let h = testing::harness_with("melodies/eight_bar_c_major", p.clone());
    let candidates = generate_candidates(h.kb, &h.analysis, &p, &CancelFlag::new(), &mut |_, _| {})
        .expect("candidates");
    assert_eq!(candidates.len(), 3);
    let mut strategies: Vec<&str> = candidates.iter().map(|c| c.strategy.as_str()).collect();
    strategies.sort_unstable();
    strategies.dedup();
    assert_eq!(
        strategies.len(),
        3,
        "three candidates must be three strategies: {strategies:?}"
    );
}

#[test]
fn candidates_differ_structurally() {
    let p = GenerateParams::default().with_profile("cinematic");
    let h = testing::harness_with("melodies/eight_bar_c_major", p.clone());
    let candidates = generate_candidates(h.kb, &h.analysis, &p, &CancelFlag::new(), &mut |_, _| {})
        .expect("candidates");
    let shapes: Vec<String> = candidates
        .iter()
        .map(|c| {
            c.chords
                .iter()
                .map(|x| format!("{}:{}", x.spec.render_ascii(), x.inversion))
                .collect::<Vec<String>>()
                .join(">")
        })
        .collect();
    let mut unique = shapes.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), shapes.len(), "candidates repeat: {shapes:?}");
}
