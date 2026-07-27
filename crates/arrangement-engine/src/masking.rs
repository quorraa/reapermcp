//! Measuring masking, and the levers that reduce it.
//!
//! The brief is explicit that masking avoidance is not an assertion. This
//! module *counts* collisions between parts — simultaneous notes close enough
//! in pitch to fight — reports the register overlap that produced them, and
//! names the levers that would help. [`crate::plan::arrange`] measures before
//! and after separation and keeps both numbers.

use music_domain::prelude::*;

/// How close in pitch two simultaneous notes must be to count as masking.
///
/// A perfect fourth. Inside that distance two sustained parts share enough of
/// the same spectral region that the quieter one stops being separately
/// audible; a fifth or wider reads as harmony rather than mud.
pub const MASKING_WINDOW_SEMITONES: i32 = 5;

/// Upper bound on recorded collisions, so a pathological input cannot produce
/// an unbounded report. The count in [`MaskingReport::onset_collisions`] is not
/// truncated.
pub const MAX_RECORDED_COLLISIONS: usize = 4096;

/// How far separation may move a part from the register the catalogue gave it.
pub const MAX_DISPLACEMENT_SEMITONES: i32 = 12;

/// The narrowest window separation will ever leave a part.
pub const MIN_WINDOW_SEMITONES: i32 = 12;

/// What masking measurement found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaskingReport {
    /// `(part_a, part_b, when, distance_in_semitones)`, ordered by time then
    /// by part index.
    pub collisions: Vec<(usize, usize, BeatTime, i32)>,
    /// Share of the parts' registers that overlaps, `0.0..=1.0`.
    pub register_overlap: f64,
    /// How many times two parts articulate at the very same instant.
    pub onset_collisions: usize,
    /// Named levers that would reduce what was measured.
    pub suggestions: Vec<String>,
}

impl MaskingReport {
    /// The number of masking collisions found.
    pub fn count(&self) -> usize {
        self.collisions.len()
    }

    /// True when nothing masks anything.
    pub fn is_clean(&self) -> bool {
        self.collisions.is_empty() && self.onset_collisions == 0
    }

    /// JSON form, so a plan can carry the measurement into its trace.
    pub fn to_json(&self) -> qjson::Json {
        qjson::json_obj! {
            "collisions" => qjson::Json::Arr(self.collisions.iter().map(|(a, b, qn, d)| {
                qjson::json_obj!{
                    "part_a" => *a as i64,
                    "part_b" => *b as i64,
                    "qn" => qn.to_display(),
                    "semitones" => *d as i64,
                }
            }).collect()),
            "collision_count" => self.collisions.len() as i64,
            "register_overlap" => self.register_overlap,
            "onset_collisions" => self.onset_collisions as i64,
            "suggestions" => qjson::Json::Arr(
                self.suggestions.iter().map(|s| qjson::Json::Str(s.clone())).collect()
            ),
        }
    }
}

/// The lowest and highest sounding pitch of a part, if it has any notes.
pub fn part_register(part: &Part) -> Option<(i32, i32)> {
    let mut iter = part.notes.iter();
    let first = iter.next()?;
    let mut lo = first.midi;
    let mut hi = first.midi;
    for n in iter {
        lo = lo.min(n.midi);
        hi = hi.max(n.midi);
    }
    Some((lo, hi))
}

