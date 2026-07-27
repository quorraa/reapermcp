//! Stage 6 — global path search.
//!
//! The search optimises a whole phrase, not a beat. Choosing the best-scoring
//! chord independently at every slot is explicitly forbidden by the brief, and
//! it is also musically wrong: whether `V7` is the right chord here depends
//! entirely on what precedes and follows it.
//!
//! The method is a bounded beam search. Each partial path carries the running
//! sum of every score component; extending it costs a transition score built
//! from voice leading, functional or modal coherence, bass quality and phrase
//! direction, with the knowledge base's `chord_pair` rules folded in. Beam
//! width, per-slot pool size and total expansions are all bounded, and the
//! cancellation flag is checked in the inner loop.
//!
//! Determinism: the beam is ordered by score, then by a seeded tie-break, then
//! by chord text. The seed can only reorder options whose scores are equal to
//! within [`TIE_EPSILON`].

use crate::candidates::{
    ChordOption, STRATEGY_ALT_DOMINANT, STRATEGY_APPLIED, STRATEGY_BACKDOOR, STRATEGY_BASS_LED,
    STRATEGY_BORROWED, STRATEGY_CADENTIAL_SIX_FOUR, STRATEGY_CHROMATIC_MEDIANT, STRATEGY_DIATONIC,
    STRATEGY_DIMINISHED, STRATEGY_MODAL, STRATEGY_NEAPOLITAN, STRATEGY_PEDAL, STRATEGY_PLANING,
    STRATEGY_TRITONE_SUB,
};
use crate::ctx::{EngineContext, RULE_DELTA_SCALE};
use crate::error::HarmonyError;
use crate::factbuild::{chord_facts, context_for};
use crate::params::{CancelFlag, GenerateParams, SearchConfig};
use music_analysis::grid::GridSlot;
use music_analysis::report::Analysis;
use music_domain::prelude::*;
use qjson::rng::DetRng;
use theory_kb::rules::facts;
use theory_kb::{KnowledgeBase, ResolvedProfile, RuleEvent};

/// Score difference below which two paths count as tied and the seed decides.
pub const TIE_EPSILON: f64 = 1e-9;

/// One complete harmonisation of the grid.
#[derive(Clone, Debug)]
pub struct HarmonicPath {
    /// The chords, one per grid slot, in time order.
    pub chords: Vec<ChordEvent>,
    /// Raw score components plus the profile-weighted total.
    pub score: ScoreVector,
    /// Every rule consulted along the way.
    pub rule_applications: Vec<RuleApplication>,
    /// The strategy bias that produced it.
    pub strategy: String,
    /// The per-slot option keys, for de-duplication and tracing.
    pub option_keys: Vec<String>,
    /// Distinct source ids behind the rules that fired.
    pub source_ids: Vec<String>,
}

impl HarmonicPath {
    /// The chord symbols in order.
    pub fn symbols(&self) -> Vec<String> {
        self.chords.iter().map(|c| c.spec.render_ascii()).collect()
    }

    /// The roman numerals in order.
    pub fn romans(&self) -> Vec<String> {
        self.chords
            .iter()
            .map(|c| c.roman.clone().unwrap_or_default())
            .collect()
    }

    /// The functional path in order.
    pub fn functions(&self) -> Vec<HarmonicFunction> {
        self.chords
            .iter()
            .map(|c| c.function.unwrap_or(HarmonicFunction::Unclassified))
            .collect()
    }

    /// A stable text identity, used for de-duplication.
    pub fn signature(&self) -> String {
        self.option_keys.join(">")
    }

    /// The audible shape of the progression: what each chord is and what note
    /// is underneath it. Two paths with the same shape sound the same, however
    /// differently the search arrived at them.
    pub fn shape(&self) -> String {
        self.chords
            .iter()
            .map(|c| {
                let pcs = c.spec.pitch_classes();
                let bass = if c.spec.bass.is_some() {
                    c.spec.bass_pc()
                } else if pcs.is_empty() {
                    0
                } else {
                    pcs[(c.inversion as usize).min(pcs.len() - 1)]
                };
                format!("{}/{bass}", c.spec.render_ascii())
            })
            .collect::<Vec<String>>()
            .join(">")
    }
}

/// A named search bias.
///
/// The same pool is searched several times under different biases so the
/// diversity stage has genuinely different strategies to choose between rather
/// than neighbours of one optimum.
#[derive(Clone, Debug, PartialEq)]
pub struct StrategyBias {
    /// Stable id, e.g. `"functional"`.
    pub id: &'static str,
    /// Human-readable name for the candidate label.
    pub label: &'static str,
    /// Multipliers applied to named score components.
    pub component_bias: &'static [(&'static str, f64)],
    /// Strategies this bias rewards, and by how much.
    pub strategy_bonus: &'static [(&'static str, f64)],
}

/// Cadence-directed functional harmony.
pub const BIAS_FUNCTIONAL: StrategyBias = StrategyBias {
    id: "functional",
    label: "Functional & cadence-directed",
    component_bias: &[
        ("functional_or_modal_coherence", 1.6),
        ("bass_quality", 1.2),
        ("harmonic_coherence", 1.2),
    ],
    strategy_bonus: &[
        (STRATEGY_DIATONIC, 0.16),
        (STRATEGY_APPLIED, 0.10),
        (STRATEGY_CADENTIAL_SIX_FOUR, 0.10),
        (STRATEGY_PLANING, -0.20),
        (STRATEGY_MODAL, -0.08),
    ],
};

/// Modal and common-tone harmony: pedals, planing, slow motion.
pub const BIAS_MODAL: StrategyBias = StrategyBias {
    id: "modal_common_tone",
    label: "Modal & common-tone",
    component_bias: &[
        ("voice_leading", 1.7),
        ("style_match", 1.2),
        ("functional_or_modal_coherence", 0.6),
    ],
    strategy_bonus: &[
        (STRATEGY_MODAL, 0.20),
        (STRATEGY_PLANING, 0.22),
        (STRATEGY_PEDAL, 0.18),
        (STRATEGY_DIATONIC, 0.04),
        (STRATEGY_APPLIED, -0.16),
        (STRATEGY_CADENTIAL_SIX_FOUR, -0.12),
    ],
};

