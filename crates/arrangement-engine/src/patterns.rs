//! Turning a catalogued pattern into notes.
//!
//! Everything rhythmic comes out of the pattern record's `rhythm` block —
//! `bar_length_qn`, `grid_qn`, the rational `onsets`, the `sustain` rule and the
//! `velocity_curve`. Everything registral comes out of the pattern's `range`
//! intersected with the instrument profile's, and every adjacent-voice spacing
//! decision reads that instrument's own `low_interval_limits`. The engine's
//! contribution is the *pitch* policy: which chord tones a given harmonic
//! responsibility is entitled to, and in what order a monophonic figure visits
//! them.

use crate::error::ArrangementError;
use music_domain::prelude::*;
use qjson::rng::DetRng;
use theory_kb::{ArrangementPattern, InstrumentProfile, KnowledgeBase};

/// The shortest note the engine will ever write, in quarter notes.
///
/// A 128th note at any sane tempo. Durations must be strictly positive, and
/// articulation scaling must not be able to shrink one to nothing.
pub fn min_duration() -> BeatTime {
    BeatTime::new(1, 32)
}

/// Ceiling on the bars one realisation walks, so a malformed span cannot spin.
pub const MAX_BARS: i64 = 4096;

/// How a monophonic figure visits the chord tones it is entitled to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Figure {
    /// Every onset takes the same tone — the root pulse and the drone.
    Root,
    /// All entitled tones sound together.
    Block,
    /// Rising cycle through the tones — broken chords, arpeggios, ostinati.
    Ascending,
    /// Low–high–middle–high alternation, the Alberti shape.
    Alternating,
    /// A bass line: the root where the harmony changes, chord tones and a
    /// step approach in between.
    Walking,
    /// An arch: up through the tones and back down, for answering figures.
    Contour,
}

impl Figure {
    /// Stable identifier.
    pub fn id(self) -> &'static str {
        match self {
            Figure::Root => "root",
            Figure::Block => "block",
            Figure::Ascending => "ascending",
            Figure::Alternating => "alternating",
            Figure::Walking => "walking",
            Figure::Contour => "contour",
        }
    }

    /// Parses the identifier produced by [`Figure::id`].
    pub fn parse(s: &str) -> Option<Figure> {
        match s {
            "root" => Some(Figure::Root),
            "block" => Some(Figure::Block),
            "ascending" => Some(Figure::Ascending),
            "alternating" => Some(Figure::Alternating),
            "walking" => Some(Figure::Walking),
            "contour" => Some(Figure::Contour),
            _ => None,
        }
    }

    /// Which chord tone the `i`-th onset of a monophonic figure takes.
    pub fn index(self, i: usize, available: usize) -> usize {
        if available == 0 {
            return 0;
        }
        match self {
            Figure::Root => 0,
            Figure::Block => 0,
            Figure::Ascending | Figure::Walking => i % available,
            Figure::Alternating => {
                // 0, top, middle, top — the Alberti shape, generalised to any
                // number of available tones.
                let top = available - 1;
                let middle = if available >= 3 { available - 2 } else { 0 };
                match i % 4 {
                    0 => 0,
                    2 => middle,
                    _ => top,
                }
            }
            Figure::Contour => {
                let period = (available * 2).saturating_sub(2).max(1);
                let step = i % period;
                if step < available {
                    step
                } else {
                    period - step
                }
            }
        }
    }
}

/// The figure a pattern uses.
///
/// A pattern record may declare `"figure"` outright, which is the editable
/// path; when it does not, the figure is derived from the fields it *does*
/// declare — the harmonic responsibility first, then polyphony and the
/// foreground/background priority, which is what separates a background
/// Alberti accompaniment from a midground broken-chord figure over the same
/// grid.
pub fn figure_for(pattern: &ArrangementPattern) -> Figure {
    if let Some(declared) = pattern
        .raw
        .get("figure")
        .and_then(qjson::Json::as_str)
        .and_then(Figure::parse)
    {
        return declared;
    }
    match pattern.harmonic_responsibility.as_str() {
        "root_only" => Figure::Root,
        "root_and_line" => Figure::Walking,
        "none" => Figure::Contour,
        _ if pattern.polyphony <= 1 => match pattern.priority.as_str() {
            "background" => Figure::Alternating,
            _ => Figure::Ascending,
        },
        _ => Figure::Block,
    }
}

/// How much of its nominal length a note keeps, given the pattern's declared
/// articulation tendency.
///
/// This is the articulation lever the brief asks for: the same grid played
/// staccato covers half the time a legato reading of it does, which is audible
/// and measurable and has nothing to do with volume.
///
/// A pattern that declares several tendencies takes the shortest of them: a
/// part marked `["accent", "staccato"]` is detached *and* accented, and it is
/// the detachment that decides how long the note is.
pub fn articulation_scale(pattern: &ArrangementPattern) -> f64 {
    let mut scale = 1.0f64;
    for a in &pattern.articulation_tendency {
        let candidate = match a.as_str() {
            "staccato" => 0.5,
            "mute" => 0.35,
            "accent" => 0.9,
            "legato" | "sustain" | "pedal" => 1.0,
            _ => continue,
        };
        scale = scale.min(candidate);
    }
    scale
}

