//! Stage 5 — the per-slot chord pool.
//!
//! Options are generated from `knowledge/` rather than from a chord table in
//! this file: `functions.json` supplies the harmonic vocabulary of the key,
//! `chord_qualities.json` supplies the sonorities, and the active profile's
//! `harmonic_vocabulary` decides which of them the style actually uses.
//!
//! Two things happen here and nowhere else:
//!
//! * **Hard violations are filtered before the path search.** A chord that
//!   fails a `hard_integrity` or `mathematical_invariant` rule never reaches
//!   stage 6 at all.
//! * **Nothing else is rejected.** An option the style dislikes keeps its place
//!   in the pool with a low `style_match` component, because the score vector —
//!   not a filter — is what expresses taste.

use crate::ctx::{EngineContext, RULE_DELTA_SCALE};
use crate::error::HarmonyError;
use crate::factbuild::{chord_facts, context_for, is_symmetric, melody_facts};
use crate::keyctx::{function_class, roman_label, spec_from_quality, Degree, KeyContext};
use crate::params::GenerateParams;
use music_analysis::grid::GridSlot;
use music_analysis::report::Analysis;
use music_domain::prelude::*;
use theory_kb::{ChordQuality, FunctionEntry, KnowledgeBase, ResolvedProfile, RuleEvent};

/// Plain diatonic harmony of the active key.
pub const STRATEGY_DIATONIC: &str = "diatonic";
/// Harmony characteristic of the mode rather than the key.
pub const STRATEGY_MODAL: &str = "modal";
/// A chord borrowed from the parallel mode.
pub const STRATEGY_BORROWED: &str = "borrowed";
/// A secondary dominant or secondary leading-tone chord.
pub const STRATEGY_APPLIED: &str = "applied";
/// A tritone substitute for the expected dominant.
pub const STRATEGY_TRITONE_SUB: &str = "tritone_sub";
/// The backdoor dominant.
pub const STRATEGY_BACKDOOR: &str = "backdoor";
/// A chromatic-mediant relationship.
pub const STRATEGY_CHROMATIC_MEDIANT: &str = "chromatic_mediant";
/// Neapolitan harmony.
pub const STRATEGY_NEAPOLITAN: &str = "neapolitan";
/// Augmented-sixth harmony.
pub const STRATEGY_AUGMENTED_SIXTH: &str = "augmented_sixth";
/// A passing or common-tone diminished chord.
pub const STRATEGY_DIMINISHED: &str = "diminished";
/// A cadential six-four.
pub const STRATEGY_CADENTIAL_SIX_FOUR: &str = "cadential_six_four";
/// Pedal harmony.
pub const STRATEGY_PEDAL: &str = "pedal";
/// Constant-structure planing.
pub const STRATEGY_PLANING: &str = "planing";
/// An inversion chosen for the bass line rather than for the harmony.
pub const STRATEGY_BASS_LED: &str = "bass_led";
/// One explicit realisation of an `alt` dominant.
pub const STRATEGY_ALT_DOMINANT: &str = "alt_dominant";
/// The chord that was already there, kept as an option in its own right.
pub const STRATEGY_EXISTING: &str = "existing";

/// Every strategy id, in the order the pool builder emits them.
pub const STRATEGIES: &[&str] = &[
    STRATEGY_DIATONIC,
    STRATEGY_MODAL,
    STRATEGY_BORROWED,
    STRATEGY_APPLIED,
    STRATEGY_TRITONE_SUB,
    STRATEGY_BACKDOOR,
    STRATEGY_CHROMATIC_MEDIANT,
    STRATEGY_NEAPOLITAN,
    STRATEGY_AUGMENTED_SIXTH,
    STRATEGY_DIMINISHED,
    STRATEGY_CADENTIAL_SIX_FOUR,
    STRATEGY_PEDAL,
    STRATEGY_PLANING,
    STRATEGY_BASS_LED,
    STRATEGY_ALT_DOMINANT,
    STRATEGY_EXISTING,
];

/// Sonorities considered per function entry, after ranking by style fit.
pub const MAX_QUALITIES_PER_ENTRY: usize = 4;

/// The explicit tension sets an `alt` dominant expands to.
///
/// `alt` names a family, not a scale. The engine writes out every realisation
/// it will consider and lets melody, destination, style and voice leading pick
/// between them, so nothing about the choice is hidden.
pub const ALT_TENSION_SETS: &[&[&str]] = &[
    &["b9", "b13"],
    &["#9", "b13"],
    &["b9", "#11"],
    &["#9", "#11"],
    &["b9", "#9", "b13"],
    &["b5", "b9"],
];

