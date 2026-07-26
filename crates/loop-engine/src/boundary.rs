//! Loop geometry: the span, the notes that touch its seam, and the single
//! mutating operation this crate exposes.
//!
//! Every measurement here is exact. Positions and durations are [`BeatTime`]
//! rationals and are compared with `==`, never with a float tolerance: a loop
//! whose length drifts by one tick drifts by one tick on *every* repeat, so the
//! comparison that decides whether a loop is the length it was asked to be has
//! to be exact arithmetic rather than an approximation.
//!
//! # Which notes are which
//!
//! The seam of a loop is one point heard twice: the material just before
//! [`LoopSpan::end`] is followed immediately by the material at
//! [`LoopSpan::start`]. Three note sets are distinguished, and the distinction
//! matters because two of them are faults and one is not:
//!
//! * a **crossing note** sounds on both sides of the seam. That is a neutral
//!   observation — it is what a pad, a drone or a tied note does.
//! * a **hanging note** is a crossing note at the loop *end* that nobody asked
//!   for. On the first repeat it is still sounding when the loop restarts, so
//!   it is audible as a stuck voice.
//! * a note **explicitly marked to carry** (see [`is_marked_carry`]) is a
//!   crossing note the caller asked for. It is reported as crossing and is
//!   never reported as hanging.

use crate::error::LoopError;
use music_domain::prelude::*;

/// The articulation [`apply_boundary_policy`] writes when it carries a note
/// across the loop boundary on purpose.
pub const CARRY_MARK: &str = "loop_carry";

/// The articulation written on the wrapped remainder of a note that
/// [`CarryPolicy::Split`] cut at the loop end.
pub const SPLIT_TAIL_MARK: &str = "loop_split_tail";

/// The articulation written on the re-attacked copy [`CarryPolicy::Rearticulate`]
/// places at the loop start.
pub const REARTICULATED_MARK: &str = "loop_rearticulated";

/// The articulation written on pickup material moved or duplicated into the
/// loop tail.
pub const WRAPPED_PICKUP_MARK: &str = "loop_wrapped_pickup";

/// Every articulation string this build reads as "the caller asked for this
/// note to sound across the loop boundary".
///
/// Both the verbose form the engine writes and the bare word a human is likely
/// to type in the REAPER note-properties dialog are accepted; the audit must
/// not call a deliberate decision a fault because of a spelling.
pub const CARRY_MARKS: &[&str] = &[CARRY_MARK, "carry", "tie", "let_ring"];

/// True when the note is explicitly marked to carry across the loop boundary.
///
/// This is the exception the `looping.no_hanging_note_past_loop_end` rule
/// names: overhanging is a fault *unless* it was asked for.
pub fn is_marked_carry(note: &Note) -> bool {
    match &note.articulation {
        Some(a) => CARRY_MARKS.iter().any(|m| *m == a.as_str()),
        None => false,
    }
}

/// True when the note can be heard at all.
///
/// A muted note is still in the take and still has an id, but it never sounds,
/// so it can neither hang nor cross.
fn sounds(note: &Note) -> bool {
    !note.muted && note.duration.is_positive()
}

/// A loop region and what the caller wants it to do.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LoopSpan {
    /// Exact loop start in quarter notes.
    pub start: BeatTime,
    /// Exact loop end in quarter notes.
    pub end: BeatTime,
    /// What the loop is for. This, not a universal preference for the tonic,
    /// decides what counts as a good wrap.
    pub intent: LoopIntent,
}

impl LoopSpan {
    /// Builds a span.
    pub fn new(start: BeatTime, end: BeatTime, intent: LoopIntent) -> LoopSpan {
        LoopSpan { start, end, intent }
    }

    /// Loop length in quarter notes, exactly.
    pub fn length(&self) -> BeatTime {
        self.end - self.start
    }

    /// True when the span encloses a positive amount of time.
    pub fn is_valid(&self) -> bool {
        self.end > self.start
    }

    /// `Err` when the span is empty or inverted.
    pub fn validate(&self) -> Result<(), LoopError> {
        if self.is_valid() {
            Ok(())
        } else {
            Err(LoopError::invalid_loop_span(format!(
                "loop end {} is not after loop start {}",
                self.end.to_display(),
                self.start.to_display()
            )))
        }
    }

