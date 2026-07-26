//! Request parameters, search bounds and the cancellation flag.
//!
//! Everything a caller controls lives here so that one canonical JSON form —
//! [`GenerateParams::canonical_json`] — captures every input that can change
//! the output. Candidate ids are derived from it, which is what makes
//! determinism testable.

use crate::error::HarmonyError;
use music_analysis::grid::GridMode;
use music_analysis::report::Strictness;
use music_domain::prelude::*;
use qjson::{Json, JsonMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Cooperative cancellation, shared across threads.
///
/// `reaper-ipc` defines an identical flag for its own transport; `harmony-engine`
/// does not depend on that crate, so it carries its own with the same shape.
#[derive(Clone, Debug, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    /// A flag that has not been raised.
    pub fn new() -> CancelFlag {
        CancelFlag::default()
    }

    /// Raises the flag; every subsequent [`CancelFlag::is_cancelled`] is true.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// True once [`CancelFlag::cancel`] has been called.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// `Err(CANCELLED)` when the flag is raised, `Ok(())` otherwise.
    pub fn check(&self) -> Result<(), HarmonyError> {
        if self.is_cancelled() {
            Err(HarmonyError::cancelled())
        } else {
            Ok(())
        }
    }
}

/// How the bass line moves.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum BassMotion {
    /// Take the profile's declared default.
    #[default]
    Auto,
    /// Chord roots only.
    Roots,
    /// Follow the chord's inversion.
    Inversions,
    /// Prefer the chord tone nearest the previous bass note.
    Stepwise,
    /// Hold the tonic underneath everything.
    Pedal,
    /// Repeat a rhythmic figure built from root and fifth.
    Ostinato,
    /// Move against the melody's direction.
    ContraryMotion,
}

impl BassMotion {
    /// Stable identifier.
    pub fn id(self) -> &'static str {
        match self {
            BassMotion::Auto => "auto",
            BassMotion::Roots => "roots",
            BassMotion::Inversions => "inversions",
            BassMotion::Stepwise => "stepwise",
            BassMotion::Pedal => "pedal",
            BassMotion::Ostinato => "ostinato",
            BassMotion::ContraryMotion => "contrary_motion",
        }
    }

    /// Reads the wire form.
    pub fn parse(s: &str) -> Option<BassMotion> {
        BassMotion::all().iter().copied().find(|m| m.id() == s)
    }

    /// Every mode, in declaration order.
    pub fn all() -> &'static [BassMotion] {
        &[
            BassMotion::Auto,
            BassMotion::Roots,
            BassMotion::Inversions,
            BassMotion::Stepwise,
            BassMotion::Pedal,
            BassMotion::Ostinato,
            BassMotion::ContraryMotion,
        ]
    }
}

/// Countermelody request.
#[derive(Clone, Debug, PartialEq)]
pub struct CountermelodyParams {
    /// Whether to generate one at all.
    pub enabled: bool,
    /// Onsets per bar, `0.0..=1.0` as a fraction of the melody's density.
    pub density: f64,
    /// The arrangement role the line plays.
    pub role: ArrangementRole,
}

impl Default for CountermelodyParams {
    fn default() -> Self {
        CountermelodyParams {
            enabled: false,
            density: 0.4,
            role: ArrangementRole::Counterlead,
        }
    }
}

impl CountermelodyParams {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("enabled", Json::Bool(self.enabled));
        m.insert("density", Json::Float(round6(self.density)));
        m.insert("role", Json::Str(self.role.id().to_string()));
        Json::Obj(m)
    }
}

/// Everything the caller controls about candidate generation.
#[derive(Clone, Debug, PartialEq)]
pub struct GenerateParams {
    /// Style profile id.
    pub profile_id: String,
    /// How many candidates to return, `1..=8`.
    pub candidate_count: usize,
    /// Every melody pitch and onset must survive unchanged.
    pub preserve_melody: bool,
    /// Melody timing must survive unchanged.
    pub preserve_rhythm: bool,
    /// Harmonic-rhythm override; [`GridMode::Auto`] keeps the analysis grid.
    pub grid: GridMode,
    /// Vocabulary complexity target, `0.0..=1.0`.
    pub complexity: f64,
    /// Chromaticism target, `0.0..=1.0`.
    pub chromaticism: f64,
    /// Extension-density target, `0.0..=1.0`.
    pub extension_density: f64,
    /// Bass behaviour.
    pub bass_motion: BassMotion,
    /// Countermelody request.
    pub countermelody: CountermelodyParams,
    /// Loop intent, when the material is a loop.
    pub loop_intent: Option<LoopIntent>,
    /// How much benefit of the doubt to give ambiguous readings.
    pub strictness: Strictness,
    /// Tie-breaking seed. It never changes a genuine ordering.
    pub seed: u64,
    /// Voices in the harmony part, `2..=8`.
    pub voice_count: usize,
}

