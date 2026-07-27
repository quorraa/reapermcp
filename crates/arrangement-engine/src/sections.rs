//! Section-level planning.
//!
//! The middle of the brief's three planning levels. A section decides its own
//! density, register, layer count, texture, rhythmic activity, harmonic
//! complexity, how it contrasts with its neighbour, and whether it has to
//! prepare a transition into the next one.
//!
//! The contrast rule is the interesting part: adjacent sections are *forced* to
//! differ in at least one non-dynamic dimension, because
//! `arrangement.contrast_beyond_dynamics` says a section that differs only in
//! velocity is a defect rather than a contrast.

use crate::energy::layer_budget;
use crate::patterns::sample_curve;
use music_domain::prelude::*;

/// How far a section displaces its non-foreground layers.
///
/// A perfect fifth: unmistakable as a change of register, small enough that a
/// part stays recognisably itself and inside the range the catalogue gave it.
pub const REGISTER_CONTRAST_SEMITONES: i32 = 7;

/// Everything one section asks of the parts written inside it.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionPlan {
    /// Position in the plan, `0`-based.
    pub index: usize,
    /// The section itself.
    pub section: Section,
    /// Onset-density multiplier for this section, `0.0..=1.0`.
    pub density: f64,
    /// Register displacement in semitones, applied to non-foreground parts.
    pub register_shift: i32,
    /// How many layers may sound.
    pub layer_budget: usize,
    /// Note-length multiplier — the articulation half of the contrast.
    pub note_length_scale: f64,
    /// Velocity multiplier.
    pub velocity_scale: f64,
    /// How much of the chord the harmony parts may spell, `0.0..=1.0`.
    pub harmonic_complexity: f64,
    /// How busy the section wants to be, `0.0..=1.0`.
    pub rhythmic_activity: f64,
    /// True when the section runs into a change and should prepare it.
    pub transition_prep: bool,
}

impl SectionPlan {
    /// True when `qn` falls inside this section.
    pub fn contains(&self, qn: BeatTime) -> bool {
        qn >= self.section.start && qn < self.section.end
    }

    /// The section's span.
    pub fn span(&self) -> (BeatTime, BeatTime) {
        (self.section.start, self.section.end)
    }

    /// JSON form.
    pub fn to_json(&self) -> qjson::Json {
        qjson::json_obj! {
            "index" => self.index as i64,
            "id" => self.section.id.clone(),
            "role" => self.section.role.clone(),
            "start" => self.section.start.to_display(),
            "end" => self.section.end.to_display(),
            "energy" => self.section.energy,
            "density" => self.density,
            "register_shift" => i64::from(self.register_shift),
            "layer_budget" => self.layer_budget as i64,
            "note_length_scale" => self.note_length_scale,
            "velocity_scale" => self.velocity_scale,
            "harmonic_complexity" => self.harmonic_complexity,
            "rhythmic_activity" => self.rhythmic_activity,
            "transition_prep" => self.transition_prep,
        }
    }
}

/// Splits a span into sections when the caller supplied none.
///
/// The fallback is one section per four bars, which is the phrase length nearly
/// every loop in the fixture corpus uses; a span shorter than that becomes a
/// single section rather than a stub.
pub fn implied_sections(span: (BeatTime, BeatTime), tm: &TimeMap) -> Vec<Section> {
    let bar = tm.meter_at(span.0).bar_length_qn();
    let block = bar * 4;
    if !block.is_positive() || span.0 + block >= span.1 {
        return vec![Section {
            id: "section_1".to_string(),
            start: span.0,
            end: span.1,
            role: "verse".to_string(),
            energy: 0.5,
        }];
    }
    let mut out: Vec<Section> = Vec::new();
    let mut cursor = span.0;
    let mut index = 1usize;
    while cursor < span.1 && index <= 64 {
        let end = (cursor + block).min(span.1);
        out.push(Section {
            id: format!("section_{index}"),
            start: cursor,
            end,
            role: if index % 2 == 1 { "verse" } else { "chorus" }.to_string(),
            energy: 0.5,
        });
        cursor = end;
        index += 1;
    }
    out
}

