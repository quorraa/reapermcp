//! Diatonic intervals with quality, derived from spelling rather than from
//! semitone distance alone.
//!
//! An [`Interval`] is a *diatonic number* (unison through seventh, plus whole
//! octaves) together with a *quality*. That pairing is what makes `C → F♭` a
//! diminished fourth and `C → E` a major third even though both span four
//! semitones. Intervals are ascending-only; melodic direction is carried by
//! [`SignedInterval`].

use crate::pitch::SpelledPitch;
use std::fmt;

/// Interval quality. `Diminished(n)` and `Augmented(n)` carry a degree so that
/// doubly-diminished and doubly-augmented intervals survive arithmetic.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum IntervalQuality {
    /// Diminished by `n` semitones below minor (imperfect) or perfect.
    Diminished(u8),
    /// Minor — only valid on seconds, thirds, sixths and sevenths.
    Minor,
    /// Perfect — only valid on unisons, fourths and fifths.
    Perfect,
    /// Major — only valid on seconds, thirds, sixths and sevenths.
    Major,
    /// Augmented by `n` semitones above major or perfect.
    Augmented(u8),
}

impl IntervalQuality {
    /// The single-character (or repeated-character) abbreviation: `d`, `m`,
    /// `P`, `M`, `A`, with the character repeated for higher degrees.
    pub fn abbrev(self) -> String {
        match self {
            IntervalQuality::Diminished(n) => "d".repeat(n.max(1) as usize),
            IntervalQuality::Minor => "m".to_string(),
            IntervalQuality::Perfect => "P".to_string(),
            IntervalQuality::Major => "M".to_string(),
            IntervalQuality::Augmented(n) => "A".repeat(n.max(1) as usize),
        }
    }

    /// True when this quality may be applied to a perfect-class number (1, 4, 5).
    pub fn fits_perfect_number(self) -> bool {
        !matches!(self, IntervalQuality::Minor | IntervalQuality::Major)
    }

    /// True when this quality may be applied to an imperfect number (2, 3, 6, 7).
    pub fn fits_imperfect_number(self) -> bool {
        !matches!(self, IntervalQuality::Perfect)
    }
}

/// Semitone span of each simple diatonic number at its major/perfect size.
const BASE_SEMITONES: [i32; 8] = [0, 0, 2, 4, 5, 7, 9, 11];

/// True when the simple diatonic number belongs to the perfect class.
fn is_perfect_number(number: i32) -> bool {
    matches!(number, 1 | 4 | 5)
}

/// An ascending diatonic interval.
///
/// `number` is always the *simple* number `1..=7`; whole octaves live in
/// `octaves`, so a major ninth is `number = 2, octaves = 1`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Interval {
    /// Simple diatonic number, `1..=7`.
    pub number: i32,
    /// Interval quality.
    pub quality: IntervalQuality,
    /// Additional whole octaves on top of the simple number.
    pub octaves: i32,
}

impl Default for Interval {
    /// The perfect unison.
    fn default() -> Self {
        Interval::P1
    }
}

