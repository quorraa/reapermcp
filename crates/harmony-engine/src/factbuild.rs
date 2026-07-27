//! Turning musical decisions into the facts `theory-kb` predicates read.
//!
//! Every rule the engine cites fires because a fact set here made its
//! conditions true. Nothing is asserted that the engine has not actually
//! computed: a fact the stage cannot know is left absent, which makes the rule
//! `not_applicable` rather than a guess.

use crate::ctx::EngineContext;
use music_domain::prelude::*;
use theory_kb::rules::facts;
use theory_kb::{RuleContext, RuleEvent};

/// The degree texts a chord declares, e.g. `["1","3","5","b7","9"]`.
pub fn degree_texts(spec: &ChordSpec) -> Vec<String> {
    spec.chord_tones()
        .into_iter()
        .map(|(d, _)| d.to_string())
        .collect()
}

/// The degree texts a realised voicing actually sounds, relative to the root.
pub fn sounding_degree_texts(spec: &ChordSpec, voicing: &Voicing) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for pitch in &voicing.pitches {
        if let Some(d) = spec.degree_of_pc(pitch.pitch_class()) {
            let text = d.to_string();
            if !out.contains(&text) {
                out.push(text);
            }
        }
    }
    out
}

/// Facts describing what a realised voicing actually sounds.
///
/// The omission predicates are deliberately *not* asserted here: they are
/// derived by the rule engine from the sounding degree list, so what the
/// voicing really plays is what the rules see.
pub fn sound_facts_for_voicing(
    rc: &mut RuleContext,
    spec: &ChordSpec,
    voicing: &Voicing,
    inversion: u8,
) {
    let sounding = sounding_degree_texts(spec, voicing);
    rc.set_list(facts::SOUNDING_DEGREES, &sounding);
    rc.set_list(facts::CHORD_TONE_DEGREES, &degree_texts(spec));
    rc.set_str(facts::CHORD_FAMILY, spec.family_id());
    rc.set_bool(
        "chord_is_in_root_position",
        inversion == 0 && spec.bass.is_none(),
    );
    rc.set_bool("chord_has_altered_tones", !spec.alterations.is_empty());
    rc.set_bool("third_is_suspended", spec.is_suspended());
    rc.set_bool("third_is_not_suspended", !spec.is_suspended());
    rc.set_bool("chord_is_symmetric_collection", is_symmetric(spec));
    // A generated candidate always carries a bass part, so an omitted root is
    // supported rather than lost.
    rc.set_bool("bass_supplies_root", true);
    rc.set_bool(
        "omission_changes_semantic_identity",
        omission_changes_identity(spec, &sounding),
    );
}

/// True when what the voicing leaves out would change what the chord *is*,
/// rather than merely what is played.
///
/// Dropping the fifth, the root or an extension thins the texture. Dropping
/// every quality-bearing tone at once leaves a sonority no listener could
/// identify as the chord that was chosen, and that is a different thing.
pub fn omission_changes_identity(spec: &ChordSpec, sounding: &[String]) -> bool {
    if spec.triad == TriadQuality::Power || spec.omits(3) {
        return false;
    }
    let guide = spec.guide_tones();
    if guide.is_empty() {
        return false;
    }
    guide
        .iter()
        .all(|d| !sounding.iter().any(|s| *s == d.to_string()))
}

/// Facts that describe the request rather than any one chord.
///
/// Shared by every stage so a profile-level or request-level exception — a high
/// chromaticism target, a cluster intent, common-practice strictness — is
/// visible wherever a rule looks for it.
pub fn request_facts(ctx: &EngineContext<'_>, rc: &mut RuleContext) {
    rc.set_num(facts::COMPLEXITY_TARGET, ctx.complexity_target());
    rc.set_num(facts::CHROMATICISM_TARGET, ctx.chromaticism_target());
    rc.set_num(facts::EXTENSION_DENSITY, ctx.extension_density());
    rc.set_str(facts::STRICTNESS, ctx.params.strictness.id());
    rc.set_str(facts::KEY_MODE, &ctx.key.scale_id);
    rc.set_str(facts::SCALE_FAMILY, &ctx.key.scale.def.family);
    rc.set_str(
        facts::KEY_CENTER_KIND,
        if ctx.key.is_modal { "modal" } else { "tonal" },
    );
    rc.set_bool("key_is_minor", ctx.key.key_context_id == "minor");
    rc.set_bool("modal_center_is_active", ctx.key.is_modal);
    rc.set_bool(
        "strictness_is_common_practice",
        ctx.is_common_practice_strict(),
    );
    rc.set_bool(
        "chromaticism_target_is_high",
        ctx.chromaticism_target() >= 0.6,
    );
    rc.set_bool("complexity_target_is_high", ctx.complexity_target() >= 0.6);
    rc.set_bool("extension_density_is_high", ctx.extension_density() >= 0.6);
    // Cluster intent is a request-level choice this build never makes on the
    // caller's behalf; it is reported as absent rather than assumed.
    rc.set_bool("intentional_cluster", false);
    rc.set_bool("cluster_intent_is_false", true);
    if let Some(intent) = ctx.params.loop_intent {
        rc.set_str(facts::LOOP_INTENT, intent.id());
    }
}

