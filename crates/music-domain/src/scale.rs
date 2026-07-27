//! Scale definitions and rooted scale instances.
//!
//! A [`ScaleDef`] is the abstract collection as it appears in
//! `knowledge/scales.json`: a formula in semitones plus degree spelling and
//! editorial metadata. A [`ScaleInstance`] is that definition rooted on a
//! concrete, spelled tonic, which is what actually spells pitches.
//!
//! The small [`builtin`] set defined here exists so that defaults, tests and
//! fixture key hints work without the knowledge base. The authoritative
//! catalogue is loaded by `theory-kb`.

use crate::error::DomainError;
use crate::pitch::{Accidental, Letter, SpelledPitch, SpellingContext};
use qjson::{Json, JsonMap};

/// An abstract scale or mode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScaleDef {
    /// Stable identifier, e.g. `"major"`, `"dorian"`, `"lydian_dominant"`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Alternative names accepted by search and by symbol lookup.
    pub aliases: Vec<String>,
    /// Ascending semitone offsets from the tonic, excluding the octave.
    pub semitones: Vec<i32>,
    /// Degree spelling parallel to `semitones`: `"1"`, `"b3"`, `"#4"`, `"b7"`.
    pub degree_spelling: Vec<String>,
    /// Degrees that give the collection its identity.
    pub characteristic_degrees: Vec<String>,
    /// Parent collection id, when this is derived from another.
    pub parent: Option<String>,
    /// `(parent id, mode index)` when this is a rotation of another scale.
    pub mode_of: Option<(String, usize)>,
    /// Chord symbols commonly built on this collection.
    pub common_chords: Vec<String>,
    /// Degrees treated as tensions rather than stable tones.
    pub tension_degrees: Vec<String>,
    /// Family: `"diatonic"`, `"pentatonic"`, `"symmetric"`, `"synthetic"`, `"blues"`, …
    pub family: String,
    /// Source ids backing this definition.
    pub source_refs: Vec<String>,
}

impl ScaleDef {
    /// Builds a minimal definition from an id and a semitone formula.
    pub fn simple(id: &str, name: &str, semitones: &[i32], family: &str) -> ScaleDef {
        ScaleDef {
            id: id.to_string(),
            name: name.to_string(),
            semitones: semitones.to_vec(),
            family: family.to_string(),
            ..ScaleDef::default()
        }
    }

    /// Number of degrees in the collection.
    pub fn len(&self) -> usize {
        self.semitones.len()
    }

    /// True when the collection has no degrees.
    pub fn is_empty(&self) -> bool {
        self.semitones.is_empty()
    }

    /// True when every semitone offset is inside one octave and ascending.
    pub fn is_well_formed(&self) -> bool {
        if self.semitones.is_empty() || self.semitones[0] != 0 {
            return false;
        }
        self.semitones.windows(2).all(|w| w[0] < w[1]) && *self.semitones.last().unwrap_or(&0) < 12
    }

