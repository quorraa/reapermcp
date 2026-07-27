//! The named cases from brief §28, "Voice leading".
//!
//! Voice leading is the clearest place where "correct" is style-dependent, so
//! most of these tests are paired: the same material audited under two profiles
//! must produce two different verdicts about the same observed fact.

use harmony_engine::params::VoicingParams;
use harmony_engine::voiceleading::{
    audit_voice_leading, audit_with, parallel_penalty_scale, AuditOptions, VoiceLeadingReport,
};
use harmony_engine::voicing::voice_progression;
use music_domain::prelude::*;
use theory_kb::{KnowledgeBase, ResolvedProfile};

fn kb() -> &'static KnowledgeBase {
    KnowledgeBase::embedded()
}

fn profile(id: &str) -> ResolvedProfile {
    kb().resolve_profile(id).expect("profile")
}

fn voicing(pitches: &[&str]) -> Voicing {
    let ps: Vec<SpelledPitch> = pitches
        .iter()
        .map(|p| SpelledPitch::parse(p).expect("pitch"))
        .collect();
    let mut v = Voicing::new(ps, VoicingFamily::Close);
    v.voices = (0..v.pitches.len()).map(|i| VoiceId(i as u16)).collect();
    v
}

fn chord(id: u32, sym: &str, onset: i64, function: HarmonicFunction) -> ChordEvent {
    let mut e = ChordEvent::new(
        id,
        symbol::parse(sym).expect("symbol"),
        BeatTime::from_quarters(onset),
        BeatTime::from_quarters(4),
    );
    e.function = Some(function);
    e
}

fn audit_pair(
    profile_id: &str,
    a: &[&str],
    b: &[&str],
    chords: &[ChordEvent],
) -> VoiceLeadingReport {
    audit_voice_leading(
        kb(),
        &profile(profile_id),
        &[voicing(a), voicing(b)],
        chords,
    )
}

/// The G7 to C progression the brief names, in three voices.
fn g7_to_c() -> Vec<ChordEvent> {
    vec![
        chord(0, "G7", 0, HarmonicFunction::Dominant),
        chord(1, "C", 4, HarmonicFunction::Tonic),
    ]
}

// ---------------------------------------------------------------------------
// Parallel and direct perfect intervals.
// ---------------------------------------------------------------------------

#[test]
fn parallel_fifths_penalised_strict() {
    let chords = [
        chord(0, "C", 0, HarmonicFunction::Tonic),
        chord(1, "D", 4, HarmonicFunction::Predominant),
    ];
    let report = audit_pair("strict_counterpoint", &["C3", "G3"], &["D3", "A3"], &chords);
    assert_eq!(report.true_parallels().len(), 1);
    assert_eq!(report.true_parallels()[0].interval.semitones(), 7);
    assert!(parallel_penalty_scale(&profile("strict_counterpoint")) >= 1.5);
    let clean = audit_pair("strict_counterpoint", &["C3", "G3"], &["B2", "G3"], &chords);
    assert!(
        report.score.get("voice_leading") < clean.score.get("voice_leading"),
        "parallel fifths must cost more than oblique motion in strict counterpoint"
    );
}

#[test]
fn parallel_fifths_free_in_power_profile() {
    let chords = [
        chord(0, "C5", 0, HarmonicFunction::Tonic),
        chord(1, "D5", 4, HarmonicFunction::Modal),
    ];
    let report = audit_pair("pop_rock", &["C3", "G3"], &["D3", "A3"], &chords);
    assert_eq!(
        report.true_parallels().len(),
        1,
        "the fact is still reported"
    );
    assert_eq!(
        parallel_penalty_scale(&profile("pop_rock")),
        0.0,
        "but it costs nothing at all"
    );
    let rule = kb()
        .rule("voice_leading.parallel_perfect_fifths")
        .expect("rule");
    assert!(
        !profile("pop_rock").applies_to(rule),
        "the rule does not even apply to a power-chord idiom"
    );
    let idiomatic = kb()
        .rule("voice_leading.power_chord_parallels_are_idiomatic")
        .expect("rule");
    assert!(profile("pop_rock").applies_to(idiomatic));
}