/// Chromatic and bass-led harmony: substitutions, mixture, stepwise bass.
pub const BIAS_CHROMATIC: StrategyBias = StrategyBias {
    id: "chromatic_bass_led",
    label: "Chromatic & bass-led",
    component_bias: &[
        ("bass_quality", 1.7),
        ("chromaticism_target", 1.4),
        ("harmonic_coherence", 1.1),
    ],
    strategy_bonus: &[
        (STRATEGY_BASS_LED, 0.20),
        (STRATEGY_TRITONE_SUB, 0.20),
        (STRATEGY_BORROWED, 0.18),
        (STRATEGY_DIMINISHED, 0.16),
        (STRATEGY_CHROMATIC_MEDIANT, 0.16),
        (STRATEGY_NEAPOLITAN, 0.12),
        (STRATEGY_BACKDOOR, 0.12),
        (STRATEGY_ALT_DOMINANT, 0.10),
        (STRATEGY_DIATONIC, -0.06),
    ],
};

/// Every bias, in the order the search runs them.
pub const BIASES: &[StrategyBias] = &[BIAS_FUNCTIONAL, BIAS_MODAL, BIAS_CHROMATIC];

/// Stage 6, as the frozen contract declares it.
///
/// The eight parameters are the frozen cross-crate signature; grouping them
/// would break every caller written against `CONTRACTS.md`.
#[allow(clippy::too_many_arguments)]
pub fn search_paths(
    kb: &KnowledgeBase,
    prof: &ResolvedProfile,
    an: &Analysis,
    pools: &[Vec<ChordOption>],
    p: &GenerateParams,
    cfg: &SearchConfig,
    cancel: &CancelFlag,
    progress: &mut dyn FnMut(f64, &str),
) -> Result<Vec<HarmonicPath>, HarmonyError> {
    let ctx = EngineContext::new(kb, prof, an, p)?;
    search_in(&ctx, pools, cfg, cancel, progress)
}

/// Stage 6 against an already-built context.
pub fn search_in(
    ctx: &EngineContext<'_>,
    pools: &[Vec<ChordOption>],
    cfg: &SearchConfig,
    cancel: &CancelFlag,
    progress: &mut dyn FnMut(f64, &str),
) -> Result<Vec<HarmonicPath>, HarmonyError> {
    cfg.validate()?;
    if pools.is_empty() || pools.iter().any(Vec::is_empty) {
        return Err(HarmonyError::no_valid_candidate(
            "at least one harmonic slot has no surviving chord option",
        ));
    }
    let slots = &ctx.analysis.grid.slots;
    if slots.len() != pools.len() {
        return Err(HarmonyError::invalid_argument(
            "the option pools do not line up with the harmonic grid",
        ));
    }
    let schemas = SchemaIndex::build(ctx);
    let jitter = Jitter::build(ctx.params.seed, pools);
    // The local and transition costs do not depend on the search bias, so they
    // are computed once and shared by every pass. This is what keeps a
    // sixteen-bar, three-strategy search well inside its time budget.
    let tables = Tables::build(ctx, pools, slots, cfg, &schemas, cancel)?;
    let mut out: Vec<HarmonicPath> = Vec::new();
    let per_bias = cfg.diversity_paths.div_ceil(BIASES.len()).max(1);

    for (bias_index, bias) in BIASES.iter().enumerate() {
        cancel.check()?;
        progress(
            bias_index as f64 / BIASES.len() as f64,
            &format!("searching {} paths", bias.id),
        );
        let found = beam_search(
            ctx, pools, slots, cfg, bias, &schemas, &tables, &jitter, cancel, per_bias,
        )?;
        out.extend(found);
    }
    progress(1.0, "path search complete");

    dedupe_paths(&mut out);
    sort_paths(&mut out);
    out.truncate(cfg.diversity_paths.max(1));
    if out.is_empty() {
        return Err(HarmonyError::no_valid_candidate(
            "the path search produced no complete progression",
        ));
    }
    Ok(out)
}

/// A partial path inside the beam.
#[derive(Clone, Debug)]
struct Partial {
    picks: Vec<usize>,
    sums: Vec<f64>,
    bias: f64,
    total: f64,
    jitter: f64,
}

/// Component order used by the running sums, matching `SCORE_COMPONENTS`.
fn component_index(name: &str) -> Option<usize> {
    SCORE_COMPONENTS.iter().position(|c| *c == name)
}

/// The per-slot and per-transition score components, computed once.
struct Tables {
    local: Vec<Vec<Vec<f64>>>,
    transition: Vec<Vec<Vec<Vec<f64>>>>,
}

impl Tables {
    /// Builds both tables, honouring the option cap and the cancel flag.
    fn build(
        ctx: &EngineContext<'_>,
        pools: &[Vec<ChordOption>],
        slots: &[GridSlot],
        cfg: &SearchConfig,
        schemas: &SchemaIndex,
        cancel: &CancelFlag,
    ) -> Result<Tables, HarmonyError> {
        let mut local = Vec::with_capacity(pools.len());
        for (index, pool) in pools.iter().enumerate() {
            cancel.check()?;
            let limit = pool.len().min(cfg.max_options_per_slot);
            local.push(
                pool.iter()
                    .take(limit)
                    .map(|o| local_vector(ctx, o, slots.get(index)))
                    .collect(),
            );
        }
        let mut transition = Vec::with_capacity(pools.len().saturating_sub(1));
        for index in 1..pools.len() {
            cancel.check()?;
            let previous = &pools[index - 1];
            let current = &pools[index];
            let prev_limit = previous.len().min(cfg.max_options_per_slot);
            let cur_limit = current.len().min(cfg.max_options_per_slot);
            let mut rows = Vec::with_capacity(prev_limit);
            for prev in previous.iter().take(prev_limit) {
                let mut row = Vec::with_capacity(cur_limit);
                for cur in current.iter().take(cur_limit) {
                    row.push(transition_vector(
                        ctx,
                        prev,
                        cur,
                        slots.get(index),
                        schemas,
                        index,
                        pools.len(),
                    ));
                }
                rows.push(row);
            }
            transition.push(rows);
        }
        Ok(Tables { local, transition })
    }
}