/// Measures masking across a set of parts, as the frozen contract declares it.
pub fn masking_report(parts: &[Part]) -> MaskingReport {
    let mut report = MaskingReport::default();
    let registers: Vec<Option<(i32, i32)>> = parts.iter().map(part_register).collect();

    let mut overlap_total = 0i64;
    let mut span_total = 0i64;
    for a in 0..parts.len() {
        for b in (a + 1)..parts.len() {
            if let (Some(ra), Some(rb)) = (registers[a], registers[b]) {
                let overlap = (ra.1.min(rb.1) - ra.0.max(rb.0) + 1).max(0);
                let smaller = (ra.1 - ra.0 + 1).min(rb.1 - rb.0 + 1).max(1);
                overlap_total += i64::from(overlap.min(smaller));
                span_total += i64::from(smaller);
            }
            for na in &parts[a].notes {
                for nb in &parts[b].notes {
                    if !na.overlaps(nb) {
                        continue;
                    }
                    if na.onset == nb.onset {
                        report.onset_collisions += 1;
                    }
                    let distance = (na.midi - nb.midi).abs();
                    if distance <= MASKING_WINDOW_SEMITONES
                        && report.collisions.len() < MAX_RECORDED_COLLISIONS
                    {
                        report
                            .collisions
                            .push((a, b, na.onset.max(nb.onset), distance));
                    }
                }
            }
        }
    }
    report.collisions.sort_by(|x, y| {
        x.2.cmp(&y.2)
            .then_with(|| x.0.cmp(&y.0))
            .then_with(|| x.1.cmp(&y.1))
            .then_with(|| x.3.cmp(&y.3))
    });
    report.register_overlap = if span_total > 0 {
        (overlap_total as f64 / span_total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    report.suggestions = suggestions(parts, &report);
    report
}

/// The levers worth pulling, given what was measured.
///
/// Every string names one of the seven avoidance mechanisms the brief lists, so
/// a caller reading the report is told which control to reach for rather than
/// simply that something is wrong.
fn suggestions(parts: &[Part], report: &MaskingReport) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if report.register_overlap > 0.5 {
        out.push(
            "register separation: raise register_spread so the parts occupy distinct windows"
                .to_string(),
        );
    }
    if report.onset_collisions > 0 {
        out.push(
            "onset-density control: thin the background parts off the foreground's onsets"
                .to_string(),
        );
        out.push(
            "contrary rhythmic activity: give a background part an offbeat or complementary grid"
                .to_string(),
        );
    }
    if !report.collisions.is_empty() {
        out.push("note-length control: shorten the lower-priority part".to_string());
        out.push("role priority: let the foreground role keep the contested register".to_string());
        out.push("reduced doubling: drop the doubled voice from the busier part".to_string());
        out.push("voice allocation: move the inner voice to a free register".to_string());
    }
    let doubled: Vec<&str> = parts
        .iter()
        .filter(|p| p.polyphonic && p.notes.len() > 1)
        .map(|p| p.name.as_str())
        .collect();
    if report.register_overlap > 0.75 && doubled.len() > 2 {
        out.push(format!(
            "role substitution: {} share the same register — substitute one role",
            doubled.join(", ")
        ));
    }
    out
}

/// Semitones two inclusive windows share.
pub fn window_overlap(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.1.min(b.1) - a.0.max(b.0) + 1).max(0)
}

