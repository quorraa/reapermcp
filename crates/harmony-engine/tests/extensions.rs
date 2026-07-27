//! The named cases from brief §28, "Extensions".
//!
//! Extensions are register- and context-sensitive rather than banned, so most
//! of these tests are about a penalty existing, being soft, and disappearing
//! when the exception that justifies it is present.

use harmony_engine::candidates::{build_slot_options, ChordOption, STRATEGY_ALT_DOMINANT};
use harmony_engine::factbuild::omission_changes_identity;
use harmony_engine::params::{GenerateParams, VoicingParams};
use harmony_engine::testing::{self, Harness};
use harmony_engine::voiceleading::{audit_with, AuditOptions, VoiceLeadingReport};
use harmony_engine::voicing::voice_progression;
use music_domain::prelude::*;
use theory_kb::{KnowledgeBase, ResolvedProfile};

/// Builds a voicing from spelled pitch names.
fn voicing(pitches: &[&str]) -> Voicing {
    let ps: Vec<SpelledPitch> = pitches
        .iter()
        .map(|p| SpelledPitch::parse(p).expect("pitch"))
        .collect();
    let mut v = Voicing::new(ps, VoicingFamily::Close);
    v.voices = (0..v.pitches.len()).map(|i| VoiceId(i as u16)).collect();
    v
}

/// A chord event for one symbol.
fn chord(sym: &str) -> ChordEvent {
    ChordEvent::new(
        0,
        symbol::parse(sym).expect("symbol"),
        BeatTime::ZERO,
        BeatTime::from_quarters(4),
    )
}

/// A melody sounding one pitch for the whole chord.
fn melody_with(pitch: &str) -> NoteSet {
    let p = SpelledPitch::parse(pitch).expect("pitch");
    let mut note = Note::new(1, p, BeatTime::ZERO, BeatTime::from_quarters(4));
    note.midi = p.midi();
    NoteSet::sorted(
        vec![note],
        TimeMap::constant(120.0, TimeSignature::new(4, 4)),
    )
}

/// Runs the voicing audit for one chord under one profile.
fn audit(
    profile: &str,
    sym: &str,
    pitches: &[&str],
    opts: AuditOptions,
) -> (ResolvedProfile, VoiceLeadingReport) {
    let kb = KnowledgeBase::embedded();
    let prof = kb.resolve_profile(profile).expect("profile");
    let report = audit_with(kb, &prof, &[voicing(pitches)], &[chord(sym)], &opts);
    (prof, report)
}

/// True when the named rule was applied with a real effect.
fn applied(report: &VoiceLeadingReport, rule: &str) -> bool {
    report
        .rule_applications
        .iter()
        .any(|a| a.rule_id == rule && a.status == RuleStatus::Applied && a.score_delta < 0.0)
}

/// True when the named rule was set aside by an exception.
fn bypassed(report: &VoiceLeadingReport, rule: &str) -> bool {
    report
        .rule_applications
        .iter()
        .any(|a| a.rule_id == rule && a.status == RuleStatus::Bypassed)
}

const NATURAL_11: &str = "extensions.major_natural_11_close_register";

// ---------------------------------------------------------------------------
// The natural eleventh over a major third.
// ---------------------------------------------------------------------------

#[test]
fn major11_close_register_penalty() {
    let (_, report) = audit(
        "common_practice",
        "Cadd11",
        &["C4", "E4", "F4", "G4"],
        AuditOptions::default(),
    );
    assert!(
        applied(&report, NATURAL_11),
        "an eleventh a semitone above the third must cost something"
    );
    let rule = KnowledgeBase::embedded().rule(NATURAL_11).expect("rule");
    assert!(
        !rule.kind.rejects(),
        "the penalty must be soft, never a rejection"
    );
    assert_eq!(rule.kind.id(), "style_sensitive_preference");
}

#[test]
fn major11_omit3_exception() {
    // The same chord with no third sounding: nothing is being clashed against.
    let (_, report) = audit(
        "common_practice",
        "Cadd11",
        &["C4", "G4", "F5"],
        AuditOptions::default(),
    );
    assert!(
        bypassed(&report, NATURAL_11) || !applied(&report, NATURAL_11),
        "omitting the third removes the clash"
    );
}

#[test]
fn major11_melody_exception() {
    let (_, report) = audit(
        "common_practice",
        "Cadd11",
        &["C4", "E4", "G4"],
        AuditOptions {
            melody: Some(melody_with("F5")),
            ..AuditOptions::default()
        },
    );
    assert!(
        bypassed(&report, NATURAL_11) || !applied(&report, NATURAL_11),
        "an eleventh in the melody is the line, not a clash"
    );
}

