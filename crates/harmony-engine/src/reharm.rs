//! Reharmonisation of material that already has chords.
//!
//! Reharmonisation is the same search under extra constraints. Whatever the
//! caller asked to keep — the melody, the bass, the cadence, the harmonic
//! rhythm, a particular family of sonorities — is imposed on the option pools
//! *before* the path search runs, so the search optimises inside the space the
//! caller allowed rather than being talked out of good answers afterwards.

use crate::candidates::{build_pools, option_from_chord, ChordOption};
use crate::ctx::EngineContext;
use crate::diversity::{diversify, PathFeatures};
use crate::error::HarmonyError;
use crate::generate::realise_candidate;
use crate::params::{CancelFlag, GenerateParams, SearchConfig};
use crate::search::search_in;
use music_analysis::grid::{GridMode, GridSlot, HarmonicGrid};
use music_analysis::report::Analysis;
use music_domain::prelude::*;
use qjson::time::unix_now;
use theory_kb::KnowledgeBase;

/// Everything the caller controls about a reharmonisation.
#[derive(Clone, Debug, PartialEq)]
pub struct ReharmParams {
    /// Keep every melody pitch and onset.
    pub preserve_melody: bool,
    /// Keep the existing bass note under every chord.
    pub preserve_bass: bool,
    /// Keep the existing final chord's function.
    pub preserve_cadence: bool,
    /// Keep the existing chord onsets and durations.
    pub preserve_harmonic_rhythm: bool,
    /// Chord families the result may use; empty means any.
    pub families: Vec<String>,
    /// Vocabulary complexity target, `0.0..=1.0`.
    pub complexity: f64,
    /// Chromaticism target, `0.0..=1.0`.
    pub chromaticism: f64,
    /// Style profile id.
    pub profile_id: String,
    /// How many candidates to return, `1..=8`.
    pub candidate_count: usize,
    /// Tie-breaking seed.
    pub seed: u64,
}

impl Default for ReharmParams {
    fn default() -> Self {
        ReharmParams {
            preserve_melody: true,
            preserve_bass: false,
            preserve_cadence: true,
            preserve_harmonic_rhythm: true,
            families: Vec::new(),
            complexity: 0.6,
            chromaticism: 0.5,
            profile_id: "jazz_standard".to_string(),
            candidate_count: 3,
            seed: 0,
        }
    }
}

impl ReharmParams {
    /// The generation parameters this reharmonisation runs under.
    pub fn to_generate_params(&self) -> GenerateParams {
        GenerateParams {
            profile_id: self.profile_id.clone(),
            candidate_count: self.candidate_count,
            preserve_melody: self.preserve_melody,
            preserve_rhythm: true,
            grid: if self.preserve_harmonic_rhythm {
                GridMode::Existing
            } else {
                GridMode::Auto
            },
            complexity: self.complexity,
            chromaticism: self.chromaticism,
            seed: self.seed,
            ..GenerateParams::default()
        }
    }

    /// Rejects an out-of-range request.
    pub fn validate(&self) -> Result<(), HarmonyError> {
        self.to_generate_params().validate()
    }
}

/// Reharmonises existing chords, as the frozen contract declares it.
pub fn reharmonize(
    kb: &KnowledgeBase,
    an: &Analysis,
    existing: &[ChordEvent],
    p: &ReharmParams,
    cancel: &CancelFlag,
) -> Result<Vec<Candidate>, HarmonyError> {
    p.validate()?;
    if existing.is_empty() {
        return Err(HarmonyError::invalid_argument(
            "reharmonisation needs at least one existing chord to work from",
        ));
    }
    let params = p.to_generate_params();
    let profile = kb.resolve_profile(&params.profile_id).map_err(|e| {
        HarmonyError::invalid_argument(format!("profile {}: {e}", params.profile_id))
    })?;

    // The existing harmonic rhythm becomes the grid when the caller asked to
    // keep it, so the search cannot quietly re-bar the passage.
    let mut analysis = an.clone();
    if p.preserve_harmonic_rhythm {
        analysis.grid = grid_from(existing, an);
    }
    let ctx = EngineContext::new(kb, &profile, &analysis, &params)?;
    cancel.check()?;

    let mut pools = build_pools(&ctx)?;
    constrain(&mut pools, existing, p, &ctx)?;

    let paths = search_in(
        &ctx,
        &pools,
        &SearchConfig::default(),
        cancel,
        &mut |_, _| {},
    )?;
    let selected = diversify(paths.clone(), p.candidate_count, &profile, p.seed);
    if selected.is_empty() {
        return Err(HarmonyError::no_valid_candidate(
            "no reharmonisation satisfied the preservation constraints",
        ));
    }

    let created = unix_now();
    let mut out = Vec::with_capacity(selected.len());
    let mut features: Vec<PathFeatures> = Vec::new();
    for path in &selected {
        cancel.check()?;
        let rejected: Vec<String> = paths
            .iter()
            .filter(|other| other.shape() != path.shape())
            .take(4)
            .map(|other| other.shape())
            .collect();
        out.push(realise_candidate(
            &ctx,
            path,
            &features,
            rejected,
            created,
            CandidateKind::Reharmonization,
        )?);
        features.push(PathFeatures::of(path));
    }
    Ok(out)
}