/// The beam search proper.
#[allow(clippy::too_many_arguments)] // Every argument is an independent bound or input.
fn beam_search(
    ctx: &EngineContext<'_>,
    pools: &[Vec<ChordOption>],
    slots: &[GridSlot],
    cfg: &SearchConfig,
    bias: &StrategyBias,
    schemas: &SchemaIndex,
    tables: &Tables,
    jitter: &Jitter,
    cancel: &CancelFlag,
    want: usize,
) -> Result<Vec<HarmonicPath>, HarmonyError> {
    let weights = biased_weights(ctx, bias);
    let mut beam: Vec<Partial> = Vec::new();
    let mut expansions: usize = 0;

    for (slot_index, pool) in pools.iter().enumerate() {
        cancel.check()?;
        let mut next: Vec<Partial> = Vec::with_capacity(beam.len().max(1) * pool.len());
        let limit = pool.len().min(cfg.max_options_per_slot);
        for (option_index, option) in pool.iter().enumerate().take(limit) {
            let local = &tables.local[slot_index][option_index];
            let bonus = strategy_bonus(bias, option.source_strategy);
            let option_jitter = jitter.at(slot_index, option_index);
            if beam.is_empty() {
                let sums: Vec<f64> = local.clone();
                let total = weighted(&sums, &weights) + bonus;
                next.push(Partial {
                    picks: vec![option_index],
                    sums,
                    bias: bonus,
                    total,
                    jitter: option_jitter,
                });
                continue;
            }
            for partial in &beam {
                if expansions >= cfg.max_paths {
                    break;
                }
                expansions += 1;
                let prev_index = *partial.picks.last().expect("a non-empty partial");
                let transition = &tables.transition[slot_index - 1][prev_index][option_index];
                let mut sums = partial.sums.clone();
                for (i, s) in sums.iter_mut().enumerate() {
                    *s += local[i] + transition[i];
                }
                let accumulated_bias = partial.bias + bonus;
                let n = (slot_index + 1) as f64;
                let total = weighted(&sums, &weights) + accumulated_bias / n;
                let mut picks = partial.picks.clone();
                picks.push(option_index);
                next.push(Partial {
                    picks,
                    sums,
                    bias: accumulated_bias,
                    total,
                    jitter: partial.jitter * 0.5 + option_jitter * 0.5,
                });
            }
        }
        sort_partials(&mut next, pools, slot_index);
        next.truncate(cfg.beam_width);
        beam = next;
        if beam.is_empty() {
            return Err(HarmonyError::no_valid_candidate(
                "the beam emptied before the phrase was complete",
            ));
        }
    }

    // Realising the whole beam and then de-duplicating by sound is what stops
    // the diversity stage from being handed eight spellings of one idea.
    let mut paths: Vec<HarmonicPath> = Vec::new();
    let mut shapes: Vec<String> = Vec::new();
    for partial in beam.into_iter() {
        let path = realise(ctx, pools, slots, &partial, bias, schemas)?;
        let shape = path.shape();
        if shapes.contains(&shape) {
            continue;
        }
        shapes.push(shape);
        paths.push(path);
        if paths.len() >= want {
            break;
        }
    }
    Ok(paths)
}

/// Profile weights multiplied by the bias, normalised so totals stay comparable.
fn biased_weights(ctx: &EngineContext<'_>, bias: &StrategyBias) -> Vec<f64> {
    let mut w: Vec<f64> = SCORE_COMPONENTS
        .iter()
        .map(|c| ctx.profile.weight(c))
        .collect();
    for (name, factor) in bias.component_bias {
        if let Some(i) = component_index(name) {
            w[i] *= factor;
        }
    }
    let sum: f64 = w.iter().sum();
    if sum > 0.0 {
        for x in w.iter_mut() {
            *x /= sum;
        }
    }
    w
}

/// The weighted total of a component vector.
fn weighted(sums: &[f64], weights: &[f64]) -> f64 {
    sums.iter().zip(weights).map(|(s, w)| s * w).sum()
}

/// One option's contribution, as a component vector.
fn local_vector(
    ctx: &EngineContext<'_>,
    option: &ChordOption,
    slot: Option<&GridSlot>,
) -> Vec<f64> {
    let mut v: Vec<f64> = SCORE_COMPONENTS
        .iter()
        .map(|c| option.local_score.get(c))
        .collect();
    let weight = slot
        .map(|s| 0.5 + 0.5 * s.weight.clamp(0.0, 1.0))
        .unwrap_or(1.0);
    let _ = ctx;
    for x in v.iter_mut() {
        *x *= weight;
    }
    v
}

/// How much a bias adds to a path's running total for choosing this strategy.
///
/// The bonus sits outside the score vector on purpose: it steers the *search*
/// towards a different corner of the space, while the score the caller sees is
/// the honest profile-weighted one.
fn strategy_bonus(bias: &StrategyBias, strategy: &str) -> f64 {
    bias.strategy_bonus
        .iter()
        .find(|(s, _)| *s == strategy)
        .map(|(_, b)| *b)
        .unwrap_or(0.0)
}

