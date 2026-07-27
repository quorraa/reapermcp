//! Notes, voices and the ordered note collection every stage shares.
//!
//! A [`Note`] keeps its spelling *and* its sounding MIDI pitch, its exact
//! rational onset and duration, and the analysis fields later stages fill in
//! (voice, role, salience, non-chord-tone hypotheses). A [`NoteSet`] is the
//! ordered collection plus the time map it was captured against, which is the
//! unit of work passed between crates.

use crate::error::DomainError;
use crate::pitch::SpelledPitch;
use crate::time::{BeatTime, TimeMap};
use qjson::{json_obj, Json};

/// Stable identifier for a note within a [`NoteSet`].
pub type NoteId = u32;

/// Voice identity, preserved across chords so that lines stay lines.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct VoiceId(pub u16);

/// The voice value used before voice assignment has run.
pub const VOICE_UNASSIGNED: VoiceId = VoiceId(u16::MAX);

impl VoiceId {
    /// True when this note has not been assigned to a voice yet.
    pub fn is_unassigned(self) -> bool {
        self == VOICE_UNASSIGNED
    }
}

/// The musical job a note is doing.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum NoteRole {
    /// Principal melodic line.
    Melody,
    /// Secondary melodic line.
    Counter,
    /// Bass line.
    Bass,
    /// Inner harmony.
    Harmony,
    /// Sustained pedal.
    Pedal,
    /// Ornamental figure.
    Ornament,
    /// Unpitched or percussive.
    Percussion,
    /// Not yet classified.
    #[default]
    Unknown,
}

impl NoteRole {
    /// Stable identifier used in JSON and fixtures.
    pub fn id(self) -> &'static str {
        match self {
            NoteRole::Melody => "melody",
            NoteRole::Counter => "counter",
            NoteRole::Bass => "bass",
            NoteRole::Harmony => "harmony",
            NoteRole::Pedal => "pedal",
            NoteRole::Ornament => "ornament",
            NoteRole::Percussion => "percussion",
            NoteRole::Unknown => "unknown",
        }
    }

    /// Parses the identifier produced by [`NoteRole::id`].
    pub fn parse(s: &str) -> Option<NoteRole> {
        NoteRole::all().iter().copied().find(|r| r.id() == s)
    }

    /// Every variant, in declaration order.
    pub fn all() -> &'static [NoteRole] {
        &[
            NoteRole::Melody,
            NoteRole::Counter,
            NoteRole::Bass,
            NoteRole::Harmony,
            NoteRole::Pedal,
            NoteRole::Ornament,
            NoteRole::Percussion,
            NoteRole::Unknown,
        ]
    }
}

/// How a note relates to the prevailing harmony.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum NctKind {
    /// A member of the chord.
    ChordTone,
    /// Stepwise motion filling a leap or a third.
    Passing,
    /// Steps away and back.
    Neighbor,
    /// Held over a chord change and resolved downward.
    Suspension,
    /// Held over a chord change and resolved upward.
    Retardation,
    /// Arrives before its chord.
    Anticipation,
    /// Approached by leap, left by step.
    Appoggiatura,
    /// Approached by step, left by leap.
    EscapeTone,
    /// Sustained through changing harmony.
    PedalTone,
    /// Chromatic approach from above or below.
    ChromaticApproach,
    /// Surrounds the target from both sides.
    Enclosure,
    /// Decorative, without a stronger classification.
    Decorative,
}

