//! What a good wrap *is*, for each of the six loop intents.
//!
//! The brief is explicit that not every loop should be pushed to end on the
//! tonic, so there is no single "loop quality" number here. Each intent has its
//! own criteria, each criterion is named and weighted, and the weights are
//! reported alongside the score so a caller can see why a loop was judged the
//! way it was.
//!
//! The two cases the brief singles out fall straight out of that:
//!
//! * `closed_tonic` weights the functional wrap most heavily, so a V-to-I
//!   across the seam scores at the top of its range;
//! * `modal_drone` gives the functional wrap **no positive weight at all** and
//!   instead rewards a stable collection, a held common tone or pedal, and a
//!   smooth seam — so a modal loop that never produces a dominant can still
//!   score 1.0.

use crate::wrap::WrapObservation;
use music_domain::prelude::*;

/// Integrity facts the intent criteria need, gathered once by the audit.
#[derive(Clone, Copy, Debug, Default)]
pub struct SeamIntegrity {
    /// How many notes hang past the loop end without being asked to.
    pub hanging: usize,
    /// How many notes sound on both sides of the seam.
    pub crossing: usize,
    /// True when there is material before the loop start.
    pub pickup: bool,
    /// True when the material occupies exactly the requested span.
    pub length_exact: bool,
    /// True when the passage is centred on a mode rather than a key.
    pub modal: bool,
}

/// One intent's verdict, with the criteria that produced it.
#[derive(Clone, Debug)]
pub struct IntentFit {
    /// The intent that was evaluated.
    pub intent: LoopIntent,
    /// How well the material serves that intent, `0.0..=1.0`.
    pub fit: f64,
    /// Named criteria and their raw `0.0..=1.0` values, in a fixed order.
    pub criteria: Vec<(&'static str, f64)>,
    /// Weights applied to those criteria, parallel to `criteria`.
    pub weights: Vec<f64>,
    /// One sentence describing the verdict.
    pub summary: String,
}

impl IntentFit {
    /// The value of a named criterion, or `0.0` when this intent does not use
    /// it.
    pub fn criterion(&self, name: &str) -> f64 {
        self.criteria
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
            .unwrap_or(0.0)
    }