// The frozen contract names these constants exactly as intervals are written
// in text — `m3`, `d5`, `M7` — so the lower-case forms are deliberate.
#[allow(non_upper_case_globals)]
impl Interval {
    /// Perfect unison.
    pub const P1: Interval = Interval {
        number: 1,
        quality: IntervalQuality::Perfect,
        octaves: 0,
    };
    /// Minor second.
    pub const m2: Interval = Interval {
        number: 2,
        quality: IntervalQuality::Minor,
        octaves: 0,
    };
    /// Major second.
    pub const M2: Interval = Interval {
        number: 2,
        quality: IntervalQuality::Major,
        octaves: 0,
    };
    /// Minor third.
    pub const m3: Interval = Interval {
        number: 3,
        quality: IntervalQuality::Minor,
        octaves: 0,
    };
    /// Major third.
    pub const M3: Interval = Interval {
        number: 3,
        quality: IntervalQuality::Major,
        octaves: 0,
    };
    /// Perfect fourth.
    pub const P4: Interval = Interval {
        number: 4,
        quality: IntervalQuality::Perfect,
        octaves: 0,
    };
    /// Augmented fourth (tritone spelled upward).
    pub const A4: Interval = Interval {
        number: 4,
        quality: IntervalQuality::Augmented(1),
        octaves: 0,
    };
    /// Diminished fifth (tritone spelled downward).
    pub const d5: Interval = Interval {
        number: 5,
        quality: IntervalQuality::Diminished(1),
        octaves: 0,
    };
    /// Perfect fifth.
    pub const P5: Interval = Interval {
        number: 5,
        quality: IntervalQuality::Perfect,
        octaves: 0,
    };
    /// Minor sixth.
    pub const m6: Interval = Interval {
        number: 6,
        quality: IntervalQuality::Minor,
        octaves: 0,
    };
    /// Major sixth.
    pub const M6: Interval = Interval {
        number: 6,
        quality: IntervalQuality::Major,
        octaves: 0,
    };
    /// Minor seventh.
    pub const m7: Interval = Interval {
        number: 7,
        quality: IntervalQuality::Minor,
        octaves: 0,
    };
    /// Major seventh.
    pub const M7: Interval = Interval {
        number: 7,
        quality: IntervalQuality::Major,
        octaves: 0,
    };
    /// Perfect octave.
    pub const P8: Interval = Interval {
        number: 1,
        quality: IntervalQuality::Perfect,
        octaves: 1,
    };

    /// Builds an interval from a diatonic number and a quality.
    ///
    /// Numbers above 7 are folded into `octaves`, so `new(9, Major)` is a major
    /// ninth. Returns `None` for a number below 1 or a quality that cannot
    /// apply to that number (a "perfect third", a "major fifth", or a
    /// zero-degree augmentation).
    pub fn new(number: i32, quality: IntervalQuality) -> Option<Interval> {
        if number < 1 {
            return None;
        }
        if matches!(
            quality,
            IntervalQuality::Diminished(0) | IntervalQuality::Augmented(0)
        ) {
            return None;
        }
        let octaves = (number - 1) / 7;
        let simple = (number - 1) % 7 + 1;
        let ok = if is_perfect_number(simple) {
            quality.fits_perfect_number()
        } else {
            quality.fits_imperfect_number()
        };
        if !ok {
            return None;
        }
        Some(Interval {
            number: simple,
            quality,
            octaves,
        })
    }

    /// Builds an interval with explicit octaves, without folding.
    pub fn with_octaves(number: i32, quality: IntervalQuality, octaves: i32) -> Option<Interval> {
        let mut iv = Interval::new(number, quality)?;
        iv.octaves += octaves;
        Some(iv)
    }

    /// Compound diatonic number: 9 for a major ninth, 13 for a thirteenth.
    pub fn compound_number(self) -> i32 {
        self.number + 7 * self.octaves
    }

    /// Size in semitones.
    pub fn semitones(self) -> i32 {
        let base = BASE_SEMITONES[self.number as usize];
        let adjust = match self.quality {
            IntervalQuality::Perfect | IntervalQuality::Major => 0,
            IntervalQuality::Minor => -1,
            IntervalQuality::Augmented(n) => n as i32,
            IntervalQuality::Diminished(n) => {
                if is_perfect_number(self.number) {
                    -(n as i32)
                } else {
                    -(n as i32) - 1
                }
            }
        };
        base + adjust + 12 * self.octaves
    }

    /// Distance in diatonic steps: `number - 1 + 7 * octaves`.
    pub fn diatonic_steps(self) -> i32 {
        self.number - 1 + 7 * self.octaves
    }

    /// The interval from `a` to `b`, with direction.
    ///
    /// Quality is derived from the *letter* distance and the semitone distance
    /// together, so `C4 → F♭4` is a diminished fourth and `C4 → E4` is a major
    /// third.
    pub fn between(a: SpelledPitch, b: SpelledPitch) -> SignedInterval {
        let mut steps = b.diatonic_step() - a.diatonic_step();
        let mut semis = b.midi() - a.midi();
        let descending = steps < 0 || (steps == 0 && semis < 0);
        if descending {
            steps = -steps;
            semis = -semis;
        }
        let octaves = steps.div_euclid(7);
        let number = steps.rem_euclid(7) + 1;
        let simple_semis = semis - 12 * octaves;
        let diff = simple_semis - BASE_SEMITONES[number as usize];
        let quality = quality_from_diff(number, diff);
        SignedInterval {
            interval: Interval {
                number,
                quality,
                octaves,
            },
            descending,
        }
    }

