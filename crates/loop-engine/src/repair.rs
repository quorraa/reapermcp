//! Repair *suggestions*.
//!
//! Nothing in this module changes a note. Each suggestion carries a stable id,
//! a description, a severity, the notes it would touch, the reason it is being
//! proposed, and the knowledge-base rule ids that justify it — so a caller can
//! decide, and a decision trace can cite. The only function in this crate that
//! writes to a note is [`crate::apply_boundary_policy`], and its policy is
//! chosen by the caller.

use crate::audit::LoopInput;
use music_domain::prelude::*;
use theory_kb::{KnowledgeBase, ResolvedProfile};

/// One proposed change, with its justification.
#[derive(Clone, Debug, PartialEq)]
pub struct LoopRepair {
    /// Stable identifier, e.g. `"split_hanging_notes_at_loop_end"`.
    pub id: String,
    /// What the repair would do.
    pub description: String,
    /// How badly the thing being repaired matters.
    pub severity: Severity,
    /// The notes the repair would touch, sorted.
    pub applies_to: Vec<NoteId>,
    /// Why it is being proposed.
    pub rationale: String,
    /// The `knowledge/` rule ids that justify it.
    pub rule_ids: Vec<String>,
}

impl LoopRepair {
    /// JSON form.
    pub fn to_json(&self) -> qjson::Json {
        qjson::json_obj! {
            "id" => self.id.clone(),
            "description" => self.description.clone(),
            "severity" => self.severity.id(),
            "applies_to" => qjson::Json::Arr(
                self.applies_to.iter().map(|n| qjson::Json::Int(i64::from(*n))).collect()),
            "rationale" => self.rationale.clone(),
            "rule_ids" => qjson::Json::Arr(
                self.rule_ids.iter().map(|r| qjson::Json::Str(r.clone())).collect()),
        }
    }
}

/// Builds a repair.
fn repair(
    id: &str,
    description: &str,
    severity: Severity,
    applies_to: Vec<NoteId>,
    rationale: &str,
    rule_ids: &[&str],
) -> LoopRepair {
    LoopRepair {
        id: id.to_string(),
        description: description.to_string(),
        severity,
        applies_to,
        rationale: rationale.to_string(),
        rule_ids: rule_ids.iter().map(|r| (*r).to_string()).collect(),
    }
}

/// True when the report carries a finding with this code.
fn has(report: &LoopReport, code: &str) -> bool {
    report.findings.iter().any(|f| f.code == code)
}

/// The ids of notes that sound before the loop start.
fn pickup_ids(input: &LoopInput<'_>) -> Vec<NoteId> {
    let mut ids: Vec<NoteId> = input
        .notes
        .notes
        .iter()
        .filter(|n| !n.muted && n.duration.is_positive() && n.onset < input.span.start)
        .map(|n| n.id)
        .collect();
    ids.sort_unstable();
    ids
}

