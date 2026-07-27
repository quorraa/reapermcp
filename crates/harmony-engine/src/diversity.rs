//! Stage 13 — candidate diversity.
//!
//! Three candidates that differ by an added ninth are one candidate. The
//! similarity model here measures the things that actually make two
//! harmonisations different musical decisions — root motion, functional path,
//! modal source, bass contour, chord family, extension family, harmonic rhythm,
//! cadential behaviour and chromaticism — and the selection is a greedy
//! maximal-marginal-relevance walk that trades score against novelty at the
//! rate the profile's `candidate_diversity` field asks for.

use crate::search::HarmonicPath;
use music_domain::prelude::*;
use qjson::rng::DetRng;
use theory_kb::ResolvedProfile;

/// The named similarity dimensions and their weights.
///
/// They are public so a trace can report which dimension made two candidates
/// look alike rather than just asserting that they did.
pub const SIMILARITY_DIMENSIONS: &[(&str, f64)] = &[
    ("root_motion", 1.4),
    ("functional_path", 1.4),
    ("modal_source", 1.0),
    ("bass_contour", 1.2),
    ("chord_family", 1.0),
    ("extension_family", 0.7),
    ("harmonic_rhythm", 0.6),
    ("cadential_behaviour", 1.1),
    ("chromaticism", 0.8),
    ("strategy", 1.0),
];

/// The measurable shape of one harmonisation.
#[derive(Clone, Debug, PartialEq)]
pub struct PathFeatures {
    /// Interval-class histogram of consecutive root motions.
    pub root_motion: [f64; 12],
    /// The functional path as a string of class ids.
    pub functional_path: Vec<String>,
    /// Fraction of chords whose function is modal or pedal.
    pub modal_source: f64,
    /// Sign of each bass step.
    pub bass_contour: Vec<i32>,
    /// Chord-family histogram, keyed by `ChordSpec::family_id`.
    pub chord_family: Vec<(String, f64)>,
    /// Mean chord-tone count.
    pub extension_family: f64,
    /// Distinct chords per chord slot.
    pub harmonic_rhythm: f64,
    /// The last two function classes.
    pub cadential_behaviour: Vec<String>,
    /// Mean fraction of chord tones outside the diatonic set of the first chord.
    pub chromaticism: f64,
    /// The search bias that produced the path.
    pub strategy: String,
}

impl PathFeatures {
    /// Measures a path.
    pub fn of(path: &HarmonicPath) -> PathFeatures {
        let chords = &path.chords;
        let mut root_motion = [0.0f64; 12];
        let mut bass_contour = Vec::new();
        for pair in chords.windows(2) {
            let motion = (pair[1].spec.root_pc() - pair[0].spec.root_pc()).rem_euclid(12) as usize;
            root_motion[motion] += 1.0;
            let bass = bass_pc(&pair[1]) - bass_pc(&pair[0]);
            bass_contour.push(bass.signum());
        }
        let total: f64 = root_motion.iter().sum();
        if total > 0.0 {
            for v in root_motion.iter_mut() {
                *v /= total;
            }
        }
        let functional_path: Vec<String> = chords
            .iter()
            .map(|c| {
                crate::factbuild::function_class_id(
                    c.function.unwrap_or(HarmonicFunction::Unclassified),
                )
                .to_string()
            })
            .collect();
        let modal_source = fraction(chords, |c| {
            matches!(
                c.function,
                Some(HarmonicFunction::Modal) | Some(HarmonicFunction::Pedal)
            )
        });
        let mut chord_family: Vec<(String, f64)> = Vec::new();
        for c in chords {
            let id = c.spec.family_id().to_string();
            match chord_family.iter_mut().find(|(k, _)| *k == id) {
                Some(entry) => entry.1 += 1.0,
                None => chord_family.push((id, 1.0)),
            }
        }
        let n = chords.len().max(1) as f64;
        for entry in chord_family.iter_mut() {
            entry.1 /= n;
        }
        chord_family.sort_by(|a, b| a.0.cmp(&b.0));
        let extension_family = chords
            .iter()
            .map(|c| c.spec.chord_tones().len() as f64)
            .sum::<f64>()
            / n;
        let mut distinct: Vec<String> = chords.iter().map(|c| c.spec.render_ascii()).collect();
        distinct.sort();
        distinct.dedup();
        let harmonic_rhythm = distinct.len() as f64 / n;
        let cadential_behaviour: Vec<String> = functional_path
            .iter()
            .rev()
            .take(2)
            .rev()
            .cloned()
            .collect();
        let home = chords
            .first()
            .map(|c| c.spec.pitch_classes())
            .unwrap_or_default();
        let chromaticism = chords
            .iter()
            .map(|c| {
                let pcs = c.spec.pitch_classes();
                if pcs.is_empty() {
                    0.0
                } else {
                    pcs.iter().filter(|pc| !home.contains(pc)).count() as f64 / pcs.len() as f64
                }
            })
            .sum::<f64>()
            / n;
        PathFeatures {
            root_motion,
            functional_path,
            modal_source,
            bass_contour,
            chord_family,
            extension_family,
            harmonic_rhythm,
            cadential_behaviour,
            chromaticism,
            strategy: path.strategy.clone(),
        }
    }

