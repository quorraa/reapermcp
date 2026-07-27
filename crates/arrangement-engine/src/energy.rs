//! Whole-loop planning: the long arc, the peak, the silence and the seam.
//!
//! This is the outermost of the brief's three planning levels. It decides the
//! shape of the energy over the whole span, how many layers that shape can
//! afford at any moment, where the maximum-density point sits, where a
//! deliberate silence belongs, which roles recur throughout, whether the loop
//! returns to its tonic and whether it ends open or closed.

use crate::patterns::sample_curve;
use music_domain::prelude::*;
use theory_kb::ResolvedProfile;

/// How a loop leaves its last bar.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Ending {
    /// The harmony resolves; the loop can stop here.
    Closed,
    /// The harmony is left hanging, expecting the wrap.
    Open,
}

impl Ending {
    /// Stable identifier.
    pub fn id(self) -> &'static str {
        match self {
            Ending::Closed => "closed",
            Ending::Open => "open",
        }
    }
}

/// The whole-loop plan.
#[derive(Clone, Debug, PartialEq)]
pub struct LoopPlan {
    /// The sampled energy curve actually used.
    pub curve: Vec<(BeatTime, f64)>,
    /// Where the arrangement is at its densest.
    pub max_density_qn: BeatTime,
    /// The energy there.
    pub max_density_energy: f64,
    /// Net energy change from the first sample to the last.
    pub long_term_energy: f64,
    /// Windows in which background parts deliberately stop.
    pub silence: Vec<(BeatTime, BeatTime)>,
    /// Roles that sound throughout, in allocation order.
    pub recurring_roles: Vec<ArrangementRole>,
    /// True when the last chord returns to the first chord's root.
    pub tonal_return: bool,
    /// How the loop leaves its last bar.
    pub ending: Ending,
    /// Where a transition figure belongs, if anywhere.
    pub transition_qn: Option<BeatTime>,
}

impl LoopPlan {
    /// The energy at a position.
    pub fn energy_at(&self, qn: BeatTime) -> f64 {
        sample_curve(&self.curve, qn, 0.5)
    }

    /// True when `qn` falls inside a planned silence.
    pub fn is_silent(&self, qn: BeatTime) -> bool {
        self.silence.iter().any(|(s, e)| qn >= *s && qn < *e)
    }

    /// JSON form.
    pub fn to_json(&self) -> qjson::Json {
        qjson::json_obj! {
            "curve" => qjson::Json::Arr(self.curve.iter().map(|(qn, v)| qjson::json_obj!{
                "qn" => qn.to_display(),
                "value" => *v,
            }).collect()),
            "max_density_qn" => self.max_density_qn.to_display(),
            "max_density_energy" => self.max_density_energy,
            "long_term_energy" => self.long_term_energy,
            "silence" => qjson::Json::Arr(self.silence.iter().map(|(s, e)| qjson::json_obj!{
                "start" => s.to_display(),
                "end" => e.to_display(),
            }).collect()),
            "recurring_roles" => qjson::Json::Arr(
                self.recurring_roles.iter().map(|r| qjson::Json::Str(r.id().to_string())).collect()
            ),
            "tonal_return" => self.tonal_return,
            "ending" => self.ending.id(),
            "transition_qn" => match self.transition_qn {
                Some(qn) => qjson::Json::Str(qn.to_display()),
                None => qjson::Json::Null,
            },
        }
    }
}

/// The default energy shape when the caller supplies none.
///
/// A gentle rise across the span, anchored on the profile's own
/// `arrangement_density`, so an ambient profile does not receive a pop build.
pub fn default_curve(
    span: (BeatTime, BeatTime),
    profile: &ResolvedProfile,
) -> Vec<(BeatTime, f64)> {
    let base = profile
        .field_f64("arrangement_density")
        .unwrap_or(0.5)
        .clamp(0.0, 1.0);
    let low = (base - 0.15).clamp(0.0, 1.0);
    let high = (base + 0.15).clamp(0.0, 1.0);
    let middle = span.0 + (span.1 - span.0).scale(1, 2);
    vec![(span.0, low), (middle, base), (span.1, high)]
}

/// The curve the plan will actually use.
///
/// An explicit request wins; otherwise a section plan's energies are used; and
/// otherwise the profile's default shape.
pub fn resolve_curve(
    requested: &[(BeatTime, f64)],
    sections: &[Section],
    span: (BeatTime, BeatTime),
    profile: &ResolvedProfile,
) -> Vec<(BeatTime, f64)> {
    if !requested.is_empty() {
        return requested.to_vec();
    }
    if !sections.is_empty() {
        let mut curve: Vec<(BeatTime, f64)> = Vec::with_capacity(sections.len() * 2);
        for s in sections {
            curve.push((s.start, s.energy.clamp(0.0, 1.0)));
            curve.push((s.end, s.energy.clamp(0.0, 1.0)));
        }
        curve.sort_by_key(|a| a.0);
        return curve;
    }
    default_curve(span, profile)
}