/// The cost of moving from one chord to the next.
#[allow(clippy::too_many_arguments)] // Every argument is an independent musical input.
fn transition_vector(
    ctx: &EngineContext<'_>,
    prev: &ChordOption,
    cur: &ChordOption,
    slot: Option<&GridSlot>,
    schemas: &SchemaIndex,
    slot_index: usize,
    slot_count: usize,
) -> Vec<f64> {
    let mut v = vec![0.0; SCORE_COMPONENTS.len()];
    let cadential = slot.map(|s| s.is_cadential).unwrap_or(false);
    let last = slot_index + 1 == slot_count;

    let vl = voice_leading_quality(prev, cur);
    let functional = functional_quality(ctx, prev, cur, schemas, cadential);
    let coherence = schema_quality(ctx, prev, cur, schemas, cadential);
    let bass = bass_quality(ctx, prev, cur);
    let direction = phrase_direction_quality(ctx, prev, cur, cadential, last, slot_index);

    set(&mut v, "voice_leading", vl);
    set(&mut v, "functional_or_modal_coherence", functional);
    set(&mut v, "harmonic_coherence", coherence);
    set(&mut v, "bass_quality", bass);
    set(&mut v, "phrase_direction", direction);

    // The knowledge base's own view of the pair.
    let mut rc = context_for(ctx, RuleEvent::ChordPair);
    chord_facts(
        &mut rc,
        &prev.spec,
        prev.function,
        category_of(ctx, prev),
        prev.inversion,
        prev.source_strategy,
        prev.chromaticism == 0.0,
        cadential,
    );
    let root_motion = (cur.spec.root_pc() - prev.spec.root_pc()).rem_euclid(12) as f64;
    rc.set_num(facts::ROOT_MOTION_SEMITONES, root_motion);
    rc.set_bool("root_motion_is_descending_fifth", root_motion == 5.0);
    rc.set_bool(
        "root_motion_is_stepwise",
        root_motion == 1.0 || root_motion == 2.0 || root_motion == 10.0 || root_motion == 11.0,
    );
    rc.set_bool(
        "root_motion_is_third_related",
        matches!(root_motion as i32, 3 | 4 | 8 | 9),
    );
    rc.set_bool("target_chord_follows", target_follows(ctx, prev, cur));
    rc.set_bool("common_tone_available", common_tones(prev, cur) > 0);
    rc.set_bool(
        "quartal_or_planing_context",
        prev.source_strategy == STRATEGY_PLANING && cur.source_strategy == STRATEGY_PLANING,
    );
    rc.set_bool(
        "chordal_seventh_resolves_down_by_step",
        seventh_resolves_down(prev, cur),
    );
    rc.set_bool(
        "suspension_resolves_down_by_step",
        suspension_resolves(prev, cur),
    );
    rc.set_bool(
        "tendency_tone_is_unresolved",
        cadential && !tendency_resolves(ctx, prev, cur),
    );
    let outcome = ctx.engine.evaluate(&rc);
    for (component, delta) in &outcome.deltas {
        if let Some(i) = component_index(component) {
            v[i] += delta * RULE_DELTA_SCALE;
        }
    }
    v
}

/// Writes a component by name.
fn set(v: &mut [f64], name: &str, value: f64) {
    if let Some(i) = component_index(name) {
        v[i] = value;
    }
}

/// Shared pitch classes between two chords.
pub fn common_tones(a: &ChordOption, b: &ChordOption) -> usize {
    let pa = a.pitch_classes();
    let pb = b.pitch_classes();
    pa.iter().filter(|pc| pb.contains(pc)).count()
}

/// How smoothly two chords can be connected, `0.0..=1.0`.
///
/// This is the pitch-class displacement a minimal realisation would need: each
/// tone of the next chord is reached from the nearest tone of the previous one.
/// The real voicing stage refines it; this is the cost the phrase-level search
/// can afford to evaluate thousands of times.
pub fn voice_leading_quality(prev: &ChordOption, cur: &ChordOption) -> f64 {
    let pa = prev.pitch_classes();
    let pb = cur.pitch_classes();
    if pa.is_empty() || pb.is_empty() {
        return 0.5;
    }
    let mut travel = 0.0;
    for pc in &pb {
        let best = pa
            .iter()
            .map(|q| {
                let d = (pc - q).rem_euclid(12);
                d.min(12 - d)
            })
            .min()
            .unwrap_or(6);
        travel += best as f64;
    }
    let mean = travel / pb.len() as f64;
    let smooth = (1.0 - mean / 3.0).clamp(0.0, 1.0);
    let shared = common_tones(prev, cur) as f64 / pb.len() as f64;
    0.65 * smooth + 0.35 * shared
}

/// The functional reading of a pair, from `functions.json` resolutions.
fn functional_quality(
    ctx: &EngineContext<'_>,
    prev: &ChordOption,
    cur: &ChordOption,
    schemas: &SchemaIndex,
    cadential: bool,
) -> f64 {
    let strength = ctx.functional_strength();
    let modal = ctx.modal_tolerance();
    let mut base = 0.45;

    if let Some(entry) = ctx.kb.functions().iter().find(|f| f.id == prev.entry_id) {
        let target = schemas.base_roman(ctx, cur);
        if entry.typical_resolutions.contains(&target) {
            base += 0.35 * strength;
        }
    }
    base += match (prev.function, cur.function) {
        (HarmonicFunction::Dominant, HarmonicFunction::Tonic) => 0.25 * strength,
        (HarmonicFunction::Predominant, HarmonicFunction::Dominant) => 0.22 * strength,
        (HarmonicFunction::Tonic, HarmonicFunction::Predominant) => 0.12 * strength,
        (HarmonicFunction::Applied, _) if target_follows(ctx, prev, cur) => 0.25 * strength,
        (HarmonicFunction::Applied, _) => -0.30 * strength,
        (HarmonicFunction::Dominant, HarmonicFunction::Predominant) => -0.18 * strength,
        (HarmonicFunction::Modal, HarmonicFunction::Modal) => 0.20 * modal,
        (HarmonicFunction::Pedal, _) | (_, HarmonicFunction::Pedal) => 0.10 * modal,
        _ => 0.0,
    };
    if prev.source_strategy == STRATEGY_CADENTIAL_SIX_FOUR && !target_follows(ctx, prev, cur) {
        // An unresolved cadential six-four is the one six-four that is simply
        // wrong; it is a dominant preparation, not a chord in its own right.
        base -= 0.45 * strength;
    }
    if cadential {
        base += match (prev.function, cur.function) {
            (HarmonicFunction::Dominant, HarmonicFunction::Tonic) => 0.2 * strength,
            (HarmonicFunction::Modal, HarmonicFunction::Tonic) => 0.2 * modal,
            (HarmonicFunction::Predominant, HarmonicFunction::Tonic) => 0.12,
            _ => -0.05,
        };
    }
    base.clamp(0.0, 1.2)
}

