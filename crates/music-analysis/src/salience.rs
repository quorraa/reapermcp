//! Stage 2b — structural salience.
//!
//! Salience answers "which notes must the harmony agree with?". It has to be
//! *explainable*, because a user who disagrees with a harmonisation almost
//! always disagrees with this step rather than with the chord choice, and
//! "the model said so" is not an answer. So it is a plain weighted sum of eight
//! named factors, the weights are public data ([`SalienceWeights`]), and every
//! note's score comes back broken down into the contribution each factor made
//! ([`SalienceDetail::components`]). The contributions always sum to the total.
//!
//! The ninth entry, `pickup`, is a *discount*: an anacrusis is rhythmically
//! prominent but structurally subordinate — it belongs to the harmony of the
//! downbeat it leads to — so it is pushed down rather than up. It is reported
//! as a negative contribution instead of being folded silently into the other
//! weights, so the arithmetic stays inspectable.

use crate::phrase::PhraseAnalysis;
use crate::util::{cmp_f64, median_duration, num, round6, unit};
use music_domain::prelude::*;
use qjson::{Json, JsonMap};
use std::collections::BTreeMap;

/// Salience above which a note is treated as structural.
///
/// Matches `theory_kb::rules::DEFAULT_SALIENCE_THRESHOLD`'s intent: the value a
/// rule's `melody_note_is_structural` predicate compares against.
pub const STRUCTURAL_THRESHOLD: f64 = 0.55;

/// The share of its salience a pickup note keeps.
const PICKUP_RETENTION: f64 = 0.6;

/// The names of the salience components, in the order they are reported.
pub const SALIENCE_COMPONENTS: &[&str] = &[
    "metric",
    "duration",
    "phrase_edge",
    "leap_target",
    "registral_extreme",
    "motivic",
    "cadential",
    "accent",
    "pickup",
];

/// The weight of each salience factor.
///
/// The defaults sum to `1.0`, so a note that maximises every factor scores
/// exactly `1.0` and the numbers are directly comparable across analyses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SalienceWeights {
    /// Metric weight of the onset.
    pub metric: f64,
    /// Duration relative to the prevailing note value.
    pub duration: f64,
    /// Being the first or last note of a phrase or subphrase.
    pub phrase_edge: f64,
    /// Being the note a leap lands on.
    pub leap_target: f64,
    /// Being the highest or lowest note of its phrase.
    pub registral_extreme: f64,
    /// Belonging to a recurring motive.
    pub motivic: f64,
    /// Closing a phrase that carries a cadence.
    pub cadential: f64,
    /// MIDI velocity, the only dynamic information a take carries.
    pub accent: f64,
}

impl Default for SalienceWeights {
    fn default() -> Self {
        SalienceWeights {
            metric: 0.20,
            duration: 0.18,
            phrase_edge: 0.18,
            leap_target: 0.09,
            registral_extreme: 0.09,
            motivic: 0.08,
            cadential: 0.13,
            accent: 0.05,
        }
    }
}

impl SalienceWeights {
    /// The sum of every weight; `1.0` for [`SalienceWeights::default`].
    pub fn total(&self) -> f64 {
        self.metric
            + self.duration
            + self.phrase_edge
            + self.leap_target
            + self.registral_extreme
            + self.motivic
            + self.cadential
            + self.accent
    }

    /// JSON form, so an analysis can report the model it was scored with.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("metric", num(self.metric));
        m.insert("duration", num(self.duration));
        m.insert("phrase_edge", num(self.phrase_edge));
        m.insert("leap_target", num(self.leap_target));
        m.insert("registral_extreme", num(self.registral_extreme));
        m.insert("motivic", num(self.motivic));
        m.insert("cadential", num(self.cadential));
        m.insert("accent", num(self.accent));
        Json::Obj(m)
    }
}

/// One note's salience, broken down.
#[derive(Clone, Debug, PartialEq)]
pub struct SalienceDetail {
    /// The weighted sum, `0.0..=1.0`.
    pub total: f64,
    /// Each factor's contribution to `total`, in [`SALIENCE_COMPONENTS`] order.
    /// These sum to `total` (up to rounding).
    pub components: Vec<(&'static str, f64)>,
}

impl SalienceDetail {
    /// The contribution of one named component.
    pub fn component(&self, name: &str) -> f64 {
        self.components
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
            .unwrap_or(0.0)
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("total", num(self.total));
        let mut c = JsonMap::new();
        for (name, v) in &self.components {
            c.insert(*name, num(*v));
        }
        m.insert("components", Json::Obj(c));
        Json::Obj(m)
    }
}

/// Salience for a whole melody.
#[derive(Clone, Debug, Default)]
pub struct SalienceReport {
    /// Per-note breakdown, keyed on note id so iteration order is stable.
    pub per_note: BTreeMap<NoteId, SalienceDetail>,
    /// Notes at or above [`STRUCTURAL_THRESHOLD`], in time order.
    pub structural: Vec<NoteId>,
    /// The weights this report was produced with.
    pub weights: SalienceWeights,
    /// The threshold used to pick [`SalienceReport::structural`].
    pub threshold: f64,
}

impl SalienceReport {
    /// One note's total salience, `0.0` when unknown.
    pub fn total(&self, id: NoteId) -> f64 {
        self.per_note.get(&id).map(|d| d.total).unwrap_or(0.0)
    }