    /// Conventional spelling of a bare semitone count.
    ///
    /// The tritone is spelled as an augmented fourth; every other simple size
    /// takes its usual name. Negative inputs are treated as their absolute
    /// value, since `Interval` is ascending-only.
    pub fn from_semitones_default(semitones: i32) -> Interval {
        let s = semitones.abs();
        let octaves = s / 12;
        let simple = s % 12;
        let base = match simple {
            0 => Interval::P1,
            1 => Interval::m2,
            2 => Interval::M2,
            3 => Interval::m3,
            4 => Interval::M3,
            5 => Interval::P4,
            6 => Interval::A4,
            7 => Interval::P5,
            8 => Interval::m6,
            9 => Interval::M6,
            10 => Interval::m7,
            _ => Interval::M7,
        };
        Interval {
            octaves: base.octaves + octaves,
            ..base
        }
    }

    /// Short name such as `"M3"`, `"P5"`, `"A4"`, `"d7"`, `"M9"`.
    pub fn name(self) -> String {
        format!("{}{}", self.quality.abbrev(), self.compound_number())
    }

    /// Parses the form produced by [`Interval::name`].
    pub fn parse(s: &str) -> Option<Interval> {
        let s = s.trim();
        let split = s.find(|c: char| c.is_ascii_digit())?;
        let (q, n) = s.split_at(split);
        let number: i32 = n.parse().ok()?;
        let mut chars = q.chars();
        let first = chars.next()?;
        let count = q.chars().count() as u8;
        if q.chars().any(|c| c != first) {
            return None;
        }
        let quality = match first {
            'd' => IntervalQuality::Diminished(count),
            'm' if count == 1 => IntervalQuality::Minor,
            'P' | 'p' if count == 1 => IntervalQuality::Perfect,
            'M' if count == 1 => IntervalQuality::Major,
            'A' => IntervalQuality::Augmented(count),
            _ => return None,
        };
        Interval::new(number, quality)
    }

    /// Perfect consonances: unison, fifth, octave.
    pub fn is_perfect_consonance(self) -> bool {
        matches!(self.quality, IntervalQuality::Perfect) && matches!(self.number, 1 | 5)
    }

    /// Imperfect consonances: minor and major thirds and sixths.
    pub fn is_imperfect_consonance(self) -> bool {
        matches!(
            self.quality,
            IntervalQuality::Minor | IntervalQuality::Major
        ) && matches!(self.number, 3 | 6)
    }

    /// Everything that is neither a perfect nor an imperfect consonance.
    ///
    /// The perfect fourth counts as dissonant here, matching the
    /// species-counterpoint convention used by the strict profile.
    pub fn is_dissonant(self) -> bool {
        !self.is_perfect_consonance() && !self.is_imperfect_consonance()
    }

    /// Inverts within the octave: `M3 → m6`, `P1 → P8`, `A4 → d5`.
    pub fn inverted(self) -> Interval {
        let quality = match self.quality {
            IntervalQuality::Perfect => IntervalQuality::Perfect,
            IntervalQuality::Major => IntervalQuality::Minor,
            IntervalQuality::Minor => IntervalQuality::Major,
            IntervalQuality::Augmented(n) => IntervalQuality::Diminished(n),
            IntervalQuality::Diminished(n) => IntervalQuality::Augmented(n),
        };
        if self.number == 1 {
            Interval {
                number: 1,
                quality,
                octaves: 1,
            }
        } else {
            Interval {
                number: 9 - self.number,
                quality,
                octaves: 0,
            }
        }
    }

    /// Drops all octaves, keeping number and quality.
    pub fn simple(self) -> Interval {
        Interval { octaves: 0, ..self }
    }

    /// A descending version of this interval.
    pub fn negate(self) -> SignedInterval {
        SignedInterval {
            interval: self,
            descending: true,
        }
    }

