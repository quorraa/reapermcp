//! Stage 11 — decision traces.
//!
//! Every sentence in an explanation is assembled from a structured decision the
//! engine actually made: the key it read, the profile it ran under, the
//! strategy it searched with, the chords it chose, the cadence it landed on,
//! the rules that fired, the exceptions that set rules aside, and the sources
//! behind them. Nothing here is generated prose about music; it is a rendering
//! of the solver's own record.
//!
//! Rule ids are checked against the knowledge base and source ids are resolved
//! before they are written, so a trace can never cite something that does not
//! exist.

use crate::keyctx::KeyContext;
use crate::search::HarmonicPath;
use crate::voiceleading::VoiceLeadingReport;
use music_analysis::report::Analysis;
use music_domain::prelude::*;
use theory_kb::{KnowledgeBase, ResolvedProfile};

/// How many rule applications an explanation names.
pub const EXPLAINED_RULES: usize = 3;

/// Everything [`build_trace`] needs.
pub struct TraceInputs<'a> {
    /// Id of the candidate being explained.
    pub candidate_id: &'a str,
    /// The knowledge bundle.
    pub kb: &'a KnowledgeBase,
    /// The resolved profile.
    pub profile: &'a ResolvedProfile,
    /// The analysis the candidate was generated from.
    pub analysis: &'a Analysis,
    /// The tonal frame.
    pub key: &'a KeyContext,
    /// The chosen progression.
    pub path: &'a HarmonicPath,
    /// Human-readable strategy name.
    pub strategy_label: &'a str,
    /// The voice-leading audit, when one was run.
    pub voice_leading: Option<&'a VoiceLeadingReport>,
    /// Tie-breaking seed.
    pub seed: u64,
    /// Ids or shapes of alternatives that were not chosen.
    pub rejected_alternatives: Vec<String>,
    /// Extra assumptions the generation stage had to make.
    pub assumptions: Vec<String>,
    /// Extra warnings.
    pub warnings: Vec<Warning>,
}

/// Stage 11, as the frozen contract declares it.
pub fn build_trace(inputs: &TraceInputs<'_>) -> DecisionTrace {
    let mut applications: Vec<RuleApplication> = Vec::new();
    let mut source_ids: Vec<String> = Vec::new();
    let mut collect = |apps: &[RuleApplication]| {
        for app in apps {
            // A trace may only cite rules this bundle really contains.
            if inputs.kb.rule(&app.rule_id).is_none() {
                continue;
            }
            if applications
                .iter()
                .any(|a| a.rule_id == app.rule_id && a.status == app.status)
            {
                continue;
            }
            for s in &app.source_ids {
                if inputs.kb.source(s).is_some() && !source_ids.iter().any(|e| e == s) {
                    source_ids.push(s.clone());
                }
            }
            applications.push(app.clone());
        }
    };
    collect(&inputs.path.rule_applications);
    if let Some(vl) = inputs.voice_leading {
        collect(&vl.rule_applications);
    }
    applications.sort_by(|a, b| {
        b.score_delta
            .abs()
            .partial_cmp(&a.score_delta.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.rule_id.cmp(&b.rule_id))
    });
    source_ids.sort();

    let mut assumptions: Vec<String> = Vec::new();
    assumptions.push(format!(
        "the passage was read as {} with confidence {:.2}",
        inputs.key.label(),
        inputs.key.confidence
    ));
    if inputs.analysis.key.ambiguous {
        assumptions.push(
            "the key reading is ambiguous; a second reading scored almost as well".to_string(),
        );
    }
    assumptions.extend(inputs.analysis.extraction.assumptions.iter().cloned());
    assumptions.push(format!(
        "the harmonic grid came from {} and gave {} slots",
        inputs.analysis.grid.mode_used.id(),
        inputs.analysis.grid.slots.len()
    ));
    assumptions.extend(inputs.assumptions.iter().cloned());

    let mut warnings = inputs.warnings.clone();
    if let Some(vl) = inputs.voice_leading {
        warnings.extend(vl.findings.iter().cloned());
        if !vl.unresolved_tendencies.is_empty() {
            warnings.push(Warning::new(
                "UNRESOLVED_TENDENCY",
                format!(
                    "{} tendency tones are left unresolved",
                    vl.unresolved_tendencies.len()
                ),
                Severity::Minor,
            ));
        }
    }

    DecisionTrace {
        candidate_id: inputs.candidate_id.to_string(),
        analysis_id: inputs.analysis.id.clone(),
        snapshot_id: inputs.analysis.snapshot_id.clone(),
        knowledge_version: inputs.kb.version().to_string(),
        profile_id: inputs.profile.id.clone(),
        seed: inputs.seed,
        assumptions,
        confidence: confidence(inputs),
        score: inputs.path.score.clone(),
        explanation: explanation(inputs, &applications),
        rule_applications: applications,
        warnings,
        source_ids,
        rejected_alternatives: inputs.rejected_alternatives.clone(),
    }
}