/// How much the pair looks like something `progressions.json` describes.
fn schema_quality(
    ctx: &EngineContext<'_>,
    prev: &ChordOption,
    cur: &ChordOption,
    schemas: &SchemaIndex,
    cadential: bool,
) -> f64 {
    let a = schemas.base_roman(ctx, prev);
    let b = schemas.base_roman(ctx, cur);
    let mut base = 0.45;
    if let Some(weight) = schemas.progression_weight(&a, &b) {
        base += 0.35 * weight;
    }
    if cadential {
        if let Some(strength) = schemas.cadence_weight(&a, &b) {
            base += 0.35 * strength;
        }
    }
    if prev.symbol() == cur.symbol() {
        // Repeating a chord is a legitimate slow-harmonic-rhythm choice, and a
        // stall in a functional style. The profile decides which.
        base -= 0.25 * (1.0 - ctx.modal_tolerance());
    }
    base.clamp(0.0, 1.0)
}

/// The bass line's own quality across the pair.
fn bass_quality(ctx: &EngineContext<'_>, prev: &ChordOption, cur: &ChordOption) -> f64 {
    let root_motion = (cur.spec.root_pc() - prev.spec.root_pc()).rem_euclid(12);
    let functional = ctx.functional_strength();
    let mut base = match root_motion {
        5 => 0.55 + 0.4 * functional,
        7 => 0.62,
        2 | 10 => 0.68,
        1 | 11 => 0.60,
        3 | 9 => 0.58,
        4 | 8 => 0.55,
        6 => 0.35,
        _ => 0.45,
    };
    let bass_step = {
        let d = (cur.bass_pc() - prev.bass_pc()).rem_euclid(12);
        d.min(12 - d)
    };
    if bass_step == 1 || bass_step == 2 {
        base += 0.12;
    }
    if bass_step == 0 && prev.symbol() != cur.symbol() {
        base += 0.06;
    }
    base.clamp(0.0, 1.0)
}

/// Where the pair sits in the phrase's shape.
fn phrase_direction_quality(
    ctx: &EngineContext<'_>,
    prev: &ChordOption,
    cur: &ChordOption,
    cadential: bool,
    last: bool,
    slot_index: usize,
) -> f64 {
    let mut base = 0.5;
    let tonic_pc = ctx.key.tonic_pc();
    let arrives_home = cur.spec.root_pc() == tonic_pc && cur.function == HarmonicFunction::Tonic;
    if last && cur.source_strategy == STRATEGY_CADENTIAL_SIX_FOUR {
        // Nothing can resolve it after the last slot.
        base -= 0.35;
    }
    if last {
        base += match ctx.params.loop_intent {
            Some(LoopIntent::OpenDominant) => {
                if cur.function == HarmonicFunction::Dominant {
                    0.35
                } else {
                    -0.1
                }
            }
            Some(LoopIntent::ModalDrone) => {
                if cur.function == HarmonicFunction::Modal || arrives_home {
                    0.3
                } else {
                    0.0
                }
            }
            _ => {
                if arrives_home {
                    0.35
                } else {
                    -0.05
                }
            }
        };
    } else if cadential {
        base += if arrives_home { 0.2 } else { 0.05 };
    } else if arrives_home && slot_index > 0 {
        // Landing on the tonic mid-phrase drains the phrase's forward motion,
        // but only in a style that has forward motion to drain.
        base -= 0.12 * ctx.functional_strength();
    }
    if prev.symbol() == cur.symbol() {
        base -= 0.08;
    }
    base.clamp(0.0, 1.0)
}

/// The `functions.json` category a generated option came from.
fn category_of(ctx: &EngineContext<'_>, option: &ChordOption) -> &'static str {
    match ctx
        .kb
        .functions()
        .iter()
        .find(|f| f.id == option.entry_id)
        .map(|f| f.category.as_str())
    {
        Some("diatonic") => "diatonic",
        Some("modal") => "modal",
        Some("borrowed") => "borrowed",
        Some("applied") => "applied",
        Some("chromatic") => "chromatic",
        Some("chromatic_mediant") => "chromatic_mediant",
        Some("neapolitan") => "neapolitan",
        Some("augmented_sixth") => "augmented_sixth",
        _ => "other",
    }
}

/// True when `cur` is the chord `prev` was pointing at.
///
/// An applied dominant points a fifth below its own root; a tritone substitute
/// and a backdoor dominant point a semitone below and a whole tone above.
pub fn target_follows(ctx: &EngineContext<'_>, prev: &ChordOption, cur: &ChordOption) -> bool {
    let motion = (cur.spec.root_pc() - prev.spec.root_pc()).rem_euclid(12);
    let _ = ctx;
    match prev.source_strategy {
        STRATEGY_APPLIED => motion == 5,
        STRATEGY_TRITONE_SUB => motion == 11,
        STRATEGY_BACKDOOR => motion == 2,
        STRATEGY_DIMINISHED => motion == 1 || motion == 11 || motion == 0,
        STRATEGY_NEAPOLITAN => motion == 6 || motion == 11,
        STRATEGY_AUGMENTED_SIXTH_ALIAS => motion == 1,
        // A six-four is a dominant that has not arrived yet: it resolves in
        // place onto a root-position dominant, and nothing else discharges it.
        STRATEGY_CADENTIAL_SIX_FOUR => {
            motion == 0 && cur.function == HarmonicFunction::Dominant && cur.inversion == 0
        }
        _ => {
            prev.function == HarmonicFunction::Dominant
                && cur.function == HarmonicFunction::Tonic
                && motion == 5
        }
    }
}

/// Local alias so the match above reads as a list of strategies.
const STRATEGY_AUGMENTED_SIXTH_ALIAS: &str = crate::candidates::STRATEGY_AUGMENTED_SIXTH;

/// True when the previous chord's seventh falls by step into this one.
pub fn seventh_resolves_down(prev: &ChordOption, cur: &ChordOption) -> bool {
    let Some(alter) = prev.spec.seventh.alter() else {
        return false;
    };
    let seventh_pc = (prev.spec.root_pc() + 11 + i32::from(alter)).rem_euclid(12);
    let target = (seventh_pc + 11).rem_euclid(12);
    cur.pitch_classes().contains(&target)
}

/// True when a suspended fourth falls by step to a third in the next chord.
pub fn suspension_resolves(prev: &ChordOption, cur: &ChordOption) -> bool {
    if !prev.spec.is_suspended() {
        return false;
    }
    let fourth = (prev.spec.root_pc() + 5).rem_euclid(12);
    let target = (fourth + 11).rem_euclid(12);
    cur.pitch_classes().contains(&target) && cur.spec.root_pc() == prev.spec.root_pc()
}

/// True when the tendency tones of `prev` find their resolutions in `cur`.
pub fn tendency_resolves(ctx: &EngineContext<'_>, prev: &ChordOption, cur: &ChordOption) -> bool {
    let pcs = cur.pitch_classes();
    let mut all = true;
    if let Some(lt) = ctx.key.leading_tone_pc() {
        if prev.pitch_classes().contains(&lt) && prev.function == HarmonicFunction::Dominant {
            all &= pcs.contains(&ctx.key.tonic_pc());
        }
    }
    if prev.spec.seventh.is_present() {
        all &= seventh_resolves_down(prev, cur);
    }
    all
}

/// Deterministic tie-breaking noise derived from the seed.
struct Jitter {
    values: Vec<Vec<f64>>,
}

impl Jitter {
    fn build(seed: u64, pools: &[Vec<ChordOption>]) -> Jitter {
        let root = DetRng::new(seed);
        let values = pools
            .iter()
            .enumerate()
            .map(|(i, pool)| {
                let mut rng = root.derive(&format!("slot:{i}"));
                pool.iter().map(|_| rng.next_f64()).collect()
            })
            .collect();
        Jitter { values }
    }

    fn at(&self, slot: usize, option: usize) -> f64 {
        self.values
            .get(slot)
            .and_then(|v| v.get(option))
            .copied()
            .unwrap_or(0.0)
    }
}

/// Deterministic beam ordering: score, then seed, then chord text.
fn sort_partials(next: &mut [Partial], pools: &[Vec<ChordOption>], slot_index: usize) {
    next.sort_by(|a, b| {
        if (a.total - b.total).abs() > TIE_EPSILON {
            return b
                .total
                .partial_cmp(&a.total)
                .unwrap_or(std::cmp::Ordering::Equal);
        }
        // Ties, and only ties, are decided by the seed.
        b.jitter
            .partial_cmp(&a.jitter)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| pick_key(a, pools, slot_index).cmp(&pick_key(b, pools, slot_index)))
    });
}