impl NctKind {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            NctKind::ChordTone => "chord_tone",
            NctKind::Passing => "passing",
            NctKind::Neighbor => "neighbor",
            NctKind::Suspension => "suspension",
            NctKind::Retardation => "retardation",
            NctKind::Anticipation => "anticipation",
            NctKind::Appoggiatura => "appoggiatura",
            NctKind::EscapeTone => "escape_tone",
            NctKind::PedalTone => "pedal_tone",
            NctKind::ChromaticApproach => "chromatic_approach",
            NctKind::Enclosure => "enclosure",
            NctKind::Decorative => "decorative",
        }
    }

    /// Parses the identifier produced by [`NctKind::id`].
    pub fn parse(s: &str) -> Option<Self> {
        NctKind::all().iter().copied().find(|k| k.id() == s)
    }

    /// Every variant, in declaration order.
    pub fn all() -> &'static [NctKind] {
        &[
            NctKind::ChordTone,
            NctKind::Passing,
            NctKind::Neighbor,
            NctKind::Suspension,
            NctKind::Retardation,
            NctKind::Anticipation,
            NctKind::Appoggiatura,
            NctKind::EscapeTone,
            NctKind::PedalTone,
            NctKind::ChromaticApproach,
            NctKind::Enclosure,
            NctKind::Decorative,
        ]
    }
}

/// One ranked hypothesis about a note's harmonic role.
#[derive(Clone, Debug, PartialEq)]
pub struct NctHypothesis {
    /// The proposed classification.
    pub kind: NctKind,
    /// Confidence in it, `0.0..=1.0`.
    pub confidence: f64,
    /// Why the analyzer proposed it.
    pub rationale: String,
}

impl NctHypothesis {
    /// Builds a hypothesis.
    pub fn new(kind: NctKind, confidence: f64, rationale: impl Into<String>) -> NctHypothesis {
        NctHypothesis {
            kind,
            confidence,
            rationale: rationale.into(),
        }
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "kind" => self.kind.id(),
            "confidence" => self.confidence,
            "rationale" => self.rationale.clone(),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<NctHypothesis, DomainError> {
        let kind_text = v.str_field("kind")?;
        let kind = NctKind::parse(kind_text).ok_or_else(|| {
            DomainError::invalid_argument(format!("unknown NCT kind {kind_text:?}"))
        })?;
        Ok(NctHypothesis {
            kind,
            confidence: v.opt_f64_field("confidence")?.unwrap_or(0.0),
            rationale: v.opt_str_field("rationale")?.unwrap_or("").to_string(),
        })
    }
}

/// A single note event.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// Stable id within the collection.
    pub id: NoteId,
    /// Notated spelling.
    pub pitch: SpelledPitch,
    /// Authoritative sounding pitch, `0..=127`.
    pub midi: i32,
    /// Microtonal offset in cents; always 0.0 in version one.
    pub cents: f64,
    /// Onset in quarter notes.
    pub onset: BeatTime,
    /// Duration in quarter notes; strictly positive.
    pub duration: BeatTime,
    /// MIDI velocity, `1..=127`.
    pub velocity: u8,
    /// MIDI channel, `0..=15`.
    pub channel: u8,
    /// Mute state.
    pub muted: bool,
    /// Selection state in the host.
    pub selected: bool,
    /// Voice identity.
    pub voice: VoiceId,
    /// Musical role.
    pub role: NoteRole,
    /// Free-form articulation metadata.
    pub articulation: Option<String>,
    /// Structural salience, `0.0..=1.0`.
    pub salience: f64,
    /// Ranked non-chord-tone hypotheses.
    pub nct: Vec<NctHypothesis>,
    /// Confidence in this note's analysis, `0.0..=1.0`.
    pub confidence: f64,
    /// Original REAPER PPQ position, for provenance.
    pub source_ppq: Option<f64>,
    /// Originating take GUID, for provenance.
    pub source_take: Option<String>,
}

impl Note {
    /// Builds a note with conventional defaults: velocity 96, channel 0,
    /// unassigned voice, unknown role.
    pub fn new(id: NoteId, pitch: SpelledPitch, onset: BeatTime, duration: BeatTime) -> Note {
        Note {
            id,
            pitch,
            midi: pitch.midi(),
            cents: 0.0,
            onset,
            duration,
            velocity: 96,
            channel: 0,
            muted: false,
            selected: false,
            voice: VOICE_UNASSIGNED,
            role: NoteRole::Unknown,
            articulation: None,
            salience: 0.0,
            nct: Vec::new(),
            confidence: 1.0,
            source_ppq: None,
            source_take: None,
        }
    }

