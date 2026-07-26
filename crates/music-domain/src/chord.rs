//! The semantic chord model.
//!
//! A chord here is never a bare pitch-class set. It is a root, a triad quality,
//! a seventh quality, and explicit lists of extensions, additions, alterations
//! and omissions, plus an optional bass. That is what lets the rest of the
//! system tell `Cadd9` from `C9`, `C6` from `C13`, `Csus4` from `C11` and
//! `CmMaj7` from `Cm7` — distinctions that a pitch-class set destroys.
//!
//! Parsing text into a [`ChordSpec`] lives in [`crate::symbol`]; rendering back
//! to text lives here, and the two are inverse:
//! `parse(render_ascii(parse(s))) == parse(s)`.

use crate::error::DomainError;
use crate::interval::{Interval, IntervalQuality};
use crate::note::VoiceId;
use crate::pitch::{Accidental, Letter, SpelledPitch};
use crate::time::BeatTime;
use qjson::{json_obj, Json};
use std::fmt;

/// Quality of the chord's triad, including the suspended and rootless cases.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum TriadQuality {
    /// Major third, perfect fifth.
    #[default]
    Major,
    /// Minor third, perfect fifth.
    Minor,
    /// Minor third, diminished fifth.
    Diminished,
    /// Major third, augmented fifth.
    Augmented,
    /// The second replaces the third.
    Sus2,
    /// The fourth replaces the third.
    Sus4,
    /// Root and fifth only.
    Power,
    /// Neither third nor fifth is implied by the symbol.
    Omitted,
}

impl TriadQuality {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            TriadQuality::Major => "major",
            TriadQuality::Minor => "minor",
            TriadQuality::Diminished => "diminished",
            TriadQuality::Augmented => "augmented",
            TriadQuality::Sus2 => "sus2",
            TriadQuality::Sus4 => "sus4",
            TriadQuality::Power => "power",
            TriadQuality::Omitted => "omitted",
        }
    }

    /// Parses the identifier produced by [`TriadQuality::id`].
    pub fn parse(s: &str) -> Option<TriadQuality> {
        Some(match s {
            "major" => TriadQuality::Major,
            "minor" => TriadQuality::Minor,
            "diminished" => TriadQuality::Diminished,
            "augmented" => TriadQuality::Augmented,
            "sus2" => TriadQuality::Sus2,
            "sus4" => TriadQuality::Sus4,
            "power" => TriadQuality::Power,
            "omitted" => TriadQuality::Omitted,
            _ => return None,
        })
    }

    /// Every variant, in declaration order.
    pub fn all() -> &'static [TriadQuality] {
        &[
            TriadQuality::Major,
            TriadQuality::Minor,
            TriadQuality::Diminished,
            TriadQuality::Augmented,
            TriadQuality::Sus2,
            TriadQuality::Sus4,
            TriadQuality::Power,
            TriadQuality::Omitted,
        ]
    }
}

/// Quality of the chord's seventh.
///
/// A dominant seventh is [`TriadQuality::Major`] plus
/// [`SeventhQuality::Minor`]; there is no separate "dominant" variant.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum SeventhQuality {
    /// No seventh in the symbol.
    #[default]
    None,
    /// Major seventh (11 semitones).
    Major,
    /// Minor seventh (10 semitones).
    Minor,
    /// Diminished seventh (9 semitones).
    Diminished,
    /// A major seventh named as augmented-major, e.g. `+Δ7`.
    ///
    /// It behaves exactly like [`SeventhQuality::Major`]; the raised fifth is
    /// carried by [`TriadQuality::Augmented`], never by the seventh itself.
    AugmentedMajor,
}

impl SeventhQuality {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            SeventhQuality::None => "none",
            SeventhQuality::Major => "major",
            SeventhQuality::Minor => "minor",
            SeventhQuality::Diminished => "diminished",
            SeventhQuality::AugmentedMajor => "augmented_major",
        }
    }

    /// Parses the identifier produced by [`SeventhQuality::id`].
    pub fn parse(s: &str) -> Option<SeventhQuality> {
        Some(match s {
            "none" => SeventhQuality::None,
            "major" => SeventhQuality::Major,
            "minor" => SeventhQuality::Minor,
            "diminished" => SeventhQuality::Diminished,
            "augmented_major" => SeventhQuality::AugmentedMajor,
            _ => return None,
        })
    }

    /// Alteration of the seventh degree in semitones, if a seventh is present.
    pub fn alter(self) -> Option<i8> {
        match self {
            SeventhQuality::None => None,
            SeventhQuality::Major | SeventhQuality::AugmentedMajor => Some(0),
            SeventhQuality::Minor => Some(-1),
            SeventhQuality::Diminished => Some(-2),
        }
    }

    /// True when the symbol carries a seventh at all.
    pub fn is_present(self) -> bool {
        self != SeventhQuality::None
    }
}

/// Diatonic size of chord degrees 1..=13, in semitones, before alteration.
const DEGREE_SEMITONES: [i32; 14] = [0, 0, 2, 4, 5, 7, 9, 11, 12, 14, 16, 17, 19, 21];

/// A chord degree above the root: a number `1..=13` plus an alteration.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ChordDegree {
    /// Diatonic degree number, `1..=13`.
    pub number: u8,
    /// Chromatic alteration in semitones.
    pub alter: i8,
}

impl ChordDegree {
    /// Builds a degree.
    pub fn new(number: u8, alter: i8) -> ChordDegree {
        ChordDegree { number, alter }
    }

    /// Parses `"b9"`, `"#11"`, `"13"`, `"b5"`, accepting Unicode accidentals
    /// and the `-`/`+` alteration prefixes used by some lead sheets.
    pub fn parse(s: &str) -> Option<ChordDegree> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let mut alter = 0i32;
        let mut rest = s;
        loop {
            let mut chars = rest.chars();
            match chars.next() {
                Some('b') | Some('♭') | Some('-') => alter -= 1,
                Some('#') | Some('♯') | Some('+') => alter += 1,
                Some('𝄫') => alter -= 2,
                Some('𝄪') => alter += 2,
                _ => break,
            }
            rest = chars.as_str();
        }
        if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let number: u32 = rest.parse().ok()?;
        if !(1..=13).contains(&number) {
            return None;
        }
        if !(-4..=4).contains(&alter) {
            return None;
        }
        Some(ChordDegree {
            number: number as u8,
            alter: alter as i8,
        })
    }

    /// Text form such as `"b9"` or `"13"`.
    // The frozen contract names this method `to_string`; a `Display` impl is
    // deliberately not provided so the two can never disagree.
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(self) -> String {
        format!("{}{}", alter_prefix(self.alter, false), self.number)
    }

    /// Text form using Unicode accidentals.
    pub fn to_unicode(self) -> String {
        format!("{}{}", alter_prefix(self.alter, true), self.number)
    }

    /// Distance above the root in semitones, keeping the compound register
    /// (a ninth is 14 semitones, not 2).
    pub fn semitones_from_root(self) -> i32 {
        let n = (self.number as usize).min(13);
        DEGREE_SEMITONES[n] + self.alter as i32
    }

    /// Distance above the root reduced into one octave.
    pub fn simple_semitones(self) -> i32 {
        self.semitones_from_root().rem_euclid(12)
    }

    /// The interval from the root to this degree.
    pub fn interval(self) -> Interval {
        let number = self.number.max(1) as i32;
        let simple = (number - 1) % 7 + 1;
        let perfect = matches!(simple, 1 | 4 | 5);
        let quality = if perfect {
            match self.alter {
                0 => IntervalQuality::Perfect,
                a if a > 0 => IntervalQuality::Augmented(a as u8),
                a => IntervalQuality::Diminished((-a) as u8),
            }
        } else {
            match self.alter {
                0 => IntervalQuality::Major,
                -1 => IntervalQuality::Minor,
                a if a > 0 => IntervalQuality::Augmented(a as u8),
                a => IntervalQuality::Diminished((-a - 1) as u8),
            }
        };
        Interval::new(number, quality).unwrap_or(Interval::P1)
    }
}

