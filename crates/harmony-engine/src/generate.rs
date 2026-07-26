//! The top-level entry point: analysis in, explained candidates out.
//!
//! This is where stages 5 to 8, 11 and 13 are wired together. It is also where
//! the hard constraints are enforced one final time on the material that is
//! actually going to be written: MIDI bounds, strictly positive durations,
//! ordering, monophony in parts declared monophonic, and nothing sounding
//! outside the generated span. Those are invariants, not preferences, so they
//! are checked on the output rather than trusted from the stages above.

use crate::candidates::build_pools;
use crate::ctx::{EngineContext, RULE_DELTA_SCALE};
use crate::diversity::{diversify, PathFeatures};
use crate::error::HarmonyError;
use crate::explain::{build_trace, TraceInputs};
use crate::factbuild::context_for;
use crate::params::{CancelFlag, GenerateParams, SearchConfig, VoicingParams};
use crate::search::{search_in, HarmonicPath, BIASES};
use crate::voiceleading::{audit_with, AuditOptions};
use crate::voicing::voice_progression;
use music_analysis::report::Analysis;
use music_domain::ids;
use music_domain::prelude::*;
use qjson::time::{iso8601_from_unix, unix_now};
use theory_kb::{KnowledgeBase, RuleEvent};

/// How long a generated candidate stays valid, in seconds.
pub const CANDIDATE_TTL_SECONDS: i64 = 3600;

/// First note id the harmony part uses.
pub const HARMONY_ID_BASE: NoteId = 300_000;

/// Default velocity of a harmony note.
pub const HARMONY_VELOCITY: u8 = 72;

/// The instrument the harmony part is written for.
pub const HARMONY_INSTRUMENT: &str = "piano_keys";

/// Similarity above which two candidates count as the same strategy.
pub const DUPLICATE_STRATEGY_SIMILARITY: f64 = 0.95;

/// The top-level entry point, as the frozen contract declares it.
pub fn generate_candidates(
    kb: &KnowledgeBase,
    an: &Analysis,
    p: &GenerateParams,
    cancel: &CancelFlag,
    progress: &mut dyn FnMut(f64, &str),
) -> Result<Vec<Candidate>, HarmonyError> {
    p.validate()?;
    let profile = kb
        .resolve_profile(&p.profile_id)
        .map_err(|e| HarmonyError::invalid_argument(format!("profile {}: {e}", p.profile_id)))?;
    let ctx = EngineContext::new(kb, &profile, an, p)?;
    cancel.check()?;

    progress(0.05, "building the per-slot chord pools");
    let pools = build_pools(&ctx)?;
    cancel.check()?;

    progress(0.2, "searching whole-phrase paths");
    let paths = search_in(
        &ctx,
        &pools,
        &SearchConfig::default(),
        cancel,
        &mut |fraction, label| progress(0.2 + 0.4 * fraction, label),
    )?;
    cancel.check()?;

    progress(0.65, "selecting distinct candidates");
    let selected = diversify(paths.clone(), p.candidate_count, &profile, p.seed);
    if selected.is_empty() {
        return Err(HarmonyError::no_valid_candidate(
            "no candidate survived diversity selection",
        ));
    }

    let created = unix_now();
    let mut out: Vec<Candidate> = Vec::with_capacity(selected.len());
    let mut features: Vec<PathFeatures> = Vec::new();
    for (index, path) in selected.iter().enumerate() {
        cancel.check()?;
        progress(
            0.7 + 0.3 * (index as f64 / selected.len() as f64),
            "realising and explaining a candidate",
        );
        let rejected: Vec<String> = paths
            .iter()
            .filter(|other| other.shape() != path.shape())
            .take(4)
            .map(|other| other.shape())
            .collect();
        let candidate = realise_candidate(
            &ctx,
            path,
            &features,
            rejected,
            created,
            CandidateKind::Harmonization,
        )?;
        features.push(PathFeatures::of(path));
        out.push(candidate);
    }
    progress(1.0, "generation complete");
    Ok(out)
}

