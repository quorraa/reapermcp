//! Chord-symbol parsing: a recursive-descent parser from lead-sheet text to a
//! semantic [`ChordSpec`].
//!
//! # Grammar
//!
//! ```text
//! symbol      := root modifier* EOF
//! root        := letter accidental*
//! letter      := 'A'..'G' | 'a'..'g'
//! accidental  := '#' | 'b' | 'x' | '♯' | '♭' | '𝄪' | '𝄫' | '♮'
//!
//! modifier    := group | slash | alteration | alias | number | separator
//! group       := '(' modifier* ')'          -- nested up to MAX_GROUP_DEPTH
//! separator   := ' ' | ','
//! slash       := '/' ( digits | root )      -- "6/9" vs a slash bass
//! alteration  := altprefix digits           -- "b9", "#11", "-13", "+5", "♭9"
//! altprefix   := 'b' | '#' | '♭' | '♯' | '-' | '+'
//! alias       := one of the literal entries in CHORD_ALIASES
//! number      := digits                     -- 2 4 5 6 7 9 11 13
//! ```
//!
//! # Semantics
//!
//! * A bare `7`, `9`, `11` or `13` implies a **minor seventh** (a diminished
//!   seventh over a diminished triad) and, outside parentheses, the whole stack
//!   below it: `C11` carries 9 and 11, `C13` carries 9, 11 and 13. Inside
//!   parentheses a number adds only that one degree, so `C7(13)` is a seventh
//!   chord with an added thirteenth and no ninth.
//! * `add` never implies a seventh: `Cadd9` is a triad with a ninth, `C9` is a
//!   dominant ninth. `Caddb9` and `C7b9` are likewise distinct.
//! * `6` is an addition, not an extension: `C6` has no seventh, `C13` does.
//! * `sus2` and `sus4` *replace* the third; `add2` and `add4` keep it.
//! * `no3` / `no5` / `omit3` record omissions. Omissions never change the
//!   chord's semantic identity, only which tones a voicing will sound.
//! * `alt` sets [`ChordSpec::alt_dominant`]. It names an altered-dominant
//!   **family**; the concrete tensions are chosen downstream by the harmony
//!   engine from melody, destination, style and voice leading. This parser
//!   deliberately does not fix them.
//! * `m7b5` normalizes to a diminished triad with a minor seventh, so a
//!   half-diminished chord has one canonical representation however it was
//!   written (`Cm7b5`, `Cø`, `Cø7`, `C-7b5`).
//!
//! Rendering is the inverse: `parse(render_ascii(parse(s))) == parse(s)` for
//! every symbol the parser accepts.
//!
//! # Alias table
//!
//! Every literal alias lives in one place, [`CHORD_ALIASES`], so it can be
//! cross-checked against `knowledge/chord_symbols.json` (which is authored
//! separately and carries the same `precedence` field). Matching is
//! longest-first; `precedence` records which spelling is canonical when several
//! mean the same thing. Purely numeric behaviour (`7`, `9`, `11`, `13`, `6`)
//! is grammar rather than alias, and is documented above.

use crate::chord::{ChordDegree, ChordSpec, SeventhQuality, TriadQuality};
use crate::error::DomainError;
use crate::pitch::{Accidental, Letter};
use qjson::{json_obj, Json};
use std::fmt;

/// What a literal alias means to the parser.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AliasKind {
    /// A major-quality marker. When a number follows it names the *seventh*
    /// (`maj7`, `maj9`); when nothing follows it names the triad, unless
    /// `bare_is_seventh` is set — `Δ` alone means a major seventh.
    MajorMarker {
        /// True when the bare marker already implies a major seventh.
        bare_is_seventh: bool,
    },
    /// Minor triad.
    MinorMarker,
    /// Diminished triad; a following `7` becomes a diminished seventh.
    DiminishedMarker,
    /// Half-diminished: diminished triad plus minor seventh.
    HalfDiminishedMarker,
    /// Augmented triad.
    AugmentedMarker,
    /// Suspended chord; the payload is the replacing degree (2 or 4).
    SusMarker(u8),
    /// `add` — the next degree is an addition.
    AddMarker,
    /// `no` / `omit` — the next number is an omission.
    OmitMarker,
    /// `alt` — altered-dominant family.
    AlteredMarker,
    /// `6/9` and its compressed spelling.
    SixNine,
    /// Power chord (root and fifth).
    PowerMarker,
}

/// One entry of the chord-symbol alias table.
#[derive(Copy, Clone, Debug)]
pub struct ChordAlias {
    /// Literal text as it appears in a symbol.
    pub text: &'static str,
    /// What it means.
    pub kind: AliasKind,
    /// 1 = canonical spelling, 2 = common variant, 3 = rare variant.
    pub precedence: u8,
}