/// Renders an alteration prefix: `"bb"`, `"b"`, `""`, `"#"`, `"##"`.
fn alter_prefix(alter: i8, unicode: bool) -> String {
    let a = Accidental(alter);
    if unicode {
        a.unicode()
    } else {
        a.ascii()
    }
}

/// The semantic identity of a chord symbol.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct ChordSpec {
    /// Spelled root.
    pub root: (Letter, Accidental),
    /// Triad quality.
    pub triad: TriadQuality,
    /// Seventh quality.
    pub seventh: SeventhQuality,
    /// Stacked extensions (9, 11, 13); their presence implies a seventh.
    pub extensions: Vec<ChordDegree>,
    /// Added tones (`6`, `add9`, `add4`); they imply no seventh.
    pub added: Vec<ChordDegree>,
    /// Altered degrees (`b5`, `#5`, `b9`, `#9`, `#11`, `b13`).
    pub alterations: Vec<ChordDegree>,
    /// Degrees the symbol explicitly omits (1, 3, 5).
    pub omissions: Vec<u8>,
    /// Slash bass, when the symbol names one.
    pub bass: Option<(Letter, Accidental)>,
    /// True when the symbol said `alt`.
    ///
    /// `alt` names an altered-dominant **family**, not one fixed pitch set: the
    /// concrete tensions (`b9`/`#9`/`#11`/`b13`, and which of them appear) are
    /// chosen later by the harmony engine from melody, destination, style and
    /// voice leading. Nothing here fixes them.
    pub alt_dominant: bool,
}

impl ChordSpec {
    /// A bare triad on `root`.
    pub fn triad(root: (Letter, Accidental), triad: TriadQuality) -> ChordSpec {
        ChordSpec {
            root,
            triad,
            ..ChordSpec::default()
        }
    }

    /// A seventh chord on `root`.
    pub fn seventh(
        root: (Letter, Accidental),
        triad: TriadQuality,
        seventh: SeventhQuality,
    ) -> ChordSpec {
        ChordSpec {
            root,
            triad,
            seventh,
            ..ChordSpec::default()
        }
    }

    /// Sorts and de-duplicates every degree list so equal chords compare equal.
    pub fn normalize(&mut self) {
        self.extensions.sort_unstable();
        self.extensions.dedup();
        self.added.sort_unstable();
        self.added.dedup();
        self.alterations.sort_unstable();
        self.alterations.dedup();
        self.omissions.sort_unstable();
        self.omissions.dedup();
    }

    /// True when the symbol explicitly omits degree `n`.
    pub fn omits(&self, n: u8) -> bool {
        self.omissions.contains(&n)
    }

    /// The chord's semantic tones as `(degree, spelled pitch class)`, in
    /// ascending degree order, root position.
    ///
    /// Omissions are honoured here because they are part of what the symbol
    /// says; they never change the chord's *identity*, which is the whole
    /// `ChordSpec`.
    pub fn chord_tones(&self) -> Vec<(ChordDegree, (Letter, Accidental))> {
        let mut degrees: Vec<ChordDegree> = Vec::new();
        let mut push = |d: ChordDegree| {
            if !degrees.contains(&d) {
                degrees.push(d);
            }
        };
        if !self.omits(1) {
            push(ChordDegree::new(1, 0));
        }
        match self.triad {
            TriadQuality::Major | TriadQuality::Augmented => {
                if !self.omits(3) {
                    push(ChordDegree::new(3, 0));
                }
            }
            TriadQuality::Minor | TriadQuality::Diminished => {
                if !self.omits(3) {
                    push(ChordDegree::new(3, -1));
                }
            }
            TriadQuality::Sus2 => push(ChordDegree::new(2, 0)),
            TriadQuality::Sus4 => push(ChordDegree::new(4, 0)),
            TriadQuality::Power | TriadQuality::Omitted => {}
        }
        if !self.omits(5) && self.triad != TriadQuality::Omitted {
            let natural = match self.triad {
                TriadQuality::Diminished => -1,
                TriadQuality::Augmented => 1,
                _ => 0,
            };
            let alter = self
                .alterations
                .iter()
                .find(|d| d.number == 5)
                .map(|d| d.alter)
                .unwrap_or(natural);
            push(ChordDegree::new(5, alter));
        }
        if let Some(alter) = self.seventh.alter() {
            push(ChordDegree::new(7, alter));
        }
        for e in &self.extensions {
            if !self.alterations.iter().any(|a| a.number == e.number) {
                push(*e);
            }
        }
        for a in &self.added {
            push(*a);
        }
        for a in &self.alterations {
            if a.number != 5 {
                push(*a);
            }
        }
        degrees.sort_unstable();
        degrees.dedup();

        let root_pitch = SpelledPitch::new(self.root.0, self.root.1, 4);
        degrees
            .into_iter()
            .map(|d| {
                let p = root_pitch.transpose(d.interval());
                (d, (p.letter, p.accidental))
            })
            .collect()
    }

    /// Sounding pitch classes of the chord tones, in degree order, de-duplicated.
    ///
    /// The slash bass is *not* included; use [`ChordSpec::bass_pc`] for that.
    pub fn pitch_classes(&self) -> Vec<i32> {
        let mut out: Vec<i32> = Vec::new();
        for (_, (letter, acc)) in self.chord_tones() {
            let pc = (letter.natural_pc() + acc.semitones()).rem_euclid(12);
            if !out.contains(&pc) {
                out.push(pc);
            }
        }
        out
    }

    /// The tones that carry the chord's quality: the third (or its suspended
    /// substitute) and the seventh (or the sixth when there is no seventh).
    pub fn guide_tones(&self) -> Vec<ChordDegree> {
        let mut out = Vec::new();
        match self.triad {
            TriadQuality::Major | TriadQuality::Augmented if !self.omits(3) => {
                out.push(ChordDegree::new(3, 0))
            }
            TriadQuality::Minor | TriadQuality::Diminished if !self.omits(3) => {
                out.push(ChordDegree::new(3, -1))
            }
            TriadQuality::Sus2 => out.push(ChordDegree::new(2, 0)),
            TriadQuality::Sus4 => out.push(ChordDegree::new(4, 0)),
            _ => {}
        }
        if let Some(alter) = self.seventh.alter() {
            out.push(ChordDegree::new(7, alter));
        } else if let Some(six) = self.added.iter().find(|d| d.number == 6) {
            out.push(*six);
        }
        out
    }

    /// True for dominant-function sonorities: a major (or suspended, or
    /// augmented) triad with a minor seventh, and anything marked `alt`.
    pub fn is_dominant_family(&self) -> bool {
        if self.alt_dominant {
            return true;
        }
        self.seventh == SeventhQuality::Minor
            && matches!(
                self.triad,
                TriadQuality::Major
                    | TriadQuality::Augmented
                    | TriadQuality::Sus2
                    | TriadQuality::Sus4
                    | TriadQuality::Omitted
            )
    }