    /// End position: `onset + duration`.
    pub fn end(&self) -> BeatTime {
        self.onset + self.duration
    }

    /// True when the two notes sound at the same time for a non-zero span.
    pub fn overlaps(&self, other: &Note) -> bool {
        self.onset < other.end() && other.onset < self.end()
    }

    /// True when `qn` falls inside `[onset, end)`.
    pub fn contains(&self, qn: BeatTime) -> bool {
        qn >= self.onset && qn < self.end()
    }

    /// Sounding pitch class.
    pub fn pitch_class(&self) -> i32 {
        self.midi.rem_euclid(12)
    }

    /// Checks the note's invariants.
    pub fn validate(&self) -> Result<(), DomainError> {
        if !self.duration.is_positive() {
            return Err(DomainError::invalid_note(format!(
                "note {} has non-positive duration {}",
                self.id, self.duration
            )));
        }
        if !(0..=127).contains(&self.midi) {
            return Err(DomainError::invalid_note(format!(
                "note {} has out-of-range MIDI pitch {}",
                self.id, self.midi
            )));
        }
        if self.velocity == 0 || self.velocity > 127 {
            return Err(DomainError::invalid_note(format!(
                "note {} has out-of-range velocity {}",
                self.id, self.velocity
            )));
        }
        if self.channel > 15 {
            return Err(DomainError::invalid_note(format!(
                "note {} has out-of-range channel {}",
                self.id, self.channel
            )));
        }
        if !self.cents.is_finite() {
            return Err(DomainError::invalid_note(format!(
                "note {} has a non-finite cents offset",
                self.id
            )));
        }
        for (name, value) in [("salience", self.salience), ("confidence", self.confidence)] {
            if !(value.is_finite() && (0.0..=1.0).contains(&value)) {
                return Err(DomainError::invalid_note(format!(
                    "note {} has {name} {value} outside 0..=1",
                    self.id
                )));
            }
        }
        Ok(())
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "id" => self.id as i64,
            "pitch" => self.pitch.to_ascii(),
            "midi" => self.midi as i64,
            "cents" => self.cents,
            "onset" => self.onset.to_json(),
            "duration" => self.duration.to_json(),
            "velocity" => self.velocity as i64,
            "channel" => self.channel as i64,
            "muted" => self.muted,
            "selected" => self.selected,
            "voice" => self.voice.0 as i64,
            "role" => self.role.id(),
            "articulation" => match &self.articulation { Some(a) => Json::Str(a.clone()), None => Json::Null },
            "salience" => self.salience,
            "nct" => Json::Arr(self.nct.iter().map(|h| h.to_json()).collect()),
            "confidence" => self.confidence,
            "source_ppq" => match self.source_ppq { Some(p) => Json::Float(p), None => Json::Null },
            "source_take" => match &self.source_take { Some(t) => Json::Str(t.clone()), None => Json::Null },
        }
    }

    /// Reads the JSON form. Missing optional fields take the same defaults as
    /// the fixture format.
    pub fn from_json(v: &Json) -> Result<Note, DomainError> {
        let pitch_text = v.str_field("pitch")?;
        let pitch = SpelledPitch::parse(pitch_text)
            .ok_or_else(|| DomainError::invalid_pitch(format!("bad pitch {pitch_text:?}")))?;
        let role = match v.opt_str_field("role")? {
            Some(r) => NoteRole::parse(r)
                .ok_or_else(|| DomainError::invalid_note(format!("unknown role {r:?}")))?,
            None => NoteRole::Unknown,
        };
        let mut nct = Vec::new();
        if let Some(Json::Arr(a)) = v.get("nct") {
            for h in a {
                nct.push(NctHypothesis::from_json(h)?);
            }
        }
        let midi = match v.opt_i64_field("midi")? {
            Some(m) => m as i32,
            None => pitch.midi(),
        };
        Ok(Note {
            id: v
                .opt_i64_field("id")?
                .unwrap_or(0)
                .clamp(0, u32::MAX as i64) as u32,
            pitch,
            midi,
            cents: v.opt_f64_field("cents")?.unwrap_or(0.0),
            onset: BeatTime::from_json(v.field("onset")?)?,
            duration: BeatTime::from_json(v.field("duration")?)?,
            velocity: v.opt_i64_field("velocity")?.unwrap_or(96).clamp(0, 127) as u8,
            channel: v.opt_i64_field("channel")?.unwrap_or(0).clamp(0, 15) as u8,
            muted: v.opt_bool_field("muted")?.unwrap_or(false),
            selected: v.opt_bool_field("selected")?.unwrap_or(false),
            voice: VoiceId(
                v.opt_i64_field("voice")?
                    .unwrap_or(VOICE_UNASSIGNED.0 as i64)
                    .clamp(0, u16::MAX as i64) as u16,
            ),
            role,
            articulation: v.opt_str_field("articulation")?.map(|s| s.to_string()),
            salience: v.opt_f64_field("salience")?.unwrap_or(0.0),
            nct,
            confidence: v.opt_f64_field("confidence")?.unwrap_or(1.0),
            source_ppq: v.opt_f64_field("source_ppq")?,
            source_take: v.opt_str_field("source_take")?.map(|s| s.to_string()),
        })
    }
}

