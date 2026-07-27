//! The arrangement request.

use crate::error::ArrangementError;
use music_domain::prelude::*;
use qjson::{json_obj, Json};

/// Everything the caller controls about an arrangement, as `CONTRACTS.md`
/// freezes it.
///
/// The three scalar controls — [`ArrangementParams::density`],
/// [`ArrangementParams::register_spread`] and the energy curve — are the levers
/// the brief requires to *measurably* change the output, and each one is
/// asserted to do so by a test.
#[derive(Clone, Debug)]
pub struct ArrangementParams {
    /// Style profile id, e.g. `"pop_rock"`.
    pub profile_id: String,
    /// Roles to write. Empty means "let the profile decide".
    pub roles: Vec<ArrangementRole>,
    /// Piecewise-linear energy curve over the span. Empty means flat.
    pub energy_curve: Vec<(BeatTime, f64)>,
    /// Global onset-density control, `0.0..=1.0`.
    pub density: f64,
    /// Force a specific `arrangement_patterns.json` id where its role matches.
    pub texture_pattern: Option<String>,
    /// How hard to push the parts apart in register, `0.0..=1.0`.
    pub register_spread: f64,
    /// Section plan. Empty means one section over the whole span.
    pub sections: Vec<Section>,
    /// Keep the candidate's lead material exactly as it is.
    pub preserve_melody: bool,
    /// What the loop is meant to do at its wrap.
    pub loop_intent: Option<LoopIntent>,
    /// Tie-breaking seed. Same seed plus same inputs means the same plan.
    pub seed: u64,
}

impl Default for ArrangementParams {
    fn default() -> ArrangementParams {
        ArrangementParams {
            profile_id: "pop_rock".to_string(),
            roles: Vec::new(),
            energy_curve: Vec::new(),
            density: 0.5,
            texture_pattern: None,
            register_spread: 0.5,
            sections: Vec::new(),
            preserve_melody: true,
            loop_intent: None,
            seed: 0,
        }
    }
}

impl ArrangementParams {
    /// Builder: sets the style profile.
    pub fn with_profile(mut self, id: &str) -> ArrangementParams {
        self.profile_id = id.to_string();
        self
    }

    /// Builder: sets the requested roles.
    pub fn with_roles(mut self, roles: &[ArrangementRole]) -> ArrangementParams {
        self.roles = roles.to_vec();
        self
    }

    /// Builder: sets the global density control.
    pub fn with_density(mut self, density: f64) -> ArrangementParams {
        self.density = density;
        self
    }

    /// Builder: sets the register-spread control.
    pub fn with_register_spread(mut self, spread: f64) -> ArrangementParams {
        self.register_spread = spread;
        self
    }

    /// Builder: sets the tie-breaking seed.
    pub fn with_seed(mut self, seed: u64) -> ArrangementParams {
        self.seed = seed;
        self
    }

    /// Builder: forces a texture pattern id.
    pub fn with_texture(mut self, pattern_id: &str) -> ArrangementParams {
        self.texture_pattern = Some(pattern_id.to_string());
        self
    }

    /// Builder: sets the energy curve.
    pub fn with_energy_curve(mut self, points: &[(BeatTime, f64)]) -> ArrangementParams {
        self.energy_curve = points.to_vec();
        self
    }

    /// Builder: sets the section plan.
    pub fn with_sections(mut self, sections: &[Section]) -> ArrangementParams {
        self.sections = sections.to_vec();
        self
    }

    /// Builder: sets the loop intent.
    pub fn with_loop_intent(mut self, intent: LoopIntent) -> ArrangementParams {
        self.loop_intent = Some(intent);
        self
    }