/// Builds one candidate from one chosen path.
pub(crate) fn realise_candidate(
    ctx: &EngineContext<'_>,
    path: &HarmonicPath,
    earlier: &[PathFeatures],
    rejected: Vec<String>,
    created: i64,
    kind: CandidateKind,
) -> Result<Candidate, HarmonyError> {
    let melody = &ctx.analysis.extraction.melody;
    let vp = voicing_params(ctx);
    let voicings = voice_progression(
        ctx.kb,
        ctx.profile,
        &path.chords,
        if ctx.params.preserve_melody {
            Some(melody)
        } else {
            None
        },
        &vp,
    )?;

    let mut chords = path.chords.clone();
    for (chord, voicing) in chords.iter_mut().zip(&voicings) {
        chord.voicing = Some(voicing.clone());
    }

    let audit = audit_with(
        ctx.kb,
        ctx.profile,
        &voicings,
        &chords,
        &AuditOptions {
            melody: Some(melody.clone()),
            planing: ctx.planing_context(),
            modal: ctx.key.is_modal,
            cluster_intent: false,
            instrument_profile: Some(HARMONY_INSTRUMENT.to_string()),
            role_is_bass: false,
        },
    );

    let bass = crate::bass::generate_bass(
        ctx.kb,
        ctx.profile,
        &chords,
        &melody.time_map,
        ctx.params.bass_motion,
        None,
        ctx.params.seed,
    )?;
    let counter = crate::countermelody::generate_countermelody(
        ctx.kb,
        ctx.profile,
        melody,
        &chords,
        ctx.analysis,
        &ctx.params.countermelody,
        ctx.params.seed,
    )?;

    let mut parts = Vec::new();
    if ctx.params.preserve_melody {
        parts.push(Part {
            role: ArrangementRole::Lead,
            name: "Melody".to_string(),
            notes: melody.notes.clone(),
            instrument_profile: Some("melody_lead".to_string()),
            channel: 0,
            polyphonic: false,
        });
    }
    parts.push(Part {
        role: ArrangementRole::HarmonicBed,
        name: "Harmony".to_string(),
        notes: harmony_notes(&chords, &voicings),
        instrument_profile: Some(HARMONY_INSTRUMENT.to_string()),
        channel: 1,
        polyphonic: true,
    });
    parts.push(Part {
        role: ArrangementRole::Bass,
        name: "Bass".to_string(),
        notes: bass,
        instrument_profile: Some("bass".to_string()),
        channel: 2,
        polyphonic: false,
    });
    if !counter.is_empty() {
        parts.push(Part {
            role: ctx.params.countermelody.role,
            name: "Countermelody".to_string(),
            notes: counter,
            instrument_profile: Some("counterlead".to_string()),
            channel: 3,
            polyphonic: false,
        });
    }

    let span = span_of(&chords, melody);
    let warnings = enforce_invariants(&mut parts, span);

    let label = BIASES
        .iter()
        .find(|b| b.id == path.strategy)
        .map(|b| b.label)
        .unwrap_or("Generated");
    let id = candidate_id(ctx, path);

    let mut scored = path.clone();
    scored.chords = chords.clone();
    merge_diversity_rule(ctx, &mut scored, earlier);

    let trace = build_trace(&TraceInputs {
        candidate_id: &id,
        kb: ctx.kb,
        profile: ctx.profile,
        analysis: ctx.analysis,
        key: &ctx.key,
        path: &scored,
        strategy_label: label,
        voice_leading: Some(&audit),
        seed: ctx.params.seed,
        rejected_alternatives: rejected,
        assumptions: vec![format!(
            "the harmony part is voiced for {} voices between MIDI {} and {}",
            vp.voice_count, vp.low, vp.high
        )],
        warnings,
    });

    Ok(Candidate {
        id,
        kind,
        label: label.to_string(),
        strategy: path.strategy.clone(),
        chords,
        parts,
        trace,
        loop_report: None,
        created_at: iso8601_from_unix(created),
        expires_at: iso8601_from_unix(created + CANDIDATE_TTL_SECONDS),
    })
}

/// The content-derived candidate id.
///
/// The same analysis, knowledge version, parameters and strategy always yield
/// the same id, which is what makes a golden test possible.
pub fn candidate_id(ctx: &EngineContext<'_>, path: &HarmonicPath) -> String {
    ids::candidate_id_from(&format!(
        "{}|{}|{}|{}|{}",
        ctx.analysis.id,
        ctx.kb.version(),
        ctx.params.canonical_json().to_canonical_string(),
        path.strategy,
        path.shape()
    ))
}