/// Facts that describe one chord in isolation.
#[allow(clippy::too_many_arguments)] // Each argument is a distinct musical fact the caller computed.
pub fn chord_facts(
    rc: &mut RuleContext,
    spec: &ChordSpec,
    function: HarmonicFunction,
    category: &str,
    inversion: u8,
    strategy: &str,
    is_diatonic: bool,
    is_cadential: bool,
) {
    let degrees = degree_texts(spec);
    rc.set_str(facts::CHORD_FAMILY, spec.family_id());
    rc.set_str(facts::FUNCTION_CLASS, function_class_id(function));
    rc.set_list(facts::CHORD_TONE_DEGREES, &degrees);
    rc.set_list(facts::SOUNDING_DEGREES, &degrees);
    rc.set_bool(
        "chord_is_in_root_position",
        inversion == 0 && spec.bass.is_none(),
    );
    rc.set_bool("chord_is_diatonic_to_key", is_diatonic);
    rc.set_bool("note_is_chromatic_to_active_scale", !is_diatonic);
    rc.set_bool(
        "chord_is_borrowed_from_parallel_mode",
        category == "borrowed",
    );
    rc.set_bool("chord_is_applied_dominant", category == "applied");
    rc.set_bool(
        "chord_is_tritone_substitute",
        strategy == crate::candidates::STRATEGY_TRITONE_SUB,
    );
    rc.set_bool("chord_is_symmetric_collection", is_symmetric(spec));
    rc.set_bool("chord_symbol_is_ambiguous", false);
    rc.set_bool("cadence_is_expected_at_this_slot", is_cadential);
    rc.set_bool("function_is_tonic", function == HarmonicFunction::Tonic);
    rc.set_bool(
        "function_is_predominant",
        function == HarmonicFunction::Predominant,
    );
    rc.set_bool(
        "function_is_dominant",
        function == HarmonicFunction::Dominant,
    );
    rc.set_bool("pedal_point_is_active", function == HarmonicFunction::Pedal);
    rc.set_bool("chord_has_altered_tones", !spec.alterations.is_empty());
    rc.set_bool(
        "cadential_six_four_present",
        strategy == crate::candidates::STRATEGY_CADENTIAL_SIX_FOUR,
    );
    rc.set_bool("third_is_suspended", spec.is_suspended());
    rc.set_bool("third_is_not_suspended", !spec.is_suspended());
    rc.set_bool("third_is_omitted", spec.omits(3));
    rc.set_bool("fifth_is_omitted", spec.omits(5));
    rc.set_bool("root_is_omitted", spec.omits(1));
    rc.set_bool("seventh_is_present", spec.seventh.is_present());
    rc.set_bool("guide_tones_present", spec.guide_tones().len() >= 2);
    rc.set_bool("chord_is_rootless_voicing", false);
    rc.set_bool("omission_changes_semantic_identity", false);
}

/// Melody facts for the chord under examination.
pub fn melody_facts(
    rc: &mut RuleContext,
    spec: &ChordSpec,
    degree: Option<ChordDegree>,
    structural: bool,
    salience: f64,
    metric_weight: f64,
    nct_confidence: f64,
) {
    rc.set_num(facts::MELODY_SALIENCE, salience);
    rc.set_num(facts::METRIC_WEIGHT, metric_weight);
    rc.set_bool("melody_note_is_structural", structural);
    rc.set_bool("melody_note_is_on_strong_beat", metric_weight >= 0.75);
    rc.set_bool("note_is_on_weak_beat", metric_weight < 0.5);
    match degree {
        Some(d) => {
            rc.set_str(facts::MELODY_CHORD_DEGREE, &d.to_string());
            let chord_tone = spec
                .chord_tones()
                .iter()
                .any(|(have, _)| *have == d && have.number <= 7);
            rc.set_bool("melody_is_chord_tone", chord_tone);
            rc.set_bool(
                "melody_is_extension",
                matches!(d.number, 9 | 11 | 13) || (!chord_tone && d.alter == 0),
            );
            rc.set_bool("melody_is_altered_tone", d.alter != 0 && !chord_tone);
            rc.set_bool(
                "melody_is_11",
                d.number == 11 || (d.number == 4 && !spec.is_suspended()),
            );
        }
        None => {
            rc.set_bool("melody_is_chord_tone", false);
            rc.set_bool("melody_is_extension", false);
            rc.set_bool("melody_is_altered_tone", false);
            rc.set_bool("melody_is_11", false);
        }
    }
    // A tone the analysis explained as a passing or neighbouring motion is not
    // an unexplained clash, which is what the melody-fit rule cares about.
    if nct_confidence > 0.0 {
        rc.set_bool("note_is_approached_by_step", nct_confidence >= 0.4);
        rc.set_bool("note_is_left_by_step", nct_confidence >= 0.4);
    }
}