    /// Rejects a request that cannot be honoured.
    ///
    /// Everything checked here is a caller mistake rather than a musical
    /// judgement: out-of-domain scalars, a duplicated role, a non-monotonic
    /// energy curve, an inverted section.
    pub fn validate(&self) -> Result<(), ArrangementError> {
        if self.profile_id.is_empty() {
            return Err(ArrangementError::invalid_argument(
                "profile_id must not be empty",
            ));
        }
        if !(0.0..=1.0).contains(&self.density) || !self.density.is_finite() {
            return Err(ArrangementError::invalid_argument(format!(
                "density must be within 0.0..=1.0, got {}",
                self.density
            )));
        }
        if !(0.0..=1.0).contains(&self.register_spread) || !self.register_spread.is_finite() {
            return Err(ArrangementError::invalid_argument(format!(
                "register_spread must be within 0.0..=1.0, got {}",
                self.register_spread
            )));
        }
        for (i, role) in self.roles.iter().enumerate() {
            if self.roles[..i].contains(role) {
                return Err(ArrangementError::invalid_argument(format!(
                    "role {} was requested twice",
                    role.id()
                )));
            }
        }
        let mut previous: Option<BeatTime> = None;
        for (qn, value) in &self.energy_curve {
            if !(0.0..=1.0).contains(value) || !value.is_finite() {
                return Err(ArrangementError::invalid_argument(format!(
                    "energy value at {qn} must be within 0.0..=1.0, got {value}"
                )));
            }
            if let Some(p) = previous {
                if *qn < p {
                    return Err(ArrangementError::invalid_argument(
                        "energy_curve points must be ordered by position",
                    ));
                }
            }
            previous = Some(*qn);
        }
        for s in &self.sections {
            if s.end <= s.start {
                return Err(ArrangementError::invalid_argument(format!(
                    "section {} ends at or before it starts",
                    s.id
                )));
            }
            if !(0.0..=1.0).contains(&s.energy) || !s.energy.is_finite() {
                return Err(ArrangementError::invalid_argument(format!(
                    "section {} energy must be within 0.0..=1.0",
                    s.id
                )));
            }
        }
        Ok(())
    }

    /// The canonical JSON form, used for plan ids and golden comparisons.
    ///
    /// Key order is fixed by construction, so two equal requests serialise
    /// byte-identically.
    pub fn canonical_json(&self) -> Json {
        json_obj! {
            "profile_id" => self.profile_id.clone(),
            "roles" => Json::Arr(self.roles.iter().map(|r| Json::Str(r.id().to_string())).collect()),
            "energy_curve" => Json::Arr(self.energy_curve.iter().map(|(qn, v)| {
                json_obj!{ "qn" => qn.to_display(), "value" => *v }
            }).collect()),
            "density" => self.density,
            "texture_pattern" => match &self.texture_pattern {
                Some(t) => Json::Str(t.clone()),
                None => Json::Null,
            },
            "register_spread" => self.register_spread,
            "sections" => Json::Arr(self.sections.iter().map(|s| json_obj!{
                "id" => s.id.clone(),
                "start" => s.start.to_display(),
                "end" => s.end.to_display(),
                "role" => s.role.clone(),
                "energy" => s.energy,
            }).collect()),
            "preserve_melody" => self.preserve_melody,
            "loop_intent" => match self.loop_intent {
                Some(i) => Json::Str(i.id().to_string()),
                None => Json::Null,
            },
            "seed" => self.seed as i64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_request_validates() {
        assert!(ArrangementParams::default().validate().is_ok());
    }

    #[test]
    fn out_of_range_scalars_are_rejected() {
        let p = ArrangementParams::default().with_density(1.5);
        assert_eq!(p.validate().unwrap_err().code, "INVALID_ARGUMENT");
        let p = ArrangementParams::default().with_register_spread(-0.1);
        assert!(p.validate().is_err());
        let p = ArrangementParams::default().with_density(f64::NAN);
        assert!(p.validate().is_err());
    }

    #[test]
    fn duplicate_roles_are_rejected() {
        let p = ArrangementParams::default()
            .with_roles(&[ArrangementRole::Bass, ArrangementRole::Bass]);
        assert!(p.validate().is_err());
    }

    #[test]
    fn an_unordered_energy_curve_is_rejected() {
        let p = ArrangementParams::default()
            .with_energy_curve(&[(BeatTime::from_quarters(4), 0.5), (BeatTime::ZERO, 0.5)]);
        assert!(p.validate().is_err());
    }

    #[test]
    fn an_inverted_section_is_rejected() {
        let p = ArrangementParams::default().with_sections(&[Section {
            id: "a".into(),
            start: BeatTime::from_quarters(8),
            end: BeatTime::from_quarters(4),
            role: "verse".into(),
            energy: 0.5,
        }]);
        assert!(p.validate().is_err());
    }

    #[test]
    fn canonical_json_is_stable_and_complete() {
        let a = ArrangementParams::default()
            .with_roles(&[ArrangementRole::Lead, ArrangementRole::Bass])
            .with_seed(9);
        let b = a.clone();
        assert_eq!(
            a.canonical_json().to_canonical_string(),
            b.canonical_json().to_canonical_string()
        );
        let json = a.canonical_json();
        for key in [
            "profile_id",
            "roles",
            "energy_curve",
            "density",
            "texture_pattern",
            "register_spread",
            "sections",
            "preserve_melody",
            "loop_intent",
            "seed",
        ] {
            assert!(json.get(key).is_some(), "{key} missing");
        }
    }
}