/// The voicing request derived from the generation request and the profile.
pub fn voicing_params(ctx: &EngineContext<'_>) -> VoicingParams {
    let instrument = ctx.kb.instrument_profile(HARMONY_INSTRUMENT);
    let (low, high) = match instrument {
        Some(p) => (
            p.comfortable_range.low_midi.max(45),
            p.comfortable_range.high_midi.min(88),
        ),
        None => (48, 84),
    };
    VoicingParams {
        voice_count: ctx.params.voice_count,
        families: Vec::new(),
        low,
        high,
        preserve_top: ctx.params.preserve_melody,
        preserve_bass: false,
        max_leap: 12,
        instrument_profile: Some(HARMONY_INSTRUMENT.to_string()),
        seed: ctx.params.seed,
    }
}

/// The harmony part's notes, one per voice per chord.
fn harmony_notes(chords: &[ChordEvent], voicings: &[Voicing]) -> Vec<Note> {
    let mut out = Vec::new();
    for (chord, voicing) in chords.iter().zip(voicings) {
        for (voice, pitch) in voicing.pitches.iter().enumerate() {
            let mut note = Note::new(
                HARMONY_ID_BASE + out.len() as NoteId,
                *pitch,
                chord.onset,
                chord.duration,
            );
            note.midi = pitch.midi();
            note.velocity = HARMONY_VELOCITY;
            note.role = NoteRole::Harmony;
            note.voice = VoiceId(voice as u16);
            note.channel = 1;
            note.confidence = chord.confidence;
            out.push(note);
        }
    }
    out
}

/// The span the candidate writes into.
pub fn span_of(chords: &[ChordEvent], melody: &NoteSet) -> (BeatTime, BeatTime) {
    let mut start = chords.first().map(|c| c.onset).unwrap_or(BeatTime::ZERO);
    let mut end = chords.last().map(|c| c.end()).unwrap_or(BeatTime::ZERO);
    for note in &melody.notes {
        start = start.min(note.onset);
        end = end.max(note.end());
    }
    (start, end)
}

/// Enforces the hard constraints on the material that will actually be written.
///
/// These are invariants: a note outside the MIDI range or a monophonic part
/// with two notes sounding at once is not a low-scoring candidate, it is an
/// invalid one, so it is repaired here and reported rather than shipped.
pub fn enforce_invariants(parts: &mut Vec<Part>, span: (BeatTime, BeatTime)) -> Vec<Warning> {
    let (start, end) = span;
    let mut warnings = Vec::new();
    for part in parts.iter_mut() {
        let before = part.notes.len();
        part.notes.retain(|n| n.duration.is_positive());
        part.notes.retain(|n| (0..=127).contains(&n.midi));
        for note in part.notes.iter_mut() {
            if note.onset < start {
                let shift = start - note.onset;
                note.onset = start;
                note.duration = note.duration - shift;
            }
            if note.end() > end {
                note.duration = end - note.onset;
            }
        }
        part.notes.retain(|n| n.duration.is_positive());
        part.notes
            .sort_by(|a, b| a.onset.cmp(&b.onset).then_with(|| a.midi.cmp(&b.midi)));
        if !part.polyphonic {
            crate::bass::enforce_monophony(&mut part.notes);
        }
        if part.notes.len() != before {
            warnings.push(Warning::new(
                "NOTES_TRIMMED",
                format!(
                    "{} note(s) in the {} part were trimmed to satisfy the hard constraints",
                    before - part.notes.len(),
                    part.name
                ),
                Severity::Minor,
            ));
        }
    }
    parts.retain(|p| !p.notes.is_empty());
    warnings
}

/// Evaluates the diversity rule for this candidate against the earlier ones.
fn merge_diversity_rule(
    ctx: &EngineContext<'_>,
    path: &mut HarmonicPath,
    earlier: &[PathFeatures],
) {
    let features = PathFeatures::of(path);
    let duplicate = earlier
        .iter()
        .any(|f| f.similarity(&features) >= DUPLICATE_STRATEGY_SIMILARITY);
    let mut rc = context_for(ctx, RuleEvent::ProgressionPath);
    rc.set_bool("candidate_duplicates_existing_strategy", duplicate);
    let outcome = ctx.engine.evaluate(&rc);
    for app in outcome.applications {
        if app.status != RuleStatus::NotApplicable
            && !path
                .rule_applications
                .iter()
                .any(|a| a.rule_id == app.rule_id && a.status == app.status)
        {
            path.rule_applications.push(app);
        }
    }
    for (component, delta) in &outcome.deltas {
        path.score.add(component, delta * RULE_DELTA_SCALE);
    }
    let weights: Vec<(&str, f64)> = SCORE_COMPONENTS
        .iter()
        .map(|c| (*c, ctx.profile.weight(c)))
        .collect();
    path.score.recompute_total(&weights);
}