/// Every repair the report implies, in a fixed order.
///
/// This is the shared core: [`suggest_repairs`] adds knowledge-base validation
/// on top, and the audit uses the ids to fill [`LoopReport::repairs`].
pub fn build(report: &LoopReport, input: &LoopInput<'_>) -> Vec<LoopRepair> {
    let mut out = Vec::new();
    let intent = report.intent.unwrap_or(LoopIntent::ClosedTonic);

    if has(report, "LOOP_HANGING_NOTE") {
        out.push(repair(
            "split_hanging_notes_at_loop_end",
            "cut each overhanging note at the loop end and wrap its remainder to the loop start",
            Severity::Major,
            report.hanging_notes.clone(),
            "a hanging note is audible as a stuck voice on the first repeat; splitting keeps the \
             sound and removes the overhang",
            &["looping.no_hanging_note_past_loop_end"],
        ));
        out.push(repair(
            "mark_hanging_notes_to_carry",
            "mark each overhanging note to carry, so the overhang becomes an explicit decision",
            Severity::Moderate,
            report.hanging_notes.clone(),
            "carrying a note across the boundary is a valid choice; the rule only objects when \
             nobody asked for it",
            &["looping.no_hanging_note_past_loop_end"],
        ));
    }

    if has(report, "LOOP_PICKUP_PRESENT") {
        let ids = pickup_ids(input);
        out.push(repair(
            "wrap_pickup_into_loop_tail",
            "move the anacrusis to the end of the loop, where the repeat will play it",
            Severity::Moderate,
            ids.clone(),
            "an anacrusis is the most common reason a loop that looks correct in the editor does \
             not repeat correctly, because the first pass is missing its own upbeat",
            &["looping.pickup_wraps_or_duplicates"],
        ));
        out.push(repair(
            "duplicate_pickup_for_first_pass",
            "keep the anacrusis where it is and add a copy at the end of the loop",
            Severity::Moderate,
            ids,
            "duplicating gives the first pass its upbeat and every repeat its own, at the cost of \
             material outside the loop span",
            &["looping.pickup_wraps_or_duplicates"],
        ));
    }

    if has(report, "LOOP_LENGTH_MISMATCH") {
        out.push(repair(
            "trim_material_to_the_loop_span",
            "clip the material to the loop span so the generated length equals the requested \
             length exactly",
            Severity::Moderate,
            report.crossing_notes.clone(),
            "loop boundaries are compared for exact equality, which is why musical time is a \
             normalised rational rather than a float; a drift of one tick compounds on every \
             repeat",
            &["looping.exact_length_is_preserved"],
        ));
    }

    if has(report, "LOOP_BASS_DISCONTINUITY") {
        out.push(repair(
            "smooth_the_bass_across_the_wrap",
            "invert the final chord, or move its bass note, so the bass crosses the wrap by step \
             or by fifth",
            Severity::Moderate,
            Vec::new(),
            "the bass is the part a listener uses to locate the beat, so a discontinuity there is \
             far more noticeable than the same leap in an inner voice",
            &["looping.bass_continuity_across_wrap"],
        ));
    }

    if has(report, "LOOP_VOICE_LEADING_DISCONTINUITY") {
        out.push(repair(
            "revoice_the_final_chord_for_stepwise_connection",
            "revoice the last chord so every voice can reach the first chord by step or by \
             holding",
            Severity::Moderate,
            Vec::new(),
            "treating the wrap as a real chord connection rather than a special case is what \
             makes loop audits agree with the harmony engine",
            &["looping.voice_leading_smooth_across_wrap"],
        ));
    }

    if has(report, "LOOP_HARMONIC_RHYTHM_CHANGE") {
        out.push(repair(
            "equalise_the_harmonic_rhythm_at_the_wrap",
            "give the last slot the same length as the first, or accept the change as a cadential \
             arrival",
            Severity::Moderate,
            Vec::new(),
            "the last slot being half the length of the first is a common artefact of generating \
             a turnaround, and it is audible immediately on repeat",
            &["looping.harmonic_rhythm_stable_at_wrap"],
        ));
    }

    if has(report, "LOOP_LAYER_REMOVAL") {
        out.push(repair(
            "sustain_the_essential_chord_tone_across_the_wrap",
            "double the disappearing guide tone in a layer that is still sounding at the loop \
             start",
            Severity::Major,
            Vec::new(),
            "the wrap is where layer changes and harmony changes coincide, so it is where a \
             missing guide tone is most likely and least noticed during generation",
            &["looping.layer_removal_at_wrap_preserves_harmony"],
        ));
    }

    if has(report, "LOOP_PEDAL_RESTART") {
        out.push(repair(
            "sustain_the_pedal_across_the_wrap",
            "extend the pedal past the loop end and mark it to carry, so it never re-articulates",
            Severity::Minor,
            Vec::new(),
            "a drone is the safest loop element precisely because it never re-articulates; \
             re-triggering it throws that advantage away",
            &["looping.pedal_continues_across_wrap"],
        ));
    }

    if has(report, "LOOP_INTENT_MISMATCH") {
        let (id, description, rationale, rule) = match intent {
            LoopIntent::ClosedTonic => (
                "add_a_turnaround_before_the_wrap",
                "end the loop on a dominant that leads back to the opening tonic",
                "the wrap becomes an authentic cadence, which is why turnarounds exist",
                "looping.closed_tonic_prefers_dominant_to_tonic_wrap",
            ),
            LoopIntent::OpenDominant => (
                "end_the_loop_on_the_dominant",
                "replace the final chord with a dominant and leave its tendency tones unresolved",
                "the unresolved-tendency penalty is suppressed for this intent, because the \
                 caller asked for exactly what it produced",
                "looping.open_dominant_ends_unresolved",
            ),
            LoopIntent::ModalDrone => (
                "hold_a_common_tone_or_pedal_across_the_wrap",
                "sustain a pedal or a shared pitch class through the loop point instead of \
                 cadencing",
                "forcing every loop to close on a tonic is the loop-engine equivalent of forcing \
                 every passage into a major key; modal loops close by continuity instead",
                "looping.modal_drone_does_not_require_dominant",
            ),
            LoopIntent::SeamlessColor => (
                "share_pitch_classes_across_the_wrap",
                "choose a final chord that shares pitch classes with the first and hold them in \
                 the same voices",
                "the goal is that the listener cannot locate the loop point at all, which is a \
                 different objective from making the loop close",
                "looping.seamless_color_uses_common_tones",
            ),
            LoopIntent::TransitionReady => (
                "open_the_final_chord",
                "replace the closing tonic with a chord that can move on",
                "this intent exists so a loop can be dropped into a longer arrangement; a fully \
                 closed ending forces an edit at the exact point the caller wanted flexibility",
                "looping.transition_ready_leaves_an_opening",
            ),
            LoopIntent::OneShotEnding => (
                "close_the_ending_on_the_tonic",
                "finish the material on a tonic so the one-shot actually stops",
                "a stinger is meant to stop; scoring it against loop criteria would penalise it \
                 for succeeding at its actual job",
                "looping.one_shot_ending_is_excluded_from_wrap_scoring",
            ),
        };
        out.push(repair(
            id,
            description,
            Severity::Moderate,
            Vec::new(),
            rationale,
            &[rule],
        ));
    }

    out
}