    /// True when the note is structural.
    pub fn is_structural(&self, id: NoteId) -> bool {
        self.structural.contains(&id)
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("weights", self.weights.to_json());
        m.insert("threshold", num(self.threshold));
        let mut per = JsonMap::new();
        for (id, d) in &self.per_note {
            per.insert(id.to_string(), d.to_json());
        }
        m.insert("per_note", Json::Obj(per));
        m.insert("structural", crate::util::id_array(&self.structural));
        Json::Obj(m)
    }
}

/// Scores every note of `m`.
pub fn analyze_salience(
    m: &NoteSet,
    ph: &PhraseAnalysis,
    tm: &TimeMap,
    w: &SalienceWeights,
) -> SalienceReport {
    analyze_salience_with_threshold(m, ph, tm, w, STRUCTURAL_THRESHOLD)
}

/// [`analyze_salience`] with an explicit structural threshold, which the
/// strictness setting varies.
pub fn analyze_salience_with_threshold(
    m: &NoteSet,
    ph: &PhraseAnalysis,
    tm: &TimeMap,
    w: &SalienceWeights,
    threshold: f64,
) -> SalienceReport {
    let mut report = SalienceReport {
        per_note: BTreeMap::new(),
        structural: Vec::new(),
        weights: *w,
        threshold,
    };
    if m.notes.is_empty() {
        return report;
    }

    let notes = &m.notes;
    let median = median_duration(notes).unwrap_or(BeatTime::ONE);
    let phrase_extremes = extremes_by_phrase(m, ph);

    for (i, n) in notes.iter().enumerate() {
        let metric = unit(tm.metric_weight(n.onset));

        let ratio = n.duration.as_f64() / median.as_f64().max(1e-9);
        let duration = unit((ratio - 0.5) / 1.5);

        // A phrase ending outranks a phrase beginning: it is where the music
        // arrives, and `melody.phrase_end_note_is_structural` treats it as
        // structural outright.
        let phrase_edge = if ph.is_phrase_end(n.id) {
            1.0
        } else if ph.is_phrase_start(n.id) {
            0.85
        } else if ph.is_subphrase_edge(n.id) {
            0.55
        } else {
            0.0
        };

        let leap_target = if i > 0 {
            let iv = (n.midi - notes[i - 1].midi).abs();
            if iv > 2 {
                unit(iv as f64 / 12.0)
            } else {
                0.0
            }
        } else {
            0.0
        };

        let registral_extreme = match phrase_extremes.get(&n.id) {
            Some(Extreme::Peak) => 1.0,
            Some(Extreme::Trough) => 0.6,
            None => 0.0,
        };

        let motivic = ph
            .motives_of(n.id)
            .iter()
            .map(|mo| mo.salience)
            .fold(0.0f64, f64::max);

        let cadential = match ph.phrase_of(n.id) {
            Some(p) if p.notes.last() == Some(&n.id) => {
                if p.cadence.is_some_and(|c| c != CadenceKind::None) {
                    1.0
                } else {
                    0.7
                }
            }
            _ => 0.0,
        };

        let accent = unit((n.velocity.saturating_sub(1)) as f64 / 126.0);

        let mut components = vec![
            ("metric", w.metric * metric),
            ("duration", w.duration * duration),
            ("phrase_edge", w.phrase_edge * phrase_edge),
            ("leap_target", w.leap_target * leap_target),
            ("registral_extreme", w.registral_extreme * registral_extreme),
            ("motivic", w.motivic * unit(motivic)),
            ("cadential", w.cadential * cadential),
            ("accent", w.accent * accent),
        ];
        let raw: f64 = components.iter().map(|(_, v)| *v).sum();
        let pickup = if ph.is_pickup(n.id) {
            -raw * (1.0 - PICKUP_RETENTION)
        } else {
            0.0
        };
        components.push(("pickup", pickup));

        let total = round6(unit(raw + pickup));
        let components = components
            .into_iter()
            .map(|(k, v)| (k, round6(v)))
            .collect();
        report
            .per_note
            .insert(n.id, SalienceDetail { total, components });
    }

    report.structural = notes
        .iter()
        .filter(|n| report.total(n.id) >= threshold)
        .map(|n| n.id)
        .collect();
    report
}

/// Whether a note is the top or bottom of its phrase.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Extreme {
    Peak,
    Trough,
}