    /// True for major triads and major-seventh family chords.
    pub fn is_major_family(&self) -> bool {
        self.triad == TriadQuality::Major
            && !self.alt_dominant
            && matches!(
                self.seventh,
                SeventhQuality::None | SeventhQuality::Major | SeventhQuality::AugmentedMajor
            )
    }

    /// True for minor-triad chords, including `mMaj7`.
    pub fn is_minor_family(&self) -> bool {
        self.triad == TriadQuality::Minor
    }

    /// True for diminished triads, half-diminished and fully diminished chords.
    pub fn is_diminished_family(&self) -> bool {
        self.triad == TriadQuality::Diminished
    }

    /// True when the third is replaced by a second or a fourth.
    pub fn is_suspended(&self) -> bool {
        matches!(self.triad, TriadQuality::Sus2 | TriadQuality::Sus4)
    }

    /// True when the chord sounds degree `n` in any form.
    pub fn has_degree(&self, n: u8) -> bool {
        self.chord_tones().iter().any(|(d, _)| d.number == n)
    }

    /// The degree that sounds pitch class `pc`, if any.
    pub fn degree_of_pc(&self, pc: i32) -> Option<ChordDegree> {
        let pc = pc.rem_euclid(12);
        self.chord_tones()
            .into_iter()
            .find(|(_, (l, a))| (l.natural_pc() + a.semitones()).rem_euclid(12) == pc)
            .map(|(d, _)| d)
    }

    /// Sounding pitch class of the root.
    pub fn root_pc(&self) -> i32 {
        (self.root.0.natural_pc() + self.root.1.semitones()).rem_euclid(12)
    }

    /// Sounding pitch class of the bass — the slash bass when present, the root
    /// otherwise.
    pub fn bass_pc(&self) -> i32 {
        match self.bass {
            Some((l, a)) => (l.natural_pc() + a.semitones()).rem_euclid(12),
            None => self.root_pc(),
        }
    }

    /// Transposes root and bass, keeping every degree relationship.
    pub fn transpose(&self, iv: Interval) -> ChordSpec {
        let move_class = |(l, a): (Letter, Accidental)| {
            let p = SpelledPitch::new(l, a, 4).transpose(iv);
            (p.letter, p.accidental)
        };
        ChordSpec {
            root: move_class(self.root),
            bass: self.bass.map(move_class),
            ..self.clone()
        }
    }

    /// Chord family identifier: `"major"`, `"minor"`, `"dominant"`,
    /// `"half_dim"`, `"dim"`, `"sus"`, `"power"` or `"aug"`.
    pub fn family_id(&self) -> &'static str {
        match self.triad {
            TriadQuality::Power => "power",
            TriadQuality::Sus2 | TriadQuality::Sus4 => "sus",
            TriadQuality::Diminished => {
                if self.seventh == SeventhQuality::Minor {
                    "half_dim"
                } else {
                    "dim"
                }
            }
            TriadQuality::Augmented => {
                if self.seventh == SeventhQuality::Minor {
                    "dominant"
                } else {
                    "aug"
                }
            }
            TriadQuality::Minor => "minor",
            TriadQuality::Major | TriadQuality::Omitted => {
                if self.seventh == SeventhQuality::Minor || self.alt_dominant {
                    "dominant"
                } else {
                    "major"
                }
            }
        }
    }

    /// Canonical ASCII symbol text. Round-trips through [`crate::symbol::parse`].
    pub fn render_ascii(&self) -> String {
        self.render(false)
    }

    /// Canonical symbol text using Unicode accidentals.
    pub fn render_unicode(&self) -> String {
        self.render(true)
    }

    /// Shared rendering path for the ASCII and Unicode forms.
    fn render(&self, unicode: bool) -> String {
        let acc = |a: Accidental| {
            if unicode {
                a.unicode()
            } else {
                a.ascii()
            }
        };
        let mut s = String::new();
        s.push(self.root.0.as_char());
        s.push_str(&acc(self.root.1));

        let stack = self.stack_top();
        let sixth = self.added.iter().any(|d| *d == ChordDegree::new(6, 0));
        let ninth_added = self.added.iter().any(|d| *d == ChordDegree::new(9, 0));
        let six_nine = sixth && ninth_added && !self.seventh.is_present();
        let mut core_consumed_six = false;
        let mut core_consumed_nine = false;
        // The extensions the core text actually expressed; anything else has to
        // be spelled out in parentheses so the symbol still round-trips.
        let mut core_stack: Option<u8> = None;

        // Core: triad marker plus the highest expressible seventh/extension.
        match self.triad {
            TriadQuality::Power => s.push('5'),
            TriadQuality::Diminished => {
                match self.seventh {
                    SeventhQuality::Diminished => s.push_str("dim7"),
                    SeventhQuality::Minor => s.push_str(&format!("m7{}5", acc(Accidental::FLAT))),
                    SeventhQuality::Major | SeventhQuality::AugmentedMajor => s.push_str("dimMaj7"),
                    SeventhQuality::None => s.push_str("dim"),
                };
            }
            TriadQuality::Augmented => {
                match self.seventh {
                    SeventhQuality::None => s.push_str("aug"),
                    SeventhQuality::Minor => {
                        s.push_str("aug");
                        s.push_str(&stack_text(stack, ""));
                        core_stack = stack;
                    }
                    SeventhQuality::Major | SeventhQuality::AugmentedMajor => {
                        s.push_str("aug");
                        s.push_str(&stack_text(stack, "maj"));
                        core_stack = stack;
                    }
                    SeventhQuality::Diminished => s.push_str("aug(bb7)"),
                };
            }
            TriadQuality::Sus2 | TriadQuality::Sus4 => {
                match self.seventh {
                    SeventhQuality::None => {}
                    SeventhQuality::Minor => {
                        s.push_str(&stack_text(stack, ""));
                        core_stack = stack;
                    }
                    SeventhQuality::Major | SeventhQuality::AugmentedMajor => {
                        s.push_str(&stack_text(stack, "maj"));
                        core_stack = stack;
                    }
                    SeventhQuality::Diminished => s.push_str("(bb7)"),
                };
                s.push_str(if self.triad == TriadQuality::Sus2 {
                    "sus2"
                } else {
                    "sus4"
                });
            }
            TriadQuality::Minor => {
                s.push('m');
                match self.seventh {
                    SeventhQuality::None => {
                        if six_nine {
                            s.push_str("6/9");
                            core_consumed_six = true;
                            core_consumed_nine = true;
                        } else if sixth {
                            s.push('6');
                            core_consumed_six = true;
                        }
                    }
                    SeventhQuality::Minor => {
                        s.push_str(&stack_text(stack, ""));
                        core_stack = stack;
                    }
                    SeventhQuality::Major | SeventhQuality::AugmentedMajor => {
                        s.push_str(&stack_text(stack, "Maj"));
                        core_stack = stack;
                    }
                    SeventhQuality::Diminished => s.push_str("(bb7)"),
                }
            }
            TriadQuality::Major | TriadQuality::Omitted => match self.seventh {
                SeventhQuality::None => {
                    if six_nine {
                        s.push_str("6/9");
                        core_consumed_six = true;
                        core_consumed_nine = true;
                    } else if sixth {
                        s.push('6');
                        core_consumed_six = true;
                    }
                }
                SeventhQuality::Minor => {
                    s.push_str(&stack_text(stack, ""));
                    core_stack = stack;
                }
                SeventhQuality::Major | SeventhQuality::AugmentedMajor => {
                    s.push_str(&stack_text(stack, "maj"));
                    core_stack = stack;
                }
                SeventhQuality::Diminished => s.push_str("(bb7)"),
            },
        }

        if self.alt_dominant {
            s.push_str("alt");
        }

        // Extensions the core could not express, e.g. a 13 with no 9 below it.
        let top = core_stack.unwrap_or(0);
        let mut leftover: Vec<ChordDegree> = self
            .extensions
            .iter()
            .copied()
            .filter(|e| e.alter != 0 || e.number > top || !matches!(e.number, 9 | 11 | 13))
            .collect();
        leftover.sort_unstable();
        for e in leftover {
            s.push('(');
            s.push_str(&format!("{}{}", acc(Accidental(e.alter)), e.number));
            s.push(')');
        }

        let mut alterations = self.alterations.clone();
        alterations.sort_unstable();
        for a in alterations {
            s.push_str(&format!("{}{}", acc(Accidental(a.alter)), a.number));
        }

        let mut added = self.added.clone();
        added.sort_unstable();
        for a in added {
            if core_consumed_six && a == ChordDegree::new(6, 0) {
                continue;
            }
            if core_consumed_nine && a == ChordDegree::new(9, 0) {
                continue;
            }
            s.push_str("add");
            s.push_str(&format!("{}{}", acc(Accidental(a.alter)), a.number));
        }

        let mut omissions = self.omissions.clone();
        omissions.sort_unstable();
        for o in omissions {
            s.push_str(&format!("no{o}"));
        }

        if let Some((l, a)) = self.bass {
            s.push('/');
            s.push(l.as_char());
            s.push_str(&acc(a));
        }
        s
    }

    /// The highest stacked extension whose whole stack is present: 13 needs 9
    /// and 11 below it, 11 needs 9. `None` when there is no complete stack.
    fn stack_top(&self) -> Option<u8> {
        let has = |n: u8| {
            self.extensions
                .iter()
                .any(|e| e.number == n && e.alter == 0)
        };
        if has(9) && has(11) && has(13) {
            Some(13)
        } else if has(9) && has(11) {
            Some(11)
        } else if has(9) {
            Some(9)
        } else {
            None
        }
    }

    /// JSON form of the chord's semantic identity.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "root" => class_text(self.root),
            "triad" => self.triad.id(),
            "seventh" => self.seventh.id(),
            "extensions" => degree_array(&self.extensions),
            "added" => degree_array(&self.added),
            "alterations" => degree_array(&self.alterations),
            "omissions" => Json::Arr(self.omissions.iter().map(|o| Json::Int(*o as i64)).collect()),
            "bass" => match self.bass { Some(b) => Json::Str(class_text(b)), None => Json::Null },
            "alt_dominant" => self.alt_dominant,
            "symbol" => self.render_ascii(),
        }
    }

    /// Reads the JSON form produced by [`ChordSpec::to_json`].
    pub fn from_json(v: &Json) -> Result<ChordSpec, DomainError> {
        let root_text = v.str_field("root")?;
        let root = SpelledPitch::parse_class(root_text)
            .ok_or_else(|| DomainError::invalid_chord_symbol(format!("bad root {root_text:?}")))?;
        let triad_text = v.str_field("triad")?;
        let triad = TriadQuality::parse(triad_text).ok_or_else(|| {
            DomainError::invalid_chord_symbol(format!("bad triad quality {triad_text:?}"))
        })?;
        let seventh_text = v.str_field("seventh")?;
        let seventh = SeventhQuality::parse(seventh_text).ok_or_else(|| {
            DomainError::invalid_chord_symbol(format!("bad seventh quality {seventh_text:?}"))
        })?;
        let bass = match v.get("bass") {
            Some(Json::Str(s)) => Some(SpelledPitch::parse_class(s).ok_or_else(|| {
                DomainError::invalid_chord_symbol(format!("bad bass note {s:?}"))
            })?),
            _ => None,
        };
        let mut omissions = Vec::new();
        if let Some(Json::Arr(a)) = v.get("omissions") {
            for o in a {
                if let Some(n) = o.as_i64() {
                    omissions.push(n.clamp(0, 255) as u8);
                }
            }
        }
        let mut spec = ChordSpec {
            root,
            triad,
            seventh,
            extensions: degrees_from_json(v, "extensions")?,
            added: degrees_from_json(v, "added")?,
            alterations: degrees_from_json(v, "alterations")?,
            omissions,
            bass,
            alt_dominant: v.opt_bool_field("alt_dominant")?.unwrap_or(false),
        };
        spec.normalize();
        Ok(spec)
    }
}

