//! Stage 9 — the arrangement plan.
//!
//! This is where the three planning levels meet: the whole-loop shape from
//! [`crate::energy`], the section controls from [`crate::sections`], the phrase
//! frame from [`crate::phrase`], realised through [`crate::patterns`] and then
//! measured by [`crate::masking`] and [`crate::density`].
//!
//! The order of operations matters and is deliberate:
//!
//! 1. roles are matched to catalogued patterns and instrument profiles;
//! 2. registers are allocated foreground-first, then pushed apart;
//! 3. each part is realised section by section, so the section controls are
//!    audible rather than nominal;
//! 4. masking is measured *before* and *after* separation, and the separation
//!    is kept only if it actually helped;
//! 5. hard constraints — range, MIDI bounds, positive durations, polyphony,
//!    span containment — are enforced on the material that will be written,
//!    not trusted from the stages above;
//! 6. the knowledge base's arrangement rules are evaluated over the finished
//!    plan, and their deltas become the score.

use crate::density::{self, PartMetrics};
use crate::energy::{self, LoopPlan};
use crate::error::ArrangementError;
use crate::masking::{self, MaskingReport};
use crate::params::ArrangementParams;
use crate::patterns::{self, RealizeOptions};
use crate::phrase::PhraseFrame;
use crate::roles::{self, Priority};
use crate::sections::{self, SectionPlan};
use harmony_engine::CancelFlag;
use music_analysis::report::Analysis;
use music_domain::prelude::*;
use theory_kb::{
    rules::facts, ArrangementPattern, InstrumentProfile, KnowledgeBase, ResolvedProfile,
    RuleContext, RuleEngine, RuleEvent,
};

/// First note id an arranged part uses, chosen so it never collides with the
/// melody, the bass or the harmony ids `harmony-engine` hands out.
pub const ARRANGEMENT_ID_BASE: NoteId = 500_000;

/// Note ids reserved per part.
pub const PART_ID_STRIDE: NoteId = 10_000;

/// How much a raw knowledge-base rule delta contributes to a score component.
///
/// The same scale `harmony-engine` uses, so the two engines' vectors are
/// comparable.
pub const RULE_DELTA_SCALE: f64 = 0.1;

/// What one role was given, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct RoleAssignment {
    /// The role.
    pub role: ArrangementRole,
    /// The `arrangement_patterns.json` id realising it.
    pub pattern_id: String,
    /// The `instrument_profiles.json` id it is written for.
    pub instrument_profile: String,
    /// The MIDI window actually allocated.
    pub register: (i32, i32),
    /// Measured attacks per quarter note.
    pub density: f64,
    /// Measured maximum simultaneous notes.
    pub polyphony: usize,
    /// `0` foreground, `1` midground, `2` background.
    pub priority: u8,
    /// Section ids the role sounds in.
    pub sections: Vec<String>,
    /// Why this pattern, this instrument, this window.
    pub rationale: String,
}

impl RoleAssignment {
    /// JSON form.
    pub fn to_json(&self) -> qjson::Json {
        qjson::json_obj! {
            "role" => self.role.id(),
            "pattern_id" => self.pattern_id.clone(),
            "instrument_profile" => self.instrument_profile.clone(),
            "register_low" => i64::from(self.register.0),
            "register_high" => i64::from(self.register.1),
            "density" => self.density,
            "polyphony" => self.polyphony as i64,
            "priority" => i64::from(self.priority),
            "sections" => qjson::Json::Arr(
                self.sections.iter().map(|s| qjson::Json::Str(s.clone())).collect()
            ),
            "rationale" => self.rationale.clone(),
        }
    }
}

/// The finished arrangement.
///
/// The first six fields are the frozen contract. The rest are additive: the
/// measurements the plan was built from, kept so a caller can audit the
/// decision instead of taking it on trust.
#[derive(Clone, Debug)]
pub struct ArrangementPlan {
    /// The parts, in allocation order. A role that was withheld has no part.
    pub parts: Vec<Part>,
    /// One entry per requested role, including withheld ones.
    pub assignments: Vec<RoleAssignment>,
    /// The energy curve actually used, sampled per bar.
    pub energy: Vec<(BeatTime, f64)>,
    /// The profile-weighted score vector.
    pub score: ScoreVector,
    /// Every arrangement rule that was evaluated.
    pub rule_applications: Vec<RuleApplication>,
    /// Everything the caller should know about.
    pub warnings: Vec<Warning>,
    /// Masking measured on the un-separated realisation.
    pub masking_before: MaskingReport,
    /// Masking measured on the realisation that was kept.
    pub masking_after: MaskingReport,
    /// The whole-loop plan.
    pub loop_plan: LoopPlan,
    /// The section-level plan.
    pub sections: Vec<SectionPlan>,
    /// Per-part measurements, parallel to `parts`.
    pub metrics: Vec<PartMetrics>,
    /// The span the plan covers.
    pub span: (BeatTime, BeatTime),
}

impl ArrangementPlan {
    /// How many parts actually sound.
    pub fn layer_count(&self) -> usize {
        self.parts.iter().filter(|p| !p.notes.is_empty()).count()
    }

    /// Every note in the plan, ordered.
    pub fn notes(&self) -> Vec<&Note> {
        let mut all: Vec<&Note> = self.parts.iter().flat_map(|p| p.notes.iter()).collect();
        all.sort_by(|a, b| {
            a.onset
                .cmp(&b.onset)
                .then_with(|| a.midi.cmp(&b.midi))
                .then_with(|| a.id.cmp(&b.id))
        });
        all
    }

    /// The part written for a role, if any.
    pub fn part(&self, role: ArrangementRole) -> Option<&Part> {
        self.parts.iter().find(|p| p.role == role)
    }

    /// The measurements of the part written for a role, if any.
    pub fn metrics_for(&self, role: ArrangementRole) -> Option<&PartMetrics> {
        self.parts
            .iter()
            .position(|p| p.role == role)
            .and_then(|i| self.metrics.get(i))
    }

    /// The assignment for a role, if it was requested.
    pub fn assignment(&self, role: ArrangementRole) -> Option<&RoleAssignment> {
        self.assignments.iter().find(|a| a.role == role)
    }