    /// True when `qn` falls inside `[start, end)`.
    pub fn contains(&self, qn: BeatTime) -> bool {
        qn >= self.start && qn < self.end
    }

    /// Folds a position into the span, as a repeat of the loop would.
    ///
    /// Returns the input unchanged for a degenerate span rather than dividing
    /// by zero.
    pub fn wrap(&self, qn: BeatTime) -> BeatTime {
        let len = self.length();
        if !len.is_positive() {
            return qn;
        }
        self.start + (qn - self.start).rem_euclid(len)
    }

    /// JSON form.
    pub fn to_json(&self) -> qjson::Json {
        qjson::json_obj! {
            "start_qn" => self.start.to_json(),
            "end_qn" => self.end.to_json(),
            "intent" => self.intent.id(),
        }
    }
}

/// What to do with material that does not fit inside the loop.
///
/// The policy is always the caller's choice. [`crate::audit`] never mutates
/// anything; [`apply_boundary_policy`] is the only function in this crate that
/// writes to a note.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum CarryPolicy {
    /// Cut a note at the loop end and wrap its remainder to the loop start, and
    /// move pickup material into the loop tail. Nothing sounds outside the span
    /// afterwards.
    #[default]
    Split,
    /// Leave the overhang where it is and mark it, so the audit reports it as a
    /// deliberate crossing rather than as a hanging note.
    Carry,
    /// Clip everything to the span and drop what falls entirely outside it.
    Truncate,
    /// Cut a note at the loop end and re-attack it at the loop start, and
    /// duplicate pickup material into the loop tail so the anacrusis exists on
    /// the first pass *and* on every repeat.
    Rearticulate,
}

impl CarryPolicy {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            CarryPolicy::Split => "split",
            CarryPolicy::Carry => "carry",
            CarryPolicy::Truncate => "truncate",
            CarryPolicy::Rearticulate => "rearticulate",
        }
    }

    /// Parses the identifier produced by [`CarryPolicy::id`].
    pub fn parse(s: &str) -> Option<CarryPolicy> {
        Some(match s {
            "split" => CarryPolicy::Split,
            "carry" => CarryPolicy::Carry,
            "truncate" => CarryPolicy::Truncate,
            "rearticulate" => CarryPolicy::Rearticulate,
            _ => return None,
        })
    }

    /// Every variant, in declaration order.
    pub fn all() -> &'static [CarryPolicy] {
        &[
            CarryPolicy::Split,
            CarryPolicy::Carry,
            CarryPolicy::Truncate,
            CarryPolicy::Rearticulate,
        ]
    }

    /// The `note_carry_policy` fact value this policy asserts.
    ///
    /// The knowledge base only distinguishes an *explicit* policy — `"carry"`
    /// or `"split"` — from the default, because those two are the ones that
    /// make an overhang intentional.
    pub fn fact_value(self) -> &'static str {
        match self {
            CarryPolicy::Carry => "carry",
            CarryPolicy::Split | CarryPolicy::Rearticulate => "split",
            CarryPolicy::Truncate => "default",
        }
    }
}