    /// Per-dimension similarity with another path, `0.0..=1.0` each.
    pub fn similarity_breakdown(&self, other: &PathFeatures) -> Vec<(&'static str, f64)> {
        vec![
            (
                "root_motion",
                histogram_similarity(&self.root_motion, &other.root_motion),
            ),
            (
                "functional_path",
                sequence_similarity(&self.functional_path, &other.functional_path),
            ),
            (
                "modal_source",
                1.0 - (self.modal_source - other.modal_source).abs(),
            ),
            (
                "bass_contour",
                sequence_similarity(&self.bass_contour, &other.bass_contour),
            ),
            (
                "chord_family",
                sparse_similarity(&self.chord_family, &other.chord_family),
            ),
            (
                "extension_family",
                1.0 - ((self.extension_family - other.extension_family).abs() / 4.0).min(1.0),
            ),
            (
                "harmonic_rhythm",
                1.0 - (self.harmonic_rhythm - other.harmonic_rhythm).abs(),
            ),
            (
                "cadential_behaviour",
                sequence_similarity(&self.cadential_behaviour, &other.cadential_behaviour),
            ),
            (
                "chromaticism",
                1.0 - (self.chromaticism - other.chromaticism).abs(),
            ),
            (
                "strategy",
                if self.strategy == other.strategy {
                    1.0
                } else {
                    0.0
                },
            ),
        ]
    }

    /// The weighted similarity with another path, `0.0..=1.0`.
    pub fn similarity(&self, other: &PathFeatures) -> f64 {
        let parts = self.similarity_breakdown(other);
        let mut sum = 0.0;
        let mut weight = 0.0;
        for (name, value) in parts {
            let w = SIMILARITY_DIMENSIONS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, w)| *w)
                .unwrap_or(1.0);
            sum += value.clamp(0.0, 1.0) * w;
            weight += w;
        }
        if weight > 0.0 {
            sum / weight
        } else {
            0.0
        }
    }
}

/// The sounding bass pitch class of a chord event.
fn bass_pc(chord: &ChordEvent) -> i32 {
    if chord.spec.bass.is_some() {
        return chord.spec.bass_pc();
    }
    let pcs = chord.spec.pitch_classes();
    if pcs.is_empty() {
        return 0;
    }
    pcs[(chord.inversion as usize).min(pcs.len() - 1)]
}

/// Fraction of chords satisfying a predicate.
fn fraction(chords: &[ChordEvent], f: impl Fn(&ChordEvent) -> bool) -> f64 {
    if chords.is_empty() {
        return 0.0;
    }
    chords.iter().filter(|c| f(c)).count() as f64 / chords.len() as f64
}

/// Overlap of two normalised histograms.
fn histogram_similarity(a: &[f64; 12], b: &[f64; 12]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| x.min(*y)).sum()
}

/// Positionwise agreement of two sequences.
fn sequence_similarity<T: PartialEq>(a: &[T], b: &[T]) -> f64 {
    let n = a.len().max(b.len());
    if n == 0 {
        return 1.0;
    }
    let matched = a.iter().zip(b.iter()).filter(|(x, y)| x == y).count();
    matched as f64 / n as f64
}

/// Overlap of two sparse normalised histograms.
fn sparse_similarity(a: &[(String, f64)], b: &[(String, f64)]) -> f64 {
    let mut sum = 0.0;
    for (key, value) in a {
        if let Some((_, other)) = b.iter().find(|(k, _)| k == key) {
            sum += value.min(*other);
        }
    }
    sum
}

