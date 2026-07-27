//! The REAPER-free test corpus format.
//!
//! Fixtures under `fixtures/` describe a piece of music with no host attached:
//! a tempo and meter map, optional notes, optional chord symbols, an optional
//! loop and key hint, and a free-form `expected` block that golden tests
//! compare against. Every musical position is a **rational string** parsed by
//! [`BeatTime`] — never a bare float — and every pitch is a spelled pitch.
//!
//! ```json
//! {
//!   "fixture_version": "1.0.0",
//!   "id": "melodies/twinkle_c_major",
//!   "title": "Human readable",
//!   "description": "What this fixture is for",
//!   "tempo_bpm": 120.0,
//!   "time_signature": "4/4",
//!   "tempo_map": [{ "qn": "0", "bpm": 120.0, "linear": false }],
//!   "meter_map": [{ "qn": "0", "sig": "4/4", "measure": 0 }],
//!   "loop": { "start_qn": "0", "end_qn": "32", "intent": "closed_tonic" },
//!   "key_hint": { "tonic": "C", "scale_id": "major" },
//!   "notes": [{ "pitch": "C4", "onset": "0", "duration": "1", "velocity": 96 }],
//!   "chords": [{ "symbol": "Cmaj7", "onset": "0", "duration": "4" }]
//! }
//! ```
//!
//! Defaults for absent note members: `velocity` 96, `channel` 0, `muted` and
//! `selected` false, `voice` 0, `role` `"unknown"`. `notes` may be absent for a
//! progression-only fixture and `chords` for a melody-only one.

use crate::chord::{ChordEvent, ChordSpec};
use crate::error::DomainError;
use crate::note::{Note, NoteRole, NoteSet, VoiceId};
use crate::pitch::{Accidental, Letter, SpelledPitch};
use crate::structure::LoopIntent;
use crate::symbol;
use crate::time::{BeatTime, MeterEvent, TempoEvent, TimeMap, TimeSignature};
use qjson::{json_obj, Json};
use std::path::Path;

/// The version this loader was written against.
pub const FIXTURE_VERSION: &str = "1.0.0";

/// A loop region declared by a fixture.
#[derive(Clone, Debug, PartialEq)]
pub struct FixtureLoop {
    /// Loop start.
    pub start_qn: BeatTime,
    /// Loop end.
    pub end_qn: BeatTime,
    /// Declared intent, when the fixture names one.
    pub intent: Option<LoopIntent>,
}

impl FixtureLoop {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "start_qn" => self.start_qn.to_json(),
            "end_qn" => self.end_qn.to_json(),
            "intent" => match self.intent { Some(i) => Json::Str(i.id().to_string()), None => Json::Null },
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<FixtureLoop, DomainError> {
        let intent = match v.get("intent") {
            Some(Json::Str(s)) => Some(LoopIntent::parse(s).ok_or_else(|| {
                DomainError::invalid_fixture(format!("unknown loop intent {s:?}"))
            })?),
            _ => None,
        };
        Ok(FixtureLoop {
            start_qn: BeatTime::from_json(v.field("start_qn")?)?,
            end_qn: BeatTime::from_json(v.field("end_qn")?)?,
            intent,
        })
    }
}

/// A key hint declared by a fixture.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyHint {
    /// Spelled tonic.
    pub tonic: (Letter, Accidental),
    /// Scale identifier.
    pub scale_id: String,
}

impl KeyHint {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "tonic" => format!("{}{}", self.tonic.0.as_char(), self.tonic.1.ascii()),
            "scale_id" => self.scale_id.clone(),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<KeyHint, DomainError> {
        let text = v.str_field("tonic")?;
        let tonic = SpelledPitch::parse_class(text)
            .ok_or_else(|| DomainError::invalid_fixture(format!("bad tonic {text:?}")))?;
        Ok(KeyHint {
            tonic,
            scale_id: v.opt_str_field("scale_id")?.unwrap_or("major").to_string(),
        })
    }
}

/// A chord entry in a fixture: the original text plus its parsed identity.
#[derive(Clone, Debug, PartialEq)]
pub struct FixtureChord {
    /// Symbol as written in the file.
    pub symbol: String,
    /// Parsed semantic identity.
    pub spec: ChordSpec,
    /// Onset in quarter notes.
    pub onset: BeatTime,
    /// Duration in quarter notes.
    pub duration: BeatTime,
}

