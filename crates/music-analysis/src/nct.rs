//! Non-chord-tone classification.
//!
//! Every classification here is made from **surrounding motion and metric
//! position** — how the note is approached, how it is left, where it sits in
//! the bar, and how long it lasts. Membership of the sounding chord decides
//! only whether a note *needs* explaining; it never decides which explanation
//! it gets. That distinction is the whole point: a D over a C chord is a
//! passing tone, an accented appoggiatura, a suspension or a ninth depending
//! entirely on what happens either side of it, and a classifier that looks the
//! pitch up in a table cannot tell those apart.
//!
//! The discriminating table is the one in `docs/THEORY_MODEL.md` §6:
//!
//! | | Approached by | Left by | Metric position |
//! |---|---|---|---|
//! | Passing | step | step, same direction | weak |
//! | Neighbour | step | step, returns to origin | weak |
//! | Suspension | tie or repeat | step down | strong |
//! | Retardation | tie or repeat | step up | strong |
//! | Appoggiatura | leap | step | strong |
//! | Escape tone | step | leap, opposite direction | weak |
//! | Anticipation | any | repeated into the next chord | weak |
//! | Chromatic approach | chromatic step | half step | weak |
//!
//! Pedal tones, enclosures and unclassified decorative tones complete the set.
//!
//! A note may carry **several** hypotheses. When the shape genuinely supports
//! more than one reading — the classic case being an accented, stepwise-
//! approached dissonance, which is an appoggiatura to one analyst and an
//! accented passing tone to another — every reading is reported with its own
//! confidence rather than one being picked and presented as fact.
//!
//! # A note on two predicate names
//!
//! `knowledge/rules/melody.json` uses `melody_leap_exceeds_fifth` in both the
//! appoggiatura rule (where the leap is the **approach**) and the escape-tone
//! rule (where it is the **departure**), and `melody_is_repeated_pitch` in the
//! anticipation rule (where the repetition is **forward**, into the next
//! chord, not backward). Both readings are correct for their own rule and
//! contradictory in a shared context, so each hypothesis is evaluated against
//! its own [`RuleContext`] with the predicate asserted in the sense that rule
//! means. This is recorded here rather than worked around silently.

use crate::util::{cmp_f64, round6};
use music_domain::prelude::*;
use std::collections::BTreeMap;
use theory_kb::prelude::*;

/// How many hypotheses one note carries by default.
pub const DEFAULT_MAX_HYPOTHESES: usize = 3;

/// The rule that keeps a genuinely ambiguous tone ambiguous.
const AMBIGUITY_RULE: &str = "melody.ambiguous_tone_keeps_multiple_hypotheses";

/// A candidate shape, before the rule base has been consulted.
struct Shape {
    kind: NctKind,
    rule: Option<&'static str>,
    base: f64,
    detail: String,
}

/// Classifies every melody note against the sounding harmony.
///
/// The active collection is taken from each chord's `local_tonic`, which
/// [`crate::chord_detect::detect_chords`] fills in. Use
/// [`classify_ncts_in_key`] to supply it directly.
pub fn classify_ncts(
    m: &NoteSet,
    harmony: &[ChordEvent],
    tm: &TimeMap,
    kb: &KnowledgeBase,
    profile: &ResolvedProfile,
) -> BTreeMap<NoteId, Vec<NctHypothesis>> {
    let scale_pcs = scale_from_harmony(kb, harmony);
    classify_ncts_in_key(
        m,
        harmony,
        tm,
        kb,
        profile,
        &scale_pcs,
        DEFAULT_MAX_HYPOTHESES,
    )
}