/// How much the engine believes its own answer.
///
/// The reading it was given, the melody's agreement with the harmony and the
/// smoothness of the realisation all bound it; a confident number over a
/// doubtful analysis would be a lie.
pub fn confidence(inputs: &TraceInputs<'_>) -> f64 {
    let key = inputs.key.confidence.clamp(0.0, 1.0);
    let melody = inputs.path.score.get("melody_fit").clamp(0.0, 1.0);
    let analysis = inputs.analysis.confidence.clamp(0.0, 1.0);
    let leading = inputs
        .voice_leading
        .map(|v| v.score.get("voice_leading").clamp(0.0, 1.0))
        .unwrap_or(0.6);
    let value = 0.3 * key + 0.3 * melody + 0.25 * analysis + 0.15 * leading;
    (value * 1_000_000.0).round() / 1_000_000.0
}

/// Renders the explanation from the structured record.
fn explanation(inputs: &TraceInputs<'_>, applications: &[RuleApplication]) -> String {
    let symbols = inputs.path.symbols().join(" | ");
    let romans = inputs.path.romans().join(" ");
    let mut text = format!(
        "In {} under the {} profile, this candidate is the {} reading: {}. \
         In roman numerals that is {}.",
        inputs.key.label(),
        inputs.profile.id,
        inputs.strategy_label.to_lowercase(),
        symbols,
        romans
    );

    if let Some(close) = cadence_sentence(inputs) {
        text.push(' ');
        text.push_str(&close);
    }

    let applied: Vec<&RuleApplication> = applications
        .iter()
        .filter(|a| a.status == RuleStatus::Applied && a.score_delta.abs() > 0.0)
        .take(EXPLAINED_RULES)
        .collect();
    if !applied.is_empty() {
        let list: Vec<String> = applied
            .iter()
            .map(|a| format!("{} ({:+.2})", a.rule_id, a.score_delta))
            .collect();
        text.push_str(&format!(" It is shaped by {}.", list.join(", ")));
    }

    let bypassed: Vec<&RuleApplication> = applications
        .iter()
        .filter(|a| a.status == RuleStatus::Bypassed)
        .take(EXPLAINED_RULES)
        .collect();
    if !bypassed.is_empty() {
        let list: Vec<String> = bypassed.iter().map(|a| a.rule_id.clone()).collect();
        text.push_str(&format!(
            " {} {} set aside here because the style or the context makes {} exception: {}.",
            list.len(),
            if list.len() == 1 {
                "rule was"
            } else {
                "rules were"
            },
            if list.len() == 1 { "an" } else { "them" },
            list.join(", ")
        ));
    }

    if let Some(vl) = inputs.voice_leading {
        text.push_str(&format!(
            " The realisation moves {} semitones in total with a largest leap of {}",
            vl.total_motion, vl.max_leap
        ));
        let parallels = vl.true_parallels().len();
        if parallels > 0 {
            text.push_str(&format!(
                ", and carries {parallels} parallel perfect interval{}",
                if parallels == 1 { "" } else { "s" }
            ));
        }
        text.push('.');
    }

    text
}

