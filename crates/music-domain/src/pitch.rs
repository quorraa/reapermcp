//! Spelled pitch: letters, accidentals, octaves, and the conversions between
//! spelling and sounding MIDI pitch.
//!
//! The central rule of this module is that **spelling is data, not decoration**.
//! `G♯` and `A♭` sound the same in twelve-tone equal temperament but they are
//! different values here, because they imply different harmonic roles and
//! different resolutions. Nothing in this crate ever collapses a spelling to a
//! pitch class except where a pitch class is explicitly what is asked for.
//!
//! Scientific pitch notation is used throughout: `C4` is MIDI 60, `A4` is
//! MIDI 69.

use crate::interval::Interval;
use crate::scale::ScaleInstance;
use std::cmp::Ordering;
use std::fmt;

/// The seven natural note letters.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Letter {
    /// C — diatonic index 0, natural pitch class 0.
    #[default]
    C,
    /// D — diatonic index 1, natural pitch class 2.
    D,
    /// E — diatonic index 2, natural pitch class 4.
    E,
    /// F — diatonic index 3, natural pitch class 5.
    F,
    /// G — diatonic index 4, natural pitch class 7.
    G,
    /// A — diatonic index 5, natural pitch class 9.
    A,
    /// B — diatonic index 6, natural pitch class 11.
    B,
}

impl Letter {
    /// Every letter in diatonic order starting from C.
    pub const ALL: [Letter; 7] = [
        Letter::C,
        Letter::D,
        Letter::E,
        Letter::F,
        Letter::G,
        Letter::A,
        Letter::B,
    ];

    /// Parses a single letter, accepting either case.
    pub fn from_char(c: char) -> Option<Letter> {
        match c {
            'C' | 'c' => Some(Letter::C),
            'D' | 'd' => Some(Letter::D),
            'E' | 'e' => Some(Letter::E),
            'F' | 'f' => Some(Letter::F),
            'G' | 'g' => Some(Letter::G),
            'A' | 'a' => Some(Letter::A),
            'B' | 'b' => Some(Letter::B),
            _ => None,
        }
    }

    /// The upper-case character for this letter.
    pub fn as_char(self) -> char {
        match self {
            Letter::C => 'C',
            Letter::D => 'D',
            Letter::E => 'E',
            Letter::F => 'F',
            Letter::G => 'G',
            Letter::A => 'A',
            Letter::B => 'B',
        }
    }

    /// Pitch class of the unaltered letter: C=0 D=2 E=4 F=5 G=7 A=9 B=11.
    pub fn natural_pc(self) -> i32 {
        match self {
            Letter::C => 0,
            Letter::D => 2,
            Letter::E => 4,
            Letter::F => 5,
            Letter::G => 7,
            Letter::A => 9,
            Letter::B => 11,
        }
    }

    /// Position in the diatonic sequence: C=0 .. B=6.
    pub fn diatonic_index(self) -> i32 {
        match self {
            Letter::C => 0,
            Letter::D => 1,
            Letter::E => 2,
            Letter::F => 3,
            Letter::G => 4,
            Letter::A => 5,
            Letter::B => 6,
        }
    }

    /// Letter at diatonic index `i`, wrapping in both directions.
    pub fn from_diatonic_index(i: i32) -> Letter {
        Letter::ALL[i.rem_euclid(7) as usize]
    }

    /// Steps `steps` letters upward (or downward when negative), returning the
    /// new letter and how many octaves were crossed.
    pub fn step(self, steps: i32) -> (Letter, i32) {
        let raw = self.diatonic_index() + steps;
        (Letter::from_diatonic_index(raw), raw.div_euclid(7))
    }
}

impl fmt::Display for Letter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Letter::C => "C",
            Letter::D => "D",
            Letter::E => "E",
            Letter::F => "F",
            Letter::G => "G",
            Letter::A => "A",
            Letter::B => "B",
        })
    }
}