/// Pushes a set of register windows apart.
///
/// Windows are separated in ascending order of centre, and each one is only
/// ever moved inside its own instrument limit, which is passed as `bounds`.
/// `spread` is the caller's `register_spread`: at `0.0` nothing moves, at `1.0`
/// the engine insists on a full fourth of clear air between neighbours.
pub fn separate(
    windows: &mut [(i32, i32)],
    bounds: &[(i32, i32)],
    order: &[usize],
    spread: f64,
) {
    let spread = spread.clamp(0.0, 1.0);
    if spread <= 0.0 || windows.len() < 2 {
        return;
    }
    // Narrow each window towards its own centre first: a part that claims less
    // room is easier to keep clear of its neighbours, and the narrowing is
    // itself a register lever.
    let shrink = 1.0 - 0.45 * spread;
    for &i in order {
        let (lo, hi) = windows[i];
        let centre = (lo + hi) / 2;
        let half = (((hi - lo) as f64 * shrink) / 2.0).round() as i32;
        let bound = bounds[i];
        windows[i] = (
            (centre - half).max(bound.0).min(bound.1 - 1),
            (centre + half).min(bound.1).max(bound.0 + 1),
        );
    }
    // No part is displaced more than an octave from where the catalogue put it:
    // separation is meant to clear space, not to rewrite the orchestration.
    let home: Vec<i32> = windows.iter().map(|(lo, hi)| (lo + hi) / 2).collect();
    let mut sorted: Vec<usize> = order.to_vec();
    sorted.sort_by_key(|i| ((windows[*i].0 + windows[*i].1) / 2, *i));
    for k in 1..sorted.len() {
        let below = windows[sorted[k - 1]];
        let i = sorted[k];
        let bound = bounds[i];
        let smaller = (below.1 - below.0 + 1).min(windows[i].1 - windows[i].0 + 1).max(1);
        // How much overlap this spread still tolerates, and the clear air it
        // insists on beyond that.
        let tolerated = ((1.0 - spread) * f64::from(smaller)).round() as i32;
        let gap = (spread * 5.0).round() as i32;
        let overlap = window_overlap(below, windows[i]);
        if overlap <= tolerated && overlap > 0 {
            continue;
        }
        let want_low = if overlap > tolerated {
            below.1 - tolerated + gap
        } else {
            windows[i].0
        };
        let want_low = want_low.min(home[i] + MAX_DISPLACEMENT_SEMITONES);
        if windows[i].0 < want_low {
            let headroom = (bound.1 - windows[i].1).max(0);
            let ceiling = (home[i] + MAX_DISPLACEMENT_SEMITONES
                - (windows[i].0 + windows[i].1) / 2)
                .max(0);
            let shift = (want_low - windows[i].0).min(headroom).min(ceiling);
            if shift > 0 {
                windows[i] = (windows[i].0 + shift, windows[i].1 + shift);
            }
            // Narrowing is the last resort, and never below an octave of room:
            // a part squeezed into a few semitones cannot spell its chord.
            if windows[i].0 < want_low && want_low <= windows[i].1 - MIN_WINDOW_SEMITONES {
                windows[i] = (want_low, windows[i].1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: NoteId, midi: i32, onset: i64, duration: i64) -> Note {
        let mut n = Note::new(
            id,
            SpelledPitch::from_midi(midi, None),
            BeatTime::from_quarters(onset),
            BeatTime::from_quarters(duration),
        );
        n.midi = midi;
        n
    }

    fn part(name: &str, pitches: &[i32]) -> Part {
        Part {
            role: ArrangementRole::HarmonicBed,
            name: name.to_string(),
            notes: pitches
                .iter()
                .enumerate()
                .map(|(i, m)| note(i as NoteId, *m, i as i64, 1))
                .collect(),
            instrument_profile: None,
            channel: 0,
            polyphonic: false,
        }
    }

    #[test]
    fn identical_parts_collide_everywhere() {
        let parts = vec![part("a", &[60, 62, 64]), part("b", &[60, 62, 64])];
        let report = masking_report(&parts);
        assert_eq!(report.collisions.len(), 3);
        assert_eq!(report.onset_collisions, 3);
        assert!(report.register_overlap > 0.99);
        assert!(!report.is_clean());
        assert!(!report.suggestions.is_empty());
    }

    #[test]
    fn separated_registers_do_not_collide() {
        let parts = vec![part("a", &[36, 38, 40]), part("b", &[84, 86, 88])];
        let report = masking_report(&parts);
        assert!(report.collisions.is_empty());
        assert_eq!(report.register_overlap, 0.0);
        assert_eq!(report.onset_collisions, 3);
    }

    #[test]
    fn a_single_part_never_masks_itself() {
        let report = masking_report(&[part("a", &[60, 60, 60])]);
        assert!(report.is_clean());
        assert_eq!(report.count(), 0);
    }

    #[test]
    fn empty_parts_are_handled() {
        assert!(masking_report(&[]).is_clean());
        assert!(masking_report(&[part("a", &[])]).is_clean());
        assert_eq!(part_register(&part("a", &[])), None);
    }

    #[test]
    fn the_report_serialises() {
        let parts = vec![part("a", &[60]), part("b", &[61])];
        let json = masking_report(&parts).to_json();
        assert!(json.get("collisions").is_some());
        assert_eq!(json.get("collision_count").and_then(qjson::Json::as_i64), Some(1));
    }

    #[test]
    fn separation_pushes_windows_apart() {
        let bounds = vec![(36, 96), (36, 96)];
        let mut windows = vec![(48, 72), (48, 72)];
        separate(&mut windows, &bounds, &[0, 1], 1.0);
        assert!(
            window_overlap(windows[0], windows[1]) < 25,
            "{windows:?} did not separate"
        );
    }

    #[test]
    fn separation_respects_instrument_bounds() {
        let bounds = vec![(40, 60), (40, 60)];
        let mut windows = vec![(40, 60), (40, 60)];
        separate(&mut windows, &bounds, &[0, 1], 1.0);
        for w in &windows {
            assert!(w.0 >= 40 && w.1 <= 60, "{w:?} left its bounds");
            assert!(w.0 < w.1);
        }
    }

    #[test]
    fn zero_spread_leaves_windows_alone() {
        let bounds = vec![(36, 96), (36, 96)];
        let mut windows = vec![(48, 72), (48, 72)];
        separate(&mut windows, &bounds, &[0, 1], 0.0);
        assert_eq!(windows, vec![(48, 72), (48, 72)]);
    }
}