/// [`classify_ncts`] with an explicit active collection and hypothesis limit.
pub fn classify_ncts_in_key(
    m: &NoteSet,
    harmony: &[ChordEvent],
    tm: &TimeMap,
    kb: &KnowledgeBase,
    profile: &ResolvedProfile,
    scale_pcs: &[i32],
    max_hypotheses: usize,
) -> BTreeMap<NoteId, Vec<NctHypothesis>> {
    let engine = RuleEngine::new(kb, profile);
    let mut out: BTreeMap<NoteId, Vec<NctHypothesis>> = BTreeMap::new();
    let notes = &m.notes;

    for (i, n) in notes.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|j| notes.get(j));
        let next = notes.get(i + 1);
        let after = notes.get(i + 2);
        let chord = chord_at(harmony, n.onset);
        let next_chord = chord_after(harmony, n.onset);

        let metric = tm.metric_weight(n.onset);
        let incoming = prev.map(|p| n.midi - p.midi);
        let outgoing = next.map(|q| q.midi - n.midi);

        let is_chord_tone = chord
            .map(|c| c.spec.pitch_classes().contains(&n.pitch_class()))
            .unwrap_or(false);

        let mut shapes: Vec<Shape> = Vec::new();
        if is_chord_tone {
            shapes.push(Shape {
                kind: NctKind::ChordTone,
                rule: None,
                base: 0.9,
                detail: format!(
                    "sounds the {} of the {} in place",
                    chord
                        .and_then(|c| c.spec.degree_of_pc(n.pitch_class()))
                        .map(|d| d.to_string())
                        .unwrap_or_else(|| "chord".to_string()),
                    chord
                        .map(|c| c.spec.render_ascii())
                        .unwrap_or_else(|| "harmony".to_string())
                ),
            });
        } else {
            shapes = dissonant_shapes(
                n, prev, next, incoming, outgoing, metric, scale_pcs, chord, next_chord,
            );
        }

        // A pedal is a role, not an alternative to being a chord tone: the same
        // pitch holding across a change of harmony is a pedal whether or not it
        // belongs to the chord under it.
        if is_pedal(n, harmony) {
            shapes.push(Shape {
                kind: NctKind::PedalTone,
                rule: None,
                base: if is_chord_tone { 0.55 } else { 0.7 },
                detail: "the same pitch is held or repeated across a change of harmony".to_string(),
            });
        }

        // An enclosure surrounds the note two ahead from both sides.
        if let (Some(next), Some(after)) = (next, after) {
            if encloses(n.midi, next.midi, after.midi) {
                shapes.push(Shape {
                    kind: NctKind::Enclosure,
                    rule: None,
                    base: 0.6,
                    detail: format!(
                        "this note and the next surround {} from opposite sides",
                        after.pitch.to_ascii()
                    ),
                });
            }
        }

        if shapes.is_empty() {
            shapes.push(Shape {
                kind: NctKind::Decorative,
                rule: None,
                base: 0.4,
                detail: "does not belong to the chord and matches no standard figure".to_string(),
            });
        }

        let mut hypotheses: Vec<NctHypothesis> = shapes
            .into_iter()
            .map(|s| {
                realise(
                    &engine, profile, kb, s, n, metric, incoming, outgoing, scale_pcs,
                )
            })
            .collect();

        // Keep a genuine ambiguity ambiguous.
        if !is_chord_tone {
            if let Some(alt) = ambiguity_alternate(
                &engine,
                profile,
                kb,
                &hypotheses,
                metric,
                incoming,
                outgoing,
            ) {
                hypotheses.push(alt);
            }
        }

        dedup_by_kind(&mut hypotheses);
        hypotheses
            .sort_by(|a, b| cmp_f64(b.confidence, a.confidence).then(a.kind.id().cmp(b.kind.id())));
        hypotheses.truncate(max_hypotheses.max(1));
        out.insert(n.id, hypotheses);
    }
    out
}