/// The articulation label written onto every note of a pattern, if it declares
/// one that the chosen instrument can actually play.
pub fn articulation_label(
    pattern: &ArrangementPattern,
    inst: &InstrumentProfile,
) -> Option<String> {
    pattern
        .articulation_tendency
        .iter()
        .find(|a| inst.articulations.iter().any(|i| i == *a))
        .or_else(|| pattern.articulation_tendency.first())
        .cloned()
}

/// Where in its window a pattern sits, from its declared `register`.
///
/// A root pulse marked `"low"` belongs at the bottom of the window it was
/// given; a `"mid_high"` answering figure belongs near the top. Reading the
/// field rather than always centring is what stops every part converging on the
/// middle of its range.
pub fn register_anchor(pattern: &ArrangementPattern, window: (i32, i32)) -> i32 {
    let span = (window.1 - window.0).max(0);
    let at = |numerator: i32, denominator: i32| window.0 + span * numerator / denominator;
    match pattern.register.as_str() {
        "low" => at(1, 6),
        "low_mid" => at(1, 3),
        "mid_high" => at(2, 3),
        "high" => at(5, 6),
        _ => at(1, 2),
    }
}

/// How much a low density shortens a note as well as thinning the onsets.
///
/// Density is not only how often a part speaks but how much of the time it
/// occupies; a part asked to be sparse that filled every gap with a long note
/// would not sound sparse at all. Genuinely sustained textures — a pad, a
/// drone, a chorale, anything whose note length tendency is the whole harmonic
/// slot — are exempt, because sustaining is the whole of what they do.
pub fn density_length_scale(pattern: &ArrangementPattern, density: f64) -> f64 {
    if is_sustained(pattern) {
        return 1.0;
    }
    0.55 + 0.45 * density.clamp(0.0, 1.0)
}

/// The share of a pattern's onsets that land where another part already plays.
///
/// `contested` holds positions *within a bar*, so a pattern can be judged
/// against a lead's rhythm before either has been realised. This is what lets
/// role selection prefer contrary rhythmic activity rather than discovering the
/// collision afterwards.
pub fn onset_conflict(pattern: &ArrangementPattern, contested: &[BeatTime]) -> f64 {
    if pattern.rhythm.onsets.is_empty() || contested.is_empty() {
        return 0.0;
    }
    let hits = pattern
        .rhythm
        .onsets
        .iter()
        .filter(|o| contested.contains(o))
        .count();
    hits as f64 / pattern.rhythm.onsets.len() as f64
}

/// True when the pattern is a sustained texture — a pad, a drone, a chorale.
///
/// Sustained parts are exempt from the rest requirement, are the ones whose
/// common tones are tied across a harmony change, and are the ones the
/// collision rules leave alone: a pad that stops sounding under the lead is no
/// longer a pad. Note that holding a note *to the next onset* is not the same
/// thing as sustaining — a legato counterline does that and is still a line.
pub fn is_sustained(pattern: &ArrangementPattern) -> bool {
    pattern.rhythmic_activity == "sustained" || pattern.note_length_tendency == "full_slot"
}

/// Everything a realisation needs beyond the pattern itself.
#[derive(Clone, Debug)]
pub struct RealizeOptions {
    /// The MIDI window the part is written into.
    pub window: (i32, i32),
    /// The span to fill.
    pub span: (BeatTime, BeatTime),
    /// Onset-density control, `0.0..=1.0`.
    pub density: f64,
    /// Multiplies every velocity taken from the pattern's curve.
    pub velocity_scale: f64,
    /// Multiplies every note length after the articulation scale.
    pub note_length_scale: f64,
    /// Hard ceiling on simultaneous notes.
    pub max_polyphony: usize,
    /// First note id this part may use.
    pub id_base: NoteId,
    /// MIDI channel.
    pub channel: u8,
    /// The note-level role stamped on every note.
    pub note_role: NoteRole,
    /// Tie-breaking seed.
    pub seed: u64,
    /// Onsets a lower-priority part should keep off, in order.
    pub avoid_onsets: Vec<BeatTime>,
    /// Windows the part is allowed to sound in; empty means the whole span.
    pub active: Vec<(BeatTime, BeatTime)>,
    /// Piecewise-linear energy curve, sampled for velocity.
    pub energy: Vec<(BeatTime, f64)>,
}

impl RealizeOptions {
    /// Neutral options over a span and window.
    pub fn new(window: (i32, i32), span: (BeatTime, BeatTime)) -> RealizeOptions {
        RealizeOptions {
            window,
            span,
            density: 0.5,
            velocity_scale: 1.0,
            note_length_scale: 1.0,
            max_polyphony: 8,
            id_base: 0,
            channel: 0,
            note_role: NoteRole::Harmony,
            seed: 0,
            avoid_onsets: Vec::new(),
            active: Vec::new(),
            energy: Vec::new(),
        }
    }

