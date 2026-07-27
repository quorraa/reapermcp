//! Stage 8b — the countermelody.
//!
//! A countermelody is not a harmonised copy of the tune. It is generated from
//! where the melody *is not*: its onsets go into the melody's rests and
//! sustains, its register is separated from the melody's, its motion is
//! contrary or oblique where it can be, and it stops at phrase arrivals. Fixed
//! parallel harmony is a strategy the caller can ask for explicitly; it is
//! never what this function does by default.

use crate::error::HarmonyError;
use crate::params::CountermelodyParams;
use music_analysis::report::Analysis;
use music_domain::prelude::*;
use qjson::rng::DetRng;
use theory_kb::{KnowledgeBase, ResolvedProfile};

/// First note id the countermelody uses.
pub const COUNTER_ID_BASE: NoteId = 200_000;

/// Default velocity of a countermelody note.
pub const COUNTER_VELOCITY: u8 = 78;

/// Smallest register gap the countermelody keeps from the melody, in semitones.
pub const MIN_REGISTER_SEPARATION: i32 = 3;

/// Stage 8b, as the frozen contract declares it.
pub fn generate_countermelody(
    kb: &KnowledgeBase,
    prof: &ResolvedProfile,
    melody: &NoteSet,
    chords: &[ChordEvent],
    an: &Analysis,
    p: &CountermelodyParams,
    seed: u64,
) -> Result<Vec<Note>, HarmonyError> {
    if !p.enabled || chords.is_empty() {
        return Ok(Vec::new());
    }
    if !(0.0..=1.0).contains(&p.density) || !p.density.is_finite() {
        return Err(HarmonyError::invalid_argument(
            "countermelody density must be within 0.0..=1.0",
        ));
    }
    let instrument = kb
        .instrument_profile("counterlead")
        .or_else(|| kb.instrument_profile("melody_lead"));
    let (low, high) = match instrument {
        Some(i) => (i.comfortable_range.low_midi, i.comfortable_range.high_midi),
        None => (48, 79),
    };
    let start = chords[0].onset;
    let end = chords[chords.len() - 1].end();
    let tm = &melody.time_map;
    let beat = tm.meter_at(start).beat_unit_qn();
    if !beat.is_positive() {
        return Ok(Vec::new());
    }

    let profile_density = prof
        .field_f64("countermelody_density")
        .unwrap_or(0.4)
        .clamp(0.0, 1.0);
    let density = (0.5 * profile_density + 0.5 * p.density).clamp(0.0, 1.0);

    let mut grid = tm.beat_grid(start, end, beat);
    // A melody that already sounds on every beat leaves no complementary
    // rhythm on the beat grid, so the countermelody moves to the offbeats
    // rather than doubling it.
    let coincident = grid
        .iter()
        .filter(|qn| melody.notes.iter().any(|n| n.onset == **qn))
        .count();
    if !grid.is_empty() && coincident * 10 >= grid.len() * 7 {
        let half = beat.scale(1, 2);
        grid = grid
            .into_iter()
            .map(|qn| qn + half)
            .filter(|qn| *qn < end)
            .collect();
    }
    let mut ranked: Vec<(f64, usize, BeatTime)> = Vec::new();
    for (i, qn) in grid.iter().enumerate() {
        if *qn >= end {
            continue;
        }
        ranked.push((slot_appetite(melody, an, *qn, tm), i, *qn));
    }
    // Deterministic order: appetite first, then position.
    ranked.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    let wanted = ((ranked.len() as f64) * density).round() as usize;
    let wanted = wanted.clamp(usize::from(!ranked.is_empty()), ranked.len());
    let mut chosen: Vec<BeatTime> = ranked.iter().take(wanted).map(|(_, _, qn)| *qn).collect();
    chosen.sort();
    chosen.dedup();

    let mut rng = DetRng::new(seed).derive("countermelody");
    let mut notes: Vec<Note> = Vec::new();
    let mut previous: Option<i32> = None;
    for (index, qn) in chosen.iter().enumerate() {
        let Some(chord) = chords.iter().find(|c| c.contains(*qn)) else {
            continue;
        };
        let next = chosen.get(index + 1).copied().unwrap_or(end);
        let duration = (next.min(chord.end()).min(end)) - *qn;
        if !duration.is_positive() {
            continue;
        }
        let melody_here = sounding_midi(melody, *qn);
        let direction = melody_direction(melody, *qn);
        let midi = choose_pitch(
            chord,
            melody_here,
            previous,
            direction,
            low,
            high,
            p.role,
            &mut rng,
        );
        previous = Some(midi);
        let mut note = Note::new(
            COUNTER_ID_BASE + notes.len() as NoteId,
            crate::bass::spell_in(midi, &chord.spec),
            *qn,
            duration,
        );
        note.midi = midi;
        note.velocity = COUNTER_VELOCITY;
        note.role = NoteRole::Counter;
        note.voice = VoiceId(1);
        note.confidence = chord.confidence;
        notes.push(note);
    }

    thin_doubled_onsets(&mut notes, melody);
    crate::bass::enforce_monophony(&mut notes);
    for (i, note) in notes.iter_mut().enumerate() {
        note.id = COUNTER_ID_BASE + i as NoteId;
    }
    for note in &notes {
        note.validate()
            .map_err(|e| HarmonyError::new(e.code.clone(), e.message.clone()))?;
    }
    Ok(notes)
}