impl Default for GenerateParams {
    fn default() -> Self {
        GenerateParams {
            profile_id: "common_practice".to_string(),
            candidate_count: 3,
            preserve_melody: true,
            preserve_rhythm: true,
            grid: GridMode::Auto,
            complexity: 0.5,
            chromaticism: 0.35,
            extension_density: 0.4,
            bass_motion: BassMotion::Auto,
            countermelody: CountermelodyParams::default(),
            loop_intent: None,
            strictness: Strictness::Balanced,
            seed: 0,
            voice_count: 4,
        }
    }
}

impl GenerateParams {
    /// Rejects out-of-range requests before any work is done.
    pub fn validate(&self) -> Result<(), HarmonyError> {
        if self.profile_id.is_empty() {
            return Err(HarmonyError::invalid_argument(
                "profile_id must not be empty",
            ));
        }
        if !(1..=8).contains(&self.candidate_count) {
            return Err(HarmonyError::invalid_argument(format!(
                "candidate_count must be 1..=8, got {}",
                self.candidate_count
            )));
        }
        if !(2..=8).contains(&self.voice_count) {
            return Err(HarmonyError::invalid_argument(format!(
                "voice_count must be 2..=8, got {}",
                self.voice_count
            )));
        }
        for (name, value) in [
            ("complexity", self.complexity),
            ("chromaticism", self.chromaticism),
            ("extension_density", self.extension_density),
            ("countermelody.density", self.countermelody.density),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(HarmonyError::invalid_argument(format!(
                    "{name} must be within 0.0..=1.0, got {value}"
                )));
            }
        }
        Ok(())
    }

    /// The canonical JSON that candidate ids are derived from.
    ///
    /// Every field that can change the output appears here, so two requests
    /// with the same canonical form are guaranteed the same candidates.
    pub fn canonical_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("profile_id", Json::Str(self.profile_id.clone()));
        m.insert("candidate_count", Json::Int(self.candidate_count as i64));
        m.insert("preserve_melody", Json::Bool(self.preserve_melody));
        m.insert("preserve_rhythm", Json::Bool(self.preserve_rhythm));
        m.insert("grid", self.grid.to_json());
        m.insert("complexity", Json::Float(round6(self.complexity)));
        m.insert("chromaticism", Json::Float(round6(self.chromaticism)));
        m.insert(
            "extension_density",
            Json::Float(round6(self.extension_density)),
        );
        m.insert("bass_motion", Json::Str(self.bass_motion.id().to_string()));
        m.insert("countermelody", self.countermelody.to_json());
        m.insert(
            "loop_intent",
            match self.loop_intent {
                Some(i) => Json::Str(i.id().to_string()),
                None => Json::Null,
            },
        );
        m.insert("strictness", Json::Str(self.strictness.id().to_string()));
        m.insert("seed", Json::Int(self.seed as i64));
        m.insert("voice_count", Json::Int(self.voice_count as i64));
        Json::Obj(m)
    }

    /// Builder helper used throughout the tests.
    pub fn with_profile(mut self, id: &str) -> GenerateParams {
        self.profile_id = id.to_string();
        self
    }

    /// Builder helper used throughout the tests.
    pub fn with_seed(mut self, seed: u64) -> GenerateParams {
        self.seed = seed;
        self
    }

    /// Builder helper used throughout the tests.
    pub fn with_candidate_count(mut self, n: usize) -> GenerateParams {
        self.candidate_count = n;
        self
    }
}

/// Hard bounds on the path search, so a pathological input cannot run away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchConfig {
    /// Partial paths retained at each slot.
    pub beam_width: usize,
    /// Chord options considered per slot.
    pub max_options_per_slot: usize,
    /// Total transitions the search may expand before it stops widening.
    pub max_paths: usize,
    /// Complete paths returned for the diversity stage to choose among.
    pub diversity_paths: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        SearchConfig {
            beam_width: 24,
            max_options_per_slot: 14,
            max_paths: 200_000,
            diversity_paths: 24,
        }
    }
}

impl SearchConfig {
    /// Rejects a configuration that would disable the search entirely.
    pub fn validate(&self) -> Result<(), HarmonyError> {
        if self.beam_width == 0 || self.max_options_per_slot == 0 || self.diversity_paths == 0 {
            return Err(HarmonyError::invalid_argument(
                "beam_width, max_options_per_slot and diversity_paths must all be positive",
            ));
        }
        Ok(())
    }
}