/// Chromatic alteration of a letter, measured in semitones.
///
/// `-2..=2` covers double-flat through double-sharp, which is everything real
/// notation needs; wider values are representable so that arithmetic never
/// silently wraps, but [`Accidental::is_common`] reports them as unusual and
/// [`Accidental::ascii`] renders them in an explicit `(+3)` form.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Accidental(pub i8);

impl Accidental {
    /// Lowered by two semitones (`bb`).
    pub const DOUBLE_FLAT: Accidental = Accidental(-2);
    /// Lowered by one semitone (`b`).
    pub const FLAT: Accidental = Accidental(-1);
    /// Unaltered.
    pub const NATURAL: Accidental = Accidental(0);
    /// Raised by one semitone (`#`).
    pub const SHARP: Accidental = Accidental(1);
    /// Raised by two semitones (`##`).
    pub const DOUBLE_SHARP: Accidental = Accidental(2);

    /// ASCII rendering: `"bb"`, `"b"`, `""`, `"#"`, `"##"`, else `"(+3)"`.
    pub fn ascii(self) -> String {
        match self.0 {
            -2 => "bb".to_string(),
            -1 => "b".to_string(),
            0 => String::new(),
            1 => "#".to_string(),
            2 => "##".to_string(),
            n => format!("({}{})", if n < 0 { "-" } else { "+" }, n.abs()),
        }
    }

    /// Unicode rendering: `"𝄫"`, `"♭"`, `""`, `"♯"`, `"𝄪"`, else the ASCII form.
    pub fn unicode(self) -> String {
        match self.0 {
            -2 => "𝄫".to_string(),
            -1 => "♭".to_string(),
            0 => String::new(),
            1 => "♯".to_string(),
            2 => "𝄪".to_string(),
            _ => self.ascii(),
        }
    }

    /// True for double-flat through double-sharp.
    pub fn is_common(self) -> bool {
        (-2..=2).contains(&self.0)
    }

    /// The alteration in semitones.
    pub fn semitones(self) -> i32 {
        self.0 as i32
    }

    /// Parses a run of accidental characters, ASCII or Unicode.
    ///
    /// Accepts `b`/`♭` (flat), `#`/`♯` (sharp), `x`/`𝄪` (double sharp),
    /// `𝄫` (double flat) and `♮` (natural), in any repeated combination.
    /// Returns `None` if `s` contains a character that is not an accidental.
    pub fn parse(s: &str) -> Option<Accidental> {
        let mut total: i32 = 0;
        for c in s.chars() {
            total += match c {
                'b' | '♭' => -1,
                '#' | '♯' => 1,
                'x' | '𝄪' => 2,
                '𝄫' => -2,
                '♮' => 0,
                _ => return None,
            };
        }
        if total > i8::MAX as i32 || total < i8::MIN as i32 {
            return None;
        }
        Some(Accidental(total as i8))
    }
}

impl fmt::Display for Accidental {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.ascii())
    }
}

/// A letter, an accidental and an octave — a note as it would be written.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SpelledPitch {
    /// Note letter.
    pub letter: Letter,
    /// Chromatic alteration.
    pub accidental: Accidental,
    /// Scientific octave; `C4` is MIDI 60.
    pub octave: i32,
}

impl Default for SpelledPitch {
    /// Middle C.
    fn default() -> Self {
        SpelledPitch::new(Letter::C, Accidental::NATURAL, 4)
    }
}

impl SpelledPitch {
    /// Builds a spelled pitch from its three components.
    pub fn new(letter: Letter, accidental: Accidental, octave: i32) -> Self {
        SpelledPitch {
            letter,
            accidental,
            octave,
        }
    }

    /// Sounding MIDI pitch in twelve-tone equal temperament, `C4 == 60`.
    pub fn midi(self) -> i32 {
        (self.octave + 1) * 12 + self.letter.natural_pc() + self.accidental.semitones()
    }

