//! Deterministic voice separation.
//!
//! The problem: a MIDI take is a bag of overlapping notes, and harmonisation
//! needs *lines*. The classical answer is a cost-based assignment — every note
//! joins the voice it continues most cheaply — and the two costs that matter
//! musically are **pitch proximity** (a line does not jump between registers)
//! and **temporal continuity** (a line does not leave a two-bar hole and come
//! back).
//!
//! Everything here is deterministic. Notes are visited in `(onset, midi, id)`
//! order, ties in the cost function break on the lower voice index, and the
//! finished voices are re-ordered by register — top line first — so
//! `separate_voices(..)[0]` is always the same line for the same input.

use crate::util::cmp_f64;
use music_domain::prelude::*;

/// The number of simultaneous lines the separator will invent before it starts
/// forcing notes into an existing voice.
///
/// Eight is well past anything the harmoniser can use and comfortably past
/// keyboard writing; beyond it the material is a texture, not a set of lines.
pub const DEFAULT_MAX_VOICES: usize = 8;

/// Semitones of register distance that cost as much as one quarter note of
/// silence. Lower values make the separator favour continuity over register.
const SEMITONES_PER_QUARTER: f64 = 2.0;

/// Weight applied to the distance from a voice's *running mean* pitch, on top
/// of the distance from its last note. This is what keeps an inner voice from
/// being captured by a passing gesture in another register.
const MEAN_PITCH_WEIGHT: f64 = 0.35;

/// The penalty for putting a note into a voice that is still sounding.
///
/// Deliberately large: a voice is a monophonic line by definition, so this only
/// happens when every voice is busy and `max_voices` forbids opening another.
const OVERLAP_PENALTY: f64 = 48.0;

/// A voice under construction.
#[derive(Clone, Debug)]
struct Line {
    ids: Vec<NoteId>,
    last_end: BeatTime,
    last_midi: i32,
    sum_midi: i64,
    count: i64,
    first_onset: BeatTime,
}

impl Line {
    fn mean_midi(&self) -> f64 {
        if self.count == 0 {
            return self.last_midi as f64;
        }
        self.sum_midi as f64 / self.count as f64
    }
}

/// Splits `src` into at most `max_voices` monophonic lines, returning the note
/// ids of each line in time order.
///
/// The returned lines are ordered by register, highest first, so index `0` is
/// the top line. Muted notes participate: they occupy their voice exactly as a
/// sounding note would, because a muted note is still an edit the user made.
///
/// `max_voices == 0` yields no lines at all; an empty input yields no lines.
pub fn separate_voices(src: &NoteSet, max_voices: usize) -> Vec<Vec<NoteId>> {
    if max_voices == 0 || src.notes.is_empty() {
        return Vec::new();
    }

    let mut order: Vec<&Note> = src.notes.iter().collect();
    order.sort_by(|a, b| {
        a.onset
            .cmp(&b.onset)
            .then(a.midi.cmp(&b.midi))
            .then(a.id.cmp(&b.id))
    });

    let mut lines: Vec<Line> = Vec::new();
    for n in order {
        let mut best: Option<(usize, f64)> = None;
        for (i, l) in lines.iter().enumerate() {
            let cost = assignment_cost(l, n);
            let better = match best {
                None => true,
                Some((_, c)) => cost < c,
            };
            if better {
                best = Some((i, cost));
            }
        }

        // A free voice whose continuation is expensive is still cheaper than
        // inventing a line, until the cost passes the "new voice" threshold.
        let open_new = match best {
            None => true,
            Some((_, cost)) => lines.len() < max_voices && cost > new_voice_cost(),
        };

        if open_new && lines.len() < max_voices {
            lines.push(Line {
                ids: vec![n.id],
                last_end: n.end(),
                last_midi: n.midi,
                sum_midi: n.midi as i64,
                count: 1,
                first_onset: n.onset,
            });
            continue;
        }

        let idx = best.map(|(i, _)| i).unwrap_or(0);
        let l = &mut lines[idx];
        l.ids.push(n.id);
        l.last_end = l.last_end.max(n.end());
        l.last_midi = n.midi;
        l.sum_midi += n.midi as i64;
        l.count += 1;
    }

    // Register order: top line first, then by entry, then by first id, so the
    // ordering never depends on the order the lines happened to be created in.
    lines.sort_by(|a, b| {
        cmp_f64(b.mean_midi(), a.mean_midi())
            .then(a.first_onset.cmp(&b.first_onset))
            .then(a.ids.first().cmp(&b.ids.first()))
    });
    lines.into_iter().map(|l| l.ids).collect()
}

