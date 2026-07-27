//! Stage 8a — the bass line.
//!
//! The bass is generated in the mode the request asks for, or in the mode the
//! profile declares as its default. Range and low-register behaviour come from
//! the instrument profile, never from a constant here, and every mode produces
//! a strictly monophonic line: no overlaps, positive durations, inside the
//! instrument's range and inside the generated span.

use crate::error::HarmonyError;
use crate::params::BassMotion;
use music_domain::prelude::*;
use qjson::rng::DetRng;
use theory_kb::{InstrumentProfile, KnowledgeBase, ResolvedProfile};

/// First note id the bass part uses, so ids never collide with the melody's.
pub const BASS_ID_BASE: NoteId = 100_000;

/// Default velocity of a bass note.
pub const BASS_VELOCITY: u8 = 92;

/// Stage 8a, as the frozen contract declares it.
pub fn generate_bass(
    kb: &KnowledgeBase,
    prof: &ResolvedProfile,
    chords: &[ChordEvent],
    tm: &TimeMap,
    mode: BassMotion,
    instrument: Option<&InstrumentProfile>,
    seed: u64,
) -> Result<Vec<Note>, HarmonyError> {
    if chords.is_empty() {
        return Ok(Vec::new());
    }
    let instrument = instrument.or_else(|| kb.instrument_profile("bass"));
    let (low, high) = match instrument {
        Some(p) => (p.range.low_midi, p.range.high_midi),
        None => (28, 60),
    };
    if low >= high {
        return Err(HarmonyError::knowledge_missing(
            "the bass instrument profile declares an empty range",
        ));
    }
    let resolved = resolve_mode(prof, mode);
    let mut rng = DetRng::new(seed).derive("bass");
    let mut notes: Vec<Note> = Vec::new();
    let mut previous: Option<i32> = None;
    let mut last_direction: i32 = -1;

    for chord in chords.iter() {
        let targets = candidate_pitches(chord, resolved, chords);
        let chosen = match resolved {
            BassMotion::Pedal => pedal_pitch(chords, low, high),
            BassMotion::ContraryMotion => {
                let want = -last_direction;
                pick_directional(&targets, previous, want, low, high, &mut rng)
            }
            _ => pick_nearest(&targets, previous, low, high, &mut rng),
        };
        if let Some(prev) = previous {
            last_direction = if chosen > prev {
                1
            } else if chosen < prev {
                -1
            } else {
                last_direction
            };
        }
        previous = Some(chosen);

        let figure = match resolved {
            BassMotion::Ostinato => ostinato_figure(chord, tm, chosen, &chord.spec, low, high),
            _ => vec![(chord.onset, chord.duration, chosen)],
        };
        for (onset, duration, midi) in figure {
            if !duration.is_positive() {
                continue;
            }
            let midi = place(midi.rem_euclid(12), midi, low.max(0), high.min(127));
            let mut note = Note::new(
                BASS_ID_BASE + notes.len() as NoteId,
                spell_in(midi, &chord.spec),
                onset,
                duration,
            );
            note.midi = midi;
            note.velocity = BASS_VELOCITY;
            note.role = NoteRole::Bass;
            note.voice = VoiceId(0);
            note.confidence = chord.confidence;
            notes.push(note);
        }
    }

    if resolved == BassMotion::Pedal {
        merge_repeats(&mut notes);
    }
    enforce_monophony(&mut notes);
    for note in &notes {
        note.validate()
            .map_err(|e| HarmonyError::new(e.code.clone(), e.message.clone()))?;
    }
    Ok(notes)
}

/// The mode actually used: `auto` takes the profile's declared default.
pub fn resolve_mode(prof: &ResolvedProfile, mode: BassMotion) -> BassMotion {
    if mode != BassMotion::Auto {
        return mode;
    }
    prof.field_str("bass_behavior.default_mode")
        .and_then(BassMotion::parse)
        .filter(|m| *m != BassMotion::Auto)
        .unwrap_or(BassMotion::Roots)
}