    /// The canonical JSON form. Deterministic by construction: every container
    /// is an ordered vector and every map is insertion-ordered.
    pub fn to_json(&self) -> qjson::Json {
        qjson::json_obj! {
            "span_start" => self.span.0.to_display(),
            "span_end" => self.span.1.to_display(),
            "parts" => qjson::Json::Arr(self.parts.iter().map(Part::to_json).collect()),
            "assignments" => qjson::Json::Arr(
                self.assignments.iter().map(RoleAssignment::to_json).collect()
            ),
            "energy" => qjson::Json::Arr(self.energy.iter().map(|(qn, v)| qjson::json_obj!{
                "qn" => qn.to_display(),
                "value" => *v,
            }).collect()),
            "score" => self.score.to_json(),
            "rule_applications" => qjson::Json::Arr(
                self.rule_applications.iter().map(RuleApplication::to_json).collect()
            ),
            "warnings" => qjson::Json::Arr(self.warnings.iter().map(Warning::to_json).collect()),
            "masking_before" => self.masking_before.to_json(),
            "masking_after" => self.masking_after.to_json(),
            "loop_plan" => self.loop_plan.to_json(),
            "sections" => qjson::Json::Arr(self.sections.iter().map(SectionPlan::to_json).collect()),
            "metrics" => qjson::Json::Arr(self.metrics.iter().map(PartMetrics::to_json).collect()),
            "layer_count" => self.layer_count() as i64,
        }
    }

    /// A stable fingerprint of the plan, for determinism tests.
    pub fn fingerprint(&self) -> String {
        qjson::sha256::sha256_hex(self.to_json().to_canonical_string().as_bytes())
    }
}

/// One role as it is being built.
struct Build<'k> {
    role: ArrangementRole,
    pattern: &'k ArrangementPattern,
    instrument: &'k InstrumentProfile,
    priority: Priority,
    stagger: usize,
    base_window: (i32, i32),
    bounds: (i32, i32),
    window: (i32, i32),
    notes: Vec<Note>,
    withheld_sections: Vec<String>,
    rationale: String,
}