/// Cost of appending `n` to `l`.
fn assignment_cost(l: &Line, n: &Note) -> f64 {
    let leap = (n.midi - l.last_midi).abs() as f64;
    let drift = (n.midi as f64 - l.mean_midi()).abs() * MEAN_PITCH_WEIGHT;
    let gap = (n.onset - l.last_end).as_f64();
    let silence = if gap > 0.0 {
        gap * SEMITONES_PER_QUARTER
    } else {
        0.0
    };
    let overlap = if n.onset < l.last_end {
        OVERLAP_PENALTY + (l.last_end - n.onset).as_f64()
    } else {
        0.0
    };
    leap + drift + silence + overlap
}

/// The cost above which opening a new voice is preferable to continuing an
/// existing one — a major seventh of register jump, or the equivalent in
/// silence.
fn new_voice_cost() -> f64 {
    11.0
}

/// Assigns [`VoiceId`]s to a copy of `src` using [`separate_voices`].
///
/// Notes keep their own ids; only [`Note::voice`] changes. Handy for callers
/// that want the separation as data on the notes rather than as an index.
pub fn assign_voices(src: &NoteSet, max_voices: usize) -> NoteSet {
    let lines = separate_voices(src, max_voices);
    let mut out = src.clone();
    for (v, ids) in lines.iter().enumerate() {
        for id in ids {
            if let Some(n) = out.notes.iter_mut().find(|n| n.id == *id) {
                n.voice = VoiceId(v as u16);
            }
        }
    }
    out
}

/// True when `src` never sounds two notes at once, i.e. it is a single line
/// already and needs no separation.
pub fn is_single_line(src: &NoteSet) -> bool {
    src.is_monophonic()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::note_set;

    #[test]
    fn a_monophonic_line_stays_one_voice() {
        let ns = note_set(&[("C4", "0", "1"), ("D4", "1", "1"), ("E4", "2", "1")]);
        let v = separate_voices(&ns, 4);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0], vec![0, 1, 2]);
    }

    #[test]
    fn two_registers_become_two_voices_top_line_first() {
        let ns = note_set(&[
            ("E5", "0", "1"),
            ("C4", "0", "2"),
            ("F5", "1", "1"),
            ("B3", "2", "2"),
            ("G5", "2", "2"),
        ]);
        let v = separate_voices(&ns, 4);
        assert_eq!(v.len(), 2, "{v:?}");
        let top: Vec<i32> = v[0]
            .iter()
            .map(|id| ns.notes.iter().find(|n| n.id == *id).unwrap().midi)
            .collect();
        assert!(top.iter().all(|m| *m >= 72), "top line is the upper one");
    }

    #[test]
    fn separation_is_deterministic_across_runs() {
        let ns = note_set(&[
            ("E5", "0", "1"),
            ("C4", "0", "2"),
            ("F5", "1", "1"),
            ("B3", "2", "2"),
            ("G5", "2", "2"),
            ("A3", "4", "2"),
            ("F5", "4", "1"),
        ]);
        let a = separate_voices(&ns, 6);
        let b = separate_voices(&ns, 6);
        assert_eq!(a, b);
    }

    #[test]
    fn input_order_does_not_change_the_result() {
        let forward = note_set(&[
            ("E5", "0", "1"),
            ("C4", "0", "2"),
            ("F5", "1", "1"),
            ("B3", "2", "2"),
        ]);
        let mut shuffled = forward.clone();
        shuffled.notes.reverse();
        assert_eq!(
            separate_voices(&forward, 4).len(),
            separate_voices(&shuffled, 4).len()
        );
    }

    #[test]
    fn max_voices_is_respected() {
        let ns = note_set(&[
            ("C3", "0", "4"),
            ("E4", "0", "4"),
            ("G5", "0", "4"),
            ("B6", "0", "4"),
        ]);
        assert_eq!(separate_voices(&ns, 2).len(), 2);
        assert_eq!(separate_voices(&ns, 0).len(), 0);
    }

    #[test]
    fn every_note_lands_in_exactly_one_voice() {
        let ns = note_set(&[
            ("C3", "0", "4"),
            ("E4", "0", "4"),
            ("G5", "0", "4"),
            ("B6", "0", "4"),
            ("C3", "4", "4"),
        ]);
        let v = separate_voices(&ns, 3);
        let mut all: Vec<NoteId> = v.iter().flatten().copied().collect();
        all.sort_unstable();
        assert_eq!(all, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn assign_voices_writes_voice_ids() {
        let ns = note_set(&[("E5", "0", "1"), ("C4", "0", "2")]);
        let out = assign_voices(&ns, 4);
        assert!(out.notes.iter().all(|n| !n.voice.is_unassigned()));
    }

    #[test]
    fn single_line_detection_matches_the_domain_model() {
        let mono = note_set(&[("C4", "0", "1"), ("D4", "1", "1")]);
        let poly = note_set(&[("C4", "0", "2"), ("E4", "1", "1")]);
        assert!(is_single_line(&mono));
        assert!(!is_single_line(&poly));
    }

    #[test]
    fn an_empty_set_has_no_voices() {
        assert!(separate_voices(&NoteSet::default(), 4).is_empty());
    }
}