/// The canonical JSON of a candidate with its wall-clock lifecycle fields
/// removed.
///
/// `Candidate::to_json` includes `created_at` and `expires_at`, which are
/// genuinely time-dependent. Determinism is asserted over everything else: two
/// runs of the same request produce the same fingerprint byte for byte.
pub fn candidate_fingerprint(candidate: &Candidate) -> String {
    let mut json = candidate.to_json();
    if let qjson::Json::Obj(map) = &mut json {
        map.remove("created_at");
        map.remove("expires_at");
    }
    json.to_canonical_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    fn generate(fixture: &str, p: &GenerateParams) -> (testing::Harness, Vec<Candidate>) {
        let h = testing::harness_with(fixture, p.clone());
        let out = generate_candidates(h.kb, &h.analysis, p, &CancelFlag::new(), &mut |_, _| {})
            .unwrap_or_else(|e| panic!("{fixture}/{}: {e}", p.profile_id));
        (h, out)
    }

    #[test]
    fn three_candidates_for_the_reference_fixture() {
        let p = GenerateParams::default().with_profile("jazz_standard");
        let (_, candidates) = generate("melodies/eight_bar_c_major", &p);
        assert_eq!(candidates.len(), 3);
        for c in &candidates {
            assert_eq!(c.kind, CandidateKind::Harmonization);
            assert!(!c.chords.is_empty());
            assert!(!c.parts.is_empty());
            assert!(!c.trace.explanation.is_empty());
        }
    }

    #[test]
    fn candidate_ids_are_unique_and_derived() {
        let p = GenerateParams::default().with_profile("pop_rock");
        let (_, a) = generate("melodies/eight_bar_c_major", &p);
        let (_, b) = generate("melodies/eight_bar_c_major", &p);
        let ids_a: Vec<&str> = a.iter().map(|c| c.id.as_str()).collect();
        let ids_b: Vec<&str> = b.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids_a, ids_b);
        let mut sorted = ids_a.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids_a.len());
    }

    #[test]
    fn every_part_respects_the_hard_constraints() {
        for profile in testing::PROFILE_IDS {
            let p = GenerateParams::default().with_profile(profile);
            let (h, candidates) = generate("melodies/eight_bar_c_major", &p);
            for c in &candidates {
                let (start, end) = span_of(&c.chords, &h.analysis.extraction.melody);
                for part in &c.parts {
                    for note in &part.notes {
                        assert!((0..=127).contains(&note.midi));
                        assert!(note.duration.is_positive());
                        assert!(note.onset >= start && note.end() <= end);
                    }
                    if !part.polyphonic {
                        for pair in part.notes.windows(2) {
                            assert!(pair[0].end() <= pair[1].onset);
                        }
                    }
                    for pair in part.notes.windows(2) {
                        assert!(pair[0].onset <= pair[1].onset, "notes are unordered");
                    }
                }
            }
        }
    }

    #[test]
    fn candidate_count_is_honoured() {
        for n in 1..=5usize {
            let p = GenerateParams::default()
                .with_profile("cinematic")
                .with_candidate_count(n);
            let (_, candidates) = generate("melodies/eight_bar_c_major", &p);
            assert_eq!(candidates.len(), n);
        }
    }

    #[test]
    fn candidate_count_is_validated() {
        let h = testing::harness("melodies/eight_bar_c_major", "pop_rock");
        let p = GenerateParams {
            candidate_count: 0,
            ..GenerateParams::default()
        };
        let e = generate_candidates(h.kb, &h.analysis, &p, &CancelFlag::new(), &mut |_, _| {})
            .expect_err("zero candidates");
        assert_eq!(e.code, crate::error::INVALID_ARGUMENT);
    }

    #[test]
    fn cancellation_is_honoured() {
        let h = testing::harness("melodies/eight_bar_c_major", "pop_rock");
        let cancel = CancelFlag::new();
        cancel.cancel();
        let e = generate_candidates(
            h.kb,
            &h.analysis,
            &GenerateParams::default().with_profile("pop_rock"),
            &cancel,
            &mut |_, _| {},
        )
        .expect_err("cancelled");
        assert!(e.is_cancelled());
    }

    #[test]
    fn progress_is_monotone_and_complete() {
        let h = testing::harness("melodies/eight_bar_c_major", "pop_rock");
        let mut seen: Vec<f64> = Vec::new();
        generate_candidates(
            h.kb,
            &h.analysis,
            &GenerateParams::default().with_profile("pop_rock"),
            &CancelFlag::new(),
            &mut |f, _| seen.push(f),
        )
        .expect("candidates");
        assert!(!seen.is_empty());
        assert_eq!(seen.last().copied(), Some(1.0));
        assert!(seen.iter().all(|f| (0.0..=1.0).contains(f)));
    }

    #[test]
    fn preserve_melody_keeps_every_pitch_and_onset() {
        let p = GenerateParams {
            preserve_melody: true,
            ..GenerateParams::default().with_profile("jazz_standard")
        };
        let (h, candidates) = generate("melodies/eight_bar_c_major", &p);
        let source = &h.analysis.extraction.melody.notes;
        for c in &candidates {
            let lead = c
                .parts
                .iter()
                .find(|p| p.role == ArrangementRole::Lead)
                .expect("a melody part");
            assert_eq!(lead.notes.len(), source.len());
            for (out, original) in lead.notes.iter().zip(source) {
                assert_eq!(out.midi, original.midi, "pitch changed");
                assert_eq!(out.onset, original.onset, "onset changed");
                assert_eq!(out.pitch.to_ascii(), original.pitch.to_ascii());
            }
        }
    }

    #[test]
    fn preserve_rhythm_keeps_every_duration() {
        let p = GenerateParams {
            preserve_rhythm: true,
            ..GenerateParams::default().with_profile("common_practice")
        };
        let (h, candidates) = generate("melodies/eight_bar_c_major", &p);
        let source = &h.analysis.extraction.melody.notes;
        for c in &candidates {
            let lead = c
                .parts
                .iter()
                .find(|p| p.role == ArrangementRole::Lead)
                .expect("a melody part");
            for (out, original) in lead.notes.iter().zip(source) {
                assert_eq!(out.onset, original.onset);
                assert_eq!(out.duration, original.duration);
                assert_eq!(out.end(), original.end());
            }
        }
    }

    #[test]
    fn candidates_carry_a_bass_and_a_harmony_part() {
        let p = GenerateParams::default().with_profile("neo_soul_rnb");
        let (_, candidates) = generate("melodies/eight_bar_c_major", &p);
        for c in &candidates {
            assert!(c.parts.iter().any(|p| p.role == ArrangementRole::Bass));
            assert!(c
                .parts
                .iter()
                .any(|p| p.role == ArrangementRole::HarmonicBed));
        }
    }

    #[test]
    fn countermelody_appears_only_when_requested() {
        let off = GenerateParams::default().with_profile("cinematic");
        let (_, without) = generate("melodies/eight_bar_c_major", &off);
        assert!(without
            .iter()
            .all(|c| !c.parts.iter().any(|p| p.name == "Countermelody")));
        let on = GenerateParams {
            countermelody: crate::params::CountermelodyParams {
                enabled: true,
                density: 0.6,
                role: ArrangementRole::Counterlead,
            },
            ..GenerateParams::default().with_profile("cinematic")
        };
        let (_, with) = generate("melodies/eight_bar_c_major", &on);
        assert!(with
            .iter()
            .all(|c| c.parts.iter().any(|p| p.name == "Countermelody")));
    }

    #[test]
    fn chords_carry_realised_voicings() {
        let p = GenerateParams::default().with_profile("jazz_standard");
        let (_, candidates) = generate("melodies/eight_bar_c_major", &p);
        for c in &candidates {
            for chord in &c.chords {
                let voicing = chord.voicing.as_ref().expect("a voicing");
                assert!(!voicing.pitches.is_empty());
            }
        }
    }

    #[test]
    fn candidate_json_round_trips() {
        let p = GenerateParams::default().with_profile("blues");
        let (_, candidates) = generate("melodies/blues_head_c", &p);
        for c in &candidates {
            let json = c.to_json();
            let back = Candidate::from_json(&json).expect("round trip");
            assert_eq!(back.id, c.id);
            assert_eq!(back.chords.len(), c.chords.len());
            assert_eq!(back.parts.len(), c.parts.len());
        }
    }
}