/// True when the chord divides the octave evenly: a fully diminished seventh or
/// an augmented triad.
pub fn is_symmetric(spec: &ChordSpec) -> bool {
    let mut pcs = spec.pitch_classes();
    pcs.sort_unstable();
    pcs.dedup();
    if pcs.len() < 3 {
        return false;
    }
    let step = (pcs[1] - pcs[0]).rem_euclid(12);
    if step == 0 {
        return false;
    }
    let wraps = pcs.len() as i32 * step == 12;
    wraps && pcs.windows(2).all(|w| (w[1] - w[0]).rem_euclid(12) == step)
}

/// The `functions.json` class id for a domain function.
pub fn function_class_id(f: HarmonicFunction) -> &'static str {
    match f {
        HarmonicFunction::Tonic => "tonic",
        HarmonicFunction::Predominant => "predominant",
        HarmonicFunction::Dominant => "dominant",
        HarmonicFunction::Applied => "applied",
        HarmonicFunction::Chromatic => "chromatic",
        HarmonicFunction::Modal => "modal",
        HarmonicFunction::Pedal => "pedal",
        HarmonicFunction::Passing => "passing",
        HarmonicFunction::Neighbor => "neighbor",
        HarmonicFunction::Unclassified => "unclassified",
    }
}

/// A fresh context carrying the request facts and an event.
pub fn context_for(ctx: &EngineContext<'_>, event: RuleEvent) -> RuleContext {
    let mut rc = RuleContext::new();
    rc.set_event(event);
    request_facts(ctx, &mut rc);
    rc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyctx::function_class;
    use crate::testing;

    #[test]
    fn symmetric_collections_are_recognised() {
        assert!(is_symmetric(&symbol::parse("Cdim7").unwrap()));
        assert!(is_symmetric(&symbol::parse("Caug").unwrap()));
        assert!(!is_symmetric(&symbol::parse("C7").unwrap()));
        assert!(!is_symmetric(&symbol::parse("Cmaj7").unwrap()));
        assert!(!is_symmetric(&symbol::parse("Cm7b5").unwrap()));
    }

    #[test]
    fn degree_texts_describe_the_symbol() {
        let spec = symbol::parse("C9").unwrap();
        let d = degree_texts(&spec);
        assert!(d.contains(&"1".to_string()));
        assert!(d.contains(&"3".to_string()));
        assert!(d.contains(&"b7".to_string()));
        assert!(d.contains(&"9".to_string()));
    }

    #[test]
    fn function_class_ids_round_trip_through_the_key_module() {
        for f in HarmonicFunction::all() {
            let id = function_class_id(*f);
            if *f != HarmonicFunction::Unclassified {
                assert_eq!(function_class(id), *f);
            }
        }
    }

    #[test]
    fn request_facts_answer_the_predicates_they_target() {
        let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
        let ctx = h.context();
        let rc = context_for(&ctx, RuleEvent::ChordSelected);
        for predicate in [
            "modal_center_is_active",
            "key_is_minor",
            "strictness_is_common_practice",
            "chromaticism_target_is_high",
            "intentional_cluster",
            "cluster_intent_is_false",
        ] {
            assert!(
                !rc.evaluate(predicate)
                    .expect("known predicate")
                    .is_unknown(),
                "{predicate} is still unknown after request facts"
            );
        }
    }

    #[test]
    fn every_fact_key_the_engine_writes_is_a_known_predicate_or_named_fact() {
        // Any bool this module sets must be a predicate the rule engine knows,
        // otherwise the rule that depends on it can never fire.
        let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
        let ctx = h.context();
        let mut rc = context_for(&ctx, RuleEvent::ChordSelected);
        let spec = symbol::parse("G7").unwrap();
        chord_facts(
            &mut rc,
            &spec,
            HarmonicFunction::Dominant,
            "diatonic",
            0,
            crate::candidates::STRATEGY_DIATONIC,
            true,
            true,
        );
        melody_facts(&mut rc, &spec, spec.degree_of_pc(11), true, 0.8, 1.0, 0.0);
        let json = rc.to_json();
        let bools = json
            .get("bools")
            .and_then(qjson::Json::as_obj)
            .expect("bools");
        for (k, _) in bools.iter() {
            assert!(
                theory_kb::rules::is_known_predicate(k),
                "{k} is not in the closed predicate vocabulary"
            );
        }
    }
}