    /// JSON form, for the decision trace.
    pub fn to_json(&self) -> qjson::Json {
        let mut m = qjson::JsonMap::new();
        m.insert("intent", qjson::Json::Str(self.intent.id().to_string()));
        m.insert("fit", qjson::Json::Float(self.fit));
        let mut cs = qjson::JsonMap::new();
        for ((name, value), weight) in self.criteria.iter().zip(self.weights.iter()) {
            cs.insert(
                (*name).to_string(),
                qjson::json_obj! { "value" => *value, "weight" => *weight },
            );
        }
        m.insert("criteria", qjson::Json::Obj(cs));
        m.insert("summary", qjson::Json::Str(self.summary.clone()));
        qjson::Json::Obj(m)
    }
}

/// True when the loop's last chord carries the named function.
fn final_is(o: &WrapObservation, f: HarmonicFunction) -> bool {
    o.final_function == Some(f)
}

/// True when the loop's first chord carries the named function.
fn first_is(o: &WrapObservation, f: HarmonicFunction) -> bool {
    o.first_function == Some(f)
}

/// How little the seam is heard as a jump, `0.0..=1.0`.
fn seam_smoothness(o: &WrapObservation) -> f64 {
    if o.end_pitches.is_empty() || o.start_pitches.is_empty() {
        return 0.5;
    }
    let leap = f64::from(o.max_leap);
    if leap <= 2.0 {
        1.0
    } else {
        (1.0 - (leap - 2.0) / 10.0).clamp(0.0, 1.0)
    }
}

/// How well the bass crosses the seam, `0.0..=1.0`.
///
/// A step and a perfect fifth both read as intended motion; anything wider is
/// where the listener hears the loop point.
fn bass_continuity(o: &WrapObservation) -> f64 {
    match o.bass_interval {
        None => 0.5,
        Some(i) => {
            let a = i.abs();
            if a == 0 {
                1.0
            } else if a <= 2 {
                0.95
            } else if a == 5 || a == 7 {
                0.9
            } else if a <= 7 {
                0.7
            } else if a <= 12 {
                0.3
            } else {
                0.1
            }
        }
    }
}

/// The strongest continuity device present at the seam, `0.0..=1.0`.
fn continuity(o: &WrapObservation) -> f64 {
    if o.pedal_continues {
        1.0
    } else if o.common_tone_retained {
        0.9
    } else if o.stepwise_available {
        0.75
    } else if o.common_tone_available {
        0.5
    } else {
        0.1
    }
}

/// Whether the harmonic rhythm survives the seam, `0.0..=1.0`.
fn rhythm_stability(o: &WrapObservation) -> f64 {
    match (o.slot_before, o.slot_after) {
        (Some(_), Some(_)) => {
            if o.harmonic_rhythm_changes {
                0.2
            } else {
                1.0
            }
        }
        _ => 0.7,
    }
}

/// How completely the wrap behaves as an authentic cadence, `0.0..=1.0`.
fn functional_wrap(o: &WrapObservation) -> f64 {
    if final_is(o, HarmonicFunction::Dominant) && first_is(o, HarmonicFunction::Tonic) {
        1.0
    } else if final_is(o, HarmonicFunction::Dominant) {
        0.6
    } else if final_is(o, HarmonicFunction::Predominant) && first_is(o, HarmonicFunction::Tonic) {
        0.55
    } else {
        0.0
    }
}

/// How much room the final chord leaves to move on, `0.0..=1.0`.
fn openness(o: &WrapObservation) -> f64 {
    match o.final_function {
        Some(HarmonicFunction::Dominant) => 1.0,
        Some(HarmonicFunction::Applied) => 0.95,
        Some(HarmonicFunction::Predominant) => 0.9,
        Some(HarmonicFunction::Chromatic) | Some(HarmonicFunction::Modal) => 0.8,
        Some(HarmonicFunction::Passing) | Some(HarmonicFunction::Neighbor) => 0.8,
        Some(HarmonicFunction::Pedal) => 0.6,
        Some(HarmonicFunction::Unclassified) => 0.7,
        Some(HarmonicFunction::Tonic) => 0.2,
        None => 0.6,
    }
}

/// How conclusively the material stops, `0.0..=1.0`.
fn conclusiveness(o: &WrapObservation) -> f64 {
    match o.final_function {
        Some(HarmonicFunction::Tonic) => 1.0,
        Some(HarmonicFunction::Pedal) => 0.6,
        Some(_) => 0.4,
        None => 0.5,
    }
}

/// How intact the seam is, ignoring what the harmony does, `0.0..=1.0`.
fn seam_integrity(i: &SeamIntegrity) -> f64 {
    let mut v: f64 = 1.0;
    if i.hanging > 0 {
        v -= 0.6;
    }
    if i.pickup {
        v -= 0.25;
    }
    if !i.length_exact {
        v -= 0.3;
    }
    v.clamp(0.0, 1.0)
}

/// Combines named, weighted criteria into a `0.0..=1.0` fit.
fn combine(intent: LoopIntent, parts: &[(&'static str, f64, f64)], summary: String) -> IntentFit {
    let total_weight: f64 = parts.iter().map(|(_, _, w)| *w).sum();
    let fit = if total_weight <= 0.0 {
        0.0
    } else {
        parts.iter().map(|(_, v, w)| v * w).sum::<f64>() / total_weight
    };
    IntentFit {
        intent,
        fit: fit.clamp(0.0, 1.0),
        criteria: parts.iter().map(|(n, v, _)| (*n, *v)).collect(),
        weights: parts.iter().map(|(_, _, w)| *w).collect(),
        summary,
    }
}

/// Judges the material against one loop intent.
pub fn evaluate(intent: LoopIntent, o: &WrapObservation, i: &SeamIntegrity) -> IntentFit {
    let smooth = seam_smoothness(o);
    let bass = bass_continuity(o);
    let cont = continuity(o);
    let rhythm = rhythm_stability(o);
    let integrity = seam_integrity(i);

    match intent {
        LoopIntent::ClosedTonic => {
            let wrap = if final_is(o, HarmonicFunction::Dominant)
                && first_is(o, HarmonicFunction::Tonic)
            {
                1.0
            } else if final_is(o, HarmonicFunction::Predominant)
                && first_is(o, HarmonicFunction::Tonic)
            {
                0.7
            } else if first_is(o, HarmonicFunction::Tonic) {
                0.55
            } else if o.final_function.is_none() && o.first_function.is_none() {
                0.5
            } else {
                0.25
            };
            let summary = if wrap >= 1.0 {
                format!(
                    "the loop ends on {} and begins on {}, so the wrap is an authentic cadence",
                    o.final_symbol.clone().unwrap_or_else(|| "V".into()),
                    o.first_symbol.clone().unwrap_or_else(|| "I".into())
                )
            } else if wrap >= 0.55 {
                "the loop returns to the tonic but the last chord does not lead there".to_string()
            } else {
                "the wrap does not resolve to a tonic, which is what this intent asked for"
                    .to_string()
            };
            combine(
                intent,
                &[
                    ("functional_wrap", wrap, 0.55),
                    ("bass_continuity", bass, 0.15),
                    ("seam_smoothness", smooth, 0.12),
                    ("harmonic_rhythm_stability", rhythm, 0.08),
                    ("seam_integrity", integrity, 0.10),
                ],
                summary,
            )
        }

        LoopIntent::OpenDominant => {
            let ends_open = if final_is(o, HarmonicFunction::Dominant) {
                1.0
            } else if final_is(o, HarmonicFunction::Applied) {
                0.85
            } else if final_is(o, HarmonicFunction::Predominant) {
                0.6
            } else if final_is(o, HarmonicFunction::Tonic) {
                0.15
            } else {
                0.5
            };
            // An unresolved tendency tone is the point of this intent, so its
            // presence is credited and its absence is merely neutral.
            let tension = if o.unresolved_tendencies.is_empty() {
                0.6
            } else {
                1.0
            };
            let restart = if first_is(o, HarmonicFunction::Tonic) {
                1.0
            } else if o.common_tone_available {
                0.7
            } else {
                0.4
            };
            let summary = if ends_open >= 1.0 {
                "the loop ends unresolved on a dominant, which is what this intent asked for"
                    .to_string()
            } else {
                "the loop closes rather than staying open".to_string()
            };
            combine(
                intent,
                &[
                    ("ends_open", ends_open, 0.50),
                    ("standing_tension", tension, 0.20),
                    ("restart_is_plausible", restart, 0.15),
                    ("seam_integrity", integrity, 0.15),
                ],
                summary,
            )
        }

        LoopIntent::ModalDrone => {
            let modal = if i.modal { 1.0 } else { 0.1 };
            // Explicitly *not* a criterion: a dominant-to-tonic wrap. It is
            // subtracted rather than rewarded, because a strong cadence turns a
            // drone into a closed tonal loop.
            let non_functional = 1.0 - functional_wrap(o);
            let summary = if i.modal {
                format!(
                    "a modal centre is active and the seam is carried by {}",
                    if o.pedal_continues {
                        "a pedal that never re-articulates"
                    } else if o.common_tone_retained {
                        "a retained common tone"
                    } else if o.stepwise_available {
                        "stepwise connection"
                    } else {
                        "nothing in particular"
                    }
                )
            } else {
                "the material reads as a functional key rather than a modal centre".to_string()
            };
            combine(
                intent,
                &[
                    ("modal_center", modal, 0.30),
                    (
                        "collection_stability",
                        o.collection_stability.max(0.5),
                        0.20,
                    ),
                    ("pedal_or_common_tone", cont, 0.20),
                    ("seam_smoothness", smooth, 0.15),
                    ("non_functional_wrap", non_functional, 0.15),
                ],
                summary,
            )
        }

        LoopIntent::SeamlessColor => {
            let shared = if o.common_tone_retained {
                1.0
            } else if o.common_tone_available {
                0.6
            } else {
                0.15
            };
            let summary = if shared >= 1.0 && smooth >= 1.0 {
                "the seam shares pitch classes and moves by step, so the loop point is inaudible"
                    .to_string()
            } else {
                "the seam is audible: the two sonorities share little and move by leap".to_string()
            };
            combine(
                intent,
                &[
                    ("seam_smoothness", smooth, 0.30),
                    ("common_tones", shared, 0.25),
                    ("bass_continuity", bass, 0.20),
                    ("harmonic_rhythm_stability", rhythm, 0.10),
                    ("seam_integrity", integrity, 0.15),
                ],
                summary,
            )
        }

        LoopIntent::TransitionReady => {
            let open = openness(o);
            let summary = if open >= 0.8 {
                "the last chord can move on, so the loop can be dropped into a longer arrangement"
                    .to_string()
            } else {
                "the loop closes on a tonic, which forces an edit at the point the caller wanted \
                 flexibility"
                    .to_string()
            };
            combine(
                intent,
                &[
                    ("leaves_an_opening", open, 0.50),
                    ("seam_integrity", integrity, 0.20),
                    ("bass_continuity", bass, 0.15),
                    ("seam_smoothness", smooth, 0.15),
                ],
                summary,
            )
        }

        LoopIntent::OneShotEnding => {
            // Wrap criteria are deliberately absent. A stinger is meant to
            // stop; scoring it on loop continuity would penalise it for
            // succeeding at its actual job.
            let stops = conclusiveness(o);
            let self_contained = if i.crossing > 0 { 0.5 } else { 1.0 };
            let summary = if stops >= 1.0 {
                "the material ends on a tonic and is not scored for wrap continuity".to_string()
            } else {
                "the material does not close, which a one-shot ending usually should".to_string()
            };
            combine(
                intent,
                &[
                    ("ends_conclusively", stops, 0.70),
                    ("self_contained", self_contained, 0.20),
                    ("length_is_exact", f64::from(i.length_exact), 0.10),
                ],
                summary,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn functional_wrap_observation() -> WrapObservation {
        WrapObservation {
            final_function: Some(HarmonicFunction::Dominant),
            first_function: Some(HarmonicFunction::Tonic),
            final_symbol: Some("G7".into()),
            first_symbol: Some("C".into()),
            end_pitches: vec![55, 59, 62, 65],
            start_pitches: vec![48, 52, 55, 60],
            bass_end: Some(55),
            bass_start: Some(48),
            bass_interval: Some(-7),
            common_tone_available: true,
            common_tone_retained: true,
            stepwise_available: true,
            slot_before: Some(BeatTime::from_quarters(4)),
            slot_after: Some(BeatTime::from_quarters(4)),
            collection_stability: 1.0,
            ..WrapObservation::default()
        }
    }

    fn modal_observation() -> WrapObservation {
        WrapObservation {
            final_function: Some(HarmonicFunction::Tonic),
            first_function: Some(HarmonicFunction::Modal),
            end_pitches: vec![60, 64, 67],
            start_pitches: vec![62, 66, 69],
            bass_end: Some(60),
            bass_start: Some(62),
            bass_interval: Some(2),
            max_leap: 2,
            total_motion: 6,
            stepwise_available: true,
            slot_before: Some(BeatTime::from_quarters(4)),
            slot_after: Some(BeatTime::from_quarters(4)),
            collection_stability: 1.0,
            ..WrapObservation::default()
        }
    }

    fn clean() -> SeamIntegrity {
        SeamIntegrity {
            hanging: 0,
            crossing: 0,
            pickup: false,
            length_exact: true,
            modal: false,
        }
    }

    #[test]
    fn closed_tonic_rates_a_dominant_to_tonic_wrap_at_the_top() {
        let fit = evaluate(
            LoopIntent::ClosedTonic,
            &functional_wrap_observation(),
            &clean(),
        );
        assert!(fit.fit > 0.9, "{:?}", fit);
        assert_eq!(fit.criterion("functional_wrap"), 1.0);
    }

    #[test]
    fn modal_drone_does_not_use_the_functional_wrap_as_a_criterion() {
        let fit = evaluate(
            LoopIntent::ModalDrone,
            &functional_wrap_observation(),
            &clean(),
        );
        assert!(
            fit.criteria.iter().all(|(n, _)| *n != "functional_wrap"),
            "modal_drone must not be scored on dominant-to-tonic motion"
        );
    }

    #[test]
    fn modal_drone_scores_a_modal_loop_highly_without_any_dominant() {
        let mut integrity = clean();
        integrity.modal = true;
        let fit = evaluate(LoopIntent::ModalDrone, &modal_observation(), &integrity);
        assert!(fit.fit > 0.85, "{:?}", fit);
    }

    #[test]
    fn one_shot_ending_ignores_the_wrap_entirely() {
        let mut o = functional_wrap_observation();
        o.final_function = Some(HarmonicFunction::Tonic);
        let fit = evaluate(LoopIntent::OneShotEnding, &o, &clean());
        assert!(fit.fit > 0.9);
        assert!(fit
            .criteria
            .iter()
            .all(|(n, _)| !n.contains("seam_smoothness")));
    }

    #[test]
    fn transition_ready_prefers_an_open_ending() {
        let open = evaluate(
            LoopIntent::TransitionReady,
            &functional_wrap_observation(),
            &clean(),
        );
        let mut closed_obs = functional_wrap_observation();
        closed_obs.final_function = Some(HarmonicFunction::Tonic);
        let closed = evaluate(LoopIntent::TransitionReady, &closed_obs, &clean());
        assert!(open.fit > closed.fit);
    }

    #[test]
    fn seam_integrity_falls_with_a_hanging_note() {
        let mut broken = clean();
        broken.hanging = 1;
        assert!(seam_integrity(&broken) < seam_integrity(&clean()));
    }

    #[test]
    fn every_intent_produces_a_bounded_fit() {
        let o = functional_wrap_observation();
        for intent in [
            LoopIntent::ClosedTonic,
            LoopIntent::OpenDominant,
            LoopIntent::ModalDrone,
            LoopIntent::SeamlessColor,
            LoopIntent::TransitionReady,
            LoopIntent::OneShotEnding,
        ] {
            let fit = evaluate(intent, &o, &clean());
            assert!((0.0..=1.0).contains(&fit.fit), "{intent:?} {}", fit.fit);
            assert_eq!(fit.criteria.len(), fit.weights.len());
            assert!(!fit.summary.is_empty());
        }
    }

    #[test]
    fn the_fit_json_names_every_criterion() {
        let fit = evaluate(
            LoopIntent::SeamlessColor,
            &functional_wrap_observation(),
            &clean(),
        );
        let json = fit.to_json();
        let criteria = json.get("criteria").and_then(qjson::Json::as_obj).unwrap();
        assert_eq!(criteria.len(), fit.criteria.len());
    }
}