/// Renders the extension core: `""`/`"maj"` plus `7`, `9`, `11` or `13`.
fn stack_text(stack: Option<u8>, prefix: &str) -> String {
    match stack {
        Some(n) => format!("{prefix}{n}"),
        None => format!("{prefix}7"),
    }
}

/// Renders a spelled pitch class such as `"Bb"`.
fn class_text((l, a): (Letter, Accidental)) -> String {
    format!("{}{}", l.as_char(), a.ascii())
}

/// Renders a degree list as a JSON array of strings.
fn degree_array(v: &[ChordDegree]) -> Json {
    Json::Arr(v.iter().map(|d| Json::Str(d.to_string())).collect())
}

/// Reads a degree list from a JSON array of strings.
fn degrees_from_json(v: &Json, key: &str) -> Result<Vec<ChordDegree>, DomainError> {
    let mut out = Vec::new();
    if let Some(Json::Arr(a)) = v.get(key) {
        for d in a {
            let text = d.as_str().ok_or_else(|| {
                DomainError::invalid_chord_symbol(format!("non-string degree in {key:?}"))
            })?;
            out.push(ChordDegree::parse(text).ok_or_else(|| {
                DomainError::invalid_chord_symbol(format!("bad degree {text:?} in {key:?}"))
            })?);
        }
    }
    Ok(out)
}

impl fmt::Display for ChordSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render_ascii())
    }
}

/// How a chord behaves in its key.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum HarmonicFunction {
    /// Tonic region.
    Tonic,
    /// Predominant region.
    Predominant,
    /// Dominant region.
    Dominant,
    /// Applied (secondary) dominant.
    Applied,
    /// Chromatic, outside the diatonic functions.
    Chromatic,
    /// Modal colour rather than functional motion.
    Modal,
    /// Pedal harmony.
    Pedal,
    /// Passing harmony.
    Passing,
    /// Neighbour harmony.
    Neighbor,
    /// Not classified.
    Unclassified,
}

impl HarmonicFunction {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            HarmonicFunction::Tonic => "tonic",
            HarmonicFunction::Predominant => "predominant",
            HarmonicFunction::Dominant => "dominant",
            HarmonicFunction::Applied => "applied",
            HarmonicFunction::Chromatic => "chromatic",
            HarmonicFunction::Modal => "modal",
            HarmonicFunction::Pedal => "pedal",
            HarmonicFunction::Passing => "passing",
            HarmonicFunction::Neighbor => "neighbor",
            HarmonicFunction::Unclassified => "unclassified",
        }
    }

    /// Parses the identifier produced by [`HarmonicFunction::id`].
    pub fn parse(s: &str) -> Option<Self> {
        HarmonicFunction::all()
            .iter()
            .copied()
            .find(|f| f.id() == s)
    }

    /// Every variant, in declaration order.
    pub fn all() -> &'static [HarmonicFunction] {
        &[
            HarmonicFunction::Tonic,
            HarmonicFunction::Predominant,
            HarmonicFunction::Dominant,
            HarmonicFunction::Applied,
            HarmonicFunction::Chromatic,
            HarmonicFunction::Modal,
            HarmonicFunction::Pedal,
            HarmonicFunction::Passing,
            HarmonicFunction::Neighbor,
            HarmonicFunction::Unclassified,
        ]
    }
}