/// Where a [`NoteSet`] came from in the host project.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteOrigin {
    /// Take GUID.
    pub take_guid: Option<String>,
    /// Media item GUID.
    pub item_guid: Option<String>,
    /// Track GUID.
    pub track_guid: Option<String>,
}

impl NoteOrigin {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "take_guid" => match &self.take_guid { Some(g) => Json::Str(g.clone()), None => Json::Null },
            "item_guid" => match &self.item_guid { Some(g) => Json::Str(g.clone()), None => Json::Null },
            "track_guid" => match &self.track_guid { Some(g) => Json::Str(g.clone()), None => Json::Null },
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<NoteOrigin, DomainError> {
        Ok(NoteOrigin {
            take_guid: v.opt_str_field("take_guid")?.map(|s| s.to_string()),
            item_guid: v.opt_str_field("item_guid")?.map(|s| s.to_string()),
            track_guid: v.opt_str_field("track_guid")?.map(|s| s.to_string()),
        })
    }
}

/// An ordered note collection with the time map it was captured against.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteSet {
    /// Notes, ordered by `(onset, midi, id)` when built with
    /// [`NoteSet::sorted`].
    pub notes: Vec<Note>,
    /// The tempo and meter map these positions refer to.
    pub time_map: TimeMap,
    /// Host provenance.
    pub origin: NoteOrigin,
}

impl NoteSet {
    /// Builds a set, sorting the notes by `(onset, midi, id)`.
    pub fn sorted(notes: Vec<Note>, time_map: TimeMap) -> NoteSet {
        let mut notes = notes;
        notes.sort_by(|a, b| {
            a.onset
                .cmp(&b.onset)
                .then(a.midi.cmp(&b.midi))
                .then(a.id.cmp(&b.id))
        });
        NoteSet {
            notes,
            time_map,
            origin: NoteOrigin::default(),
        }
    }

    /// True when the set holds no notes.
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// Number of notes.
    pub fn len(&self) -> usize {
        self.notes.len()
    }

    /// First onset and last end. An empty set spans `(0, 0)`.
    pub fn span(&self) -> (BeatTime, BeatTime) {
        let mut start = BeatTime::ZERO;
        let mut end = BeatTime::ZERO;
        for (i, n) in self.notes.iter().enumerate() {
            if i == 0 {
                start = n.onset;
                end = n.end();
            } else {
                start = start.min(n.onset);
                end = end.max(n.end());
            }
        }
        (start, end)
    }

    /// True when no two notes ever sound together.
    pub fn is_monophonic(&self) -> bool {
        for (i, a) in self.notes.iter().enumerate() {
            for b in &self.notes[i + 1..] {
                if b.onset >= a.end() {
                    break;
                }
                if a.overlaps(b) {
                    return false;
                }
            }
        }
        true
    }