#[test]
fn power_chord_no_parallel_penalty() {
    let chords = [
        chord(0, "C5", 0, HarmonicFunction::Tonic),
        chord(1, "F5", 4, HarmonicFunction::Predominant),
    ];
    let strict = audit_pair("strict_counterpoint", &["C3", "G3"], &["F3", "C4"], &chords);
    let power = audit_pair("pop_rock", &["C3", "G3"], &["F3", "C4"], &chords);
    assert_eq!(strict.true_parallels().len(), power.true_parallels().len());
    assert!(
        power.score.get("voice_leading") > strict.score.get("voice_leading"),
        "power {} should beat strict {}",
        power.score.get("voice_leading"),
        strict.score.get("voice_leading")
    );
}

#[test]
fn parallel_octaves_penalised_strict() {
    let chords = [
        chord(0, "C", 0, HarmonicFunction::Tonic),
        chord(1, "D", 4, HarmonicFunction::Predominant),
    ];
    let report = audit_pair("strict_counterpoint", &["C3", "C4"], &["D3", "D4"], &chords);
    assert_eq!(report.true_parallels().len(), 1);
    assert_eq!(report.true_parallels()[0].interval.semitones() % 12, 0);
}

#[test]
fn octave_doubling_allowed_when_declared() {
    let rule = kb().rule("voice_leading.parallel_octaves").expect("rule");
    assert!(
        rule.exceptions
            .iter()
            .any(|e| e == "doubling_is_allowed_for_role"),
        "a declared doubling is not a parallel-octave fault"
    );
    assert!(!profile("electronic_loop").applies_to(rule));
}

#[test]
fn hidden_fifths_penalised_strict() {
    let chords = [
        chord(0, "C", 0, HarmonicFunction::Tonic),
        chord(1, "G", 4, HarmonicFunction::Dominant),
    ];
    // Both outer voices rise, arriving on a perfect fifth by leap.
    let report = audit_pair("strict_counterpoint", &["C3", "E3"], &["G3", "D4"], &chords);
    assert!(
        report.parallels.iter().any(|p| p.hidden),
        "a direct fifth between outer voices is reported in a strict profile"
    );
}

#[test]
fn hidden_fifths_ignored_pop() {
    let chords = [
        chord(0, "C", 0, HarmonicFunction::Tonic),
        chord(1, "G", 4, HarmonicFunction::Dominant),
    ];
    let report = audit_pair("pop_rock", &["C3", "E3"], &["G3", "D4"], &chords);
    assert!(
        !report.parallels.iter().any(|p| p.hidden),
        "a pop profile does not police direct fifths"
    );
    let rule = kb()
        .rule("voice_leading.direct_perfect_in_outer_voices")
        .expect("rule");
    assert!(!profile("pop_rock").applies_to(rule));
    assert!(
        !profile("jazz_standard").is_rule_enabled("voice_leading.direct_perfect_in_outer_voices"),
        "jazz switches it off explicitly"
    );
}

#[test]
fn planing_zeroes_parallel_penalty() {
    let chords = [
        chord(0, "Cmaj7", 0, HarmonicFunction::Modal),
        chord(1, "Dmaj7", 4, HarmonicFunction::Modal),
    ];
    let plain = audit_with(
        kb(),
        &profile("jazz_standard"),
        &[voicing(&["C4", "G4"]), voicing(&["D4", "A4"])],
        &chords,
        &AuditOptions::default(),
    );
    let planed = audit_with(
        kb(),
        &profile("jazz_standard"),
        &[voicing(&["C4", "G4"]), voicing(&["D4", "A4"])],
        &chords,
        &AuditOptions {
            planing: true,
            ..AuditOptions::default()
        },
    );
    assert!(plain
        .rule_applications
        .iter()
        .any(|a| a.rule_id == "voice_leading.parallel_perfect_fifths"
            && a.status == RuleStatus::Applied
            && a.score_delta < 0.0));
    assert!(
        planed
            .rule_applications
            .iter()
            .any(|a| a.rule_id == "voice_leading.parallel_perfect_fifths"
                && a.status == RuleStatus::Bypassed),
        "planing sets the parallel rule aside rather than obeying it"
    );
    assert_eq!(plain.true_parallels().len(), planed.true_parallels().len());
}

// ---------------------------------------------------------------------------
// Tendency tones in G7 to C.
// ---------------------------------------------------------------------------