/// The text identity of a partial path, used as the final tie-break.
fn pick_key(p: &Partial, pools: &[Vec<ChordOption>], slot_index: usize) -> String {
    p.picks
        .iter()
        .enumerate()
        .map(|(i, idx)| {
            pools
                .get(slot_index + 1 - p.picks.len() + i)
                .and_then(|pool| pool.get(*idx))
                .map(|o| o.key())
                .unwrap_or_default()
        })
        .collect::<Vec<String>>()
        .join(">")
}

/// Turns a finished partial path into a scored [`HarmonicPath`].
fn realise(
    ctx: &EngineContext<'_>,
    pools: &[Vec<ChordOption>],
    slots: &[GridSlot],
    partial: &Partial,
    bias: &StrategyBias,
    schemas: &SchemaIndex,
) -> Result<HarmonicPath, HarmonyError> {
    let mut chords = Vec::with_capacity(partial.picks.len());
    let mut option_keys = Vec::with_capacity(partial.picks.len());
    let mut applications: Vec<RuleApplication> = Vec::new();
    let mut source_ids: Vec<String> = Vec::new();
    let mut options: Vec<&ChordOption> = Vec::new();

    for (i, pick) in partial.picks.iter().enumerate() {
        let option = pools[i]
            .get(*pick)
            .ok_or_else(|| HarmonyError::no_valid_candidate("beam referenced a missing option"))?;
        options.push(option);
        option_keys.push(option.key());
        let slot = &slots[i];
        let mut event = ChordEvent::new(
            i as u32,
            option.spec.clone(),
            slot.start,
            slot.end - slot.start,
        );
        event.inversion = option.inversion;
        event.function = Some(option.function);
        event.roman = Some(option.roman.clone());
        event.local_tonic = Some((ctx.key.tonic, ctx.key.scale_id.clone()));
        event.confidence = ctx.key.confidence.clamp(0.0, 1.0);
        event.inference_source = "generated".to_string();
        chords.push(event);
        for app in &option.rule_applications {
            record(&mut applications, &mut source_ids, app);
        }
    }

    // Whole-path rules: this is the only place the knowledge base sees the
    // progression as one object.
    let mut rc = context_for(ctx, RuleEvent::ProgressionPath);
    let functions: Vec<HarmonicFunction> = options.iter().map(|o| o.function).collect();
    let tags = schemas.matched_tags(ctx, &options);
    rc.set_list(facts::PROGRESSION_TAGS, &tags);
    rc.set_bool(
        "function_is_predominant",
        functions.contains(&HarmonicFunction::Predominant),
    );
    rc.set_bool(
        "function_is_dominant",
        functions.contains(&HarmonicFunction::Dominant),
    );
    rc.set_bool(
        "cadence_is_expected_at_this_slot",
        slots.iter().any(|s| s.is_cadential),
    );
    rc.set_bool(
        "chord_is_applied_dominant",
        options
            .iter()
            .any(|o| o.source_strategy == STRATEGY_APPLIED),
    );
    rc.set_bool(
        "quartal_or_planing_context",
        options
            .iter()
            .all(|o| o.source_strategy == STRATEGY_PLANING),
    );
    rc.set_bool(
        "root_motion_is_stepwise",
        stepwise_fraction(&options) >= 0.5,
    );
    rc.set_bool(
        "root_motion_is_descending_fifth",
        descending_fifth_fraction(&options) >= 0.5,
    );
    rc.set_bool(
        "chordal_seventh_resolves_down_by_step",
        options
            .windows(2)
            .all(|w| !w[0].spec.seventh.is_present() || seventh_resolves_down(w[0], w[1])),
    );
    rc.set_bool("stepwise_connection_available", true);
    rc.set_bool("outer_voice_span_exceeds_two_octaves", false);
    rc.set_bool("candidate_duplicates_existing_strategy", false);
    let outcome = ctx.engine.evaluate(&rc);
    for app in &outcome.applications {
        record(&mut applications, &mut source_ids, app);
    }

    let n = partial.picks.len().max(1) as f64;
    let mut score = ScoreVector::new();
    for (i, name) in SCORE_COMPONENTS.iter().enumerate() {
        score.set(name, partial.sums[i] / n);
    }
    for (component, delta) in &outcome.deltas {
        score.add(component, delta * RULE_DELTA_SCALE);
    }
    score.set("candidate_diversity", 0.0);
    let weights: Vec<(&str, f64)> = SCORE_COMPONENTS
        .iter()
        .map(|c| (*c, ctx.profile.weight(c)))
        .collect();
    score.recompute_total(&weights);

    Ok(HarmonicPath {
        chords,
        score,
        rule_applications: applications,
        strategy: bias.id.to_string(),
        option_keys,
        source_ids,
    })
}