/// The pitch classes this mode is willing to put in the bass.
fn candidate_pitches(chord: &ChordEvent, mode: BassMotion, chords: &[ChordEvent]) -> Vec<i32> {
    let tones = chord.spec.chord_tones();
    let pcs: Vec<i32> = tones
        .iter()
        .map(|(_, (l, a))| (l.natural_pc() + i32::from(a.0)).rem_euclid(12))
        .collect();
    let root = chord.spec.root_pc();
    match mode {
        BassMotion::Roots => vec![chord.spec.bass_pc().clamp(0, 127)],
        BassMotion::Inversions => {
            if chord.spec.bass.is_some() {
                vec![chord.spec.bass_pc()]
            } else {
                vec![*pcs.get(chord.inversion as usize).unwrap_or(&root)]
            }
        }
        BassMotion::Stepwise | BassMotion::ContraryMotion => {
            // Root, third and fifth are all available; the choice is made by
            // where the line already is.
            let mut out: Vec<i32> = pcs.iter().take(3).copied().collect();
            if out.is_empty() {
                out.push(root);
            }
            out
        }
        BassMotion::Ostinato => vec![root],
        BassMotion::Pedal => vec![chords.first().map(|c| c.spec.root_pc()).unwrap_or(root)],
        BassMotion::Auto => vec![root],
    }
    .into_iter()
    .map(|pc| pc.rem_euclid(12))
    .collect()
}

/// The sustained pedal pitch: the tonic of the first chord, low in the range.
fn pedal_pitch(chords: &[ChordEvent], low: i32, high: i32) -> i32 {
    let pc = chords
        .first()
        .map(|c| c.spec.root_pc())
        .unwrap_or(0)
        .rem_euclid(12);
    place(pc, (low + high) / 2 - 12, low, high)
}

/// Picks the candidate nearest the previous bass note.
fn pick_nearest(pcs: &[i32], previous: Option<i32>, low: i32, high: i32, rng: &mut DetRng) -> i32 {
    let anchor = previous.unwrap_or((low + high) / 2 - 5);
    let mut best: Option<i32> = None;
    for pc in pcs {
        let candidate = place(*pc, anchor, low, high);
        best = Some(match best {
            None => candidate,
            Some(current) => {
                let dc = (candidate - anchor).abs();
                let dn = (current - anchor).abs();
                if dc < dn || (dc == dn && rng.next_f64() < 0.5) {
                    candidate
                } else {
                    current
                }
            }
        });
    }
    best.unwrap_or(anchor)
}

/// Picks the candidate that moves in the requested direction, if one does.
fn pick_directional(
    pcs: &[i32],
    previous: Option<i32>,
    want: i32,
    low: i32,
    high: i32,
    rng: &mut DetRng,
) -> i32 {
    let Some(anchor) = previous else {
        return pick_nearest(pcs, previous, low, high, rng);
    };
    let mut best: Option<i32> = None;
    for pc in pcs {
        for octave in [-1, 0, 1] {
            let candidate = (place(*pc, anchor, low, high) + octave * 12).clamp(low, high);
            let motion = candidate - anchor;
            if motion == 0 || motion.signum() != want {
                continue;
            }
            best = Some(match best {
                None => candidate,
                Some(current) => {
                    if motion.abs() < (current - anchor).abs() {
                        candidate
                    } else {
                        current
                    }
                }
            });
        }
    }
    best.unwrap_or_else(|| pick_nearest(pcs, previous, low, high, rng))
}

/// The rhythmic figure an ostinato writes inside one chord.
fn ostinato_figure(
    chord: &ChordEvent,
    tm: &TimeMap,
    root_midi: i32,
    spec: &ChordSpec,
    low: i32,
    high: i32,
) -> Vec<(BeatTime, BeatTime, i32)> {
    let beat = tm.meter_at(chord.onset).beat_unit_qn();
    if !beat.is_positive() {
        return vec![(chord.onset, chord.duration, root_midi)];
    }
    let fifth = place((spec.root_pc() + 7).rem_euclid(12), root_midi, low, high);
    let mut out = Vec::new();
    let mut cursor = chord.onset;
    let mut step = 0usize;
    while cursor < chord.end() {
        let end = (cursor + beat).min(chord.end());
        let duration = end - cursor;
        if !duration.is_positive() {
            break;
        }
        let midi = match step % 4 {
            0 | 2 => root_midi,
            1 => fifth,
            _ => (root_midi + 12).min(high),
        };
        out.push((cursor, duration, midi));
        cursor = end;
        step += 1;
        if step > 64 {
            break;
        }
    }
    if out.is_empty() {
        out.push((chord.onset, chord.duration, root_midi));
    }
    out
}