#[test]
fn g7_to_c_b_resolves_to_c() {
    let report = audit_pair(
        "common_practice",
        &["G3", "B3", "F4"],
        &["G3", "C4", "E4"],
        &g7_to_c(),
    );
    assert!(
        !report
            .unresolved_tendencies
            .iter()
            .any(|t| t.contains("leading tone")),
        "B rises to C: {:?}",
        report.unresolved_tendencies
    );
    let connection = report
        .connections
        .iter()
        .find(|c| c.from.to_ascii() == "B3")
        .expect("the leading tone's connection");
    assert_eq!(connection.semitones, 1);
    assert_eq!(connection.to.pitch_class(), 0);
    assert_eq!(connection.motion, MotionKind::Step);
}

#[test]
fn g7_to_c_f_resolves_to_e() {
    let report = audit_pair(
        "common_practice",
        &["G3", "B3", "F4"],
        &["G3", "C4", "E4"],
        &g7_to_c(),
    );
    let connection = report
        .connections
        .iter()
        .find(|c| c.from.to_ascii() == "F4")
        .expect("the seventh's connection");
    assert_eq!(connection.semitones, -1, "F falls a semitone to E");
    assert_eq!(connection.to.pitch_class(), 4);
    assert!(!report
        .unresolved_tendencies
        .iter()
        .any(|t| t.contains("seventh")));
}

#[test]
fn leading_tone_rises_in_soprano() {
    let good = audit_pair(
        "strict_counterpoint",
        &["G3", "B3", "F4"],
        &["G3", "C4", "E4"],
        &g7_to_c(),
    );
    let bad = audit_pair(
        "strict_counterpoint",
        &["G3", "B3", "F4"],
        &["G3", "A3", "A4"],
        &g7_to_c(),
    );
    assert!(good.unresolved_tendencies.is_empty());
    assert!(!bad.unresolved_tendencies.is_empty());
    assert!(
        good.score.get("voice_leading") > bad.score.get("voice_leading"),
        "resolving the tendency tones must score better"
    );
}

#[test]
fn seventh_resolves_down() {
    let rule = kb()
        .rule("voice_leading.chordal_seventh_resolves_down")
        .expect("rule");
    assert!(rule.effect.score_delta > 0.0);
    let report = audit_pair(
        "common_practice",
        &["G3", "B3", "F4"],
        &["G3", "C4", "E4"],
        &g7_to_c(),
    );
    assert!(report.rule_applications.iter().any(|a| a.rule_id
        == "voice_leading.chordal_seventh_resolves_down"
        && a.status == RuleStatus::Applied));
}

#[test]
fn tonic_seventh_exempt_from_resolution() {
    let rule = kb()
        .rule("voice_leading.chordal_seventh_resolves_down")
        .expect("rule");
    assert!(
        rule.exceptions.iter().any(|e| e == "function_is_tonic"),
        "a tonic seventh does not have to fall"
    );
    assert!(rule
        .exceptions
        .iter()
        .any(|e| e == "modal_center_is_active"));
    assert!(
        !profile("blues").applies_to(rule),
        "the blues tonic seventh is not an unresolved dominant"
    );
}

#[test]
fn altered_tones_resolve_stepwise() {
    let rule = kb()
        .rule("voice_leading.altered_tone_resolves_by_step")
        .expect("rule");
    assert!(rule.effect.score_delta > 0.0);
    let chords = [
        chord(0, "G7b9", 0, HarmonicFunction::Dominant),
        chord(1, "Cm", 4, HarmonicFunction::Tonic),
    ];
    // Ab (the flat ninth) falls by step to G.
    let report = audit_pair(
        "jazz_standard",
        &["G3", "B3", "Ab4"],
        &["C4", "C4", "G4"],
        &chords,
    );
    let connection = report
        .connections
        .iter()
        .find(|c| c.from.pitch_class() == 8)
        .expect("the flat ninth's connection");
    assert_eq!(connection.semitones.abs(), 1);
}

#[test]
fn unresolved_tendency_at_cadence_penalised() {
    let rule = kb()
        .rule("voice_leading.tendency_tone_unresolved_at_cadence")
        .expect("rule");
    assert!(rule.effect.score_delta < 0.0);
    assert!(rule
        .conditions
        .iter()
        .any(|c| c == "cadence_is_expected_at_this_slot"));
    assert!(
        rule.exceptions
            .iter()
            .any(|e| e == "modal_center_is_active"),
        "modal profiles resolve flexibly"
    );
}