impl FixtureChord {
    /// JSON form, in the fixture's own shape.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "symbol" => self.symbol.clone(),
            "onset" => self.onset.to_json(),
            "duration" => self.duration.to_json(),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<FixtureChord, DomainError> {
        let symbol_text = v.str_field("symbol")?;
        let spec = symbol::parse(symbol_text).map_err(|e| {
            DomainError::invalid_fixture(format!("chord symbol {symbol_text:?}: {}", e.message))
        })?;
        Ok(FixtureChord {
            symbol: symbol_text.to_string(),
            spec,
            onset: BeatTime::from_json(v.field("onset")?)?,
            duration: BeatTime::from_json(v.field("duration")?)?,
        })
    }
}

/// A parsed fixture file.
#[derive(Clone, Debug, PartialEq)]
pub struct Fixture {
    /// Format version declared by the file.
    pub fixture_version: String,
    /// Fixture identifier, e.g. `"melodies/twinkle_c_major"`.
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// What the fixture is for.
    pub description: String,
    /// Nominal tempo.
    pub tempo_bpm: f64,
    /// Nominal time signature.
    pub time_signature: TimeSignature,
    /// Tempo events.
    pub tempo_map: Vec<TempoEvent>,
    /// Meter events.
    pub meter_map: Vec<MeterEvent>,
    /// Loop region, when declared.
    pub loop_region: Option<FixtureLoop>,
    /// Key hint, when declared.
    pub key_hint: Option<KeyHint>,
    /// Notes, in file order.
    pub notes: Vec<Note>,
    /// Chords, in file order.
    pub chords: Vec<FixtureChord>,
    /// Free-form expectations compared by golden tests.
    pub expected: Option<Json>,
}

impl Fixture {
    /// Parses a fixture from its JSON form.
    pub fn from_json(v: &Json) -> Result<Fixture, DomainError> {
        let id = v.str_field("id")?.to_string();
        let bad = |what: &str, e: DomainError| {
            DomainError::invalid_fixture(format!("fixture {id}: {what}: {}", e.message))
        };

        let time_signature = match v.opt_str_field("time_signature")? {
            Some(s) => TimeSignature::parse(s).ok_or_else(|| {
                DomainError::invalid_fixture(format!("fixture {id}: bad time signature {s:?}"))
            })?,
            None => TimeSignature::default(),
        };
        let tempo_bpm = v.opt_f64_field("tempo_bpm")?.unwrap_or(120.0);

        let mut tempo_map = Vec::new();
        if let Some(Json::Arr(a)) = v.get("tempo_map") {
            for t in a {
                tempo_map.push(TempoEvent::from_json(t).map_err(|e| bad("tempo_map", e))?);
            }
        }
        if tempo_map.is_empty() {
            tempo_map.push(TempoEvent::new(BeatTime::ZERO, tempo_bpm, false));
        }

        let mut meter_map = Vec::new();
        if let Some(Json::Arr(a)) = v.get("meter_map") {
            for m in a {
                meter_map.push(MeterEvent::from_json(m).map_err(|e| bad("meter_map", e))?);
            }
        }
        if meter_map.is_empty() {
            meter_map.push(MeterEvent::new(BeatTime::ZERO, time_signature, 0));
        }

        let loop_region = match v.get("loop") {
            Some(l @ Json::Obj(_)) => Some(FixtureLoop::from_json(l).map_err(|e| bad("loop", e))?),
            _ => None,
        };
        let key_hint = match v.get("key_hint") {
            Some(k @ Json::Obj(_)) => Some(KeyHint::from_json(k).map_err(|e| bad("key_hint", e))?),
            _ => None,
        };

        let mut notes = Vec::new();
        if let Some(Json::Arr(a)) = v.get("notes") {
            for (i, n) in a.iter().enumerate() {
                notes.push(parse_note(n, i as u32).map_err(|e| bad("notes", e))?);
            }
        }

        let mut chords = Vec::new();
        if let Some(Json::Arr(a)) = v.get("chords") {
            for c in a {
                chords.push(FixtureChord::from_json(c).map_err(|e| bad("chords", e))?);
            }
        }

        let fixture = Fixture {
            fixture_version: v
                .opt_str_field("fixture_version")?
                .unwrap_or(FIXTURE_VERSION)
                .to_string(),
            id,
            title: v.opt_str_field("title")?.unwrap_or("").to_string(),
            description: v.opt_str_field("description")?.unwrap_or("").to_string(),
            tempo_bpm,
            time_signature,
            tempo_map,
            meter_map,
            loop_region,
            key_hint,
            notes,
            chords,
            expected: v.get("expected").cloned(),
        };
        fixture.validate()?;
        Ok(fixture)
    }