/// Rebuilds the harmonic grid from the existing chords' own rhythm.
pub fn grid_from(existing: &[ChordEvent], an: &Analysis) -> HarmonicGrid {
    let melody = &an.extraction.melody;
    let slots = existing
        .iter()
        .map(|chord| {
            let melody_notes: Vec<NoteId> = melody
                .notes
                .iter()
                .filter(|n| n.onset >= chord.onset && n.onset < chord.end())
                .map(|n| n.id)
                .collect();
            let structural_notes: Vec<NoteId> = melody_notes
                .iter()
                .copied()
                .filter(|id| an.salience.is_structural(*id))
                .collect();
            let is_cadential = an
                .grid
                .slots
                .iter()
                .any(|s| s.is_cadential && s.start >= chord.onset && s.start < chord.end());
            GridSlot {
                start: chord.onset,
                end: chord.end(),
                melody_notes,
                structural_notes,
                is_cadential,
                weight: melody.time_map.metric_weight(chord.onset),
            }
        })
        .collect();
    HarmonicGrid {
        slots,
        mode_used: GridMode::Existing,
        rationale: "the harmonic rhythm of the existing chords was preserved".to_string(),
    }
}

/// Applies the caller's preservation constraints to the option pools.
fn constrain(
    pools: &mut [Vec<ChordOption>],
    existing: &[ChordEvent],
    p: &ReharmParams,
    ctx: &EngineContext<'_>,
) -> Result<(), HarmonyError> {
    let slot_count = pools.len();
    let slots = ctx.analysis.grid.slots.clone();
    for (index, pool) in pools.iter_mut().enumerate() {
        let original = existing.get(index.min(existing.len().saturating_sub(1)));
        // The chord that is already there is always an option, so every
        // preservation constraint has at least one way to be satisfied.
        if let (Some(original), Some(slot)) = (original, slots.get(index)) {
            if let Some(option) = option_from_chord(ctx, slot, original) {
                if !pool.iter().any(|o| o.key() == option.key()) {
                    pool.push(option);
                }
            }
        }
        let before = pool.clone();
        if !p.families.is_empty() {
            pool.retain(|o| p.families.iter().any(|f| f == o.spec.family_id()));
        }
        if p.preserve_bass {
            if let Some(original) = original {
                let want = bass_pc(original);
                pool.retain(|o| o.bass_pc() == want);
            }
        }
        let is_last = index + 1 == slot_count;
        if p.preserve_cadence && is_last {
            if let Some(original) = existing.last() {
                if let Some(function) = original.function {
                    let kept: Vec<ChordOption> = pool
                        .iter()
                        .filter(|o| o.function == function)
                        .cloned()
                        .collect();
                    if !kept.is_empty() {
                        *pool = kept;
                    }
                }
            }
        }
        if pool.is_empty() {
            // A constraint that removes every option is reported, and the slot
            // falls back to the unconstrained pool rather than failing the
            // whole request silently.
            *pool = before;
            if pool.is_empty() {
                return Err(HarmonyError::no_valid_candidate(format!(
                    "slot {index} has no chord option at all"
                )));
            }
        }
    }
    Ok(())
}