#[test]
fn suspension_resolves_down_by_step() {
    let rule = kb()
        .rule("voice_leading.suspension_resolves_down")
        .expect("rule");
    assert!(rule.effect.score_delta > 0.0);
    assert!(rule
        .conditions
        .iter()
        .any(|c| c == "dissonance_is_prepared"));
    // C over G7sus4 falls to B over G7.
    let sus = symbol::parse("G7sus4").expect("G7sus4");
    let plain = symbol::parse("G7").expect("G7");
    let fourth = (sus.root_pc() + 5).rem_euclid(12);
    assert!(plain.pitch_classes().contains(&((fourth + 11) % 12)));
}

// ---------------------------------------------------------------------------
// Motion, common tones and register.
// ---------------------------------------------------------------------------

#[test]
fn common_tone_retained_same_voice() {
    let chords = [
        chord(0, "C", 0, HarmonicFunction::Tonic),
        chord(1, "F", 4, HarmonicFunction::Predominant),
    ];
    let held = audit_pair(
        "common_practice",
        &["C4", "E4", "G4"],
        &["C4", "F4", "A4"],
        &chords,
    );
    let moved = audit_pair(
        "common_practice",
        &["C4", "E4", "G4"],
        &["F3", "A3", "C4"],
        &chords,
    );
    assert_eq!(held.common_tones_retained(), 1);
    assert!(
        held.score.get("voice_leading") > moved.score.get("voice_leading"),
        "holding the common tone in the same voice must score better"
    );
    assert!(held
        .rule_applications
        .iter()
        .any(|a| a.rule_id == "voice_leading.common_tone_retention"
            && a.status == RuleStatus::Applied));
}

#[test]
fn common_tone_bonus_modal() {
    let rule = kb()
        .rule("voice_leading.common_tone_retention")
        .expect("rule");
    assert!(
        profile("modal_ambient").applies_to(rule),
        "common-tone retention is what modal writing is built on"
    );
    assert!(rule.effect.score_delta > 0.0);
}

#[test]
fn minimal_motion_preferred() {
    let chords = [
        chord(0, "C", 0, HarmonicFunction::Tonic),
        chord(1, "Am", 4, HarmonicFunction::Tonic),
    ];
    let near = audit_pair(
        "common_practice",
        &["C4", "E4", "G4"],
        &["C4", "E4", "A4"],
        &chords,
    );
    let far = audit_pair(
        "common_practice",
        &["C4", "E4", "G4"],
        &["A2", "C3", "E3"],
        &chords,
    );
    assert!(near.total_motion < far.total_motion);
    assert!(near.score.get("voice_leading") > far.score.get("voice_leading"));
}

#[test]
fn minimal_motion_is_not_sole_objective() {
    let rule = kb()
        .rule("voice_leading.minimal_total_motion")
        .expect("rule");
    assert_eq!(rule.kind.id(), "theory_default");
    assert!(!rule.kind.rejects());
    // The DP that assigns voices also pays for range, spacing and guide-tone
    // retention, so the smallest-motion answer is not automatically chosen.
    let events = vec![
        chord(0, "Cmaj7", 0, HarmonicFunction::Tonic),
        chord(1, "F7", 4, HarmonicFunction::Predominant),
    ];
    let voicings = voice_progression(
        kb(),
        &profile("jazz_standard"),
        &events,
        None,
        &VoicingParams::default(),
    )
    .expect("voicings");
    for (event, v) in events.iter().zip(&voicings) {
        let sounding: Vec<i32> = v.pitches.iter().map(|p| p.pitch_class()).collect();
        for degree in event.spec.guide_tones() {
            let pc = (event.spec.root_pc() + degree.simple_semitones()).rem_euclid(12);
            assert!(sounding.contains(&pc), "motion was not the only objective");
        }
    }
}

#[test]
fn contrary_motion_outer_voices_bonus() {
    let rule = kb()
        .rule("voice_leading.contrary_motion_in_outer_voices")
        .expect("rule");
    assert!(rule.effect.score_delta > 0.0);
    assert!(rule.conditions.iter().any(|c| c == "motion_is_contrary"));
    assert!(rule.conditions.iter().any(|c| c == "outer_voices_involved"));
    assert_eq!(
        harmony_engine::voiceleading::relative_motion(2, -2),
        RelativeMotion::Contrary
    );
}