    /// True when `qn` falls inside an active window.
    pub fn is_active(&self, qn: BeatTime) -> bool {
        self.active.is_empty() || self.active.iter().any(|(s, e)| qn >= *s && qn < *e)
    }
}

/// Samples a piecewise-linear curve, holding the end values outside its range.
pub fn sample_curve(curve: &[(BeatTime, f64)], qn: BeatTime, default: f64) -> f64 {
    if curve.is_empty() {
        return default;
    }
    if qn <= curve[0].0 {
        return curve[0].1;
    }
    let last = curve[curve.len() - 1];
    if qn >= last.0 {
        return last.1;
    }
    for w in curve.windows(2) {
        let (a, b) = (w[0], w[1]);
        if qn >= a.0 && qn <= b.0 {
            let span = (b.0 - a.0).as_f64();
            if span <= 0.0 {
                return b.1;
            }
            let t = (qn - a.0).as_f64() / span;
            return a.1 + (b.1 - a.1) * t;
        }
    }
    default
}

/// Stage 9's public entry point, as the frozen contract declares it.
///
/// The window is the pattern's range intersected with the instrument's, the
/// polyphony ceiling is the smaller of the two records', and the span is the
/// span the chords cover.
pub fn realize_pattern(
    kb: &KnowledgeBase,
    pat: &ArrangementPattern,
    chords: &[ChordEvent],
    tm: &TimeMap,
    inst: &InstrumentProfile,
    density: f64,
    seed: u64,
) -> Result<Vec<Note>, ArrangementError> {
    let _ = kb;
    if chords.is_empty() {
        return Ok(Vec::new());
    }
    let span = (
        chords[0].onset,
        chords
            .iter()
            .map(|c| c.onset + c.duration)
            .fold(chords[0].onset, BeatTime::max),
    );
    let window = crate::roles::base_window(pat, inst);
    let mut opts = RealizeOptions::new(window, span);
    opts.density = density.clamp(0.0, 1.0);
    opts.max_polyphony = (pat.polyphony.min(inst.polyphony)).max(1) as usize;
    opts.seed = seed;
    realize_with(pat, chords, tm, inst, &opts)
}

/// The full realisation, with every lever exposed.
pub fn realize_with(
    pattern: &ArrangementPattern,
    chords: &[ChordEvent],
    tm: &TimeMap,
    inst: &InstrumentProfile,
    opts: &RealizeOptions,
) -> Result<Vec<Note>, ArrangementError> {
    if chords.is_empty() {
        return Ok(Vec::new());
    }
    let (start, end) = opts.span;
    if end <= start {
        return Err(ArrangementError::invalid_argument(
            "the arrangement span is empty",
        ));
    }
    let window = normalize_window(opts.window, inst);
    let onsets = plan_onsets(pattern, chords, tm, opts)?;
    let figure = figure_for(pattern);
    let art_scale = articulation_scale(pattern);
    let label = articulation_label(pattern, inst);
    let max_voices = opts.max_polyphony.clamp(1, inst.polyphony.max(1) as usize);
    let sustained = is_sustained(pattern);
    let density_scale = density_length_scale(pattern, opts.density);
    let home = register_anchor(pattern, window);

    let mut notes: Vec<Note> = Vec::new();
    let mut previous_top: Option<i32> = None;
    for (i, onset) in onsets.iter().enumerate() {
        let chord = chord_at(chords, *onset);
        let slot_end = chord.onset + chord.duration;
        let next = onsets.get(i + 1).copied();
        let length = note_length(pattern, *onset, next, slot_end, end, tm);
        let length = scale_length(length, art_scale * opts.note_length_scale * density_scale);
        if !length.is_positive() {
            continue;
        }
        let material = chord_material(&chord.spec, &pattern.harmonic_responsibility);
        if material.is_empty() {
            continue;
        }
        // More than one voice means a stack, whatever the figure: a two-voice
        // pedal is a root and its declared octave, not a root on its own.
        let pitches = if max_voices > 1 {
            let mut stack = place_stack(&material, window, inst, max_voices);
            if opts.max_polyphony > material.len() && allows_doubling(pattern) {
                double_top(&mut stack, &material, window, inst, opts.max_polyphony);
            }
            stack
        } else {
            let idx = figure.index(i, material.len());
            let (degree, class) = material[idx.min(material.len() - 1)];
            // A root figure stays put; every other figure follows the line it
            // has been drawing, so an arpeggio does not jump octaves mid-bar.
            let anchor = match figure {
                Figure::Root => home,
                _ => previous_top.unwrap_or(home),
            };
            let midi = place_single(class, window, anchor);
            vec![(degree, spelled_at(class, midi), midi)]
        };
        if pitches.is_empty() {
            continue;
        }
        previous_top = pitches.last().map(|(_, _, m)| *m);
        let energy = sample_curve(&opts.energy, *onset, 0.5);
        for (voice, (_, pitch, midi)) in pitches.iter().enumerate() {
            let velocity = velocity_for(pattern, i, energy, opts.velocity_scale, voice);
            let mut note = Note::new(opts.id_base + notes.len() as NoteId, *pitch, *onset, length);
            note.midi = *midi;
            note.velocity = velocity;
            note.channel = opts.channel;
            note.role = opts.note_role;
            note.voice = VoiceId(voice as u16);
            note.articulation = label.clone();
            note.confidence = chord.confidence;
            note.salience = if pattern.priority == "foreground" {
                0.8
            } else {
                0.3
            };
            notes.push(note);
        }
    }

    if sustained {
        tie_common_tones(&mut notes);
    }
    renumber(&mut notes, opts.id_base);
    Ok(notes)
}