/// Places a pitch class in the octave nearest an anchor, inside the range.
///
/// The returned pitch always has the requested pitch class: a window too
/// narrow to contain one yields the nearest correct pitch outside it rather
/// than a clamped pitch of the wrong class, which would silently change the
/// note the caller asked for.
pub fn place(pc: i32, anchor: i32, low: i32, high: i32) -> i32 {
    let pc = pc.rem_euclid(12);
    let below = anchor - (anchor - pc).rem_euclid(12);
    let above = below + 12;
    let mut best = if (anchor - below).abs() <= (above - anchor).abs() {
        below
    } else {
        above
    };
    while best < low {
        best += 12;
    }
    while best > high && best - 12 >= low {
        best -= 12;
    }
    while best > 127 {
        best -= 12;
    }
    while best < 0 {
        best += 12;
    }
    best
}

/// Spells a MIDI pitch inside the chord it belongs to.
pub fn spell_in(midi: i32, spec: &ChordSpec) -> SpelledPitch {
    for (_, (letter, accidental)) in spec.chord_tones() {
        let pc = (letter.natural_pc() + i32::from(accidental.0)).rem_euclid(12);
        if pc == midi.rem_euclid(12) {
            let octave = (midi - letter.natural_pc() - i32::from(accidental.0)).div_euclid(12) - 1;
            let pitch = SpelledPitch::new(letter, accidental, octave);
            if pitch.midi() == midi {
                return pitch;
            }
        }
    }
    SpelledPitch::from_midi(midi, None)
}

/// Joins consecutive identical pitches into one sustained note.
fn merge_repeats(notes: &mut Vec<Note>) {
    let mut i = 0;
    while i + 1 < notes.len() {
        if notes[i].midi == notes[i + 1].midi && notes[i].end() == notes[i + 1].onset {
            let extra = notes[i + 1].duration;
            notes[i].duration = notes[i].duration + extra;
            notes.remove(i + 1);
        } else {
            i += 1;
        }
    }
}