/// A voicing family — how the chord tones are distributed.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum VoicingFamily {
    /// Close position.
    #[default]
    Close,
    /// Open position.
    Open,
    /// Drop the second voice from the top an octave.
    Drop2,
    /// Drop the third voice from the top an octave.
    Drop3,
    /// Guide-tone shell.
    Shell,
    /// Rootless.
    Rootless,
    /// Widely spread.
    Spread,
    /// Stacked fourths.
    Quartal,
    /// Stacked fifths.
    Quintal,
    /// Cluster.
    Cluster,
    /// Root and fifth only.
    Power,
    /// Upper structure over a lower voice.
    UpperStructure,
    /// Sustained pedal.
    Pedal,
}

impl VoicingFamily {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            VoicingFamily::Close => "close",
            VoicingFamily::Open => "open",
            VoicingFamily::Drop2 => "drop2",
            VoicingFamily::Drop3 => "drop3",
            VoicingFamily::Shell => "shell",
            VoicingFamily::Rootless => "rootless",
            VoicingFamily::Spread => "spread",
            VoicingFamily::Quartal => "quartal",
            VoicingFamily::Quintal => "quintal",
            VoicingFamily::Cluster => "cluster",
            VoicingFamily::Power => "power",
            VoicingFamily::UpperStructure => "upper_structure",
            VoicingFamily::Pedal => "pedal",
        }
    }

    /// Parses the identifier produced by [`VoicingFamily::id`].
    pub fn parse(s: &str) -> Option<Self> {
        VoicingFamily::all().iter().copied().find(|f| f.id() == s)
    }

    /// Every variant, in declaration order.
    pub fn all() -> &'static [VoicingFamily] {
        &[
            VoicingFamily::Close,
            VoicingFamily::Open,
            VoicingFamily::Drop2,
            VoicingFamily::Drop3,
            VoicingFamily::Shell,
            VoicingFamily::Rootless,
            VoicingFamily::Spread,
            VoicingFamily::Quartal,
            VoicingFamily::Quintal,
            VoicingFamily::Cluster,
            VoicingFamily::Power,
            VoicingFamily::UpperStructure,
            VoicingFamily::Pedal,
        ]
    }
}

/// A realized set of sounding pitches for a chord.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Voicing {
    /// Pitches from lowest to highest.
    pub pitches: Vec<SpelledPitch>,
    /// Which voicing family this realization belongs to.
    pub family: VoicingFamily,
    /// Voice identity per pitch, parallel to `pitches` when assigned.
    pub voices: Vec<VoiceId>,
}

impl Voicing {
    /// Builds a close voicing from pitches.
    pub fn new(pitches: Vec<SpelledPitch>, family: VoicingFamily) -> Voicing {
        Voicing {
            pitches,
            family,
            voices: Vec::new(),
        }
    }

    /// Lowest sounding pitch.
    pub fn bottom(&self) -> Option<SpelledPitch> {
        self.pitches.iter().copied().min()
    }

    /// Highest sounding pitch.
    pub fn top(&self) -> Option<SpelledPitch> {
        self.pitches.iter().copied().max()
    }

    /// Distance in semitones between the lowest and highest pitch.
    pub fn span_semitones(&self) -> i32 {
        match (self.bottom(), self.top()) {
            (Some(b), Some(t)) => t.midi() - b.midi(),
            _ => 0,
        }
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "pitches" => Json::Arr(self.pitches.iter().map(|p| Json::Str(p.to_ascii())).collect()),
            "family" => self.family.id(),
            "voices" => Json::Arr(self.voices.iter().map(|v| Json::Int(v.0 as i64)).collect()),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<Voicing, DomainError> {
        let mut pitches = Vec::new();
        for p in v.arr_field("pitches")? {
            let text = p
                .as_str()
                .ok_or_else(|| DomainError::invalid_pitch("non-string pitch in voicing"))?;
            pitches.push(
                SpelledPitch::parse(text)
                    .ok_or_else(|| DomainError::invalid_pitch(format!("bad pitch {text:?}")))?,
            );
        }
        let family_text = v.str_field("family")?;
        let family = VoicingFamily::parse(family_text).ok_or_else(|| {
            DomainError::invalid_argument(format!("bad voicing family {family_text:?}"))
        })?;
        let mut voices = Vec::new();
        if let Some(Json::Arr(a)) = v.get("voices") {
            for x in a {
                if let Some(n) = x.as_i64() {
                    voices.push(VoiceId(n.clamp(0, u16::MAX as i64) as u16));
                }
            }
        }
        Ok(Voicing {
            pitches,
            family,
            voices,
        })
    }
}

/// A chord placed in time, with analysis and an optional realization.
#[derive(Clone, Debug)]
pub struct ChordEvent {
    /// Stable id within the analysis or candidate.
    pub id: u32,
    /// Semantic chord identity.
    pub spec: ChordSpec,
    /// Onset in quarter notes.
    pub onset: BeatTime,
    /// Duration in quarter notes.
    pub duration: BeatTime,
    /// Inversion number: 0 root position, 1 first inversion, …
    pub inversion: u8,
    /// Functional label, when analysis assigned one.
    pub function: Option<HarmonicFunction>,
    /// Roman-numeral label, when analysis assigned one.
    pub roman: Option<String>,
    /// Local tonic and scale id, when analysis assigned one.
    pub local_tonic: Option<((Letter, Accidental), String)>,
    /// Realized voicing, when one has been generated.
    pub voicing: Option<Voicing>,
    /// Confidence in this chord, `0.0..=1.0`.
    pub confidence: f64,
    /// Where the chord came from: `"parsed_symbol"`, `"detected"`,
    /// `"generated"` or `"user"`.
    pub inference_source: String,
    /// The symbol text as originally written, when there was one.
    pub original_symbol: Option<String>,
}

impl ChordEvent {
    /// Builds a chord event from a parsed symbol.
    pub fn new(id: u32, spec: ChordSpec, onset: BeatTime, duration: BeatTime) -> ChordEvent {
        ChordEvent {
            id,
            spec,
            onset,
            duration,
            inversion: 0,
            function: None,
            roman: None,
            local_tonic: None,
            voicing: None,
            confidence: 1.0,
            inference_source: "parsed_symbol".to_string(),
            original_symbol: None,
        }
    }

    /// End position of the chord.
    pub fn end(&self) -> BeatTime {
        self.onset + self.duration
    }