/// The registral peak and trough of every phrase, by note id.
fn extremes_by_phrase(m: &NoteSet, ph: &PhraseAnalysis) -> BTreeMap<NoteId, Extreme> {
    let mut out = BTreeMap::new();
    let groups: Vec<&Vec<NoteId>> = if ph.phrases.is_empty() {
        Vec::new()
    } else {
        ph.phrases.iter().map(|p| &p.notes).collect()
    };
    for ids in groups {
        let mut hi: Option<(NoteId, i32)> = None;
        let mut lo: Option<(NoteId, i32)> = None;
        for id in ids {
            let Some(n) = m.notes.iter().find(|n| n.id == *id) else {
                continue;
            };
            if hi.is_none_or(|(hid, hm)| (n.midi, n.id) > (hm, hid)) {
                hi = Some((n.id, n.midi));
            }
            if lo.is_none_or(|(lid, lm)| (n.midi, n.id) < (lm, lid)) {
                lo = Some((n.id, n.midi));
            }
        }
        if let Some((id, _)) = hi {
            out.insert(id, Extreme::Peak);
        }
        if let Some((id, _)) = lo {
            out.entry(id).or_insert(Extreme::Trough);
        }
    }
    out
}

/// The `n` most salient notes, most salient first, ties broken by note id.
pub fn top_structural(report: &SalienceReport, n: usize) -> Vec<NoteId> {
    let mut ids: Vec<(NoteId, f64)> = report.per_note.iter().map(|(k, v)| (*k, v.total)).collect();
    ids.sort_by(|a, b| cmp_f64(b.1, a.1).then(a.0.cmp(&b.0)));
    ids.into_iter().take(n).map(|(id, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phrase::analyze_phrases;
    use crate::testing::note_set;

    fn scored(ns: &NoteSet) -> SalienceReport {
        let ph = analyze_phrases(ns, &ns.time_map.clone());
        analyze_salience(ns, &ph, &ns.time_map.clone(), &SalienceWeights::default())
    }

    #[test]
    fn default_weights_sum_to_one() {
        assert!((SalienceWeights::default().total() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn components_sum_to_the_total() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("E4", "1", "1"),
            ("G4", "2", "2"),
            ("C5", "4", "4"),
        ]);
        let r = scored(&ns);
        for (id, d) in &r.per_note {
            let sum: f64 = d.components.iter().map(|(_, v)| *v).sum();
            assert!(
                (sum - d.total).abs() < 1e-6,
                "note {id}: components sum to {sum}, total is {}",
                d.total
            );
        }
    }

    #[test]
    fn every_component_is_named_and_reported() {
        let ns = note_set(&[("C4", "0", "1"), ("D4", "1", "1")]);
        let r = scored(&ns);
        let d = &r.per_note[&0];
        let names: Vec<&str> = d.components.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, SALIENCE_COMPONENTS);
    }

    #[test]
    fn a_long_note_on_a_downbeat_outscores_a_short_offbeat_one() {
        let ns = note_set(&[
            ("C4", "0", "4"),
            ("D4", "9/2", "1/2"),
            ("E4", "5", "1"),
            ("F4", "6", "2"),
        ]);
        let r = scored(&ns);
        assert!(r.total(0) > r.total(1), "{} vs {}", r.total(0), r.total(1));
        assert!(r.is_structural(0));
        assert!(!r.is_structural(1));
    }

    #[test]
    fn metric_weight_actually_moves_the_score() {
        let strong = note_set(&[("C4", "0", "1"), ("D4", "1", "1")]);
        let r = scored(&strong);
        assert!(r.per_note[&0].component("metric") > r.per_note[&1].component("metric"));
    }

    #[test]
    fn a_pickup_is_discounted() {
        let ns = note_set(&[
            ("G3", "-1", "1"),
            ("C4", "0", "2"),
            ("D4", "2", "1"),
            ("E4", "3", "1"),
            ("F4", "4", "2"),
            ("E4", "6", "2"),
        ]);
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        assert!(ph.is_pickup(0), "the fixture must have a pickup");
        let r = analyze_salience(&ns, &ph, &ns.time_map.clone(), &SalienceWeights::default());
        let d = &r.per_note[&0];
        assert!(d.component("pickup") < 0.0, "pickup discount is missing");
        let raw: f64 = d
            .components
            .iter()
            .filter(|(n, _)| *n != "pickup")
            .map(|(_, v)| *v)
            .sum();
        assert!(d.total < raw);
    }

    #[test]
    fn a_leap_target_scores_on_the_leap_component() {
        let ns = note_set(&[("C4", "0", "1"), ("C5", "1", "1"), ("B4", "2", "1")]);
        let r = scored(&ns);
        assert!(r.per_note[&1].component("leap_target") > 0.0);
        assert_eq!(r.per_note[&2].component("leap_target"), 0.0);
    }

    #[test]
    fn the_registral_peak_of_a_phrase_is_credited() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("E4", "1", "1"),
            ("C5", "2", "1"),
            ("G4", "3", "1"),
        ]);
        let r = scored(&ns);
        assert!(r.per_note[&2].component("registral_extreme") > 0.0);
    }

    #[test]
    fn velocity_feeds_the_accent_component() {
        let mut ns = note_set(&[("C4", "0", "1"), ("D4", "1", "1")]);
        ns.notes[1].velocity = 127;
        ns.notes[0].velocity = 1;
        let r = scored(&ns);
        assert!(r.per_note[&1].component("accent") > r.per_note[&0].component("accent"));
    }

    #[test]
    fn a_cadential_phrase_ending_is_credited() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("D4", "1", "1"),
            ("E4", "2", "2"),
            ("G4", "8", "1"),
            ("F4", "9", "1"),
            ("E4", "10", "2"),
        ]);
        let r = scored(&ns);
        assert!(r.per_note[&2].component("cadential") > 0.0);
    }

    #[test]
    fn structural_notes_are_a_strict_subset_in_time_order() {
        let ns = note_set(&[
            ("C4", "0", "2"),
            ("D4", "2", "1/2"),
            ("E4", "5/2", "1/2"),
            ("F4", "3", "1"),
            ("G4", "4", "4"),
        ]);
        let r = scored(&ns);
        assert!(r.structural.len() < ns.notes.len());
        let mut sorted = r.structural.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, r.structural);
    }

    #[test]
    fn a_custom_threshold_changes_the_structural_set() {
        let ns = note_set(&[("C4", "0", "1"), ("D4", "1", "1"), ("E4", "2", "2")]);
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        let loose = analyze_salience_with_threshold(
            &ns,
            &ph,
            &ns.time_map.clone(),
            &SalienceWeights::default(),
            0.1,
        );
        let tight = analyze_salience_with_threshold(
            &ns,
            &ph,
            &ns.time_map.clone(),
            &SalienceWeights::default(),
            0.9,
        );
        assert!(loose.structural.len() > tight.structural.len());
    }

    #[test]
    fn custom_weights_are_honoured_and_reported() {
        let ns = note_set(&[("C4", "0", "1"), ("D4", "1", "1")]);
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        let w = SalienceWeights {
            metric: 1.0,
            duration: 0.0,
            phrase_edge: 0.0,
            leap_target: 0.0,
            registral_extreme: 0.0,
            motivic: 0.0,
            cadential: 0.0,
            accent: 0.0,
        };
        let r = analyze_salience(&ns, &ph, &ns.time_map.clone(), &w);
        assert_eq!(r.per_note[&0].total, 1.0);
        assert_eq!(r.weights, w);
        assert_eq!(
            r.to_json()
                .get("weights")
                .unwrap()
                .get("metric")
                .unwrap()
                .as_f64(),
            Some(1.0)
        );
    }

    #[test]
    fn salience_is_deterministic() {
        let ns = note_set(&[("C4", "0", "1"), ("E4", "1", "1"), ("G4", "2", "2")]);
        let a = scored(&ns).to_json().to_canonical_string();
        let b = scored(&ns).to_json().to_canonical_string();
        assert_eq!(a, b);
    }

    #[test]
    fn top_structural_returns_the_most_salient_first() {
        let ns = note_set(&[
            ("C4", "0", "4"),
            ("D4", "4", "1/2"),
            ("E4", "9/2", "1/2"),
            ("F4", "5", "4"),
        ]);
        let r = scored(&ns);
        let top = top_structural(&r, 2);
        assert_eq!(top.len(), 2);
        assert!(r.total(top[0]) >= r.total(top[1]));
    }

    #[test]
    fn an_empty_melody_scores_nothing() {
        let r = analyze_salience(
            &NoteSet::default(),
            &PhraseAnalysis::default(),
            &TimeMap::constant(120.0, TimeSignature::new(4, 4)),
            &SalienceWeights::default(),
        );
        assert!(r.per_note.is_empty());
        assert!(r.structural.is_empty());
    }
}