#[test]
fn major11_penalty_is_profile_configurable() {
    let kb = KnowledgeBase::embedded();
    let rule = kb.rule(NATURAL_11).expect("rule");
    assert!(rule.profiles.iter().any(|p| p == "common_practice"));
    assert!(
        !kb.resolve_profile("modal_ambient")
            .expect("profile")
            .applies_to(rule),
        "a modal profile does not police the natural eleventh"
    );
    let (_, modal) = audit(
        "modal_ambient",
        "Cadd11",
        &["C4", "E4", "F4", "G4"],
        AuditOptions::default(),
    );
    assert!(!applied(&modal, NATURAL_11));
}

#[test]
fn major11_penalty_bypassed_in_quartal_context() {
    let (_, report) = audit(
        "jazz_standard",
        "Cadd11",
        &["C4", "E4", "F4", "G4"],
        AuditOptions {
            planing: true,
            ..AuditOptions::default()
        },
    );
    assert!(bypassed(&report, NATURAL_11) || !applied(&report, NATURAL_11));
}

#[test]
fn sharp11_distinct_from_natural11() {
    let natural = symbol::parse("C7(11)")
        .or_else(|_| symbol::parse("C11"))
        .expect("C11");
    let sharp = symbol::parse("C7#11").expect("C7#11");
    assert!(natural.pitch_classes().contains(&5), "natural 11 is F");
    assert!(sharp.pitch_classes().contains(&6), "sharp 11 is F#");
    assert_ne!(natural.pitch_classes(), sharp.pitch_classes());
    assert!(sharp
        .alterations
        .iter()
        .any(|d| d.number == 11 && d.alter == 1));
    assert!(natural.alterations.iter().all(|d| d.number != 11));
}

#[test]
fn sharp11_preferred_on_major7() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("extensions.sharp_eleven_preferred_over_natural_eleven")
        .expect("rule");
    assert!(rule.effect.score_delta > 0.0);
    assert!(rule.exceptions.iter().any(|e| e == "melody_is_11"));
    assert!(kb.chord_quality("major7_sharp11").is_some());
}

// ---------------------------------------------------------------------------
// Altered dominants.
// ---------------------------------------------------------------------------

/// Every alt realisation the engine reaches under a jazz profile.
fn alt_options(h: &Harness) -> Vec<ChordOption> {
    let ctx = h.context();
    let mut out = Vec::new();
    for slot in &ctx.analysis.grid.slots {
        out.extend(
            build_slot_options(&ctx, slot)
                .into_iter()
                .filter(|o| o.source_strategy == STRATEGY_ALT_DOMINANT),
        );
    }
    out
}

#[test]
fn alt_generates_explicit_alternatives() {
    let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
    let options = alt_options(&h);
    assert!(!options.is_empty(), "jazz reaches altered dominants");
    let mut sets: Vec<Vec<i32>> = options.iter().map(|o| o.pitch_classes()).collect();
    sets.sort();
    sets.dedup();
    assert!(
        sets.len() >= 2,
        "alt must expand to several explicit tension sets, saw {}",
        sets.len()
    );
    for option in &options {
        assert!(
            !option.spec.alt_dominant,
            "a realised alt chord names its tensions instead of staying a family"
        );
        assert!(!option.spec.alterations.is_empty());
    }
}

#[test]
fn alt_not_fixed_pitch_set() {
    // The alternatives the engine will consider are enumerated, and they are
    // genuinely different sonorities rather than one scale wearing six names.
    let sets = harmony_engine::candidates::ALT_TENSION_SETS;
    assert_eq!(sets.len(), 6);
    let mut realised: Vec<Vec<i32>> = Vec::new();
    for set in sets {
        let mut spec = ChordSpec {
            root: (Letter::C, Accidental::NATURAL),
            triad: TriadQuality::Major,
            seventh: SeventhQuality::Minor,
            alterations: set.iter().filter_map(|d| ChordDegree::parse(d)).collect(),
            ..ChordSpec::default()
        };
        spec.normalize();
        realised.push(spec.pitch_classes());
    }
    let mut distinct = realised.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        realised.len(),
        "every alt realisation must be a different chord"
    );

    // And the engine actually reaches more than one of them.
    let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
    let mut reached: Vec<Vec<i32>> = alt_options(&h)
        .iter()
        .map(|o| {
            let mut d: Vec<i32> = o
                .spec
                .alterations
                .iter()
                .map(|a| a.simple_semitones())
                .collect();
            d.sort_unstable();
            d
        })
        .collect();
    reached.sort();
    reached.dedup();
    assert!(
        reached.len() >= 2,
        "only {} tension set(s) reached",
        reached.len()
    );
}