    /// Number of notes sounding at `qn`.
    pub fn polyphony_at(&self, qn: BeatTime) -> usize {
        self.notes.iter().filter(|n| n.contains(qn)).count()
    }

    /// Largest simultaneous note count anywhere in the set.
    pub fn max_polyphony(&self) -> usize {
        let mut best = 0;
        for n in &self.notes {
            best = best.max(self.polyphony_at(n.onset));
        }
        best
    }

    /// Notes sounding at `qn`, in collection order.
    pub fn sounding_at(&self, qn: BeatTime) -> Vec<&Note> {
        self.notes.iter().filter(|n| n.contains(qn)).collect()
    }

    /// Notes assigned to voice `v`.
    pub fn by_voice(&self, v: VoiceId) -> Vec<&Note> {
        self.notes.iter().filter(|n| n.voice == v).collect()
    }

    /// The top line: at every onset, the highest note sounding there, with
    /// consecutive repeats of the same note collapsed.
    pub fn highest_line(&self) -> Vec<&Note> {
        self.extreme_line(true)
    }

    /// The bottom line, mirroring [`NoteSet::highest_line`].
    pub fn lowest_line(&self) -> Vec<&Note> {
        self.extreme_line(false)
    }

    /// Shared skyline walk for the highest and lowest lines.
    fn extreme_line(&self, highest: bool) -> Vec<&Note> {
        let mut onsets: Vec<BeatTime> = self.notes.iter().map(|n| n.onset).collect();
        onsets.sort();
        onsets.dedup();
        let mut out: Vec<&Note> = Vec::new();
        for qn in onsets {
            let sounding = self.sounding_at(qn);
            let pick = sounding.into_iter().reduce(|a, b| {
                let take_b = if highest {
                    (b.midi, b.id) > (a.midi, a.id)
                } else {
                    (b.midi, b.id) < (a.midi, a.id)
                };
                if take_b {
                    b
                } else {
                    a
                }
            });
            if let Some(n) = pick {
                if out.last().map(|p| p.id) != Some(n.id) {
                    out.push(n);
                }
            }
        }
        out
    }