    /// JSON form using the knowledge-file field names.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert("name", Json::Str(self.name.clone()));
        m.insert("aliases", str_array(&self.aliases));
        m.insert(
            "semitones",
            Json::Arr(
                self.semitones
                    .iter()
                    .map(|s| Json::Int(*s as i64))
                    .collect(),
            ),
        );
        m.insert("degree_spelling", str_array(&self.degree_spelling));
        m.insert(
            "characteristic_degrees",
            str_array(&self.characteristic_degrees),
        );
        m.insert(
            "parent",
            match &self.parent {
                Some(p) => Json::Str(p.clone()),
                None => Json::Null,
            },
        );
        m.insert(
            "mode_of",
            match &self.mode_of {
                Some((p, i)) => Json::Arr(vec![Json::Str(p.clone()), Json::Int(*i as i64)]),
                None => Json::Null,
            },
        );
        m.insert("common_chords", str_array(&self.common_chords));
        m.insert("tension_degrees", str_array(&self.tension_degrees));
        m.insert("family", Json::Str(self.family.clone()));
        m.insert("source_refs", str_array(&self.source_refs));
        Json::Obj(m)
    }

    /// Reads the JSON form; only `id` and `semitones` are required.
    pub fn from_json(v: &Json) -> Result<ScaleDef, DomainError> {
        let id = v.str_field("id")?.to_string();
        let mut semitones = Vec::new();
        for s in v.arr_field("semitones")? {
            semitones.push(s.as_i64().ok_or_else(|| {
                DomainError::invalid_scale(format!("non-integer semitone in scale {id:?}"))
            })? as i32);
        }
        let mode_of = match v.get("mode_of") {
            Some(Json::Arr(a)) if a.len() == 2 => match (a[0].as_str(), a[1].as_i64()) {
                (Some(p), Some(i)) => Some((p.to_string(), i.max(0) as usize)),
                _ => None,
            },
            _ => None,
        };
        Ok(ScaleDef {
            name: v.opt_str_field("name")?.unwrap_or(&id).to_string(),
            aliases: string_list(v, "aliases"),
            degree_spelling: string_list(v, "degree_spelling"),
            characteristic_degrees: string_list(v, "characteristic_degrees"),
            parent: v.opt_str_field("parent")?.map(|s| s.to_string()),
            mode_of,
            common_chords: string_list(v, "common_chords"),
            tension_degrees: string_list(v, "tension_degrees"),
            family: v.opt_str_field("family")?.unwrap_or("").to_string(),
            source_refs: string_list(v, "source_refs"),
            id,
            semitones,
        })
    }
}

/// Renders a string vector as a JSON array.
fn str_array(v: &[String]) -> Json {
    Json::Arr(v.iter().map(|s| Json::Str(s.clone())).collect())
}