#[test]
fn altered_tension_connects_to_destination() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("extensions.alteration_requires_destination")
        .expect("rule");
    assert!(rule.effect.score_delta < 0.0);
    assert!(rule.exceptions.iter().any(|e| e == "target_chord_follows"));
    assert!(rule
        .exceptions
        .iter()
        .any(|e| e == "altered_tone_resolves_by_step"));
}

#[test]
fn unresolved_alteration_penalised() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("extensions.alteration_requires_destination")
        .expect("rule");
    assert_eq!(rule.kind.id(), "strong_theory_principle");
    assert!(
        !rule.kind.rejects(),
        "it is a strong principle, not a filter"
    );
    assert!(
        kb.resolve_profile("jazz_standard")
            .expect("profile")
            .rule_multiplier("extensions.alteration_requires_destination")
            > 1.0
    );
}

#[test]
fn b9_to_minor_target() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("extensions.flat_nine_prefers_minor_destination")
        .expect("rule");
    assert!(rule.effect.score_delta > 0.0);
    assert!(rule.conditions.iter().any(|c| c == "key_is_minor"));
}

#[test]
fn sharp9_needs_register_separation() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("extensions.sharp_nine_separated_from_major_third")
        .expect("rule");
    assert!(rule.effect.score_delta < 0.0);
    assert!(rule.conditions.iter().any(|c| c == "register_is_low"));
}

#[test]
fn sharp9_on_top_is_fine() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("extensions.sharp_nine_separated_from_major_third")
        .expect("rule");
    assert!(rule.exceptions.iter().any(|e| e == "register_is_high"));
    assert!(rule
        .exceptions
        .iter()
        .any(|e| e == "voices_are_widely_spaced"));
}

#[test]
fn b13_omits_natural_fifth() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("extensions.flat_thirteen_conflicts_with_natural_fifth")
        .expect("rule");
    assert!(rule.exceptions.iter().any(|e| e == "fifth_is_omitted"));
    let (_, report) = audit(
        "jazz_standard",
        "C7b13",
        &["C3", "E3", "G3", "Ab3", "Bb3"],
        AuditOptions::default(),
    );
    assert!(
        applied(
            &report,
            "extensions.flat_thirteen_conflicts_with_natural_fifth"
        ),
        "a b13 next to a natural fifth costs something"
    );
}

#[test]
fn major_seventh_not_on_dominant() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("extensions.major_seventh_not_over_dominant_function")
        .expect("rule");
    assert!(rule.effect.score_delta < 0.0);
    assert!(rule.conditions.iter().any(|c| c == "function_is_dominant"));
    assert!(rule.conditions.iter().any(|c| c == "seventh_is_present"));
}

// ---------------------------------------------------------------------------
// Minor-family colour and modal context.
// ---------------------------------------------------------------------------

#[test]
fn minor11_no_penalty() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("extensions.minor_eleven_is_consonant")
        .expect("rule");
    assert!(
        rule.effect.score_delta > 0.0,
        "the eleventh over a minor third is a colour, not a clash"
    );
    let minor = symbol::parse("Cm11").expect("Cm11");
    assert!(minor.pitch_classes().contains(&5));
    assert!(harmony_engine::voiceleading::third_to_eleventh(&minor, &[48, 51, 53]).is_none());
}

#[test]
fn minor11_default_modal_color() {
    let h = testing::harness("melodies/dorian_vamp_d", "modal_ambient");
    let ctx = h.context();
    assert_eq!(ctx.key.scale_id, "dorian");
    // The eleventh of the tonic minor chord is a scale degree of the mode.
    let eleventh = (ctx.key.tonic_pc() + 5).rem_euclid(12);
    assert!(ctx.key.contains_pc(eleventh));
}

#[test]
fn minor13_requires_dorian() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("extensions.natural_thirteen_needs_dorian_context")
        .expect("rule");
    assert!(rule.effect.score_delta < 0.0);
    assert!(rule
        .conditions
        .iter()
        .any(|c| c == "modal_center_is_active"));
    assert!(rule.exceptions.iter().any(|e| e == "melody_is_extension"));
}