/// The dissonance shapes a note's motion supports.
#[allow(clippy::too_many_arguments)] // one argument per discriminating feature; grouping them would only hide the model
fn dissonant_shapes(
    n: &Note,
    prev: Option<&Note>,
    next: Option<&Note>,
    incoming: Option<i32>,
    outgoing: Option<i32>,
    metric: f64,
    scale_pcs: &[i32],
    chord: Option<&ChordEvent>,
    next_chord: Option<&ChordEvent>,
) -> Vec<Shape> {
    let strong = metric >= 0.5;
    let weak = !strong;
    let step_in = incoming.is_some_and(|v| (1..=2).contains(&v.abs()));
    let step_out = outgoing.is_some_and(|v| (1..=2).contains(&v.abs()));
    let leap_in = incoming.is_some_and(|v| v.abs() > 2);
    let leap_out = outgoing.is_some_and(|v| v.abs() > 2);
    let same_direction = matches!((incoming, outgoing), (Some(a), Some(b)) if a != 0 && b != 0 && a.signum() == b.signum());
    let opposite = matches!((incoming, outgoing), (Some(a), Some(b)) if a != 0 && b != 0 && a.signum() != b.signum());
    let returns = matches!((prev, next), (Some(p), Some(q)) if p.midi == q.midi);
    let prepared = prev.is_some_and(|p| p.midi == n.midi);
    let chromatic = !scale_pcs.is_empty() && !scale_pcs.contains(&n.pitch_class());

    let mut out = Vec::new();

    if step_in && step_out && same_direction {
        out.push(Shape {
            kind: NctKind::Passing,
            rule: Some("melody.passing_tone_hypothesis"),
            // An accented passing tone is a real but weaker reading.
            base: if weak { 0.85 } else { 0.55 },
            detail: format!(
                "approached by step and left by step in the same direction, on a beat of weight \
                 {metric:.2}"
            ),
        });
    }
    if step_in && step_out && returns {
        out.push(Shape {
            kind: NctKind::Neighbor,
            rule: Some("melody.neighbor_tone_hypothesis"),
            base: if weak { 0.85 } else { 0.6 },
            detail: "approached by step and returning to the pitch it left".to_string(),
        });
    }
    if prepared && strong && outgoing.is_some_and(|v| (-2..=-1).contains(&v)) {
        out.push(Shape {
            kind: NctKind::Suspension,
            rule: Some("melody.suspension_hypothesis"),
            base: 0.85,
            detail: "prepared by the same pitch, accented, and resolving down by step".to_string(),
        });
    }
    if prepared && strong && outgoing.is_some_and(|v| (1..=2).contains(&v)) {
        out.push(Shape {
            kind: NctKind::Retardation,
            rule: Some("melody.retardation_hypothesis"),
            base: 0.8,
            detail: "prepared by the same pitch, accented, and resolving up by step".to_string(),
        });
    }
    if strong && leap_in && step_out && !prepared {
        out.push(Shape {
            kind: NctKind::Appoggiatura,
            rule: Some("melody.appoggiatura_hypothesis"),
            base: if incoming.is_some_and(|v| v.abs() > 7) {
                0.8
            } else {
                0.65
            },
            detail: "leapt into on an accented beat and left by step".to_string(),
        });
    }
    if weak && step_in && leap_out && opposite {
        out.push(Shape {
            kind: NctKind::EscapeTone,
            rule: Some("melody.escape_tone_hypothesis"),
            base: 0.75,
            detail: "stepped into on a weak beat and left by leap in the opposite direction"
                .to_string(),
        });
    }
    if weak && anticipates(n, next, chord, next_chord) {
        out.push(Shape {
            kind: NctKind::Anticipation,
            rule: Some("melody.anticipation_hypothesis"),
            base: 0.75,
            detail: "sounds a pitch of the coming chord before the chord arrives".to_string(),
        });
    }
    if chromatic && outgoing.is_some_and(|v| v.abs() == 1) {
        out.push(Shape {
            kind: NctKind::ChromaticApproach,
            rule: Some("melody.chromatic_approach_hypothesis"),
            base: if weak { 0.8 } else { 0.6 },
            detail: "outside the active collection and resolving by half step".to_string(),
        });
    }
    out
}

/// Turns a shape into a hypothesis, consulting the rule that owns it.
#[allow(clippy::too_many_arguments)] // the rule context needs every discriminating feature
fn realise(
    engine: &RuleEngine<'_>,
    profile: &ResolvedProfile,
    kb: &KnowledgeBase,
    shape: Shape,
    n: &Note,
    metric: f64,
    incoming: Option<i32>,
    outgoing: Option<i32>,
    scale_pcs: &[i32],
) -> NctHypothesis {
    let Some(rule_id) = shape.rule else {
        return NctHypothesis::new(shape.kind, round6(shape.base), shape.detail);
    };
    let Some(rule) = kb.rule(rule_id) else {
        return NctHypothesis::new(shape.kind, round6(shape.base * 0.85), shape.detail);
    };
    let ctx = context_for(shape.kind, n, metric, incoming, outgoing, scale_pcs);
    let app = engine.evaluate_rule(rule, &ctx);
    match app.status {
        RuleStatus::Applied => {
            let mult = profile.rule_multiplier(rule_id).clamp(0.5, 1.5);
            NctHypothesis::new(
                shape.kind,
                round6((rule.confidence * mult).clamp(0.0, 1.0)),
                format!("{} — {}", shape.detail, app.explanation),
            )
        }
        RuleStatus::Bypassed => NctHypothesis::new(
            shape.kind,
            round6(shape.base * 0.6),
            format!("{} — {}", shape.detail, app.explanation),
        ),
        _ => NctHypothesis::new(
            shape.kind,
            round6(shape.base * 0.85),
            format!(
                "{} — the motion matches, but {rule_id} did not fire here",
                shape.detail
            ),
        ),
    }
}

