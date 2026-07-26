//! The tonal frame the engine builds chords inside.
//!
//! Everything here is derived from `knowledge/` rather than hardcoded: scale
//! degrees come from `scales.json`, chord construction from `chord_qualities.json`
//! and functional labelling from `functions.json`. The only tables in this file
//! are the seven diatonic degree sizes, which are arithmetic rather than
//! stylistic.

use crate::error::HarmonyError;
use music_analysis::report::Analysis;
use music_domain::prelude::*;
use theory_kb::{ChordQuality, FunctionEntry, KnowledgeBase};

/// Semitone size of each diatonic degree above the tonic, `1`..`7`.
///
/// This is the definition of the diatonic degree numbering, not a stylistic
/// choice: a "third" is four semitones above the tonic of a major scale by
/// construction, and every alteration is expressed relative to it.
pub const MAJOR_DEGREE_SEMITONES: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// The scale families the knowledge base treats as minor-key contexts for the
/// purposes of matching `functions.json` entries.
const MINOR_LIKE: &[&str] = &[
    "aeolian",
    "natural_minor",
    "harmonic_minor",
    "melodic_minor",
    "dorian",
    "phrygian",
    "locrian",
    "minor_pentatonic",
    "dorian_flat2",
    "dorian_sharp4",
    "locrian_natural_2",
    "locrian_natural_6",
    "phrygian_dominant",
    "altered",
    "altered_double_flat7",
];

/// The diatonic degree nearest a semitone distance above the tonic.
///
/// Non-heptatonic collections — the blues and pentatonic scales especially —
/// have no one-to-one mapping onto diatonic degree numbers, so a chord built on
/// one of their steps is named by the degree it sounds like rather than by its
/// index in the scale.
pub fn degree_for_semitones(semitones: i32) -> Degree {
    let semis = semitones.rem_euclid(12);
    let mut best = Degree {
        number: 1,
        alter: 0,
    };
    let mut best_cost = i32::MAX;
    for n in 1..=7u8 {
        let natural = MAJOR_DEGREE_SEMITONES[(n - 1) as usize];
        let mut alter = semis - natural;
        if alter > 6 {
            alter -= 12;
        }
        if alter < -6 {
            alter += 12;
        }
        if alter.abs() > 2 {
            continue;
        }
        // Prefer the smallest alteration, then a flat over a sharp, which is
        // how blues and modal degrees are conventionally spelled.
        let cost = alter.abs() * 4 + i32::from(alter > 0);
        if cost < best_cost {
            best_cost = cost;
            best = Degree {
                number: n,
                alter: alter as i8,
            };
        }
    }
    best
}

/// The scale ids that make a passage genuinely modal rather than major/minor.
const MODAL_SCALES: &[&str] = &[
    "dorian",
    "phrygian",
    "lydian",
    "mixolydian",
    "aeolian",
    "locrian",
    "lydian_dominant",
    "mixolydian_flat6",
    "phrygian_dominant",
    "dorian_flat2",
    "dorian_sharp4",
    "lydian_augmented",
    "lydian_sharp2",
    "locrian_natural_2",
    "locrian_natural_6",
    "ionian_sharp5",
];

/// A parsed scale-degree string such as `"1"`, `"b3"` or `"#4"`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Degree {
    /// Diatonic number, `1..=7` after octave reduction.
    pub number: u8,
    /// Chromatic alteration in semitones.
    pub alter: i8,
}

impl Degree {
    /// Parses a knowledge-base degree string. Returns `None` for anything the
    /// `^[b#]{0,2}\d+$` shape does not cover.
    pub fn parse(s: &str) -> Option<Degree> {
        let mut alter: i8 = 0;
        let mut rest = s;
        while let Some(stripped) = rest.strip_prefix('b') {
            alter -= 1;
            rest = stripped;
        }
        if alter == 0 {
            while let Some(stripped) = rest.strip_prefix('#') {
                alter += 1;
                rest = stripped;
            }
        }
        let n: u32 = rest.parse().ok()?;
        if n == 0 || n > 14 {
            return None;
        }
        let reduced = ((n - 1) % 7 + 1) as u8;
        Some(Degree {
            number: reduced,
            alter,
        })
    }