    /// Sounding pitch class, `0..=11`.
    pub fn pitch_class(self) -> i32 {
        self.midi().rem_euclid(12)
    }

    /// Absolute diatonic position: `octave * 7 + letter index`.
    ///
    /// Two pitches with the same `diatonic_step` are written on the same staff
    /// line regardless of accidental.
    pub fn diatonic_step(self) -> i32 {
        self.octave * 7 + self.letter.diatonic_index()
    }

    /// Parses scientific pitch notation with a mandatory octave.
    ///
    /// `"C#4"`, `"Db4"`, `"C♯4"`, `"Bbb3"`, `"G-1"` all parse; `"C#"` does not.
    pub fn parse(s: &str) -> Option<SpelledPitch> {
        let s = s.trim();
        let mut chars = s.char_indices();
        let (_, first) = chars.next()?;
        let letter = Letter::from_char(first)?;
        // Split the remainder into an accidental run and an octave.
        let rest = &s[first.len_utf8()..];
        let split = rest
            .char_indices()
            .find(|(_, c)| c.is_ascii_digit() || *c == '-' || *c == '+')
            .map(|(i, _)| i)?;
        let accidental = Accidental::parse(&rest[..split])?;
        let oct_text = &rest[split..];
        let oct_text = oct_text.strip_prefix('+').unwrap_or(oct_text);
        let octave: i32 = oct_text.parse().ok()?;
        Some(SpelledPitch::new(letter, accidental, octave))
    }

    /// Parses an octave-less pitch class spelling such as `"C#"` or `"F♯"`.
    pub fn parse_class(s: &str) -> Option<(Letter, Accidental)> {
        let s = s.trim();
        let mut chars = s.chars();
        let letter = Letter::from_char(chars.next()?)?;
        let accidental = Accidental::parse(chars.as_str())?;
        Some((letter, accidental))
    }

    /// ASCII rendering including the octave, e.g. `"C#4"`.
    pub fn to_ascii(self) -> String {
        format!(
            "{}{}{}",
            self.letter.as_char(),
            self.accidental.ascii(),
            self.octave
        )
    }

    /// Unicode rendering including the octave, e.g. `"C♯4"`.
    pub fn to_unicode(self) -> String {
        format!(
            "{}{}{}",
            self.letter.as_char(),
            self.accidental.unicode(),
            self.octave
        )
    }

    /// ASCII rendering without the octave, e.g. `"C#"`.
    pub fn class_ascii(self) -> String {
        format!("{}{}", self.letter.as_char(), self.accidental.ascii())
    }

    /// Unicode rendering without the octave, e.g. `"C♯"`.
    pub fn class_unicode(self) -> String {
        format!("{}{}", self.letter.as_char(), self.accidental.unicode())
    }

    /// Transposes upward by `iv` with theoretically correct spelling.
    ///
    /// The letter moves by the interval's diatonic step count and the
    /// accidental is then whatever is needed to reach the correct sounding
    /// pitch, so `C + M3 = E`, `C + d4 = Fb`, `F# + m3 = A` and `Bb + A4 = E`.
    pub fn transpose(self, iv: Interval) -> SpelledPitch {
        let (letter, octave_delta) = self.letter.step(iv.diatonic_steps());
        let octave = self.octave + octave_delta;
        let target = self.midi() + iv.semitones();
        let natural = (octave + 1) * 12 + letter.natural_pc();
        SpelledPitch::new(letter, Accidental((target - natural) as i8), octave)
    }

    /// Transposes downward by `iv`, mirroring [`SpelledPitch::transpose`].
    pub fn transpose_down(self, iv: Interval) -> SpelledPitch {
        let (letter, octave_delta) = self.letter.step(-iv.diatonic_steps());
        let octave = self.octave + octave_delta;
        let target = self.midi() - iv.semitones();
        let natural = (octave + 1) * 12 + letter.natural_pc();
        SpelledPitch::new(letter, Accidental((target - natural) as i8), octave)
    }