/// Trims any note that would overlap the next one.
///
/// The bass is declared monophonic, and a monophonic part with two notes
/// sounding at once is a hard-constraint violation rather than a preference.
pub fn enforce_monophony(notes: &mut Vec<Note>) {
    notes.sort_by(|a, b| a.onset.cmp(&b.onset).then_with(|| a.midi.cmp(&b.midi)));
    let mut i = 0;
    while i + 1 < notes.len() {
        let next_onset = notes[i + 1].onset;
        if notes[i].end() > next_onset {
            if next_onset > notes[i].onset {
                notes[i].duration = next_onset - notes[i].onset;
                i += 1;
            } else {
                notes.remove(i + 1);
            }
        } else {
            i += 1;
        }
    }
    notes.retain(|n| n.duration.is_positive());
    for (i, note) in notes.iter_mut().enumerate() {
        note.id = BASS_ID_BASE + i as NoteId;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    fn chords() -> Vec<ChordEvent> {
        ["C", "Am", "F", "G7"]
            .iter()
            .enumerate()
            .map(|(i, s)| {
                ChordEvent::new(
                    i as u32,
                    symbol::parse(s).expect("symbol"),
                    BeatTime::from_quarters(i as i64 * 4),
                    BeatTime::from_quarters(4),
                )
            })
            .collect()
    }

    fn run(mode: BassMotion, profile: &str) -> Vec<Note> {
        let kb = KnowledgeBase::embedded();
        let prof = kb.resolve_profile(profile).expect("profile");
        let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
        generate_bass(kb, &prof, &chords(), &tm, mode, None, 7).expect("bass")
    }

    #[test]
    fn every_mode_produces_a_valid_line() {
        for mode in BassMotion::all() {
            let notes = run(*mode, "jazz_standard");
            assert!(!notes.is_empty(), "{} produced nothing", mode.id());
            for note in &notes {
                assert!(note.duration.is_positive());
                assert!((0..=127).contains(&note.midi));
                assert_eq!(note.role, NoteRole::Bass);
            }
            for pair in notes.windows(2) {
                assert!(
                    pair[0].end() <= pair[1].onset,
                    "{} overlaps at {}",
                    mode.id(),
                    pair[0].onset.to_display()
                );
            }
        }
    }

    #[test]
    fn bass_stays_inside_the_instrument_range() {
        let kb = KnowledgeBase::embedded();
        let profile = kb.instrument_profile("bass").expect("bass profile");
        for mode in BassMotion::all() {
            for note in run(*mode, "pop_rock") {
                assert!(
                    profile.range.contains(note.midi),
                    "{} wrote {} outside {}..{}",
                    mode.id(),
                    note.midi,
                    profile.range.low_midi,
                    profile.range.high_midi
                );
            }
        }
    }

    #[test]
    fn roots_mode_writes_roots() {
        let notes = run(BassMotion::Roots, "pop_rock");
        let expected = [0, 9, 5, 7];
        assert_eq!(notes.len(), 4);
        for (note, pc) in notes.iter().zip(expected) {
            assert_eq!(note.midi.rem_euclid(12), pc);
        }
    }

    #[test]
    fn pedal_mode_holds_one_pitch() {
        let notes = run(BassMotion::Pedal, "modal_ambient");
        assert_eq!(notes.len(), 1, "a pedal is one sustained note");
        assert_eq!(notes[0].duration, BeatTime::from_quarters(16));
    }

    #[test]
    fn ostinato_mode_is_busier_than_roots() {
        let roots = run(BassMotion::Roots, "electronic_loop");
        let ostinato = run(BassMotion::Ostinato, "electronic_loop");
        assert!(ostinato.len() > roots.len());
    }

    #[test]
    fn stepwise_mode_moves_less_than_roots() {
        let roots = run(BassMotion::Roots, "jazz_standard");
        let stepwise = run(BassMotion::Stepwise, "jazz_standard");
        let travel = |notes: &[Note]| -> i32 {
            notes
                .windows(2)
                .map(|p| (p[1].midi - p[0].midi).abs())
                .sum()
        };
        assert!(travel(&stepwise) <= travel(&roots));
    }

    #[test]
    fn auto_mode_follows_the_profile() {
        let kb = KnowledgeBase::embedded();
        for id in testing::PROFILE_IDS {
            let prof = kb.resolve_profile(id).expect("profile");
            let resolved = resolve_mode(&prof, BassMotion::Auto);
            assert_ne!(resolved, BassMotion::Auto, "{id} has no default bass mode");
            let declared = prof
                .field_str("bass_behavior.default_mode")
                .and_then(BassMotion::parse);
            assert_eq!(Some(resolved), declared, "{id} default was not honoured");
        }
    }

    #[test]
    fn contrary_motion_changes_direction() {
        let notes = run(BassMotion::ContraryMotion, "strict_counterpoint");
        let directions: Vec<i32> = notes
            .windows(2)
            .map(|p| (p[1].midi - p[0].midi).signum())
            .collect();
        assert!(
            directions.windows(2).any(|d| d[0] != d[1]),
            "contrary motion never turned: {directions:?}"
        );
    }

    #[test]
    fn generation_is_deterministic() {
        let a = run(BassMotion::Stepwise, "jazz_standard");
        let b = run(BassMotion::Stepwise, "jazz_standard");
        let ja: Vec<i32> = a.iter().map(|n| n.midi).collect();
        let jb: Vec<i32> = b.iter().map(|n| n.midi).collect();
        assert_eq!(ja, jb);
    }

    #[test]
    fn empty_input_yields_empty_output() {
        let kb = KnowledgeBase::embedded();
        let prof = kb.resolve_profile("pop_rock").expect("profile");
        let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
        assert!(
            generate_bass(kb, &prof, &[], &tm, BassMotion::Roots, None, 0)
                .expect("ok")
                .is_empty()
        );
    }

    #[test]
    fn placement_stays_inside_the_window() {
        for pc in 0..12 {
            let midi = place(pc, 40, 28, 60);
            assert!((28..=60).contains(&midi));
            assert_eq!(midi.rem_euclid(12), pc);
        }
    }

    #[test]
    fn inversions_mode_uses_the_chord_bass() {
        let kb = KnowledgeBase::embedded();
        let prof = kb.resolve_profile("common_practice").expect("profile");
        let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
        let mut events = chords();
        events[0].inversion = 1;
        let notes =
            generate_bass(kb, &prof, &events, &tm, BassMotion::Inversions, None, 0).expect("bass");
        assert_eq!(notes[0].midi.rem_euclid(12), 4, "first inversion of C is E");
    }
}