/// One chord the engine is willing to place in one slot.
#[derive(Clone, Debug)]
pub struct ChordOption {
    /// Semantic chord identity.
    pub spec: ChordSpec,
    /// Functional reading.
    pub function: HarmonicFunction,
    /// Roman-numeral label.
    pub roman: String,
    /// Which generation source produced it.
    pub source_strategy: &'static str,
    /// The degrees the slot's melody notes occupy over this chord.
    pub melody_degrees: Vec<ChordDegree>,
    /// Raw score components; never collapsed away.
    pub local_score: ScoreVector,
    /// Every rule consulted while scoring it.
    pub rule_applications: Vec<RuleApplication>,
    /// Fraction of the chord foreign to the key, `0.0..=1.0`.
    pub chromaticism: f64,
    /// How much vocabulary the chord asks for, `0.0..=1.0`.
    pub complexity: f64,
    /// The `functions.json` entry it came from, when it came from one.
    pub entry_id: String,
    /// The `chord_qualities.json` record it was built from.
    pub quality_id: String,
    /// Inversion: 0 root position, 1 first inversion, and so on.
    pub inversion: u8,
    /// Sources behind the rules that fired.
    pub source_ids: Vec<String>,
}

impl ChordOption {
    /// The ASCII chord symbol.
    pub fn symbol(&self) -> String {
        self.spec.render_ascii()
    }

    /// A stable identity used for de-duplication and deterministic ordering.
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.symbol(),
            self.roman,
            self.inversion,
            self.source_strategy
        )
    }

    /// Pitch classes of the chord tones.
    pub fn pitch_classes(&self) -> Vec<i32> {
        self.spec.pitch_classes()
    }

    /// Sounding pitch class of the bass, honouring inversion and slash bass.
    pub fn bass_pc(&self) -> i32 {
        if self.spec.bass.is_some() {
            return self.spec.bass_pc();
        }
        let pcs = self.pitch_classes();
        if pcs.is_empty() {
            return 0;
        }
        pcs[(self.inversion as usize).min(pcs.len() - 1)]
    }
}

/// Stage 5 for one slot, as the frozen contract declares it.
pub fn slot_options(
    kb: &KnowledgeBase,
    prof: &ResolvedProfile,
    an: &Analysis,
    slot: &GridSlot,
    p: &GenerateParams,
) -> Vec<ChordOption> {
    match EngineContext::new(kb, prof, an, p) {
        Ok(ctx) => build_slot_options(&ctx, slot),
        Err(_) => Vec::new(),
    }
}

/// Stage 5 against an already-built context.
pub fn build_slot_options(ctx: &EngineContext<'_>, slot: &GridSlot) -> Vec<ChordOption> {
    let mut out: Vec<ChordOption> = Vec::new();
    for entry in ctx.kb.functions() {
        if !entry_applies(ctx, entry) {
            continue;
        }
        emit_entry_options(ctx, slot, entry, &mut out);
    }
    emit_planing_options(ctx, slot, &mut out);

    // Hard filtering happens here, before any path search sees the pool.
    out.retain(|o| {
        o.rule_applications
            .iter()
            .all(|a| a.status != RuleStatus::Violated)
    });

    dedupe(&mut out);
    stratified_trim(ctx, &mut out);
    out
}

/// Whether a function entry describes harmony of the active key context.
fn entry_applies(ctx: &EngineContext<'_>, entry: &FunctionEntry) -> bool {
    if entry.scale_degree.is_none() {
        return false;
    }
    entry.key_context == "any" || entry.key_context == ctx.key.key_context_id
}

/// The strategy id a function entry belongs to.
fn strategy_of(entry: &FunctionEntry) -> &'static str {
    match entry.category.as_str() {
        "diatonic" => STRATEGY_DIATONIC,
        "modal" => STRATEGY_MODAL,
        "borrowed" => STRATEGY_BORROWED,
        "applied" => STRATEGY_APPLIED,
        "neapolitan" => STRATEGY_NEAPOLITAN,
        "augmented_sixth" => STRATEGY_AUGMENTED_SIXTH,
        "chromatic_mediant" => STRATEGY_CHROMATIC_MEDIANT,
        "chromatic" => match entry.id.as_str() {
            "tritone_substitute_of_v" => STRATEGY_TRITONE_SUB,
            "backdoor_dominant" => STRATEGY_BACKDOOR,
            _ => STRATEGY_DIMINISHED,
        },
        _ => match entry.id.as_str() {
            "cadential_six_four" => STRATEGY_CADENTIAL_SIX_FOUR,
            "modal_pedal_harmony" => STRATEGY_PEDAL,
            _ => STRATEGY_DIATONIC,
        },
    }
}

/// The base complexity a category costs, before extensions are counted.
fn category_complexity(category: &str) -> f64 {
    match category {
        "diatonic" => 0.05,
        "modal" => 0.2,
        "borrowed" => 0.4,
        "applied" => 0.45,
        "chromatic" => 0.6,
        "chromatic_mediant" => 0.7,
        "neapolitan" => 0.75,
        "augmented_sixth" => 0.85,
        _ => 0.3,
    }
}

/// How many chord tones the blended extension-density target permits.
fn tone_budget(ctx: &EngineContext<'_>) -> usize {
    let density = (0.65
        * ctx
            .profile
            .field_f64("extension_density")
            .unwrap_or(0.4)
            .clamp(0.0, 1.0)
        + 0.35 * ctx.params.extension_density)
        .clamp(0.0, 1.0);
    (3 + (density * 5.0).floor() as usize).min(7)
}