/// Clips a requested window to what the instrument can play, never empty.
pub fn normalize_window(window: (i32, i32), inst: &InstrumentProfile) -> (i32, i32) {
    let low = window.0.max(inst.range.low_midi).clamp(0, 127);
    let high = window.1.min(inst.range.high_midi).clamp(0, 127);
    if low >= high {
        (
            inst.range.low_midi.clamp(0, 127),
            inst.range.high_midi.clamp(0, 127),
        )
    } else {
        (low, high)
    }
}

/// The onsets a realisation will write, after the density control.
///
/// The pattern's authored grid is walked bar by bar over the actual meter, and
/// then thinned: the onsets that survive are the metrically strongest ones,
/// which is why reducing density leaves a musically coherent skeleton rather
/// than a random subset. `sustain: "full_slot"` additionally pins an onset to
/// every harmonic slot boundary, because that is what "the slot" means.
pub fn plan_onsets(
    pattern: &ArrangementPattern,
    chords: &[ChordEvent],
    tm: &TimeMap,
    opts: &RealizeOptions,
) -> Result<Vec<BeatTime>, ArrangementError> {
    let (start, end) = opts.span;
    if pattern.rhythm.onsets.is_empty() {
        return Err(ArrangementError::knowledge_missing(format!(
            "pattern {} declares no onsets",
            pattern.id
        )));
    }
    let mut raw: Vec<BeatTime> = Vec::new();
    let first_bar = tm.bar_of(start);
    let mut bar = first_bar;
    let mut guard = 0i64;
    loop {
        guard += 1;
        if guard > MAX_BARS {
            break;
        }
        let bar_start = tm.bar_start(bar);
        if bar_start >= end {
            break;
        }
        let bar_len = tm.meter_at(bar_start).bar_length_qn();
        for offset in &pattern.rhythm.onsets {
            if *offset >= bar_len {
                continue;
            }
            let qn = bar_start + *offset;
            if qn < start || qn >= end {
                continue;
            }
            if !raw.contains(&qn) {
                raw.push(qn);
            }
        }
        bar += 1;
    }
    if pattern.rhythm.sustain == "full_slot" {
        for c in chords {
            if c.onset >= start && c.onset < end && !raw.contains(&c.onset) {
                raw.push(c.onset);
            }
        }
    }
    raw.sort();
    raw = displace_from_contested(raw, pattern, opts, start, end);
    // A part that carries harmony must articulate every chord change, however
    // low the density control goes: thinning is allowed to make a part sparse,
    // never to make it wrong.
    let changes: Vec<BeatTime> = chords
        .iter()
        .map(|c| c.onset)
        .filter(|qn| *qn >= start && *qn < end)
        .collect();
    let floor = if pattern.harmonic_responsibility == "none" {
        1
    } else {
        changes.len().max(1)
    };
    let kept = thin(&raw, pattern, tm, opts, &changes, floor);
    Ok(kept)
}

/// Moves a whole figure off the beats a more important part already occupies.
///
/// This is *contrary rhythmic activity* as an action rather than a hope: when
/// more than half a pattern's attacks would land on the lead's, the figure is
/// displaced by half its own grid so it answers between the tune's notes
/// instead of doubling them. Sustained textures are exempt — they are supposed
/// to be underneath — and the displacement is half the pattern's declared grid,
/// so the figure keeps its own rhythmic identity.
pub fn displace_from_contested(
    raw: Vec<BeatTime>,
    pattern: &ArrangementPattern,
    opts: &RealizeOptions,
    start: BeatTime,
    end: BeatTime,
) -> Vec<BeatTime> {
    if opts.avoid_onsets.is_empty() || raw.is_empty() || is_sustained(pattern) {
        return raw;
    }
    let hits = raw
        .iter()
        .filter(|qn| opts.avoid_onsets.contains(qn))
        .count();
    if hits * 2 <= raw.len() {
        return raw;
    }
    // Half the grid first, then quarter, then eighth: the smallest displacement
    // that actually clears the contested attacks is the one that keeps the
    // figure closest to what the catalogue wrote.
    for divisor in DISPLACEMENT_DIVISORS {
        let step = pattern.rhythm.grid_qn.scale(1, *divisor);
        if !step.is_positive() {
            continue;
        }
        let moved: Vec<BeatTime> = raw
            .iter()
            .map(|qn| *qn + step)
            .filter(|qn| *qn >= start && *qn < end)
            .collect();
        if moved.is_empty() {
            continue;
        }
        let after = moved
            .iter()
            .filter(|qn| opts.avoid_onsets.contains(qn))
            .count();
        if after * 2 <= moved.len() {
            return moved;
        }
    }
    raw
}