    /// Moves `steps` scale degrees within `scale`, keeping the scale's spelling.
    ///
    /// A pitch that is not in the collection keeps its chromatic offset from the
    /// nearest lower degree.
    pub fn transpose_diatonic(self, steps: i32, scale: &ScaleInstance) -> SpelledPitch {
        let semis = scale.def.semitones.clone();
        if semis.is_empty() {
            return self;
        }
        let n = semis.len() as i32;
        let tonic_ref = SpelledPitch::new(scale.tonic.0, scale.tonic.1, -1).midi();
        let midi = self.midi();
        // Locate the degree at or below this pitch and the leftover chromatic offset.
        let mut best: Option<(i32, i32)> = None; // (absolute degree index, offset)
        for (i, s) in semis.iter().enumerate() {
            let base = tonic_ref + s;
            let diff = midi - base;
            let block = diff.div_euclid(12);
            let offset = diff - block * 12;
            let abs_index = block * n + i as i32;
            match best {
                Some((_, o)) if o <= offset => {}
                _ => best = Some((abs_index, offset)),
            }
        }
        let (abs_index, offset) = best.unwrap_or((0, 0));
        let target_index = abs_index + steps;
        let block = target_index.div_euclid(n);
        let idx = target_index.rem_euclid(n) as usize;
        let target_midi = tonic_ref + 12 * block + semis[idx] + offset;
        let (letter, acc) = scale.spelled_degrees()[idx];
        let natural_pc = letter.natural_pc() + acc.semitones();
        let octave = (target_midi - natural_pc).div_euclid(12) - 1;
        SpelledPitch::new(letter, acc, octave)
    }

    /// Chooses a spelling for a sounding MIDI pitch.
    ///
    /// With a [`SpellingContext`] the active collection wins: if the pitch class
    /// appears in `scale_pcs`, the parallel entry in `scale_spelling` is used.
    /// Otherwise `prefer_flats` decides.
    ///
    /// Without a context the neutral default is: the seven pitch classes of C
    /// major are spelled as naturals, the *sharp side* pitch classes 1, 6 and 8
    /// are spelled `C#`, `F#` and `G#`, and the *flat side* pitch classes 3 and
    /// 10 are spelled `Eb` and `Bb`. This is the conventional neutral spelling
    /// used by notation software and it never produces a double accidental.
    pub fn from_midi(midi: i32, ctx: Option<&SpellingContext>) -> SpelledPitch {
        let pc = midi.rem_euclid(12);
        if let Some(ctx) = ctx {
            if let Some(i) = ctx.scale_pcs.iter().position(|p| p.rem_euclid(12) == pc) {
                if let Some(&(letter, acc)) = ctx.scale_spelling.get(i) {
                    return Self::at_octave_for(letter, acc, midi);
                }
            }
            let (letter, acc) = if ctx.prefer_flats {
                FLAT_SPELLING[pc as usize]
            } else {
                SHARP_SPELLING[pc as usize]
            };
            return Self::at_octave_for(letter, acc, midi);
        }
        let (letter, acc) = NEUTRAL_SPELLING[pc as usize];
        Self::at_octave_for(letter, acc, midi)
    }

    /// Places `(letter, accidental)` in the octave that sounds `midi`.
    fn at_octave_for(letter: Letter, acc: Accidental, midi: i32) -> SpelledPitch {
        let natural_pc = letter.natural_pc() + acc.semitones();
        let octave = (midi - natural_pc).div_euclid(12) - 1;
        SpelledPitch::new(letter, acc, octave)
    }

    /// Rewrites this pitch on a different letter, keeping the sounding pitch.
    ///
    /// Returns `None` when the target letter would need more than a double
    /// accidental (for example respelling `C4` as an `A`).
    pub fn enharmonic_respell(self, target_letter: Letter) -> Option<SpelledPitch> {
        let midi = self.midi();
        let d = midi - target_letter.natural_pc();
        // Round to the nearest octave so the residual accidental is minimal.
        let octave = (d as f64 / 12.0).round() as i32 - 1;
        let acc = d - (octave + 1) * 12;
        if !(-2..=2).contains(&acc) {
            return None;
        }
        Some(SpelledPitch::new(
            target_letter,
            Accidental(acc as i8),
            octave,
        ))
    }