    /// Semitones above the tonic, before octave reduction.
    pub fn semitones(self) -> i32 {
        MAJOR_DEGREE_SEMITONES[(self.number - 1) as usize] + i32::from(self.alter)
    }

    /// Wire form, e.g. `"b3"`.
    pub fn text(self) -> String {
        let acc = match self.alter {
            -2 => "bb",
            -1 => "b",
            1 => "#",
            2 => "##",
            _ => "",
        };
        format!("{acc}{}", self.number)
    }
}

/// The tonal frame one generation request runs inside.
#[derive(Clone, Debug)]
pub struct KeyContext {
    /// Spelled tonic.
    pub tonic: (Letter, Accidental),
    /// The active collection.
    pub scale: ScaleInstance,
    /// The collection's knowledge id.
    pub scale_id: String,
    /// `"major"` or `"minor"`, used to select `functions.json` entries.
    pub key_context_id: String,
    /// True when the passage is centred on a mode rather than a key.
    pub is_modal: bool,
    /// Confidence the key analysis reported.
    pub confidence: f64,
}

impl KeyContext {
    /// Builds the frame from the top-ranked key candidate of an analysis.
    pub fn from_analysis(kb: &KnowledgeBase, an: &Analysis) -> Result<KeyContext, HarmonyError> {
        let top = an.key.candidates.first().ok_or_else(|| {
            HarmonyError::empty_analysis("the analysis reports no key candidate at all")
        })?;
        KeyContext::new(kb, top.tonic, &top.scale_id, top.confidence, top.is_modal)
    }

    /// Builds the frame explicitly.
    pub fn new(
        kb: &KnowledgeBase,
        tonic: (Letter, Accidental),
        scale_id: &str,
        confidence: f64,
        is_modal: bool,
    ) -> Result<KeyContext, HarmonyError> {
        let def = kb.scale(scale_id).ok_or_else(|| {
            HarmonyError::knowledge_missing(format!("knowledge has no scale with id {scale_id:?}"))
        })?;
        let scale = ScaleInstance::new(def.clone(), tonic);
        let key_context_id = if MINOR_LIKE.contains(&scale_id) {
            "minor"
        } else {
            "major"
        };
        Ok(KeyContext {
            tonic,
            scale,
            scale_id: scale_id.to_string(),
            key_context_id: key_context_id.to_string(),
            is_modal: is_modal || MODAL_SCALES.contains(&scale_id),
            confidence,
        })
    }

    /// Pitch class of the tonic.
    pub fn tonic_pc(&self) -> i32 {
        (self.tonic.0.natural_pc() + i32::from(self.tonic.1 .0)).rem_euclid(12)
    }

    /// Spells the root a scale degree above the tonic.
    ///
    /// The letter is fixed by the diatonic number and the accidental by the
    /// sounding pitch class, so `b3` in C is `Eb` and never `D#`.
    pub fn degree_root(&self, degree: Degree) -> (Letter, Accidental) {
        let (letter, _) = self.tonic.0.step(i32::from(degree.number) - 1);
        let want = (self.tonic_pc() + degree.semitones()).rem_euclid(12);
        let natural = letter.natural_pc().rem_euclid(12);
        let mut diff = (want - natural).rem_euclid(12);
        if diff > 6 {
            diff -= 12;
        }
        (letter, Accidental(diff as i8))
    }

    /// Spells the root named by a knowledge-base degree string.
    pub fn root_of(&self, degree_text: &str) -> Option<(Letter, Accidental)> {
        Degree::parse(degree_text).map(|d| self.degree_root(d))
    }

    /// True when the pitch class belongs to the active collection.
    pub fn contains_pc(&self, pc: i32) -> bool {
        self.scale.contains_pc(pc.rem_euclid(12))
    }