#[test]
fn oblique_motion_over_pedal_bonus() {
    let rule = kb()
        .rule("voice_leading.oblique_motion_over_pedal")
        .expect("rule");
    assert!(rule.effect.score_delta > 0.0);
    assert_eq!(
        harmony_engine::voiceleading::relative_motion(0, 3),
        RelativeMotion::Oblique
    );
}

#[test]
fn pedal_dissonance_not_penalised() {
    let rule = kb()
        .rule("voice_leading.oblique_motion_over_pedal")
        .expect("rule");
    assert!(rule.conditions.iter().any(|c| c == "pedal_point_is_active"));
    assert!(
        profile("modal_ambient").applies_to(rule),
        "a pedal is a modal device, not a dissonance to fix"
    );
}

#[test]
fn voice_crossing_penalised() {
    let chords = [chord(0, "C", 0, HarmonicFunction::Tonic)];
    let mut crossed = voicing(&["C4", "E4"]);
    crossed.pitches = vec![
        SpelledPitch::parse("E4").expect("E4"),
        SpelledPitch::parse("C4").expect("C4"),
    ];
    let report = audit_voice_leading(kb(), &profile("strict_counterpoint"), &[crossed], &chords);
    assert_eq!(report.crossings, 1);
    let clean = audit_voice_leading(
        kb(),
        &profile("strict_counterpoint"),
        &[voicing(&["C4", "E4"])],
        &chords,
    );
    assert!(report.score.get("voice_leading") < clean.score.get("voice_leading"));
}

#[test]
fn voice_crossing_allowed_when_declared() {
    let rule = kb().rule("voice_leading.voice_crossing").expect("rule");
    assert!(rule.exceptions.iter().any(|e| e == "intentional_cluster"));
    assert!(rule
        .exceptions
        .iter()
        .any(|e| e == "quartal_or_planing_context"));
    assert!(!profile("modal_ambient").applies_to(rule));
}

#[test]
fn voice_overlap_penalised() {
    let chords = [
        chord(0, "C", 0, HarmonicFunction::Tonic),
        chord(1, "Am", 4, HarmonicFunction::Tonic),
    ];
    // The lower voice climbs above where the upper voice was.
    let report = audit_pair("common_practice", &["C4", "G4"], &["A4", "C5"], &chords);
    assert!(report.overlaps >= 1, "the overlap is detected");
}

#[test]
fn upper_voice_spacing_within_octave() {
    let rule = kb()
        .rule("voice_leading.upper_voices_within_an_octave")
        .expect("rule");
    assert!(rule.effect.score_delta < 0.0);
    assert!(rule
        .conditions
        .iter()
        .any(|c| c == "outer_voice_span_exceeds_two_octaves"));
    assert!(rule
        .exceptions
        .iter()
        .any(|e| e == "role_is_pad_or_sustained"));
}

#[test]
fn register_drift_penalised() {
    let rule = kb()
        .rule("voice_leading.register_drift_control")
        .expect("rule");
    assert!(rule.effect.score_delta < 0.0);
    assert_eq!(rule.kind.id(), "implementation_heuristic");
}

#[test]
fn voicings_stay_in_register() {
    let events: Vec<ChordEvent> = ["C", "F", "G", "Am", "Dm", "G7", "C"]
        .iter()
        .enumerate()
        .map(|(i, s)| chord(i as u32, s, i as i64 * 4, HarmonicFunction::Tonic))
        .collect();
    let vp = VoicingParams {
        low: 52,
        high: 79,
        ..VoicingParams::default()
    };
    let voicings =
        voice_progression(kb(), &profile("common_practice"), &events, None, &vp).expect("voicings");
    for v in &voicings {
        for p in &v.pitches {
            assert!((52..=79).contains(&p.midi()), "{} drifted", p.to_ascii());
        }
    }
}

#[test]
fn range_enforced_per_instrument() {
    let events = vec![chord(0, "C", 0, HarmonicFunction::Tonic)];
    let vp = VoicingParams {
        low: 0,
        high: 127,
        instrument_profile: Some("choir".to_string()),
        ..VoicingParams::default()
    };
    let voicings =
        voice_progression(kb(), &profile("common_practice"), &events, None, &vp).expect("voicings");
    let choir = kb().instrument_profile("choir").expect("choir");
    for p in &voicings[0].pitches {
        assert!(
            choir.range.contains(p.midi()),
            "{} is outside the choir's range",
            p.to_ascii()
        );
    }
}