/// Fraction of transitions whose roots move by step.
fn stepwise_fraction(options: &[&ChordOption]) -> f64 {
    if options.len() < 2 {
        return 0.0;
    }
    let n = options.len() - 1;
    let hits = options
        .windows(2)
        .filter(|w| {
            let d = (w[1].spec.root_pc() - w[0].spec.root_pc()).rem_euclid(12);
            matches!(d, 1 | 2 | 10 | 11)
        })
        .count();
    hits as f64 / n as f64
}

/// Fraction of transitions whose roots fall a fifth.
fn descending_fifth_fraction(options: &[&ChordOption]) -> f64 {
    if options.len() < 2 {
        return 0.0;
    }
    let n = options.len() - 1;
    let hits = options
        .windows(2)
        .filter(|w| (w[1].spec.root_pc() - w[0].spec.root_pc()).rem_euclid(12) == 5)
        .count();
    hits as f64 / n as f64
}

/// Appends a rule application and its sources, without duplicates.
fn record(into: &mut Vec<RuleApplication>, sources: &mut Vec<String>, app: &RuleApplication) {
    if app.status == RuleStatus::NotApplicable {
        return;
    }
    if !into
        .iter()
        .any(|a| a.rule_id == app.rule_id && a.status == app.status)
    {
        into.push(app.clone());
    }
    for s in &app.source_ids {
        if !sources.iter().any(|e| e == s) {
            sources.push(s.clone());
        }
    }
}

/// Drops paths that pick exactly the same options.
fn dedupe_paths(paths: &mut Vec<HarmonicPath>) {
    let mut seen: Vec<String> = Vec::new();
    paths.retain(|p| {
        let sig = p.shape();
        if seen.contains(&sig) {
            false
        } else {
            seen.push(sig);
            true
        }
    });
}

/// Deterministic path ordering.
pub fn sort_paths(paths: &mut [HarmonicPath]) {
    paths.sort_by(|a, b| {
        b.score
            .total()
            .partial_cmp(&a.score.total())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.strategy.cmp(&b.strategy))
            .then_with(|| a.signature().cmp(&b.signature()))
    });
}

/// Progression and cadence bigrams drawn from `knowledge/`.
///
/// Built once per search so the transition cost can ask "does the knowledge
/// base describe this pair?" without rescanning 51 schemas each time.
pub struct SchemaIndex {
    progression: Vec<(String, String, f64, String)>,
    cadence: Vec<(String, String, f64, String)>,
    tags: Vec<(String, String, Vec<String>)>,
}

impl SchemaIndex {
    /// Indexes every schema the active profile and scale admit.
    pub fn build(ctx: &EngineContext<'_>) -> SchemaIndex {
        let mut progression = Vec::new();
        let mut cadence = Vec::new();
        let mut tags = Vec::new();
        for schema in ctx.kb.progressions() {
            if !schema
                .style_profiles
                .iter()
                .any(|p| ctx.profile.chain.iter().any(|c| c == p))
            {
                continue;
            }
            let scale_ok = schema.applicable_scales.is_empty()
                || schema.applicable_scales.contains(&ctx.key.scale_id);
            let weight = if scale_ok { 1.0 } else { 0.5 };
            for pair in schema.degrees.windows(2) {
                progression.push((
                    normalise_roman(&pair[0]),
                    normalise_roman(&pair[1]),
                    weight,
                    schema.id.clone(),
                ));
                tags.push((
                    normalise_roman(&pair[0]),
                    normalise_roman(&pair[1]),
                    schema.tags.clone(),
                ));
            }
        }
        for schema in ctx.kb.cadences() {
            if !schema
                .schema
                .style_profiles
                .iter()
                .any(|p| ctx.profile.chain.iter().any(|c| c == p))
            {
                continue;
            }
            for pair in schema.schema.degrees.windows(2) {
                cadence.push((
                    normalise_roman(&pair[0]),
                    normalise_roman(&pair[1]),
                    schema.closure_strength.clamp(0.0, 1.0),
                    schema.schema.id.clone(),
                ));
            }
        }
        SchemaIndex {
            progression,
            cadence,
            tags,
        }
    }

    /// The best progression weight for a roman-numeral pair.
    pub fn progression_weight(&self, a: &str, b: &str) -> Option<f64> {
        self.progression
            .iter()
            .filter(|(x, y, _, _)| x == a && y == b)
            .map(|(_, _, w, _)| *w)
            .fold(None, |acc, w| Some(acc.map_or(w, |m: f64| m.max(w))))
    }

    /// The best cadence closure strength for a roman-numeral pair.
    pub fn cadence_weight(&self, a: &str, b: &str) -> Option<f64> {
        self.cadence
            .iter()
            .filter(|(x, y, _, _)| x == a && y == b)
            .map(|(_, _, w, _)| *w)
            .fold(None, |acc, w| Some(acc.map_or(w, |m: f64| m.max(w))))
    }

