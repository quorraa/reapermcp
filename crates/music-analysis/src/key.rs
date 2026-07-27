//! Stage 3 — key and mode inference.
//!
//! Key detection here is a **ranked hypothesis set**, never a label. Every
//! collection in `theory-kb` is tried on every one of the twelve pitch classes
//! and scored against thirteen independent sources of evidence, each of which
//! is reported with its own value so a user can see *why* one answer beat
//! another. That matters most where naive detectors fail:
//!
//! * A D-Dorian vamp and a C-major tune share a pitch collection. Only the
//!   tonic-emphasis, phrase-edge and cadential evidence tell them apart, and a
//!   detector that scores pitch content alone will get it wrong every time.
//! * A blues head is not a major key with wrong notes and not a minor key with
//!   wrong notes; it is a blues collection, and forcing it into major/minor
//!   throws away the flat five that makes it what it is.
//!
//! So the candidate space is the whole scale catalogue, modal answers can and
//! do win, and the confidence gap between the leading candidates is reported so
//! callers can act on genuine ambiguity instead of a fabricated certainty.
//!
//! # Evidence
//!
//! | Name | What it measures |
//! |---|---|
//! | `pitch_distribution` | duration-weighted share of the material inside the collection, corrected for collection size |
//! | `collection_parsimony` | how much of the collection the material actually uses |
//! | `tonic_emphasis` | duration on the tonic, relative to the most-used pitch class |
//! | `metric_placement` | the same, weighted by where in the bar the notes fall |
//! | `long_notes` | share of the longest notes that are tonic-triad members |
//! | `phrase_edges` | tonic/dominant/third at phrase beginnings and endings |
//! | `bass_emphasis` | tonic weight in the lowest line (polyphonic material only) |
//! | `cadential_motion` | melodic approach to phrase endings |
//! | `leading_tone` | presence of the leading tone and whether it resolves |
//! | `tonic_dominant_repetition` | repeated tonic and dominant onsets |
//! | `chord_evidence` | verticalities explained as diatonic chords (polyphonic only) |
//! | `modal_characteristics` | characteristic degrees present, absent or contradicted |
//! | `pedal_tone` | a sustained pedal on the tonic or dominant |

use crate::util::{class_pc, class_text, cmp_f64, num, round6};
use music_domain::prelude::*;
use qjson::{Json, JsonMap};
use theory_kb::KnowledgeBase;

/// Every evidence source, in reporting order.
pub const KEY_EVIDENCE: &[&str] = &[
    "pitch_distribution",
    "collection_parsimony",
    "tonic_emphasis",
    "metric_placement",
    "long_notes",
    "phrase_edges",
    "bass_emphasis",
    "cadential_motion",
    "leading_tone",
    "tonic_dominant_repetition",
    "chord_evidence",
    "modal_characteristics",
    "pedal_tone",
];

/// The weight of each evidence source in the total score.
const WEIGHTS: [f64; 13] = [
    0.90, // pitch_distribution
    0.50, // collection_parsimony
    1.20, // tonic_emphasis
    0.60, // metric_placement
    0.60, // long_notes
    1.70, // phrase_edges
    0.70, // bass_emphasis
    1.20, // cadential_motion
    0.50, // leading_tone
    0.60, // tonic_dominant_repetition
    0.70, // chord_evidence
    0.80, // modal_characteristics
    0.40, // pedal_tone
];

/// How many candidates are reported by default.
pub const DEFAULT_MAX_CANDIDATES: usize = 8;

/// Relative score gap below which the leading candidates are called ambiguous.
const AMBIGUITY_GAP: f64 = 0.05;

/// The default local-analysis window, in bars.
const DEFAULT_WINDOW_BARS: i64 = 4;

/// How far behind a window's best answer the incumbent key may fall before a
/// new region is opened.
const REGION_HYSTERESIS: f64 = 0.15;

/// How many notes a window needs before it is allowed an opinion.
const MIN_WINDOW_NOTES: usize = 4;

/// Scales that are never offered as a key: they are analytical devices rather
/// than tonal centres, and the chromatic collection would trivially contain any
/// material at all.
const NON_KEY_SCALES: &[&str] = &["chromatic"];

/// One ranked key hypothesis.
#[derive(Clone, Debug)]
pub struct KeyCandidate {
    /// Spelled tonic.
    pub tonic: (Letter, Accidental),
    /// Scale identifier from `knowledge/scales.json`.
    pub scale_id: String,
    /// Weighted total of the evidence.
    pub score: f64,
    /// Confidence relative to the rest of the field, `0.0..=1.0`.
    pub confidence: f64,
    /// Every evidence source and the raw value it produced.
    pub evidence: Vec<(&'static str, f64)>,
    /// True for any collection other than major, natural minor, harmonic minor
    /// and melodic minor — that is, for every answer a major/minor-only
    /// detector could not have given.
    pub is_modal: bool,
}

impl KeyCandidate {
    /// `"D dorian"`.
    pub fn label(&self) -> String {
        format!("{} {}", class_text(self.tonic), self.scale_id)
    }

    /// Sounding pitch class of the tonic.
    pub fn tonic_pc(&self) -> i32 {
        class_pc(self.tonic)
    }

    /// One evidence value by name, `0.0` when unknown.
    pub fn evidence_value(&self, name: &str) -> f64 {
        self.evidence
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
            .unwrap_or(0.0)
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("tonic", Json::Str(class_text(self.tonic)));
        m.insert("scale_id", Json::Str(self.scale_id.clone()));
        m.insert("label", Json::Str(self.label()));
        m.insert("score", num(self.score));
        m.insert("confidence", num(self.confidence));
        m.insert("is_modal", Json::Bool(self.is_modal));
        let mut e = JsonMap::new();
        for (k, v) in &self.evidence {
            e.insert(*k, num(*v));
        }
        m.insert("evidence", Json::Obj(e));
        Json::Obj(m)
    }
}

/// The result of stage 3.
#[derive(Clone, Debug, Default)]
pub struct KeyAnalysis {
    /// Ranked hypotheses, best first.
    pub candidates: Vec<KeyCandidate>,
    /// Local key regions, including tonicizations.
    pub regions: Vec<KeyRegion>,
    /// Score gap between the two leading candidates.
    pub gap: f64,
    /// True when the leading candidates are too close to choose between.
    pub ambiguous: bool,
}

impl KeyAnalysis {
    /// The leading candidate.
    pub fn top(&self) -> Option<&KeyCandidate> {
        self.candidates.first()
    }