#[test]
fn bass_within_configured_range() {
    let events: Vec<ChordEvent> = ["C", "Ab", "F", "G"]
        .iter()
        .enumerate()
        .map(|(i, s)| chord(i as u32, s, i as i64 * 4, HarmonicFunction::Tonic))
        .collect();
    let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
    let bass_profile = kb().instrument_profile("bass").expect("bass");
    for mode in harmony_engine::params::BassMotion::all() {
        let notes = harmony_engine::bass::generate_bass(
            kb(),
            &profile("pop_rock"),
            &events,
            &tm,
            *mode,
            Some(bass_profile),
            3,
        )
        .expect("bass");
        for n in &notes {
            assert!(
                bass_profile.range.contains(n.midi),
                "{} wrote {} outside the bass range",
                mode.id(),
                n.midi
            );
        }
    }
}

#[test]
fn low_interval_limit_from_profile() {
    let rule = kb()
        .rule("voice_leading.spacing_respects_instrument_low_interval_limit")
        .expect("rule");
    assert!(rule
        .conditions
        .iter()
        .any(|c| c == "spacing_violates_instrument_low_interval_limit"));
    let bass = kb().instrument_profile("bass").expect("bass");
    let mallet = kb().instrument_profile("mallet").expect("mallet");
    assert_ne!(
        bass.min_spacing_at(40),
        mallet.min_spacing_at(64),
        "different instruments declare different limits"
    );
}

#[test]
fn low_interval_limit_is_not_universal() {
    let mut limits: Vec<Option<i32>> = kb()
        .instrument_profiles()
        .iter()
        .map(|p| p.min_spacing_at(45))
        .collect();
    limits.sort();
    limits.dedup();
    assert!(
        limits.len() > 1,
        "there is no single universal low interval limit"
    );
}

// ---------------------------------------------------------------------------
// Doubling.
// ---------------------------------------------------------------------------

#[test]
fn leading_tone_not_doubled() {
    let rule = kb()
        .rule("voice_leading.leading_tone_not_doubled")
        .expect("rule");
    assert!(rule.effect.score_delta < 0.0);
    let chords = [chord(0, "G7", 0, HarmonicFunction::Dominant)];
    let report = audit_voice_leading(
        kb(),
        &profile("common_practice"),
        &[voicing(&["G3", "B3", "F4", "B4"])],
        &chords,
    );
    assert!(report
        .rule_applications
        .iter()
        .any(|a| a.rule_id == "voice_leading.leading_tone_not_doubled"
            && a.status == RuleStatus::Applied));
}

#[test]
fn chordal_seventh_not_doubled() {
    let rule = kb()
        .rule("voice_leading.chordal_seventh_not_doubled")
        .expect("rule");
    assert!(rule.effect.score_delta < 0.0);
    assert!(rule.conditions.iter().any(|c| c == "seventh_is_present"));
}

#[test]
fn root_doubled_by_default() {
    let rule = kb()
        .rule("voice_leading.double_the_root_by_default")
        .expect("rule");
    assert!(rule.effect.score_delta > 0.0);
    assert!(rule
        .exceptions
        .iter()
        .any(|e| e == "chord_is_rootless_voicing"));
}

#[test]
fn doubling_choice_reported() {
    let chords = [chord(0, "C", 0, HarmonicFunction::Tonic)];
    let report = audit_voice_leading(
        kb(),
        &profile("common_practice"),
        &[voicing(&["C3", "G3", "C4", "E4"])],
        &chords,
    );
    assert!(
        !report.rule_applications.is_empty(),
        "the doubling decision is recorded, not silent"
    );
    for app in &report.rule_applications {
        assert!(kb().rule(&app.rule_id).is_some());
        assert!(!app.explanation.is_empty());
    }
}

#[test]
fn voice_leap_limited() {
    let rule = kb().rule("voice_leading.large_leap_limit").expect("rule");
    assert!(rule.effect.score_delta < 0.0);
    assert!(rule.conditions.iter().any(|c| c == "voice_leap_is_large"));
}

#[test]
fn bass_octave_leap_allowed() {
    let rule = kb().rule("voice_leading.large_leap_limit").expect("rule");
    assert!(
        rule.exceptions.iter().any(|e| e == "role_is_bass"),
        "the bass may leap where an inner voice may not"
    );
}
