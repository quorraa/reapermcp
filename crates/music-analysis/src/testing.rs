//! Note-set builders shared by this crate's unit tests.
//!
//! Not compiled into the library; integration tests carry their own copy so the
//! public API stays exactly what the contract specifies.

use music_domain::prelude::*;

/// Builds a 4/4 note set from `(pitch, onset, duration)` triples.
///
/// Ids are assigned in argument order, every note is selected, and velocity is
/// the fixture-format default of 96.
pub(crate) fn note_set(spec: &[(&str, &str, &str)]) -> NoteSet {
    note_set_in(spec, TimeSignature::new(4, 4))
}

/// [`note_set`] in an explicit meter.
pub(crate) fn note_set_in(spec: &[(&str, &str, &str)], sig: TimeSignature) -> NoteSet {
    let notes = spec
        .iter()
        .enumerate()
        .map(|(i, (p, on, dur))| {
            let mut n = Note::new(
                i as NoteId,
                SpelledPitch::parse(p).expect("a spelled pitch"),
                BeatTime::parse(on).expect("a rational onset"),
                BeatTime::parse(dur).expect("a rational duration"),
            );
            n.selected = true;
            n
        })
        .collect();
    NoteSet::sorted(notes, TimeMap::constant(120.0, sig))
}

/// A note set whose notes carry explicit selection flags.
pub(crate) fn note_set_selected(spec: &[(&str, &str, &str, bool)]) -> NoteSet {
    let notes = spec
        .iter()
        .enumerate()
        .map(|(i, (p, on, dur, sel))| {
            let mut n = Note::new(
                i as NoteId,
                SpelledPitch::parse(p).expect("a spelled pitch"),
                BeatTime::parse(on).expect("a rational onset"),
                BeatTime::parse(dur).expect("a rational duration"),
            );
            n.selected = *sel;
            n
        })
        .collect();
    NoteSet::sorted(notes, TimeMap::constant(120.0, TimeSignature::new(4, 4)))
}

/// Chord events parsed from `(symbol, onset, duration)` triples.
pub(crate) fn chords(spec: &[(&str, &str, &str)]) -> Vec<ChordEvent> {
    spec.iter()
        .enumerate()
        .map(|(i, (sym, on, dur))| {
            let mut e = ChordEvent::new(
                i as u32,
                symbol::parse(sym).expect("a chord symbol"),
                BeatTime::parse(on).expect("a rational onset"),
                BeatTime::parse(dur).expect("a rational duration"),
            );
            e.original_symbol = Some((*sym).to_string());
            e
        })
        .collect()
}