/// The authoritative alias table for chord-symbol parsing.
///
/// Matching is longest-first, so `maj7` wins over `ma` and `omit` over `o`.
/// Case is significant: `M` is major, `m` is minor.
pub const CHORD_ALIASES: &[ChordAlias] = &[
    // Major-quality markers.
    ChordAlias {
        text: "maj",
        kind: AliasKind::MajorMarker {
            bare_is_seventh: false,
        },
        precedence: 1,
    },
    ChordAlias {
        text: "Maj",
        kind: AliasKind::MajorMarker {
            bare_is_seventh: false,
        },
        precedence: 1,
    },
    ChordAlias {
        text: "MAJ",
        kind: AliasKind::MajorMarker {
            bare_is_seventh: false,
        },
        precedence: 3,
    },
    ChordAlias {
        text: "ma",
        kind: AliasKind::MajorMarker {
            bare_is_seventh: false,
        },
        precedence: 2,
    },
    ChordAlias {
        text: "M",
        kind: AliasKind::MajorMarker {
            bare_is_seventh: false,
        },
        precedence: 1,
    },
    ChordAlias {
        text: "Δ",
        kind: AliasKind::MajorMarker {
            bare_is_seventh: true,
        },
        precedence: 1,
    },
    ChordAlias {
        text: "^",
        kind: AliasKind::MajorMarker {
            bare_is_seventh: true,
        },
        precedence: 3,
    },
    // Minor.
    ChordAlias {
        text: "min",
        kind: AliasKind::MinorMarker,
        precedence: 1,
    },
    ChordAlias {
        text: "Min",
        kind: AliasKind::MinorMarker,
        precedence: 2,
    },
    ChordAlias {
        text: "MIN",
        kind: AliasKind::MinorMarker,
        precedence: 3,
    },
    ChordAlias {
        text: "mi",
        kind: AliasKind::MinorMarker,
        precedence: 2,
    },
    ChordAlias {
        text: "m",
        kind: AliasKind::MinorMarker,
        precedence: 1,
    },
    ChordAlias {
        text: "-",
        kind: AliasKind::MinorMarker,
        precedence: 2,
    },
    // Diminished.
    ChordAlias {
        text: "dim",
        kind: AliasKind::DiminishedMarker,
        precedence: 1,
    },
    ChordAlias {
        text: "Dim",
        kind: AliasKind::DiminishedMarker,
        precedence: 2,
    },
    ChordAlias {
        text: "°",
        kind: AliasKind::DiminishedMarker,
        precedence: 1,
    },
    ChordAlias {
        text: "o",
        kind: AliasKind::DiminishedMarker,
        precedence: 3,
    },
    // Half-diminished.
    ChordAlias {
        text: "ø7",
        kind: AliasKind::HalfDiminishedMarker,
        precedence: 1,
    },
    ChordAlias {
        text: "Ø7",
        kind: AliasKind::HalfDiminishedMarker,
        precedence: 2,
    },
    ChordAlias {
        text: "ø",
        kind: AliasKind::HalfDiminishedMarker,
        precedence: 1,
    },
    ChordAlias {
        text: "Ø",
        kind: AliasKind::HalfDiminishedMarker,
        precedence: 2,
    },
    ChordAlias {
        text: "%",
        kind: AliasKind::HalfDiminishedMarker,
        precedence: 3,
    },
    // Augmented.
    ChordAlias {
        text: "aug",
        kind: AliasKind::AugmentedMarker,
        precedence: 1,
    },
    ChordAlias {
        text: "Aug",
        kind: AliasKind::AugmentedMarker,
        precedence: 2,
    },
    ChordAlias {
        text: "+",
        kind: AliasKind::AugmentedMarker,
        precedence: 2,
    },
    // Suspensions.
    ChordAlias {
        text: "sus2",
        kind: AliasKind::SusMarker(2),
        precedence: 1,
    },
    ChordAlias {
        text: "sus4",
        kind: AliasKind::SusMarker(4),
        precedence: 1,
    },
    ChordAlias {
        text: "sus9",
        kind: AliasKind::SusMarker(2),
        precedence: 3,
    },
    ChordAlias {
        text: "sus",
        kind: AliasKind::SusMarker(4),
        precedence: 2,
    },
    // Additions and omissions.
    ChordAlias {
        text: "add",
        kind: AliasKind::AddMarker,
        precedence: 1,
    },
    ChordAlias {
        text: "omit",
        kind: AliasKind::OmitMarker,
        precedence: 2,
    },
    ChordAlias {
        text: "no",
        kind: AliasKind::OmitMarker,
        precedence: 1,
    },
    // Altered dominant.
    ChordAlias {
        text: "alt",
        kind: AliasKind::AlteredMarker,
        precedence: 1,
    },
    // Six-nine and power.
    ChordAlias {
        text: "6/9",
        kind: AliasKind::SixNine,
        precedence: 1,
    },
    ChordAlias {
        text: "69",
        kind: AliasKind::SixNine,
        precedence: 2,
    },
    ChordAlias {
        text: "5",
        kind: AliasKind::PowerMarker,
        precedence: 1,
    },
];

/// Maximum nesting of parenthesised groups before the parser gives up.
const MAX_GROUP_DEPTH: usize = 8;

/// Aliases that only apply when a degree number follows them.
///
/// `ma` means major in `Cma7`, but `Cmadd9` is a minor chord with an added
/// ninth — the `m` is the quality and `add9` is the addition. Requiring a digit
/// resolves that overlap in favour of the reading players expect.
const DIGIT_ONLY_ALIASES: &[&str] = &["ma"];

/// Numbers that a bare `-`/`+` prefix may alter. `-7` is the minor-seventh
/// spelling `C-7`, not a lowered seventh, so 7 is deliberately absent.
const SIGNED_ALTERABLE: [u32; 5] = [5, 6, 9, 11, 13];

/// A chord symbol that could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolError {
    /// The symbol as given.
    pub input: String,
    /// Character offset where parsing failed.
    pub position: usize,
    /// What went wrong.
    pub message: String,
}

impl SymbolError {
    /// Builds a parse error.
    pub fn new(input: &str, position: usize, message: impl Into<String>) -> SymbolError {
        SymbolError {
            input: input.to_string(),
            position,
            message: message.into(),
        }
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "input" => self.input.clone(),
            "position" => self.position as i64,
            "message" => self.message.clone(),
        }
    }
}

impl fmt::Display for SymbolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot parse chord symbol {:?} at position {}: {}",
            self.input, self.position, self.message
        )
    }
}

impl std::error::Error for SymbolError {}

impl From<SymbolError> for DomainError {
    fn from(e: SymbolError) -> Self {
        DomainError::invalid_chord_symbol(e.to_string())
    }
}

/// Parses a chord symbol into its semantic identity.
///
/// See the module documentation for the grammar and the semantic rules.
pub fn parse(s: &str) -> Result<ChordSpec, SymbolError> {
    let mut p = Parser::new(s);
    p.parse_root()?;
    p.modifiers(false)?;
    Ok(p.finish())
}

/// Parses a chord symbol, returning a [`DomainError`] instead of a
/// [`SymbolError`]. Convenience for callers that only speak `DomainError`.
pub fn parse_domain(s: &str) -> Result<ChordSpec, DomainError> {
    parse(s).map_err(DomainError::from)
}

/// Recursive-descent parser state.
struct Parser<'a> {
    input: &'a str,
    chars: Vec<char>,
    pos: usize,
    depth: usize,
    root: (Letter, Accidental),
    triad: Option<TriadQuality>,
    sus: Option<u8>,
    power: bool,
    seventh: SeventhQuality,
    seventh_set: bool,
    extensions: Vec<ChordDegree>,
    added: Vec<ChordDegree>,
    alterations: Vec<ChordDegree>,
    omissions: Vec<u8>,
    bass: Option<(Letter, Accidental)>,
    alt: bool,
}