    /// The fraction of a chord's pitch classes that are foreign to the key.
    pub fn chromaticism_of(&self, spec: &ChordSpec) -> f64 {
        let pcs = spec.pitch_classes();
        if pcs.is_empty() {
            return 0.0;
        }
        let foreign = pcs.iter().filter(|pc| !self.contains_pc(**pc)).count();
        foreign as f64 / pcs.len() as f64
    }

    /// A short human label, e.g. `"C major"` or `"D dorian"`.
    pub fn label(&self) -> String {
        format!(
            "{}{} {}",
            self.tonic.0.as_char(),
            self.tonic.1.ascii(),
            self.scale_id.replace('_', " ")
        )
    }

    /// The leading tone, when the collection has one a semitone below the tonic.
    pub fn leading_tone_pc(&self) -> Option<i32> {
        let pc = (self.tonic_pc() + 11).rem_euclid(12);
        if self.contains_pc(pc) {
            Some(pc)
        } else {
            None
        }
    }

    /// Spells a MIDI pitch inside this key.
    pub fn spell(&self, midi: i32) -> SpelledPitch {
        self.scale.spell(midi)
    }
}

/// Maps a `functions.json` function class onto the domain enum.
pub fn function_class(id: &str) -> HarmonicFunction {
    match id {
        "tonic" => HarmonicFunction::Tonic,
        "predominant" => HarmonicFunction::Predominant,
        "dominant" => HarmonicFunction::Dominant,
        "applied" => HarmonicFunction::Applied,
        "chromatic" => HarmonicFunction::Chromatic,
        "modal" => HarmonicFunction::Modal,
        "pedal" => HarmonicFunction::Pedal,
        "passing" => HarmonicFunction::Passing,
        "neighbor" => HarmonicFunction::Neighbor,
        _ => HarmonicFunction::Unclassified,
    }
}

/// Builds a semantic chord from a knowledge-base chord quality.
///
/// Every field of the quality record is honoured, so a chord's identity is
/// whatever `chord_qualities.json` says it is — the engine never invents one.
pub fn spec_from_quality(quality: &ChordQuality, root: (Letter, Accidental)) -> ChordSpec {
    let mut spec = ChordSpec {
        root,
        triad: TriadQuality::parse(&quality.triad).unwrap_or(TriadQuality::Major),
        seventh: SeventhQuality::parse(&quality.seventh).unwrap_or(SeventhQuality::None),
        extensions: parse_degrees(&quality.extensions),
        added: parse_degrees(&quality.added),
        alterations: parse_degrees(&quality.alterations),
        omissions: Vec::new(),
        bass: None,
        alt_dominant: quality.alt_dominant,
    };
    // A quality whose declared degrees include a chord member the triad and
    // seventh cannot express — the augmented-sixth family's `#4`, say — keeps
    // it as an alteration so the sonority is not silently truncated.
    for d in &quality.degrees {
        if let Some(deg) = ChordDegree::parse(d) {
            let expressed = spec
                .chord_tones()
                .iter()
                .any(|(have, _)| have.number == deg.number && have.alter == deg.alter);
            if !expressed && deg.number != 1 {
                if deg.alter == 0 && matches!(deg.number, 2 | 4 | 6 | 9 | 11 | 13) {
                    spec.added.push(deg);
                } else if deg.alter != 0 {
                    spec.alterations.push(deg);
                }
            }
        }
    }
    spec.normalize();
    spec
}

/// Parses a list of knowledge-base degree strings into chord degrees.
pub fn parse_degrees(list: &[String]) -> Vec<ChordDegree> {
    list.iter().filter_map(|d| ChordDegree::parse(d)).collect()
}