/// The fractions of its own grid a displaced figure is offset by, in order.
pub const DISPLACEMENT_DIVISORS: &[i64] = &[2, 4, 8, 3];

/// The share of a pattern's onsets a density setting keeps.
///
/// Monotone in `density` by construction, and never zero: a part that is asked
/// for is still written, only sparsely.
pub fn keep_ratio(pattern: &ArrangementPattern, density: f64) -> f64 {
    let requested = density.clamp(0.0, 1.0);
    let blended = 0.35 * pattern.density.clamp(0.0, 1.0) + 0.65 * requested;
    (0.12 + 0.88 * blended).clamp(0.05, 1.0)
}

/// Thins an onset list down to the density budget.
fn thin(
    raw: &[BeatTime],
    pattern: &ArrangementPattern,
    tm: &TimeMap,
    opts: &RealizeOptions,
    changes: &[BeatTime],
    floor: usize,
) -> Vec<BeatTime> {
    let active: Vec<BeatTime> = raw
        .iter()
        .copied()
        .filter(|qn| opts.is_active(*qn))
        .collect();
    if active.is_empty() {
        return active;
    }
    let ratio = keep_ratio(pattern, opts.density);
    let want = ((active.len() as f64) * ratio)
        .ceil()
        .max(1.0)
        .max(floor as f64) as usize;
    if want >= active.len() && opts.avoid_onsets.is_empty() {
        return active;
    }
    // Rank by musical importance: metric weight first, then whether the onset
    // collides with a part that outranks this one, then position. Every term is
    // a total order, so the result never depends on sort stability alone.
    let mut ranked: Vec<(usize, f64)> = active
        .iter()
        .enumerate()
        .map(|(i, qn)| {
            let mut score = tm.metric_weight(*qn);
            if changes.contains(qn) {
                score += 2.0;
            }
            if opts.avoid_onsets.contains(qn) {
                score -= 0.75;
            }
            if i == 0 {
                score += 0.5;
            }
            (i, score)
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut keep: Vec<usize> = ranked
        .into_iter()
        .take(want.min(active.len()))
        .map(|(i, _)| i)
        .collect();
    keep.sort_unstable();
    keep.into_iter().map(|i| active[i]).collect()
}

/// The nominal length of a note starting at `onset`.
fn note_length(
    pattern: &ArrangementPattern,
    onset: BeatTime,
    next: Option<BeatTime>,
    slot_end: BeatTime,
    span_end: BeatTime,
    tm: &TimeMap,
) -> BeatTime {
    let grid = if pattern.rhythm.grid_qn.is_positive() {
        pattern.rhythm.grid_qn
    } else {
        tm.meter_at(onset).beat_unit_qn()
    };
    let cap = slot_end.min(span_end);
    let raw = match pattern.rhythm.sustain.as_str() {
        // A note holds until the harmony changes — or until this part next
        // articulates, whichever comes first. Without the second cap a pattern
        // with two onsets inside one harmonic slot would stack its own voices
        // on top of each other and blow past the instrument's polyphony.
        "full_slot" | "to_next" => match next {
            Some(n) => n.min(cap),
            None => cap,
        },
        "mixed" => {
            let alternate = onset + grid;
            match next {
                Some(n) if n <= alternate => n.min(cap),
                _ => alternate.min(cap),
            }
        }
        _ => {
            let ends = onset + grid;
            match next {
                Some(n) => ends.min(n).min(span_end),
                None => ends.min(span_end),
            }
        }
    };
    if raw <= onset {
        min_duration().min(span_end - onset).max(min_duration())
    } else {
        raw - onset
    }
}

/// Applies an articulation or note-length scale, never producing a zero.
pub fn scale_length(length: BeatTime, scale: f64) -> BeatTime {
    if scale >= 1.0 {
        return length;
    }
    let scaled = BeatTime::from_f64((length.as_f64() * scale.max(0.05)).max(0.0));
    if scaled.is_positive() {
        scaled.min(length)
    } else {
        min_duration().min(length)
    }
}

/// The chord sounding at a position, or the nearest earlier one.
pub fn chord_at(chords: &[ChordEvent], qn: BeatTime) -> &ChordEvent {
    let mut best = &chords[0];
    for c in chords {
        if c.onset <= qn {
            best = c;
        }
        if c.onset <= qn && qn < c.onset + c.duration {
            return c;
        }
    }
    best
}

/// The chord tones a harmonic responsibility entitles a part to, in order from
/// the root upward.
pub fn chord_material(
    spec: &ChordSpec,
    responsibility: &str,
) -> Vec<(ChordDegree, (Letter, Accidental))> {
    let all = spec.chord_tones();
    if all.is_empty() {
        return all;
    }
    match responsibility {
        "root_only" => vec![all[0]],
        "guide_tones" => {
            let guides = spec.guide_tones();
            let picked: Vec<(ChordDegree, (Letter, Accidental))> = all
                .iter()
                .copied()
                .filter(|(d, _)| guides.contains(d))
                .collect();
            if picked.is_empty() {
                all
            } else {
                picked
            }
        }
        "partial" => {
            // Root, fifth and third: the frame of the chord without the
            // colour tones a fuller part is carrying.
            let mut picked: Vec<(ChordDegree, (Letter, Accidental))> = all
                .iter()
                .copied()
                .filter(|(d, _)| matches!(d.number, 1 | 3 | 5))
                .collect();
            if picked.is_empty() {
                picked = all;
            }
            picked
        }
        "root_and_line" => all,
        "none" => all,
        _ => all,
    }
}

/// True when the pattern declares any doubling at all.
pub fn allows_doubling(pattern: &ArrangementPattern) -> bool {
    !pattern.allowed_doubling.is_empty()
}

/// Builds a voicing upward inside the window, honouring the instrument's own
/// low-interval limits.
pub fn place_stack(
    material: &[(ChordDegree, (Letter, Accidental))],
    window: (i32, i32),
    inst: &InstrumentProfile,
    max_voices: usize,
) -> Vec<(ChordDegree, SpelledPitch, i32)> {
    let mut out: Vec<(ChordDegree, SpelledPitch, i32)> = Vec::new();
    let mut floor = window.0;
    for (degree, class) in material.iter().take(max_voices.max(1)) {
        let pc = class_pc(*class);
        let midi = lowest_at_or_above(pc, floor);
        if midi > window.1 {
            break;
        }
        out.push((*degree, spelled_at(*class, midi), midi));
        let gap = inst.min_spacing_at(midi).unwrap_or(1).max(1);
        floor = midi + gap;
    }
    if out.is_empty() {
        if let Some((degree, class)) = material.first() {
            let midi = place_single(*class, window, (window.0 + window.1) / 2);
            out.push((*degree, spelled_at(*class, midi), midi));
        }
    }
    out
}

/// Adds a declared octave doubling of the bottom voice above the stack.
fn double_top(
    stack: &mut Vec<(ChordDegree, SpelledPitch, i32)>,
    material: &[(ChordDegree, (Letter, Accidental))],
    window: (i32, i32),
    inst: &InstrumentProfile,
    max_voices: usize,
) {
    if stack.len() >= max_voices || stack.is_empty() || material.is_empty() {
        return;
    }
    let (degree, class) = material[0];
    let top = stack[stack.len() - 1].2;
    let gap = inst.min_spacing_at(top).unwrap_or(1).max(1);
    let midi = lowest_at_or_above(class_pc(class), top + gap);
    if midi <= window.1 {
        stack.push((degree, spelled_at(class, midi), midi));
    }
}

/// Places one pitch class in the octave nearest an anchor, inside the window.
pub fn place_single(class: (Letter, Accidental), window: (i32, i32), anchor: i32) -> i32 {
    let pc = class_pc(class);
    let mut best = lowest_at_or_above(pc, window.0);
    let mut candidate = best;
    let mut distance = (candidate - anchor).abs();
    while candidate + 12 <= window.1 {
        candidate += 12;
        let d = (candidate - anchor).abs();
        if d < distance {
            distance = d;
            best = candidate;
        }
    }
    best.clamp(window.0.max(0), window.1.min(127))
}

/// The lowest MIDI number at or above `floor` with the given pitch class.
pub fn lowest_at_or_above(pc: i32, floor: i32) -> i32 {
    let pc = pc.rem_euclid(12);
    let base = floor.rem_euclid(12);
    let delta = (pc - base).rem_euclid(12);
    floor + delta
}

/// The sounding pitch class of a spelled class.
pub fn class_pc(class: (Letter, Accidental)) -> i32 {
    (class.0.natural_pc() + i32::from(class.1 .0)).rem_euclid(12)
}

/// The spelled pitch for a class at a concrete MIDI number.
pub fn spelled_at(class: (Letter, Accidental), midi: i32) -> SpelledPitch {
    let semitones = class.0.natural_pc() + i32::from(class.1 .0);
    let octave = (midi - semitones).div_euclid(12) - 1;
    SpelledPitch::new(class.0, class.1, octave)
}

/// The velocity of the `i`-th onset.
///
/// The pattern's own curve provides the accent shape, energy scales it, and
/// voices below the top of a stack are softened so the top of the voicing
/// speaks — dynamics as one lever among many, never the only one.
pub fn velocity_for(
    pattern: &ArrangementPattern,
    i: usize,
    energy: f64,
    scale: f64,
    voice: usize,
) -> u8 {
    let curve = &pattern.rhythm.velocity_curve;
    let base = if curve.is_empty() {
        96.0
    } else {
        curve[i % curve.len()] as f64
    };
    let inner = if voice == 0 { 1.0 } else { 0.94 };
    let shaped = base * (0.65 + 0.7 * energy.clamp(0.0, 1.0)) * scale * inner;
    shaped.round().clamp(1.0, 127.0) as u8
}

/// Ties adjacent notes of the same pitch and voice into one.
///
/// This is what makes a pad hold a common tone across a harmony change instead
/// of re-striking it, which is exactly what
/// `arrangement.pad_sustains_common_tones` asks for.
pub fn tie_common_tones(notes: &mut Vec<Note>) {
    notes.sort_by(|a, b| a.midi.cmp(&b.midi).then_with(|| a.onset.cmp(&b.onset)));
    let mut out: Vec<Note> = Vec::with_capacity(notes.len());
    for note in notes.drain(..) {
        let merged = match out.last_mut() {
            // Same sounding pitch, picked up exactly where the last one left
            // off: one held note, whatever voice index the stack gave it.
            Some(prev) if prev.midi == note.midi && prev.end() == note.onset => {
                prev.duration = prev.duration + note.duration;
                true
            }
            _ => false,
        };
        if !merged {
            out.push(note);
        }
    }
    out.sort_by(|a, b| {
        a.onset
            .cmp(&b.onset)
            .then_with(|| a.midi.cmp(&b.midi))
            .then_with(|| a.voice.cmp(&b.voice))
    });
    *notes = out;
}

/// Re-assigns note ids so they are contiguous and ordered.
pub fn renumber(notes: &mut [Note], base: NoteId) {
    notes.sort_by(|a, b| {
        a.onset
            .cmp(&b.onset)
            .then_with(|| a.midi.cmp(&b.midi))
            .then_with(|| a.voice.cmp(&b.voice))
    });
    for (i, n) in notes.iter_mut().enumerate() {
        n.id = base + i as NoteId;
    }
}

/// A seeded stream for a named decision, so adding a consumer never shifts an
/// existing one.
pub fn stream(seed: u64, label: &str) -> DetRng {
    DetRng::new(seed).derive(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use theory_kb::KnowledgeBase;

    fn kb() -> &'static KnowledgeBase {
        KnowledgeBase::embedded()
    }

    fn pattern(id: &str) -> &'static ArrangementPattern {
        kb().arrangement_patterns()
            .iter()
            .find(|p| p.id == id)
            .expect("pattern")
    }

    fn inst(id: &str) -> &'static InstrumentProfile {
        kb().instrument_profile(id).expect("instrument")
    }

    fn chords() -> Vec<ChordEvent> {
        ["C", "Am", "F", "G7"]
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let spec = music_domain::symbol::parse(s).expect("symbol");
                let mut c = ChordEvent::new(
                    i as u32,
                    spec,
                    BeatTime::from_quarters(i as i64 * 4),
                    BeatTime::from_quarters(4),
                );
                c.confidence = 1.0;
                c
            })
            .collect()
    }

    #[test]
    fn lowest_at_or_above_is_exact() {
        assert_eq!(lowest_at_or_above(0, 60), 60);
        assert_eq!(lowest_at_or_above(1, 60), 61);
        assert_eq!(lowest_at_or_above(11, 60), 71);
        assert_eq!(lowest_at_or_above(0, 61), 72);
    }

    #[test]
    fn spelled_at_round_trips_through_midi() {
        for midi in 24..100 {
            let sp = spelled_at(
                (Letter::C, Accidental::NATURAL),
                lowest_at_or_above(0, midi),
            );
            assert_eq!(sp.midi(), lowest_at_or_above(0, midi));
        }
        let sp = spelled_at((Letter::B, Accidental::FLAT), 58);
        assert_eq!(sp.midi(), 58);
        assert_eq!(sp.to_ascii(), "Bb3");
    }

    #[test]
    fn figures_stay_in_bounds() {
        for f in [
            Figure::Root,
            Figure::Block,
            Figure::Ascending,
            Figure::Alternating,
            Figure::Walking,
            Figure::Contour,
        ] {
            assert_eq!(Figure::parse(f.id()), Some(f));
            for available in 1..6usize {
                for i in 0..24usize {
                    assert!(f.index(i, available) < available, "{} {i}", f.id());
                }
            }
            assert_eq!(f.index(3, 0), 0);
        }
    }

    #[test]
    fn root_only_patterns_use_the_root_figure() {
        assert_eq!(figure_for(pattern("arr_root_pulse_bass")), Figure::Root);
        assert_eq!(figure_for(pattern("arr_drone")), Figure::Root);
        assert_eq!(figure_for(pattern("arr_walking_bass")), Figure::Walking);
        assert_eq!(figure_for(pattern("arr_alberti")), Figure::Alternating);
        assert_eq!(figure_for(pattern("arr_broken_chords")), Figure::Ascending);
        assert_eq!(figure_for(pattern("arr_block_chords")), Figure::Block);
    }

    #[test]
    fn articulation_scales_come_from_the_catalogue() {
        assert_eq!(articulation_scale(pattern("arr_block_chords")), 0.5);
        assert_eq!(articulation_scale(pattern("arr_sustained_pad")), 1.0);
    }

    #[test]
    fn keep_ratio_is_monotone_in_density() {
        let p = pattern("arr_block_chords");
        let mut previous = 0.0;
        for step in 0..=10 {
            let d = step as f64 / 10.0;
            let r = keep_ratio(p, d);
            assert!(r >= previous, "not monotone at {d}");
            previous = r;
        }
    }

    #[test]
    fn a_grid_pattern_lands_only_on_its_grid() {
        let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
        let p = pattern("arr_block_chords");
        let notes = realize_pattern(kb(), p, &chords(), &tm, inst("piano_keys"), 1.0, 7).unwrap();
        assert!(!notes.is_empty());
        for n in &notes {
            let in_bar = tm.position_in_bar(n.onset);
            assert!(
                p.rhythm.onsets.contains(&in_bar),
                "{} is off the grid",
                n.onset
            );
        }
    }

    #[test]
    fn a_pad_sustains() {
        let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
        let p = pattern("arr_sustained_pad");
        let notes = realize_pattern(kb(), p, &chords(), &tm, inst("pad"), 0.5, 3).unwrap();
        assert!(!notes.is_empty());
        let mean = notes.iter().map(|n| n.duration.as_f64()).sum::<f64>() / notes.len() as f64;
        assert!(mean >= 3.0, "pad notes average {mean} QN");
    }

    #[test]
    fn every_note_is_inside_the_instrument_range() {
        let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
        for p in kb().arrangement_patterns() {
            let role = ArrangementRole::parse(&p.role).expect("a known role");
            let i = crate::roles::select_instrument(kb(), p, role).expect("instrument");
            let notes = realize_pattern(kb(), p, &chords(), &tm, i, 0.7, 11).unwrap();
            for n in &notes {
                assert!(
                    i.range.contains(n.midi),
                    "{} wrote {} outside {}..{}",
                    p.id,
                    n.midi,
                    i.range.low_midi,
                    i.range.high_midi
                );
                assert!(n.duration.is_positive());
                assert!((0..=127).contains(&n.midi));
            }
        }
    }

    #[test]
    fn realisation_is_deterministic() {
        let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
        for p in kb().arrangement_patterns() {
            let role = ArrangementRole::parse(&p.role).expect("a known role");
            let i = crate::roles::select_instrument(kb(), p, role).expect("instrument");
            let a = realize_pattern(kb(), p, &chords(), &tm, i, 0.6, 5).unwrap();
            let b = realize_pattern(kb(), p, &chords(), &tm, i, 0.6, 5).unwrap();
            assert_eq!(a.len(), b.len());
            for (x, y) in a.iter().zip(b.iter()) {
                assert_eq!(
                    (x.midi, x.onset, x.duration, x.velocity),
                    (y.midi, y.onset, y.duration, y.velocity)
                );
            }
        }
    }

    #[test]
    fn sample_curve_interpolates_and_holds() {
        let curve = vec![(BeatTime::ZERO, 0.0), (BeatTime::from_quarters(4), 1.0)];
        assert_eq!(sample_curve(&curve, BeatTime::from_quarters(-1), 0.5), 0.0);
        assert_eq!(sample_curve(&curve, BeatTime::from_quarters(2), 0.5), 0.5);
        assert_eq!(sample_curve(&curve, BeatTime::from_quarters(9), 0.5), 1.0);
        assert_eq!(sample_curve(&[], BeatTime::ZERO, 0.42), 0.42);
    }

    #[test]
    fn empty_chords_realise_to_nothing() {
        let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
        let notes = realize_pattern(
            kb(),
            pattern("arr_block_chords"),
            &[],
            &tm,
            inst("piano_keys"),
            0.5,
            0,
        )
        .unwrap();
        assert!(notes.is_empty());
    }

    #[test]
    fn guide_tone_material_is_the_third_and_seventh() {
        let spec = music_domain::symbol::parse("Dm7").expect("symbol");
        let material = chord_material(&spec, "guide_tones");
        let numbers: Vec<u8> = material.iter().map(|(d, _)| d.number).collect();
        assert_eq!(numbers, vec![3, 7]);
    }

    #[test]
    fn root_only_material_is_one_tone() {
        let spec = music_domain::symbol::parse("G7").expect("symbol");
        assert_eq!(chord_material(&spec, "root_only").len(), 1);
    }

    #[test]
    fn spacing_respects_the_instrument_low_interval_limits() {
        let spec = music_domain::symbol::parse("Cmaj7").expect("symbol");
        let material = chord_material(&spec, "full");
        let bass = inst("bass");
        let stack = place_stack(&material, (28, 60), bass, 4);
        for w in stack.windows(2) {
            let gap = w[1].2 - w[0].2;
            let limit = bass.min_spacing_at(w[0].2).unwrap_or(1);
            assert!(gap >= limit, "gap {gap} below limit {limit}");
        }
    }
}