#[test]
fn minor13_uses_modal_context() {
    let kb = KnowledgeBase::embedded();
    let dorian = harmony_engine::keyctx::KeyContext::new(
        kb,
        (Letter::D, Accidental::NATURAL),
        "dorian",
        0.9,
        true,
    )
    .expect("D dorian");
    let aeolian = harmony_engine::keyctx::KeyContext::new(
        kb,
        (Letter::D, Accidental::NATURAL),
        "aeolian",
        0.9,
        true,
    )
    .expect("D aeolian");
    let natural_thirteen = (dorian.tonic_pc() + 9).rem_euclid(12);
    assert!(
        dorian.contains_pc(natural_thirteen),
        "the natural 13 belongs to dorian"
    );
    assert!(
        !aeolian.contains_pc(natural_thirteen),
        "it does not belong to aeolian"
    );
}

#[test]
fn avoid_tone_soft_penalty() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("extensions.avoid_tone_is_profile_sensitive")
        .expect("rule");
    assert_eq!(rule.kind.id(), "style_sensitive_preference");
    assert!(!rule.kind.rejects());
    assert!(rule.effect.score_delta > -2.0, "the penalty is small");
}

#[test]
fn avoid_tone_bypassed_modal() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("extensions.avoid_tone_is_profile_sensitive")
        .expect("rule");
    assert!(rule
        .exceptions
        .iter()
        .any(|e| e == "modal_center_is_active"));
    assert!(rule
        .exceptions
        .iter()
        .any(|e| e == "melody_note_is_structural"));
}

// ---------------------------------------------------------------------------
// Omission never rewrites identity.
// ---------------------------------------------------------------------------

#[test]
fn omission_preserves_identity() {
    let spec = symbol::parse("C13").expect("C13");
    // The fifth is gone; the third and seventh are not.
    let sounding = vec![
        "1".to_string(),
        "3".to_string(),
        "b7".to_string(),
        "13".to_string(),
    ];
    assert!(!omission_changes_identity(&spec, &sounding));
    assert_eq!(spec.render_ascii(), "C13", "the symbol is untouched");
}

#[test]
fn root_omission_keeps_identity() {
    let spec = symbol::parse("Cmaj9").expect("Cmaj9");
    let sounding = vec![
        "3".to_string(),
        "5".to_string(),
        "7".to_string(),
        "9".to_string(),
    ];
    assert!(
        !omission_changes_identity(&spec, &sounding),
        "a rootless voicing is still the same chord"
    );
}

#[test]
fn c13_identity_survives_omissions() {
    let spec = symbol::parse("C13").expect("C13");
    assert!(spec.pitch_classes().contains(&4), "the third is declared");
    assert!(
        spec.pitch_classes().contains(&10),
        "the seventh is declared"
    );
    // Losing everything that carries the quality does change what it is.
    let gutted = vec!["1".to_string(), "5".to_string()];
    assert!(omission_changes_identity(&spec, &gutted));
}

#[test]
fn fifth_omitted_first() {
    let kb = KnowledgeBase::embedded();
    let rule = kb.rule("extensions.fifth_is_omitted_first").expect("rule");
    assert!(rule.effect.score_delta > 0.0);
    let templates: Vec<&str> = kb
        .voicing_templates()
        .iter()
        .filter(|t| t.omissible_degrees.iter().any(|d| d == "5"))
        .map(|t| t.id.as_str())
        .collect();
    assert!(
        templates.len() > 5,
        "the fifth is the standard omission across the template set"
    );
}

#[test]
fn altered_fifth_not_omissible() {
    let kb = KnowledgeBase::embedded();
    let rule = kb.rule("extensions.fifth_is_omitted_first").expect("rule");
    assert!(
        rule.exceptions
            .iter()
            .any(|e| e == "chord_has_altered_tones"),
        "an altered fifth is the colour, not the filler"
    );
}

#[test]
fn guide_tones_retained_first() {
    let kb = KnowledgeBase::embedded();
    let prof = kb.resolve_profile("jazz_standard").expect("profile");
    let events = vec![chord("C13"), {
        let mut e = chord("Fmaj9");
        e.onset = BeatTime::from_quarters(4);
        e
    }];
    let vp = VoicingParams {
        voice_count: 3,
        ..VoicingParams::default()
    };
    let voicings = voice_progression(kb, &prof, &events, None, &vp).expect("voicings");
    for (event, v) in events.iter().zip(&voicings) {
        let sounding: Vec<i32> = v.pitches.iter().map(|p| p.pitch_class()).collect();
        for degree in event.spec.guide_tones() {
            let pc = (event.spec.root_pc() + degree.simple_semitones()).rem_euclid(12);
            assert!(
                sounding.contains(&pc),
                "{} lost guide tone {}",
                event.spec.render_ascii(),
                degree.to_string()
            );
        }
    }
}