/// Voicing-stage request.
#[derive(Clone, Debug, PartialEq)]
pub struct VoicingParams {
    /// Voices in the realised chord.
    pub voice_count: usize,
    /// Voicing families to draw templates from; empty means "any the profile allows".
    pub families: Vec<VoicingFamily>,
    /// Lowest MIDI pitch any voice may take.
    pub low: i32,
    /// Highest MIDI pitch any voice may take.
    pub high: i32,
    /// Keep the melody as the top voice.
    pub preserve_top: bool,
    /// Keep the requested bass note as the bottom voice.
    pub preserve_bass: bool,
    /// Largest permitted single-voice leap in semitones.
    pub max_leap: i32,
    /// Instrument profile id whose range and spacing limits apply.
    pub instrument_profile: Option<String>,
    /// Tie-breaking seed.
    pub seed: u64,
}

impl Default for VoicingParams {
    fn default() -> Self {
        VoicingParams {
            voice_count: 4,
            families: Vec::new(),
            low: 48,
            high: 84,
            preserve_top: true,
            preserve_bass: false,
            max_leap: 12,
            instrument_profile: Some("piano_keys".to_string()),
            seed: 0,
        }
    }
}

impl VoicingParams {
    /// Rejects a request whose range cannot hold the requested voices.
    pub fn validate(&self) -> Result<(), HarmonyError> {
        if self.voice_count == 0 {
            return Err(HarmonyError::invalid_argument(
                "voice_count must be positive",
            ));
        }
        if self.low < 0 || self.high > 127 || self.low >= self.high {
            return Err(HarmonyError::invalid_argument(format!(
                "voicing range {}..{} is not a valid MIDI window",
                self.low, self.high
            )));
        }
        Ok(())
    }
}

/// Rounds to six decimals so emitted floats are byte-stable.
pub(crate) fn round6(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    (v * 1_000_000.0).round() / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_flag_starts_clear_and_latches() {
        let f = CancelFlag::new();
        assert!(!f.is_cancelled());
        assert!(f.check().is_ok());
        let clone = f.clone();
        clone.cancel();
        assert!(f.is_cancelled());
        assert!(f.check().is_err());
    }

    #[test]
    fn bass_motion_round_trips() {
        for m in BassMotion::all() {
            assert_eq!(BassMotion::parse(m.id()), Some(*m));
        }
        assert_eq!(BassMotion::parse("nope"), None);
    }

    #[test]
    fn params_reject_out_of_range() {
        let mut p = GenerateParams::default();
        assert!(p.validate().is_ok());
        p.candidate_count = 0;
        assert!(p.validate().is_err());
        p.candidate_count = 9;
        assert!(p.validate().is_err());
        p.candidate_count = 3;
        p.voice_count = 1;
        assert!(p.validate().is_err());
        p.voice_count = 4;
        p.complexity = 1.5;
        assert!(p.validate().is_err());
        p.complexity = f64::NAN;
        assert!(p.validate().is_err());
        p.complexity = 0.5;
        p.profile_id = String::new();
        assert!(p.validate().is_err());
    }

    #[test]
    fn canonical_json_is_stable_and_complete() {
        let p = GenerateParams::default();
        let a = p.canonical_json().to_canonical_string();
        let b = p.canonical_json().to_canonical_string();
        assert_eq!(a, b);
        for key in [
            "profile_id",
            "candidate_count",
            "preserve_melody",
            "preserve_rhythm",
            "grid",
            "complexity",
            "chromaticism",
            "extension_density",
            "bass_motion",
            "countermelody",
            "loop_intent",
            "strictness",
            "seed",
            "voice_count",
        ] {
            assert!(a.contains(key), "canonical form is missing {key}");
        }
    }

    #[test]
    fn canonical_json_changes_with_every_field() {
        let base = GenerateParams::default()
            .canonical_json()
            .to_canonical_string();
        let variants: Vec<GenerateParams> = vec![
            GenerateParams {
                profile_id: "blues".into(),
                ..GenerateParams::default()
            },
            GenerateParams {
                seed: 9,
                ..GenerateParams::default()
            },
            GenerateParams {
                voice_count: 5,
                ..GenerateParams::default()
            },
            GenerateParams {
                loop_intent: Some(LoopIntent::ModalDrone),
                ..GenerateParams::default()
            },
        ];
        for v in variants {
            assert_ne!(base, v.canonical_json().to_canonical_string());
        }
    }

    #[test]
    fn search_config_rejects_zero_bounds() {
        let mut c = SearchConfig::default();
        assert!(c.validate().is_ok());
        c.beam_width = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn voicing_params_reject_bad_range() {
        let mut v = VoicingParams::default();
        assert!(v.validate().is_ok());
        v.low = 90;
        assert!(v.validate().is_err());
        v = VoicingParams::default();
        v.high = 200;
        assert!(v.validate().is_err());
        v = VoicingParams::default();
        v.voice_count = 0;
        assert!(v.validate().is_err());
    }
}