/// Notes still sounding after the loop end that nobody asked to keep sounding.
///
/// Muted notes and notes carrying an explicit carry mark are excluded: the
/// first cannot be heard and the second is a decision, not a defect. The result
/// is sorted by note id so the report is byte-stable.
pub fn hanging_notes(notes: &NoteSet, span: &LoopSpan) -> Vec<NoteId> {
    let mut out: Vec<NoteId> = notes
        .notes
        .iter()
        .filter(|n| sounds(n) && !is_marked_carry(n))
        .filter(|n| n.onset < span.end && n.end() > span.end)
        .map(|n| n.id)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Notes sounding on both sides of the loop seam, whether or not that was
/// intended.
///
/// A note crosses the seam if it is still sounding at the loop end, or if it
/// began before the loop start and is still sounding when the loop starts.
pub fn crossing_notes(notes: &NoteSet, span: &LoopSpan) -> Vec<NoteId> {
    let mut out: Vec<NoteId> = notes
        .notes
        .iter()
        .filter(|n| sounds(n))
        .filter(|n| {
            (n.onset < span.end && n.end() > span.end)
                || (n.onset < span.start && n.end() > span.start)
        })
        .map(|n| n.id)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Notes marked to carry that actually reach across the seam.
pub fn carried_notes(notes: &NoteSet, span: &LoopSpan) -> Vec<NoteId> {
    let mut out: Vec<NoteId> = notes
        .notes
        .iter()
        .filter(|n| sounds(n) && is_marked_carry(n))
        .filter(|n| n.onset < span.end && n.end() > span.end)
        .map(|n| n.id)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// How much material sounds before the loop start.
///
/// Zero when there is no anacrusis. This is measured from the earliest onset
/// before the loop start, so a two-note pickup reports the whole upbeat rather
/// than only its last note.
pub fn pickup_length(notes: &NoteSet, span: &LoopSpan) -> BeatTime {
    let earliest = notes
        .notes
        .iter()
        .filter(|n| sounds(n) && n.onset < span.start)
        .map(|n| n.onset)
        .min();
    match earliest {
        Some(o) => span.start - o,
        None => BeatTime::ZERO,
    }
}

/// How much material sounds after the loop end.
///
/// Zero when nothing overhangs. A carried note counts: the tail is a
/// measurement of the material, not a verdict about it.
pub fn tail_length(notes: &NoteSet, span: &LoopSpan) -> BeatTime {
    let latest = notes
        .notes
        .iter()
        .filter(|n| sounds(n) && n.end() > span.end)
        .map(|n| n.end())
        .max();
    match latest {
        Some(e) => e - span.end,
        None => BeatTime::ZERO,
    }
}

/// The notes that begin the loop, i.e. the entry points at `span.start`.
pub fn entry_points(notes: &NoteSet, span: &LoopSpan) -> Vec<NoteId> {
    let mut out: Vec<NoteId> = notes
        .notes
        .iter()
        .filter(|n| sounds(n) && n.onset == span.start)
        .map(|n| n.id)
        .collect();
    out.sort_unstable();
    out
}

/// The notes that end the loop, i.e. the exit points at `span.end`.
pub fn exit_points(notes: &NoteSet, span: &LoopSpan) -> Vec<NoteId> {
    let mut out: Vec<NoteId> = notes
        .notes
        .iter()
        .filter(|n| sounds(n) && n.end() == span.end)
        .map(|n| n.id)
        .collect();
    out.sort_unstable();
    out
}

/// The span the material actually occupies once the loop region is included.
///
/// A loop whose notes fit exactly reports the requested length; a pickup or an
/// overhang makes it longer. Returned as an exact [`BeatTime`].
pub fn occupied_length(notes: &NoteSet, span: &LoopSpan) -> BeatTime {
    let mut lo = span.start;
    let mut hi = span.end;
    for n in notes.notes.iter().filter(|n| sounds(n)) {
        lo = lo.min(n.onset);
        hi = hi.max(n.end());
    }
    hi - lo
}

/// The next unused note id.
fn next_id(notes: &[Note]) -> NoteId {
    notes.iter().map(|n| n.id).max().map_or(0, |m| m + 1)
}

/// Rewrites the note list so the material obeys `policy` at the loop seam.
///
/// This is the only mutating function in the crate, and the policy is always
/// the caller's: the audit reports, it never silently repairs. The loop span is
/// never touched, so `span.length()` is bit-for-bit the same rational before
/// and after — the invariant `looping.exact_length_is_preserved` exists to
/// protect.
///
/// Afterwards the list is sorted by `(onset, midi, id)` so two runs over the
/// same input produce the same list in the same order.
pub fn apply_boundary_policy(notes: &mut Vec<Note>, span: &LoopSpan, policy: CarryPolicy) {
    if !span.is_valid() {
        return;
    }
    let length = span.length();
    let mut added: Vec<Note> = Vec::new();
    let mut id = next_id(notes);

    match policy {
        CarryPolicy::Carry => {
            for n in notes.iter_mut() {
                if sounds(n) && n.onset < span.end && n.end() > span.end && !is_marked_carry(n) {
                    n.articulation = Some(CARRY_MARK.to_string());
                }
            }
        }
        CarryPolicy::Truncate => {
            notes.retain(|n| n.end() > span.start && n.onset < span.end);
            for n in notes.iter_mut() {
                let start = n.onset.max(span.start);
                let end = n.end().min(span.end);
                n.onset = start;
                n.duration = end - start;
            }
            notes.retain(|n| n.duration.is_positive());
        }
        CarryPolicy::Split => {
            // The overhang becomes the head of the next repeat.
            for n in notes.iter_mut() {
                if sounds(n) && n.onset < span.end && n.end() > span.end {
                    let remainder = (n.end() - span.end).min(length);
                    n.duration = span.end - n.onset;
                    let mut tail = n.clone();
                    tail.id = id;
                    id += 1;
                    tail.onset = span.start;
                    tail.duration = remainder;
                    tail.articulation = Some(SPLIT_TAIL_MARK.to_string());
                    added.push(tail);
                }
            }
            // The anacrusis moves into the tail, where the repeat will play it.
            for n in notes.iter_mut() {
                if !sounds(n) || n.onset >= span.start {
                    continue;
                }
                if n.end() <= span.start {
                    n.onset = n.onset + length;
                    n.articulation = Some(WRAPPED_PICKUP_MARK.to_string());
                } else {
                    let head = span.start - n.onset;
                    let mut wrapped = n.clone();
                    wrapped.id = id;
                    id += 1;
                    wrapped.onset = span.end - head;
                    wrapped.duration = head;
                    wrapped.articulation = Some(WRAPPED_PICKUP_MARK.to_string());
                    added.push(wrapped);
                    n.duration = n.end() - span.start;
                    n.onset = span.start;
                }
            }
        }
        CarryPolicy::Rearticulate => {
            for n in notes.iter_mut() {
                if sounds(n) && n.onset < span.end && n.end() > span.end {
                    let original = n.duration;
                    n.duration = span.end - n.onset;
                    let mut head = n.clone();
                    head.id = id;
                    id += 1;
                    head.onset = span.start;
                    head.duration = original.min(length);
                    head.articulation = Some(REARTICULATED_MARK.to_string());
                    added.push(head);
                }
            }
            // Duplicated, not moved: the anacrusis is wanted on the first pass
            // as well as on every repeat.
            let pickups: Vec<Note> = notes
                .iter()
                .filter(|n| sounds(n) && n.onset < span.start)
                .cloned()
                .collect();
            for p in pickups {
                let mut copy = p.clone();
                copy.id = id;
                id += 1;
                copy.onset = p.onset + length;
                copy.duration = p.duration.min(length);
                copy.articulation = Some(WRAPPED_PICKUP_MARK.to_string());
                added.push(copy);
            }
        }
    }

    notes.extend(added);
    notes.retain(|n| n.duration.is_positive());
    notes.sort_by(|a, b| {
        a.onset
            .cmp(&b.onset)
            .then(a.midi.cmp(&b.midi))
            .then(a.id.cmp(&b.id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: NoteId, midi: i32, onset: (i64, i64), dur: (i64, i64)) -> Note {
        let mut n = Note::new(
            id,
            SpelledPitch::from_midi(midi, None),
            BeatTime::new(onset.0, onset.1),
            BeatTime::new(dur.0, dur.1),
        );
        n.midi = midi;
        n
    }

    fn set(notes: Vec<Note>) -> NoteSet {
        NoteSet::sorted(notes, TimeMap::constant(120.0, TimeSignature::new(4, 4)))
    }

    fn span() -> LoopSpan {
        LoopSpan::new(
            BeatTime::ZERO,
            BeatTime::from_quarters(16),
            LoopIntent::ClosedTonic,
        )
    }

    #[test]
    fn length_is_exact_rational_arithmetic() {
        let s = LoopSpan::new(
            BeatTime::new(1, 3),
            BeatTime::new(16, 3),
            LoopIntent::SeamlessColor,
        );
        assert_eq!(s.length(), BeatTime::new(5, 1));
    }

    #[test]
    fn an_inverted_span_is_rejected() {
        let s = LoopSpan::new(
            BeatTime::from_quarters(4),
            BeatTime::ZERO,
            LoopIntent::ClosedTonic,
        );
        assert!(!s.is_valid());
        assert_eq!(s.validate().unwrap_err().code, crate::error::INVALID_LOOP_SPAN);
    }

    #[test]
    fn wrap_folds_into_the_span() {
        let s = span();
        assert_eq!(s.wrap(BeatTime::from_quarters(17)), BeatTime::from_quarters(1));
        assert_eq!(s.wrap(BeatTime::from_quarters(-1)), BeatTime::from_quarters(15));
    }

    #[test]
    fn hanging_excludes_a_marked_carry_but_crossing_does_not() {
        let mut carried = note(1, 60, (14, 1), (4, 1));
        carried.articulation = Some(CARRY_MARK.to_string());
        let plain = note(2, 64, (15, 1), (2, 1));
        let s = set(vec![carried, plain]);
        assert_eq!(hanging_notes(&s, &span()), vec![2]);
        assert_eq!(crossing_notes(&s, &span()), vec![1, 2]);
        assert_eq!(carried_notes(&s, &span()), vec![1]);
    }

    #[test]
    fn a_muted_overhang_is_neither_hanging_nor_crossing() {
        let mut n = note(1, 60, (15, 1), (4, 1));
        n.muted = true;
        let s = set(vec![n]);
        assert!(hanging_notes(&s, &span()).is_empty());
        assert!(crossing_notes(&s, &span()).is_empty());
    }

    #[test]
    fn pickup_and_tail_are_measured_from_the_extremes() {
        let s = set(vec![
            note(1, 55, (-3, 2), (1, 2)),
            note(2, 57, (-1, 1), (1, 1)),
            note(3, 60, (15, 1), (5, 2)),
        ]);
        assert_eq!(pickup_length(&s, &span()), BeatTime::new(3, 2));
        assert_eq!(tail_length(&s, &span()), BeatTime::new(3, 2));
    }

    #[test]
    fn no_pickup_and_no_tail_measure_zero() {
        let s = set(vec![note(1, 60, (0, 1), (4, 1))]);
        assert_eq!(pickup_length(&s, &span()), BeatTime::ZERO);
        assert_eq!(tail_length(&s, &span()), BeatTime::ZERO);
    }

    #[test]
    fn entry_and_exit_points_are_the_notes_on_the_boundaries() {
        let s = set(vec![
            note(1, 60, (0, 1), (2, 1)),
            note(2, 64, (0, 1), (2, 1)),
            note(3, 67, (14, 1), (2, 1)),
        ]);
        assert_eq!(entry_points(&s, &span()), vec![1, 2]);
        assert_eq!(exit_points(&s, &span()), vec![3]);
    }

    #[test]
    fn occupied_length_grows_with_a_pickup_and_an_overhang() {
        let s = set(vec![
            note(1, 55, (-1, 1), (1, 1)),
            note(2, 60, (14, 1), (3, 1)),
        ]);
        assert_eq!(occupied_length(&s, &span()), BeatTime::from_quarters(18));
    }

    #[test]
    fn split_wraps_the_remainder_and_keeps_the_span_exact() {
        let mut notes = vec![note(1, 60, (14, 1), (3, 1))];
        let s = span();
        let before = s.length();
        apply_boundary_policy(&mut notes, &s, CarryPolicy::Split);
        assert_eq!(s.length(), before);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].onset, BeatTime::ZERO);
        assert_eq!(notes[0].duration, BeatTime::from_quarters(1));
        assert_eq!(notes[1].onset, BeatTime::from_quarters(14));
        assert_eq!(notes[1].duration, BeatTime::from_quarters(2));
        assert!(notes.iter().all(|n| n.end() <= s.end));
    }

    #[test]
    fn split_moves_a_pickup_into_the_tail() {
        let mut notes = vec![note(1, 55, (-1, 1), (1, 1)), note(2, 60, (0, 1), (1, 1))];
        let s = span();
        apply_boundary_policy(&mut notes, &s, CarryPolicy::Split);
        assert_eq!(notes.len(), 2);
        let wrapped = notes.iter().find(|n| n.midi == 55).unwrap();
        assert_eq!(wrapped.onset, BeatTime::from_quarters(15));
        assert_eq!(
            wrapped.articulation.as_deref(),
            Some(WRAPPED_PICKUP_MARK),
            "the wrapped pickup is marked so a later audit knows why it is there"
        );
    }

    #[test]
    fn carry_marks_the_overhang_and_moves_nothing() {
        let mut notes = vec![note(1, 60, (14, 1), (3, 1))];
        let s = span();
        apply_boundary_policy(&mut notes, &s, CarryPolicy::Carry);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].end(), BeatTime::from_quarters(17));
        assert!(is_marked_carry(&notes[0]));
    }

    #[test]
    fn truncate_clips_to_the_span_and_drops_the_outside() {
        let mut notes = vec![
            note(1, 55, (-1, 1), (1, 1)),
            note(2, 60, (14, 1), (3, 1)),
            note(3, 64, (20, 1), (1, 1)),
        ];
        let s = span();
        apply_boundary_policy(&mut notes, &s, CarryPolicy::Truncate);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, 2);
        assert_eq!(notes[0].end(), s.end);
    }

    #[test]
    fn rearticulate_reattacks_and_duplicates_the_pickup() {
        let mut notes = vec![note(1, 55, (-1, 1), (1, 1)), note(2, 60, (14, 1), (3, 1))];
        let s = span();
        apply_boundary_policy(&mut notes, &s, CarryPolicy::Rearticulate);
        // original pickup + its tail copy + truncated note + re-attacked head
        assert_eq!(notes.len(), 4);
        assert!(notes
            .iter()
            .any(|n| n.onset == BeatTime::from_quarters(15) && n.midi == 55));
        assert!(notes
            .iter()
            .any(|n| n.onset == BeatTime::from_quarters(-1) && n.midi == 55));
        let head = notes
            .iter()
            .find(|n| n.articulation.as_deref() == Some(REARTICULATED_MARK))
            .unwrap();
        assert_eq!(head.onset, BeatTime::ZERO);
        assert_eq!(head.duration, BeatTime::from_quarters(3));
    }

    #[test]
    fn every_policy_preserves_the_span_exactly() {
        for policy in CarryPolicy::all() {
            let mut notes = vec![note(1, 55, (-1, 1), (1, 1)), note(2, 60, (14, 1), (7, 2))];
            let s = LoopSpan::new(
                BeatTime::new(1, 2),
                BeatTime::new(33, 2),
                LoopIntent::SeamlessColor,
            );
            let before = s.length();
            apply_boundary_policy(&mut notes, &s, *policy);
            assert_eq!(s.length(), before, "policy {}", policy.id());
            assert_eq!(s.length(), BeatTime::from_quarters(16));
        }
    }

    #[test]
    fn applying_a_policy_twice_is_deterministic() {
        let build = || vec![note(1, 55, (-1, 1), (1, 1)), note(2, 60, (14, 1), (3, 1))];
        for policy in CarryPolicy::all() {
            let mut a = build();
            let mut b = build();
            apply_boundary_policy(&mut a, &span(), *policy);
            apply_boundary_policy(&mut b, &span(), *policy);
            let ja: Vec<String> = a.iter().map(|n| n.to_json().to_canonical_string()).collect();
            let jb: Vec<String> = b.iter().map(|n| n.to_json().to_canonical_string()).collect();
            assert_eq!(ja, jb, "policy {}", policy.id());
        }
    }

    #[test]
    fn a_degenerate_span_is_left_alone() {
        let mut notes = vec![note(1, 60, (0, 1), (4, 1))];
        let s = LoopSpan::new(BeatTime::ZERO, BeatTime::ZERO, LoopIntent::OneShotEnding);
        apply_boundary_policy(&mut notes, &s, CarryPolicy::Split);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].duration, BeatTime::from_quarters(4));
    }

    #[test]
    fn policy_ids_round_trip_and_map_to_facts() {
        for p in CarryPolicy::all() {
            assert_eq!(CarryPolicy::parse(p.id()), Some(*p));
        }
        assert_eq!(CarryPolicy::Carry.fact_value(), "carry");
        assert_eq!(CarryPolicy::Split.fact_value(), "split");
        assert_eq!(CarryPolicy::Truncate.fact_value(), "default");
        assert_eq!(CarryPolicy::parse("nope"), None);
    }
}