/// Stage 13, as the frozen contract declares it.
///
/// Returns at most `want` paths, each as different from the others as the
/// profile's diversity appetite allows, with `candidate_diversity` written into
/// every returned score vector so the trade-off is visible rather than implied.
pub fn diversify(
    paths: Vec<HarmonicPath>,
    want: usize,
    prof: &ResolvedProfile,
    seed: u64,
) -> Vec<HarmonicPath> {
    if paths.is_empty() || want == 0 {
        return Vec::new();
    }
    let appetite = prof
        .field_f64("candidate_diversity")
        .unwrap_or(0.5)
        .clamp(0.0, 1.0);
    // A high appetite is willing to trade a lot of score for novelty; a low one
    // keeps close to the optimum.
    let lambda = 0.25 + 0.75 * appetite;

    let features: Vec<PathFeatures> = paths.iter().map(PathFeatures::of).collect();
    let totals: Vec<f64> = paths.iter().map(|p| p.score.total()).collect();
    let (min, max) = totals
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let span = (max - min).max(1e-9);
    let mut rng = DetRng::new(seed).derive("diversity");
    let jitter: Vec<f64> = paths.iter().map(|_| rng.next_f64()).collect();

    // Clustering first: one representative per search strategy, best-scoring
    // first. This is what makes a three-candidate request return a functional,
    // a modal/common-tone and a chromatic/bass-led reading rather than three
    // spellings of the same idea.
    let mut strategies: Vec<String> = Vec::new();
    for path in &paths {
        if !strategies.contains(&path.strategy) {
            strategies.push(path.strategy.clone());
        }
    }
    let mut representatives: Vec<usize> = Vec::new();
    for strategy in &strategies {
        let best = (0..paths.len())
            .filter(|i| paths[*i].strategy == *strategy)
            .max_by(|a, b| {
                totals[*a]
                    .partial_cmp(&totals[*b])
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        jitter[*b]
                            .partial_cmp(&jitter[*a])
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
            });
        if let Some(index) = best {
            representatives.push(index);
        }
    }
    representatives.sort_by(|a, b| {
        totals[*b]
            .partial_cmp(&totals[*a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| paths[*a].shape().cmp(&paths[*b].shape()))
    });

    let mut chosen: Vec<usize> = Vec::new();
    let mut remaining: Vec<usize> = (0..paths.len()).collect();
    for index in representatives {
        if chosen.len() >= want.min(paths.len()) {
            break;
        }
        if let Some(position) = remaining.iter().position(|i| *i == index) {
            chosen.push(index);
            remaining.remove(position);
        }
    }
    while chosen.len() < want.min(paths.len()) {
        let mut best: Option<(f64, f64, usize, usize)> = None;
        for (position, index) in remaining.iter().enumerate() {
            let normalised = (totals[*index] - min) / span;
            let similarity = chosen
                .iter()
                .map(|c| features[*c].similarity(&features[*index]))
                .fold(0.0f64, f64::max);
            let value = normalised - lambda * similarity;
            let candidate = (value, jitter[*index], *index, position);
            best = Some(match best {
                None => candidate,
                Some(current) => {
                    if candidate.0 > current.0 + 1e-9
                        || ((candidate.0 - current.0).abs() <= 1e-9 && candidate.1 > current.1)
                    {
                        candidate
                    } else {
                        current
                    }
                }
            });
        }
        let Some((_, _, index, position)) = best else {
            break;
        };
        chosen.push(index);
        remaining.remove(position);
    }

    let mut out: Vec<HarmonicPath> = Vec::with_capacity(chosen.len());
    for (rank, index) in chosen.iter().enumerate() {
        let mut path = paths[*index].clone();
        let similarity = chosen
            .iter()
            .take(rank)
            .map(|c| features[*c].similarity(&features[*index]))
            .fold(0.0f64, f64::max);
        path.score.set("candidate_diversity", 1.0 - similarity);
        let weights: Vec<(&str, f64)> = SCORE_COMPONENTS
            .iter()
            .map(|c| (*c, prof.weight(c)))
            .collect();
        path.score.recompute_total(&weights);
        out.push(path);
    }
    out
}

/// The largest pairwise similarity in a set, for tests and reports.
pub fn max_pairwise_similarity(paths: &[HarmonicPath]) -> f64 {
    let features: Vec<PathFeatures> = paths.iter().map(PathFeatures::of).collect();
    let mut worst = 0.0f64;
    for i in 0..features.len() {
        for j in (i + 1)..features.len() {
            worst = worst.max(features[i].similarity(&features[j]));
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidates::build_pools;
    use crate::params::{CancelFlag, SearchConfig};
    use crate::search::search_in;
    use crate::testing;
    use theory_kb::KnowledgeBase;

    fn paths(fixture: &str, profile: &str) -> (testing::Harness, Vec<HarmonicPath>) {
        let h = testing::harness(fixture, profile);
        let found = {
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
        };
        (h, found)
    }

    #[test]
    fn a_path_is_identical_to_itself() {
        let (_, found) = paths("melodies/eight_bar_c_major", "jazz_standard");
        let f = PathFeatures::of(&found[0]);
        assert!((f.similarity(&f) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn selection_is_deterministic() {
        let (h, found) = paths("melodies/eight_bar_c_major", "jazz_standard");
        let a = diversify(found.clone(), 3, &h.profile, 5);
        let b = diversify(found, 3, &h.profile, 5);
        let sa: Vec<String> = a.iter().map(|p| p.signature()).collect();
        let sb: Vec<String> = b.iter().map(|p| p.signature()).collect();
        assert_eq!(sa, sb);
    }

    #[test]
    fn selection_returns_distinct_paths() {
        let (h, found) = paths("melodies/eight_bar_c_major", "jazz_standard");
        let picked = diversify(found, 3, &h.profile, 0);
        assert_eq!(picked.len(), 3);
        let mut signatures: Vec<String> = picked.iter().map(|p| p.signature()).collect();
        signatures.sort();
        signatures.dedup();
        assert_eq!(signatures.len(), 3);
    }

    #[test]
    fn selection_beats_taking_the_top_three() {
        let (h, found) = paths("melodies/eight_bar_c_major", "common_practice");
        let top: Vec<HarmonicPath> = found.iter().take(3).cloned().collect();
        let picked = diversify(found, 3, &h.profile, 0);
        assert!(
            max_pairwise_similarity(&picked) < max_pairwise_similarity(&top),
            "diversified {:.3} should be less similar than the top three {:.3}",
            max_pairwise_similarity(&picked),
            max_pairwise_similarity(&top)
        );
    }

    #[test]
    fn diversity_component_is_written() {
        let (h, found) = paths("melodies/eight_bar_c_major", "cinematic");
        let picked = diversify(found, 3, &h.profile, 0);
        for path in &picked {
            assert!(path.score.contains("candidate_diversity"));
            assert!((0.0..=1.0).contains(&path.score.get("candidate_diversity")));
        }
        assert_eq!(picked[0].score.get("candidate_diversity"), 1.0);
    }

    #[test]
    fn empty_and_zero_requests_are_handled() {
        let kb = KnowledgeBase::embedded();
        let prof = kb.resolve_profile("pop_rock").expect("profile");
        assert!(diversify(Vec::new(), 3, &prof, 0).is_empty());
        let (h, found) = paths("melodies/eight_bar_c_major", "pop_rock");
        let _ = h;
        assert!(diversify(found, 0, &prof, 0).is_empty());
    }

    #[test]
    fn asking_for_more_than_exists_returns_what_exists() {
        let (h, found) = paths("melodies/eight_bar_c_major", "blues");
        let n = found.len();
        let picked = diversify(found, n + 10, &h.profile, 0);
        assert_eq!(picked.len(), n);
    }

    #[test]
    fn similarity_dimensions_are_all_reported() {
        let (_, found) = paths("melodies/eight_bar_c_major", "jazz_standard");
        let a = PathFeatures::of(&found[0]);
        let b = PathFeatures::of(&found[found.len() - 1]);
        let breakdown = a.similarity_breakdown(&b);
        assert_eq!(breakdown.len(), SIMILARITY_DIMENSIONS.len());
        for (name, value) in breakdown {
            assert!(
                (0.0..=1.0).contains(&value),
                "{name} produced {value}, outside 0..1"
            );
        }
    }

    #[test]
    fn sequence_similarity_handles_lengths() {
        assert_eq!(sequence_similarity::<i32>(&[], &[]), 1.0);
        assert_eq!(sequence_similarity(&[1, 2], &[1, 2]), 1.0);
        assert_eq!(sequence_similarity(&[1, 2], &[1, 3]), 0.5);
        assert_eq!(sequence_similarity(&[1], &[1, 1]), 0.5);
    }
}