/// Emits every option one function entry gives rise to.
fn emit_entry_options(
    ctx: &EngineContext<'_>,
    slot: &GridSlot,
    entry: &FunctionEntry,
    out: &mut Vec<ChordOption>,
) {
    let Some(degree_text) = entry.scale_degree.as_deref() else {
        return;
    };
    let Some(degree) = Degree::parse(degree_text) else {
        return;
    };
    let root = ctx.key.degree_root(degree);
    let strategy = strategy_of(entry);
    let function = function_class(&entry.function_class);

    if entry.triad_quality == "any" {
        // The pedal entry describes a behaviour, not a sonority: it is realised
        // as the tonic triad of the key sounding over its own root.
        if let Some(q) = ctx.kb.chord_quality(if ctx.key.key_context_id == "minor" {
            "minor_triad"
        } else {
            "major_triad"
        }) {
            let spec = spec_from_quality(q, ctx.key.tonic);
            push_option(
                ctx, slot, out, spec, function, entry, strategy, &q.id, 0, true,
            );
        }
        return;
    }

    if entry.id == "cadential_six_four" {
        emit_cadential_six_four(ctx, slot, entry, out);
        return;
    }

    let Some(base_triad) = ctx.kb.chord_quality(&entry.triad_quality) else {
        return;
    };
    let base_seventh = ctx.kb.chord_quality(&entry.seventh_quality);
    let budget = tone_budget(ctx);

    // Rank the sonorities that could realise this function and keep only the
    // few the style actually reaches for. Scoring every one of the fifty-one
    // qualities on every entry in every slot is what makes a sixteen-bar
    // request slow, and the ones past the cap never survive the pool trim.
    let mut usable: Vec<(&ChordQuality, bool)> = Vec::new();
    for quality in ctx.kb.chord_qualities() {
        if quality.triad != base_triad.triad {
            continue;
        }
        // A few qualities are the whole point of one function entry and
        // nonsense anywhere else: an augmented sixth is not a colour a
        // Neapolitan can borrow just because both are built on a major triad.
        if crate::keyctx::SINGLE_PURPOSE_QUALITIES.contains(&quality.id.as_str())
            && quality.id != entry.triad_quality
        {
            continue;
        }
        let seventh_matches = match base_seventh {
            Some(s) => quality.seventh == s.seventh,
            None => quality.seventh == "none",
        };
        let is_base_triad = quality.id == base_triad.id;
        let is_base_seventh = base_seventh.map(|s| s.id == quality.id).unwrap_or(false);
        let tones = quality.degrees.len();

        // The vocabulary the profile does not list at all is still reachable,
        // but only at the sizes the density target already allows.
        let size_ok = tones <= budget
            || is_base_triad
            || (is_base_seventh && function == HarmonicFunction::Dominant);
        if !size_ok {
            continue;
        }
        // A quality that changes the entry's declared seventh is a genuine
        // reinterpretation (C7 as a blues tonic); it is allowed, and paid for
        // in `functional_or_modal_coherence` in proportion to how functional
        // the profile is.
        if !seventh_matches && !is_base_triad && ctx.vocabulary_rank(&quality.id) < 0.7 {
            continue;
        }

        usable.push((quality, seventh_matches));
    }

    // An `alt` symbol is a family the style either uses or does not; it is not
    // competing for a place among the ordinary sonorities, so it is emitted
    // outside the cap.
    let mut index = 0;
    while index < usable.len() {
        if usable[index].0.alt_dominant {
            let (quality, _) = usable.remove(index);
            if ctx.vocabulary_rank(&quality.id) >= 0.7 {
                emit_alt_options(ctx, slot, entry, root, quality, out);
            }
        } else {
            index += 1;
        }
    }

    let target_tones = 3.0 + 4.0 * ctx.extension_density();
    usable.sort_by(|a, b| {
        ctx.vocabulary_rank(&b.0.id)
            .partial_cmp(&ctx.vocabulary_rank(&a.0.id))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let da = (a.0.degrees.len() as f64 - target_tones).abs();
                let db = (b.0.degrees.len() as f64 - target_tones).abs();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.0.id.cmp(&b.0.id))
    });
    usable.truncate(MAX_QUALITIES_PER_ENTRY);

    for (quality, seventh_matches) in usable {
        let tones = quality.degrees.len();
        let spec = spec_from_quality(quality, root);
        push_option(
            ctx,
            slot,
            out,
            spec.clone(),
            function,
            entry,
            strategy,
            &quality.id,
            0,
            seventh_matches,
        );

        // Bass-led readings: the same harmony with the third or the fifth
        // underneath. Inversions are a bass decision, not a chord decision, so
        // they carry their own strategy id.
        if entry.category == "diatonic" && tones <= 4 {
            let pcs = spec.pitch_classes();
            for inversion in 1..(pcs.len().min(3) as u8) {
                push_option(
                    ctx,
                    slot,
                    out,
                    spec.clone(),
                    function,
                    entry,
                    STRATEGY_BASS_LED,
                    &quality.id,
                    inversion,
                    seventh_matches,
                );
            }
        }
    }
}