/// Reads an optional array-of-strings field, ignoring non-string entries.
fn string_list(v: &Json, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Json::Arr(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// A [`ScaleDef`] rooted on a concrete spelled tonic.
#[derive(Clone, Debug, PartialEq)]
pub struct ScaleInstance {
    /// The abstract collection.
    pub def: ScaleDef,
    /// The spelled tonic.
    pub tonic: (Letter, Accidental),
}

impl ScaleInstance {
    /// Roots `def` on `tonic`.
    pub fn new(def: ScaleDef, tonic: (Letter, Accidental)) -> ScaleInstance {
        ScaleInstance { def, tonic }
    }

    /// Sounding pitch class of the tonic.
    pub fn tonic_pc(&self) -> i32 {
        (self.tonic.0.natural_pc() + self.tonic.1.semitones()).rem_euclid(12)
    }

    /// Sounding pitch classes of every degree, in degree order.
    pub fn pitch_classes(&self) -> Vec<i32> {
        let t = self.tonic_pc();
        self.def
            .semitones
            .iter()
            .map(|s| (t + s).rem_euclid(12))
            .collect()
    }

    /// Spelled `(letter, accidental)` of every degree, in degree order.
    ///
    /// Degree spelling from the definition decides which letter each degree
    /// uses; the accidental is then whatever reaches the correct sounding
    /// pitch, so `Db major` spells its fourth as `Gb` and never as `F#`.
    pub fn spelled_degrees(&self) -> Vec<(Letter, Accidental)> {
        let tonic_pitch = SpelledPitch::new(self.tonic.0, self.tonic.1, 4);
        let mut out = Vec::with_capacity(self.def.semitones.len());
        for (i, semi) in self.def.semitones.iter().enumerate() {
            let number = match self
                .def
                .degree_spelling
                .get(i)
                .and_then(|s| degree_number(s))
            {
                Some(n) => n,
                None => fallback_degree_number(*semi, self.def.semitones.len(), i),
            };
            let (letter, octave_delta) = self.tonic.0.step(number - 1);
            let octave = 4 + octave_delta;
            let target = tonic_pitch.midi() + semi;
            let natural = (octave + 1) * 12 + letter.natural_pc();
            let alter = (target - natural).clamp(i8::MIN as i32, i8::MAX as i32) as i8;
            out.push((letter, Accidental(alter)));
        }
        out
    }

    /// True when `pc` is in the collection.
    pub fn contains_pc(&self, pc: i32) -> bool {
        let pc = pc.rem_euclid(12);
        self.pitch_classes().contains(&pc)
    }

    /// Degree index of `pc`, if it is in the collection.
    pub fn degree_of_pc(&self, pc: i32) -> Option<usize> {
        let pc = pc.rem_euclid(12);
        self.pitch_classes().iter().position(|p| *p == pc)
    }

    /// Spells a sounding MIDI pitch using this collection.
    pub fn spell(&self, midi: i32) -> SpelledPitch {
        SpelledPitch::from_midi(midi, Some(&self.spelling_context()))
    }

    /// The spelling context this collection implies.
    ///
    /// `prefer_flats` is true when the tonic itself is flat or when more of the
    /// collection's degrees are spelled with flats than with sharps.
    pub fn spelling_context(&self) -> SpellingContext {
        let spelled = self.spelled_degrees();
        let flats = spelled.iter().filter(|(_, a)| a.semitones() < 0).count();
        let sharps = spelled.iter().filter(|(_, a)| a.semitones() > 0).count();
        SpellingContext {
            tonic_pc: Some(self.tonic_pc()),
            prefer_flats: self.tonic.1.semitones() < 0 || flats > sharps,
            scale_pcs: self.pitch_classes(),
            scale_spelling: spelled,
        }
    }

    /// Index of the degree closest to `midi`'s pitch class, measured
    /// circularly; ties resolve to the lower index. Returns 0 for an empty
    /// collection.
    pub fn nearest_degree(&self, midi: i32) -> i32 {
        let pc = midi.rem_euclid(12);
        let mut best = (i32::MAX, 0usize);
        for (i, p) in self.pitch_classes().iter().enumerate() {
            let raw = (pc - p).rem_euclid(12);
            let dist = raw.min(12 - raw);
            if dist < best.0 {
                best = (dist, i);
            }
        }
        if best.0 == i32::MAX {
            0
        } else {
            best.1 as i32
        }
    }

    /// The spelled pitch of degree `index` (0-based) in `octave`.
    pub fn degree_pitch(&self, index: usize, octave: i32) -> Option<SpelledPitch> {
        let (letter, acc) = *self.spelled_degrees().get(index)?;
        let semi = *self.def.semitones.get(index)?;
        let tonic_midi = SpelledPitch::new(self.tonic.0, self.tonic.1, octave).midi();
        let midi = tonic_midi + semi;
        let natural_pc = letter.natural_pc() + acc.semitones();
        let oct = (midi - natural_pc).div_euclid(12) - 1;
        Some(SpelledPitch::new(letter, acc, oct))
    }
}

/// Parses a degree spelling such as `"1"`, `"b3"`, `"#11"` into its number.
fn degree_number(s: &str) -> Option<i32> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let n: i32 = digits.parse().ok()?;
    if n < 1 {
        return None;
    }
    Some(if n > 7 { (n - 1) % 7 + 1 } else { n })
}

/// Diatonic number used when a definition has no degree spelling.
///
/// Seven-note collections take one letter per degree, which is what makes a
/// mode spell cleanly. Other collections use a conventional semitone-to-number
/// table; repeated numbers are allowed on purpose, so a blues scale spells
/// `b5` and `5` as `Gb` and `G` rather than inventing a double flat.
fn fallback_degree_number(semi: i32, len: usize, index: usize) -> i32 {
    if len == 7 {
        return index as i32 + 1;
    }
    const TABLE: [i32; 12] = [1, 2, 2, 3, 3, 4, 5, 5, 6, 6, 7, 7];
    TABLE[semi.rem_euclid(12) as usize]
}

/// A small built-in catalogue used for defaults and tests.
///
/// These definitions carry no editorial metadata; `theory-kb` supplies the real
/// catalogue with sources, characteristic degrees and chord relationships.
pub mod builtin {
    use super::ScaleDef;

    /// Ionian / major.
    pub fn major() -> ScaleDef {
        def("major", "Major", &[0, 2, 4, 5, 7, 9, 11], "diatonic")
    }
    /// Aeolian / natural minor.
    pub fn natural_minor() -> ScaleDef {
        def(
            "natural_minor",
            "Natural minor",
            &[0, 2, 3, 5, 7, 8, 10],
            "diatonic",
        )
    }
    /// Harmonic minor.
    pub fn harmonic_minor() -> ScaleDef {
        def(
            "harmonic_minor",
            "Harmonic minor",
            &[0, 2, 3, 5, 7, 8, 11],
            "diatonic",
        )
    }
    /// Ascending melodic minor.
    pub fn melodic_minor() -> ScaleDef {
        def(
            "melodic_minor",
            "Melodic minor (ascending)",
            &[0, 2, 3, 5, 7, 9, 11],
            "diatonic",
        )
    }
    /// Dorian.
    pub fn dorian() -> ScaleDef {
        def("dorian", "Dorian", &[0, 2, 3, 5, 7, 9, 10], "diatonic")
    }
    /// Phrygian.
    pub fn phrygian() -> ScaleDef {
        def("phrygian", "Phrygian", &[0, 1, 3, 5, 7, 8, 10], "diatonic")
    }
    /// Lydian.
    pub fn lydian() -> ScaleDef {
        def("lydian", "Lydian", &[0, 2, 4, 6, 7, 9, 11], "diatonic")
    }
    /// Mixolydian.
    pub fn mixolydian() -> ScaleDef {
        def(
            "mixolydian",
            "Mixolydian",
            &[0, 2, 4, 5, 7, 9, 10],
            "diatonic",
        )
    }
    /// Locrian.
    pub fn locrian() -> ScaleDef {
        def("locrian", "Locrian", &[0, 1, 3, 5, 6, 8, 10], "diatonic")
    }
    /// Major pentatonic.
    pub fn major_pentatonic() -> ScaleDef {
        def(
            "major_pentatonic",
            "Major pentatonic",
            &[0, 2, 4, 7, 9],
            "pentatonic",
        )
    }
    /// Minor pentatonic.
    pub fn minor_pentatonic() -> ScaleDef {
        def(
            "minor_pentatonic",
            "Minor pentatonic",
            &[0, 3, 5, 7, 10],
            "pentatonic",
        )
    }
    /// Six-note blues scale.
    pub fn blues() -> ScaleDef {
        def("blues", "Blues", &[0, 3, 5, 6, 7, 10], "blues")
    }
    /// Whole-tone.
    pub fn whole_tone() -> ScaleDef {
        def(
            "whole_tone",
            "Whole tone",
            &[0, 2, 4, 6, 8, 10],
            "symmetric",
        )
    }
    /// The altered scale (seventh mode of melodic minor).
    pub fn altered() -> ScaleDef {
        def("altered", "Altered", &[0, 1, 3, 4, 6, 8, 10], "diatonic")
    }
    /// The full chromatic collection.
    pub fn chromatic() -> ScaleDef {
        def(
            "chromatic",
            "Chromatic",
            &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            "symmetric",
        )
    }

    /// Every built-in definition, in a stable order.
    pub fn all() -> Vec<ScaleDef> {
        vec![
            major(),
            natural_minor(),
            harmonic_minor(),
            melodic_minor(),
            dorian(),
            phrygian(),
            lydian(),
            mixolydian(),
            locrian(),
            major_pentatonic(),
            minor_pentatonic(),
            blues(),
            whole_tone(),
            altered(),
            chromatic(),
        ]
    }

    /// Looks up a built-in definition by id.
    pub fn by_id(id: &str) -> Option<ScaleDef> {
        all().into_iter().find(|d| d.id == id)
    }

    /// Builds a definition with conventional degree spelling.
    fn def(id: &str, name: &str, semitones: &[i32], family: &str) -> ScaleDef {
        let mut d = ScaleDef::simple(id, name, semitones, family);
        d.degree_spelling = default_spelling(semitones);
        d
    }

    /// Degree spelling for a formula, e.g. `[0,2,3,5,7,8,10] -> 1 2 b3 4 5 b6 b7`.
    fn default_spelling(semitones: &[i32]) -> Vec<String> {
        const NATURAL: [i32; 8] = [0, 0, 2, 4, 5, 7, 9, 11];
        let mut out = Vec::with_capacity(semitones.len());
        for (i, s) in semitones.iter().enumerate() {
            let number = super::fallback_degree_number(*s, semitones.len(), i);
            let alter = s - NATURAL[number as usize];
            let prefix = match alter {
                -2 => "bb".to_string(),
                -1 => "b".to_string(),
                0 => String::new(),
                1 => "#".to_string(),
                2 => "##".to_string(),
                n => format!("({n:+})"),
            };
            out.push(format!("{prefix}{number}"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c_major() -> ScaleInstance {
        ScaleInstance::new(builtin::major(), (Letter::C, Accidental::NATURAL))
    }

    #[test]
    fn pitch_classes_of_c_major() {
        assert_eq!(c_major().pitch_classes(), vec![0, 2, 4, 5, 7, 9, 11]);
        assert_eq!(c_major().tonic_pc(), 0);
    }

    #[test]
    fn pitch_classes_of_transposed_scales() {
        let eb = ScaleInstance::new(builtin::major(), (Letter::E, Accidental::FLAT));
        assert_eq!(eb.pitch_classes(), vec![3, 5, 7, 8, 10, 0, 2]);
        let fs = ScaleInstance::new(builtin::major(), (Letter::F, Accidental::SHARP));
        assert_eq!(fs.pitch_classes(), vec![6, 8, 10, 11, 1, 3, 5]);
    }

    #[test]
    fn spelled_degrees_keep_one_letter_per_degree() {
        let names: Vec<String> = c_major()
            .spelled_degrees()
            .iter()
            .map(|(l, a)| format!("{}{}", l.as_char(), a.ascii()))
            .collect();
        assert_eq!(names, ["C", "D", "E", "F", "G", "A", "B"]);
    }

    #[test]
    fn flat_key_spelling_avoids_sharps() {
        let db = ScaleInstance::new(builtin::major(), (Letter::D, Accidental::FLAT));
        let names: Vec<String> = db
            .spelled_degrees()
            .iter()
            .map(|(l, a)| format!("{}{}", l.as_char(), a.ascii()))
            .collect();
        assert_eq!(names, ["Db", "Eb", "F", "Gb", "Ab", "Bb", "C"]);
    }

    #[test]
    fn sharp_key_spelling_uses_sharps() {
        let fs = ScaleInstance::new(builtin::major(), (Letter::F, Accidental::SHARP));
        let names: Vec<String> = fs
            .spelled_degrees()
            .iter()
            .map(|(l, a)| format!("{}{}", l.as_char(), a.ascii()))
            .collect();
        assert_eq!(names, ["F#", "G#", "A#", "B", "C#", "D#", "E#"]);
    }

    #[test]
    fn minor_modes_spell_flat_degrees() {
        let c_dorian = ScaleInstance::new(builtin::dorian(), (Letter::C, Accidental::NATURAL));
        let names: Vec<String> = c_dorian
            .spelled_degrees()
            .iter()
            .map(|(l, a)| format!("{}{}", l.as_char(), a.ascii()))
            .collect();
        assert_eq!(names, ["C", "D", "Eb", "F", "G", "A", "Bb"]);
    }

    #[test]
    fn membership_and_degree_lookup() {
        let s = c_major();
        assert!(s.contains_pc(4));
        assert!(!s.contains_pc(6));
        assert!(s.contains_pc(16));
        assert_eq!(s.degree_of_pc(7), Some(4));
        assert_eq!(s.degree_of_pc(6), None);
    }

    #[test]
    fn spelling_context_reflects_the_collection() {
        let ctx = c_major().spelling_context();
        assert_eq!(ctx.tonic_pc, Some(0));
        assert!(!ctx.prefer_flats);
        assert_eq!(ctx.scale_pcs.len(), 7);
        let eb = ScaleInstance::new(builtin::major(), (Letter::E, Accidental::FLAT));
        assert!(eb.spelling_context().prefer_flats);
    }

    #[test]
    fn spell_uses_the_collection() {
        let eb = ScaleInstance::new(builtin::major(), (Letter::E, Accidental::FLAT));
        assert_eq!(eb.spell(70).to_ascii(), "Bb4");
        assert_eq!(eb.spell(63).to_ascii(), "Eb4");
        let fs = ScaleInstance::new(builtin::major(), (Letter::F, Accidental::SHARP));
        assert_eq!(fs.spell(70).to_ascii(), "A#4");
    }

    #[test]
    fn nearest_degree_is_circular() {
        let s = c_major();
        assert_eq!(s.nearest_degree(60), 0);
        assert_eq!(s.nearest_degree(61), 0);
        assert_eq!(s.nearest_degree(66), 3);
        assert_eq!(s.nearest_degree(71), 6);
    }

    #[test]
    fn degree_pitch_places_the_octave() {
        let s = c_major();
        assert_eq!(s.degree_pitch(0, 4).unwrap().to_ascii(), "C4");
        assert_eq!(s.degree_pitch(4, 4).unwrap().to_ascii(), "G4");
        assert_eq!(s.degree_pitch(6, 4).unwrap().to_ascii(), "B4");
        assert!(s.degree_pitch(9, 4).is_none());
    }

    #[test]
    fn diatonic_transposition_within_a_scale() {
        let s = c_major();
        let c4 = SpelledPitch::parse("C4").unwrap();
        assert_eq!(c4.transpose_diatonic(1, &s).to_ascii(), "D4");
        assert_eq!(c4.transpose_diatonic(2, &s).to_ascii(), "E4");
        assert_eq!(c4.transpose_diatonic(7, &s).to_ascii(), "C5");
        assert_eq!(c4.transpose_diatonic(-1, &s).to_ascii(), "B3");
        let e4 = SpelledPitch::parse("E4").unwrap();
        assert_eq!(e4.transpose_diatonic(2, &s).to_ascii(), "G4");
    }

    #[test]
    fn diatonic_transposition_in_a_flat_key() {
        let eb = ScaleInstance::new(builtin::major(), (Letter::E, Accidental::FLAT));
        let g4 = SpelledPitch::parse("G4").unwrap();
        assert_eq!(g4.transpose_diatonic(1, &eb).to_ascii(), "Ab4");
        assert_eq!(g4.transpose_diatonic(-1, &eb).to_ascii(), "F4");
    }

    #[test]
    fn pentatonic_and_blues_are_well_formed() {
        for d in builtin::all() {
            assert!(d.is_well_formed(), "{} is malformed", d.id);
            assert_eq!(d.degree_spelling.len(), d.semitones.len(), "{}", d.id);
            assert!(!d.is_empty());
            assert_eq!(d.len(), d.semitones.len());
        }
    }

    #[test]
    fn builtin_lookup() {
        assert!(builtin::by_id("major").is_some());
        assert!(builtin::by_id("nope").is_none());
        assert_eq!(builtin::all().len(), 15);
    }

    #[test]
    fn scale_def_json_round_trip() {
        let mut d = builtin::dorian();
        d.aliases = vec!["dor".to_string()];
        d.parent = Some("major".to_string());
        d.mode_of = Some(("major".to_string(), 1));
        d.source_refs = vec!["src.levine".to_string()];
        let back = ScaleDef::from_json(&d.to_json()).expect("round trip");
        assert_eq!(back, d);
    }

    #[test]
    fn scale_def_json_requires_semitones() {
        let bad = qjson::json_obj! { "id" => "x" };
        assert!(ScaleDef::from_json(&bad).is_err());
    }

    #[test]
    fn malformed_definitions_are_detected() {
        let d = ScaleDef::simple("bad", "Bad", &[2, 4, 5], "synthetic");
        assert!(!d.is_well_formed());
        let empty = ScaleDef::simple("empty", "Empty", &[], "synthetic");
        assert!(!empty.is_well_formed());
        assert!(empty.is_empty());
    }

    #[test]
    fn default_spelling_for_pentatonic() {
        let p = builtin::minor_pentatonic();
        assert_eq!(p.degree_spelling, ["1", "b3", "4", "5", "b7"]);
        let b = builtin::blues();
        assert_eq!(b.degree_spelling, ["1", "b3", "4", "b5", "5", "b7"]);
    }
}