/// Builds the section-level plan.
///
/// `curve` supplies the energy where the section itself declares none of its
/// own, and every derived control is a monotone function of that energy so the
/// caller's curve is genuinely audible.
pub fn plan_sections(
    sections: &[Section],
    span: (BeatTime, BeatTime),
    tm: &TimeMap,
    curve: &[(BeatTime, f64)],
    available_layers: usize,
    global_density: f64,
) -> Vec<SectionPlan> {
    let source: Vec<Section> = if sections.is_empty() {
        implied_sections(span, tm)
    } else {
        let mut s = sections.to_vec();
        s.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.id.cmp(&b.id)));
        s
    };

    let mut out: Vec<SectionPlan> = Vec::with_capacity(source.len());
    for (index, section) in source.iter().enumerate() {
        let midpoint = section.start + (section.end - section.start).scale(1, 2);
        let energy = if sections.is_empty() {
            sample_curve(curve, midpoint, section.energy)
        } else {
            section.energy
        }
        .clamp(0.0, 1.0);
        let mut planned = section.clone();
        planned.energy = energy;
        out.push(SectionPlan {
            index,
            section: planned,
            density: (global_density * (0.55 + 0.9 * energy)).clamp(0.0, 1.0),
            register_shift: 0,
            layer_budget: layer_budget(energy, available_layers),
            note_length_scale: 1.0 - 0.35 * energy,
            velocity_scale: 0.8 + 0.35 * energy,
            harmonic_complexity: (0.35 + 0.65 * energy).clamp(0.0, 1.0),
            rhythmic_activity: energy,
            transition_prep: false,
        });
    }

    enforce_contrast(&mut out);
    mark_transitions(&mut out);
    out
}

/// Guarantees adjacent sections differ in something other than volume.
///
/// The register shift alternates so that neighbouring sections never occupy the
/// same window, and where the energies are close enough that the derived
/// controls would coincide, the density and note length are nudged apart. Both
/// are measurable, and neither is a velocity change.
pub fn enforce_contrast(plans: &mut [SectionPlan]) {
    for i in 1..plans.len() {
        let previous = plans[i - 1].clone();
        let plan = &mut plans[i];
        let rising = plan.section.energy >= previous.section.energy;
        plan.register_shift = if rising {
            REGISTER_CONTRAST_SEMITONES
        } else {
            -REGISTER_CONTRAST_SEMITONES
        };
        if (plan.density - previous.density).abs() < 0.05 {
            plan.density = if rising {
                (previous.density + 0.2).clamp(0.0, 1.0)
            } else {
                (previous.density - 0.2).clamp(0.0, 1.0)
            };
        }
        if (plan.note_length_scale - previous.note_length_scale).abs() < 0.1 {
            plan.note_length_scale = if rising {
                (previous.note_length_scale - 0.3).clamp(0.15, 1.0)
            } else {
                (previous.note_length_scale + 0.3).clamp(0.15, 1.0)
            };
        }
        if plan.layer_budget == previous.layer_budget && previous.layer_budget > 1 && !rising {
            plan.layer_budget = previous.layer_budget - 1;
        }
    }
}

/// Marks every section that runs into another one.
fn mark_transitions(plans: &mut [SectionPlan]) {
    let last = plans.len().saturating_sub(1);
    for (i, plan) in plans.iter_mut().enumerate() {
        plan.transition_prep = i < last;
    }
}