    /// True when the sounding pitch fits the MIDI range `0..=127`.
    pub fn is_valid_midi(self) -> bool {
        (0..=127).contains(&self.midi())
    }
}

/// Neutral spelling table, indexed by pitch class. See [`SpelledPitch::from_midi`].
const NEUTRAL_SPELLING: [(Letter, Accidental); 12] = [
    (Letter::C, Accidental::NATURAL),
    (Letter::C, Accidental::SHARP),
    (Letter::D, Accidental::NATURAL),
    (Letter::E, Accidental::FLAT),
    (Letter::E, Accidental::NATURAL),
    (Letter::F, Accidental::NATURAL),
    (Letter::F, Accidental::SHARP),
    (Letter::G, Accidental::NATURAL),
    (Letter::G, Accidental::SHARP),
    (Letter::A, Accidental::NATURAL),
    (Letter::B, Accidental::FLAT),
    (Letter::B, Accidental::NATURAL),
];

/// All-sharps spelling table, indexed by pitch class.
const SHARP_SPELLING: [(Letter, Accidental); 12] = [
    (Letter::C, Accidental::NATURAL),
    (Letter::C, Accidental::SHARP),
    (Letter::D, Accidental::NATURAL),
    (Letter::D, Accidental::SHARP),
    (Letter::E, Accidental::NATURAL),
    (Letter::F, Accidental::NATURAL),
    (Letter::F, Accidental::SHARP),
    (Letter::G, Accidental::NATURAL),
    (Letter::G, Accidental::SHARP),
    (Letter::A, Accidental::NATURAL),
    (Letter::A, Accidental::SHARP),
    (Letter::B, Accidental::NATURAL),
];

/// All-flats spelling table, indexed by pitch class.
const FLAT_SPELLING: [(Letter, Accidental); 12] = [
    (Letter::C, Accidental::NATURAL),
    (Letter::D, Accidental::FLAT),
    (Letter::D, Accidental::NATURAL),
    (Letter::E, Accidental::FLAT),
    (Letter::E, Accidental::NATURAL),
    (Letter::F, Accidental::NATURAL),
    (Letter::G, Accidental::FLAT),
    (Letter::G, Accidental::NATURAL),
    (Letter::A, Accidental::FLAT),
    (Letter::A, Accidental::NATURAL),
    (Letter::B, Accidental::FLAT),
    (Letter::B, Accidental::NATURAL),
];

impl PartialOrd for SpelledPitch {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SpelledPitch {
    /// Orders by sounding pitch first, then by notated position, so that `B#3`
    /// and `C4` are ordered but never equal.
    fn cmp(&self, other: &Self) -> Ordering {
        self.midi()
            .cmp(&other.midi())
            .then(self.diatonic_step().cmp(&other.diatonic_step()))
    }
}

impl fmt::Display for SpelledPitch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_ascii())
    }
}

/// Preferred-accidental context used when converting sounding pitch to spelling.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpellingContext {
    /// Pitch class of the local tonic, when one is known.
    pub tonic_pc: Option<i32>,
    /// Fallback direction for pitches outside the collection.
    pub prefer_flats: bool,
    /// Sounding pitch classes of the active collection, in degree order.
    pub scale_pcs: Vec<i32>,
    /// Preferred spelling for each degree, parallel to `scale_pcs`.
    pub scale_spelling: Vec<(Letter, Accidental)>,
}