    /// Reads and parses a fixture file.
    pub fn from_path(p: &Path) -> Result<Fixture, DomainError> {
        let text = std::fs::read_to_string(p).map_err(|e| {
            DomainError::invalid_fixture(format!("cannot read {}: {e}", p.display()))
        })?;
        let json = Json::parse(&text).map_err(|e| {
            DomainError::invalid_fixture(format!("cannot parse {}: {e}", p.display()))
        })?;
        Fixture::from_json(&json)
            .map_err(|e| DomainError::invalid_fixture(format!("{}: {}", p.display(), e.message)))
    }

    /// The tempo and meter map this fixture describes.
    pub fn time_map(&self) -> TimeMap {
        TimeMap::new(self.tempo_map.clone(), self.meter_map.clone())
    }

    /// The fixture's notes as an ordered [`NoteSet`].
    pub fn note_set(&self) -> NoteSet {
        NoteSet::sorted(self.notes.clone(), self.time_map())
    }

    /// The fixture's chords as [`ChordEvent`]s, ids assigned in file order.
    pub fn chord_events(&self) -> Vec<ChordEvent> {
        self.chords
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut ev = ChordEvent::new(i as u32, c.spec.clone(), c.onset, c.duration);
                ev.original_symbol = Some(c.symbol.clone());
                ev
            })
            .collect()
    }

    /// True when the fixture carries notes.
    pub fn has_notes(&self) -> bool {
        !self.notes.is_empty()
    }

    /// True when the fixture carries chords.
    pub fn has_chords(&self) -> bool {
        !self.chords.is_empty()
    }

    /// The golden-output file name for this fixture: the id with slashes
    /// replaced by dashes, plus `.json`.
    pub fn expected_file_name(&self) -> String {
        format!("{}.json", self.id.replace('/', "-"))
    }

    /// Checks the invariants the corpus relies on: positive durations, in-range
    /// MIDI, a usable time map, and a sane loop region.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.id.is_empty() {
            return Err(DomainError::invalid_fixture("fixture id is empty"));
        }
        for n in &self.notes {
            n.validate().map_err(|e| {
                DomainError::invalid_fixture(format!("fixture {}: {}", self.id, e.message))
            })?;
        }
        for c in &self.chords {
            if !c.duration.is_positive() {
                return Err(DomainError::invalid_fixture(format!(
                    "fixture {}: chord {} has non-positive duration",
                    self.id, c.symbol
                )));
            }
        }
        if self.tempo_map.is_empty() || self.meter_map.is_empty() {
            return Err(DomainError::invalid_fixture(format!(
                "fixture {}: empty tempo or meter map",
                self.id
            )));
        }
        for t in &self.tempo_map {
            if !(t.bpm.is_finite() && t.bpm > 0.0) {
                return Err(DomainError::invalid_fixture(format!(
                    "fixture {}: tempo {} is not a usable bpm",
                    self.id, t.bpm
                )));
            }
        }
        if let Some(l) = &self.loop_region {
            if l.end_qn <= l.start_qn {
                return Err(DomainError::invalid_fixture(format!(
                    "fixture {}: loop end must be after its start",
                    self.id
                )));
            }
        }
        if !self.has_notes() && !self.has_chords() {
            return Err(DomainError::invalid_fixture(format!(
                "fixture {}: has neither notes nor chords",
                self.id
            )));
        }
        Ok(())
    }

    /// JSON form, in the same shape the files use.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "fixture_version" => self.fixture_version.clone(),
            "id" => self.id.clone(),
            "title" => self.title.clone(),
            "description" => self.description.clone(),
            "tempo_bpm" => self.tempo_bpm,
            "time_signature" => self.time_signature.to_display(),
            "tempo_map" => Json::Arr(self.tempo_map.iter().map(|t| t.to_json()).collect()),
            "meter_map" => Json::Arr(self.meter_map.iter().map(|m| m.to_json()).collect()),
            "loop" => match &self.loop_region { Some(l) => l.to_json(), None => Json::Null },
            "key_hint" => match &self.key_hint { Some(k) => k.to_json(), None => Json::Null },
            "notes" => Json::Arr(self.notes.iter().map(|n| n.to_json()).collect()),
            "chords" => Json::Arr(self.chords.iter().map(|c| c.to_json()).collect()),
            "expected" => self.expected.clone().unwrap_or(Json::Null),
        }
    }
}