    /// True when `qn` falls inside `[onset, end)`.
    pub fn contains(&self, qn: BeatTime) -> bool {
        qn >= self.onset && qn < self.end()
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "id" => self.id as i64,
            "spec" => self.spec.to_json(),
            "symbol" => self.spec.render_ascii(),
            "onset" => self.onset.to_json(),
            "duration" => self.duration.to_json(),
            "inversion" => self.inversion as i64,
            "function" => match self.function { Some(f) => Json::Str(f.id().to_string()), None => Json::Null },
            "roman" => match &self.roman { Some(r) => Json::Str(r.clone()), None => Json::Null },
            "local_tonic" => match &self.local_tonic {
                Some((t, s)) => json_obj! { "tonic" => class_text(*t), "scale_id" => s.clone() },
                None => Json::Null,
            },
            "voicing" => match &self.voicing { Some(v) => v.to_json(), None => Json::Null },
            "confidence" => self.confidence,
            "inference_source" => self.inference_source.clone(),
            "original_symbol" => match &self.original_symbol { Some(s) => Json::Str(s.clone()), None => Json::Null },
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<ChordEvent, DomainError> {
        let function = match v.get("function") {
            Some(Json::Str(s)) => Some(HarmonicFunction::parse(s).ok_or_else(|| {
                DomainError::invalid_argument(format!("bad harmonic function {s:?}"))
            })?),
            _ => None,
        };
        let local_tonic = match v.get("local_tonic") {
            Some(o @ Json::Obj(_)) => {
                let t = o.str_field("tonic")?;
                let tonic = SpelledPitch::parse_class(t)
                    .ok_or_else(|| DomainError::invalid_pitch(format!("bad tonic {t:?}")))?;
                Some((tonic, o.str_field("scale_id")?.to_string()))
            }
            _ => None,
        };
        let voicing = match v.get("voicing") {
            Some(o @ Json::Obj(_)) => Some(Voicing::from_json(o)?),
            _ => None,
        };
        Ok(ChordEvent {
            id: v
                .opt_i64_field("id")?
                .unwrap_or(0)
                .clamp(0, u32::MAX as i64) as u32,
            spec: ChordSpec::from_json(v.field("spec")?)?,
            onset: BeatTime::from_json(v.field("onset")?)?,
            duration: BeatTime::from_json(v.field("duration")?)?,
            inversion: v.opt_i64_field("inversion")?.unwrap_or(0).clamp(0, 255) as u8,
            function,
            roman: v.opt_str_field("roman")?.map(|s| s.to_string()),
            local_tonic,
            voicing,
            confidence: v.opt_f64_field("confidence")?.unwrap_or(1.0),
            inference_source: v
                .opt_str_field("inference_source")?
                .unwrap_or("parsed_symbol")
                .to_string(),
            original_symbol: v.opt_str_field("original_symbol")?.map(|s| s.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol;

    fn c() -> (Letter, Accidental) {
        (Letter::C, Accidental::NATURAL)
    }

    fn pcs(s: &str) -> Vec<i32> {
        symbol::parse(s).expect(s).pitch_classes()
    }

    #[test]
    fn degree_parsing() {
        assert_eq!(ChordDegree::parse("b9"), Some(ChordDegree::new(9, -1)));
        assert_eq!(ChordDegree::parse("#11"), Some(ChordDegree::new(11, 1)));
        assert_eq!(ChordDegree::parse("13"), Some(ChordDegree::new(13, 0)));
        assert_eq!(ChordDegree::parse("b5"), Some(ChordDegree::new(5, -1)));
        assert_eq!(ChordDegree::parse("♭9"), Some(ChordDegree::new(9, -1)));
        assert_eq!(ChordDegree::parse("♯11"), Some(ChordDegree::new(11, 1)));
        assert_eq!(ChordDegree::parse("-9"), Some(ChordDegree::new(9, -1)));
        assert_eq!(ChordDegree::parse("+9"), Some(ChordDegree::new(9, 1)));
        assert_eq!(ChordDegree::parse("bb7"), Some(ChordDegree::new(7, -2)));
        assert!(ChordDegree::parse("").is_none());
        assert!(ChordDegree::parse("b").is_none());
        assert!(ChordDegree::parse("14").is_none());
        assert!(ChordDegree::parse("0").is_none());
        assert!(ChordDegree::parse("b9x").is_none());
    }

    #[test]
    fn degree_rendering() {
        assert_eq!(ChordDegree::new(9, -1).to_string(), "b9");
        assert_eq!(ChordDegree::new(11, 1).to_string(), "#11");
        assert_eq!(ChordDegree::new(13, 0).to_string(), "13");
        assert_eq!(ChordDegree::new(9, -1).to_unicode(), "♭9");
    }

    #[test]
    fn degree_semitones() {
        assert_eq!(ChordDegree::new(1, 0).semitones_from_root(), 0);
        assert_eq!(ChordDegree::new(3, -1).semitones_from_root(), 3);
        assert_eq!(ChordDegree::new(5, 0).semitones_from_root(), 7);
        assert_eq!(ChordDegree::new(7, -1).semitones_from_root(), 10);
        assert_eq!(ChordDegree::new(9, 0).semitones_from_root(), 14);
        assert_eq!(ChordDegree::new(9, -1).semitones_from_root(), 13);
        assert_eq!(ChordDegree::new(11, 1).semitones_from_root(), 18);
        assert_eq!(ChordDegree::new(13, 0).semitones_from_root(), 21);
        assert_eq!(ChordDegree::new(13, 0).simple_semitones(), 9);
        assert_eq!(ChordDegree::new(9, 0).simple_semitones(), 2);
    }

    #[test]
    fn degree_intervals() {
        assert_eq!(ChordDegree::new(3, 0).interval().name(), "M3");
        assert_eq!(ChordDegree::new(3, -1).interval().name(), "m3");
        assert_eq!(ChordDegree::new(5, -1).interval().name(), "d5");
        assert_eq!(ChordDegree::new(5, 1).interval().name(), "A5");
        assert_eq!(ChordDegree::new(7, -2).interval().name(), "d7");
        assert_eq!(ChordDegree::new(9, -1).interval().name(), "m9");
        assert_eq!(ChordDegree::new(11, 1).interval().name(), "A11");
        for n in 1..=13u8 {
            for a in -1..=1i8 {
                let d = ChordDegree::new(n, a);
                assert_eq!(d.interval().semitones(), d.semitones_from_root());
            }
        }
    }

    #[test]
    fn quality_ids_round_trip() {
        for t in TriadQuality::all() {
            assert_eq!(TriadQuality::parse(t.id()), Some(*t));
        }
        for s in [
            SeventhQuality::None,
            SeventhQuality::Major,
            SeventhQuality::Minor,
            SeventhQuality::Diminished,
            SeventhQuality::AugmentedMajor,
        ] {
            assert_eq!(SeventhQuality::parse(s.id()), Some(s));
        }
        assert!(TriadQuality::parse("nope").is_none());
        assert!(SeventhQuality::parse("nope").is_none());
        assert_eq!(SeventhQuality::Minor.alter(), Some(-1));
        assert_eq!(SeventhQuality::None.alter(), None);
        assert!(SeventhQuality::Major.is_present());
        assert!(!SeventhQuality::None.is_present());
    }

    #[test]
    fn triad_pitch_classes() {
        assert_eq!(
            ChordSpec::triad(c(), TriadQuality::Major).pitch_classes(),
            vec![0, 4, 7]
        );
        assert_eq!(
            ChordSpec::triad(c(), TriadQuality::Minor).pitch_classes(),
            vec![0, 3, 7]
        );
        assert_eq!(
            ChordSpec::triad(c(), TriadQuality::Diminished).pitch_classes(),
            vec![0, 3, 6]
        );
        assert_eq!(
            ChordSpec::triad(c(), TriadQuality::Augmented).pitch_classes(),
            vec![0, 4, 8]
        );
        assert_eq!(
            ChordSpec::triad(c(), TriadQuality::Sus4).pitch_classes(),
            vec![0, 5, 7]
        );
        assert_eq!(
            ChordSpec::triad(c(), TriadQuality::Sus2).pitch_classes(),
            vec![0, 2, 7]
        );
        assert_eq!(
            ChordSpec::triad(c(), TriadQuality::Power).pitch_classes(),
            vec![0, 7]
        );
    }

    #[test]
    fn golden_chord_shapes_from_the_brief() {
        // C E G — C major.
        assert_eq!(pcs("C"), vec![0, 4, 7]);
        // C E G Bb D — C9.
        assert_eq!(pcs("C9"), vec![0, 4, 7, 10, 2]);
        // C E G D — Cadd9, not C9: no seventh.
        assert_eq!(pcs("Cadd9"), vec![0, 4, 7, 2]);
        assert!(!symbol::parse("Cadd9").unwrap().seventh.is_present());
        assert!(symbol::parse("C9").unwrap().seventh.is_present());
        // C E G A — C6, not C13.
        assert_eq!(pcs("C6"), vec![0, 4, 7, 9]);
        assert!(!symbol::parse("C6").unwrap().seventh.is_present());
        assert!(symbol::parse("C13").unwrap().seventh.is_present());
        // C F G — Csus4, not C11.
        assert_eq!(pcs("Csus4"), vec![0, 5, 7]);
        assert!(symbol::parse("Csus4").unwrap().is_suspended());
        assert!(!symbol::parse("C11").unwrap().is_suspended());
        // C E G B — Cmaj7.
        assert_eq!(pcs("Cmaj7"), vec![0, 4, 7, 11]);
        // C Eb G B — CmMaj7.
        assert_eq!(pcs("CmMaj7"), vec![0, 3, 7, 11]);
        // C Eb G Bb — Cm7.
        assert_eq!(pcs("Cm7"), vec![0, 3, 7, 10]);
        // B D F A — B half-diminished seventh.
        let bm7b5 = symbol::parse("Bm7b5").unwrap();
        assert_eq!(bm7b5.pitch_classes(), vec![11, 2, 5, 9]);
        assert_eq!(bm7b5.family_id(), "half_dim");
        // Fully diminished seventh keeps its spelling.
        let bdim7 = symbol::parse("Bdim7").unwrap();
        assert_eq!(bdim7.pitch_classes(), vec![11, 2, 5, 8]);
        let names: Vec<String> = bdim7
            .chord_tones()
            .iter()
            .map(|(_, (l, a))| format!("{}{}", l.as_char(), a.ascii()))
            .collect();
        assert_eq!(names, ["B", "D", "F", "Ab"]);
    }

    #[test]
    fn seventh_chord_spelling_is_letter_correct() {
        let spelled = |s: &str| -> Vec<String> {
            symbol::parse(s)
                .expect(s)
                .chord_tones()
                .iter()
                .map(|(_, (l, a))| format!("{}{}", l.as_char(), a.ascii()))
                .collect()
        };
        assert_eq!(spelled("Cmaj7"), ["C", "E", "G", "B"]);
        assert_eq!(spelled("CmMaj7"), ["C", "Eb", "G", "B"]);
        assert_eq!(spelled("Cm7"), ["C", "Eb", "G", "Bb"]);
        assert_eq!(spelled("C7"), ["C", "E", "G", "Bb"]);
        assert_eq!(spelled("Cdim7"), ["C", "Eb", "Gb", "Bbb"]);
        assert_eq!(spelled("Cm7b5"), ["C", "Eb", "Gb", "Bb"]);
        assert_eq!(spelled("Caug"), ["C", "E", "G#"]);
        assert_eq!(spelled("Ebmaj7"), ["Eb", "G", "Bb", "D"]);
        assert_eq!(spelled("F#7"), ["F#", "A#", "C#", "E"]);
        assert_eq!(spelled("Gb7"), ["Gb", "Bb", "Db", "Fb"]);
    }

    #[test]
    fn enharmonic_roots_differ_in_spelling_not_in_sound() {
        let fs = symbol::parse("F#7").unwrap();
        let gb = symbol::parse("Gb7").unwrap();
        assert_ne!(fs, gb);
        assert_eq!(fs.root_pc(), gb.root_pc());
        assert_eq!(fs.pitch_classes(), gb.pitch_classes());
        assert_eq!(fs.render_ascii(), "F#7");
        assert_eq!(gb.render_ascii(), "Gb7");
    }

    #[test]
    fn extension_stacks() {
        assert_eq!(pcs("C9"), vec![0, 4, 7, 10, 2]);
        assert_eq!(pcs("C11"), vec![0, 4, 7, 10, 2, 5]);
        assert_eq!(pcs("C13"), vec![0, 4, 7, 10, 2, 5, 9]);
        assert_eq!(pcs("Cmaj9"), vec![0, 4, 7, 11, 2]);
        assert_eq!(pcs("Cm9"), vec![0, 3, 7, 10, 2]);
        assert_eq!(pcs("Cm11"), vec![0, 3, 7, 10, 2, 5]);
    }

    #[test]
    fn alterations_replace_natural_extensions() {
        let c13b9 = symbol::parse("C13b9").unwrap();
        let tones: Vec<String> = c13b9
            .chord_tones()
            .iter()
            .map(|(d, _)| d.to_string())
            .collect();
        assert!(tones.contains(&"b9".to_string()));
        assert!(!tones.contains(&"9".to_string()));
        assert!(tones.contains(&"13".to_string()));
    }

    #[test]
    fn omissions_do_not_change_identity() {
        let full = symbol::parse("C13").unwrap();
        let thinned = symbol::parse("C13no5").unwrap();
        assert_eq!(full.family_id(), thinned.family_id());
        assert_eq!(full.seventh, thinned.seventh);
        assert_eq!(full.extensions, thinned.extensions);
        assert!(!thinned.pitch_classes().contains(&7));
        assert!(full.pitch_classes().contains(&7));
        let no_root = symbol::parse("C7no1").unwrap();
        assert!(!no_root.pitch_classes().contains(&0));
        assert_eq!(no_root.root_pc(), 0);
        assert!(no_root.is_dominant_family());
    }

    #[test]
    fn families() {
        assert_eq!(symbol::parse("C").unwrap().family_id(), "major");
        assert_eq!(symbol::parse("Cmaj7").unwrap().family_id(), "major");
        assert_eq!(symbol::parse("C6").unwrap().family_id(), "major");
        assert_eq!(symbol::parse("Cm").unwrap().family_id(), "minor");
        assert_eq!(symbol::parse("CmMaj7").unwrap().family_id(), "minor");
        assert_eq!(symbol::parse("C7").unwrap().family_id(), "dominant");
        assert_eq!(symbol::parse("C13").unwrap().family_id(), "dominant");
        assert_eq!(symbol::parse("Cm7b5").unwrap().family_id(), "half_dim");
        assert_eq!(symbol::parse("Cdim7").unwrap().family_id(), "dim");
        assert_eq!(symbol::parse("Cdim").unwrap().family_id(), "dim");
        assert_eq!(symbol::parse("Csus4").unwrap().family_id(), "sus");
        assert_eq!(symbol::parse("C7sus4").unwrap().family_id(), "sus");
        assert_eq!(symbol::parse("C5").unwrap().family_id(), "power");
        assert_eq!(symbol::parse("Caug").unwrap().family_id(), "aug");
        assert_eq!(symbol::parse("Caug7").unwrap().family_id(), "dominant");
        assert_eq!(symbol::parse("C7alt").unwrap().family_id(), "dominant");
    }

    #[test]
    fn family_predicates() {
        let c7 = symbol::parse("C7").unwrap();
        assert!(c7.is_dominant_family());
        assert!(!c7.is_major_family());
        assert!(symbol::parse("Cmaj7").unwrap().is_major_family());
        assert!(symbol::parse("Cm9").unwrap().is_minor_family());
        assert!(symbol::parse("Cm7b5").unwrap().is_diminished_family());
        assert!(symbol::parse("Csus2").unwrap().is_suspended());
        assert!(symbol::parse("C7sus4").unwrap().is_dominant_family());
        assert!(symbol::parse("Calt").unwrap().is_dominant_family());
    }

    #[test]
    fn guide_tones_are_third_and_seventh() {
        let g = |s: &str| -> Vec<String> {
            symbol::parse(s)
                .unwrap()
                .guide_tones()
                .iter()
                .map(|d| d.to_string())
                .collect()
        };
        assert_eq!(g("C7"), ["3", "b7"]);
        assert_eq!(g("Cm7"), ["b3", "b7"]);
        assert_eq!(g("Cmaj7"), ["3", "7"]);
        assert_eq!(g("C6"), ["3", "6"]);
        assert_eq!(g("Csus4"), ["4"]);
        assert_eq!(g("C7sus4"), ["4", "b7"]);
        assert_eq!(g("C7no3"), ["b7"]);
    }

    #[test]
    fn degree_lookup_and_membership() {
        let c13 = symbol::parse("C13").unwrap();
        assert!(c13.has_degree(13));
        assert!(c13.has_degree(7));
        assert!(!c13.has_degree(6));
        assert_eq!(c13.degree_of_pc(10), Some(ChordDegree::new(7, -1)));
        assert_eq!(c13.degree_of_pc(1), None);
        assert_eq!(
            symbol::parse("C").unwrap().degree_of_pc(4).unwrap().number,
            3
        );
    }

    #[test]
    fn slash_bass_is_preserved() {
        let c_over_e = symbol::parse("C/E").unwrap();
        assert_eq!(c_over_e.bass_pc(), 4);
        assert_eq!(c_over_e.root_pc(), 0);
        assert_eq!(c_over_e.render_ascii(), "C/E");
        let plain = symbol::parse("C").unwrap();
        assert_eq!(plain.bass_pc(), plain.root_pc());
        let d_over_bb = symbol::parse("Dm7/Bb").unwrap();
        assert_eq!(d_over_bb.bass_pc(), 10);
    }

    #[test]
    fn transposition_keeps_relationships() {
        let c7 = symbol::parse("C7").unwrap();
        let e7 = c7.transpose(Interval::M3);
        assert_eq!(e7.render_ascii(), "E7");
        assert_eq!(e7.pitch_classes(), vec![4, 8, 11, 2]);
        let slash = symbol::parse("C/E").unwrap().transpose(Interval::P5);
        assert_eq!(slash.render_ascii(), "G/B");
        let flat_side = symbol::parse("Bb7").unwrap().transpose(Interval::P4);
        assert_eq!(flat_side.render_ascii(), "Eb7");
    }

    #[test]
    fn chord_spec_json_round_trip() {
        for s in [
            "C", "Cm7", "Cmaj9", "C13b9", "C7#9b13", "Cadd9", "C6/9", "Csus4", "C/E", "Cdim7",
            "Cm7b5", "C7alt", "C5", "C7no3",
        ] {
            let spec = symbol::parse(s).expect(s);
            let back = ChordSpec::from_json(&spec.to_json()).expect(s);
            assert_eq!(back, spec, "{s}");
        }
    }

    #[test]
    fn chord_spec_json_rejects_garbage() {
        let bad = json_obj! { "root" => "H", "triad" => "major", "seventh" => "none" };
        assert!(ChordSpec::from_json(&bad).is_err());
        let bad2 = json_obj! { "root" => "C", "triad" => "weird", "seventh" => "none" };
        assert!(ChordSpec::from_json(&bad2).is_err());
    }

    #[test]
    fn harmonic_function_ids() {
        for f in HarmonicFunction::all() {
            assert_eq!(HarmonicFunction::parse(f.id()), Some(*f));
        }
        assert!(HarmonicFunction::parse("nope").is_none());
    }

    #[test]
    fn voicing_family_ids() {
        for f in VoicingFamily::all() {
            assert_eq!(VoicingFamily::parse(f.id()), Some(*f));
        }
        assert!(VoicingFamily::parse("nope").is_none());
        assert_eq!(VoicingFamily::default(), VoicingFamily::Close);
        assert_eq!(VoicingFamily::all().len(), 13);
    }

    #[test]
    fn voicing_geometry_and_json() {
        let v = Voicing::new(
            vec![
                SpelledPitch::parse("C3").unwrap(),
                SpelledPitch::parse("E4").unwrap(),
                SpelledPitch::parse("Bb4").unwrap(),
            ],
            VoicingFamily::Shell,
        );
        assert_eq!(v.bottom().unwrap().to_ascii(), "C3");
        assert_eq!(v.top().unwrap().to_ascii(), "Bb4");
        assert_eq!(v.span_semitones(), 22);
        let back = Voicing::from_json(&v.to_json()).expect("round trip");
        assert_eq!(back, v);
        assert_eq!(Voicing::default().span_semitones(), 0);
    }

    #[test]
    fn chord_event_json_round_trip() {
        let mut ev = ChordEvent::new(
            7,
            symbol::parse("Dm7").unwrap(),
            BeatTime::from_quarters(4),
            BeatTime::from_quarters(4),
        );
        ev.function = Some(HarmonicFunction::Predominant);
        ev.roman = Some("ii7".to_string());
        ev.local_tonic = Some(((Letter::C, Accidental::NATURAL), "major".to_string()));
        ev.voicing = Some(Voicing::new(
            vec![SpelledPitch::parse("D3").unwrap()],
            VoicingFamily::Close,
        ));
        ev.original_symbol = Some("Dm7".to_string());
        ev.confidence = 0.75;
        let back = ChordEvent::from_json(&ev.to_json()).expect("round trip");
        assert_eq!(back.id, ev.id);
        assert_eq!(back.spec, ev.spec);
        assert_eq!(back.onset, ev.onset);
        assert_eq!(back.duration, ev.duration);
        assert_eq!(back.function, ev.function);
        assert_eq!(back.roman, ev.roman);
        assert_eq!(back.local_tonic, ev.local_tonic);
        assert_eq!(back.voicing, ev.voicing);
        assert_eq!(back.confidence, ev.confidence);
        assert_eq!(back.original_symbol, ev.original_symbol);
    }

    #[test]
    fn chord_event_time_helpers() {
        let ev = ChordEvent::new(
            0,
            symbol::parse("C").unwrap(),
            BeatTime::from_quarters(4),
            BeatTime::from_quarters(4),
        );
        assert_eq!(ev.end(), BeatTime::from_quarters(8));
        assert!(ev.contains(BeatTime::from_quarters(4)));
        assert!(ev.contains(BeatTime::from_quarters(7)));
        assert!(!ev.contains(BeatTime::from_quarters(8)));
        assert!(!ev.contains(BeatTime::from_quarters(3)));
    }

    #[test]
    fn display_uses_the_canonical_symbol() {
        assert_eq!(symbol::parse("Cmaj7").unwrap().to_string(), "Cmaj7");
        assert_eq!(symbol::parse("Cmaj7").unwrap().render_unicode(), "Cmaj7");
        assert_eq!(symbol::parse("C#m7b5").unwrap().render_unicode(), "C♯m7♭5");
    }

    #[test]
    fn normalize_sorts_and_dedups() {
        let mut spec = ChordSpec::triad(c(), TriadQuality::Major);
        spec.extensions = vec![ChordDegree::new(13, 0), ChordDegree::new(9, 0)];
        spec.added = vec![ChordDegree::new(9, 0), ChordDegree::new(9, 0)];
        spec.omissions = vec![5, 3, 5];
        spec.normalize();
        assert_eq!(spec.extensions[0].number, 9);
        assert_eq!(spec.added.len(), 1);
        assert_eq!(spec.omissions, vec![3, 5]);
    }
}