impl<'a> Parser<'a> {
    /// Prepares a parser over `input`, trimming surrounding whitespace.
    fn new(input: &'a str) -> Parser<'a> {
        Parser {
            input,
            chars: input.trim().chars().collect(),
            pos: 0,
            depth: 0,
            root: (Letter::C, Accidental::NATURAL),
            triad: None,
            sus: None,
            power: false,
            seventh: SeventhQuality::None,
            seventh_set: false,
            extensions: Vec::new(),
            added: Vec::new(),
            alterations: Vec::new(),
            omissions: Vec::new(),
            bass: None,
            alt: false,
        }
    }

    /// Builds an error at the current position.
    fn err(&self, message: impl Into<String>) -> SymbolError {
        SymbolError::new(self.input, self.pos, message)
    }

    /// True when every character has been consumed.
    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    /// The character at the cursor, if any.
    fn cur(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// The character `n` positions ahead, if any.
    fn ahead(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    /// Reads the root note and its accidentals.
    fn parse_root(&mut self) -> Result<(), SymbolError> {
        let c = self.cur().ok_or_else(|| self.err("empty chord symbol"))?;
        let letter =
            Letter::from_char(c).ok_or_else(|| self.err(format!("{c:?} is not a note letter")))?;
        self.pos += 1;
        let mut alter = 0i32;
        while let Some(c) = self.cur() {
            let step = match c {
                'b' | '♭' => -1,
                '#' | '♯' => 1,
                'x' | '𝄪' => 2,
                '𝄫' => -2,
                '♮' => 0,
                _ => break,
            };
            // A trailing 'b' or '#' that introduces an alteration belongs to the
            // quality, not to the root: "Cb9" is a C-flat ninth, but "C(b9)" and
            // "C7b9" are not affected because the digit follows a quality token.
            alter += step;
            self.pos += 1;
        }
        if !(-4..=4).contains(&alter) {
            return Err(self.err("root accidental is out of range"));
        }
        self.root = (letter, Accidental(alter as i8));
        Ok(())
    }

    /// Consumes a run of ASCII digits, if present.
    fn take_digits(&mut self) -> Option<u32> {
        let start = self.pos;
        while self.cur().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        text.parse().ok()
    }

    /// Peeks a run of digits without consuming it.
    fn peek_digits(&self, offset: usize) -> Option<u32> {
        let mut i = self.pos + offset;
        let start = i;
        while self
            .chars
            .get(i)
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            i += 1;
        }
        if i == start {
            return None;
        }
        let text: String = self.chars[start..i].iter().collect();
        text.parse().ok()
    }

    /// Longest alias whose text starts at the cursor.
    fn match_alias(&self) -> Option<ChordAlias> {
        let rest: String = self.chars[self.pos..].iter().collect();
        let mut best: Option<ChordAlias> = None;
        for a in CHORD_ALIASES {
            if rest.starts_with(a.text) {
                let len = a.text.chars().count();
                if DIGIT_ONLY_ALIASES.contains(&a.text)
                    && !self.ahead(len).map(|c| c.is_ascii_digit()).unwrap_or(false)
                {
                    continue;
                }
                let better = match &best {
                    Some(b) => len > b.text.chars().count(),
                    None => true,
                };
                if better {
                    best = Some(*a);
                }
            }
        }
        best
    }

    /// True once a triad or seventh has been named.
    ///
    /// Before that point a bare `-` or `+` is a minor or augmented marker
    /// (`C-9` is a minor ninth); afterwards it is an alteration sign
    /// (`C7-9` is a flat ninth).
    fn quality_started(&self) -> bool {
        self.triad.is_some() || self.seventh_set || self.power || self.sus.is_some()
    }

    /// The seventh quality a bare `7`/`9`/`11`/`13` implies here.
    fn implied_seventh(&self) -> SeventhQuality {
        if self.triad == Some(TriadQuality::Diminished) {
            SeventhQuality::Diminished
        } else {
            SeventhQuality::Minor
        }
    }

    /// Sets the seventh unless one was already named.
    fn set_seventh_if_unset(&mut self, q: SeventhQuality) {
        if !self.seventh_set {
            self.seventh = q;
            self.seventh_set = true;
        }
    }

    /// Sets the seventh, overriding any earlier value.
    fn force_seventh(&mut self, q: SeventhQuality) {
        self.seventh = q;
        self.seventh_set = true;
    }

    /// Records an extension. Outside parentheses the whole stack below `n` is
    /// implied; inside parentheses only `n` itself.
    fn push_stack(&mut self, n: u8, single: bool) {
        let mut push = |d: u8| {
            let deg = ChordDegree::new(d, 0);
            if !self.extensions.contains(&deg) {
                self.extensions.push(deg);
            }
        };
        if single {
            push(n);
            return;
        }
        match n {
            13 => {
                push(9);
                push(11);
                push(13);
            }
            11 => {
                push(9);
                push(11);
            }
            _ => push(9),
        }
    }

    /// Records an added degree.
    fn push_added(&mut self, d: ChordDegree) {
        if !self.added.contains(&d) {
            self.added.push(d);
        }
    }

    /// Records an alteration.
    fn push_alteration(&mut self, d: ChordDegree) {
        if !self.alterations.contains(&d) {
            self.alterations.push(d);
        }
    }

    /// The modifier loop. `in_group` is true inside parentheses, where a
    /// closing `)` ends the loop and numbers do not imply a whole stack.
    fn modifiers(&mut self, in_group: bool) -> Result<(), SymbolError> {
        loop {
            while matches!(self.cur(), Some(' ') | Some(',') | Some('\t')) {
                self.pos += 1;
            }
            if self.at_end() {
                if in_group {
                    return Err(self.err("unterminated '('"));
                }
                return Ok(());
            }
            let c = self.cur().unwrap_or(' ');
            match c {
                ')' => {
                    if in_group {
                        self.pos += 1;
                        return Ok(());
                    }
                    return Err(self.err("unmatched ')'"));
                }
                '(' => {
                    if self.depth + 1 > MAX_GROUP_DEPTH {
                        return Err(self.err("chord symbol nests too deeply"));
                    }
                    self.pos += 1;
                    self.depth += 1;
                    self.modifiers(true)?;
                    self.depth -= 1;
                }
                '/' => self.slash()?,
                _ => self.modifier_token(in_group)?,
            }
        }
    }

    /// Parses one non-structural modifier.
    fn modifier_token(&mut self, in_group: bool) -> Result<(), SymbolError> {
        let c = self.cur().unwrap_or(' ');
        // A signed alteration such as "-9" or "+11". "-7" is the minor-seventh
        // spelling, so only the degrees in SIGNED_ALTERABLE take this path.
        if matches!(c, '-' | '+') && self.quality_started() {
            if let Some(n) = self.peek_digits(1) {
                if SIGNED_ALTERABLE.contains(&n) {
                    self.pos += 1;
                    let alter = if c == '-' { -1 } else { 1 };
                    let digits = self.take_digits().unwrap_or(0);
                    return self.apply_degree_token(digits, alter);
                }
            }
        }
        // An accidental-prefixed degree such as "b9", "#11", "bb7", "♭13".
        if matches!(c, 'b' | '#' | '♭' | '♯' | '𝄫' | '𝄪') {
            let mut offset = 0usize;
            let mut alter = 0i32;
            while let Some(ch) = self.ahead(offset) {
                let step = match ch {
                    'b' | '♭' => -1,
                    '#' | '♯' => 1,
                    '𝄫' => -2,
                    '𝄪' => 2,
                    _ => break,
                };
                alter += step;
                offset += 1;
            }
            if let Some(n) = self.peek_digits(offset) {
                self.pos += offset;
                let digits = self.take_digits().unwrap_or(n);
                return self.apply_degree_token(digits, alter);
            }
        }
        if let Some(alias) = self.match_alias() {
            self.pos += alias.text.chars().count();
            return self.apply_alias(alias, in_group);
        }
        if c.is_ascii_digit() {
            let n = self.take_digits().unwrap_or(0);
            return self.apply_number(n, in_group);
        }
        Err(self.err(format!("unexpected {c:?} in chord symbol")))
    }

    /// Applies a degree written with an explicit alteration.
    fn apply_degree_token(&mut self, number: u32, alter: i32) -> Result<(), SymbolError> {
        if !(1..=13).contains(&number) {
            return Err(self.err(format!("degree {number} is out of range")));
        }
        if !(-2..=2).contains(&alter) {
            return Err(self.err("alteration is out of range"));
        }
        let degree = ChordDegree::new(number as u8, alter as i8);
        match number {
            7 => {
                let q = match alter {
                    0 => SeventhQuality::Major,
                    -1 => SeventhQuality::Minor,
                    -2 => SeventhQuality::Diminished,
                    _ => return Err(self.err("a seventh cannot be raised")),
                };
                self.force_seventh(q);
            }
            5 | 9 | 11 | 13 => self.push_alteration(degree),
            2 | 4 | 6 => self.push_added(degree),
            _ => return Err(self.err(format!("degree {number} cannot be altered here"))),
        }
        Ok(())
    }

    /// Applies a bare number.
    fn apply_number(&mut self, n: u32, in_group: bool) -> Result<(), SymbolError> {
        match n {
            5 => {
                if self.triad.is_some() || self.seventh_set {
                    return Err(self.err("'5' must stand alone as a power chord"));
                }
                self.power = true;
            }
            6 => {
                self.push_added(ChordDegree::new(6, 0));
                // "6/9" and "69" also reach here when written after a quality.
                if self.cur() == Some('9') {
                    self.pos += 1;
                    self.push_added(ChordDegree::new(9, 0));
                } else if self.cur() == Some('/') && self.ahead(1) == Some('9') {
                    self.pos += 2;
                    self.push_added(ChordDegree::new(9, 0));
                }
            }
            7 => {
                let q = self.implied_seventh();
                self.set_seventh_if_unset(q);
            }
            9 | 11 | 13 => {
                let q = self.implied_seventh();
                self.set_seventh_if_unset(q);
                self.push_stack(n as u8, in_group);
            }
            2 | 4 => self.push_added(ChordDegree::new(n as u8, 0)),
            _ => return Err(self.err(format!("unexpected number {n} in chord symbol"))),
        }
        Ok(())
    }

    /// Applies a literal alias.
    fn apply_alias(&mut self, alias: ChordAlias, in_group: bool) -> Result<(), SymbolError> {
        match alias.kind {
            AliasKind::MajorMarker { bare_is_seventh } => match self.take_digits() {
                Some(6) => {
                    self.push_added(ChordDegree::new(6, 0));
                    if self.cur() == Some('9') {
                        self.pos += 1;
                        self.push_added(ChordDegree::new(9, 0));
                    }
                }
                Some(7) => self.force_seventh(SeventhQuality::Major),
                Some(n @ (9 | 11 | 13)) => {
                    self.force_seventh(SeventhQuality::Major);
                    self.push_stack(n as u8, in_group);
                }
                Some(n) => {
                    return Err(self.err(format!("'{}{}' is not a chord quality", alias.text, n)))
                }
                None => {
                    if bare_is_seventh {
                        self.force_seventh(SeventhQuality::Major);
                    } else if self.triad.is_none() {
                        self.triad = Some(TriadQuality::Major);
                    }
                }
            },
            AliasKind::MinorMarker => {
                if self.triad.is_none() {
                    self.triad = Some(TriadQuality::Minor);
                }
            }
            AliasKind::DiminishedMarker => self.triad = Some(TriadQuality::Diminished),
            AliasKind::HalfDiminishedMarker => {
                self.triad = Some(TriadQuality::Diminished);
                self.force_seventh(SeventhQuality::Minor);
            }
            AliasKind::AugmentedMarker => self.triad = Some(TriadQuality::Augmented),
            AliasKind::SusMarker(n) => self.sus = Some(n),
            AliasKind::AddMarker => {
                let (number, alter) = self.read_degree_after_marker("add")?;
                self.push_added(ChordDegree::new(number, alter));
            }
            AliasKind::OmitMarker => {
                let n = self
                    .take_digits()
                    .ok_or_else(|| self.err("'no' must be followed by a degree number"))?;
                if !(1..=13).contains(&n) {
                    return Err(self.err(format!("cannot omit degree {n}")));
                }
                if !self.omissions.contains(&(n as u8)) {
                    self.omissions.push(n as u8);
                }
            }
            AliasKind::AlteredMarker => {
                self.alt = true;
                if !self.seventh_set {
                    self.force_seventh(SeventhQuality::Minor);
                }
            }
            AliasKind::SixNine => {
                self.push_added(ChordDegree::new(6, 0));
                self.push_added(ChordDegree::new(9, 0));
            }
            AliasKind::PowerMarker => {
                if self.triad.is_some() || self.seventh_set {
                    return Err(self.err("'5' must stand alone as a power chord"));
                }
                self.power = true;
            }
        }
        Ok(())
    }

    /// Reads the degree that follows `add`, allowing an accidental prefix.
    fn read_degree_after_marker(&mut self, marker: &str) -> Result<(u8, i8), SymbolError> {
        let mut alter = 0i32;
        while let Some(c) = self.cur() {
            let step = match c {
                'b' | '♭' | '-' => -1,
                '#' | '♯' | '+' => 1,
                '𝄫' => -2,
                '𝄪' => 2,
                _ => break,
            };
            alter += step;
            self.pos += 1;
        }
        let n = self
            .take_digits()
            .ok_or_else(|| self.err(format!("'{marker}' must be followed by a degree number")))?;
        if !(1..=13).contains(&n) {
            return Err(self.err(format!("degree {n} is out of range")));
        }
        if !(-2..=2).contains(&alter) {
            return Err(self.err("alteration is out of range"));
        }
        Ok((n as u8, alter as i8))
    }

    /// Handles `/`: either the `6/9` tail or a slash bass.
    fn slash(&mut self) -> Result<(), SymbolError> {
        self.pos += 1;
        if let Some(n) = self.peek_digits(0) {
            let digits = self.take_digits().unwrap_or(n);
            if !(1..=13).contains(&digits) {
                return Err(self.err(format!("degree {digits} is out of range")));
            }
            self.push_added(ChordDegree::new(digits as u8, 0));
            return Ok(());
        }
        let c = self
            .cur()
            .ok_or_else(|| self.err("'/' must be followed by a bass note"))?;
        let letter = Letter::from_char(c)
            .ok_or_else(|| self.err(format!("{c:?} is not a bass note letter")))?;
        self.pos += 1;
        let mut alter = 0i32;
        while let Some(c) = self.cur() {
            let step = match c {
                'b' | '♭' => -1,
                '#' | '♯' => 1,
                'x' | '𝄪' => 2,
                '𝄫' => -2,
                '♮' => 0,
                _ => break,
            };
            alter += step;
            self.pos += 1;
        }
        if !(-2..=2).contains(&alter) {
            return Err(self.err("bass accidental is out of range"));
        }
        while matches!(self.cur(), Some(' ') | Some('\t')) {
            self.pos += 1;
        }
        if !self.at_end() {
            let c = self.cur().unwrap_or(' ');
            return Err(self.err(format!("unexpected {c:?} after the bass note")));
        }
        self.bass = Some((letter, Accidental(alter as i8)));
        Ok(())
    }

    /// Resolves defaults and normalizes into a [`ChordSpec`].
    fn finish(self) -> ChordSpec {
        let mut triad = self.triad.unwrap_or(TriadQuality::Major);
        if self.power {
            triad = TriadQuality::Power;
        }
        if let Some(n) = self.sus {
            triad = if n == 2 {
                TriadQuality::Sus2
            } else {
                TriadQuality::Sus4
            };
        }
        let mut alterations = self.alterations;
        // "m7b5" and friends are half-diminished chords, not minor chords with
        // a lowered fifth; normalizing here gives them one representation.
        if triad == TriadQuality::Minor {
            if let Some(i) = alterations
                .iter()
                .position(|d| *d == ChordDegree::new(5, -1))
            {
                alterations.remove(i);
                triad = TriadQuality::Diminished;
            }
        }
        let mut spec = ChordSpec {
            root: self.root,
            triad,
            seventh: self.seventh,
            extensions: self.extensions,
            added: self.added,
            alterations,
            omissions: self.omissions,
            bass: self.bass,
            alt_dominant: self.alt,
        };
        spec.normalize();
        spec
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord::SeventhQuality;

    /// The round-trip corpus: every symbol the parser is expected to accept.
    const CORPUS: &[&str] = &[
        "C",
        "Cm",
        "C-",
        "Cmin",
        "Cmi",
        "CM",
        "Cmaj",
        "CΔ",
        "Cdim",
        "C°",
        "Caug",
        "C+",
        "C5",
        "C6",
        "Cm6",
        "C6/9",
        "C69",
        "Cm6/9",
        "Cadd9",
        "Cadd2",
        "Cadd4",
        "Caddb9",
        "Cmadd9",
        "Cmadd4",
        "C7",
        "Cmaj7",
        "CM7",
        "CΔ7",
        "Cma7",
        "Cm7",
        "Cmin7",
        "C-7",
        "CmMaj7",
        "CminMaj7",
        "Cm(maj7)",
        "CmΔ7",
        "Cdim7",
        "C°7",
        "Cm7b5",
        "Cø",
        "Cø7",
        "C-7b5",
        "C9",
        "Cmaj9",
        "Cm9",
        "C11",
        "Cm11",
        "Cmaj11",
        "C13",
        "Cmaj13",
        "Cm13",
        "Csus2",
        "Csus4",
        "Csus",
        "C7sus4",
        "C9sus4",
        "C13sus4",
        "Cmaj7sus4",
        "C7sus2",
        "C7b5",
        "C7#5",
        "C7b9",
        "C7#9",
        "C7#11",
        "C7b13",
        "C7b9b13",
        "C7#9#11",
        "C7b9#11",
        "C7-9",
        "C7+9",
        "C7alt",
        "Calt",
        "C13b9",
        "C13#11",
        "Cmaj7#11",
        "Cmaj9#11",
        "Cm7add11",
        "Cm11b5",
        "C7no3",
        "C7no5",
        "Cno3",
        "Comit5",
        "C7(no3)",
        "C7(b9)",
        "C7(#9,b13)",
        "C(add9)",
        "Cmaj7(9)",
        "C7(13)",
        "C/E",
        "C/G",
        "Cm7/Bb",
        "Cmaj7/B",
        "C6/9/E",
        "F#7",
        "Gb7",
        "Bb7",
        "Ebmaj7",
        "Abm7",
        "D#m7b5",
        "C♯m7",
        "D♭maj7",
        "F♯m7♭5",
        "A7b9",
        "Dm7",
        "G13",
        "E7#9",
        "Bm7b5",
        "F°7",
        "Bbmaj9",
        "Ab13",
        "Caug7",
        "CaugMaj7",
        "C+7",
        "G7sus4",
        "Cmb6",
        "Bbb7",
        "Fx7",
        "C-9",
        "Cm7b5add11",
        "Cmaj6/9",
        "C7(#11)",
        "Cadd#11",
        "C%",
        "CØ7",
        "Cmin7b5",
        "Co7",
        "C^7",
        "Cm^7",
        "CMAJ7",
        "Cmaj6",
        "Cm(add9)",
        "C7(9)",
        "Cdim(9)",
        "Csus9",
        "C13sus2",
    ];

    fn spec(s: &str) -> ChordSpec {
        parse(s).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn corpus_is_large_enough() {
        assert!(CORPUS.len() >= 80, "corpus has {} symbols", CORPUS.len());
    }

    #[test]
    fn every_corpus_symbol_parses() {
        for s in CORPUS {
            assert!(parse(s).is_ok(), "{s} should parse");
        }
    }

    #[test]
    fn render_ascii_round_trips_for_the_whole_corpus() {
        for s in CORPUS {
            let first = spec(s);
            let rendered = first.render_ascii();
            let second = parse(&rendered)
                .unwrap_or_else(|e| panic!("{s} rendered as {rendered:?} which failed: {e}"));
            assert_eq!(second, first, "{s} rendered as {rendered:?}");
        }
    }

    #[test]
    fn render_unicode_round_trips_for_the_whole_corpus() {
        for s in CORPUS {
            let first = spec(s);
            let rendered = first.render_unicode();
            let second = parse(&rendered)
                .unwrap_or_else(|e| panic!("{s} rendered as {rendered:?} which failed: {e}"));
            assert_eq!(second, first, "{s} rendered as {rendered:?}");
        }
    }

    #[test]
    fn rendering_is_idempotent() {
        for s in CORPUS {
            let once = spec(s).render_ascii();
            let twice = spec(&once).render_ascii();
            assert_eq!(once, twice, "{s}");
        }
    }

    #[test]
    fn basic_triads() {
        assert_eq!(spec("C").triad, TriadQuality::Major);
        assert_eq!(spec("Cm").triad, TriadQuality::Minor);
        assert_eq!(spec("Cmin").triad, TriadQuality::Minor);
        assert_eq!(spec("C-").triad, TriadQuality::Minor);
        assert_eq!(spec("Cdim").triad, TriadQuality::Diminished);
        assert_eq!(spec("C°").triad, TriadQuality::Diminished);
        assert_eq!(spec("Caug").triad, TriadQuality::Augmented);
        assert_eq!(spec("C+").triad, TriadQuality::Augmented);
        assert_eq!(spec("C5").triad, TriadQuality::Power);
        assert_eq!(spec("Csus2").triad, TriadQuality::Sus2);
        assert_eq!(spec("Csus4").triad, TriadQuality::Sus4);
        assert_eq!(spec("Csus").triad, TriadQuality::Sus4);
    }

    #[test]
    fn roots_and_accidentals() {
        assert_eq!(spec("F#7").root, (Letter::F, Accidental::SHARP));
        assert_eq!(spec("Gb7").root, (Letter::G, Accidental::FLAT));
        assert_eq!(spec("C♯m7").root, (Letter::C, Accidental::SHARP));
        assert_eq!(spec("D♭maj7").root, (Letter::D, Accidental::FLAT));
        assert_eq!(spec("Bbb7").root, (Letter::B, Accidental::DOUBLE_FLAT));
        assert_eq!(spec("Fx7").root, (Letter::F, Accidental::DOUBLE_SHARP));
        assert_eq!(spec("c").root, (Letter::C, Accidental::NATURAL));
    }

    #[test]
    fn add9_is_not_nine() {
        let add9 = spec("Cadd9");
        let nine = spec("C9");
        assert_ne!(add9, nine);
        assert_eq!(add9.seventh, SeventhQuality::None);
        assert_eq!(add9.added, vec![ChordDegree::new(9, 0)]);
        assert!(add9.extensions.is_empty());
        assert_eq!(nine.seventh, SeventhQuality::Minor);
        assert_eq!(nine.extensions, vec![ChordDegree::new(9, 0)]);
        assert!(nine.added.is_empty());
    }

    #[test]
    fn six_is_not_thirteen() {
        let six = spec("C6");
        let thirteen = spec("C13");
        assert_ne!(six, thirteen);
        assert_eq!(six.seventh, SeventhQuality::None);
        assert_eq!(six.added, vec![ChordDegree::new(6, 0)]);
        assert_eq!(thirteen.seventh, SeventhQuality::Minor);
        assert_eq!(
            thirteen.extensions,
            vec![
                ChordDegree::new(9, 0),
                ChordDegree::new(11, 0),
                ChordDegree::new(13, 0)
            ]
        );
    }

    #[test]
    fn sus4_is_not_eleven() {
        let sus = spec("Csus4");
        let eleven = spec("C11");
        assert_ne!(sus, eleven);
        assert!(sus.is_suspended());
        assert!(!sus.has_degree(3));
        assert_eq!(sus.seventh, SeventhQuality::None);
        assert!(!eleven.is_suspended());
        assert!(eleven.has_degree(3));
        assert!(eleven.has_degree(11));
        assert_eq!(eleven.seventh, SeventhQuality::Minor);
    }

    #[test]
    fn sus_replaces_the_third_and_add_keeps_it() {
        assert!(!spec("Csus2").has_degree(3));
        assert!(!spec("Csus4").has_degree(3));
        assert!(spec("Cadd2").has_degree(3));
        assert!(spec("Cadd4").has_degree(3));
        assert!(spec("Cadd9").has_degree(3));
        assert!(spec("Cadd2").has_degree(2));
        assert!(spec("Cadd4").has_degree(4));
    }

    #[test]
    fn slash_bass_is_an_inversion_not_a_root_change() {
        let s = spec("C/E");
        assert_eq!(s.root, (Letter::C, Accidental::NATURAL));
        assert_eq!(s.bass, Some((Letter::E, Accidental::NATURAL)));
        assert_eq!(s.root_pc(), 0);
        assert_eq!(s.bass_pc(), 4);
        assert_eq!(spec("Cm7/Bb").bass, Some((Letter::B, Accidental::FLAT)));
        assert_eq!(spec("C6/9/E").bass, Some((Letter::E, Accidental::NATURAL)));
        assert_eq!(spec("C6/9/E").added.len(), 2);
    }

    #[test]
    fn sharp_and_flat_roots_stay_distinct() {
        let fs = spec("F#7");
        let gb = spec("Gb7");
        assert_ne!(fs, gb);
        assert_eq!(fs.root_pc(), gb.root_pc());
        assert_eq!(fs.render_ascii(), "F#7");
        assert_eq!(gb.render_ascii(), "Gb7");
    }

    #[test]
    fn minor_major_seventh_is_not_minor_seventh() {
        let mmaj = spec("CmMaj7");
        let m7 = spec("Cm7");
        assert_ne!(mmaj, m7);
        assert_eq!(mmaj.triad, TriadQuality::Minor);
        assert_eq!(mmaj.seventh, SeventhQuality::Major);
        assert_eq!(m7.seventh, SeventhQuality::Minor);
        for alias in ["CmMaj7", "CminMaj7", "Cm(maj7)", "CmΔ7", "Cm^7"] {
            assert_eq!(spec(alias), mmaj, "{alias}");
        }
    }

    #[test]
    fn flat_nine_alteration_versus_added_flat_nine() {
        let altered = spec("C7b9");
        let added = spec("Caddb9");
        assert_ne!(altered, added);
        assert_eq!(altered.seventh, SeventhQuality::Minor);
        assert_eq!(altered.alterations, vec![ChordDegree::new(9, -1)]);
        assert!(altered.added.is_empty());
        assert_eq!(added.seventh, SeventhQuality::None);
        assert_eq!(added.added, vec![ChordDegree::new(9, -1)]);
        assert!(added.alterations.is_empty());
    }

    #[test]
    fn omissions_are_recorded_without_changing_identity() {
        let s = spec("C7no3");
        assert_eq!(s.omissions, vec![3]);
        assert_eq!(s.seventh, SeventhQuality::Minor);
        assert_eq!(s.family_id(), "dominant");
        assert_eq!(spec("C7no5").omissions, vec![5]);
        assert_eq!(spec("Comit5").omissions, vec![5]);
        assert_eq!(spec("C7(no3)").omissions, vec![3]);
        assert_eq!(spec("Cno3").triad, TriadQuality::Major);
    }

    #[test]
    fn alt_is_a_family_not_a_pitch_set() {
        let s = spec("C7alt");
        assert!(s.alt_dominant);
        assert_eq!(s.seventh, SeventhQuality::Minor);
        assert_eq!(s.triad, TriadQuality::Major);
        // No tensions are fixed by the parser.
        assert!(s.alterations.is_empty());
        assert!(s.extensions.is_empty());
        assert!(s.is_dominant_family());
        assert_eq!(spec("Calt"), s);
    }

    #[test]
    fn half_diminished_aliases_agree() {
        let canonical = spec("Cm7b5");
        assert_eq!(canonical.triad, TriadQuality::Diminished);
        assert_eq!(canonical.seventh, SeventhQuality::Minor);
        assert!(canonical.alterations.is_empty());
        for alias in ["Cø", "Cø7", "CØ7", "C-7b5", "Cmin7b5", "C%"] {
            assert_eq!(spec(alias), canonical, "{alias}");
        }
        assert_eq!(canonical.render_ascii(), "Cm7b5");
    }

    #[test]
    fn diminished_seventh_versus_diminished_triad() {
        let tri = spec("Cdim");
        let seventh = spec("Cdim7");
        assert_eq!(tri.seventh, SeventhQuality::None);
        assert_eq!(seventh.seventh, SeventhQuality::Diminished);
        assert_eq!(spec("C°7"), seventh);
        assert_eq!(spec("Co7"), seventh);
        assert_eq!(seventh.pitch_classes(), vec![0, 3, 6, 9]);
    }

    #[test]
    fn dash_seven_is_minor_seven_not_a_lowered_seventh() {
        assert_eq!(spec("C-7"), spec("Cm7"));
        assert_eq!(spec("C-"), spec("Cm"));
        assert_eq!(spec("C-9"), spec("Cm9"));
        // A lowered ninth still uses the dash form.
        assert_eq!(spec("C7-9"), spec("C7b9"));
        assert_eq!(spec("C7+9"), spec("C7#9"));
        assert_eq!(spec("C7-5"), spec("C7b5"));
    }

    #[test]
    fn plus_seven_is_an_augmented_seventh() {
        assert_eq!(spec("C+7"), spec("Caug7"));
        assert_eq!(spec("C+7").triad, TriadQuality::Augmented);
        assert_eq!(spec("C+7").seventh, SeventhQuality::Minor);
    }

    #[test]
    fn major_marker_variants() {
        let maj7 = spec("Cmaj7");
        for alias in ["CM7", "CΔ7", "CΔ", "Cma7", "CMAJ7", "C^7", "C^"] {
            assert_eq!(spec(alias), maj7, "{alias}");
        }
        assert_eq!(spec("Cmaj").triad, TriadQuality::Major);
        assert_eq!(spec("Cmaj").seventh, SeventhQuality::None);
        assert_eq!(spec("CM").seventh, SeventhQuality::None);
    }

    #[test]
    fn extension_stacks_are_implied_outside_parentheses() {
        assert_eq!(spec("C9").extensions.len(), 1);
        assert_eq!(spec("C11").extensions.len(), 2);
        assert_eq!(spec("C13").extensions.len(), 3);
        assert_eq!(spec("Cmaj13").extensions.len(), 3);
        assert_eq!(spec("Cm11").extensions.len(), 2);
        // Inside parentheses a number adds only itself.
        assert_eq!(spec("C7(13)").extensions, vec![ChordDegree::new(13, 0)]);
        assert_eq!(spec("Cmaj7(9)").extensions, vec![ChordDegree::new(9, 0)]);
        assert_eq!(spec("Cmaj7(9)").seventh, SeventhQuality::Major);
    }

    #[test]
    fn multiple_simultaneous_alterations() {
        let s = spec("C7b9b13");
        assert_eq!(
            s.alterations,
            vec![ChordDegree::new(9, -1), ChordDegree::new(13, -1)]
        );
        let t = spec("C7#9#11");
        assert_eq!(
            t.alterations,
            vec![ChordDegree::new(9, 1), ChordDegree::new(11, 1)]
        );
        let u = spec("C7(#9,b13)");
        assert_eq!(
            u.alterations,
            vec![ChordDegree::new(9, 1), ChordDegree::new(13, -1)]
        );
        assert_eq!(spec("C7#9b13"), u);
    }

    #[test]
    fn parenthesised_tensions_match_the_bare_form() {
        assert_eq!(spec("C7(b9)"), spec("C7b9"));
        assert_eq!(spec("C(add9)"), spec("Cadd9"));
        assert_eq!(spec("Cm(maj7)"), spec("CmMaj7"));
        assert_eq!(spec("C7(#11)"), spec("C7#11"));
        assert_eq!(spec("C7 (b9)"), spec("C7b9"));
    }

    #[test]
    fn unicode_and_ascii_normalize_together() {
        assert_eq!(spec("F♯m7♭5"), spec("F#m7b5"));
        assert_eq!(spec("D♭maj7"), spec("Dbmaj7"));
        assert_eq!(spec("C7♭9"), spec("C7b9"));
        assert_eq!(spec("C7♯9"), spec("C7#9"));
        assert_eq!(spec("C𝄪7").root.1, Accidental::DOUBLE_SHARP);
    }

    #[test]
    fn six_nine_forms() {
        let s = spec("C6/9");
        assert_eq!(
            s.added,
            vec![ChordDegree::new(6, 0), ChordDegree::new(9, 0)]
        );
        assert_eq!(spec("C69"), s);
        assert_eq!(spec("Cmaj6/9"), s);
        assert_eq!(s.render_ascii(), "C6/9");
        assert_eq!(spec("Cm6/9").triad, TriadQuality::Minor);
    }

    #[test]
    fn sevenths_over_suspensions() {
        let s = spec("C7sus4");
        assert_eq!(s.triad, TriadQuality::Sus4);
        assert_eq!(s.seventh, SeventhQuality::Minor);
        assert_eq!(s.render_ascii(), "C7sus4");
        assert_eq!(spec("C9sus4").extensions.len(), 1);
        assert_eq!(spec("Cmaj7sus4").seventh, SeventhQuality::Major);
        assert_eq!(spec("C7sus2").triad, TriadQuality::Sus2);
    }

    #[test]
    fn added_degrees_with_accidentals() {
        assert_eq!(spec("Caddb9").added, vec![ChordDegree::new(9, -1)]);
        assert_eq!(spec("Cadd#11").added, vec![ChordDegree::new(11, 1)]);
        assert_eq!(spec("Cmb6").added, vec![ChordDegree::new(6, -1)]);
        assert_eq!(spec("Cm7add11").added, vec![ChordDegree::new(11, 0)]);
    }

    #[test]
    fn whitespace_and_case_are_tolerated() {
        assert_eq!(spec("  Cmaj7  "), spec("Cmaj7"));
        assert_eq!(spec("c"), spec("C"));
        assert_eq!(spec("C 7"), spec("C7"));
    }

    #[test]
    fn malformed_symbols_are_rejected() {
        let bad = [
            "", "   ", "H", "Hm7", "7", "#", "Cmaj7/", "C/", "C(", "C)", "C(b9", "Cxyz", "Cadd",
            "Cno", "C13#", "C/H", "C/E7", "Cm5", "Cmaj4", "C3", "Cb", "C7b", "C(9",
        ];
        for s in bad {
            let r = parse(s);
            if s == "Cb" {
                // "Cb" is a legitimate C-flat major triad.
                assert!(r.is_ok(), "{s:?} should parse as a C-flat triad");
                continue;
            }
            assert!(r.is_err(), "{s:?} should not parse");
        }
    }

    #[test]
    fn errors_carry_input_and_position() {
        let e = parse("Cxyz").unwrap_err();
        assert_eq!(e.input, "Cxyz");
        assert!(e.position > 0);
        assert!(!e.message.is_empty());
        assert!(e.to_string().contains("Cxyz"));
        let json = e.to_json();
        assert_eq!(json.str_field("input").unwrap(), "Cxyz");
        let domain: DomainError = e.into();
        assert_eq!(domain.code, "INVALID_CHORD_SYMBOL");
    }

    #[test]
    fn empty_input_reports_position_zero() {
        let e = parse("").unwrap_err();
        assert_eq!(e.position, 0);
    }

    #[test]
    fn deep_nesting_is_rejected_rather_than_overflowing() {
        let deep = format!("C7{}b9{}", "(".repeat(40), ")".repeat(40));
        assert!(parse(&deep).is_err());
        let shallow = "C7((b9))";
        assert!(parse(shallow).is_ok());
    }

    #[test]
    fn alias_table_is_consistent() {
        let mut seen: Vec<&str> = CHORD_ALIASES.iter().map(|a| a.text).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "duplicate alias text");
        for a in CHORD_ALIASES {
            assert!(!a.text.is_empty());
            assert!((1..=3).contains(&a.precedence), "{}", a.text);
        }
        assert!(CHORD_ALIASES.iter().any(|a| a.text == "maj"));
        assert!(CHORD_ALIASES.iter().any(|a| a.text == "ø"));
        assert!(CHORD_ALIASES.iter().any(|a| a.text == "alt"));
    }

    #[test]
    fn every_alias_parses_after_a_root() {
        for a in CHORD_ALIASES {
            let text = match a.kind {
                AliasKind::AddMarker => "Cadd9".to_string(),
                AliasKind::OmitMarker => format!("C{}5", a.text),
                // Aliases that only apply before a degree number need one.
                _ if DIGIT_ONLY_ALIASES.contains(&a.text) => format!("C{}7", a.text),
                _ => format!("C{}", a.text),
            };
            assert!(parse(&text).is_ok(), "{text} should parse");
        }
    }

    #[test]
    fn parse_domain_maps_the_error_type() {
        assert!(parse_domain("Cmaj7").is_ok());
        let e = parse_domain("H").unwrap_err();
        assert_eq!(e.code, "INVALID_CHORD_SYMBOL");
    }

    #[test]
    fn fixture_symbols_all_parse() {
        for s in [
            "Am", "C", "C7", "Cmaj7", "D", "Dm7", "F", "F7", "G", "G7", "Bbmaj7", "Ebmaj7",
        ] {
            assert!(parse(s).is_ok(), "{s}");
        }
    }
}