/// Parses one fixture note, applying the format's documented defaults.
fn parse_note(v: &Json, index: u32) -> Result<Note, DomainError> {
    let pitch_text = v.str_field("pitch")?;
    let pitch = SpelledPitch::parse(pitch_text)
        .ok_or_else(|| DomainError::invalid_fixture(format!("bad pitch {pitch_text:?}")))?;
    let role = match v.opt_str_field("role")? {
        Some(r) => NoteRole::parse(r)
            .ok_or_else(|| DomainError::invalid_fixture(format!("unknown role {r:?}")))?,
        None => NoteRole::Unknown,
    };
    let mut note = Note::new(
        v.opt_i64_field("id")?.unwrap_or(index as i64).max(0) as u32,
        pitch,
        BeatTime::from_json(v.field("onset")?)?,
        BeatTime::from_json(v.field("duration")?)?,
    );
    note.velocity = v.opt_i64_field("velocity")?.unwrap_or(96).clamp(0, 127) as u8;
    note.channel = v.opt_i64_field("channel")?.unwrap_or(0).clamp(0, 15) as u8;
    note.muted = v.opt_bool_field("muted")?.unwrap_or(false);
    note.selected = v.opt_bool_field("selected")?.unwrap_or(false);
    note.voice = VoiceId(
        v.opt_i64_field("voice")?
            .unwrap_or(0)
            .clamp(0, u16::MAX as i64) as u16,
    );
    note.role = role;
    note.articulation = v.opt_str_field("articulation")?.map(|s| s.to_string());
    Ok(note)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The workspace `fixtures/` directory.
    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("fixtures")
    }

    /// Every `*.json` file under `fixtures/`, excluding the golden outputs.
    fn fixture_files() -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![fixtures_dir()];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    // `expected/` holds golden outputs and `mock-reaper/` holds
                    // IPC envelopes; neither is a fixture.
                    if name == "expected" || name == "mock-reaper" {
                        continue;
                    }
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    fn minimal_json() -> Json {
        json_obj! {
            "fixture_version" => "1.0.0",
            "id" => "melodies/minimal",
            "title" => "Minimal",
            "description" => "One note",
            "tempo_bpm" => 120.0,
            "time_signature" => "4/4",
            "tempo_map" => Json::Arr(vec![json_obj!{ "qn" => "0", "bpm" => 120.0, "linear" => false }]),
            "meter_map" => Json::Arr(vec![json_obj!{ "qn" => "0", "sig" => "4/4", "measure" => 0 }]),
            "notes" => Json::Arr(vec![json_obj!{ "pitch" => "C4", "onset" => "0", "duration" => "1" }]),
        }
    }

    #[test]
    fn the_corpus_is_present() {
        let files = fixture_files();
        assert!(
            files.len() >= 11,
            "expected the 11 committed fixtures, found {}",
            files.len()
        );
    }

    #[test]
    fn every_committed_fixture_loads() {
        for path in fixture_files() {
            let f = Fixture::from_path(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(!f.id.is_empty(), "{}", path.display());
            assert_eq!(f.fixture_version, FIXTURE_VERSION, "{}", path.display());
            assert!(f.has_notes() || f.has_chords(), "{}", path.display());
        }
    }

    #[test]
    fn every_committed_fixture_has_sane_notes() {
        for path in fixture_files() {
            let f = Fixture::from_path(&path).expect("loads");
            for n in &f.notes {
                assert!(
                    n.duration.is_positive(),
                    "{}: note {} has duration {}",
                    path.display(),
                    n.id,
                    n.duration
                );
                assert!(
                    (0..=127).contains(&n.midi),
                    "{}: note {} has midi {}",
                    path.display(),
                    n.id,
                    n.midi
                );
                assert_eq!(n.midi, n.pitch.midi(), "{}", path.display());
            }
        }
    }

    #[test]
    fn every_committed_fixture_has_a_valid_time_map() {
        for path in fixture_files() {
            let f = Fixture::from_path(&path).expect("loads");
            let tm = f.time_map();
            assert!(!tm.tempos.is_empty(), "{}", path.display());
            assert!(!tm.meters.is_empty(), "{}", path.display());
            assert_eq!(tm.tempos[0].qn, BeatTime::ZERO, "{}", path.display());
            assert_eq!(tm.meters[0].qn, BeatTime::ZERO, "{}", path.display());
            assert!(tm.tempo_at(BeatTime::ZERO) > 0.0, "{}", path.display());
            assert_eq!(tm.bar_of(BeatTime::ZERO), 0, "{}", path.display());
            assert_eq!(tm.hash_hex().len(), 64, "{}", path.display());
        }
    }

    #[test]
    fn every_committed_fixture_chord_parses_and_round_trips() {
        let mut total = 0;
        for path in fixture_files() {
            let f = Fixture::from_path(&path).expect("loads");
            for c in &f.chords {
                total += 1;
                assert!(c.duration.is_positive(), "{}", path.display());
                let rendered = c.spec.render_ascii();
                let reparsed = symbol::parse(&rendered).expect("renders to a parseable symbol");
                assert_eq!(reparsed, c.spec, "{} {}", path.display(), c.symbol);
            }
        }
        assert!(total > 0, "the corpus should contain chords");
    }

    #[test]
    fn note_sets_and_chord_events_come_out_of_a_fixture() {
        for path in fixture_files() {
            let f = Fixture::from_path(&path).expect("loads");
            let set = f.note_set();
            assert_eq!(set.notes.len(), f.notes.len(), "{}", path.display());
            assert!(set.validate().is_ok(), "{}", path.display());
            let events = f.chord_events();
            assert_eq!(events.len(), f.chords.len(), "{}", path.display());
            for (i, e) in events.iter().enumerate() {
                assert_eq!(e.id, i as u32);
                assert_eq!(
                    e.original_symbol.as_deref(),
                    Some(f.chords[i].symbol.as_str())
                );
                assert!(e.duration.is_positive());
            }
        }
    }

    #[test]
    fn loop_regions_are_well_formed() {
        for path in fixture_files() {
            let f = Fixture::from_path(&path).expect("loads");
            if let Some(l) = &f.loop_region {
                assert!(l.end_qn > l.start_qn, "{}", path.display());
            }
        }
    }

    #[test]
    fn minimal_fixture_loads_with_defaults() {
        let f = Fixture::from_json(&minimal_json()).expect("loads");
        assert_eq!(f.notes.len(), 1);
        assert_eq!(f.notes[0].velocity, 96);
        assert_eq!(f.notes[0].channel, 0);
        assert_eq!(f.notes[0].voice, VoiceId(0));
        assert_eq!(f.notes[0].role, NoteRole::Unknown);
        assert!(!f.notes[0].selected);
        assert!(!f.notes[0].muted);
        assert!(f.chords.is_empty());
        assert!(f.loop_region.is_none());
        assert!(f.key_hint.is_none());
        assert_eq!(f.expected_file_name(), "melodies-minimal.json");
    }

    #[test]
    fn rational_strings_are_required_and_exact() {
        let mut v = minimal_json();
        if let Json::Obj(m) = &mut v {
            m.insert(
                "notes",
                Json::Arr(vec![
                    json_obj! { "pitch" => "C4", "onset" => "1/3", "duration" => "1/3" },
                ]),
            );
        }
        let f = Fixture::from_json(&v).expect("loads");
        assert_eq!(f.notes[0].onset, BeatTime::new(1, 3));
        assert_eq!(f.notes[0].duration, BeatTime::new(1, 3));
    }

    #[test]
    fn missing_id_or_empty_content_is_rejected() {
        let no_id = json_obj! { "fixture_version" => "1.0.0" };
        assert!(Fixture::from_json(&no_id).is_err());
        let empty = json_obj! {
            "fixture_version" => "1.0.0",
            "id" => "x/y",
            "tempo_bpm" => 120.0,
        };
        let err = Fixture::from_json(&empty).unwrap_err();
        assert_eq!(err.code, "INVALID_FIXTURE");
    }

    #[test]
    fn bad_members_are_rejected_with_context() {
        let mut v = minimal_json();
        if let Json::Obj(m) = &mut v {
            m.insert(
                "notes",
                Json::Arr(vec![
                    json_obj! { "pitch" => "H4", "onset" => "0", "duration" => "1" },
                ]),
            );
        }
        let err = Fixture::from_json(&v).unwrap_err();
        assert!(err.message.contains("melodies/minimal"), "{}", err.message);

        let mut v = minimal_json();
        if let Json::Obj(m) = &mut v {
            m.insert(
                "chords",
                Json::Arr(vec![
                    json_obj! { "symbol" => "H7", "onset" => "0", "duration" => "4" },
                ]),
            );
        }
        assert!(Fixture::from_json(&v).is_err());

        let mut v = minimal_json();
        if let Json::Obj(m) = &mut v {
            m.insert("time_signature", Json::Str("nonsense".to_string()));
        }
        assert!(Fixture::from_json(&v).is_err());

        let mut v = minimal_json();
        if let Json::Obj(m) = &mut v {
            m.insert(
                "loop",
                json_obj! { "start_qn" => "8", "end_qn" => "4", "intent" => "closed_tonic" },
            );
        }
        assert!(Fixture::from_json(&v).is_err());

        let mut v = minimal_json();
        if let Json::Obj(m) = &mut v {
            m.insert(
                "loop",
                json_obj! { "start_qn" => "0", "end_qn" => "4", "intent" => "spiral" },
            );
        }
        assert!(Fixture::from_json(&v).is_err());
    }

    #[test]
    fn zero_duration_notes_are_rejected() {
        let mut v = minimal_json();
        if let Json::Obj(m) = &mut v {
            m.insert(
                "notes",
                Json::Arr(vec![
                    json_obj! { "pitch" => "C4", "onset" => "0", "duration" => "0" },
                ]),
            );
        }
        assert!(Fixture::from_json(&v).is_err());
    }

    #[test]
    fn key_hint_and_loop_round_trip() {
        let k = KeyHint {
            tonic: (Letter::E, Accidental::FLAT),
            scale_id: "dorian".to_string(),
        };
        assert_eq!(KeyHint::from_json(&k.to_json()).unwrap(), k);
        let l = FixtureLoop {
            start_qn: BeatTime::ZERO,
            end_qn: BeatTime::from_quarters(16),
            intent: Some(LoopIntent::ModalDrone),
        };
        assert_eq!(FixtureLoop::from_json(&l.to_json()).unwrap(), l);
    }

    #[test]
    fn fixture_json_round_trips() {
        for path in fixture_files() {
            let f = Fixture::from_path(&path).expect("loads");
            let back = Fixture::from_json(&f.to_json())
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(back.id, f.id);
            assert_eq!(back.notes, f.notes);
            assert_eq!(back.chords, f.chords);
            assert_eq!(back.loop_region, f.loop_region);
            assert_eq!(back.key_hint, f.key_hint);
            assert_eq!(back.time_map(), f.time_map());
        }
    }

    #[test]
    fn missing_file_reports_a_domain_error() {
        let err = Fixture::from_path(Path::new("/nonexistent/fixture.json")).unwrap_err();
        assert_eq!(err.code, "INVALID_FIXTURE");
    }

    #[test]
    fn malformed_file_reports_a_domain_error() {
        let dir = std::env::temp_dir().join("music-domain-fixture-tests");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("broken.json");
        std::fs::write(&path, "{ not json").expect("write");
        let err = Fixture::from_path(&path).unwrap_err();
        assert_eq!(err.code, "INVALID_FIXTURE");
        let _ = std::fs::remove_file(&path);
    }
}