    /// An ascending version of this interval.
    pub fn ascending(self) -> SignedInterval {
        SignedInterval {
            interval: self,
            descending: false,
        }
    }
}

/// Derives the quality of a simple `number` whose size differs from the
/// major/perfect size by `diff` semitones.
fn quality_from_diff(number: i32, diff: i32) -> IntervalQuality {
    if is_perfect_number(number) {
        match diff {
            0 => IntervalQuality::Perfect,
            d if d > 0 => IntervalQuality::Augmented(d.min(u8::MAX as i32) as u8),
            d => IntervalQuality::Diminished((-d).min(u8::MAX as i32) as u8),
        }
    } else {
        match diff {
            0 => IntervalQuality::Major,
            -1 => IntervalQuality::Minor,
            d if d > 0 => IntervalQuality::Augmented(d.min(u8::MAX as i32) as u8),
            d => IntervalQuality::Diminished((-d - 1).min(u8::MAX as i32) as u8),
        }
    }
}

impl fmt::Display for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// An interval plus a melodic direction.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SignedInterval {
    /// The ascending interval size.
    pub interval: Interval,
    /// True when the motion is downward.
    pub descending: bool,
}

impl SignedInterval {
    /// Builds a signed interval.
    pub fn new(interval: Interval, descending: bool) -> SignedInterval {
        SignedInterval {
            interval,
            descending,
        }
    }

    /// Signed semitone distance; negative when descending.
    pub fn semitones(self) -> i32 {
        if self.descending {
            -self.interval.semitones()
        } else {
            self.interval.semitones()
        }
    }

    /// Signed diatonic step distance; negative when descending.
    pub fn diatonic_steps(self) -> i32 {
        if self.descending {
            -self.interval.diatonic_steps()
        } else {
            self.interval.diatonic_steps()
        }
    }

    /// Applies this motion to a pitch, keeping correct spelling.
    pub fn apply(self, p: SpelledPitch) -> SpelledPitch {
        if self.descending {
            p.transpose_down(self.interval)
        } else {
            p.transpose(self.interval)
        }
    }

    /// Reverses the direction.
    pub fn reversed(self) -> SignedInterval {
        SignedInterval {
            interval: self.interval,
            descending: !self.descending,
        }
    }

    /// Name with a leading `-` when descending, e.g. `"-M3"`.
    pub fn name(self) -> String {
        if self.descending {
            format!("-{}", self.interval.name())
        } else {
            self.interval.name()
        }
    }
}