/// The rule context for one hypothesis.
///
/// Each hypothesis gets its own context because two predicate names carry
/// different senses in different rules; see the module documentation.
fn context_for(
    kind: NctKind,
    n: &Note,
    metric: f64,
    incoming: Option<i32>,
    outgoing: Option<i32>,
    scale_pcs: &[i32],
) -> RuleContext {
    let mut ctx = RuleContext::new();
    ctx.set_event(RuleEvent::NctHypothesis);
    ctx.set_num(facts::METRIC_WEIGHT, metric);
    if let Some(v) = incoming {
        ctx.set_num(facts::MELODY_INTERVAL_SEMITONES, v as f64);
    }
    ctx.set_bool(
        "note_is_approached_by_step",
        incoming.is_some_and(|v| (1..=2).contains(&v.abs())),
    );
    ctx.set_bool(
        "note_is_left_by_step",
        outgoing.is_some_and(|v| (1..=2).contains(&v.abs())),
    );
    ctx.set_bool(
        "note_is_chromatic_to_active_scale",
        !scale_pcs.is_empty() && !scale_pcs.contains(&n.pitch_class()),
    );
    ctx.set_bool(
        "suspension_resolves_down_by_step",
        outgoing.is_some_and(|v| (-2..=-1).contains(&v)),
    );

    match kind {
        NctKind::Appoggiatura => {
            // The leap is the approach.
            ctx.set_bool(
                "melody_leap_exceeds_fifth",
                incoming.is_some_and(|v| v.abs() > 7),
            );
            ctx.set_bool("dissonance_is_prepared", false);
        }
        NctKind::EscapeTone => {
            // The leap is the departure.
            ctx.set_bool(
                "melody_leap_exceeds_fifth",
                outgoing.is_some_and(|v| v.abs() > 7),
            );
        }
        NctKind::Anticipation => {
            // The repetition is forward, into the next chord.
            ctx.set_bool("melody_is_repeated_pitch", true);
        }
        NctKind::Suspension | NctKind::Retardation => {
            ctx.set_bool("dissonance_is_prepared", true);
        }
        NctKind::Neighbor => {
            ctx.set_bool("note_returns_to_previous_pitch", true);
        }
        _ => {}
    }
    ctx
}

/// The second reading of a genuinely ambiguous tone, when the rule base says
/// one is owed.
fn ambiguity_alternate(
    engine: &RuleEngine<'_>,
    _profile: &ResolvedProfile,
    kb: &KnowledgeBase,
    existing: &[NctHypothesis],
    metric: f64,
    incoming: Option<i32>,
    outgoing: Option<i32>,
) -> Option<NctHypothesis> {
    if existing.len() >= 2 {
        return None;
    }
    let rule = kb.rule(AMBIGUITY_RULE)?;
    let mut ctx = RuleContext::new();
    ctx.set_event(RuleEvent::NctHypothesis);
    ctx.set_num(facts::METRIC_WEIGHT, metric);
    ctx.set_bool(
        "note_is_approached_by_step",
        incoming.is_some_and(|v| (1..=2).contains(&v.abs())),
    );
    let app = engine.evaluate_rule(rule, &ctx);
    if app.status != RuleStatus::Applied {
        return None;
    }
    // Accented and stepped into: the alternative reading to an accented
    // passing tone is an appoggiatura, and vice versa.
    let held = existing.first().map(|h| h.kind);
    let alternate = match held {
        Some(NctKind::Passing) => NctKind::Appoggiatura,
        Some(NctKind::Appoggiatura) => NctKind::Passing,
        Some(NctKind::Suspension) => NctKind::Appoggiatura,
        Some(NctKind::Neighbor) => NctKind::Passing,
        _ => NctKind::Decorative,
    };
    if existing.iter().any(|h| h.kind == alternate) {
        return None;
    }
    let _ = outgoing;
    Some(NctHypothesis::new(
        alternate,
        round6(rule.confidence * 0.6),
        format!(
            "an accented, stepwise-approached dissonance supports more than one reading — {}",
            app.explanation
        ),
    ))
}