/// How much the countermelody wants to sound at a position.
///
/// Rests come first, then sustained melody notes, then metrically weak
/// positions. A melody onset is the least attractive place to add another one.
fn slot_appetite(melody: &NoteSet, an: &Analysis, qn: BeatTime, tm: &TimeMap) -> f64 {
    let has_onset = melody.notes.iter().any(|n| n.onset == qn);
    let sustaining = melody
        .notes
        .iter()
        .any(|n| n.onset < qn && n.onset + n.duration > qn);
    let in_gap = an.phrases.gaps.iter().any(|(a, b)| *a <= qn && qn < *b);
    let mut score = 0.0;
    if in_gap {
        score += 3.0;
    }
    if !has_onset && !sustaining {
        score += 2.5;
    } else if sustaining {
        score += 1.5;
    }
    if has_onset {
        score -= 2.0;
    }
    // A countermelody that only ever moves on weak beats sounds like an echo,
    // so metric weight is a mild tiebreak rather than a filter.
    score += 0.4 * tm.metric_weight(qn);
    score
}

/// The melody pitch sounding at a position, if any.
fn sounding_midi(melody: &NoteSet, qn: BeatTime) -> Option<i32> {
    melody
        .notes
        .iter()
        .find(|n| n.onset <= qn && n.onset + n.duration > qn)
        .map(|n| n.midi)
}

/// Which way the melody is moving around a position.
fn melody_direction(melody: &NoteSet, qn: BeatTime) -> i32 {
    let before = melody.notes.iter().rfind(|n| n.onset <= qn);
    let after = melody.notes.iter().find(|n| n.onset > qn);
    match (before, after) {
        (Some(a), Some(b)) => (b.midi - a.midi).signum(),
        _ => 0,
    }
}

/// Chooses the countermelody pitch for one position.
#[allow(clippy::too_many_arguments)] // Each argument is an independent musical input.
fn choose_pitch(
    chord: &ChordEvent,
    melody: Option<i32>,
    previous: Option<i32>,
    melody_direction: i32,
    low: i32,
    high: i32,
    role: ArrangementRole,
    rng: &mut DetRng,
) -> i32 {
    let above = matches!(role, ArrangementRole::Ornament | ArrangementRole::EarCandy);
    let ceiling = match melody {
        Some(m) if !above => (m - MIN_REGISTER_SEPARATION).min(high),
        _ => high,
    };
    let floor = match melody {
        Some(m) if above => (m + MIN_REGISTER_SEPARATION).max(low),
        _ => low,
    };
    let anchor = previous.unwrap_or((floor + ceiling) / 2);
    // The register separation is a preference; a chord tone is not. When the
    // window is too tight to hold one, the window gives way rather than the
    // harmony.
    for (floor, ceiling) in [(floor, ceiling), (low, high)] {
        if let Some(midi) = best_chord_tone(
            chord,
            previous,
            melody_direction,
            anchor,
            floor,
            ceiling,
            rng,
        ) {
            return midi;
        }
    }
    anchor.clamp(low.max(0), high.min(127))
}