/// The ids of the repairs the report implies, in the same order.
pub fn repair_ids(report: &LoopReport, input: &LoopInput<'_>) -> Vec<String> {
    build(report, input).into_iter().map(|r| r.id).collect()
}

/// The repairs the audit proposes, validated against the knowledge base.
///
/// Suggestions only: nothing is applied. A repair whose justifying rule the
/// profile has switched off is dropped, because proposing a change on the
/// authority of a rule this profile does not use would be dishonest.
pub fn suggest_repairs(
    kb: &KnowledgeBase,
    prof: &ResolvedProfile,
    input: &LoopInput<'_>,
    report: &LoopReport,
) -> Vec<LoopRepair> {
    build(report, input)
        .into_iter()
        .filter(|r| {
            r.rule_ids.iter().all(|id| match kb.rule(id) {
                Some(rule) => prof.is_rule_enabled(id) && prof.applies_to(rule),
                None => false,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    #[test]
    fn every_cited_rule_id_exists_in_the_knowledge_base() {
        let kb = KnowledgeBase::embedded();
        for intent in testing::ALL_INTENTS {
            let h = testing::pickup_and_hanging_note(*intent, "common_practice");
            let report = h.report();
            for r in build(&report, &h.input()) {
                assert!(!r.rule_ids.is_empty(), "{} cites no rule", r.id);
                for id in &r.rule_ids {
                    assert!(kb.rule(id).is_some(), "{id} is not a rule in knowledge/");
                }
            }
        }
    }

    #[test]
    fn a_clean_loop_needs_no_repairs() {
        let h = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice");
        let report = h.report();
        assert!(report.repairs.is_empty(), "{:?}", report.repairs);
    }

    #[test]
    fn a_hanging_note_offers_both_a_split_and_a_carry() {
        let h = testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice");
        let ids = h.report().repairs;
        assert!(ids.contains(&"split_hanging_notes_at_loop_end".to_string()));
        assert!(ids.contains(&"mark_hanging_notes_to_carry".to_string()));
    }

    #[test]
    fn repairs_name_the_notes_they_would_touch() {
        let h = testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice");
        let report = h.report();
        let repairs = build(&report, &h.input());
        let split = repairs
            .iter()
            .find(|r| r.id == "split_hanging_notes_at_loop_end")
            .expect("split repair");
        assert_eq!(split.applies_to, report.hanging_notes);
        let wrap = repairs
            .iter()
            .find(|r| r.id == "wrap_pickup_into_loop_tail")
            .expect("pickup repair");
        assert!(!wrap.applies_to.is_empty());
    }

    #[test]
    fn suggest_repairs_drops_a_rule_the_profile_does_not_use() {
        // `looping.modal_drone_does_not_require_dominant` is scoped to the
        // modal-leaning profiles, so a common-practice run must not propose a
        // repair on its authority.
        let h = testing::dominant_wrap(LoopIntent::ModalDrone, "common_practice");
        let report = h.report();
        let repairs = suggest_repairs(h.kb, &h.profile, &h.input(), &report);
        assert!(repairs
            .iter()
            .all(|r| r.id != "hold_a_common_tone_or_pedal_across_the_wrap"));
    }

    #[test]
    fn the_repair_json_carries_the_rule_ids() {
        let r = repair(
            "x",
            "d",
            Severity::Minor,
            vec![1, 2],
            "because",
            &["looping.no_hanging_note_past_loop_end"],
        );
        let j = r.to_json();
        assert_eq!(j.get("id").and_then(qjson::Json::as_str), Some("x"));
        assert_eq!(
            j.get("rule_ids")
                .and_then(qjson::Json::as_arr)
                .map(<[_]>::len),
            Some(1)
        );
    }
}