#[test]
fn quality_survives_reduction() {
    let kb = KnowledgeBase::embedded();
    let prof = kb.resolve_profile("jazz_standard").expect("profile");
    for symbol_text in ["Cmaj7", "Cm7", "C7", "Cm7b5", "Cdim7"] {
        let events = vec![chord(symbol_text)];
        let vp = VoicingParams {
            voice_count: 2,
            ..VoicingParams::default()
        };
        let voicings = voice_progression(kb, &prof, &events, None, &vp).expect("voicings");
        let sounding: Vec<i32> = voicings[0]
            .pitches
            .iter()
            .map(|p| p.pitch_class())
            .collect();
        let spec = &events[0].spec;
        let kept = spec
            .guide_tones()
            .iter()
            .filter(|d| sounding.contains(&(spec.root_pc() + d.simple_semitones()).rem_euclid(12)))
            .count();
        assert!(
            kept >= 1,
            "{symbol_text} reduced to two voices kept no quality tone"
        );
    }
}

#[test]
fn rootless_requires_bass() {
    let kb = KnowledgeBase::embedded();
    let rule = kb
        .rule("extensions.root_omitted_when_bass_supplies_it")
        .expect("rule");
    assert!(rule.conditions.iter().any(|c| c == "root_is_omitted"));
    assert!(rule.conditions.iter().any(|c| c == "bass_supplies_root"));
    assert!(rule.effect.score_delta > 0.0);
}

// ---------------------------------------------------------------------------
// Symbol semantics that omissions and extensions must not blur.
// ---------------------------------------------------------------------------

#[test]
fn sus_replaces_third() {
    let sus = symbol::parse("Csus4").expect("Csus4");
    assert!(sus.is_suspended());
    assert!(!sus.pitch_classes().contains(&4), "no major third");
    assert!(sus.pitch_classes().contains(&5), "the fourth is there");
    assert_eq!(sus.render_ascii(), "Csus4");
}

#[test]
fn sus4_not_eleven() {
    let sus = symbol::parse("Csus4").expect("Csus4");
    let eleven = symbol::parse("C11").expect("C11");
    assert!(sus.is_suspended());
    assert!(!eleven.is_suspended(), "an eleventh chord keeps its third");
    assert!(eleven.pitch_classes().contains(&4));
    assert_ne!(sus.pitch_classes(), eleven.pitch_classes());
}

#[test]
fn add4_retains_third() {
    let add4 = symbol::parse("Cadd4").expect("Cadd4");
    assert!(!add4.is_suspended());
    assert!(add4.pitch_classes().contains(&4), "the third stays");
    assert!(add4.pitch_classes().contains(&5), "the fourth is added");
}

#[test]
fn c6_not_c13() {
    let six = symbol::parse("C6").expect("C6");
    assert!(!six.seventh.is_present(), "a sixth implies no seventh");
    assert_eq!(six.pitch_classes(), vec![0, 4, 7, 9]);
    let thirteen = symbol::parse("C13").expect("C13");
    assert!(thirteen.seventh.is_present());
}

#[test]
fn cadd9_not_c9() {
    let add9 = symbol::parse("Cadd9").expect("Cadd9");
    assert!(!add9.seventh.is_present());
    let nine = symbol::parse("C9").expect("C9");
    assert!(nine.seventh.is_present());
    assert_ne!(add9.pitch_classes(), nine.pitch_classes());
}

#[test]
fn extension_density_control_effective() {
    let mean_tones = |density: f64| -> f64 {
        let p = GenerateParams {
            extension_density: density,
            ..GenerateParams::default().with_profile("jazz_standard")
        };
        let h = testing::harness_with("melodies/eight_bar_c_major", p);
        let ctx = h.context();
        let pool = build_slot_options(&ctx, &ctx.analysis.grid.slots[0]);
        pool.iter()
            .take(6)
            .map(|o| o.spec.chord_tones().len() as f64)
            .sum::<f64>()
            / 6.0
    };
    let sparse = mean_tones(0.0);
    let dense = mean_tones(1.0);
    assert!(
        dense > sparse,
        "extension density must change the vocabulary: {dense} vs {sparse}"
    );
}