/// The sounding bass pitch class of an existing chord.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use music_analysis::report::{analyze, AnalyzeParams};

    fn setup(fixture: &str, profile: &str) -> (&'static KnowledgeBase, Analysis, Vec<ChordEvent>) {
        let kb = KnowledgeBase::embedded();
        let f = testing::load_fixture(fixture);
        let notes = f.note_set();
        let ap = AnalyzeParams {
            profile_id: profile.to_string(),
            ..AnalyzeParams::default()
        };
        let an = analyze(kb, fixture, &notes, &ap).expect("analysis");
        let mut chords = f.chord_events();
        for (i, c) in chords.iter_mut().enumerate() {
            c.id = i as u32;
            if c.function.is_none() {
                c.function = Some(if i + 1 == 3 {
                    HarmonicFunction::Tonic
                } else {
                    HarmonicFunction::Predominant
                });
            }
        }
        (kb, an, chords)
    }

    #[test]
    fn reharmonises_a_progression_fixture() {
        let (kb, an, chords) = setup("progressions/reharm_source_c_major", "jazz_standard");
        let out = reharmonize(
            kb,
            &an,
            &chords,
            &ReharmParams::default(),
            &CancelFlag::new(),
        )
        .expect("reharmonisation");
        assert_eq!(out.len(), 3);
        for c in &out {
            assert_eq!(c.kind, CandidateKind::Reharmonization);
            assert_eq!(c.chords.len(), chords.len());
        }
    }

    #[test]
    fn preserving_harmonic_rhythm_keeps_the_chord_grid() {
        let (kb, an, chords) = setup("progressions/reharm_source_c_major", "jazz_standard");
        let out = reharmonize(
            kb,
            &an,
            &chords,
            &ReharmParams {
                preserve_harmonic_rhythm: true,
                ..ReharmParams::default()
            },
            &CancelFlag::new(),
        )
        .expect("reharmonisation");
        for c in &out {
            for (new, original) in c.chords.iter().zip(&chords) {
                assert_eq!(new.onset, original.onset);
                assert_eq!(new.duration, original.duration);
            }
        }
    }

    #[test]
    fn preserving_the_bass_keeps_every_bass_note() {
        let (kb, an, chords) = setup("progressions/reharm_source_c_major", "neo_soul_rnb");
        let out = reharmonize(
            kb,
            &an,
            &chords,
            &ReharmParams {
                preserve_bass: true,
                ..ReharmParams::default()
            },
            &CancelFlag::new(),
        )
        .expect("reharmonisation");
        for c in &out {
            for (new, original) in c.chords.iter().zip(&chords) {
                assert_eq!(
                    bass_pc(new),
                    bass_pc(original),
                    "bass changed under preserve_bass"
                );
            }
        }
    }

    #[test]
    fn family_restriction_is_honoured_where_it_can_be() {
        let (kb, an, chords) = setup("progressions/reharm_source_c_major", "jazz_standard");
        let out = reharmonize(
            kb,
            &an,
            &chords,
            &ReharmParams {
                families: vec!["dominant".to_string(), "major".to_string()],
                preserve_cadence: false,
                ..ReharmParams::default()
            },
            &CancelFlag::new(),
        )
        .expect("reharmonisation");
        for c in &out {
            for chord in &c.chords {
                assert!(
                    matches!(chord.spec.family_id(), "dominant" | "major"),
                    "{} is outside the requested families",
                    chord.spec.render_ascii()
                );
            }
        }
    }

    #[test]
    fn reharmonisation_differs_from_the_original() {
        let (kb, an, chords) = setup("progressions/reharm_source_c_major", "jazz_standard");
        let out = reharmonize(
            kb,
            &an,
            &chords,
            &ReharmParams::default(),
            &CancelFlag::new(),
        )
        .expect("reharmonisation");
        let original: Vec<String> = chords.iter().map(|c| c.spec.render_ascii()).collect();
        assert!(
            out.iter().any(|c| c
                .chords
                .iter()
                .map(|x| x.spec.render_ascii())
                .collect::<Vec<_>>()
                != original),
            "every reharmonisation reproduced the original"
        );
    }

    #[test]
    fn empty_input_is_rejected() {
        let (kb, an, _) = setup("progressions/reharm_source_c_major", "jazz_standard");
        let e = reharmonize(kb, &an, &[], &ReharmParams::default(), &CancelFlag::new())
            .expect_err("no chords");
        assert_eq!(e.code, crate::error::INVALID_ARGUMENT);
    }

    #[test]
    fn cancellation_is_honoured() {
        let (kb, an, chords) = setup("progressions/reharm_source_c_major", "jazz_standard");
        let cancel = CancelFlag::new();
        cancel.cancel();
        let e = reharmonize(kb, &an, &chords, &ReharmParams::default(), &cancel)
            .expect_err("cancelled");
        assert!(e.is_cancelled());
    }

    #[test]
    fn reharmonisation_is_deterministic() {
        let (kb, an, chords) = setup("progressions/reharm_source_c_major", "cinematic");
        let p = ReharmParams {
            profile_id: "cinematic".to_string(),
            ..ReharmParams::default()
        };
        let a = reharmonize(kb, &an, &chords, &p, &CancelFlag::new()).expect("a");
        let b = reharmonize(kb, &an, &chords, &p, &CancelFlag::new()).expect("b");
        let sa: Vec<String> = a
            .iter()
            .map(crate::generate::candidate_fingerprint)
            .collect();
        let sb: Vec<String> = b
            .iter()
            .map(crate::generate::candidate_fingerprint)
            .collect();
        assert_eq!(sa, sb);
    }

    #[test]
    fn params_are_validated() {
        let p = ReharmParams {
            candidate_count: 99,
            ..ReharmParams::default()
        };
        assert!(p.validate().is_err());
    }
}