/// The best-sounding chord tone inside a register window, if there is one.
#[allow(clippy::too_many_arguments)] // Each argument is an independent musical input.
fn best_chord_tone(
    chord: &ChordEvent,
    previous: Option<i32>,
    melody_direction: i32,
    anchor: i32,
    floor: i32,
    ceiling: i32,
    rng: &mut DetRng,
) -> Option<i32> {
    if ceiling <= floor {
        return None;
    }
    let mut best: Option<(f64, i32)> = None;
    for (_, (letter, accidental)) in chord.spec.chord_tones() {
        let pc = (letter.natural_pc() + i32::from(accidental.0)).rem_euclid(12);
        for octave in -1..=1 {
            let midi =
                crate::bass::place(pc, anchor, floor.max(0), ceiling.max(floor + 1).min(127))
                    + octave * 12;
            if midi < floor || midi > ceiling || !(0..=127).contains(&midi) {
                continue;
            }
            let motion = previous.map(|p| midi - p).unwrap_or(0);
            let mut cost = (motion.abs() as f64) * 0.4;
            if melody_direction != 0 && motion != 0 && motion.signum() == melody_direction {
                // Similar motion is allowed but never preferred.
                cost += 2.0;
            }
            if motion == 0 {
                cost += 0.5;
            }
            if motion.abs() > 9 {
                cost += 3.0;
            }
            cost += rng.next_f64() * 1e-6;
            let better = match best {
                None => true,
                Some((c, _)) => cost < c,
            };
            if better {
                best = Some((cost, midi));
            }
        }
    }
    best.map(|(_, m)| m)
}