/// Stage 9's entry point, as the frozen contract declares it.
pub fn arrange(
    kb: &KnowledgeBase,
    an: &Analysis,
    candidate: &Candidate,
    p: &ArrangementParams,
    cancel: &CancelFlag,
) -> Result<ArrangementPlan, ArrangementError> {
    p.validate()?;
    let profile = kb
        .resolve_profile(&p.profile_id)
        .map_err(|e| ArrangementError::invalid_argument(format!("profile {}: {e}", p.profile_id)))?;
    let chords = &candidate.chords;
    if chords.is_empty() {
        return Err(ArrangementError::no_harmony(
            "the candidate carries no chords to arrange",
        ));
    }
    cancel.check()?;

    let tm = an.extraction.melody.time_map.clone();
    let span = (
        chords[0].onset,
        chords
            .iter()
            .map(ChordEvent::end)
            .fold(chords[0].onset, BeatTime::max),
    );
    if span.1 <= span.0 {
        return Err(ArrangementError::no_harmony(
            "the candidate's chords cover no time",
        ));
    }

    let requested: Vec<ArrangementRole> = if p.roles.is_empty() {
        roles::default_roles(&profile)
    } else {
        p.roles.clone()
    };
    if requested.is_empty() {
        return Err(ArrangementError::invalid_argument(
            "no roles were requested and the profile declares none",
        ));
    }

    let curve = energy::resolve_curve(&p.energy_curve, &p.sections, span, &profile);
    let loop_plan = energy::plan_loop(&curve, chords, span, &tm, &requested, p.loop_intent);
    let section_plans = sections::plan_sections(
        &p.sections,
        span,
        &tm,
        &curve,
        requested.len(),
        p.density,
    );
    let frame = PhraseFrame::from_analysis(an, span);
    cancel.check()?;

    let mut warnings: Vec<Warning> = Vec::new();
    // The positions within a bar that the lead already speaks for. Pattern
    // selection reads these so a supporting role can choose a grid that fits
    // between the tune's attacks instead of on top of them.
    let contested: Vec<BeatTime> = {
        let mut v: Vec<BeatTime> = frame
            .contested_onsets()
            .iter()
            .map(|qn| tm.position_in_bar(*qn))
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let mut builds = select_builds(
        kb,
        &profile,
        &requested,
        p,
        &section_plans,
        &contested,
        &mut warnings,
    )?;
    if builds.is_empty() {
        return Err(ArrangementError::no_valid_plan(
            "no requested role could be matched to a pattern and an instrument",
        ));
    }

    // Foreground first: the part the listener follows chooses its register
    // before anything else does.
    let ordered: Vec<(ArrangementRole, Priority)> =
        builds.iter().map(|b| (b.role, b.priority)).collect();
    let order = roles::allocation_order(&ordered);
    for (stagger, index) in order.iter().enumerate() {
        // A role that holds the loop together never enters late; only the
        // decorative layers build in.
        builds[*index].stagger = if builds[*index].priority == Priority::Foreground
            || loop_plan.recurring_roles.contains(&builds[*index].role)
        {
            0
        } else {
            stagger.min(3)
        };
    }

    // Pass one: the un-separated realisation, purely to measure what masking
    // the raw catalogue windows would have produced.
    realize_all(
        &mut builds,
        kb,
        an,
        candidate,
        chords,
        &tm,
        span,
        p,
        &section_plans,
        &frame,
        &loop_plan,
        &curve,
        cancel,
    )?;
    let before_parts = to_parts(&builds);
    let masking_before = masking::masking_report(&before_parts);
    cancel.check()?;

    // Pass two: separate the registers and realise again. The separation is
    // kept only if the measurement says it helped.
    // Separation may move a part an octave beyond the register the catalogue
    // gave it, and no further: clearing space must not turn a mid-register comp
    // into a top-octave one, but a strict reading of the catalogue window
    // leaves six parts nowhere to go.
    let bounds: Vec<(i32, i32)> = builds.iter().map(|b| b.bounds).collect();
    let mut windows: Vec<(i32, i32)> = builds.iter().map(|b| b.base_window).collect();
    masking::separate(&mut windows, &bounds, &order, p.register_spread);
    for (i, w) in windows.iter().enumerate() {
        builds[i].window = *w;
    }
    realize_all(
        &mut builds,
        kb,
        an,
        candidate,
        chords,
        &tm,
        span,
        p,
        &section_plans,
        &frame,
        &loop_plan,
        &curve,
        cancel,
    )?;
    thin_backgrounds(&mut builds, &order);
    let mut parts = to_parts(&builds);
    let mut masking_after = masking::masking_report(&parts);
    if masking_after.count() > masking_before.count() {
        for (i, b) in builds.iter_mut().enumerate() {
            b.window = b.base_window;
            let _ = i;
        }
        realize_all(
            &mut builds,
            kb,
            an,
            candidate,
            chords,
            &tm,
            span,
            p,
            &section_plans,
            &frame,
            &loop_plan,
            &curve,
            cancel,
        )?;
        parts = to_parts(&builds);
        masking_after = masking::masking_report(&parts);
        warnings.push(Warning {
            code: "REGISTER_SEPARATION_REVERTED".to_string(),
            message: "separating the registers increased masking; the catalogue windows were kept"
                .to_string(),
            severity: Severity::Info,
        });
    }
    cancel.check()?;

    let metrics: Vec<PartMetrics> = parts
        .iter()
        .map(|part| density::measure(&part.notes, span))
        .collect();
    let assignments = build_assignments(&builds, &metrics, &parts, &section_plans);

    for (i, part) in parts.iter().enumerate() {
        if part.notes.is_empty() {
            warnings.push(Warning {
                code: "ROLE_WITHHELD".to_string(),
                message: format!(
                    "{} was requested but the energy curve left it no room to sound",
                    part.role.id()
                ),
                severity: Severity::Minor,
            });
        }
        let _ = i;
    }
    parts.retain(|part| !part.notes.is_empty());
    let metrics: Vec<PartMetrics> = parts
        .iter()
        .map(|part| density::measure(&part.notes, span))
        .collect();

    let (rule_applications, score) = evaluate(
        kb,
        &profile,
        &parts,
        &metrics,
        &builds,
        &frame,
        &masking_after,
        &section_plans,
        p,
    );

    Ok(ArrangementPlan {
        parts,
        assignments,
        energy: loop_plan.curve.clone(),
        score,
        rule_applications,
        warnings,
        masking_before,
        masking_after,
        loop_plan,
        sections: section_plans,
        metrics,
        span,
    })
}

/// Matches every requested role to a pattern and an instrument.
#[allow(clippy::too_many_arguments)] // Selection reads the request, the profile, the section plan and the lead's rhythm.
fn select_builds<'k>(
    kb: &'k KnowledgeBase,
    profile: &ResolvedProfile,
    requested: &[ArrangementRole],
    p: &ArrangementParams,
    section_plans: &[SectionPlan],
    contested: &[BeatTime],
    warnings: &mut Vec<Warning>,
) -> Result<Vec<Build<'k>>, ArrangementError> {
    let energy_target = section_plans
        .iter()
        .map(|s| s.section.energy)
        .fold(0.0, f64::max)
        .max(0.0);
    let section_role = section_plans.first().map(|s| s.section.role.as_str());
    let mut builds: Vec<Build<'k>> = Vec::with_capacity(requested.len());
    for role in requested {
        let Some(pattern) = roles::select_pattern(
            kb,
            *role,
            profile,
            p.texture_pattern.as_deref(),
            p.density,
            energy_target,
            section_role,
            if *role == ArrangementRole::Lead {
                &[]
            } else {
                contested
            },
        ) else {
            warnings.push(Warning {
                code: "NO_PATTERN_FOR_ROLE".to_string(),
                message: format!("the catalogue has no pattern for {}", role.id()),
                severity: Severity::Moderate,
            });
            continue;
        };
        let Some(instrument) = roles::select_instrument(kb, pattern, *role) else {
            warnings.push(Warning {
                code: "NO_INSTRUMENT_FOR_ROLE".to_string(),
                message: format!("no instrument profile suits {}", role.id()),
                severity: Severity::Moderate,
            });
            continue;
        };
        let substituted = pattern.role != role.id();
        if substituted {
            warnings.push(Warning {
                code: "ROLE_SUBSTITUTED".to_string(),
                message: format!(
                    "{} has no pattern of its own; {} stands in for it",
                    role.id(),
                    pattern.id
                ),
                severity: Severity::Info,
            });
        }
        let window = roles::base_window(pattern, instrument);
        let rationale = format!(
            "{} realises {} on {} in MIDI {}..{}: {} activity, {} responsibility, {} priority{}",
            pattern.name,
            role.id(),
            instrument.name,
            window.0,
            window.1,
            pattern.rhythmic_activity,
            pattern.harmonic_responsibility,
            pattern.priority,
            if substituted {
                format!(" (substituted from the {} role)", pattern.role)
            } else {
                String::new()
            }
        );
        builds.push(Build {
            role: *role,
            pattern,
            instrument,
            priority: Priority::parse(&pattern.priority),
            stagger: 0,
            base_window: window,
            bounds: separation_bounds(window, instrument),
            window,
            notes: Vec::new(),
            withheld_sections: Vec::new(),
            rationale,
        });
    }
    Ok(builds)
}