/// How many layers an energy level can afford, out of `available`.
///
/// Monotone and never zero: even the quietest moment keeps the part that
/// carries the tune. Half energy affords every requested layer, so a caller who
/// asks for roles and says nothing about energy gets all of them; below that
/// the arrangement genuinely thins out, down to half the layers at silence.
pub fn layer_budget(energy: f64, available: usize) -> usize {
    if available == 0 {
        return 0;
    }
    let e = energy.clamp(0.0, 1.0);
    let scaled = (available as f64 * (0.5 + e)).round() as usize;
    scaled.clamp(1, available)
}

/// Samples the curve once per bar over the span.
pub fn sample_per_bar(
    curve: &[(BeatTime, f64)],
    span: (BeatTime, BeatTime),
    tm: &TimeMap,
) -> Vec<(BeatTime, f64)> {
    let mut out: Vec<(BeatTime, f64)> = Vec::new();
    let first = tm.bar_of(span.0);
    let mut bar = first;
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > crate::patterns::MAX_BARS {
            break;
        }
        let at = tm.bar_start(bar);
        if at >= span.1 {
            break;
        }
        let qn = at.max(span.0);
        if out.last().map(|(p, _)| *p) != Some(qn) {
            out.push((qn, sample_curve(curve, qn, 0.5)));
        }
        bar += 1;
    }
    if out.is_empty() {
        out.push((span.0, sample_curve(curve, span.0, 0.5)));
    }
    out.push((span.1, sample_curve(curve, span.1, 0.5)));
    out
}