impl SpellingContext {
    /// A context that only expresses a sharp/flat preference.
    pub fn preference(prefer_flats: bool) -> SpellingContext {
        SpellingContext {
            prefer_flats,
            ..SpellingContext::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interval::Interval;

    #[test]
    fn letter_tables() {
        assert_eq!(Letter::C.natural_pc(), 0);
        assert_eq!(Letter::D.natural_pc(), 2);
        assert_eq!(Letter::E.natural_pc(), 4);
        assert_eq!(Letter::F.natural_pc(), 5);
        assert_eq!(Letter::G.natural_pc(), 7);
        assert_eq!(Letter::A.natural_pc(), 9);
        assert_eq!(Letter::B.natural_pc(), 11);
        for (i, l) in Letter::ALL.iter().enumerate() {
            assert_eq!(l.diatonic_index(), i as i32);
            assert_eq!(Letter::from_diatonic_index(i as i32), *l);
        }
    }

    #[test]
    fn letter_from_char_accepts_both_cases() {
        assert_eq!(Letter::from_char('f'), Some(Letter::F));
        assert_eq!(Letter::from_char('F'), Some(Letter::F));
        assert_eq!(Letter::from_char('H'), None);
        assert_eq!(Letter::B.as_char(), 'B');
    }

    #[test]
    fn letter_step_wraps_with_octave_delta() {
        assert_eq!(Letter::B.step(1), (Letter::C, 1));
        assert_eq!(Letter::C.step(-1), (Letter::B, -1));
        assert_eq!(Letter::C.step(7), (Letter::C, 1));
        assert_eq!(Letter::C.step(0), (Letter::C, 0));
        assert_eq!(Letter::G.step(3), (Letter::C, 1));
    }

    #[test]
    fn accidental_rendering() {
        assert_eq!(Accidental::DOUBLE_FLAT.ascii(), "bb");
        assert_eq!(Accidental::FLAT.ascii(), "b");
        assert_eq!(Accidental::NATURAL.ascii(), "");
        assert_eq!(Accidental::SHARP.ascii(), "#");
        assert_eq!(Accidental::DOUBLE_SHARP.ascii(), "##");
        assert_eq!(Accidental(3).ascii(), "(+3)");
        assert_eq!(Accidental(-3).ascii(), "(-3)");
        assert_eq!(Accidental::FLAT.unicode(), "♭");
        assert_eq!(Accidental::DOUBLE_SHARP.unicode(), "𝄪");
        assert!(Accidental(2).is_common());
        assert!(!Accidental(3).is_common());
    }

    #[test]
    fn accidental_parsing() {
        assert_eq!(Accidental::parse(""), Some(Accidental::NATURAL));
        assert_eq!(Accidental::parse("b"), Some(Accidental::FLAT));
        assert_eq!(Accidental::parse("bb"), Some(Accidental::DOUBLE_FLAT));
        assert_eq!(Accidental::parse("##"), Some(Accidental::DOUBLE_SHARP));
        assert_eq!(Accidental::parse("♯"), Some(Accidental::SHARP));
        assert_eq!(Accidental::parse("𝄫"), Some(Accidental::DOUBLE_FLAT));
        assert_eq!(Accidental::parse("x"), Some(Accidental::DOUBLE_SHARP));
        assert_eq!(Accidental::parse("q"), None);
    }

    #[test]
    fn midi_of_reference_pitches() {
        assert_eq!(SpelledPitch::parse("C4").unwrap().midi(), 60);
        assert_eq!(SpelledPitch::parse("A4").unwrap().midi(), 69);
        assert_eq!(SpelledPitch::parse("C-1").unwrap().midi(), 0);
        assert_eq!(SpelledPitch::parse("G9").unwrap().midi(), 127);
    }

    #[test]
    fn enharmonics_agree_on_midi_and_differ_as_values() {
        let gs = SpelledPitch::parse("G#4").unwrap();
        let ab = SpelledPitch::parse("Ab4").unwrap();
        assert_eq!(gs.midi(), ab.midi());
        assert_ne!(gs, ab);
        assert_ne!(gs.diatonic_step(), ab.diatonic_step());
    }

    #[test]
    fn boundary_enharmonics() {
        assert_eq!(SpelledPitch::parse("B#3").unwrap().midi(), 60);
        assert_eq!(SpelledPitch::parse("Cb4").unwrap().midi(), 59);
        assert_eq!(SpelledPitch::parse("Bbb3").unwrap().midi(), 57);
        assert_eq!(SpelledPitch::parse("Fx4").unwrap().midi(), 67);
    }

    #[test]
    fn parse_requires_an_octave() {
        assert!(SpelledPitch::parse("C#").is_none());
        assert!(SpelledPitch::parse("").is_none());
        assert!(SpelledPitch::parse("H4").is_none());
        assert!(SpelledPitch::parse("C#x4").is_some());
    }

    #[test]
    fn parse_unicode_accidentals() {
        assert_eq!(
            SpelledPitch::parse("C♯4").unwrap(),
            SpelledPitch::new(Letter::C, Accidental::SHARP, 4)
        );
        assert_eq!(
            SpelledPitch::parse("E♭3").unwrap(),
            SpelledPitch::new(Letter::E, Accidental::FLAT, 3)
        );
    }

    #[test]
    fn parse_class_forms() {
        assert_eq!(
            SpelledPitch::parse_class("Bb"),
            Some((Letter::B, Accidental::FLAT))
        );
        assert_eq!(
            SpelledPitch::parse_class("F♯"),
            Some((Letter::F, Accidental::SHARP))
        );
        assert_eq!(SpelledPitch::parse_class("Q"), None);
    }

    #[test]
    fn rendering_round_trips() {
        for s in ["C4", "C#4", "Db3", "Bbb3", "F##5", "G-1"] {
            let p = SpelledPitch::parse(s).expect(s);
            assert_eq!(p.to_ascii(), s);
            assert_eq!(SpelledPitch::parse(&p.to_unicode()).unwrap(), p);
        }
    }

    #[test]
    fn transposition_spelling_table() {
        let cases = [
            ("C4", Interval::M3, "E4"),
            ("C4", Interval::new(4, crate::interval::IntervalQuality::Diminished(1)).unwrap(), "Fb4"),
            ("F#4", Interval::m3, "A4"),
            ("Bb3", Interval::A4, "E4"),
            ("C4", Interval::P5, "G4"),
            ("E4", Interval::m3, "G4"),
            ("B3", Interval::m2, "C4"),
            ("Eb4", Interval::M3, "G4"),
            ("A4", Interval::P4, "D5"),
            ("G4", Interval::M6, "E5"),
            ("C4", Interval::P8, "C5"),
            ("Ab3", Interval::M2, "Bb3"),
            ("D4", Interval::d5, "Ab4"),
            ("F4", Interval::A4, "B4"),
        ];
        for (from, iv, to) in cases {
            let p = SpelledPitch::parse(from).unwrap();
            assert_eq!(p.transpose(iv).to_ascii(), to, "{from} + {}", iv.name());
        }
    }

    #[test]
    fn transpose_down_is_the_inverse() {
        for s in ["C4", "F#3", "Bb5", "E4"] {
            let p = SpelledPitch::parse(s).unwrap();
            for iv in [Interval::M3, Interval::P5, Interval::m7, Interval::A4] {
                assert_eq!(p.transpose(iv).transpose_down(iv), p);
            }
        }
    }

    #[test]
    fn from_midi_neutral_default() {
        let expected = [
            "C4", "C#4", "D4", "Eb4", "E4", "F4", "F#4", "G4", "G#4", "A4", "Bb4", "B4",
        ];
        for (i, want) in expected.iter().enumerate() {
            let p = SpelledPitch::from_midi(60 + i as i32, None);
            assert_eq!(&p.to_ascii(), want);
            assert_eq!(p.midi(), 60 + i as i32);
        }
    }

    #[test]
    fn from_midi_prefers_context() {
        let ctx = SpellingContext {
            tonic_pc: Some(6),
            prefer_flats: false,
            scale_pcs: vec![6, 8, 10, 11, 1, 3, 5],
            scale_spelling: vec![
                (Letter::F, Accidental::SHARP),
                (Letter::G, Accidental::SHARP),
                (Letter::A, Accidental::SHARP),
                (Letter::B, Accidental::NATURAL),
                (Letter::C, Accidental::SHARP),
                (Letter::D, Accidental::SHARP),
                (Letter::E, Accidental::SHARP),
            ],
        };
        assert_eq!(SpelledPitch::from_midi(70, Some(&ctx)).to_ascii(), "A#4");
        assert_eq!(SpelledPitch::from_midi(65, Some(&ctx)).to_ascii(), "E#4");
        // Outside the collection: falls back to the sharp preference.
        assert_eq!(SpelledPitch::from_midi(69, Some(&ctx)).to_ascii(), "A4");
    }

    #[test]
    fn from_midi_flat_preference() {
        let ctx = SpellingContext::preference(true);
        assert_eq!(SpelledPitch::from_midi(61, Some(&ctx)).to_ascii(), "Db4");
        assert_eq!(SpelledPitch::from_midi(66, Some(&ctx)).to_ascii(), "Gb4");
        let sharp = SpellingContext::preference(false);
        assert_eq!(SpelledPitch::from_midi(66, Some(&sharp)).to_ascii(), "F#4");
    }

    #[test]
    fn enharmonic_respell_cases() {
        let c4 = SpelledPitch::parse("C4").unwrap();
        assert_eq!(c4.enharmonic_respell(Letter::B).unwrap().to_ascii(), "B#3");
        assert_eq!(c4.enharmonic_respell(Letter::D).unwrap().to_ascii(), "Dbb4");
        assert_eq!(c4.enharmonic_respell(Letter::C).unwrap(), c4);
        assert!(c4.enharmonic_respell(Letter::A).is_none());
        assert!(c4.enharmonic_respell(Letter::F).is_none());
        let fs = SpelledPitch::parse("F#4").unwrap();
        assert_eq!(fs.enharmonic_respell(Letter::G).unwrap().to_ascii(), "Gb4");
    }

    #[test]
    fn enharmonic_respell_preserves_sound() {
        for midi in 55..75 {
            let p = SpelledPitch::from_midi(midi, None);
            for l in Letter::ALL {
                if let Some(r) = p.enharmonic_respell(l) {
                    assert_eq!(r.midi(), midi);
                    assert_eq!(r.letter, l);
                }
            }
        }
    }

    #[test]
    fn ordering_is_by_sound_then_spelling() {
        let bs3 = SpelledPitch::parse("B#3").unwrap();
        let c4 = SpelledPitch::parse("C4").unwrap();
        assert!(bs3 < c4);
        assert!(SpelledPitch::parse("C4").unwrap() < SpelledPitch::parse("D4").unwrap());
        let mut v = vec![
            SpelledPitch::parse("G4").unwrap(),
            SpelledPitch::parse("C4").unwrap(),
            SpelledPitch::parse("E4").unwrap(),
        ];
        v.sort();
        assert_eq!(v[0].to_ascii(), "C4");
        assert_eq!(v[2].to_ascii(), "G4");
    }

    #[test]
    fn midi_range_check() {
        assert!(SpelledPitch::parse("C-1").unwrap().is_valid_midi());
        assert!(!SpelledPitch::parse("Cb-1").unwrap().is_valid_midi());
        assert!(!SpelledPitch::parse("C10").unwrap().is_valid_midi());
    }

    #[test]
    fn pitch_class_and_diatonic_step() {
        let p = SpelledPitch::parse("Bb3").unwrap();
        assert_eq!(p.pitch_class(), 10);
        assert_eq!(p.diatonic_step(), 3 * 7 + 6);
        assert_eq!(p.class_ascii(), "Bb");
        assert_eq!(p.class_unicode(), "B♭");
    }
}
