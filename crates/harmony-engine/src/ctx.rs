//! The shared frame every generation stage reads.
//!
//! One [`EngineContext`] is built per request and threaded through stages 5 to
//! 11, so the knowledge base, the resolved profile, the tonal frame and the
//! melody's per-slot facts are computed once rather than rediscovered.

use crate::error::HarmonyError;
use crate::keyctx::KeyContext;
use crate::params::GenerateParams;
use music_analysis::grid::GridSlot;
use music_analysis::report::{Analysis, Strictness};
use music_domain::prelude::*;
use theory_kb::{KnowledgeBase, ResolvedProfile, RuleEngine};

/// How much a raw knowledge-base rule delta contributes to a score component.
///
/// Rule deltas are authored on a `-1000.0 ..= +3.0` scale where the extremes
/// mean "reject outright"; the engine's own heuristics live in `0.0 ..= 1.0`.
/// Scaling the deltas keeps both visible in the same vector without either one
/// drowning the other, and keeps the raw components inspectable.
pub const RULE_DELTA_SCALE: f64 = 0.1;

/// One melody note as the harmony stages need to see it.
#[derive(Clone, Debug, PartialEq)]
pub struct MelodyNote {
    /// The note's id in the analysis.
    pub id: NoteId,
    /// Sounding pitch.
    pub midi: i32,
    /// Pitch class.
    pub pc: i32,
    /// Onset in quarter notes.
    pub onset: BeatTime,
    /// Duration in quarter notes.
    pub duration: BeatTime,
    /// Structural salience, `0.0..=1.0`.
    pub salience: f64,
    /// True when the analysis calls the note structural.
    pub structural: bool,
    /// Best non-chord-tone confidence the analysis reported, `0.0` when none.
    pub nct_confidence: f64,
    /// Metric weight of the onset.
    pub metric_weight: f64,
}

impl MelodyNote {
    /// How much this note's agreement with the harmony counts.
    ///
    /// A decorative note still counts — a chord that clashes with everything is
    /// worse than one that clashes with a passing tone — but far less than a
    /// structural arrival.
    pub fn weight(&self) -> f64 {
        let base = if self.structural { 1.0 } else { 0.35 };
        base * (0.35 + 0.65 * self.salience.clamp(0.0, 1.0))
    }
}

/// Everything the generation stages share.
pub struct EngineContext<'a> {
    /// The knowledge bundle.
    pub kb: &'a KnowledgeBase,
    /// The resolved style profile.
    pub profile: &'a ResolvedProfile,
    /// The analysis being harmonised.
    pub analysis: &'a Analysis,
    /// The tonal frame.
    pub key: KeyContext,
    /// The request.
    pub params: &'a GenerateParams,
    /// The rule engine bound to `kb` and `profile`.
    pub engine: RuleEngine<'a>,
    /// Melody notes in analysis order.
    pub melody: Vec<MelodyNote>,
    /// Profile scalars and vocabulary lists, read once.
    ///
    /// These are consulted for every option in every slot, and reading them
    /// out of the profile's JSON each time dominated the generation cost.
    settings: Settings,
}

/// The profile fields the engine reads on every option.
#[derive(Clone, Debug, Default)]
struct Settings {
    extension_density: f64,
    chromaticism_target: f64,
    complexity_target: f64,
    functional_strength: f64,
    modal_tolerance: f64,
    voice_leading_strictness: f64,
    alteration_preference: f64,
    raw_extension_density: f64,
    core: Vec<String>,
    occasional: Vec<String>,
    avoided: Vec<String>,
}