/// Builds the whole-loop plan.
pub fn plan_loop(
    curve: &[(BeatTime, f64)],
    chords: &[ChordEvent],
    span: (BeatTime, BeatTime),
    tm: &TimeMap,
    roles: &[ArrangementRole],
    intent: Option<LoopIntent>,
) -> LoopPlan {
    let samples = sample_per_bar(curve, span, tm);
    let mut peak = samples[0];
    for s in &samples {
        if s.1 > peak.1 || (s.1 == peak.1 && s.0 < peak.0) {
            peak = *s;
        }
    }
    let mut trough = samples[0];
    for s in &samples {
        if s.1 < trough.1 || (s.1 == trough.1 && s.0 < trough.0) {
            trough = *s;
        }
    }
    let long_term = samples[samples.len() - 1].1 - samples[0].1;

    // Strategic silence: one bar at the lowest-energy point, but never the
    // first bar (nothing has been established yet) and never the last (the
    // wrap needs something to hand over).
    let bar_len = tm.meter_at(trough.0).bar_length_qn();
    let silence = if samples.len() >= 4 && trough.0 > span.0 && trough.0 + bar_len < span.1 {
        vec![(trough.0, (trough.0 + bar_len).min(span.1))]
    } else {
        Vec::new()
    };

    let tonal_return = match (chords.first(), chords.last()) {
        (Some(first), Some(last)) => last.spec.root_pc() == first.spec.root_pc(),
        _ => false,
    };
    let ending = match intent {
        Some(LoopIntent::OpenDominant) | Some(LoopIntent::TransitionReady) => Ending::Open,
        Some(LoopIntent::ClosedTonic) | Some(LoopIntent::OneShotEnding) => Ending::Closed,
        _ => match chords.last() {
            Some(c) if c.function == Some(HarmonicFunction::Dominant) => Ending::Open,
            _ if tonal_return => Ending::Closed,
            _ => Ending::Open,
        },
    };

    // The loop transition: the last bar, where a fill belongs — unless the
    // loop is meant to be seamless, in which case nothing announces the seam.
    let transition_qn = if matches!(
        intent,
        Some(LoopIntent::SeamlessColor) | Some(LoopIntent::ModalDrone)
    ) {
        None
    } else {
        let last_bar = tm.bar_start(tm.bar_of(span.1 - BeatTime::new(1, 8)));
        if last_bar > span.0 {
            Some(last_bar)
        } else {
            None
        }
    };

    // Recurring roles: the ones that hold the loop together end to end.
    let recurring: Vec<ArrangementRole> = roles
        .iter()
        .copied()
        .filter(|r| {
            matches!(
                r,
                ArrangementRole::Lead
                    | ArrangementRole::Bass
                    | ArrangementRole::Pad
                    | ArrangementRole::HarmonicBed
                    | ArrangementRole::Ostinato
            )
        })
        .collect();

    LoopPlan {
        curve: samples,
        max_density_qn: peak.0,
        max_density_energy: peak.1,
        long_term_energy: long_term,
        silence,
        recurring_roles: recurring,
        tonal_return,
        ending,
        transition_qn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theory_kb::KnowledgeBase;

    fn tm() -> TimeMap {
        TimeMap::constant(120.0, TimeSignature::new(4, 4))
    }

    fn chords() -> Vec<ChordEvent> {
        ["C", "F", "G7", "C"]
            .iter()
            .enumerate()
            .map(|(i, s)| {
                ChordEvent::new(
                    i as u32,
                    music_domain::symbol::parse(s).expect("symbol"),
                    BeatTime::from_quarters(i as i64 * 4),
                    BeatTime::from_quarters(4),
                )
            })
            .collect()
    }

    fn span() -> (BeatTime, BeatTime) {
        (BeatTime::ZERO, BeatTime::from_quarters(16))
    }

    #[test]
    fn layer_budget_is_monotone_and_never_zero() {
        let mut previous = 0;
        for step in 0..=10 {
            let b = layer_budget(step as f64 / 10.0, 6);
            assert!(b >= previous);
            assert!(b >= 1);
            previous = b;
        }
        assert_eq!(layer_budget(0.5, 0), 0);
        assert_eq!(layer_budget(1.0, 6), 6);
    }

    #[test]
    fn the_default_curve_rises() {
        let kb = KnowledgeBase::embedded();
        let profile = kb.resolve_profile("pop_rock").expect("profile");
        let curve = default_curve(span(), &profile);
        assert!(curve.last().unwrap().1 >= curve[0].1);
        assert_eq!(curve[0].0, span().0);
        assert_eq!(curve.last().unwrap().0, span().1);
    }

    #[test]
    fn a_requested_curve_wins() {
        let kb = KnowledgeBase::embedded();
        let profile = kb.resolve_profile("pop_rock").expect("profile");
        let requested = vec![(BeatTime::ZERO, 0.9)];
        let curve = resolve_curve(&requested, &[], span(), &profile);
        assert_eq!(curve, requested);
    }

    #[test]
    fn sections_become_a_step_curve() {
        let kb = KnowledgeBase::embedded();
        let profile = kb.resolve_profile("pop_rock").expect("profile");
        let sections = vec![
            Section {
                id: "a".into(),
                start: BeatTime::ZERO,
                end: BeatTime::from_quarters(8),
                role: "verse".into(),
                energy: 0.2,
            },
            Section {
                id: "b".into(),
                start: BeatTime::from_quarters(8),
                end: BeatTime::from_quarters(16),
                role: "chorus".into(),
                energy: 0.9,
            },
        ];
        let curve = resolve_curve(&[], &sections, span(), &profile);
        assert_eq!(sample_curve(&curve, BeatTime::from_quarters(2), 0.5), 0.2);
        assert_eq!(sample_curve(&curve, BeatTime::from_quarters(12), 0.5), 0.9);
    }

    #[test]
    fn the_peak_is_where_the_energy_is() {
        let curve = vec![
            (BeatTime::ZERO, 0.1),
            (BeatTime::from_quarters(8), 1.0),
            (BeatTime::from_quarters(16), 0.1),
        ];
        let plan = plan_loop(&curve, &chords(), span(), &tm(), &[], None);
        assert_eq!(plan.max_density_qn, BeatTime::from_quarters(8));
        assert!(plan.max_density_energy > 0.9);
        assert!(plan.long_term_energy.abs() < 1e-9);
    }

    #[test]
    fn a_returning_progression_is_a_tonal_return() {
        let plan = plan_loop(&[], &chords(), span(), &tm(), &[], None);
        assert!(plan.tonal_return);
        assert_eq!(plan.ending, Ending::Closed);
    }

    #[test]
    fn loop_intent_decides_the_ending() {
        let plan = plan_loop(
            &[],
            &chords(),
            span(),
            &tm(),
            &[],
            Some(LoopIntent::OpenDominant),
        );
        assert_eq!(plan.ending, Ending::Open);
        assert_eq!(Ending::Open.id(), "open");
    }

    #[test]
    fn a_seamless_loop_gets_no_transition_figure() {
        let plan = plan_loop(
            &[],
            &chords(),
            span(),
            &tm(),
            &[],
            Some(LoopIntent::SeamlessColor),
        );
        assert_eq!(plan.transition_qn, None);
        let other = plan_loop(
            &[],
            &chords(),
            span(),
            &tm(),
            &[],
            Some(LoopIntent::ClosedTonic),
        );
        assert_eq!(other.transition_qn, Some(BeatTime::from_quarters(12)));
    }

    #[test]
    fn strategic_silence_avoids_the_edges() {
        let curve = vec![
            (BeatTime::ZERO, 0.9),
            (BeatTime::from_quarters(8), 0.0),
            (BeatTime::from_quarters(16), 0.9),
        ];
        let plan = plan_loop(&curve, &chords(), span(), &tm(), &[], None);
        assert_eq!(
            plan.silence,
            vec![(BeatTime::from_quarters(8), BeatTime::from_quarters(12))]
        );
        assert!(plan.is_silent(BeatTime::from_quarters(9)));
        assert!(!plan.is_silent(BeatTime::ZERO));
    }

    #[test]
    fn recurring_roles_are_the_structural_ones() {
        let roles = vec![
            ArrangementRole::Lead,
            ArrangementRole::Bass,
            ArrangementRole::Transition,
        ];
        let plan = plan_loop(&[], &chords(), span(), &tm(), &roles, None);
        assert_eq!(
            plan.recurring_roles,
            vec![ArrangementRole::Lead, ArrangementRole::Bass]
        );
        assert!(plan.to_json().get("recurring_roles").is_some());
    }
}