    /// Duration-weighted pitch-class profile, in quarter notes.
    ///
    /// Muted notes are excluded; they do not sound and must not steer key
    /// detection.
    pub fn pitch_class_durations(&self) -> [f64; 12] {
        let mut out = [0.0f64; 12];
        for n in &self.notes {
            if n.muted {
                continue;
            }
            let pc = n.pitch_class();
            out[pc as usize] += n.duration.as_f64();
        }
        out
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "notes" => Json::Arr(self.notes.iter().map(|n| n.to_json()).collect()),
            "time_map" => self.time_map.to_json(),
            "origin" => self.origin.to_json(),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<NoteSet, DomainError> {
        let mut notes = Vec::new();
        for n in v.arr_field("notes")? {
            notes.push(Note::from_json(n)?);
        }
        let time_map = match v.get("time_map") {
            Some(m @ Json::Obj(_)) => TimeMap::from_json(m)?,
            _ => TimeMap::default(),
        };
        let origin = match v.get("origin") {
            Some(o @ Json::Obj(_)) => NoteOrigin::from_json(o)?,
            _ => NoteOrigin::default(),
        };
        Ok(NoteSet {
            notes,
            time_map,
            origin,
        })
    }

    /// SHA-256 of the canonical JSON form. Deterministic for equal content.
    pub fn hash_hex(&self) -> String {
        qjson::sha256::sha256_hex(self.to_json().to_canonical_string().as_bytes())
    }

    /// Validates every note, reporting the first failure.
    pub fn validate(&self) -> Result<(), DomainError> {
        for n in &self.notes {
            n.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::TimeSignature;

    fn note(id: NoteId, pitch: &str, onset: i64, dur: i64) -> Note {
        Note::new(
            id,
            SpelledPitch::parse(pitch).expect(pitch),
            BeatTime::from_quarters(onset),
            BeatTime::from_quarters(dur),
        )
    }

    fn scale_set() -> NoteSet {
        let notes = vec![
            note(0, "C4", 0, 1),
            note(1, "D4", 1, 1),
            note(2, "E4", 2, 1),
            note(3, "F4", 3, 1),
        ];
        NoteSet::sorted(notes, TimeMap::constant(120.0, TimeSignature::new(4, 4)))
    }

    fn chordal_set() -> NoteSet {
        let notes = vec![
            note(0, "C4", 0, 4),
            note(1, "E4", 0, 4),
            note(2, "G4", 0, 4),
            note(3, "C5", 2, 2),
        ];
        NoteSet::sorted(notes, TimeMap::default())
    }

    #[test]
    fn note_defaults() {
        let n = note(1, "C4", 0, 1);
        assert_eq!(n.midi, 60);
        assert_eq!(n.velocity, 96);
        assert_eq!(n.channel, 0);
        assert_eq!(n.voice, VOICE_UNASSIGNED);
        assert!(n.voice.is_unassigned());
        assert_eq!(n.role, NoteRole::Unknown);
        assert_eq!(n.cents, 0.0);
        assert!(!n.muted);
    }

    #[test]
    fn note_time_helpers() {
        let a = note(0, "C4", 0, 2);
        let b = note(1, "E4", 1, 2);
        let c = note(2, "G4", 2, 2);
        assert_eq!(a.end(), BeatTime::from_quarters(2));
        assert!(a.overlaps(&b));
        assert!(!a.overlaps(&c));
        assert!(a.contains(BeatTime::ZERO));
        assert!(!a.contains(BeatTime::from_quarters(2)));
        assert_eq!(a.pitch_class(), 0);
    }

    #[test]
    fn note_validation_accepts_good_notes() {
        assert!(note(0, "C4", 0, 1).validate().is_ok());
    }

    #[test]
    fn note_validation_rejects_bad_notes() {
        let mut n = note(0, "C4", 0, 1);
        n.duration = BeatTime::ZERO;
        assert_eq!(n.validate().unwrap_err().code, "INVALID_NOTE");
        let mut n = note(0, "C4", 0, 1);
        n.duration = BeatTime::from_quarters(-1);
        assert!(n.validate().is_err());
        let mut n = note(0, "C4", 0, 1);
        n.midi = 200;
        assert!(n.validate().is_err());
        let mut n = note(0, "C4", 0, 1);
        n.velocity = 0;
        assert!(n.validate().is_err());
        let mut n = note(0, "C4", 0, 1);
        n.channel = 42;
        assert!(n.validate().is_err());
        let mut n = note(0, "C4", 0, 1);
        n.salience = 2.0;
        assert!(n.validate().is_err());
        let mut n = note(0, "C4", 0, 1);
        n.cents = f64::NAN;
        assert!(n.validate().is_err());
    }

    #[test]
    fn note_json_round_trip() {
        let mut n = note(3, "Bb3", 2, 1);
        n.velocity = 100;
        n.channel = 2;
        n.muted = true;
        n.selected = true;
        n.voice = VoiceId(1);
        n.role = NoteRole::Bass;
        n.articulation = Some("staccato".to_string());
        n.salience = 0.5;
        n.nct = vec![NctHypothesis::new(NctKind::Passing, 0.7, "weak beat step")];
        n.confidence = 0.9;
        n.source_ppq = Some(1920.0);
        n.source_take = Some("{GUID}".to_string());
        let back = Note::from_json(&n.to_json()).expect("round trip");
        assert_eq!(back, n);
    }

    #[test]
    fn note_json_defaults_missing_fields() {
        let v = json_obj! { "pitch" => "C4", "onset" => "0", "duration" => "1" };
        let n = Note::from_json(&v).expect("defaults");
        assert_eq!(n.midi, 60);
        assert_eq!(n.velocity, 96);
        assert_eq!(n.role, NoteRole::Unknown);
        assert!(!n.selected);
    }

    #[test]
    fn note_json_rejects_bad_pitch_and_role() {
        let v = json_obj! { "pitch" => "H4", "onset" => "0", "duration" => "1" };
        assert!(Note::from_json(&v).is_err());
        let v =
            json_obj! { "pitch" => "C4", "onset" => "0", "duration" => "1", "role" => "wizard" };
        assert!(Note::from_json(&v).is_err());
        let v = json_obj! { "pitch" => "C4", "duration" => "1" };
        assert!(Note::from_json(&v).is_err());
    }

    #[test]
    fn role_and_nct_identifiers() {
        for r in NoteRole::all() {
            assert_eq!(NoteRole::parse(r.id()), Some(*r));
        }
        for k in NctKind::all() {
            assert_eq!(NctKind::parse(k.id()), Some(*k));
        }
        assert!(NoteRole::parse("nope").is_none());
        assert!(NctKind::parse("nope").is_none());
        assert_eq!(NctKind::all().len(), 12);
    }

    #[test]
    fn nct_hypothesis_json() {
        let h = NctHypothesis::new(NctKind::Suspension, 0.8, "prepared and resolved down");
        let back = NctHypothesis::from_json(&h.to_json()).expect("round trip");
        assert_eq!(back, h);
        let bad = json_obj! { "kind" => "nope" };
        assert!(NctHypothesis::from_json(&bad).is_err());
    }

    #[test]
    fn sorting_is_by_onset_then_pitch_then_id() {
        let set = NoteSet::sorted(
            vec![
                note(2, "G4", 0, 1),
                note(1, "C4", 0, 1),
                note(0, "E4", 1, 1),
            ],
            TimeMap::default(),
        );
        assert_eq!(set.notes[0].id, 1);
        assert_eq!(set.notes[1].id, 2);
        assert_eq!(set.notes[2].id, 0);
    }

    #[test]
    fn span_of_a_set() {
        let set = scale_set();
        assert_eq!(set.span(), (BeatTime::ZERO, BeatTime::from_quarters(4)));
        assert_eq!(NoteSet::default().span(), (BeatTime::ZERO, BeatTime::ZERO));
        assert_eq!(set.len(), 4);
        assert!(!set.is_empty());
        assert!(NoteSet::default().is_empty());
    }

    #[test]
    fn monophony_detection() {
        assert!(scale_set().is_monophonic());
        assert!(!chordal_set().is_monophonic());
        assert!(NoteSet::default().is_monophonic());
        // Touching but not overlapping stays monophonic.
        let touching = NoteSet::sorted(
            vec![note(0, "C4", 0, 1), note(1, "D4", 1, 1)],
            TimeMap::default(),
        );
        assert!(touching.is_monophonic());
    }

    #[test]
    fn polyphony_counts() {
        let set = chordal_set();
        assert_eq!(set.polyphony_at(BeatTime::ZERO), 3);
        assert_eq!(set.polyphony_at(BeatTime::from_quarters(2)), 4);
        assert_eq!(set.polyphony_at(BeatTime::from_quarters(4)), 0);
        assert_eq!(set.max_polyphony(), 4);
        assert_eq!(scale_set().max_polyphony(), 1);
        assert_eq!(NoteSet::default().max_polyphony(), 0);
    }

    #[test]
    fn sounding_at_returns_the_right_notes() {
        let set = chordal_set();
        let sounding = set.sounding_at(BeatTime::from_quarters(2));
        assert_eq!(sounding.len(), 4);
        let names: Vec<String> = sounding.iter().map(|n| n.pitch.to_ascii()).collect();
        assert!(names.contains(&"C5".to_string()));
        assert!(set.sounding_at(BeatTime::from_quarters(10)).is_empty());
    }

    #[test]
    fn voice_filtering() {
        let mut notes = vec![note(0, "C4", 0, 1), note(1, "E4", 0, 1)];
        notes[0].voice = VoiceId(0);
        notes[1].voice = VoiceId(1);
        let set = NoteSet::sorted(notes, TimeMap::default());
        assert_eq!(set.by_voice(VoiceId(0)).len(), 1);
        assert_eq!(set.by_voice(VoiceId(1))[0].pitch.to_ascii(), "E4");
        assert!(set.by_voice(VoiceId(7)).is_empty());
    }

    #[test]
    fn highest_and_lowest_lines() {
        let set = chordal_set();
        let top: Vec<String> = set
            .highest_line()
            .iter()
            .map(|n| n.pitch.to_ascii())
            .collect();
        assert_eq!(top, ["G4", "C5"]);
        let bottom: Vec<String> = set
            .lowest_line()
            .iter()
            .map(|n| n.pitch.to_ascii())
            .collect();
        assert_eq!(bottom, ["C4"]);
    }

    #[test]
    fn lines_of_a_monophonic_set_are_the_set() {
        let set = scale_set();
        assert_eq!(set.highest_line().len(), 4);
        assert_eq!(set.lowest_line().len(), 4);
        assert!(NoteSet::default().highest_line().is_empty());
    }

    #[test]
    fn two_voice_line_extraction() {
        let notes = vec![
            note(0, "E5", 0, 1),
            note(1, "C4", 0, 2),
            note(2, "F5", 1, 1),
            note(3, "G5", 2, 2),
            note(4, "E4", 2, 2),
        ];
        let set = NoteSet::sorted(notes, TimeMap::default());
        let top: Vec<String> = set
            .highest_line()
            .iter()
            .map(|n| n.pitch.to_ascii())
            .collect();
        assert_eq!(top, ["E5", "F5", "G5"]);
        let bottom: Vec<String> = set
            .lowest_line()
            .iter()
            .map(|n| n.pitch.to_ascii())
            .collect();
        assert_eq!(bottom, ["C4", "E4"]);
    }

    #[test]
    fn pitch_class_durations_are_duration_weighted() {
        let set = scale_set();
        let profile = set.pitch_class_durations();
        assert_eq!(profile[0], 1.0);
        assert_eq!(profile[2], 1.0);
        assert_eq!(profile[4], 1.0);
        assert_eq!(profile[5], 1.0);
        assert_eq!(profile[1], 0.0);
        let long = NoteSet::sorted(vec![note(0, "C4", 0, 4)], TimeMap::default());
        assert_eq!(long.pitch_class_durations()[0], 4.0);
    }

    #[test]
    fn muted_notes_are_excluded_from_the_profile() {
        let mut n = note(0, "C4", 0, 4);
        n.muted = true;
        let set = NoteSet::sorted(vec![n, note(1, "D4", 0, 1)], TimeMap::default());
        let profile = set.pitch_class_durations();
        assert_eq!(profile[0], 0.0);
        assert_eq!(profile[2], 1.0);
    }

    #[test]
    fn hashing_is_deterministic_and_content_sensitive() {
        let a = scale_set();
        let b = scale_set();
        assert_eq!(a.hash_hex(), b.hash_hex());
        assert_eq!(a.hash_hex().len(), 64);
        let mut c = scale_set();
        c.notes[0].velocity = 1;
        assert_ne!(a.hash_hex(), c.hash_hex());
        // Spelling matters even when the sound does not.
        let sharp = NoteSet::sorted(vec![note(0, "G#4", 0, 1)], TimeMap::default());
        let flat = NoteSet::sorted(vec![note(0, "Ab4", 0, 1)], TimeMap::default());
        assert_ne!(sharp.hash_hex(), flat.hash_hex());
    }

    #[test]
    fn note_set_json_round_trip() {
        let mut set = chordal_set();
        set.origin = NoteOrigin {
            take_guid: Some("take".to_string()),
            item_guid: Some("item".to_string()),
            track_guid: None,
        };
        let back = NoteSet::from_json(&set.to_json()).expect("round trip");
        assert_eq!(back, set);
        assert_eq!(back.hash_hex(), set.hash_hex());
    }

    #[test]
    fn note_set_validation() {
        assert!(scale_set().validate().is_ok());
        let mut set = scale_set();
        set.notes[1].duration = BeatTime::ZERO;
        assert!(set.validate().is_err());
    }

    #[test]
    fn origin_json_round_trip() {
        let o = NoteOrigin::default();
        assert_eq!(NoteOrigin::from_json(&o.to_json()).unwrap(), o);
    }
}