    /// The schema ids a pair matches, for the trace.
    pub fn matching_schemas(&self, a: &str, b: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .progression
            .iter()
            .chain(self.cadence.iter())
            .filter(|(x, y, _, _)| x == a && y == b)
            .map(|(_, _, _, id)| id.clone())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The tags of every schema the path touches.
    pub fn matched_tags(&self, ctx: &EngineContext<'_>, options: &[&ChordOption]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for pair in options.windows(2) {
            let a = self.base_roman(ctx, pair[0]);
            let b = self.base_roman(ctx, pair[1]);
            for (x, y, tags) in &self.tags {
                if *x == a && *y == b {
                    for t in tags {
                        if !out.contains(t) {
                            out.push(t.clone());
                        }
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// The bare roman numeral of an option, as the schemas write it.
    pub fn base_roman(&self, ctx: &EngineContext<'_>, option: &ChordOption) -> String {
        match ctx.kb.functions().iter().find(|f| f.id == option.entry_id) {
            Some(entry) => normalise_roman(&entry.roman),
            None => normalise_roman(&option.roman),
        }
    }
}

/// Strips the figures and quality suffixes a schema does not write.
fn normalise_roman(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            'I' | 'V' | 'i' | 'v' | 'b' | '#' | '°' | 'ø' | '+' => out.push(ch),
            _ => break,
        }
    }
    if out.is_empty() {
        text.trim().to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidates::build_pools;
    use crate::testing;

    fn run(fixture: &str, profile: &str) -> (testing::Harness, Vec<HarmonicPath>) {
        let h = testing::harness(fixture, profile);
        let paths = {
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
        (h, paths)
    }

    #[test]
    fn search_returns_complete_paths() {
        let (h, paths) = run("melodies/eight_bar_c_major", "common_practice");
        assert!(!paths.is_empty());
        for path in &paths {
            assert_eq!(path.chords.len(), h.analysis.grid.slots.len());
            for (chord, slot) in path.chords.iter().zip(&h.analysis.grid.slots) {
                assert_eq!(chord.onset, slot.start);
                assert_eq!(chord.duration, slot.end - slot.start);
                assert!(chord.duration.is_positive());
            }
        }
    }

    #[test]
    fn search_is_deterministic() {
        let (_, a) = run("melodies/eight_bar_c_major", "jazz_standard");
        let (_, b) = run("melodies/eight_bar_c_major", "jazz_standard");
        let sa: Vec<String> = a.iter().map(|p| p.signature()).collect();
        let sb: Vec<String> = b.iter().map(|p| p.signature()).collect();
        assert_eq!(sa, sb);
    }

    #[test]
    fn search_reaches_every_bias() {
        let (_, paths) = run("melodies/eight_bar_c_major", "jazz_standard");
        let mut ids: Vec<&str> = paths.iter().map(|p| p.strategy.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), BIASES.len(), "every bias should contribute");
    }

    #[test]
    fn search_is_not_greedy() {
        // A greedy walk would pick the top-scoring option in every slot. The
        // beam must be able to do better than that on the whole phrase.
        let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
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
        let greedy: Vec<String> = pools.iter().map(|p| p[0].key()).collect();
        assert!(
            paths.iter().any(|p| p.option_keys != greedy),
            "the search never departed from the greedy choice"
        );
    }

    #[test]
    fn cancellation_stops_the_search() {
        let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
        let ctx = h.context();
        let pools = build_pools(&ctx).expect("pools");
        let cancel = CancelFlag::new();
        cancel.cancel();
        let e = search_in(
            &ctx,
            &pools,
            &SearchConfig::default(),
            &cancel,
            &mut |_, _| {},
        )
        .expect_err("cancelled");
        assert!(e.is_cancelled());
    }

    #[test]
    fn progress_is_reported() {
        let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
        let ctx = h.context();
        let pools = build_pools(&ctx).expect("pools");
        let mut seen: Vec<f64> = Vec::new();
        search_in(
            &ctx,
            &pools,
            &SearchConfig::default(),
            &CancelFlag::new(),
            &mut |p, _| seen.push(p),
        )
        .expect("paths");
        assert!(!seen.is_empty());
        assert!(seen.iter().all(|p| (0.0..=1.0).contains(p)));
        assert_eq!(seen.last().copied(), Some(1.0));
    }

    #[test]
    fn empty_pools_are_rejected() {
        let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
        let ctx = h.context();
        let e = search_in(
            &ctx,
            &[],
            &SearchConfig::default(),
            &CancelFlag::new(),
            &mut |_, _| {},
        )
        .expect_err("empty pools");
        assert_eq!(e.code, crate::error::NO_VALID_CANDIDATE);
    }

    #[test]
    fn voice_leading_quality_prefers_common_tones() {
        let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
        let ctx = h.context();
        let pool = crate::candidates::build_slot_options(&ctx, &ctx.analysis.grid.slots[0]);
        let c = pool
            .iter()
            .find(|o| o.symbol().starts_with('C') && o.inversion == 0)
            .expect("a C chord");
        let am = pool.iter().find(|o| o.symbol().starts_with("Am"));
        let fs = pool.iter().find(|o| o.symbol().starts_with("F#"));
        if let (Some(am), Some(fs)) = (am, fs) {
            assert!(voice_leading_quality(c, am) > voice_leading_quality(c, fs));
        }
    }

    #[test]
    fn schema_index_finds_the_ii_v_bigram() {
        let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
        let ctx = h.context();
        let index = SchemaIndex::build(&ctx);
        assert!(index.progression_weight("ii", "V").is_some());
        assert!(index.cadence_weight("V", "I").is_some());
        assert!(!index.matching_schemas("V", "I").is_empty());
    }

    #[test]
    fn roman_normalisation_strips_figures() {
        assert_eq!(normalise_roman("ii7"), "ii");
        assert_eq!(normalise_roman("Imaj7"), "I");
        assert_eq!(normalise_roman("bVII7"), "bVII");
        assert_eq!(normalise_roman("V7"), "V");
        assert_eq!(normalise_roman("iiø7"), "iiø");
        assert_eq!(normalise_roman("vii°"), "vii°");
    }

    #[test]
    fn seventh_resolution_is_detected() {
        let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
        let ctx = h.context();
        let pool = crate::candidates::build_slot_options(&ctx, &ctx.analysis.grid.slots[0]);
        let g7 = pool.iter().find(|o| o.symbol() == "G7");
        let c = pool
            .iter()
            .find(|o| o.symbol() == "C" || o.symbol() == "Cmaj7");
        if let (Some(g7), Some(c)) = (g7, c) {
            assert!(seventh_resolves_down(g7, c), "F should fall to E");
        }
    }
}