/// Drops repeated kinds, keeping the most confident of each.
fn dedup_by_kind(v: &mut Vec<NctHypothesis>) {
    v.sort_by(|a, b| {
        a.kind
            .id()
            .cmp(b.kind.id())
            .then(cmp_f64(b.confidence, a.confidence))
    });
    v.dedup_by(|a, b| a.kind == b.kind);
}

/// True when the note anticipates the coming chord.
fn anticipates(
    n: &Note,
    next: Option<&Note>,
    chord: Option<&ChordEvent>,
    next_chord: Option<&ChordEvent>,
) -> bool {
    let Some(next_chord) = next_chord else {
        return false;
    };
    if chord.is_some_and(|c| c.onset == next_chord.onset) {
        return false;
    }
    // Either the note is repeated across the bar line, or it already sounds a
    // pitch of the chord that is about to arrive.
    let repeated_forward = next.is_some_and(|q| q.midi == n.midi && q.onset >= next_chord.onset);
    let belongs_to_next = next_chord.spec.pitch_classes().contains(&n.pitch_class());
    let close_to_change = n.end() >= next_chord.onset || (next_chord.onset - n.end()).is_zero();
    (repeated_forward || belongs_to_next) && close_to_change
}

/// True when the pitch is held or repeated across a change of harmony.
fn is_pedal(n: &Note, harmony: &[ChordEvent]) -> bool {
    let spanned = harmony
        .iter()
        .filter(|c| c.onset < n.end() && c.end() > n.onset)
        .count();
    spanned >= 2
}

/// True when `a` and `b` surround `target` from opposite sides, both within a
/// whole tone.
fn encloses(a: i32, b: i32, target: i32) -> bool {
    let da = a - target;
    let db = b - target;
    da != 0 && db != 0 && da.signum() != db.signum() && da.abs() <= 2 && db.abs() <= 2
}

/// The chord sounding at `qn`, or the last one that started before it.
fn chord_at(harmony: &[ChordEvent], qn: BeatTime) -> Option<&ChordEvent> {
    harmony
        .iter()
        .find(|c| c.contains(qn))
        .or_else(|| harmony.iter().rfind(|c| c.onset <= qn))
}

/// The first chord starting strictly after the one sounding at `qn`.
fn chord_after(harmony: &[ChordEvent], qn: BeatTime) -> Option<&ChordEvent> {
    let here = chord_at(harmony, qn)?;
    harmony.iter().find(|c| c.onset > here.onset)
}