impl<'a> EngineContext<'a> {
    /// Builds the frame, failing when the analysis carries no key reading.
    pub fn new(
        kb: &'a KnowledgeBase,
        profile: &'a ResolvedProfile,
        analysis: &'a Analysis,
        params: &'a GenerateParams,
    ) -> Result<EngineContext<'a>, HarmonyError> {
        let key = KeyContext::from_analysis(kb, analysis)?;
        let melody = collect_melody(analysis);
        let field = |name: &str, default: f64| -> f64 {
            profile.field_f64(name).unwrap_or(default).clamp(0.0, 1.0)
        };
        let vocabulary = |tier: &str| -> Vec<String> {
            profile
                .field(&format!("harmonic_vocabulary.{tier}"))
                .and_then(qjson::Json::as_arr)
                .map(|a| {
                    a.iter()
                        .filter_map(qjson::Json::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        let raw_extension_density = field("extension_density", 0.4);
        let settings = Settings {
            extension_density: (0.5 * raw_extension_density + 0.5 * params.extension_density)
                .clamp(0.0, 1.0),
            chromaticism_target: (0.5 * field("chromaticism_preference", 0.3)
                + 0.5 * params.chromaticism)
                .clamp(0.0, 1.0),
            complexity_target: (0.5 * field("default_complexity", 0.5) + 0.5 * params.complexity)
                .clamp(0.0, 1.0),
            functional_strength: field("functional_strength", 0.7),
            modal_tolerance: field("modal_tolerance", 0.4),
            voice_leading_strictness: field("voice_leading_strictness", 0.6),
            alteration_preference: field("alteration_preference", 0.4),
            raw_extension_density,
            core: vocabulary("core"),
            occasional: vocabulary("occasional"),
            avoided: vocabulary("avoided"),
        };
        Ok(EngineContext {
            kb,
            profile,
            analysis,
            key,
            params,
            engine: RuleEngine::new(kb, profile),
            melody,
            settings,
        })
    }

    /// The melody notes whose onsets fall inside a slot.
    pub fn slot_melody(&self, slot: &GridSlot) -> Vec<&MelodyNote> {
        self.melody
            .iter()
            .filter(|n| n.onset >= slot.start && n.onset < slot.end)
            .collect()
    }

    /// The melody note sounding at a position, preferring one that starts there.
    pub fn melody_at(&self, qn: BeatTime) -> Option<&MelodyNote> {
        self.melody.iter().find(|n| n.onset == qn).or_else(|| {
            self.melody
                .iter()
                .find(|n| n.onset <= qn && qn < n.onset + n.duration)
        })
    }

    /// The profile's own extension-density preference blended with the request's.
    pub fn extension_density(&self) -> f64 {
        self.settings.extension_density
    }

    /// The profile's declared extension density, unblended.
    pub fn profile_extension_density(&self) -> f64 {
        self.settings.raw_extension_density
    }

    /// How much the profile likes altered tones.
    pub fn alteration_preference(&self) -> f64 {
        self.settings.alteration_preference
    }

    /// The profile's own chromaticism preference blended with the request's.
    pub fn chromaticism_target(&self) -> f64 {
        self.settings.chromaticism_target
    }

    /// The profile's own complexity preference blended with the request's.
    pub fn complexity_target(&self) -> f64 {
        self.settings.complexity_target
    }

    /// How strongly the profile expects functional resolution.
    pub fn functional_strength(&self) -> f64 {
        self.settings.functional_strength
    }

    /// How willingly the profile accepts modal, non-functional harmony.
    pub fn modal_tolerance(&self) -> f64 {
        self.settings.modal_tolerance
    }

    /// How strictly the profile polices voice leading.
    pub fn voice_leading_strictness(&self) -> f64 {
        self.settings.voice_leading_strictness
    }

    /// True when the profile treats parallel perfect intervals as idiomatic
    /// rather than as faults.
    pub fn parallels_are_idiomatic(&self) -> bool {
        matches!(
            self.profile.field_str("parallel_motion_treatment"),
            Some("idiomatic") | Some("free") | Some("allowed")
        )
    }

    /// True when the request asked for common-practice strictness.
    pub fn is_common_practice_strict(&self) -> bool {
        self.params.strictness == Strictness::CommonPracticeStrict
    }

    /// True when the profile treats planing and constant structure as a normal
    /// device, which switches several voice-leading penalties off.
    pub fn planing_context(&self) -> bool {
        self.key.is_modal && self.modal_tolerance() >= 0.6
    }

    /// Chord-quality ids the profile lists at the given vocabulary tier.
    pub fn vocabulary(&self, tier: &str) -> &[String] {
        match tier {
            "core" => &self.settings.core,
            "occasional" => &self.settings.occasional,
            "avoided" => &self.settings.avoided,
            _ => &[],
        }
    }

    /// A style-match score for a chord quality: what the profile thinks of it.
    pub fn vocabulary_rank(&self, quality_id: &str) -> f64 {
        if self.settings.core.iter().any(|q| q == quality_id) {
            1.0
        } else if self.settings.occasional.iter().any(|q| q == quality_id) {
            0.7
        } else if self.settings.avoided.iter().any(|q| q == quality_id) {
            0.05
        } else {
            0.4
        }
    }
}

/// Extracts the melody facts from the analysis.
fn collect_melody(an: &Analysis) -> Vec<MelodyNote> {
    an.extraction
        .melody
        .notes
        .iter()
        .map(|n| {
            let nct_confidence = an
                .nct(n.id)
                .iter()
                .map(|h| {
                    if h.kind == NctKind::ChordTone {
                        0.0
                    } else {
                        h.confidence
                    }
                })
                .fold(0.0f64, f64::max);
            MelodyNote {
                id: n.id,
                midi: n.midi,
                pc: n.midi.rem_euclid(12),
                onset: n.onset,
                duration: n.duration,
                salience: an.salience.total(n.id),
                structural: an.salience.is_structural(n.id),
                nct_confidence,
                metric_weight: an.extraction.melody.time_map.metric_weight(n.onset),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::testing;

    #[test]
    fn context_reads_the_profile_fields_it_relies_on() {
        let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
        let ctx = h.context();
        assert!(ctx.extension_density() > 0.0);
        assert!(ctx.chromaticism_target() > 0.0);
        assert!(ctx.complexity_target() > 0.0);
        assert!(ctx.functional_strength() > 0.0);
        assert!(ctx.voice_leading_strictness() > 0.0);
        assert!(!ctx.vocabulary("core").is_empty());
    }

    #[test]
    fn vocabulary_rank_orders_core_above_avoided() {
        let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
        let ctx = h.context();
        assert!(ctx.vocabulary_rank("major7") > ctx.vocabulary_rank("diminished7"));
        assert!(ctx.vocabulary_rank("diminished7") > ctx.vocabulary_rank("power"));
    }

    #[test]
    fn every_profile_declares_the_scalar_fields_the_engine_reads() {
        for id in testing::PROFILE_IDS {
            let h = testing::harness("melodies/eight_bar_c_major", id);
            let ctx = h.context();
            for key in [
                "functional_strength",
                "modal_tolerance",
                "chromaticism_preference",
                "extension_density",
                "voice_leading_strictness",
                "default_complexity",
                "candidate_diversity",
            ] {
                assert!(
                    ctx.profile.field_f64(key).is_some(),
                    "profile {id} is missing {key}"
                );
            }
            assert!(
                ctx.profile.field_str("parallel_motion_treatment").is_some(),
                "profile {id} is missing parallel_motion_treatment"
            );
        }
    }

    #[test]
    fn melody_notes_carry_salience_and_metric_weight() {
        let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
        let ctx = h.context();
        assert!(!ctx.melody.is_empty());
        assert!(ctx.melody.iter().any(|n| n.structural));
        assert!(ctx.melody.iter().all(|n| (0.0..=1.0).contains(&n.salience)));
        assert!(ctx.melody[0].weight() > 0.0);
    }

    #[test]
    fn slot_melody_partitions_the_line() {
        let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
        let ctx = h.context();
        let mut seen = 0usize;
        for slot in &ctx.analysis.grid.slots {
            seen += ctx.slot_melody(slot).len();
        }
        assert_eq!(seen, ctx.melody.len());
    }
}