/// Removes countermelody onsets that merely double the melody's.
///
/// Some coincidence is normal and musical; matching every melody onset is the
/// note-for-note doubling the brief forbids by default.
fn thin_doubled_onsets(notes: &mut Vec<Note>, melody: &NoteSet) {
    if notes.is_empty() {
        return;
    }
    let doubled: Vec<usize> = notes
        .iter()
        .enumerate()
        .filter(|(_, n)| melody.notes.iter().any(|m| m.onset == n.onset))
        .map(|(i, _)| i)
        .collect();
    if doubled.len() * 5 <= notes.len() * 2 {
        return;
    }
    // Keep only enough doubled onsets that the line still has shape.
    let keep = notes.len() * 2 / 5;
    let drop: Vec<usize> = doubled
        .iter()
        .enumerate()
        .filter(|(i, _)| *i + keep < doubled.len())
        .map(|(_, index)| *index)
        .collect();
    let mut kept = Vec::with_capacity(notes.len());
    for (i, note) in notes.drain(..).enumerate() {
        if !drop.contains(&i) {
            kept.push(note);
        }
    }
    *notes = kept;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidates::build_pools;
    use crate::params::{CancelFlag, SearchConfig};
    use crate::search::search_in;
    use crate::testing;

    fn setup(profile: &str, density: f64) -> (testing::Harness, Vec<ChordEvent>, Vec<Note>) {
        let h = testing::harness("melodies/eight_bar_c_major", profile);
        let (chords, notes) = {
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
            let chords = paths[0].chords.clone();
            let p = CountermelodyParams {
                enabled: true,
                density,
                role: ArrangementRole::Counterlead,
            };
            let notes = generate_countermelody(
                h.kb,
                &h.profile,
                &h.analysis.extraction.melody,
                &chords,
                &h.analysis,
                &p,
                11,
            )
            .expect("countermelody");
            (chords, notes)
        };
        (h, chords, notes)
    }

    #[test]
    fn disabled_by_default() {
        let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
        let notes = generate_countermelody(
            h.kb,
            &h.profile,
            &h.analysis.extraction.melody,
            &[],
            &h.analysis,
            &CountermelodyParams::default(),
            0,
        )
        .expect("ok");
        assert!(notes.is_empty());
    }

    #[test]
    fn line_is_monophonic_and_valid() {
        let (_, chords, notes) = setup("jazz_standard", 0.6);
        assert!(!notes.is_empty());
        let start = chords[0].onset;
        let end = chords[chords.len() - 1].end();
        for note in &notes {
            assert!(note.duration.is_positive());
            assert!((0..=127).contains(&note.midi));
            assert!(note.onset >= start && note.end() <= end);
            assert_eq!(note.role, NoteRole::Counter);
        }
        for pair in notes.windows(2) {
            assert!(pair[0].end() <= pair[1].onset, "countermelody overlaps");
        }
    }

    #[test]
    fn does_not_duplicate_every_melody_onset() {
        let (h, _, notes) = setup("jazz_standard", 0.8);
        let melody = &h.analysis.extraction.melody;
        let doubled = notes
            .iter()
            .filter(|n| melody.notes.iter().any(|m| m.onset == n.onset))
            .count();
        assert!(
            (doubled as f64) < 0.6 * notes.len() as f64,
            "{doubled} of {} onsets double the melody",
            notes.len()
        );
    }

    #[test]
    fn stays_below_the_melody() {
        let (h, _, notes) = setup("common_practice", 0.7);
        let melody = &h.analysis.extraction.melody;
        for note in &notes {
            if let Some(m) = sounding_midi(melody, note.onset) {
                assert!(
                    note.midi <= m - MIN_REGISTER_SEPARATION,
                    "countermelody {} crowds the melody {m}",
                    note.midi
                );
            }
        }
    }

    #[test]
    fn density_changes_note_count() {
        let (_, _, sparse) = setup("cinematic", 0.1);
        let (_, _, dense) = setup("cinematic", 1.0);
        assert!(
            dense.len() > sparse.len(),
            "dense {} should exceed sparse {}",
            dense.len(),
            sparse.len()
        );
    }

    #[test]
    fn generation_is_deterministic() {
        let (_, _, a) = setup("neo_soul_rnb", 0.5);
        let (_, _, b) = setup("neo_soul_rnb", 0.5);
        let ja: Vec<(i32, String)> = a.iter().map(|n| (n.midi, n.onset.to_display())).collect();
        let jb: Vec<(i32, String)> = b.iter().map(|n| (n.midi, n.onset.to_display())).collect();
        assert_eq!(ja, jb);
    }

    #[test]
    fn bad_density_is_rejected() {
        let h = testing::harness("melodies/eight_bar_c_major", "jazz_standard");
        let chords = vec![ChordEvent::new(
            0,
            symbol::parse("C").unwrap(),
            BeatTime::ZERO,
            BeatTime::from_quarters(4),
        )];
        let p = CountermelodyParams {
            enabled: true,
            density: 2.0,
            role: ArrangementRole::Counterlead,
        };
        let e = generate_countermelody(
            h.kb,
            &h.profile,
            &h.analysis.extraction.melody,
            &chords,
            &h.analysis,
            &p,
            0,
        )
        .expect_err("bad density");
        assert_eq!(e.code, crate::error::INVALID_ARGUMENT);
    }

    #[test]
    fn notes_are_chord_tones() {
        let (_, chords, notes) = setup("common_practice", 0.6);
        for note in &notes {
            let Some(chord) = chords.iter().find(|c| c.contains(note.onset)) else {
                continue;
            };
            assert!(
                chord
                    .spec
                    .pitch_classes()
                    .contains(&note.midi.rem_euclid(12)),
                "{} is not a tone of {}",
                note.midi,
                chord.spec.render_ascii()
            );
        }
    }
}