impl fmt::Display for SignedInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::{Accidental, Letter, SpelledPitch};

    fn p(s: &str) -> SpelledPitch {
        SpelledPitch::parse(s).expect("valid pitch")
    }

    #[test]
    fn constant_semitone_sizes() {
        assert_eq!(Interval::P1.semitones(), 0);
        assert_eq!(Interval::m2.semitones(), 1);
        assert_eq!(Interval::M2.semitones(), 2);
        assert_eq!(Interval::m3.semitones(), 3);
        assert_eq!(Interval::M3.semitones(), 4);
        assert_eq!(Interval::P4.semitones(), 5);
        assert_eq!(Interval::A4.semitones(), 6);
        assert_eq!(Interval::d5.semitones(), 6);
        assert_eq!(Interval::P5.semitones(), 7);
        assert_eq!(Interval::m6.semitones(), 8);
        assert_eq!(Interval::M6.semitones(), 9);
        assert_eq!(Interval::m7.semitones(), 10);
        assert_eq!(Interval::M7.semitones(), 11);
        assert_eq!(Interval::P8.semitones(), 12);
    }

    #[test]
    fn diatonic_steps_include_octaves() {
        assert_eq!(Interval::P1.diatonic_steps(), 0);
        assert_eq!(Interval::M3.diatonic_steps(), 2);
        assert_eq!(Interval::P8.diatonic_steps(), 7);
        assert_eq!(
            Interval::new(9, IntervalQuality::Major)
                .unwrap()
                .diatonic_steps(),
            8
        );
    }

    #[test]
    fn compound_construction_folds_octaves() {
        let m9 = Interval::new(9, IntervalQuality::Major).unwrap();
        assert_eq!(m9.number, 2);
        assert_eq!(m9.octaves, 1);
        assert_eq!(m9.semitones(), 14);
        assert_eq!(m9.name(), "M9");
        let p11 = Interval::new(11, IntervalQuality::Perfect).unwrap();
        assert_eq!(p11.semitones(), 17);
        assert_eq!(p11.name(), "P11");
        let m13 = Interval::new(13, IntervalQuality::Major).unwrap();
        assert_eq!(m13.semitones(), 21);
    }

    #[test]
    fn invalid_quality_number_pairs_rejected() {
        assert!(Interval::new(3, IntervalQuality::Perfect).is_none());
        assert!(Interval::new(5, IntervalQuality::Major).is_none());
        assert!(Interval::new(4, IntervalQuality::Minor).is_none());
        assert!(Interval::new(0, IntervalQuality::Perfect).is_none());
        assert!(Interval::new(-3, IntervalQuality::Major).is_none());
        assert!(Interval::new(3, IntervalQuality::Augmented(0)).is_none());
    }

    #[test]
    fn between_derives_quality_from_spelling() {
        assert_eq!(Interval::between(p("C4"), p("E4")).interval.name(), "M3");
        assert_eq!(Interval::between(p("C4"), p("Fb4")).interval.name(), "d4");
        assert_eq!(Interval::between(p("C4"), p("Eb4")).interval.name(), "m3");
        assert_eq!(Interval::between(p("C4"), p("D#4")).interval.name(), "A2");
        assert_eq!(Interval::between(p("C4"), p("F#4")).interval.name(), "A4");
        assert_eq!(Interval::between(p("C4"), p("Gb4")).interval.name(), "d5");
        assert_eq!(Interval::between(p("B3"), p("F4")).interval.name(), "d5");
        assert_eq!(Interval::between(p("F4"), p("B4")).interval.name(), "A4");
        assert_eq!(Interval::between(p("C4"), p("C5")).interval.name(), "P8");
        // A chromatic step with no letter motion is an augmented unison; the
        // direction is carried by `descending`, not by the quality.
        let down_chromatic = Interval::between(p("C4"), p("Cb4"));
        assert_eq!(down_chromatic.interval.name(), "A1");
        assert!(down_chromatic.descending);
        assert_eq!(Interval::between(p("C4"), p("C#4")).interval.name(), "A1");
        assert_eq!(Interval::between(p("Bb3"), p("Db4")).interval.name(), "m3");
        assert_eq!(Interval::between(p("C4"), p("Bbb4")).interval.name(), "d7");
        assert_eq!(Interval::between(p("C4"), p("E5")).interval.name(), "M10");
    }

    #[test]
    fn between_reports_direction() {
        let down = Interval::between(p("E4"), p("C4"));
        assert!(down.descending);
        assert_eq!(down.interval.name(), "M3");
        assert_eq!(down.semitones(), -4);
        assert_eq!(down.name(), "-M3");
        let up = Interval::between(p("C4"), p("E4"));
        assert!(!up.descending);
        assert_eq!(up.semitones(), 4);
    }

    #[test]
    fn between_round_trips_through_apply() {
        let pitches = ["C4", "F#3", "Bb4", "E5", "Ab3", "D#4"];
        for a in pitches {
            for b in pitches {
                let si = Interval::between(p(a), p(b));
                assert_eq!(si.apply(p(a)), p(b), "{a} -> {b}");
            }
        }
    }

    #[test]
    fn from_semitones_default_table() {
        let names = [
            "P1", "m2", "M2", "m3", "M3", "P4", "A4", "P5", "m6", "M6", "m7", "M7",
        ];
        for (i, n) in names.iter().enumerate() {
            assert_eq!(Interval::from_semitones_default(i as i32).name(), *n);
        }
        assert_eq!(Interval::from_semitones_default(12).name(), "P8");
        assert_eq!(Interval::from_semitones_default(14).name(), "M9");
        assert_eq!(Interval::from_semitones_default(-4).name(), "M3");
    }

    #[test]
    fn name_and_parse_round_trip() {
        let names = [
            "P1", "m2", "M2", "m3", "M3", "P4", "A4", "d5", "P5", "m6", "M6", "m7", "M7", "P8",
            "M9", "P11", "M13", "d7", "A2", "dd5", "AA4",
        ];
        for n in names {
            let iv = Interval::parse(n).unwrap_or_else(|| panic!("parse {n}"));
            assert_eq!(iv.name(), n);
        }
        assert!(Interval::parse("").is_none());
        assert!(Interval::parse("X3").is_none());
        assert!(Interval::parse("P3").is_none());
        assert!(Interval::parse("mM3").is_none());
        assert!(Interval::parse("M").is_none());
    }

    #[test]
    fn doubly_altered_sizes() {
        let dd5 = Interval::parse("dd5").unwrap();
        assert_eq!(dd5.semitones(), 5);
        let aa4 = Interval::parse("AA4").unwrap();
        assert_eq!(aa4.semitones(), 7);
        let dd7 = Interval::parse("dd7").unwrap();
        assert_eq!(dd7.semitones(), 8);
    }

    #[test]
    fn consonance_classification() {
        assert!(Interval::P1.is_perfect_consonance());
        assert!(Interval::P5.is_perfect_consonance());
        assert!(Interval::P8.is_perfect_consonance());
        assert!(Interval::M3.is_imperfect_consonance());
        assert!(Interval::m6.is_imperfect_consonance());
        assert!(Interval::P4.is_dissonant());
        assert!(Interval::A4.is_dissonant());
        assert!(Interval::M7.is_dissonant());
        assert!(!Interval::M3.is_dissonant());
    }

    #[test]
    fn inversion_table() {
        assert_eq!(Interval::M3.inverted().name(), "m6");
        assert_eq!(Interval::m3.inverted().name(), "M6");
        assert_eq!(Interval::P5.inverted().name(), "P4");
        assert_eq!(Interval::A4.inverted().name(), "d5");
        assert_eq!(Interval::d5.inverted().name(), "A4");
        assert_eq!(Interval::P1.inverted().name(), "P8");
        assert_eq!(Interval::m2.inverted().name(), "M7");
        for iv in [Interval::M3, Interval::m3, Interval::P5, Interval::m7] {
            assert_eq!(iv.semitones() + iv.inverted().semitones(), 12);
        }
    }

    #[test]
    fn simple_drops_octaves() {
        let m9 = Interval::new(9, IntervalQuality::Major).unwrap();
        assert_eq!(m9.simple(), Interval::M2);
        assert_eq!(m9.simple().semitones(), 2);
    }

    #[test]
    fn signed_helpers() {
        let down = Interval::M3.negate();
        assert!(down.descending);
        assert_eq!(down.semitones(), -4);
        assert_eq!(down.diatonic_steps(), -2);
        assert_eq!(down.reversed().semitones(), 4);
        assert_eq!(Interval::M3.ascending().semitones(), 4);
        assert_eq!(
            down.apply(SpelledPitch::new(Letter::E, Accidental::NATURAL, 4))
                .to_ascii(),
            "C4"
        );
    }

    #[test]
    fn quality_abbreviations() {
        assert_eq!(IntervalQuality::Diminished(2).abbrev(), "dd");
        assert_eq!(IntervalQuality::Augmented(1).abbrev(), "A");
        assert_eq!(IntervalQuality::Minor.abbrev(), "m");
        assert_eq!(IntervalQuality::Perfect.abbrev(), "P");
        assert_eq!(IntervalQuality::Major.abbrev(), "M");
        assert!(IntervalQuality::Perfect.fits_perfect_number());
        assert!(!IntervalQuality::Perfect.fits_imperfect_number());
        assert!(IntervalQuality::Major.fits_imperfect_number());
        assert!(!IntervalQuality::Major.fits_perfect_number());
    }

    #[test]
    fn display_matches_name() {
        assert_eq!(Interval::M3.to_string(), "M3");
        assert_eq!(Interval::M3.negate().to_string(), "-M3");
        assert_eq!(Interval::default(), Interval::P1);
    }

    #[test]
    fn with_octaves_adds_registers() {
        let iv = Interval::with_octaves(3, IntervalQuality::Major, 1).unwrap();
        assert_eq!(iv.semitones(), 16);
        assert_eq!(iv.compound_number(), 10);
    }
}