    /// Rank of a `(tonic pitch class, scale id)` pair, if it is in the field.
    pub fn rank_of(&self, tonic_pc: i32, scale_id: &str) -> Option<usize> {
        self.candidates
            .iter()
            .position(|c| c.tonic_pc() == tonic_pc && c.scale_id == scale_id)
    }

    /// Sounding pitch classes of the leading candidate's collection.
    pub fn top_pcs(&self, kb: &KnowledgeBase) -> Vec<i32> {
        match self.top() {
            Some(c) => match kb.scale(&c.scale_id) {
                Some(def) => def
                    .semitones
                    .iter()
                    .map(|s| (c.tonic_pc() + s).rem_euclid(12))
                    .collect(),
                None => Vec::new(),
            },
            None => Vec::new(),
        }
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert(
            "candidates",
            Json::Arr(self.candidates.iter().map(KeyCandidate::to_json).collect()),
        );
        m.insert(
            "regions",
            Json::Arr(self.regions.iter().map(KeyRegion::to_json).collect()),
        );
        m.insert("gap", num(self.gap));
        m.insert("ambiguous", Json::Bool(self.ambiguous));
        Json::Obj(m)
    }
}

/// Runs stage 3.
///
/// `hint` is a `(tonic, scale id)` pair such as `("C", "major")`. A hint
/// **overrides** the ranking — it is placed first — but it is still scored
/// against the same evidence and reported with its real numbers, so a caller
/// can see when their declared key disagrees with the material.
///
/// `window_qn` sets the local-analysis window; the default is four bars.
pub fn analyze_key(
    kb: &KnowledgeBase,
    notes: &NoteSet,
    tm: &TimeMap,
    hint: Option<(&str, &str)>,
    window_qn: Option<BeatTime>,
) -> KeyAnalysis {
    analyze_key_ranked(kb, notes, tm, hint, window_qn, DEFAULT_MAX_CANDIDATES)
}

/// [`analyze_key`] with an explicit candidate limit, which the strictness
/// setting varies.
pub fn analyze_key_ranked(
    kb: &KnowledgeBase,
    notes: &NoteSet,
    tm: &TimeMap,
    hint: Option<(&str, &str)>,
    window_qn: Option<BeatTime>,
    max_candidates: usize,
) -> KeyAnalysis {
    let material = Material::of(notes, tm);
    let mut ranked = score_all(kb, &material);

    let mut gap = 0.0;
    let mut ambiguous = true;
    if ranked.len() >= 2 {
        gap = ranked[0].score - ranked[1].score;
        let rel = gap / ranked[0].score.abs().max(1e-6);
        ambiguous = rel < AMBIGUITY_GAP;
    } else if ranked.len() == 1 {
        ambiguous = false;
    }
    assign_confidence(&mut ranked);
    ranked.truncate(max_candidates.max(1));

    // A user hint outranks inference, but is scored like everything else.
    if let Some((tonic, scale_id)) = hint {
        if let Some(mut c) = hinted(kb, &material, tonic, scale_id) {
            c.evidence.push(("user_hint", 1.0));
            c.confidence = c.confidence.max(0.9);
            ranked.retain(|k| !(k.tonic == c.tonic && k.scale_id == c.scale_id));
            ranked.insert(0, c);
            ranked.truncate(max_candidates.max(1));
        }
    }

    let regions = local_regions(kb, notes, tm, window_qn, ranked.first());

    KeyAnalysis {
        candidates: ranked,
        regions,
        gap: round6(gap),
        ambiguous,
    }
}

/// Scores every `(pitch class, scale)` pair and sorts the field.
fn score_all(kb: &KnowledgeBase, material: &Material) -> Vec<KeyCandidate> {
    // `(catalogue index, candidate, alias rank)`; alias rank 0 marks the record
    // that owns a collection rather than naming a mode of another one.
    let mut out: Vec<(usize, KeyCandidate, u8)> = Vec::new();
    if material.total <= 0.0 {
        return Vec::new();
    }
    for (index, def) in kb.scales().iter().enumerate() {
        if NON_KEY_SCALES.contains(&def.id.as_str()) || def.semitones.is_empty() {
            continue;
        }
        for pc in 0..12 {
            let Some(tonic) = spell_tonic(def, pc) else {
                continue;
            };
            let alias_rank = u8::from(def.mode_of.is_some());
            out.push((index, evaluate(def, tonic, pc, material), alias_rank));
        }
    }
    // Deterministic total order: score, then the preference between two
    // records of the same collection, then catalogue order, then tonic.
    out.sort_by(|a, b| {
        cmp_f64(b.1.score, a.1.score)
            .then(a.2.cmp(&b.2))
            .then(a.0.cmp(&b.0))
            .then(a.1.tonic_pc().cmp(&b.1.tonic_pc()))
            .then(class_text(a.1.tonic).cmp(&class_text(b.1.tonic)))
    });

    // `major` and `ionian` are the same seven notes; so are `natural_minor`
    // and `aeolian`. Reporting both as separate hypotheses is noise that also
    // fabricates a zero gap between the two leading candidates, so only the
    // first record of each collection survives.
    let mut seen: Vec<(i32, Vec<i32>)> = Vec::new();
    let mut ranked = Vec::new();
    for (_, c, _) in out {
        let mut key = collection_of(kb, &c);
        key.1.sort_unstable();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        ranked.push(c);
    }
    ranked
}

/// The `(tonic pitch class, sounding pitch classes)` a candidate denotes.
fn collection_of(kb: &KnowledgeBase, c: &KeyCandidate) -> (i32, Vec<i32>) {
    let pcs = match kb.scale(&c.scale_id) {
        Some(def) => def
            .semitones
            .iter()
            .map(|s| (c.tonic_pc() + s).rem_euclid(12))
            .collect(),
        None => Vec::new(),
    };
    (c.tonic_pc(), pcs)
}

/// Builds the candidate a hint names, or `None` when it does not resolve.
fn hinted(
    kb: &KnowledgeBase,
    material: &Material,
    tonic: &str,
    scale_id: &str,
) -> Option<KeyCandidate> {
    let spelled = SpelledPitch::parse_class(tonic)?;
    let def = kb.scale(scale_id)?;
    Some(evaluate(def, spelled, class_pc(spelled), material))
}

/// Turns raw scores into confidences.
///
/// The leading candidate's confidence rises with the *relative* gap to the
/// runner-up, so a clear winner reads as confident and a photo finish does not.
fn assign_confidence(ranked: &mut [KeyCandidate]) {
    let Some(best) = ranked.first().map(|c| c.score) else {
        return;
    };
    if best <= 0.0 {
        for c in ranked.iter_mut() {
            c.confidence = 0.0;
        }
        return;
    }
    let second = ranked.get(1).map(|c| c.score).unwrap_or(0.0);
    let rel = ((best - second) / best.abs().max(1e-6)).clamp(0.0, 1.0);
    let top = (0.5 + (rel * 5.0).min(1.0) * 0.45).clamp(0.0, 0.99);
    for c in ranked.iter_mut() {
        c.confidence = round6((top * (c.score / best).clamp(0.0, 1.0)).clamp(0.0, 1.0));
    }
}

/// Scores one hypothesis.
fn evaluate(
    def: &ScaleDef,
    tonic: (Letter, Accidental),
    tonic_pc: i32,
    m: &Material,
) -> KeyCandidate {
    let pcs: Vec<i32> = def
        .semitones
        .iter()
        .map(|s| (tonic_pc + s).rem_euclid(12))
        .collect();
    let size = pcs.len() as f64;
    let dominant_pc = degree_pc(def, &pcs, 4).unwrap_or((tonic_pc + 7).rem_euclid(12));
    let third_pc = degree_pc(def, &pcs, 2).unwrap_or((tonic_pc + 4).rem_euclid(12));

    // 1: duration-weighted pitch distribution, corrected for collection size.
    let inside: f64 = pcs.iter().map(|p| m.pc_duration[*p as usize]).sum();
    let expected = size / 12.0;
    let pitch_distribution = ((inside / m.total) - expected) / (1.0 - expected).max(1e-6);

    // 2: how much of the collection is actually used.
    let used = pcs
        .iter()
        .filter(|p| m.pc_duration[**p as usize] > 0.0)
        .count() as f64;
    let collection_parsimony = used / size;

    // 3: tonic emphasis.
    let tonic_emphasis = if m.max_pc_share > 0.0 {
        (m.pc_duration[tonic_pc as usize] / m.total) / m.max_pc_share
    } else {
        0.0
    };

    // 4: the same evidence, weighted by metric position.
    let metric_inside: f64 = pcs.iter().map(|p| m.pc_metric[*p as usize]).sum();
    let metric_share = if m.metric_total > 0.0 {
        ((metric_inside / m.metric_total) - expected) / (1.0 - expected).max(1e-6)
    } else {
        0.0
    };
    let tonic_metric = if m.metric_total > 0.0 && m.max_metric_share > 0.0 {
        (m.pc_metric[tonic_pc as usize] / m.metric_total) / m.max_metric_share
    } else {
        0.0
    };
    let metric_placement = 0.6 * metric_share + 0.4 * tonic_metric;

    // 5: what the longest notes land on.
    let triad = [tonic_pc, third_pc, dominant_pc];
    let long_hit: f64 = triad.iter().map(|p| m.long_duration[*p as usize]).sum();
    let long_notes = if m.long_total > 0.0 {
        ((long_hit / m.long_total) - 0.25) / 0.75
    } else {
        0.0
    };

    // 6: phrase beginnings and endings.
    let phrase_edges = weighted_credit(&m.edges, |pc| {
        if pc == tonic_pc {
            1.0
        } else if pc == dominant_pc {
            0.55
        } else if pc == third_pc {
            0.35
        } else if pcs.contains(&pc) {
            0.05
        } else {
            -0.25
        }
    });

    // 7: the bass line, when there is one.
    let bass_emphasis = if m.bass_total > 0.0 && m.max_bass_share > 0.0 {
        (m.bass_duration[tonic_pc as usize] / m.bass_total) / m.max_bass_share
    } else {
        0.0
    };

    // 8: how phrase endings are approached.
    let cadential_motion = weighted_pair_credit(&m.cadence_moves, |from, to| {
        let to_deg = (to - tonic_pc).rem_euclid(12);
        let step = (to - from).rem_euclid(12);
        match (to_deg, step) {
            (0, 1) => 1.0,   // leading tone rises to the tonic
            (0, 11) => 0.85, // supertonic falls to the tonic
            (0, 10) => 0.8,  // flat seventh falls to the tonic: modal close
            (0, 5) | (0, 7) => 0.75,
            (0, _) => 0.6,
            (7, _) => 0.5, // arrival on the dominant: a half close
            (4, _) | (3, _) => 0.35,
            _ => 0.0,
        }
    });

    // 9: the leading tone, where the collection has one.
    let lt_pc = (tonic_pc + 11).rem_euclid(12);
    let leading_tone = if pcs.contains(&lt_pc) {
        let presence = if m.max_pc_share > 0.0 {
            ((m.pc_duration[lt_pc as usize] / m.total) / m.max_pc_share).min(1.0)
        } else {
            0.0
        };
        // Presence alone is weak evidence: an E is in F major and in C major
        // alike. What distinguishes a leading tone is that it *leads*.
        0.3 * presence + 0.7 * m.resolution_rate(lt_pc, tonic_pc)
    } else {
        0.0
    };

    // 10: repeated tonic and dominant onsets.
    let td = (m.pc_onsets[tonic_pc as usize] + m.pc_onsets[dominant_pc as usize]) as f64;
    let tonic_dominant_repetition = if m.onset_count > 0 {
        ((td / m.onset_count as f64) - (2.0 / 12.0)) / (1.0 - 2.0 / 12.0)
    } else {
        0.0
    };

    // 11: vertical evidence, when the material is polyphonic.
    let chord_evidence = if m.sonority_total > 0.0 {
        let mut acc = 0.0;
        for (set, dur) in &m.sonorities {
            acc += dur * sonority_credit(set, &pcs);
        }
        acc / m.sonority_total
    } else {
        0.0
    };

    // 12: the degrees that make this collection what it is.
    let modal_characteristics = characteristic_credit(def, tonic_pc, &pcs, m);

    // 13: a pedal on the tonic or dominant.
    let pedal_tone = match m.pedal_pc {
        Some(p) if p == tonic_pc => 1.0,
        Some(p) if p == dominant_pc => 0.6,
        Some(_) => -0.2,
        None => 0.0,
    };

    let raw = [
        pitch_distribution,
        collection_parsimony,
        tonic_emphasis,
        metric_placement,
        long_notes,
        phrase_edges,
        bass_emphasis,
        cadential_motion,
        leading_tone,
        tonic_dominant_repetition,
        chord_evidence,
        modal_characteristics,
        pedal_tone,
    ];
    let score: f64 = raw
        .iter()
        .zip(WEIGHTS.iter())
        .map(|(r, w)| r.clamp(-1.5, 1.5) * w)
        .sum();

    KeyCandidate {
        tonic,
        scale_id: def.id.clone(),
        score: round6(score),
        confidence: 0.0,
        evidence: KEY_EVIDENCE
            .iter()
            .copied()
            .zip(raw.iter().map(|v| round6(v.clamp(-1.5, 1.5))))
            .collect(),
        is_modal: is_modal(&def.id),
    }
}

/// True for every collection a major/minor-only detector could not name.
fn is_modal(scale_id: &str) -> bool {
    !matches!(
        scale_id,
        "major" | "natural_minor" | "harmonic_minor" | "melodic_minor"
    )
}

/// Credit for the collection's characteristic degrees.
///
/// A characteristic degree that is **present** is positive evidence in
/// proportion to how much of the material sits on it. One that is
/// **contradicted** — absent, while a chromatic neighbour outside the
/// collection is present — is strong negative evidence: that is exactly the
/// case where the material is telling us it is a different mode. One that is
/// merely **unused** is weak negative evidence, because a short melody may
/// simply never reach it.
fn characteristic_credit(def: &ScaleDef, tonic_pc: i32, pcs: &[i32], m: &Material) -> f64 {
    if def.characteristic_degrees.is_empty() {
        return 0.0;
    }
    let mut acc = 0.0;
    let mut n = 0.0;
    for degree in &def.characteristic_degrees {
        let Some(semitones) = degree_semitones(def, degree) else {
            continue;
        };
        let pc = (tonic_pc + semitones).rem_euclid(12);
        n += 1.0;
        let share = m.pc_duration[pc as usize] / m.total;
        if share > 0.0 {
            acc += if m.max_pc_share > 0.0 {
                (share / m.max_pc_share).min(1.0)
            } else {
                0.0
            };
            continue;
        }
        let contradicted = [-1i32, 1]
            .iter()
            .map(|d| (pc + d).rem_euclid(12))
            .any(|alt| m.pc_duration[alt as usize] > 0.0 && !pcs.contains(&alt));
        acc += if contradicted { -1.0 } else { -0.25 };
    }
    if n == 0.0 {
        0.0
    } else {
        acc / n
    }
}

/// How well a simultaneity is explained by a collection.
fn sonority_credit(set: &[i32], pcs: &[i32]) -> f64 {
    if set.is_empty() {
        return 0.0;
    }
    let inside = set.iter().filter(|p| pcs.contains(p)).count();
    if inside < set.len() {
        return inside as f64 / set.len() as f64 - 0.5;
    }
    // Fully inside; a stack of thirds inside the collection is stronger
    // evidence than an arbitrary subset of it.
    if set.len() >= 3 && is_tertian(set, pcs) {
        1.0
    } else {
        0.6
    }
}

/// True when the pitch classes stack in thirds within the collection.
fn is_tertian(set: &[i32], pcs: &[i32]) -> bool {
    for root in set {
        let mut ok = true;
        for p in set {
            let iv = (p - root).rem_euclid(12);
            if !matches!(iv, 0 | 3 | 4 | 6 | 7 | 8 | 10 | 11) {
                ok = false;
                break;
            }
        }
        if ok && pcs.contains(root) {
            return true;
        }
    }
    false
}

/// Weighted average of a credit function over `(pitch class, weight)` pairs.
fn weighted_credit(items: &[(i32, f64)], credit: impl Fn(i32) -> f64) -> f64 {
    let total: f64 = items.iter().map(|(_, w)| *w).sum();
    if total <= 0.0 {
        return 0.0;
    }
    items.iter().map(|(pc, w)| w * credit(*pc)).sum::<f64>() / total
}

/// Weighted average over `(from, to, weight)` triples.
fn weighted_pair_credit(items: &[(i32, i32, f64)], credit: impl Fn(i32, i32) -> f64) -> f64 {
    let total: f64 = items.iter().map(|(_, _, w)| *w).sum();
    if total <= 0.0 {
        return 0.0;
    }
    items
        .iter()
        .map(|(a, b, w)| w * credit(*a, *b))
        .sum::<f64>()
        / total
}

/// The pitch class of the `index`-th scale degree, when the collection has one.
fn degree_pc(def: &ScaleDef, pcs: &[i32], index: usize) -> Option<i32> {
    let _ = def;
    pcs.get(index).copied()
}

/// Semitones above the tonic for a degree string such as `"b6"`.
///
/// The collection's own `degree_spelling` is authoritative; the generic degree
/// parser is only a fallback for a collection that does not spell that degree.
fn degree_semitones(def: &ScaleDef, degree: &str) -> Option<i32> {
    if let Some(i) = def.degree_spelling.iter().position(|d| d == degree) {
        return def.semitones.get(i).copied();
    }
    ChordDegree::parse(degree).map(|d| d.simple_semitones())
}

/// Picks the spelling of a tonic that spells the collection most simply.
///
/// `F#` and `Gb` sound alike; which one is right depends on how many
/// accidentals the resulting scale needs. Ties prefer the flat, matching
/// conventional practice for the ambiguous keys.
fn spell_tonic(def: &ScaleDef, pc: i32) -> Option<(Letter, Accidental)> {
    let mut best: Option<((Letter, Accidental), (i32, i32))> = None;
    for cand in spelling_options(pc) {
        let inst = ScaleInstance::new(def.clone(), cand);
        let cost: i32 = inst
            .spelled_degrees()
            .iter()
            .map(|(_, a)| (a.0 as i32).abs())
            .sum();
        let prefer = i32::from(cand.1 .0 > 0); // sharps lose ties
        if best.is_none_or(|(_, b)| (cost, prefer) < b) {
            best = Some((cand, (cost, prefer)));
        }
    }
    best.map(|(c, _)| c)
}

/// The plausible spellings of a pitch class: the natural letter when there is
/// one, otherwise the sharp below and the flat above.
fn spelling_options(pc: i32) -> Vec<(Letter, Accidental)> {
    const NATURALS: [(i32, Letter); 7] = [
        (0, Letter::C),
        (2, Letter::D),
        (4, Letter::E),
        (5, Letter::F),
        (7, Letter::G),
        (9, Letter::A),
        (11, Letter::B),
    ];
    let pc = pc.rem_euclid(12);
    if let Some((_, l)) = NATURALS.iter().find(|(n, _)| *n == pc) {
        return vec![(*l, Accidental::NATURAL)];
    }
    let mut out = Vec::new();
    if let Some((_, l)) = NATURALS.iter().find(|(n, _)| (*n + 1).rem_euclid(12) == pc) {
        out.push((*l, Accidental::SHARP));
    }
    if let Some((_, l)) = NATURALS.iter().find(|(n, _)| (*n - 1).rem_euclid(12) == pc) {
        out.push((*l, Accidental::FLAT));
    }
    out
}

// ---------------------------------------------------------------------------
// local regions
// ---------------------------------------------------------------------------

/// Runs the same scorer over sliding windows and merges the answers.
///
/// Two rules keep this from turning every passing chromaticism into a key
/// change. First, a window only opens a new region when the incumbent key has
/// fallen more than [`REGION_HYSTERESIS`] behind the window's best answer —
/// a modulation has to be *confirmed*, not merely suggested. Second, a region
/// shorter than two bars that disagrees with the global answer is reported as
/// a **tonicization** rather than as a modulation, because a two-bar excursion
/// is a colour and calling every applied dominant a new key is the classic
/// false positive here.
fn local_regions(
    kb: &KnowledgeBase,
    notes: &NoteSet,
    tm: &TimeMap,
    window_qn: Option<BeatTime>,
    global: Option<&KeyCandidate>,
) -> Vec<KeyRegion> {
    let Some(global) = global else {
        return Vec::new();
    };
    if notes.notes.is_empty() {
        return Vec::new();
    }
    let (start, end) = notes.span();
    let bar = tm.meter_at(start).bar_length_qn();
    let window = window_qn.unwrap_or(bar * DEFAULT_WINDOW_BARS);
    if !window.is_positive() || (end - start) <= window {
        return vec![region_of(global, start, end, false)];
    }
    let step = if window.scale(1, 2).is_positive() {
        window.scale(1, 2)
    } else {
        window
    };

    let mut out: Vec<KeyRegion> = Vec::new();
    let mut at = start;
    let mut guard = 0;
    while at < end && guard < 4096 {
        guard += 1;
        let stop = (at + window).min(end);
        let slice: Vec<Note> = notes
            .notes
            .iter()
            .filter(|n| n.onset >= at && n.onset < stop)
            .cloned()
            .collect();
        if slice.len() < MIN_WINDOW_NOTES {
            at = at + step;
            continue;
        }
        let sub = NoteSet::sorted(slice, notes.time_map.clone());
        let material = Material::of(&sub, tm);
        let mut ranked = score_all(kb, &material);
        assign_confidence(&mut ranked);
        let Some(best) = ranked.first().cloned() else {
            at = at + step;
            continue;
        };

        // Does the incumbent still hold up in this window?
        let incumbent_holds = out.last().is_some_and(|r| {
            ranked
                .iter()
                .find(|c| c.tonic == r.tonic && c.scale_id == r.scale_id)
                .is_some_and(|c| c.score >= best.score * (1.0 - REGION_HYSTERESIS))
        });
        match out.last_mut() {
            Some(last) if incumbent_holds => last.end = last.end.max(stop),
            Some(last) => {
                last.end = last.end.min(at).max(last.start);
                let same_as_global = best.tonic == global.tonic && best.scale_id == global.scale_id;
                out.push(region_of(&best, at, stop, !same_as_global));
            }
            None => {
                let same_as_global = best.tonic == global.tonic && best.scale_id == global.scale_id;
                out.push(region_of(&best, at, stop, !same_as_global));
            }
        }
        at = at + step;
    }

    if let Some(last) = out.last_mut() {
        last.end = last.end.max(end);
    }
    out.retain(|r| r.end > r.start);
    // A differing region only counts as a modulation once it has survived a
    // whole confirmation window; anything shorter is a tonicization.
    let _ = bar;
    for r in out.iter_mut() {
        if r.is_tonicization && (r.end - r.start) >= window {
            r.is_tonicization = false;
        }
    }
    out
}

/// Builds a region record from a candidate.
fn region_of(c: &KeyCandidate, start: BeatTime, end: BeatTime, tonicization: bool) -> KeyRegion {
    let mut evidence: Vec<(&'static str, f64)> = c.evidence.clone();
    evidence.sort_by(|a, b| cmp_f64(b.1, a.1).then(a.0.cmp(b.0)));
    KeyRegion {
        start,
        end,
        tonic: c.tonic,
        scale_id: c.scale_id.clone(),
        confidence: c.confidence,
        evidence: evidence
            .iter()
            .take(3)
            .map(|(n, v)| format!("{n}={:.3}", v))
            .collect(),
        is_tonicization: tonicization,
    }
}

// ---------------------------------------------------------------------------
// precomputed material
// ---------------------------------------------------------------------------

/// Everything the scorer needs from the notes, computed once.
struct Material {
    total: f64,
    pc_duration: [f64; 12],
    max_pc_share: f64,
    pc_metric: [f64; 12],
    metric_total: f64,
    max_metric_share: f64,
    long_duration: [f64; 12],
    long_total: f64,
    edges: Vec<(i32, f64)>,
    cadence_moves: Vec<(i32, i32, f64)>,
    bass_duration: [f64; 12],
    bass_total: f64,
    max_bass_share: f64,
    pc_onsets: [usize; 12],
    onset_count: usize,
    sonorities: Vec<(Vec<i32>, f64)>,
    sonority_total: f64,
    pedal_pc: Option<i32>,
    successions: Vec<(i32, i32)>,
}

impl Material {
    fn of(notes: &NoteSet, tm: &TimeMap) -> Material {
        let mut m = Material {
            total: 0.0,
            pc_duration: [0.0; 12],
            max_pc_share: 0.0,
            pc_metric: [0.0; 12],
            metric_total: 0.0,
            max_metric_share: 0.0,
            long_duration: [0.0; 12],
            long_total: 0.0,
            edges: Vec::new(),
            cadence_moves: Vec::new(),
            bass_duration: [0.0; 12],
            bass_total: 0.0,
            max_bass_share: 0.0,
            pc_onsets: [0; 12],
            onset_count: 0,
            sonorities: Vec::new(),
            sonority_total: 0.0,
            pedal_pc: None,
            successions: Vec::new(),
        };
        let audible: Vec<&Note> = notes.notes.iter().filter(|n| !n.muted).collect();
        if audible.is_empty() {
            return m;
        }

        for n in &audible {
            let pc = n.pitch_class() as usize;
            let d = n.duration.as_f64();
            m.pc_duration[pc] += d;
            m.total += d;
            let w = tm.metric_weight(n.onset);
            m.pc_metric[pc] += d * w;
            m.metric_total += d * w;
            m.pc_onsets[pc] += 1;
            m.onset_count += 1;
        }
        if m.total > 0.0 {
            m.max_pc_share = m
                .pc_duration
                .iter()
                .map(|d| d / m.total)
                .fold(0.0f64, f64::max);
        }
        if m.metric_total > 0.0 {
            m.max_metric_share = m
                .pc_metric
                .iter()
                .map(|d| d / m.metric_total)
                .fold(0.0f64, f64::max);
        }

        // Long notes: everything at or above the 75th-percentile duration.
        let mut durations: Vec<f64> = audible.iter().map(|n| n.duration.as_f64()).collect();
        durations.sort_by(|a, b| cmp_f64(*a, *b));
        let cut_index = ((durations.len() * 3) / 4).min(durations.len() - 1);
        let cut = durations[cut_index];
        for n in &audible {
            let d = n.duration.as_f64();
            if d >= cut {
                m.long_duration[n.pitch_class() as usize] += d;
                m.long_total += d;
            }
        }

        // The skyline is what carries the tune; edges and cadences are read
        // from it so an accompaniment cannot drown the melodic evidence.
        let line: Vec<&Note> = if notes.max_polyphony() > 1 {
            notes.highest_line()
        } else {
            audible.clone()
        };
        let beat = tm.meter_at(audible[0].onset).beat_unit_qn();
        for (i, n) in line.iter().enumerate() {
            let opens = i == 0
                || line
                    .get(i.wrapping_sub(1))
                    .is_some_and(|p| (n.onset - p.end()) >= beat.scale(1, 2));
            let closes = i + 1 == line.len()
                || line
                    .get(i + 1)
                    .is_some_and(|q| (q.onset - n.end()) >= beat.scale(1, 2));
            let mut weight = 0.0;
            if opens {
                weight += 1.0;
            }
            if closes {
                weight += 1.5;
            }
            if i == 0 {
                weight += 0.5;
            }
            if i + 1 == line.len() {
                weight += 1.5;
            }
            if weight > 0.0 {
                m.edges.push((n.pitch_class(), weight));
            }
            if closes && i > 0 {
                let from = line[i - 1].pitch_class();
                let w = if i + 1 == line.len() { 2.0 } else { 1.0 };
                m.cadence_moves.push((from, n.pitch_class(), w));
            }
        }
        for w in line.windows(2) {
            m.successions.push((w[0].pitch_class(), w[1].pitch_class()));
        }

        // Bass and vertical evidence only exist in polyphonic material.
        if notes.max_polyphony() > 1 {
            for n in notes.lowest_line() {
                if n.muted {
                    continue;
                }
                m.bass_duration[n.pitch_class() as usize] += n.duration.as_f64();
                m.bass_total += n.duration.as_f64();
            }
            if m.bass_total > 0.0 {
                m.max_bass_share = m
                    .bass_duration
                    .iter()
                    .map(|d| d / m.bass_total)
                    .fold(0.0f64, f64::max);
            }
            m.sonorities = verticals(notes);
            m.sonority_total = m.sonorities.iter().map(|(_, d)| *d).sum();
            m.pedal_pc = pedal(notes);
        }
        m
    }

    /// Fraction of `from`-to-`to` successions among all successions leaving
    /// `from`; `0.0` when `from` never occurs.
    fn resolution_rate(&self, from: i32, to: i32) -> f64 {
        let leaving = self.successions.iter().filter(|(a, _)| *a == from).count();
        if leaving == 0 {
            return 0.0;
        }
        let resolving = self
            .successions
            .iter()
            .filter(|(a, b)| *a == from && *b == to)
            .count();
        resolving as f64 / leaving as f64
    }
}

/// The distinct simultaneities of a note set, with the time each one sounds.
fn verticals(notes: &NoteSet) -> Vec<(Vec<i32>, f64)> {
    let mut points: Vec<BeatTime> = Vec::new();
    for n in &notes.notes {
        if n.muted {
            continue;
        }
        points.push(n.onset);
        points.push(n.end());
    }
    points.sort();
    points.dedup();
    let mut out = Vec::new();
    for w in points.windows(2) {
        let mid = w[0] + (w[1] - w[0]).scale(1, 2);
        let mut set: Vec<i32> = notes
            .sounding_at(mid)
            .iter()
            .filter(|n| !n.muted)
            .map(|n| n.pitch_class())
            .collect();
        set.sort_unstable();
        set.dedup();
        if set.len() >= 2 {
            out.push((set, (w[1] - w[0]).as_f64()));
        }
    }
    out
}

/// The pitch class of a pedal, when one covers at least half the span in the
/// bottom of the texture.
fn pedal(notes: &NoteSet) -> Option<i32> {
    let (start, end) = notes.span();
    let span = (end - start).as_f64();
    if span <= 0.0 {
        return None;
    }
    let mut covered = [0.0f64; 12];
    for n in notes.lowest_line() {
        if n.muted {
            continue;
        }
        covered[n.pitch_class() as usize] += n.duration.as_f64();
    }
    let mut best: Option<(i32, f64)> = None;
    for (pc, c) in covered.iter().enumerate() {
        if best.is_none_or(|(_, b)| *c > b) {
            best = Some((pc as i32, *c));
        }
    }
    best.filter(|(_, c)| c / span >= 0.5).map(|(pc, _)| pc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::note_set;
    use theory_kb::KnowledgeBase;

    fn kb() -> &'static KnowledgeBase {
        KnowledgeBase::embedded()
    }

    fn analyse(ns: &NoteSet) -> KeyAnalysis {
        analyze_key(kb(), ns, &ns.time_map.clone(), None, None)
    }

    fn dorian_vamp() -> NoteSet {
        note_set(&[
            ("D4", "0", "2"),
            ("F4", "2", "1"),
            ("G4", "3", "1"),
            ("A4", "4", "2"),
            ("B4", "6", "2"),
            ("A4", "8", "1"),
            ("G4", "9", "1"),
            ("F4", "10", "2"),
            ("E4", "12", "2"),
            ("D4", "14", "2"),
            ("A4", "16", "2"),
            ("C5", "18", "2"),
            ("B4", "20", "1"),
            ("A4", "21", "1"),
            ("G4", "22", "2"),
            ("F4", "24", "2"),
            ("E4", "26", "1"),
            ("D4", "27", "1"),
            ("D4", "28", "4"),
        ])
    }

    fn blues_head() -> NoteSet {
        note_set(&[
            ("C4", "0", "1/2"),
            ("Eb4", "1/2", "1/2"),
            ("F4", "1", "1/2"),
            ("Gb4", "3/2", "1/2"),
            ("G4", "2", "1"),
            ("Bb4", "3", "1"),
            ("C5", "4", "2"),
            ("Bb4", "6", "1"),
            ("G4", "7", "1"),
            ("F4", "8", "1"),
            ("Eb4", "9", "1/2"),
            ("C4", "19/2", "3/2"),
            ("C4", "11", "1"),
            ("G4", "12", "1"),
            ("F4", "13", "1"),
            ("Eb4", "14", "1"),
            ("C4", "15", "1"),
        ])
    }

    #[test]
    fn every_evidence_source_is_reported() {
        let ns = note_set(&[("C4", "0", "1"), ("E4", "1", "1"), ("G4", "2", "2")]);
        let k = analyse(&ns);
        let top = k.top().expect("a candidate");
        let names: Vec<&str> = top.evidence.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, KEY_EVIDENCE);
        assert_eq!(KEY_EVIDENCE.len(), 13);
    }

    #[test]
    fn candidates_are_ranked_not_singular() {
        let ns = dorian_vamp();
        let k = analyse(&ns);
        assert!(k.candidates.len() >= 5);
        for w in k.candidates.windows(2) {
            assert!(w[0].score >= w[1].score, "candidates are not sorted");
        }
    }

    #[test]
    fn a_dorian_vamp_ranks_dorian_above_natural_minor() {
        let ns = dorian_vamp();
        let k = analyse(&ns);
        let d = 2; // D
        let dorian = k
            .candidates
            .iter()
            .position(|c| c.tonic_pc() == d && c.scale_id == "dorian");
        let minor = k
            .candidates
            .iter()
            .position(|c| c.tonic_pc() == d && c.scale_id == "natural_minor");
        assert!(dorian.is_some(), "D dorian is not in the field");
        match (dorian, minor) {
            (Some(a), Some(b)) => assert!(a < b, "dorian ranked {a}, natural minor {b}"),
            (Some(_), None) => {}
            _ => panic!("unexpected ranking"),
        }
    }

    #[test]
    fn a_dorian_vamp_leads_with_d_dorian() {
        let k = analyse(&dorian_vamp());
        let top = k.top().expect("a candidate");
        assert_eq!(top.scale_id, "dorian", "top is {}", top.label());
        assert_eq!(top.tonic_pc(), 2);
        assert!(top.is_modal);
    }

    #[test]
    fn a_blues_head_is_not_forced_into_major_or_minor() {
        let k = analyse(&blues_head());
        let top = k.top().expect("a candidate");
        assert!(
            !matches!(
                top.scale_id.as_str(),
                "major" | "ionian" | "natural_minor" | "aeolian"
            ),
            "blues was forced into {}",
            top.label()
        );
        assert!(top.is_modal);
    }

    #[test]
    fn a_blues_head_leads_with_a_blues_collection() {
        let k = analyse(&blues_head());
        let top = k.top().expect("a candidate");
        assert!(
            top.scale_id.starts_with("blues") || top.scale_id.ends_with("pentatonic"),
            "top is {}",
            top.label()
        );
    }

    #[test]
    fn a_plain_diatonic_tune_leads_with_major() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("E4", "1", "1"),
            ("G4", "2", "1"),
            ("E4", "3", "1"),
            ("F4", "4", "1"),
            ("A4", "5", "1"),
            ("G4", "6", "2"),
            ("D4", "8", "1"),
            ("F4", "9", "1"),
            ("B4", "10", "1"),
            ("C5", "11", "1"),
            ("B4", "12", "1"),
            ("A4", "13", "1"),
            ("G4", "14", "1"),
            ("C4", "15", "4"),
        ]);
        let k = analyse(&ns);
        let top = k.top().expect("a candidate");
        assert_eq!(top.tonic_pc(), 0, "top is {}", top.label());
        assert!(
            matches!(top.scale_id.as_str(), "major" | "ionian"),
            "top is {}",
            top.label()
        );
    }

    #[test]
    fn the_scale_catalogue_is_the_candidate_space() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("D4", "1", "1"),
            ("E4", "2", "1"),
            ("F#4", "3", "1"),
            ("G#4", "4", "1"),
            ("A#4", "5", "1"),
            ("C5", "6", "4"),
        ]);
        let k = analyse(&ns);
        assert!(
            k.candidates.iter().any(|c| c.scale_id == "whole_tone"),
            "a whole-tone melody must reach whole-tone candidates: {:?}",
            k.candidates.iter().map(|c| c.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_user_hint_leads_but_is_still_scored() {
        let ns = dorian_vamp();
        let k = analyze_key(kb(), &ns, &ns.time_map.clone(), Some(("F", "lydian")), None);
        let top = k.top().expect("a candidate");
        assert_eq!(top.scale_id, "lydian");
        assert_eq!(top.tonic.0, Letter::F);
        assert!(top.evidence.iter().any(|(n, _)| *n == "user_hint"));
        assert!(
            top.evidence_value("tonic_emphasis") < 0.9,
            "the hint must still carry its real evidence"
        );
    }

    #[test]
    fn the_gap_and_ambiguity_flag_agree_with_the_field() {
        for ns in [
            dorian_vamp(),
            blues_head(),
            note_set(&[("C4", "0", "1"), ("F#4", "1", "1"), ("C4", "2", "1")]),
        ] {
            let k = analyse(&ns);
            assert!(k.gap >= 0.0, "the gap is a magnitude");
            let expected = match (k.candidates.first(), k.candidates.get(1)) {
                (Some(a), Some(b)) => (a.score - b.score) / a.score.abs().max(1e-6) < 0.05,
                _ => k.candidates.is_empty(),
            };
            assert_eq!(
                k.ambiguous,
                expected,
                "gap {} on {:?}",
                k.gap,
                k.top().map(|c| c.label())
            );
        }
    }

    #[test]
    fn confidence_falls_out_of_the_gap() {
        let k = analyse(&dorian_vamp());
        let top = k.top().expect("a candidate");
        assert!(top.confidence > 0.0 && top.confidence <= 0.99);
        for w in k.candidates.windows(2) {
            assert!(w[0].confidence >= w[1].confidence - 1e-9);
        }
    }

    #[test]
    fn tonic_emphasis_separates_relative_collections() {
        let k = analyse(&dorian_vamp());
        let d_dorian = k
            .candidates
            .iter()
            .find(|c| c.tonic_pc() == 2 && c.scale_id == "dorian")
            .expect("D dorian");
        assert!(d_dorian.evidence_value("tonic_emphasis") > 0.8);
    }

    #[test]
    fn modal_characteristics_punish_a_contradicted_degree() {
        let k = analyse(&dorian_vamp());
        let aeolian = k.candidates.iter().find(|c| {
            c.tonic_pc() == 2 && (c.scale_id == "aeolian" || c.scale_id == "natural_minor")
        });
        if let Some(a) = aeolian {
            assert!(
                a.evidence_value("modal_characteristics") < 0.0,
                "the flat sixth is contradicted by the natural sixth"
            );
        }
    }

    #[test]
    fn polyphonic_material_produces_bass_and_chord_evidence() {
        let ns = note_set(&[
            ("E5", "0", "2"),
            ("C4", "0", "2"),
            ("G4", "0", "2"),
            ("D5", "2", "2"),
            ("G3", "2", "2"),
            ("B4", "2", "2"),
        ]);
        let k = analyse(&ns);
        let top = k.top().expect("a candidate");
        assert!(top.evidence_value("bass_emphasis") != 0.0);
        assert!(top.evidence_value("chord_evidence") != 0.0);
    }

    #[test]
    fn monophonic_material_reports_zero_for_the_vertical_sources() {
        let ns = note_set(&[("C4", "0", "1"), ("E4", "1", "1"), ("G4", "2", "2")]);
        let k = analyse(&ns);
        let top = k.top().expect("a candidate");
        assert_eq!(top.evidence_value("bass_emphasis"), 0.0);
        assert_eq!(top.evidence_value("chord_evidence"), 0.0);
        assert_eq!(top.evidence_value("pedal_tone"), 0.0);
    }

    #[test]
    fn a_pedal_is_recognised() {
        let ns = note_set(&[
            ("C3", "0", "4"),
            ("E4", "0", "2"),
            ("G4", "2", "2"),
            ("C3", "4", "4"),
            ("F4", "4", "2"),
            ("A4", "6", "2"),
        ]);
        let k = analyse(&ns);
        let c_major = k
            .candidates
            .iter()
            .find(|c| c.tonic_pc() == 0)
            .expect("a C candidate");
        assert!(c_major.evidence_value("pedal_tone") > 0.0);
    }

    #[test]
    fn local_regions_cover_a_long_melody() {
        let ns = dorian_vamp();
        let k = analyse(&ns);
        assert!(!k.regions.is_empty());
        for r in &k.regions {
            assert!(r.end > r.start);
            assert!(!r.evidence.is_empty());
        }
    }

    #[test]
    fn a_short_melody_yields_a_single_region() {
        let ns = note_set(&[("C4", "0", "1"), ("E4", "1", "1"), ("G4", "2", "2")]);
        let k = analyse(&ns);
        assert_eq!(k.regions.len(), 1);
        assert!(!k.regions[0].is_tonicization);
    }

    #[test]
    fn key_analysis_is_deterministic() {
        let ns = dorian_vamp();
        let a = analyse(&ns).to_json().to_canonical_string();
        let b = analyse(&ns).to_json().to_canonical_string();
        assert_eq!(a, b);
    }

    #[test]
    fn an_empty_note_set_produces_no_candidates() {
        let k = analyze_key(
            kb(),
            &NoteSet::default(),
            &TimeMap::constant(120.0, TimeSignature::new(4, 4)),
            None,
            None,
        );
        assert!(k.candidates.is_empty());
        assert!(k.regions.is_empty());
        assert!(k.ambiguous);
    }

    #[test]
    fn tonic_spelling_avoids_needless_accidentals() {
        let def = kb().scale("major").expect("major");
        assert_eq!(spell_tonic(def, 6).map(class_text), Some("Gb".to_string()));
        assert_eq!(spell_tonic(def, 0).map(class_text), Some("C".to_string()));
        assert_eq!(spell_tonic(def, 10).map(class_text), Some("Bb".to_string()));
    }

    #[test]
    fn the_candidate_limit_is_respected() {
        let ns = dorian_vamp();
        let k = analyze_key_ranked(kb(), &ns, &ns.time_map.clone(), None, None, 3);
        assert_eq!(k.candidates.len(), 3);
    }

    #[test]
    fn chromatic_is_never_offered_as_a_key() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("C#4", "1", "1"),
            ("D4", "2", "1"),
            ("D#4", "3", "1"),
            ("E4", "4", "1"),
            ("F4", "5", "1"),
            ("F#4", "6", "1"),
            ("G4", "7", "1"),
        ]);
        let k = analyze_key_ranked(kb(), &ns, &ns.time_map.clone(), None, None, 64);
        assert!(k.candidates.iter().all(|c| c.scale_id != "chromatic"));
    }

    #[test]
    fn top_pcs_returns_the_leading_collection() {
        let k = analyse(&dorian_vamp());
        let pcs = k.top_pcs(kb());
        assert_eq!(pcs.len(), 7);
        assert!(pcs.contains(&11), "D dorian contains B natural");
    }
}