/// Expands an `alt` dominant into its explicit alternatives.
fn emit_alt_options(
    ctx: &EngineContext<'_>,
    slot: &GridSlot,
    entry: &FunctionEntry,
    root: (Letter, Accidental),
    quality: &ChordQuality,
    out: &mut Vec<ChordOption>,
) {
    for set in ALT_TENSION_SETS {
        let mut spec = ChordSpec {
            root,
            triad: TriadQuality::Major,
            seventh: SeventhQuality::Minor,
            extensions: Vec::new(),
            added: Vec::new(),
            alterations: set.iter().filter_map(|d| ChordDegree::parse(d)).collect(),
            omissions: Vec::new(),
            bass: None,
            // The realisation is explicit, so it is no longer an `alt` symbol.
            alt_dominant: false,
        };
        spec.normalize();
        push_option(
            ctx,
            slot,
            out,
            spec,
            function_class(&entry.function_class),
            entry,
            STRATEGY_ALT_DOMINANT,
            &quality.id,
            0,
            true,
        );
    }
}

/// The cadential six-four: tonic harmony sounding over the dominant degree.
fn emit_cadential_six_four(
    ctx: &EngineContext<'_>,
    slot: &GridSlot,
    entry: &FunctionEntry,
    out: &mut Vec<ChordOption>,
) {
    let tonic_quality = if ctx.key.key_context_id == "minor" {
        "minor_triad"
    } else {
        "major_triad"
    };
    let Some(q) = ctx.kb.chord_quality(tonic_quality) else {
        return;
    };
    let mut spec = spec_from_quality(q, ctx.key.tonic);
    spec.bass = Some(ctx.key.degree_root(Degree {
        number: 5,
        alter: 0,
    }));
    push_option(
        ctx,
        slot,
        out,
        spec,
        HarmonicFunction::Dominant,
        entry,
        STRATEGY_CADENTIAL_SIX_FOUR,
        &q.id,
        2,
        true,
    );
}

/// Constant-structure planing: the tonic sonority moved to every scale degree.
///
/// Planing is a texture rather than a function, so these options are labelled
/// `modal` and are only worth generating where the profile tolerates modal
/// harmony at all.
fn emit_planing_options(ctx: &EngineContext<'_>, slot: &GridSlot, out: &mut Vec<ChordOption>) {
    if ctx.modal_tolerance() < 0.3 {
        return;
    }
    let quality_id = planing_quality_id(ctx);
    let Some(quality) = ctx.kb.chord_quality(&quality_id) else {
        return;
    };
    let Some(entry) = ctx
        .kb
        .functions()
        .iter()
        .find(|f| f.id == format!("{}_i", ctx.key.key_context_id))
    else {
        return;
    };
    for degree in planing_degrees(&ctx.key) {
        let root = ctx.key.degree_root(degree);
        let spec = spec_from_quality(quality, root);
        push_option(
            ctx,
            slot,
            out,
            spec,
            HarmonicFunction::Modal,
            entry,
            STRATEGY_PLANING,
            &quality.id,
            0,
            true,
        );
    }
}

/// The sonority a planing chain is built from, taken from the active scale's
/// declared common chords wherever the profile actually uses one of them.
fn planing_quality_id(ctx: &EngineContext<'_>) -> String {
    for candidate in &ctx.key.scale.def.common_chords {
        if ctx.vocabulary("core").iter().any(|c| c == candidate) {
            return candidate.clone();
        }
    }
    ctx.key
        .scale
        .def
        .common_chords
        .first()
        .cloned()
        .unwrap_or_else(|| "major_triad".to_string())
}

/// The degrees a planing chain moves through: the steps of the active
/// collection, named by the diatonic degree each one sounds like.
fn planing_degrees(key: &KeyContext) -> Vec<Degree> {
    let tonic = key.tonic_pc();
    let mut out: Vec<Degree> = Vec::new();
    for pc in key.scale.pitch_classes() {
        let degree = crate::keyctx::degree_for_semitones(pc - tonic);
        if !out.contains(&degree) {
            out.push(degree);
        }
    }
    out
}

/// Scores one option and appends it to the pool.
#[allow(clippy::too_many_arguments)] // Each argument is a distinct musical fact.
fn push_option(
    ctx: &EngineContext<'_>,
    slot: &GridSlot,
    out: &mut Vec<ChordOption>,
    spec: ChordSpec,
    function: HarmonicFunction,
    entry: &FunctionEntry,
    strategy: &'static str,
    quality_id: &str,
    inversion: u8,
    seventh_matches: bool,
) {
    if spec.pitch_classes().len() < 2 {
        return;
    }
    let option = score_option(
        ctx,
        slot,
        spec,
        function,
        entry,
        strategy,
        quality_id,
        inversion,
        seventh_matches,
    );
    out.push(option);
}