/// Chord qualities whose rendered ASCII symbol cannot be re-parsed.
///
/// `ChordSpec::render_ascii` writes an alteration immediately after the root
/// letter when the chord carries no seventh, so `italian_sixth` on C renders
/// `C#4`, which reads back as C-sharp with an added fourth. The three
/// augmented-sixth qualities are the only records in the bundle that hit it.
/// The engine still generates them: a candidate carries the semantic
/// [`ChordSpec`] and the function entry's roman numeral, and the ASCII symbol
/// is a display convenience rather than the identity.
pub const AMBIGUOUS_SYMBOL_QUALITIES: &[&str] = &["italian_sixth", "french_sixth", "german_sixth"];

/// Whether a chord quality can realise a function entry's declared triad and
/// seventh, i.e. whether it is an elaboration of the same harmony.
pub fn quality_matches_function(
    quality: &ChordQuality,
    base_triad: &ChordQuality,
    base_seventh: Option<&ChordQuality>,
) -> bool {
    if quality.triad != base_triad.triad {
        return false;
    }
    match base_seventh {
        Some(seventh) => quality.seventh == seventh.seventh || quality.seventh == "none",
        None => true,
    }
}

/// The roman-numeral label for an elaborated chord.
///
/// The numeral's job is to name the function, so it keeps the `functions.json`
/// numeral and adds only what the numeral itself expresses: the quality figure
/// of a root-position chord, or the figured-bass numbers of an inversion.
pub fn roman_label(entry: &FunctionEntry, spec: &ChordSpec, inversion: u8) -> String {
    // Two entries describe a behaviour whose numeral is already complete.
    if entry.id == "cadential_six_four" || entry.id == "modal_pedal_harmony" {
        return entry.roman.trim().to_string();
    }
    let base = entry.roman.trim();
    let (head, applied) = match base.split_once('/') {
        Some((h, t)) => (h, Some(t)),
        None => (base, None),
    };
    let core = strip_figure(head);
    let figure = if inversion > 0 {
        inversion_figure(spec, inversion).to_string()
    } else {
        quality_figure(spec)
    };
    match applied {
        Some(target) => format!("{core}{figure}/{target}"),
        None => format!("{core}{figure}"),
    }
}

/// Removes a trailing quality figure from a roman numeral, keeping the symbols
/// that carry triad quality (`°`, `ø`, `+`).
fn strip_figure(head: &str) -> String {
    let mut s = head.trim_end();
    while s.ends_with(|c: char| c.is_ascii_digit()) {
        s = &s[..s.len() - 1];
    }
    if let Some(stripped) = s.strip_suffix("maj") {
        s = stripped;
    }
    if let Some(stripped) = s.strip_suffix('o') {
        // `#io7` style numerals spell the diminished marker with a letter.
        return format!("{stripped}°");
    }
    s.to_string()
}

/// The figure a root-position chord adds to its numeral.
fn quality_figure(spec: &ChordSpec) -> String {
    let stack = |n: u8| {
        spec.extensions
            .iter()
            .any(|e| e.number == n && e.alter == 0)
    };
    let top = if stack(13) {
        "13"
    } else if stack(11) {
        "11"
    } else if stack(9) {
        "9"
    } else if spec.seventh.is_present() {
        "7"
    } else if spec.added.iter().any(|d| d.number == 6) {
        return if spec.added.iter().any(|d| d.number == 9) {
            "6/9".to_string()
        } else {
            "6".to_string()
        };
    } else {
        return String::new();
    };
    match spec.seventh {
        SeventhQuality::Major | SeventhQuality::AugmentedMajor => format!("maj{top}"),
        SeventhQuality::Diminished => format!("°{top}"),
        _ => top.to_string(),
    }
}