/// A sentence about how the progression closes, when it closes on something
/// the knowledge base names.
fn cadence_sentence(inputs: &TraceInputs<'_>) -> Option<String> {
    let chords = &inputs.path.chords;
    if chords.len() < 2 {
        return None;
    }
    let a = chords[chords.len() - 2].roman.clone().unwrap_or_default();
    let b = chords[chords.len() - 1].roman.clone().unwrap_or_default();
    let base = |r: &str| -> String {
        r.chars()
            .take_while(|c| matches!(c, 'I' | 'V' | 'i' | 'v' | 'b' | '#' | '°' | 'ø' | '+'))
            .collect()
    };
    let (x, y) = (base(&a), base(&b));
    let schema = inputs.kb.cadences().iter().find(|c| {
        c.schema.degrees.len() >= 2
            && c.schema.degrees[c.schema.degrees.len() - 2] == x
            && c.schema.degrees[c.schema.degrees.len() - 1] == y
    })?;
    Some(format!(
        "It closes with a {} ({} to {}), whose closure strength is {:.2}.",
        schema.schema.name.to_lowercase(),
        a,
        b,
        schema.closure_strength
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidates::build_pools;
    use crate::params::{CancelFlag, SearchConfig};
    use crate::search::search_in;
    use crate::testing;

    fn trace_for(profile: &str) -> (testing::Harness, DecisionTrace) {
        let h = testing::harness("melodies/eight_bar_c_major", profile);
        let trace = {
            let ctx = h.context();
            let pools = build_pools(&ctx).expect("pools");
            let paths = search_in(
                &ctx,
                &pools,
                &SearchConfig::default(),
                &CancelFlag::new(),
                &mut |_, _| {},
            )
            .expect("paths");
            build_trace(&TraceInputs {
                candidate_id: "candidate-1",
                kb: ctx.kb,
                profile: ctx.profile,
                analysis: ctx.analysis,
                key: &ctx.key,
                path: &paths[0],
                strategy_label: "Functional & cadence-directed",
                voice_leading: None,
                seed: 3,
                rejected_alternatives: vec![paths[1].shape()],
                assumptions: Vec::new(),
                warnings: Vec::new(),
            })
        };
        (h, trace)
    }

    #[test]
    fn trace_cites_only_real_rules_and_sources() {
        let (h, trace) = trace_for("jazz_standard");
        assert!(!trace.rule_applications.is_empty());
        for app in &trace.rule_applications {
            assert!(
                h.kb.rule(&app.rule_id).is_some(),
                "unknown rule {}",
                app.rule_id
            );
        }
        assert!(!trace.source_ids.is_empty());
        for source in &trace.source_ids {
            assert!(h.kb.source(source).is_some(), "unknown source {source}");
        }
    }

    #[test]
    fn trace_carries_the_frozen_identity_fields() {
        let (h, trace) = trace_for("common_practice");
        assert_eq!(trace.candidate_id, "candidate-1");
        assert_eq!(trace.analysis_id, h.analysis.id);
        assert_eq!(trace.snapshot_id, h.analysis.snapshot_id);
        assert_eq!(trace.knowledge_version, h.kb.version());
        assert_eq!(trace.profile_id, "common_practice");
        assert_eq!(trace.seed, 3);
    }

    #[test]
    fn explanation_names_the_key_profile_and_chords() {
        let (_, trace) = trace_for("jazz_standard");
        assert!(trace.explanation.contains("C major"));
        assert!(trace.explanation.contains("jazz_standard"));
        assert!(trace.explanation.contains('|'), "chords are listed");
        assert!(trace.explanation.contains("roman numerals"));
    }

    #[test]
    fn explanation_mentions_rules_it_lists_in_the_trace() {
        let (_, trace) = trace_for("common_practice");
        for app in trace
            .rule_applications
            .iter()
            .filter(|a| a.status == RuleStatus::Applied && a.score_delta.abs() > 0.0)
            .take(EXPLAINED_RULES)
        {
            assert!(
                trace.explanation.contains(&app.rule_id),
                "explanation does not name {}",
                app.rule_id
            );
        }
    }

    #[test]
    fn assumptions_are_reported() {
        let (_, trace) = trace_for("modal_ambient");
        assert!(trace.assumptions.iter().any(|a| a.contains("read as")));
        assert!(trace
            .assumptions
            .iter()
            .any(|a| a.contains("harmonic grid")));
    }

    #[test]
    fn confidence_is_bounded() {
        for profile in testing::PROFILE_IDS {
            let (_, trace) = trace_for(profile);
            assert!(
                (0.0..=1.0).contains(&trace.confidence),
                "{profile} produced confidence {}",
                trace.confidence
            );
        }
    }

    #[test]
    fn rejected_alternatives_survive() {
        let (_, trace) = trace_for("cinematic");
        assert_eq!(trace.rejected_alternatives.len(), 1);
        assert!(!trace.rejected_alternatives[0].is_empty());
    }

    #[test]
    fn trace_json_round_trips() {
        let (_, trace) = trace_for("blues");
        let json = trace.to_json();
        let back = DecisionTrace::from_json(&json).expect("round trip");
        assert_eq!(back.candidate_id, trace.candidate_id);
        assert_eq!(back.profile_id, trace.profile_id);
        assert_eq!(back.seed, trace.seed);
        assert_eq!(
            json.to_canonical_string(),
            trace.to_json().to_canonical_string()
        );
    }

    #[test]
    fn explanation_is_free_of_placeholder_language() {
        for profile in testing::PROFILE_IDS {
            let (_, trace) = trace_for(profile);
            let lowered = trace.explanation.to_lowercase();
            for banned in ["todo", "tbd", "lorem", "as an ai", "probably", "i think"] {
                assert!(
                    !lowered.contains(banned),
                    "{profile} explanation contains {banned:?}"
                );
            }
            assert!(trace.explanation.len() > 40);
        }
    }
}