/// The section covering a position, or the last one before it.
pub fn section_at(plans: &[SectionPlan], qn: BeatTime) -> Option<&SectionPlan> {
    let mut best: Option<&SectionPlan> = None;
    for p in plans {
        if p.contains(qn) {
            return Some(p);
        }
        if p.section.start <= qn {
            best = Some(p);
        }
    }
    best.or_else(|| plans.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tm() -> TimeMap {
        TimeMap::constant(120.0, TimeSignature::new(4, 4))
    }

    fn span() -> (BeatTime, BeatTime) {
        (BeatTime::ZERO, BeatTime::from_quarters(32))
    }

    #[test]
    fn a_short_span_becomes_one_section() {
        let short = (BeatTime::ZERO, BeatTime::from_quarters(8));
        let s = implied_sections(short, &tm());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].start, short.0);
        assert_eq!(s[0].end, short.1);
    }

    #[test]
    fn a_long_span_splits_into_four_bar_blocks() {
        let s = implied_sections(span(), &tm());
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].end, BeatTime::from_quarters(16));
        assert_eq!(s[1].role, "chorus");
    }

    #[test]
    fn implied_sections_cover_the_whole_span_without_gaps() {
        let s = implied_sections(span(), &tm());
        assert_eq!(s[0].start, span().0);
        assert_eq!(s.last().unwrap().end, span().1);
        for w in s.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
    }

    #[test]
    fn energy_drives_every_derived_control() {
        let sections = vec![
            Section {
                id: "quiet".into(),
                start: BeatTime::ZERO,
                end: BeatTime::from_quarters(16),
                role: "verse".into(),
                energy: 0.1,
            },
            Section {
                id: "loud".into(),
                start: BeatTime::from_quarters(16),
                end: BeatTime::from_quarters(32),
                role: "chorus".into(),
                energy: 0.95,
            },
        ];
        let plans = plan_sections(&sections, span(), &tm(), &[], 6, 0.5);
        assert!(plans[1].density > plans[0].density);
        assert!(plans[1].layer_budget > plans[0].layer_budget);
        assert!(plans[1].note_length_scale < plans[0].note_length_scale);
        assert!(plans[1].velocity_scale > plans[0].velocity_scale);
        assert!(plans[1].harmonic_complexity > plans[0].harmonic_complexity);
    }

    #[test]
    fn adjacent_sections_always_differ_beyond_velocity() {
        let sections = vec![
            Section {
                id: "a".into(),
                start: BeatTime::ZERO,
                end: BeatTime::from_quarters(16),
                role: "verse".into(),
                energy: 0.5,
            },
            Section {
                id: "b".into(),
                start: BeatTime::from_quarters(16),
                end: BeatTime::from_quarters(32),
                role: "verse".into(),
                energy: 0.5,
            },
        ];
        let plans = plan_sections(&sections, span(), &tm(), &[], 6, 0.5);
        assert_ne!(plans[0].register_shift, plans[1].register_shift);
        assert!((plans[0].density - plans[1].density).abs() >= 0.05);
        assert!((plans[0].note_length_scale - plans[1].note_length_scale).abs() >= 0.1);
    }

    #[test]
    fn every_section_but_the_last_prepares_a_transition() {
        let plans = plan_sections(&[], span(), &tm(), &[], 4, 0.5);
        assert!(plans[0].transition_prep);
        assert!(!plans.last().unwrap().transition_prep);
    }

    #[test]
    fn section_lookup_finds_the_right_plan() {
        let plans = plan_sections(&[], span(), &tm(), &[], 4, 0.5);
        let found = section_at(&plans, BeatTime::from_quarters(20)).expect("a section");
        assert_eq!(found.index, 1);
        assert!(found.contains(BeatTime::from_quarters(20)));
        assert_eq!(found.span().1, BeatTime::from_quarters(32));
        assert!(section_at(&plans, BeatTime::from_quarters(-4)).is_some());
    }

    #[test]
    fn plans_serialise() {
        let plans = plan_sections(&[], span(), &tm(), &[], 4, 0.5);
        let json = plans[0].to_json();
        for key in ["index", "id", "density", "layer_budget", "transition_prep"] {
            assert!(json.get(key).is_some(), "{key} missing");
        }
    }

    #[test]
    fn an_unsorted_section_list_is_ordered() {
        let sections = vec![
            Section {
                id: "b".into(),
                start: BeatTime::from_quarters(16),
                end: BeatTime::from_quarters(32),
                role: "chorus".into(),
                energy: 0.8,
            },
            Section {
                id: "a".into(),
                start: BeatTime::ZERO,
                end: BeatTime::from_quarters(16),
                role: "verse".into(),
                energy: 0.3,
            },
        ];
        let plans = plan_sections(&sections, span(), &tm(), &[], 4, 0.5);
        assert_eq!(plans[0].section.id, "a");
        assert_eq!(plans[1].section.id, "b");
    }
}