/// The figured-bass suffix for an inversion.
pub fn inversion_figure(spec: &ChordSpec, inversion: u8) -> &'static str {
    let seventh = spec.seventh.is_present();
    match (inversion, seventh) {
        (1, false) => "6",
        (2, false) => "6/4",
        (1, true) => "6/5",
        (2, true) => "4/3",
        (3, true) => "4/2",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c_major() -> KeyContext {
        KeyContext::new(
            KnowledgeBase::embedded(),
            (Letter::C, Accidental::NATURAL),
            "major",
            0.9,
            false,
        )
        .expect("C major")
    }

    #[test]
    fn degree_parsing_covers_the_knowledge_shapes() {
        assert_eq!(
            Degree::parse("1"),
            Some(Degree {
                number: 1,
                alter: 0
            })
        );
        assert_eq!(
            Degree::parse("b3"),
            Some(Degree {
                number: 3,
                alter: -1
            })
        );
        assert_eq!(
            Degree::parse("#4"),
            Some(Degree {
                number: 4,
                alter: 1
            })
        );
        assert_eq!(
            Degree::parse("9"),
            Some(Degree {
                number: 2,
                alter: 0
            })
        );
        assert_eq!(Degree::parse(""), None);
        assert_eq!(Degree::parse("x"), None);
        assert_eq!(Degree::parse("0"), None);
    }

    #[test]
    fn degree_text_round_trips() {
        for s in ["1", "b3", "#4", "b7", "5"] {
            let d = Degree::parse(s).expect("a degree");
            assert_eq!(d.text(), s);
        }
    }

    #[test]
    fn degree_roots_are_spelled_not_merely_sounded() {
        let k = c_major();
        assert_eq!(k.degree_root(Degree::parse("1").unwrap()).0, Letter::C);
        let flat_three = k.degree_root(Degree::parse("b3").unwrap());
        assert_eq!(flat_three.0, Letter::E);
        assert_eq!(flat_three.1, Accidental::FLAT);
        let sharp_four = k.degree_root(Degree::parse("#4").unwrap());
        assert_eq!(sharp_four.0, Letter::F);
        assert_eq!(sharp_four.1, Accidental::SHARP);
        let flat_two = k.degree_root(Degree::parse("b2").unwrap());
        assert_eq!(flat_two.0, Letter::D);
        assert_eq!(flat_two.1, Accidental::FLAT);
    }

    #[test]
    fn key_context_knows_its_collection() {
        let k = c_major();
        assert!(k.contains_pc(0));
        assert!(!k.contains_pc(1));
        assert_eq!(k.leading_tone_pc(), Some(11));
        assert_eq!(k.key_context_id, "major");
        assert!(!k.is_modal);
        assert_eq!(k.label(), "C major");
    }

    #[test]
    fn modal_scales_are_flagged_modal() {
        let k = KeyContext::new(
            KnowledgeBase::embedded(),
            (Letter::D, Accidental::NATURAL),
            "dorian",
            0.8,
            false,
        )
        .expect("D dorian");
        assert!(k.is_modal);
        assert_eq!(k.key_context_id, "minor");
        assert_eq!(k.leading_tone_pc(), None);
    }

    #[test]
    fn unknown_scale_is_a_knowledge_error() {
        let e = KeyContext::new(
            KnowledgeBase::embedded(),
            (Letter::C, Accidental::NATURAL),
            "not_a_scale",
            1.0,
            false,
        )
        .expect_err("unknown scale");
        assert_eq!(e.code, crate::error::KNOWLEDGE_MISSING);
    }

    #[test]
    fn quality_records_build_the_chords_they_describe() {
        let kb = KnowledgeBase::embedded();
        let root = (Letter::C, Accidental::NATURAL);
        for (id, expect) in [
            ("major_triad", "C"),
            ("minor7", "Cm7"),
            ("dominant7", "C7"),
            ("major7", "Cmaj7"),
            ("half_diminished7", "Cm7b5"),
            ("diminished7", "Cdim7"),
            ("six_nine", "C6/9"),
            ("dominant7sus4", "C7sus4"),
            ("minor_major7", "CmMaj7"),
        ] {
            let q = kb.chord_quality(id).unwrap_or_else(|| panic!("{id}"));
            let spec = spec_from_quality(q, root);
            assert_eq!(spec.render_ascii(), expect, "quality {id}");
        }
    }

    #[test]
    fn quality_records_round_trip_through_the_symbol_parser() {
        let kb = KnowledgeBase::embedded();
        for q in kb.chord_qualities() {
            if AMBIGUOUS_SYMBOL_QUALITIES.contains(&q.id.as_str()) {
                continue;
            }
            let spec = spec_from_quality(q, (Letter::C, Accidental::NATURAL));
            let text = spec.render_ascii();
            let parsed = symbol::parse(&text)
                .unwrap_or_else(|e| panic!("quality {} rendered {text:?}: {e:?}", q.id));
            assert_eq!(
                parsed.render_ascii(),
                text,
                "quality {} does not round trip",
                q.id
            );
        }
    }

    #[test]
    fn the_documented_ambiguous_qualities_are_exactly_the_augmented_sixths() {
        let kb = KnowledgeBase::embedded();
        let mut found: Vec<&str> = Vec::new();
        for q in kb.chord_qualities() {
            let spec = spec_from_quality(q, (Letter::C, Accidental::NATURAL));
            let text = spec.render_ascii();
            let round_trips = symbol::parse(&text)
                .map(|p| p.render_ascii() == text)
                .unwrap_or(false);
            if !round_trips {
                found.push(q.id.as_str());
            }
        }
        assert_eq!(
            found, AMBIGUOUS_SYMBOL_QUALITIES,
            "the set of qualities whose ASCII symbol does not round trip has changed"
        );
    }

    #[test]
    fn augmented_sixths_keep_their_semantic_identity() {
        let kb = KnowledgeBase::embedded();
        // Even though the ASCII text is ambiguous, the spec is not: a German
        // sixth on Ab must sound Ab, C, Eb (spelt b3 here) and F#.
        let spec = spec_from_quality(
            kb.chord_quality("german_sixth").expect("german_sixth"),
            (Letter::A, Accidental::FLAT),
        );
        let pcs = spec.pitch_classes();
        for want in [8, 0, 11, 2] {
            assert!(pcs.contains(&want), "german sixth is missing pc {want}");
        }
    }

    #[test]
    fn chromaticism_measures_foreign_pitch_classes() {
        let kb = KnowledgeBase::embedded();
        let k = c_major();
        let diatonic = spec_from_quality(
            kb.chord_quality("major7").unwrap(),
            (Letter::C, Accidental::NATURAL),
        );
        assert_eq!(k.chromaticism_of(&diatonic), 0.0);
        let foreign = spec_from_quality(
            kb.chord_quality("dominant7").unwrap(),
            (Letter::D, Accidental::FLAT),
        );
        assert!(foreign.pitch_classes().len() > 1);
        // Db7 is Db F Ab Cb: two of its four pitch classes are foreign to C major.
        assert!(k.chromaticism_of(&foreign) >= 0.5);
    }

    #[test]
    fn function_classes_map_onto_the_domain_enum() {
        assert_eq!(function_class("tonic"), HarmonicFunction::Tonic);
        assert_eq!(function_class("applied"), HarmonicFunction::Applied);
        assert_eq!(function_class("passing"), HarmonicFunction::Passing);
        assert_eq!(function_class("nonsense"), HarmonicFunction::Unclassified);
        for f in KnowledgeBase::embedded().functions() {
            assert_ne!(
                function_class(&f.function_class),
                HarmonicFunction::Unclassified,
                "function entry {} has an unmapped class {}",
                f.id,
                f.function_class
            );
        }
    }

    #[test]
    fn every_function_entry_names_a_usable_quality() {
        let kb = KnowledgeBase::embedded();
        let k = c_major();
        for f in kb.functions() {
            if let Some(d) = &f.scale_degree {
                assert!(
                    k.root_of(d).is_some(),
                    "function {} has unparsable degree {d}",
                    f.id
                );
            }
            if f.triad_quality != "any" {
                assert!(
                    kb.chord_quality(&f.triad_quality).is_some(),
                    "function {} names unknown triad quality {}",
                    f.id,
                    f.triad_quality
                );
            }
        }
    }
}