/// The active collection implied by the harmony's `local_tonic` annotations.
fn scale_from_harmony(kb: &KnowledgeBase, harmony: &[ChordEvent]) -> Vec<i32> {
    for c in harmony {
        if let Some((tonic, scale_id)) = &c.local_tonic {
            if let Some(def) = kb.scale(scale_id) {
                let pc = crate::util::class_pc(*tonic);
                return def
                    .semitones
                    .iter()
                    .map(|s| (pc + s).rem_euclid(12))
                    .collect();
            }
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{chords, note_set, note_set_in};
    use theory_kb::KnowledgeBase;

    fn kb() -> &'static KnowledgeBase {
        KnowledgeBase::embedded()
    }

    fn profile(id: &str) -> ResolvedProfile {
        kb().resolve_profile(id).expect("a profile")
    }

    fn c_major_pcs() -> Vec<i32> {
        vec![0, 2, 4, 5, 7, 9, 11]
    }

    fn classify(ns: &NoteSet, h: &[ChordEvent]) -> BTreeMap<NoteId, Vec<NctHypothesis>> {
        classify_ncts_in_key(
            ns,
            h,
            &ns.time_map.clone(),
            kb(),
            &profile("common_practice"),
            &c_major_pcs(),
            DEFAULT_MAX_HYPOTHESES,
        )
    }

    fn kinds(map: &BTreeMap<NoteId, Vec<NctHypothesis>>, id: NoteId) -> Vec<NctKind> {
        map[&id].iter().map(|h| h.kind).collect()
    }

    #[test]
    fn a_chord_tone_is_reported_as_such() {
        let ns = note_set(&[("C4", "0", "1"), ("E4", "1", "1"), ("G4", "2", "2")]);
        let h = chords(&[("C", "0", "4")]);
        let map = classify(&ns, &h);
        assert!(kinds(&map, 0).contains(&NctKind::ChordTone));
        assert!(kinds(&map, 1).contains(&NctKind::ChordTone));
    }

    #[test]
    fn a_stepwise_weak_beat_passing_tone_is_classified() {
        // C - D - E over C: D is stepped into and out of, in the same
        // direction, on the metrically weak half of beat one.
        let ns = note_set(&[
            ("C4", "0", "1/2"),
            ("D4", "1/2", "1/2"),
            ("E4", "1", "1"),
            ("G4", "2", "2"),
        ]);
        let h = chords(&[("C", "0", "4")]);
        let map = classify(&ns, &h);
        assert_eq!(map[&1][0].kind, NctKind::Passing, "{:?}", kinds(&map, 1));
        assert!(map[&1][0].confidence >= 0.8);
    }

    #[test]
    fn a_passing_tone_is_not_decided_by_pitch_membership_alone() {
        // The same D over the same C chord, but leapt into and leapt out of:
        // it is no longer a passing tone.
        let stepwise = note_set(&[("C4", "0", "1"), ("D4", "1", "1"), ("E4", "2", "2")]);
        let leapt = note_set(&[("G4", "0", "1"), ("D4", "1", "1"), ("A4", "2", "2")]);
        let h = chords(&[("C", "0", "4")]);
        let a = classify(&stepwise, &h);
        let b = classify(&leapt, &h);
        assert!(a[&1].iter().any(|x| x.kind == NctKind::Passing));
        assert!(
            !b[&1].iter().any(|x| x.kind == NctKind::Passing),
            "the same pitch over the same chord: {:?}",
            kinds(&b, 1)
        );
    }

    #[test]
    fn a_neighbour_tone_returns_to_its_origin() {
        let ns = note_set(&[
            ("E4", "0", "1"),
            ("F4", "1", "1"),
            ("E4", "2", "1"),
            ("C4", "3", "1"),
        ]);
        let h = chords(&[("C", "0", "4")]);
        let map = classify(&ns, &h);
        assert!(
            kinds(&map, 1).contains(&NctKind::Neighbor),
            "{:?}",
            kinds(&map, 1)
        );
    }

    #[test]
    fn a_suspension_resolves_down_by_step() {
        // F prepared on the previous beat, sounding on the downbeat over C,
        // resolving down to E.
        let ns = note_set(&[
            ("F4", "3", "1"),
            ("F4", "4", "2"),
            ("E4", "6", "2"),
            ("C4", "8", "2"),
        ]);
        let h = chords(&[("G7", "0", "4"), ("C", "4", "8")]);
        let map = classify(&ns, &h);
        assert_eq!(map[&1][0].kind, NctKind::Suspension, "{:?}", kinds(&map, 1));
        assert!(map[&1][0].rationale.contains("prepared"));
    }

    #[test]
    fn a_retardation_resolves_up() {
        // B prepared, accented over C, resolving up to C.
        let ns = note_set(&[
            ("B3", "3", "1"),
            ("B3", "4", "2"),
            ("C4", "6", "2"),
            ("E4", "8", "2"),
        ]);
        let h = chords(&[("G7", "0", "4"), ("C", "4", "8")]);
        let map = classify(&ns, &h);
        assert!(
            kinds(&map, 1).contains(&NctKind::Retardation),
            "{:?}",
            kinds(&map, 1)
        );
    }

    #[test]
    fn an_anticipation_sounds_the_coming_chord_early() {
        // The last eighth of the G7 bar is already a C.
        let ns = note_set(&[
            ("D4", "0", "2"),
            ("B3", "2", "3/2"),
            ("C4", "7/2", "1/2"),
            ("C4", "4", "2"),
        ]);
        let h = chords(&[("G7", "0", "4"), ("C", "4", "4")]);
        let map = classify(&ns, &h);
        assert!(
            kinds(&map, 2).contains(&NctKind::Anticipation),
            "{:?}",
            kinds(&map, 2)
        );
    }

    #[test]
    fn an_appoggiatura_leaps_in_and_steps_out() {
        let ns = note_set(&[
            ("C4", "3", "1"),
            ("A4", "4", "2"),
            ("G4", "6", "2"),
            ("E4", "8", "2"),
        ]);
        let h = chords(&[("C", "0", "4"), ("C", "4", "8")]);
        let map = classify(&ns, &h);
        assert!(
            kinds(&map, 1).contains(&NctKind::Appoggiatura),
            "{:?}",
            kinds(&map, 1)
        );
    }

    #[test]
    fn an_escape_tone_steps_in_and_leaps_out() {
        let ns = note_set(&[
            ("E4", "0", "1/2"),
            ("F4", "1/2", "1/2"),
            ("C4", "1", "1"),
            ("G4", "2", "2"),
        ]);
        let h = chords(&[("C", "0", "8")]);
        let map = classify(&ns, &h);
        assert!(
            kinds(&map, 1).contains(&NctKind::EscapeTone),
            "{:?}",
            kinds(&map, 1)
        );
    }

    #[test]
    fn a_chromatic_approach_is_recognised() {
        let ns = note_set(&[
            ("C4", "0", "1/2"),
            ("C#4", "1/2", "1/2"),
            ("D4", "1", "1"),
            ("E4", "2", "2"),
        ]);
        let h = chords(&[("C", "0", "8")]);
        let map = classify(&ns, &h);
        assert!(
            kinds(&map, 1).contains(&NctKind::ChromaticApproach),
            "{:?}",
            kinds(&map, 1)
        );
    }

    #[test]
    fn a_chromatic_approach_is_not_read_as_a_modulation() {
        // Only the classification is this module's business, but the point of
        // the rule is that one chromatic step never rewrites the collection.
        let ns = note_set(&[("C4", "0", "1/2"), ("C#4", "1/2", "1/2"), ("D4", "1", "2")]);
        let h = chords(&[("C", "0", "4")]);
        let map = classify(&ns, &h);
        assert!(map[&1].iter().all(|h| h.kind != NctKind::ChordTone));
        assert!(map[&0].iter().any(|h| h.kind == NctKind::ChordTone));
    }

    #[test]
    fn a_pedal_tone_is_recognised_across_a_chord_change() {
        let ns = note_set(&[("G3", "0", "8"), ("C4", "0", "2")]);
        let h = chords(&[("C", "0", "4"), ("F", "4", "4")]);
        let map = classify(&ns, &h);
        assert!(
            map.values()
                .any(|v| v.iter().any(|h| h.kind == NctKind::PedalTone)),
            "no pedal found"
        );
    }

    #[test]
    fn an_enclosure_surrounds_its_target() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("F#4", "1", "1/2"),
            ("A4", "3/2", "1/2"),
            ("G4", "2", "2"),
        ]);
        let h = chords(&[("C", "0", "4")]);
        let map = classify(&ns, &h);
        assert!(
            kinds(&map, 1).contains(&NctKind::Enclosure),
            "{:?}",
            kinds(&map, 1)
        );
    }

    #[test]
    fn an_unrecognisable_dissonance_is_decorative() {
        let ns = note_set(&[("C4", "0", "1"), ("F#4", "1", "1"), ("C4", "2", "2")]);
        let h = chords(&[("C", "0", "4")]);
        let map = classify(&ns, &h);
        assert!(
            kinds(&map, 1).contains(&NctKind::Decorative)
                || kinds(&map, 1).contains(&NctKind::Neighbor),
            "{:?}",
            kinds(&map, 1)
        );
    }

    #[test]
    fn an_ambiguous_tone_keeps_several_hypotheses() {
        // Accented, stepped into, stepped out of: an accented passing tone or
        // an appoggiatura, depending on who you ask.
        let ns = note_set(&[
            ("C4", "3", "1"),
            ("D4", "4", "2"),
            ("E4", "6", "2"),
            ("G4", "8", "2"),
        ]);
        let h = chords(&[("C", "0", "4"), ("C", "4", "8")]);
        let map = classify(&ns, &h);
        assert!(map[&1].len() >= 2, "one reading only: {:?}", kinds(&map, 1));
        let sum: f64 = map[&1].iter().map(|h| h.confidence).sum();
        assert!(sum > 0.0);
    }

    #[test]
    fn a_long_accented_dissonance_is_a_real_event_not_a_decoration() {
        let ns = note_set(&[("C4", "3", "1"), ("F4", "4", "4"), ("E4", "8", "4")]);
        let h = chords(&[("C", "0", "4"), ("C", "4", "4"), ("C", "8", "4")]);
        let map = classify(&ns, &h);
        let ks = kinds(&map, 1);
        assert!(
            ks.iter().any(|k| matches!(
                k,
                NctKind::Appoggiatura | NctKind::Suspension | NctKind::PedalTone
            )),
            "a four-beat accented dissonance was written off as {ks:?}"
        );
        assert!(!ks.contains(&NctKind::Passing));
    }

    #[test]
    fn hypotheses_are_ordered_by_confidence() {
        let ns = note_set(&[("C4", "3", "1"), ("D4", "4", "2"), ("E4", "6", "2")]);
        let h = chords(&[("C", "0", "4"), ("C", "4", "4")]);
        let map = classify(&ns, &h);
        for v in map.values() {
            for w in v.windows(2) {
                assert!(w[0].confidence >= w[1].confidence - 1e-9);
            }
        }
    }

    #[test]
    fn every_note_receives_at_least_one_hypothesis() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("D4", "1", "1"),
            ("E4", "2", "1"),
            ("F#4", "3", "1"),
        ]);
        let h = chords(&[("C", "0", "4")]);
        let map = classify(&ns, &h);
        assert_eq!(map.len(), ns.notes.len());
        assert!(map.values().all(|v| !v.is_empty()));
    }

    #[test]
    fn the_hypothesis_limit_is_respected() {
        let ns = note_set(&[("C4", "3", "1"), ("D4", "4", "2"), ("E4", "6", "2")]);
        let h = chords(&[("C", "0", "4"), ("C", "4", "4")]);
        let map = classify_ncts_in_key(
            &ns,
            &h,
            &ns.time_map.clone(),
            kb(),
            &profile("common_practice"),
            &c_major_pcs(),
            1,
        );
        assert!(map.values().all(|v| v.len() == 1));
    }

    #[test]
    fn classification_is_deterministic() {
        let ns = note_set(&[("C4", "0", "1"), ("D4", "1", "1"), ("E4", "2", "2")]);
        let h = chords(&[("C", "0", "4")]);
        let a = classify(&ns, &h);
        let b = classify(&ns, &h);
        for (k, v) in &a {
            let w = &b[k];
            assert_eq!(v.len(), w.len());
            for (x, y) in v.iter().zip(w) {
                assert_eq!(x.kind, y.kind);
                assert_eq!(x.confidence, y.confidence);
            }
        }
    }

    #[test]
    fn the_profile_reaches_the_classification() {
        let ns = note_set(&[("C4", "0", "1/2"), ("C#4", "1/2", "1/2"), ("D4", "1", "2")]);
        let h = chords(&[("C", "0", "4")]);
        let jazz = classify_ncts_in_key(
            &ns,
            &h,
            &ns.time_map.clone(),
            kb(),
            &profile("jazz_standard"),
            &c_major_pcs(),
            DEFAULT_MAX_HYPOTHESES,
        );
        let modal = classify_ncts_in_key(
            &ns,
            &h,
            &ns.time_map.clone(),
            kb(),
            &profile("modal_ambient"),
            &c_major_pcs(),
            DEFAULT_MAX_HYPOTHESES,
        );
        // `melody.chromatic_approach_hypothesis` is not in the modal profile's
        // scope, so the same motion earns a different confidence.
        let j = jazz[&1]
            .iter()
            .find(|h| h.kind == NctKind::ChromaticApproach)
            .map(|h| h.confidence);
        let m = modal[&1]
            .iter()
            .find(|h| h.kind == NctKind::ChromaticApproach)
            .map(|h| h.confidence);
        assert!(j.is_some());
        assert_ne!(j, m);
    }

    #[test]
    fn three_four_metre_is_read_correctly() {
        // In 3/4 the downbeat is strong and beat two is weak, so a suspension
        // on the downbeat is a suspension and not a passing tone.
        let ns = note_set_in(
            &[("F4", "2", "1"), ("F4", "3", "2"), ("E4", "5", "1")],
            TimeSignature::new(3, 4),
        );
        let h = chords(&[("G7", "0", "3"), ("C", "3", "3")]);
        let map = classify_ncts_in_key(
            &ns,
            &h,
            &ns.time_map.clone(),
            kb(),
            &profile("common_practice"),
            &c_major_pcs(),
            DEFAULT_MAX_HYPOTHESES,
        );
        assert!(
            kinds(&map, 1).contains(&NctKind::Suspension),
            "{:?}",
            kinds(&map, 1)
        );
    }

    #[test]
    fn no_harmony_still_classifies_every_note() {
        let ns = note_set(&[("C4", "0", "1"), ("D4", "1", "1")]);
        let map = classify(&ns, &[]);
        assert_eq!(map.len(), 2);
    }
}