/// Builds and scores a single option.
#[allow(clippy::too_many_arguments)] // Each argument is a distinct musical fact.
fn score_option(
    ctx: &EngineContext<'_>,
    slot: &GridSlot,
    spec: ChordSpec,
    function: HarmonicFunction,
    entry: &FunctionEntry,
    strategy: &'static str,
    quality_id: &str,
    inversion: u8,
    seventh_matches: bool,
) -> ChordOption {
    let chromaticism = ctx.key.chromaticism_of(&spec);
    let is_diatonic = chromaticism == 0.0;
    let tone_count = spec.chord_tones().len();
    let complexity = (0.6 * category_complexity(&entry.category)
        + 0.4 * (((tone_count as f64) - 3.0) / 4.0).clamp(0.0, 1.0))
    .clamp(0.0, 1.0);

    let notes = ctx.slot_melody(slot);
    let mut melody_degrees: Vec<ChordDegree> = Vec::new();
    let mut fit_sum = 0.0;
    let mut fit_weight = 0.0;
    for note in &notes {
        let degree = spec.degree_of_pc(note.pc);
        if let Some(d) = degree {
            if !melody_degrees.contains(&d) {
                melody_degrees.push(d);
            }
        }
        let value = melody_note_fit(ctx, &spec, degree, note.nct_confidence);
        let w = note.weight();
        fit_sum += value * w;
        fit_weight += w;
    }
    let melody_fit = if fit_weight > 0.0 {
        fit_sum / fit_weight
    } else {
        0.6
    };

    // The rule engine sees the most structural melody note in the slot, which
    // is the one a melody-fit rule is actually about.
    let mut rc = context_for(ctx, RuleEvent::ChordSelected);
    chord_facts(
        &mut rc,
        &spec,
        function,
        &entry.category,
        inversion,
        strategy,
        is_diatonic,
        slot.is_cadential,
    );
    rc.set_bool(
        "quartal_or_planing_context",
        strategy == STRATEGY_PLANING || ctx.planing_context(),
    );
    if let Some(note) = notes.iter().max_by(|a, b| {
        a.weight()
            .partial_cmp(&b.weight())
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        melody_facts(
            &mut rc,
            &spec,
            spec.degree_of_pc(note.pc),
            note.structural,
            note.salience,
            note.metric_weight,
            note.nct_confidence,
        );
    }
    let outcome = ctx.engine.evaluate(&rc);

    let mut score = ScoreVector::new();
    score.set("melody_fit", melody_fit);
    score.set("harmonic_coherence", 0.5);
    score.set(
        "functional_or_modal_coherence",
        functional_base(ctx, function, entry, seventh_matches, slot.is_cadential),
    );
    score.set("voice_leading", 0.0);
    score.set(
        "extension_appropriateness",
        extension_base(ctx, &spec, tone_count),
    );
    score.set("style_match", style_base(ctx, quality_id, entry, strategy));
    score.set("phrase_direction", 0.0);
    score.set("bass_quality", bass_base(ctx, &spec, inversion));
    score.set("arrangement_clarity", 0.5);
    score.set("loop_compatibility", 0.5);
    score.set(
        "complexity_target",
        1.0 - (complexity - ctx.complexity_target()).abs(),
    );
    score.set(
        "chromaticism_target",
        1.0 - (chromaticism - ctx.chromaticism_target()).abs(),
    );
    score.set("candidate_diversity", 0.0);
    for (component, delta) in &outcome.deltas {
        score.add(component, delta * RULE_DELTA_SCALE);
    }
    let weights: Vec<(&str, f64)> = SCORE_COMPONENTS
        .iter()
        .map(|c| (*c, ctx.profile.weight(c)))
        .collect();
    score.recompute_total(&weights);

    let roman = roman_label(entry, &spec, inversion);

    ChordOption {
        spec,
        function,
        roman,
        source_strategy: strategy,
        melody_degrees,
        local_score: score,
        rule_applications: outcome.applications,
        chromaticism,
        complexity,
        entry_id: entry.id.clone(),
        quality_id: quality_id.to_string(),
        inversion,
        source_ids: outcome.source_ids,
    }
}

/// How well one melody note sits over a chord.
fn melody_note_fit(
    ctx: &EngineContext<'_>,
    spec: &ChordSpec,
    degree: Option<ChordDegree>,
    nct_confidence: f64,
) -> f64 {
    match degree {
        // A natural eleventh sitting over a real major third is the one
        // consonance-by-membership that is not one. It is a soft cost, not a
        // ban, and it disappears the moment the third does.
        Some(d) if is_natural_eleventh(d) && has_major_third(spec) => 0.55,
        Some(d) if d.number <= 7 && d.alter == 0 => 1.0,
        Some(d) if d.number <= 7 => 0.9,
        Some(d) if matches!(d.number, 9 | 11 | 13) && d.alter == 0 => 0.85,
        Some(_) => 0.7,
        None => {
            // Not a chord tone at all. A tone the analysis has already
            // explained is a colour; an unexplained one is a clash — but the
            // clash is scored, never rejected.
            let explained = 0.12 + 0.38 * nct_confidence.clamp(0.0, 1.0);
            if ctx.key.is_modal {
                explained + 0.06
            } else {
                explained
            }
        }
    }
}

/// True for a natural eleventh, however the symbol spells it.
pub fn is_natural_eleventh(d: ChordDegree) -> bool {
    d.alter == 0 && (d.number == 11 || d.number == 4)
}

/// True when the chord sounds a real, unsuspended major third.
pub fn has_major_third(spec: &ChordSpec) -> bool {
    !spec.omits(3)
        && !spec.is_suspended()
        && matches!(spec.triad, TriadQuality::Major | TriadQuality::Augmented)
}

/// The functional-coherence baseline of a chord in its slot.
fn functional_base(
    ctx: &EngineContext<'_>,
    function: HarmonicFunction,
    entry: &FunctionEntry,
    seventh_matches: bool,
    cadential: bool,
) -> f64 {
    let strength = ctx.functional_strength();
    let modal = ctx.modal_tolerance();
    let mut base = match function {
        HarmonicFunction::Tonic => 0.7,
        HarmonicFunction::Dominant => 0.65,
        HarmonicFunction::Predominant => 0.6,
        HarmonicFunction::Applied => 0.45,
        HarmonicFunction::Modal => 0.3 + 0.5 * modal,
        HarmonicFunction::Pedal => 0.3 + 0.4 * modal,
        HarmonicFunction::Chromatic | HarmonicFunction::Passing | HarmonicFunction::Neighbor => 0.4,
        HarmonicFunction::Unclassified => 0.3,
    };
    if cadential {
        base += match function {
            HarmonicFunction::Dominant => 0.25 * strength,
            HarmonicFunction::Tonic => 0.2 * strength,
            HarmonicFunction::Predominant => 0.1 * strength,
            _ => -0.1 * strength,
        };
    }
    if entry.category == "diatonic" {
        base += 0.1 * strength;
    }
    if !seventh_matches {
        // Reinterpreting the entry's declared seventh costs exactly as much as
        // the profile cares about function.
        base -= 0.45 * strength;
    }
    base.clamp(0.0, 1.0)
}

/// The extension baseline: how close the sonority's size is to the target.
fn extension_base(ctx: &EngineContext<'_>, spec: &ChordSpec, tone_count: usize) -> f64 {
    let target = ctx.extension_density();
    let actual = (((tone_count as f64) - 3.0) / 4.0).clamp(0.0, 1.0);
    let mut base = 1.0 - (actual - target).abs();
    if has_major_third(spec)
        && spec
            .chord_tones()
            .iter()
            .any(|(d, _)| is_natural_eleventh(*d))
    {
        base -= 0.25;
    }
    if !spec.alterations.is_empty() {
        base -= 0.1
            * (1.0
                - ctx
                    .profile
                    .field_f64("alteration_preference")
                    .unwrap_or(0.4));
    }
    base.clamp(0.0, 1.0)
}

/// The style baseline: what the profile's vocabulary thinks of the sonority.
fn style_base(
    ctx: &EngineContext<'_>,
    quality_id: &str,
    entry: &FunctionEntry,
    strategy: &str,
) -> f64 {
    let mut base = ctx.vocabulary_rank(quality_id);
    if strategy == STRATEGY_PLANING || strategy == STRATEGY_PEDAL {
        base = base * 0.5 + 0.5 * ctx.modal_tolerance();
    }
    if entry.category != "diatonic" {
        base *= 0.55 + 0.45 * ctx.chromaticism_target();
    }
    base.clamp(0.0, 1.0)
}

/// The bass baseline: root position is the safe default, and how safe depends
/// on the profile.
fn bass_base(ctx: &EngineContext<'_>, spec: &ChordSpec, inversion: u8) -> f64 {
    let root_position_bias = ctx.functional_strength();
    if inversion == 0 && spec.bass.is_none() {
        0.45 + 0.3 * root_position_bias
    } else {
        0.45 + 0.2 * (1.0 - root_position_bias)
    }
}

/// Drops exact duplicates, keeping the highest-scoring reading of each chord.
fn dedupe(out: &mut Vec<ChordOption>) {
    out.sort_by(|a, b| {
        a.key()
            .cmp(&b.key())
            .then_with(|| {
                b.local_score
                    .total()
                    .partial_cmp(&a.local_score.total())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.entry_id.cmp(&b.entry_id))
    });
    out.dedup_by(|a, b| a.key() == b.key());
}

/// Trims the pool to the configured size while keeping every strategy present.
///
/// A flat top-N cut would delete the chromatic and modal options wholesale in a
/// functional profile, which is exactly the diversity the brief asks for. This
/// takes the best of each strategy in turn instead.
fn stratified_trim(ctx: &EngineContext<'_>, out: &mut Vec<ChordOption>) {
    let cap = crate::params::SearchConfig::default().max_options_per_slot;
    if out.len() <= cap {
        sort_pool(out);
        return;
    }
    sort_pool(out);
    let mut buckets: Vec<(&'static str, Vec<ChordOption>)> =
        STRATEGIES.iter().map(|s| (*s, Vec::new())).collect();
    for option in out.drain(..) {
        if let Some(b) = buckets
            .iter_mut()
            .find(|(s, _)| *s == option.source_strategy)
        {
            b.1.push(option);
        }
    }
    let mut round = 0usize;
    while out.len() < cap {
        let mut took = false;
        for (_, bucket) in buckets.iter_mut() {
            if let Some(option) = bucket.get(round).cloned() {
                out.push(option);
                took = true;
                if out.len() >= cap {
                    break;
                }
            }
        }
        if !took {
            break;
        }
        round += 1;
    }
    let _ = ctx;
    sort_pool(out);
}

/// Deterministic pool ordering: best first, ties broken by symbol text.
fn sort_pool(out: &mut [ChordOption]) {
    out.sort_by(|a, b| {
        b.local_score
            .total()
            .partial_cmp(&a.local_score.total())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.key().cmp(&b.key()))
    });
}

/// True when the option's chord is a fully symmetric collection.
pub fn option_is_symmetric(option: &ChordOption) -> bool {
    is_symmetric(&option.spec)
}

/// Scores a chord that is already in the material as an option in its own right.
///
/// Reharmonisation must be able to keep what is there — that is what makes
/// "preserve the bass" or "preserve the cadence" satisfiable — so the existing
/// chord is admitted to the pool and scored on exactly the same terms as
/// everything the engine invented.
pub fn option_from_chord(
    ctx: &EngineContext<'_>,
    slot: &GridSlot,
    chord: &ChordEvent,
) -> Option<ChordOption> {
    let root = chord.spec.root_pc();
    let entry = ctx
        .kb
        .functions()
        .iter()
        .filter(|f| entry_applies(ctx, f))
        .find(|f| {
            f.scale_degree
                .as_deref()
                .and_then(Degree::parse)
                .map(|d| {
                    let (letter, accidental) = ctx.key.degree_root(d);
                    (letter.natural_pc() + i32::from(accidental.0)).rem_euclid(12) == root
                        && ctx
                            .kb
                            .chord_quality(&f.triad_quality)
                            .and_then(|q| TriadQuality::parse(&q.triad))
                            .map(|t| t == chord.spec.triad)
                            .unwrap_or(false)
                })
                .unwrap_or(false)
        })
        .or_else(|| {
            ctx.kb
                .functions()
                .iter()
                .find(|f| f.id == format!("{}_i", ctx.key.key_context_id))
        })?;
    let quality_id = ctx
        .kb
        .chord_qualities()
        .iter()
        .find(|q| {
            TriadQuality::parse(&q.triad) == Some(chord.spec.triad)
                && SeventhQuality::parse(&q.seventh) == Some(chord.spec.seventh)
                && q.degrees.len() == chord.spec.chord_tones().len()
        })
        .map(|q| q.id.clone())
        .unwrap_or_default();
    let function = chord
        .function
        .unwrap_or_else(|| function_class(&entry.function_class));
    Some(score_option(
        ctx,
        slot,
        chord.spec.clone(),
        function,
        entry,
        STRATEGY_EXISTING,
        &quality_id,
        chord.inversion,
        true,
    ))
}

/// Convenience for callers that want the whole grid's pools at once.
pub fn build_pools(ctx: &EngineContext<'_>) -> Result<Vec<Vec<ChordOption>>, HarmonyError> {
    let mut pools = Vec::with_capacity(ctx.analysis.grid.slots.len());
    for slot in &ctx.analysis.grid.slots {
        let pool = build_slot_options(ctx, slot);
        if pool.is_empty() {
            return Err(HarmonyError::no_valid_candidate(format!(
                "no chord survived hard-constraint filtering for the slot at {}",
                slot.start.to_display()
            )));
        }
        pools.push(pool);
    }
    if pools.is_empty() {
        return Err(HarmonyError::empty_analysis(
            "the harmonic grid has no slots to harmonise",
        ));
    }
    Ok(pools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    #[test]
    fn pools_are_non_empty_for_every_fixture_and_profile() {
        for fixture in testing::MELODY_FIXTURES {
            for profile in testing::PROFILE_IDS {
                let h = testing::harness(fixture, profile);
                let ctx = h.context();
                let pools =
                    build_pools(&ctx).unwrap_or_else(|e| panic!("{fixture}/{profile}: {e}"));
                assert_eq!(pools.len(), ctx.analysis.grid.slots.len());
                for pool in &pools {
                    assert!(!pool.is_empty());
                }
            }
        }
    }

    #[test]
    fn pool_is_deterministic() {
        let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
        let ctx = h.context();
        let a = build_slot_options(&ctx, &ctx.analysis.grid.slots[0]);
        let b = build_slot_options(&ctx, &ctx.analysis.grid.slots[0]);
        let ka: Vec<String> = a.iter().map(|o| o.key()).collect();
        let kb: Vec<String> = b.iter().map(|o| o.key()).collect();
        assert_eq!(ka, kb);
    }

    #[test]
    fn pool_carries_several_strategies() {
        let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
        let ctx = h.context();
        let pool = build_slot_options(&ctx, &ctx.analysis.grid.slots[0]);
        let mut strategies: Vec<&str> = pool.iter().map(|o| o.source_strategy).collect();
        strategies.sort_unstable();
        strategies.dedup();
        assert!(
            strategies.len() >= 4,
            "expected several generation sources, got {strategies:?}"
        );
    }

    #[test]
    fn every_option_cites_only_real_rule_ids() {
        let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
        let ctx = h.context();
        for slot in &ctx.analysis.grid.slots {
            for option in build_slot_options(&ctx, slot) {
                for app in &option.rule_applications {
                    assert!(
                        ctx.kb.rule(&app.rule_id).is_some(),
                        "option cites unknown rule {}",
                        app.rule_id
                    );
                    for source in &app.source_ids {
                        assert!(
                            ctx.kb.source(source).is_some(),
                            "option cites unknown source {source}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn hard_violations_never_reach_the_pool() {
        for profile in testing::PROFILE_IDS {
            let h = testing::harness("melodies/eight_bar_c_major", profile);
            let ctx = h.context();
            for slot in &ctx.analysis.grid.slots {
                for option in build_slot_options(&ctx, slot) {
                    assert!(
                        option
                            .rule_applications
                            .iter()
                            .all(|a| a.status != RuleStatus::Violated),
                        "a violated option survived in profile {profile}"
                    );
                }
            }
        }
    }

    #[test]
    fn score_vectors_keep_every_named_component() {
        let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
        let ctx = h.context();
        let pool = build_slot_options(&ctx, &ctx.analysis.grid.slots[0]);
        for option in &pool {
            for c in SCORE_COMPONENTS {
                assert!(
                    option.local_score.contains(c),
                    "option {} is missing component {c}",
                    option.symbol()
                );
            }
        }
    }

    #[test]
    fn alt_dominants_expand_to_explicit_alternatives() {
        let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
        let ctx = h.context();
        let mut seen: Vec<String> = Vec::new();
        for slot in &ctx.analysis.grid.slots {
            for option in build_slot_options(&ctx, slot) {
                if option.source_strategy == STRATEGY_ALT_DOMINANT {
                    assert!(
                        !option.spec.alt_dominant,
                        "an alt option must name its tensions rather than stay a family"
                    );
                    assert!(!option.spec.alterations.is_empty());
                    seen.push(option.symbol());
                }
            }
        }
        seen.sort_unstable();
        seen.dedup();
        assert!(
            seen.len() >= 2,
            "alt should reach more than one explicit realisation, saw {seen:?}"
        );
    }

    #[test]
    fn planing_options_are_constant_structure() {
        let h = testing::harness("melodies/dorian_vamp_d", "modal_ambient");
        let ctx = h.context();
        let pool = build_slot_options(&ctx, &ctx.analysis.grid.slots[0]);
        let planing: Vec<&ChordOption> = pool
            .iter()
            .filter(|o| o.source_strategy == STRATEGY_PLANING)
            .collect();
        assert!(!planing.is_empty(), "modal_ambient should reach planing");
        let mut qualities: Vec<&str> = planing.iter().map(|o| o.quality_id.as_str()).collect();
        qualities.sort_unstable();
        qualities.dedup();
        assert_eq!(qualities.len(), 1, "planing must keep one sonority");
    }

    #[test]
    fn strict_counterpoint_prefers_triads_over_extensions() {
        let strict = testing::harness("melodies/eight_bar_c_major", "strict_counterpoint");
        let jazz = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
        let strict_ctx = strict.context();
        let jazz_ctx = jazz.context();
        let strict_top = build_slot_options(&strict_ctx, &strict_ctx.analysis.grid.slots[0]);
        let jazz_top = build_slot_options(&jazz_ctx, &jazz_ctx.analysis.grid.slots[0]);
        let strict_tones: f64 = strict_top
            .iter()
            .take(4)
            .map(|o| o.spec.chord_tones().len() as f64)
            .sum();
        let jazz_tones: f64 = jazz_top
            .iter()
            .take(4)
            .map(|o| o.spec.chord_tones().len() as f64)
            .sum();
        assert!(
            strict_tones < jazz_tones,
            "strict counterpoint should reach for smaller sonorities: {strict_tones} vs {jazz_tones}"
        );
    }

    #[test]
    fn inversions_are_labelled_with_figures() {
        let h = testing::harness("melodies/eight_bar_c_major", "common_practice");
        let ctx = h.context();
        let pool = build_slot_options(&ctx, &ctx.analysis.grid.slots[0]);
        let inverted: Vec<&ChordOption> = pool.iter().filter(|o| o.inversion > 0).collect();
        for option in inverted {
            assert!(
                option.roman.ends_with('6')
                    || option.roman.contains("6/4")
                    || option.roman.contains("6/5")
                    || option.roman.contains("4/3")
                    || option.roman.contains("4/2"),
                "inverted option {} has no figure",
                option.roman
            );
        }
    }

    #[test]
    fn bass_pitch_class_follows_the_inversion() {
        let spec = symbol::parse("C").expect("C");
        let option = ChordOption {
            spec,
            function: HarmonicFunction::Tonic,
            roman: "I".into(),
            source_strategy: STRATEGY_DIATONIC,
            melody_degrees: Vec::new(),
            local_score: ScoreVector::new(),
            rule_applications: Vec::new(),
            chromaticism: 0.0,
            complexity: 0.0,
            entry_id: "major_i".into(),
            quality_id: "major_triad".into(),
            inversion: 1,
            source_ids: Vec::new(),
        };
        assert_eq!(option.bass_pc(), 4);
    }

    #[test]
    fn symmetric_chords_are_flagged() {
        let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
        let ctx = h.context();
        let mut found = false;
        for slot in &ctx.analysis.grid.slots {
            for option in build_slot_options(&ctx, slot) {
                if option.spec.seventh == SeventhQuality::Diminished {
                    assert!(option_is_symmetric(&option));
                    found = true;
                }
            }
        }
        // The fixture need not reach one, but if it does the flag must be right.
        let _ = found;
    }
}