/// Realises every build, section by section.
#[allow(clippy::too_many_arguments)] // The stage genuinely needs every input; bundling them would only hide the wiring.
fn realize_all(
    builds: &mut [Build<'_>],
    kb: &KnowledgeBase,
    an: &Analysis,
    candidate: &Candidate,
    chords: &[ChordEvent],
    tm: &TimeMap,
    span: (BeatTime, BeatTime),
    p: &ArrangementParams,
    section_plans: &[SectionPlan],
    frame: &PhraseFrame,
    loop_plan: &LoopPlan,
    curve: &[(BeatTime, f64)],
    cancel: &CancelFlag,
) -> Result<(), ArrangementError> {
    let _ = kb;
    let ranks: Vec<usize> = rank_by_priority(builds);
    for index in 0..builds.len() {
        cancel.check()?;
        builds[index].notes.clear();
        builds[index].withheld_sections.clear();
        let id_base = ARRANGEMENT_ID_BASE + index as NoteId * PART_ID_STRIDE;
        let channel = (index % 16) as u8;

        if builds[index].role == ArrangementRole::Lead && p.preserve_melody {
            let notes = preserved_lead(an, candidate, span, id_base, channel);
            if !notes.is_empty() {
                builds[index].notes = notes;
                continue;
            }
        }

        let entrance = frame.entrance(builds[index].stagger);
        let silence: Vec<(BeatTime, BeatTime)> = if builds[index].priority == Priority::Foreground {
            Vec::new()
        } else {
            loop_plan.silence.clone()
        };
        let active = placement_windows(
            builds[index].role,
            builds[index].pattern,
            frame,
            section_plans,
            loop_plan,
            tm,
            span,
            entrance,
            &silence,
        );
        // Everything but the lead itself keeps out of the lead's way.
        let avoid: Vec<BeatTime> = if builds[index].role == ArrangementRole::Lead {
            Vec::new()
        } else {
            frame.contested_onsets().to_vec()
        };

        let mut notes: Vec<Note> = Vec::new();
        for sp in section_plans {
            let sec = (sp.section.start.max(span.0), sp.section.end.min(span.1));
            if sec.1 <= sec.0 {
                continue;
            }
            if ranks[index] >= sp.layer_budget {
                builds[index].withheld_sections.push(sp.section.id.clone());
                continue;
            }
            let window = shift_window(
                builds[index].window,
                builds[index].bounds,
                if builds[index].priority == Priority::Foreground {
                    0
                } else {
                    sp.register_shift
                },
            );
            let variation = frame
                .phrase_index(sec.0)
                .map(|i| frame.variation(i))
                .unwrap_or(1.0);
            let complexity_voices = ((builds[index].pattern.polyphony as f64)
                * (0.4 + 0.6 * sp.harmonic_complexity))
                .ceil()
                .max(1.0) as usize;
            let mut opts = RealizeOptions::new(window, sec);
            opts.density = (sp.density * variation).clamp(0.0, 1.0);
            opts.velocity_scale = sp.velocity_scale;
            opts.note_length_scale = sp.note_length_scale;
            opts.max_polyphony = complexity_voices
                .min(builds[index].pattern.polyphony.max(1) as usize)
                .min(builds[index].instrument.polyphony.max(1) as usize);
            opts.id_base = id_base + notes.len() as NoteId;
            opts.channel = channel;
            opts.note_role = note_role(builds[index].role);
            opts.seed = p.seed ^ (index as u64) << 8 ^ (sp.index as u64);
            opts.avoid_onsets = avoid.clone();
            opts.active = clip_windows(&active, sec);
            opts.energy = curve.to_vec();
            if opts.active.is_empty() {
                builds[index].withheld_sections.push(sp.section.id.clone());
                continue;
            }
            let written = patterns::realize_with(
                builds[index].pattern,
                chords,
                tm,
                builds[index].instrument,
                &opts,
            )?;
            notes.extend(written);
        }

        if !patterns::is_sustained(builds[index].pattern) {
            let bar = tm.meter_at(span.0).bar_length_qn();
            let minimum = bar.scale(1, 8);
            density::ensure_rest(&mut notes, &frame.phrases, minimum);
        }
        enforce(
            &mut notes,
            builds[index].instrument,
            span,
            builds[index].pattern.polyphony.max(1) as usize,
        );
        patterns::renumber(&mut notes, id_base);
        builds[index].notes = notes;
    }
    Ok(())
}

/// Where in the span a role is allowed to sound.
///
/// This is the phrase- and section-level placement the brief asks for, and it
/// is what stops a transition figure being sprayed across the whole loop:
///
/// * a **transition** occupies the half-bar before each section change and the
///   bar before the loop wrap, and nothing else;
/// * an **impact** lands on the section arrivals it is announcing;
/// * a pattern whose rhythmic activity is `"phrase_gaps"` — the answering
///   figures — sounds only where the lead is silent;
/// * everything else runs from its entrance to the end, minus any strategic
///   silence the whole-loop plan asked for.
#[allow(clippy::too_many_arguments)] // Placement is a function of the whole frame; each argument is a distinct level of the plan.
pub fn placement_windows(
    role: ArrangementRole,
    pattern: &ArrangementPattern,
    frame: &PhraseFrame,
    section_plans: &[SectionPlan],
    loop_plan: &LoopPlan,
    tm: &TimeMap,
    span: (BeatTime, BeatTime),
    entrance: BeatTime,
    silence: &[(BeatTime, BeatTime)],
) -> Vec<(BeatTime, BeatTime)> {
    let bar = tm.meter_at(span.0).bar_length_qn();
    match role {
        ArrangementRole::Transition => {
            let mut out: Vec<(BeatTime, BeatTime)> = Vec::new();
            for sp in section_plans.iter().filter(|s| s.transition_prep) {
                if let Some(w) = frame.fill_window(sp.section.end, bar) {
                    out.push(w);
                }
            }
            if let Some(qn) = loop_plan.transition_qn {
                let w = (qn.max(span.0), span.1);
                if w.1 > w.0 && !out.contains(&w) {
                    out.push(w);
                }
            }
            out.sort();
            out.dedup();
            if out.is_empty() {
                vec![(span.1 - bar.min(span.1 - span.0), span.1)]
            } else {
                out
            }
        }
        ArrangementRole::Impact => {
            let mut out: Vec<(BeatTime, BeatTime)> = section_plans
                .iter()
                .map(|sp| {
                    (
                        sp.section.start.max(span.0),
                        (sp.section.start + bar).min(span.1),
                    )
                })
                .filter(|(s, e)| e > s)
                .collect();
            out.sort();
            out.dedup();
            out
        }
        _ if pattern.rhythmic_activity == "phrase_gaps" => {
            let windows = frame.answer_windows();
            if windows.is_empty() {
                vec![(entrance, span.1)]
            } else {
                windows
            }
        }
        _ => frame.active_windows(entrance, silence),
    }
}

/// Which roles cannot be muted without a chord losing its third or seventh.
///
/// `arrangement.layer_removal_preserves_chord_identity` is a rule about what a
/// breakdown may take away; this is the measurement behind it.
pub fn essential_layers(parts: &[Part], chords: &[ChordEvent]) -> Vec<ArrangementRole> {
    parts
        .iter()
        .filter(|part| !guide_tones_survive_without(parts, chords, part.role))
        .map(|part| part.role)
        .collect()
}

/// True when every chord still has a guide tone sounding once `role` is muted.
pub fn guide_tones_survive_without(
    parts: &[Part],
    chords: &[ChordEvent],
    role: ArrangementRole,
) -> bool {
    for chord in chords {
        let guides = chord.spec.guide_tones();
        if guides.is_empty() {
            continue;
        }
        let wanted: Vec<i32> = guides
            .iter()
            .map(|d| (chord.spec.root_pc() + d.simple_semitones()).rem_euclid(12))
            .collect();
        let sounding: Vec<i32> = parts
            .iter()
            .filter(|part| part.role != role)
            .flat_map(|part| part.notes.iter())
            .filter(|n| n.onset < chord.end() && chord.onset < n.end())
            .map(|n| n.midi.rem_euclid(12))
            .collect();
        if !wanted.iter().any(|pc| sounding.contains(pc)) {
            return false;
        }
    }
    true
}

/// The candidate's own lead material, preserved note for note.
fn preserved_lead(
    an: &Analysis,
    candidate: &Candidate,
    span: (BeatTime, BeatTime),
    id_base: NoteId,
    channel: u8,
) -> Vec<Note> {
    let source: Vec<&Note> = candidate
        .parts
        .iter()
        .find(|part| part.role == ArrangementRole::Lead)
        .map(|part| part.notes.iter().collect())
        .unwrap_or_else(|| an.extraction.melody.notes.iter().collect());
    let mut notes: Vec<Note> = source
        .into_iter()
        .filter(|n| n.onset >= span.0 && n.onset < span.1)
        .cloned()
        .collect();
    for n in notes.iter_mut() {
        n.channel = channel;
        n.role = NoteRole::Melody;
        n.voice = VoiceId(0);
        if n.end() > span.1 {
            n.duration = span.1 - n.onset;
        }
    }
    notes.retain(|n| n.duration.is_positive());
    patterns::renumber(&mut notes, id_base);
    notes
}

/// The priority rank of every build, `0` being the most important.
fn rank_by_priority(builds: &[Build<'_>]) -> Vec<usize> {
    let ordered: Vec<(ArrangementRole, Priority)> =
        builds.iter().map(|b| (b.role, b.priority)).collect();
    let order = roles::allocation_order(&ordered);
    let mut ranks = vec![0usize; builds.len()];
    for (rank, index) in order.iter().enumerate() {
        ranks[*index] = rank;
    }
    ranks
}

/// How far beyond its catalogue window a part may be displaced.
pub const SEPARATION_HEADROOM_SEMITONES: i32 = 12;

/// The window separation and section contrast are allowed to work inside.
pub fn separation_bounds(catalogue: (i32, i32), inst: &InstrumentProfile) -> (i32, i32) {
    (
        (catalogue.0 - SEPARATION_HEADROOM_SEMITONES).max(inst.range.low_midi),
        (catalogue.1 + SEPARATION_HEADROOM_SEMITONES).min(inst.range.high_midi),
    )
}

/// The narrowest window a section-level register shift will leave behind.
pub const MIN_SHIFTED_WINDOW: i32 = 12;

/// Displaces a window for section-level register contrast.
///
/// The shift is clamped into the window the catalogue gave the part — the
/// pattern's range intersected with the instrument's — so a section can move a
/// layer up or down without ever moving it out of the register the data says it
/// belongs in.
fn shift_window(window: (i32, i32), catalogue: (i32, i32), shift: i32) -> (i32, i32) {
    if shift == 0 || catalogue.1 - catalogue.0 < MIN_SHIFTED_WINDOW {
        return window;
    }
    let lo = (window.0 + shift).clamp(catalogue.0, catalogue.1 - MIN_SHIFTED_WINDOW);
    let hi = (window.1 + shift).clamp(lo + MIN_SHIFTED_WINDOW, catalogue.1);
    (lo, hi)
}

/// Intersects a set of windows with a span.
fn clip_windows(windows: &[(BeatTime, BeatTime)], span: (BeatTime, BeatTime)) -> Vec<(BeatTime, BeatTime)> {
    windows
        .iter()
        .copied()
        .map(|(s, e)| (s.max(span.0), e.min(span.1)))
        .filter(|(s, e)| e > s)
        .collect()
}

/// The note-level role a part's notes carry.
fn note_role(role: ArrangementRole) -> NoteRole {
    match role {
        ArrangementRole::Lead => NoteRole::Melody,
        ArrangementRole::Counterlead => NoteRole::Counter,
        ArrangementRole::Bass => NoteRole::Bass,
        ArrangementRole::Pad
        | ArrangementRole::HarmonicBed
        | ArrangementRole::Comping
        | ArrangementRole::Ostinato
        | ArrangementRole::Riff => NoteRole::Harmony,
        ArrangementRole::Texture | ArrangementRole::Ambience => NoteRole::Pedal,
        ArrangementRole::Percussion | ArrangementRole::Pulse => NoteRole::Percussion,
        ArrangementRole::Ornament
        | ArrangementRole::EarCandy
        | ArrangementRole::Transition
        | ArrangementRole::Impact => NoteRole::Ornament,
    }
}

/// The hard constraints, enforced on the material that will actually be
/// written: instrument range, MIDI bounds, strictly positive durations, span
/// containment, monophony where the pattern declares it, and the instrument's
/// polyphony ceiling.
pub fn enforce(
    notes: &mut Vec<Note>,
    inst: &InstrumentProfile,
    span: (BeatTime, BeatTime),
    max_polyphony: usize,
) {
    let low = inst.range.low_midi.clamp(0, 127);
    let high = inst.range.high_midi.clamp(0, 127);
    for n in notes.iter_mut() {
        let mut midi = n.midi;
        let mut octaves = 0;
        while midi < low && midi + 12 <= high {
            midi += 12;
            octaves += 1;
        }
        while midi > high && midi - 12 >= low {
            midi -= 12;
            octaves -= 1;
        }
        if octaves != 0 {
            n.pitch = SpelledPitch::new(n.pitch.letter, n.pitch.accidental, n.pitch.octave + octaves);
        }
        n.midi = midi.clamp(low, high).clamp(0, 127);
        if n.onset < span.0 {
            let shrink = span.0 - n.onset;
            n.duration = n.duration - shrink;
            n.onset = span.0;
        }
        if n.end() > span.1 {
            n.duration = span.1 - n.onset;
        }
        if n.velocity == 0 {
            n.velocity = 1;
        }
    }
    notes.retain(|n| n.duration.is_positive() && n.onset >= span.0 && n.onset < span.1);
    notes.sort_by(|a, b| {
        a.onset
            .cmp(&b.onset)
            .then_with(|| a.midi.cmp(&b.midi))
            .then_with(|| a.voice.cmp(&b.voice))
    });

    if max_polyphony <= 1 {
        // Monophonic parts may not overlap: the earlier note gives way.
        let mut kept: Vec<Note> = Vec::with_capacity(notes.len());
        for note in notes.drain(..) {
            if let Some(prev) = kept.last_mut() {
                if prev.onset == note.onset {
                    continue;
                }
                if prev.end() > note.onset {
                    prev.duration = note.onset - prev.onset;
                }
            }
            kept.push(note);
        }
        kept.retain(|n| n.duration.is_positive());
        *notes = kept;
        return;
    }

    // Polyphonic parts may not exceed the instrument's simultaneous-note
    // ceiling; the innermost voices are the ones dropped.
    let ceiling = max_polyphony.min(inst.polyphony.max(1) as usize);
    let mut kept: Vec<Note> = Vec::with_capacity(notes.len());
    for note in notes.iter() {
        let sounding = kept.iter().filter(|k| k.overlaps(note)).count();
        if sounding < ceiling {
            kept.push(note.clone());
        }
    }
    *notes = kept;
}

/// Applies onset-density control to background parts after separation.
fn thin_backgrounds(builds: &mut [Build<'_>], order: &[usize]) {
    let mut busy: Vec<BeatTime> = Vec::new();
    for index in order {
        let priority = builds[*index].priority;
        // A sustained part is exempt: a pad that stops articulating stops being
        // a pad, and thinning it would take chord tones out of the arrangement.
        let thinnable =
            priority == Priority::Background && !patterns::is_sustained(builds[*index].pattern);
        if thinnable && !busy.is_empty() {
            let mut onsets: Vec<BeatTime> = builds[*index].notes.iter().map(|n| n.onset).collect();
            onsets.sort();
            onsets.dedup();
            let floor = (onsets.len() * 2 / 3).max(1);
            density::thin_against(&mut builds[*index].notes, &busy, floor);
        }
        if priority != Priority::Background {
            for n in &builds[*index].notes {
                if !busy.contains(&n.onset) {
                    busy.push(n.onset);
                }
            }
            busy.sort();
        }
    }
}

/// Turns builds into parts.
fn to_parts(builds: &[Build<'_>]) -> Vec<Part> {
    builds
        .iter()
        .enumerate()
        .map(|(i, b)| Part {
            role: b.role,
            name: format!("{} — {}", b.role.id(), b.pattern.name),
            notes: b.notes.clone(),
            instrument_profile: Some(b.instrument.id.clone()),
            channel: (i % 16) as u8,
            polyphonic: b.pattern.polyphony > 1,
        })
        .collect()
}

/// Builds the assignment record for every requested role.
fn build_assignments(
    builds: &[Build<'_>],
    metrics: &[PartMetrics],
    parts: &[Part],
    section_plans: &[SectionPlan],
) -> Vec<RoleAssignment> {
    builds
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let m = metrics.get(i);
            let sections: Vec<String> = section_plans
                .iter()
                .filter(|s| !b.withheld_sections.contains(&s.section.id))
                .map(|s| s.section.id.clone())
                .collect();
            let rationale = if parts.get(i).map(|p| p.notes.is_empty()).unwrap_or(true) {
                format!("{} — withheld: the energy curve left no layer budget", b.rationale)
            } else {
                b.rationale.clone()
            };
            RoleAssignment {
                role: b.role,
                pattern_id: b.pattern.id.clone(),
                instrument_profile: b.instrument.id.clone(),
                register: b.window,
                density: m.map(|m| m.onsets_per_qn).unwrap_or(0.0),
                polyphony: m.map(|m| m.max_polyphony).unwrap_or(0),
                priority: b.priority.rank(),
                sections,
                rationale,
            }
        })
        .collect()
}

/// Evaluates the knowledge base's arrangement rules over the finished plan.
#[allow(clippy::too_many_arguments)] // Every argument is a distinct fact source the rules read.
fn evaluate(
    kb: &KnowledgeBase,
    profile: &ResolvedProfile,
    parts: &[Part],
    metrics: &[PartMetrics],
    builds: &[Build<'_>],
    frame: &PhraseFrame,
    masking: &MaskingReport,
    section_plans: &[SectionPlan],
    p: &ArrangementParams,
) -> (Vec<RuleApplication>, ScoreVector) {
    let engine = RuleEngine::new(kb, profile);
    let mut applications: Vec<RuleApplication> = Vec::new();
    let mut score = ScoreVector::new();
    for component in SCORE_COMPONENTS {
        score.set(component, 0.0);
    }

    for (i, part) in parts.iter().enumerate() {
        let Some(build) = builds.iter().find(|b| b.role == part.role) else {
            continue;
        };
        let m = &metrics[i];
        let mut ctx = RuleContext::new();
        ctx.set_event(RuleEvent::PartGenerated);
        ctx.set_str(facts::ROLE, part.role.id());
        ctx.set_num(facts::PART_DENSITY, m.onsets_per_qn);
        ctx.set_num(
            facts::ROLE_DENSITY_TARGET,
            role_density_target(build.pattern, build.instrument),
        );
        ctx.set_num(
            facts::ONSET_COLLISION_RATIO,
            collision_ratio(part, frame, parts),
        );
        ctx.set_num(
            facts::REGISTER_OVERLAP_SEMITONES,
            f64::from(max_register_overlap(i, parts)),
        );
        ctx.set_num(facts::PART_POLYPHONY, m.max_polyphony as f64);
        ctx.set_num(facts::INSTRUMENT_POLYPHONY, build.instrument.polyphony as f64);
        if let Some((lo, hi)) = m.register {
            ctx.set_num(facts::MIN_SOUNDING_MIDI, f64::from(lo));
            ctx.set_num(facts::MAX_SOUNDING_MIDI, f64::from(hi));
        }
        ctx.set_num(
            facts::INSTRUMENT_LOW_MIDI,
            f64::from(build.instrument.range.low_midi),
        );
        ctx.set_num(
            facts::INSTRUMENT_HIGH_MIDI,
            f64::from(build.instrument.range.high_midi),
        );
        ctx.set_num(facts::COMPLEXITY_TARGET, p.density);
        ctx.set_bool(
            "phrase_contains_no_rest",
            phrase_without_rest(&part.notes, frame),
        );
        ctx.set_bool(
            "doubling_is_allowed_for_role",
            patterns::allows_doubling(build.pattern),
        );
        ctx.set_bool(
            "role_is_pad_or_sustained",
            patterns::is_sustained(build.pattern),
        );
        ctx.set_bool("common_tone_available", tied_notes(&part.notes) > 0);
        ctx.set_bool("pedal_point_is_active", build.pattern.role == "texture");
        ctx.set_bool("intentional_cluster", false);
        let outcome = engine.evaluate(&ctx);
        for (component, delta) in &outcome.deltas {
            score.add(component, delta * RULE_DELTA_SCALE);
        }
        applications.extend(outcome.applications);
    }

    // The plan as a whole.
    let mut ctx = RuleContext::new();
    ctx.set_event(RuleEvent::ArrangementPlan);
    let lead = parts
        .iter()
        .find(|part| part.role == ArrangementRole::Lead)
        .map(|part| part.role.id())
        .unwrap_or_else(|| {
            parts
                .first()
                .map(|part| part.role.id())
                .unwrap_or(ArrangementRole::HarmonicBed.id())
        });
    ctx.set_str(facts::ROLE, lead);
    ctx.set_num(
        facts::REGISTER_OVERLAP_SEMITONES,
        f64::from(worst_register_overlap(parts)),
    );
    ctx.set_num(facts::PART_DENSITY, mean_density(metrics));
    ctx.set_num(facts::ROLE_DENSITY_TARGET, target_density(builds));
    ctx.set_bool(
        "doubling_is_allowed_for_role",
        builds.iter().any(|b| patterns::allows_doubling(b.pattern)),
    );
    ctx.set_bool(
        "role_is_pad_or_sustained",
        builds.iter().all(|b| patterns::is_sustained(b.pattern)),
    );
    ctx.set_bool("contrast_is_dynamics_only", contrast_is_dynamics_only(section_plans));
    ctx.set_bool(
        "cadence_is_expected_at_this_slot",
        !frame.cadences.is_empty(),
    );
    ctx.set_bool("is_loop_wrap_boundary", false);
    ctx.set_bool(
        "essential_chord_tone_only_in_this_layer",
        essential_tone_isolated(parts),
    );
    ctx.set_bool("intentional_cluster", false);
    ctx.set_bool("quartal_or_planing_context", false);
    let outcome = engine.evaluate(&ctx);
    for (component, delta) in &outcome.deltas {
        score.add(component, delta * RULE_DELTA_SCALE);
    }
    applications.extend(outcome.applications);

    // The engine's own measurements, alongside the rules'.
    let clarity = 1.0 - (masking.register_overlap * 0.5).clamp(0.0, 1.0);
    score.add("arrangement_clarity", clarity);
    score.set(
        "complexity_target",
        1.0 - (mean_density(metrics) - p.density).abs().clamp(0.0, 1.0),
    );
    score.set(
        "loop_compatibility",
        if parts.iter().all(|part| {
            part.notes
                .iter()
                .all(|n| n.end() <= section_plans.last().map(|s| s.section.end).unwrap_or(n.end()))
        }) {
            1.0
        } else {
            0.0
        },
    );
    score.set(
        "phrase_direction",
        if frame.cadences.is_empty() { 0.5 } else { 1.0 },
    );

    let weights: Vec<(&str, f64)> = SCORE_COMPONENTS
        .iter()
        .map(|c| (*c, profile.weight(c)))
        .collect();
    score.recompute_total(&weights);
    applications.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    applications.dedup_by(|a, b| a.rule_id == b.rule_id && a.status == b.status);
    (applications, score)
}

/// The density budget a role's pattern and instrument agree on.
pub fn role_density_target(pattern: &ArrangementPattern, inst: &InstrumentProfile) -> f64 {
    let onsets = pattern.rhythm.onsets.len() as f64;
    let bar = pattern.rhythm.bar_length_qn.as_f64().max(1.0);
    let nominal = onsets / bar;
    let preference = 0.5 * pattern.density + 0.5 * inst.density_preference;
    nominal * (0.5 + preference)
}

/// How much of a part's articulation lands on a higher-priority part's.
fn collision_ratio(part: &Part, frame: &PhraseFrame, parts: &[Part]) -> f64 {
    if part.notes.is_empty() {
        return 0.0;
    }
    let mut busy: Vec<BeatTime> = frame.contested_onsets().to_vec();
    for other in parts {
        if other.role == part.role {
            continue;
        }
        if other.role == ArrangementRole::Lead {
            for n in &other.notes {
                if !busy.contains(&n.onset) {
                    busy.push(n.onset);
                }
            }
        }
    }
    if busy.is_empty() {
        return 0.0;
    }
    let hits = part.notes.iter().filter(|n| busy.contains(&n.onset)).count();
    hits as f64 / part.notes.len() as f64
}

/// The worst register overlap between one part and any other, in semitones.
fn max_register_overlap(index: usize, parts: &[Part]) -> i32 {
    let Some(mine) = masking::part_register(&parts[index]) else {
        return 0;
    };
    let mut worst = 0;
    for (i, other) in parts.iter().enumerate() {
        if i == index {
            continue;
        }
        if let Some(theirs) = masking::part_register(other) {
            worst = worst.max(masking::window_overlap(mine, theirs));
        }
    }
    worst
}

/// The worst register overlap anywhere in the plan.
fn worst_register_overlap(parts: &[Part]) -> i32 {
    (0..parts.len())
        .map(|i| max_register_overlap(i, parts))
        .max()
        .unwrap_or(0)
}

/// True when some phrase of this part sounds without a break.
fn phrase_without_rest(notes: &[Note], frame: &PhraseFrame) -> bool {
    if notes.is_empty() {
        return false;
    }
    frame
        .phrases
        .iter()
        .any(|w| density::rest_ratio(notes, *w) <= 0.0)
}

/// How many notes are longer than the grid they started on — a proxy for a
/// common tone having been tied rather than restruck.
fn tied_notes(notes: &[Note]) -> usize {
    notes
        .iter()
        .filter(|n| n.duration > BeatTime::from_quarters(2))
        .count()
}

/// Mean measured density across the plan.
fn mean_density(metrics: &[PartMetrics]) -> f64 {
    if metrics.is_empty() {
        return 0.0;
    }
    metrics.iter().map(|m| m.onsets_per_qn).sum::<f64>() / metrics.len() as f64
}

/// Mean density target across the plan.
fn target_density(builds: &[Build<'_>]) -> f64 {
    if builds.is_empty() {
        return 0.0;
    }
    builds
        .iter()
        .map(|b| role_density_target(b.pattern, b.instrument))
        .sum::<f64>()
        / builds.len() as f64
}

/// True when adjacent sections differ only in how loud they are.
fn contrast_is_dynamics_only(section_plans: &[SectionPlan]) -> bool {
    if section_plans.len() < 2 {
        return false;
    }
    section_plans.windows(2).any(|w| {
        (w[0].velocity_scale - w[1].velocity_scale).abs() > 0.01
            && w[0].register_shift == w[1].register_shift
            && (w[0].density - w[1].density).abs() < 0.01
            && (w[0].note_length_scale - w[1].note_length_scale).abs() < 0.01
            && w[0].layer_budget == w[1].layer_budget
    })
}

/// True when some sounding pitch class exists in exactly one part, so removing
/// that layer would change the chord.
fn essential_tone_isolated(parts: &[Part]) -> bool {
    let sounding: Vec<Vec<i32>> = parts
        .iter()
        .map(|p| {
            let mut pcs: Vec<i32> = p.notes.iter().map(|n| n.midi.rem_euclid(12)).collect();
            pcs.sort_unstable();
            pcs.dedup();
            pcs
        })
        .collect();
    for (i, mine) in sounding.iter().enumerate() {
        for pc in mine {
            if !sounding
                .iter()
                .enumerate()
                .any(|(j, other)| j != i && other.contains(pc))
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    #[test]
    fn the_default_request_produces_a_plan() {
        let h = testing::harness("melodies/eight_bar_c_major", "pop_rock");
        let plan = h.arrange(&ArrangementParams::default()).expect("a plan");
        assert!(!plan.parts.is_empty());
        assert!(plan.layer_count() >= 1);
        assert!(!plan.notes().is_empty());
        assert!(!plan.energy.is_empty());
    }

    #[test]
    fn role_density_target_reads_the_catalogue() {
        let kb = KnowledgeBase::embedded();
        let block = kb
            .arrangement_patterns()
            .iter()
            .find(|p| p.id == "arr_block_chords")
            .expect("pattern");
        let pad = kb
            .arrangement_patterns()
            .iter()
            .find(|p| p.id == "arr_sustained_pad")
            .expect("pattern");
        let piano = kb.instrument_profile("piano_keys").expect("instrument");
        assert!(role_density_target(block, piano) > role_density_target(pad, piano));
    }

    #[test]
    fn enforce_folds_out_of_range_notes_back_in() {
        let kb = KnowledgeBase::embedded();
        let bass = kb.instrument_profile("bass").expect("instrument");
        let mut notes = vec![{
            let mut n = Note::new(
                0,
                SpelledPitch::from_midi(84, None),
                BeatTime::ZERO,
                BeatTime::from_quarters(1),
            );
            n.midi = 84;
            n
        }];
        enforce(
            &mut notes,
            bass,
            (BeatTime::ZERO, BeatTime::from_quarters(4)),
            1,
        );
        assert_eq!(notes.len(), 1);
        assert!(bass.range.contains(notes[0].midi));
        assert_eq!(notes[0].pitch.midi(), notes[0].midi);
    }

    #[test]
    fn enforce_clips_notes_to_the_span() {
        let kb = KnowledgeBase::embedded();
        let piano = kb.instrument_profile("piano_keys").expect("instrument");
        let mut notes = vec![{
            let mut n = Note::new(
                0,
                SpelledPitch::from_midi(60, None),
                BeatTime::ZERO,
                BeatTime::from_quarters(8),
            );
            n.midi = 60;
            n
        }];
        enforce(
            &mut notes,
            piano,
            (BeatTime::ZERO, BeatTime::from_quarters(4)),
            4,
        );
        assert_eq!(notes[0].end(), BeatTime::from_quarters(4));
    }

    #[test]
    fn enforce_removes_monophonic_overlap() {
        let kb = KnowledgeBase::embedded();
        let lead = kb.instrument_profile("melody_lead").expect("instrument");
        let mut notes: Vec<Note> = [60, 64]
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let mut n = Note::new(
                    i as NoteId,
                    SpelledPitch::from_midi(*m, None),
                    BeatTime::from_quarters(i as i64),
                    BeatTime::from_quarters(4),
                );
                n.midi = *m;
                n
            })
            .collect();
        enforce(
            &mut notes,
            lead,
            (BeatTime::ZERO, BeatTime::from_quarters(8)),
            1,
        );
        for w in notes.windows(2) {
            assert!(!w[0].overlaps(&w[1]));
        }
    }
}
