//! Musical structure above the note: phrases, motives, cadences, key and
//! harmonic regions, sections, arrangement roles, loop intents and
//! voice-leading connections.
//!
//! These are plain data types with JSON forms; the analysis and generation
//! crates fill them in.

use crate::chord::HarmonicFunction;
use crate::error::DomainError;
use crate::note::{NoteId, VoiceId};
use crate::pitch::{Accidental, Letter, SpelledPitch};
use crate::time::BeatTime;
use qjson::{json_obj, Json};

/// Renders a spelled pitch class such as `"Bb"`.
fn class_text((l, a): (Letter, Accidental)) -> String {
    format!("{}{}", l.as_char(), a.ascii())
}

/// Reads a spelled pitch class from a JSON string field.
fn class_field(v: &Json, key: &str) -> Result<(Letter, Accidental), DomainError> {
    let text = v.str_field(key)?;
    SpelledPitch::parse_class(text)
        .ok_or_else(|| DomainError::invalid_pitch(format!("bad pitch class {text:?}")))
}

/// Renders a note-id list.
fn id_array(ids: &[NoteId]) -> Json {
    Json::Arr(ids.iter().map(|i| Json::Int(*i as i64)).collect())
}

/// Reads a note-id list, ignoring non-integer entries.
fn id_list(v: &Json, key: &str) -> Vec<NoteId> {
    match v.get(key) {
        Some(Json::Arr(a)) => a
            .iter()
            .filter_map(|x| x.as_i64())
            .map(|n| n.clamp(0, u32::MAX as i64) as NoteId)
            .collect(),
        _ => Vec::new(),
    }
}

/// Reads a string list.
fn string_list(v: &Json, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Json::Arr(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// A melodic phrase.
#[derive(Clone, Debug, PartialEq)]
pub struct Phrase {
    /// Stable id within the analysis.
    pub id: u32,
    /// Start position.
    pub start: BeatTime,
    /// End position.
    pub end: BeatTime,
    /// Notes belonging to the phrase.
    pub notes: Vec<NoteId>,
    /// Cadence closing the phrase, when one was identified.
    pub cadence: Option<CadenceKind>,
    /// True when the phrase begins with a pickup.
    pub is_pickup: bool,
    /// Confidence in the segmentation, `0.0..=1.0`.
    pub confidence: f64,
}

impl Phrase {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "id" => self.id as i64,
            "start" => self.start.to_json(),
            "end" => self.end.to_json(),
            "notes" => id_array(&self.notes),
            "cadence" => match self.cadence { Some(c) => Json::Str(c.id().to_string()), None => Json::Null },
            "is_pickup" => self.is_pickup,
            "confidence" => self.confidence,
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<Phrase, DomainError> {
        let cadence = match v.get("cadence") {
            Some(Json::Str(s)) => Some(
                CadenceKind::parse(s)
                    .ok_or_else(|| DomainError::invalid_argument(format!("bad cadence {s:?}")))?,
            ),
            _ => None,
        };
        Ok(Phrase {
            id: v
                .opt_i64_field("id")?
                .unwrap_or(0)
                .clamp(0, u32::MAX as i64) as u32,
            start: BeatTime::from_json(v.field("start")?)?,
            end: BeatTime::from_json(v.field("end")?)?,
            notes: id_list(v, "notes"),
            cadence,
            is_pickup: v.opt_bool_field("is_pickup")?.unwrap_or(false),
            confidence: v.opt_f64_field("confidence")?.unwrap_or(0.0),
        })
    }

    /// Length in quarter notes.
    pub fn length(&self) -> BeatTime {
        self.end - self.start
    }
}

/// A recurring melodic idea.
#[derive(Clone, Debug, PartialEq)]
pub struct Motive {
    /// Stable id within the analysis.
    pub id: u32,
    /// Where it occurs.
    pub occurrences: Vec<MotiveOccurrence>,
    /// Interval profile in semitones between consecutive notes.
    pub interval_profile: Vec<i32>,
    /// Rhythm profile as inter-onset durations.
    pub rhythm_profile: Vec<BeatTime>,
    /// How prominent the motive is, `0.0..=1.0`.
    pub salience: f64,
}

impl Motive {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "id" => self.id as i64,
            "occurrences" => Json::Arr(self.occurrences.iter().map(|o| o.to_json()).collect()),
            "interval_profile" => Json::Arr(self.interval_profile.iter().map(|i| Json::Int(*i as i64)).collect()),
            "rhythm_profile" => Json::Arr(self.rhythm_profile.iter().map(|r| r.to_json()).collect()),
            "salience" => self.salience,
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<Motive, DomainError> {
        let mut occurrences = Vec::new();
        if let Some(Json::Arr(a)) = v.get("occurrences") {
            for o in a {
                occurrences.push(MotiveOccurrence::from_json(o)?);
            }
        }
        let mut rhythm_profile = Vec::new();
        if let Some(Json::Arr(a)) = v.get("rhythm_profile") {
            for r in a {
                rhythm_profile.push(BeatTime::from_json(r)?);
            }
        }
        let interval_profile = match v.get("interval_profile") {
            Some(Json::Arr(a)) => a
                .iter()
                .filter_map(|x| x.as_i64())
                .map(|n| n as i32)
                .collect(),
            _ => Vec::new(),
        };
        Ok(Motive {
            id: v
                .opt_i64_field("id")?
                .unwrap_or(0)
                .clamp(0, u32::MAX as i64) as u32,
            occurrences,
            interval_profile,
            rhythm_profile,
            salience: v.opt_f64_field("salience")?.unwrap_or(0.0),
        })
    }
}

/// One appearance of a motive.
#[derive(Clone, Debug, PartialEq)]
pub struct MotiveOccurrence {
    /// Where it starts.
    pub start: BeatTime,
    /// Notes involved.
    pub notes: Vec<NoteId>,
    /// Transposition in semitones from the reference form.
    pub transposition: i32,
    /// How the reference form was transformed.
    pub transform: MotiveTransform,
}

impl MotiveOccurrence {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "start" => self.start.to_json(),
            "notes" => id_array(&self.notes),
            "transposition" => self.transposition as i64,
            "transform" => self.transform.id(),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<MotiveOccurrence, DomainError> {
        let t = v.str_field("transform")?;
        let transform = MotiveTransform::parse(t)
            .ok_or_else(|| DomainError::invalid_argument(format!("bad transform {t:?}")))?;
        Ok(MotiveOccurrence {
            start: BeatTime::from_json(v.field("start")?)?,
            notes: id_list(v, "notes"),
            transposition: v.opt_i64_field("transposition")?.unwrap_or(0) as i32,
            transform,
        })
    }
}

/// How a motive occurrence relates to its reference form.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MotiveTransform {
    /// Identical.
    Exact,
    /// Same shape at a different pitch level.
    Transposed,
    /// Repeated at successive pitch levels.
    Sequence,
    /// Intervals inverted.
    Inverted,
    /// Order reversed.
    Retrograde,
    /// Rhythmically lengthened.
    Augmented,
    /// Rhythmically shortened.
    Diminished,
    /// Recognisably related but freely varied.
    Varied,
}

impl MotiveTransform {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            MotiveTransform::Exact => "exact",
            MotiveTransform::Transposed => "transposed",
            MotiveTransform::Sequence => "sequence",
            MotiveTransform::Inverted => "inverted",
            MotiveTransform::Retrograde => "retrograde",
            MotiveTransform::Augmented => "augmented",
            MotiveTransform::Diminished => "diminished",
            MotiveTransform::Varied => "varied",
        }
    }

    /// Parses the identifier produced by [`MotiveTransform::id`].
    pub fn parse(s: &str) -> Option<Self> {
        MotiveTransform::all().iter().copied().find(|t| t.id() == s)
    }

    /// Every variant, in declaration order.
    pub fn all() -> &'static [MotiveTransform] {
        &[
            MotiveTransform::Exact,
            MotiveTransform::Transposed,
            MotiveTransform::Sequence,
            MotiveTransform::Inverted,
            MotiveTransform::Retrograde,
            MotiveTransform::Augmented,
            MotiveTransform::Diminished,
            MotiveTransform::Varied,
        ]
    }
}

/// Cadence types recognised by the analyzer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CadenceKind {
    /// Perfect authentic cadence.
    PerfectAuthentic,
    /// Imperfect authentic cadence.
    ImperfectAuthentic,
    /// Half cadence.
    Half,
    /// Plagal cadence.
    Plagal,
    /// Deceptive cadence.
    Deceptive,
    /// Phrygian cadence.
    Phrygian,
    /// Modal cadence without functional dominant motion.
    Modal,
    /// No cadence.
    None,
}

impl CadenceKind {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            CadenceKind::PerfectAuthentic => "perfect_authentic",
            CadenceKind::ImperfectAuthentic => "imperfect_authentic",
            CadenceKind::Half => "half",
            CadenceKind::Plagal => "plagal",
            CadenceKind::Deceptive => "deceptive",
            CadenceKind::Phrygian => "phrygian",
            CadenceKind::Modal => "modal",
            CadenceKind::None => "none",
        }
    }

    /// Parses the identifier produced by [`CadenceKind::id`].
    pub fn parse(s: &str) -> Option<Self> {
        CadenceKind::all().iter().copied().find(|c| c.id() == s)
    }

    /// Every variant, in declaration order.
    pub fn all() -> &'static [CadenceKind] {
        &[
            CadenceKind::PerfectAuthentic,
            CadenceKind::ImperfectAuthentic,
            CadenceKind::Half,
            CadenceKind::Plagal,
            CadenceKind::Deceptive,
            CadenceKind::Phrygian,
            CadenceKind::Modal,
            CadenceKind::None,
        ]
    }
}

/// A stretch of music governed by one key or modal centre.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyRegion {
    /// Start position.
    pub start: BeatTime,
    /// End position.
    pub end: BeatTime,
    /// Spelled tonic.
    pub tonic: (Letter, Accidental),
    /// Scale identifier from the knowledge base.
    pub scale_id: String,
    /// Confidence, `0.0..=1.0`.
    pub confidence: f64,
    /// Evidence strings backing the decision.
    pub evidence: Vec<String>,
    /// True for a local tonicization rather than a full modulation.
    pub is_tonicization: bool,
}

impl KeyRegion {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "start" => self.start.to_json(),
            "end" => self.end.to_json(),
            "tonic" => class_text(self.tonic),
            "scale_id" => self.scale_id.clone(),
            "confidence" => self.confidence,
            "evidence" => Json::Arr(self.evidence.iter().map(|e| Json::Str(e.clone())).collect()),
            "is_tonicization" => self.is_tonicization,
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<KeyRegion, DomainError> {
        Ok(KeyRegion {
            start: BeatTime::from_json(v.field("start")?)?,
            end: BeatTime::from_json(v.field("end")?)?,
            tonic: class_field(v, "tonic")?,
            scale_id: v.str_field("scale_id")?.to_string(),
            confidence: v.opt_f64_field("confidence")?.unwrap_or(0.0),
            evidence: string_list(v, "evidence"),
            is_tonicization: v.opt_bool_field("is_tonicization")?.unwrap_or(false),
        })
    }
}

/// A stretch of music governed by one harmonic function.
#[derive(Clone, Debug, PartialEq)]
pub struct HarmonicRegion {
    /// Start position.
    pub start: BeatTime,
    /// End position.
    pub end: BeatTime,
    /// Human-readable label.
    pub label: String,
    /// The function this region performs.
    pub function: HarmonicFunction,
}

impl HarmonicRegion {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "start" => self.start.to_json(),
            "end" => self.end.to_json(),
            "label" => self.label.clone(),
            "function" => self.function.id(),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<HarmonicRegion, DomainError> {
        let f = v.str_field("function")?;
        let function = HarmonicFunction::parse(f)
            .ok_or_else(|| DomainError::invalid_argument(format!("bad function {f:?}")))?;
        Ok(HarmonicRegion {
            start: BeatTime::from_json(v.field("start")?)?,
            end: BeatTime::from_json(v.field("end")?)?,
            label: v.opt_str_field("label")?.unwrap_or("").to_string(),
            function,
        })
    }
}

/// A formal section such as a verse or chorus.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    /// Stable identifier.
    pub id: String,
    /// Start position.
    pub start: BeatTime,
    /// End position.
    pub end: BeatTime,
    /// Formal role, e.g. `"verse"`, `"chorus"`, `"bridge"`.
    pub role: String,
    /// Energy level, `0.0..=1.0`.
    pub energy: f64,
}

impl Section {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "id" => self.id.clone(),
            "start" => self.start.to_json(),
            "end" => self.end.to_json(),
            "role" => self.role.clone(),
            "energy" => self.energy,
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<Section, DomainError> {
        Ok(Section {
            id: v.str_field("id")?.to_string(),
            start: BeatTime::from_json(v.field("start")?)?,
            end: BeatTime::from_json(v.field("end")?)?,
            role: v.opt_str_field("role")?.unwrap_or("").to_string(),
            energy: v.opt_f64_field("energy")?.unwrap_or(0.0),
        })
    }
}

/// The job a generated part performs in an arrangement.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ArrangementRole {
    /// Principal melodic line.
    Lead,
    /// Secondary melodic line.
    Counterlead,
    /// Bass line.
    Bass,
    /// Sustained harmonic bed.
    HarmonicBed,
    /// Pad.
    Pad,
    /// Rhythmic comping.
    Comping,
    /// Steady pulse.
    Pulse,
    /// Repeating figure.
    Ostinato,
    /// Riff.
    Riff,
    /// Percussion.
    Percussion,
    /// Impact or accent.
    Impact,
    /// Transition effect.
    Transition,
    /// Textural layer.
    Texture,
    /// Ambience.
    Ambience,
    /// Ornamental figure.
    Ornament,
    /// Ear candy.
    EarCandy,
}

impl ArrangementRole {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            ArrangementRole::Lead => "lead",
            ArrangementRole::Counterlead => "counterlead",
            ArrangementRole::Bass => "bass",
            ArrangementRole::HarmonicBed => "harmonic_bed",
            ArrangementRole::Pad => "pad",
            ArrangementRole::Comping => "comping",
            ArrangementRole::Pulse => "pulse",
            ArrangementRole::Ostinato => "ostinato",
            ArrangementRole::Riff => "riff",
            ArrangementRole::Percussion => "percussion",
            ArrangementRole::Impact => "impact",
            ArrangementRole::Transition => "transition",
            ArrangementRole::Texture => "texture",
            ArrangementRole::Ambience => "ambience",
            ArrangementRole::Ornament => "ornament",
            ArrangementRole::EarCandy => "ear_candy",
        }
    }

    /// Parses the identifier produced by [`ArrangementRole::id`].
    pub fn parse(s: &str) -> Option<Self> {
        ArrangementRole::all().iter().copied().find(|r| r.id() == s)
    }

    /// Every variant, in declaration order.
    pub fn all() -> &'static [ArrangementRole] {
        &[
            ArrangementRole::Lead,
            ArrangementRole::Counterlead,
            ArrangementRole::Bass,
            ArrangementRole::HarmonicBed,
            ArrangementRole::Pad,
            ArrangementRole::Comping,
            ArrangementRole::Pulse,
            ArrangementRole::Ostinato,
            ArrangementRole::Riff,
            ArrangementRole::Percussion,
            ArrangementRole::Impact,
            ArrangementRole::Transition,
            ArrangementRole::Texture,
            ArrangementRole::Ambience,
            ArrangementRole::Ornament,
            ArrangementRole::EarCandy,
        ]
    }
}

/// What a loop is meant to do at its wrap point.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LoopIntent {
    /// Resolves to the tonic at the wrap.
    ClosedTonic,
    /// Ends on the dominant, expecting to continue.
    OpenDominant,
    /// Static modal drone.
    ModalDrone,
    /// Colour change without functional closure.
    SeamlessColor,
    /// Prepared to move into another section.
    TransitionReady,
    /// One-shot with an ending.
    OneShotEnding,
}

impl LoopIntent {
    /// Stable identifier used in JSON and fixtures.
    pub fn id(self) -> &'static str {
        match self {
            LoopIntent::ClosedTonic => "closed_tonic",
            LoopIntent::OpenDominant => "open_dominant",
            LoopIntent::ModalDrone => "modal_drone",
            LoopIntent::SeamlessColor => "seamless_color",
            LoopIntent::TransitionReady => "transition_ready",
            LoopIntent::OneShotEnding => "one_shot_ending",
        }
    }

    /// Parses the identifier produced by [`LoopIntent::id`].
    pub fn parse(s: &str) -> Option<Self> {
        LoopIntent::all().iter().copied().find(|i| i.id() == s)
    }

    /// Every variant, in declaration order.
    pub fn all() -> &'static [LoopIntent] {
        &[
            LoopIntent::ClosedTonic,
            LoopIntent::OpenDominant,
            LoopIntent::ModalDrone,
            LoopIntent::SeamlessColor,
            LoopIntent::TransitionReady,
            LoopIntent::OneShotEnding,
        ]
    }
}

/// One voice's motion from one chord to the next.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceLeadingConnection {
    /// Departing pitch.
    pub from: SpelledPitch,
    /// Arriving pitch.
    pub to: SpelledPitch,
    /// Which voice moved.
    pub voice: VoiceId,
    /// Size class of the motion.
    pub motion: MotionKind,
    /// Signed semitone distance.
    pub semitones: i32,
}

impl VoiceLeadingConnection {
    /// Builds a connection, deriving the motion class and distance.
    pub fn new(from: SpelledPitch, to: SpelledPitch, voice: VoiceId) -> VoiceLeadingConnection {
        let semitones = to.midi() - from.midi();
        let motion = match semitones.abs() {
            0 => MotionKind::Static,
            1 | 2 => MotionKind::Step,
            _ => MotionKind::Leap,
        };
        VoiceLeadingConnection {
            from,
            to,
            voice,
            motion,
            semitones,
        }
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "from" => self.from.to_ascii(),
            "to" => self.to.to_ascii(),
            "voice" => self.voice.0 as i64,
            "motion" => self.motion.id(),
            "semitones" => self.semitones as i64,
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<VoiceLeadingConnection, DomainError> {
        let parse_pitch = |key: &str| -> Result<SpelledPitch, DomainError> {
            let text = v.str_field(key)?;
            SpelledPitch::parse(text)
                .ok_or_else(|| DomainError::invalid_pitch(format!("bad pitch {text:?}")))
        };
        let m = v.str_field("motion")?;
        let motion = MotionKind::parse(m)
            .ok_or_else(|| DomainError::invalid_argument(format!("bad motion {m:?}")))?;
        Ok(VoiceLeadingConnection {
            from: parse_pitch("from")?,
            to: parse_pitch("to")?,
            voice: VoiceId(
                v.opt_i64_field("voice")?
                    .unwrap_or(0)
                    .clamp(0, u16::MAX as i64) as u16,
            ),
            motion,
            semitones: v.opt_i64_field("semitones")?.unwrap_or(0) as i32,
        })
    }
}

/// Size class of a single voice's motion.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MotionKind {
    /// No motion — a common tone.
    Static,
    /// A step of one or two semitones.
    Step,
    /// Anything larger.
    Leap,
}

impl MotionKind {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            MotionKind::Static => "static",
            MotionKind::Step => "step",
            MotionKind::Leap => "leap",
        }
    }

    /// Parses the identifier produced by [`MotionKind::id`].
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "static" => Some(MotionKind::Static),
            "step" => Some(MotionKind::Step),
            "leap" => Some(MotionKind::Leap),
            _ => None,
        }
    }
}

/// How two voices move relative to each other.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RelativeMotion {
    /// Same direction, same interval.
    Parallel,
    /// Same direction, different interval.
    Similar,
    /// Opposite directions.
    Contrary,
    /// One voice holds.
    Oblique,
    /// Neither voice moves.
    Static,
}

impl RelativeMotion {
    /// Stable identifier used in JSON.
    pub fn id(self) -> &'static str {
        match self {
            RelativeMotion::Parallel => "parallel",
            RelativeMotion::Similar => "similar",
            RelativeMotion::Contrary => "contrary",
            RelativeMotion::Oblique => "oblique",
            RelativeMotion::Static => "static",
        }
    }

    /// Parses the identifier produced by [`RelativeMotion::id`].
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "parallel" => Some(RelativeMotion::Parallel),
            "similar" => Some(RelativeMotion::Similar),
            "contrary" => Some(RelativeMotion::Contrary),
            "oblique" => Some(RelativeMotion::Oblique),
            "static" => Some(RelativeMotion::Static),
            _ => None,
        }
    }

    /// Classifies two simultaneous motions by their signed semitone deltas.
    pub fn classify(a: i32, b: i32) -> RelativeMotion {
        match (a, b) {
            (0, 0) => RelativeMotion::Static,
            (0, _) | (_, 0) => RelativeMotion::Oblique,
            _ if (a > 0) != (b > 0) => RelativeMotion::Contrary,
            _ if a == b => RelativeMotion::Parallel,
            _ => RelativeMotion::Similar,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qn(n: i64) -> BeatTime {
        BeatTime::from_quarters(n)
    }

    #[test]
    fn phrase_json_round_trip() {
        let p = Phrase {
            id: 1,
            start: qn(0),
            end: qn(8),
            notes: vec![0, 1, 2],
            cadence: Some(CadenceKind::PerfectAuthentic),
            is_pickup: true,
            confidence: 0.8,
        };
        assert_eq!(Phrase::from_json(&p.to_json()).unwrap(), p);
        assert_eq!(p.length(), qn(8));
        let none = Phrase {
            cadence: None,
            ..p.clone()
        };
        assert_eq!(Phrase::from_json(&none.to_json()).unwrap(), none);
    }

    #[test]
    fn motive_json_round_trip() {
        let m = Motive {
            id: 2,
            occurrences: vec![MotiveOccurrence {
                start: qn(4),
                notes: vec![3, 4],
                transposition: -2,
                transform: MotiveTransform::Sequence,
            }],
            interval_profile: vec![2, -1, 3],
            rhythm_profile: vec![BeatTime::new(1, 2), qn(1)],
            salience: 0.6,
        };
        assert_eq!(Motive::from_json(&m.to_json()).unwrap(), m);
    }

    #[test]
    fn key_region_json_round_trip() {
        let k = KeyRegion {
            start: qn(0),
            end: qn(16),
            tonic: (Letter::E, Accidental::FLAT),
            scale_id: "major".to_string(),
            confidence: 0.9,
            evidence: vec!["cadence".to_string()],
            is_tonicization: false,
        };
        assert_eq!(KeyRegion::from_json(&k.to_json()).unwrap(), k);
        let bad = json_obj! { "start" => "0", "end" => "4", "tonic" => "H", "scale_id" => "major" };
        assert!(KeyRegion::from_json(&bad).is_err());
    }

    #[test]
    fn harmonic_region_and_section_json() {
        let h = HarmonicRegion {
            start: qn(0),
            end: qn(4),
            label: "ii".to_string(),
            function: HarmonicFunction::Predominant,
        };
        assert_eq!(HarmonicRegion::from_json(&h.to_json()).unwrap(), h);
        let s = Section {
            id: "A".to_string(),
            start: qn(0),
            end: qn(32),
            role: "verse".to_string(),
            energy: 0.4,
        };
        assert_eq!(Section::from_json(&s.to_json()).unwrap(), s);
    }

    #[test]
    fn enum_identifiers_round_trip() {
        for t in MotiveTransform::all() {
            assert_eq!(MotiveTransform::parse(t.id()), Some(*t));
        }
        for c in CadenceKind::all() {
            assert_eq!(CadenceKind::parse(c.id()), Some(*c));
        }
        for r in ArrangementRole::all() {
            assert_eq!(ArrangementRole::parse(r.id()), Some(*r));
        }
        for i in LoopIntent::all() {
            assert_eq!(LoopIntent::parse(i.id()), Some(*i));
        }
        for m in [MotionKind::Static, MotionKind::Step, MotionKind::Leap] {
            assert_eq!(MotionKind::parse(m.id()), Some(m));
        }
        for m in [
            RelativeMotion::Parallel,
            RelativeMotion::Similar,
            RelativeMotion::Contrary,
            RelativeMotion::Oblique,
            RelativeMotion::Static,
        ] {
            assert_eq!(RelativeMotion::parse(m.id()), Some(m));
        }
        assert!(ArrangementRole::parse("nope").is_none());
        assert!(LoopIntent::parse("nope").is_none());
        assert_eq!(ArrangementRole::all().len(), 16);
        assert_eq!(LoopIntent::all().len(), 6);
    }

    #[test]
    fn fixture_loop_intents_are_recognised() {
        assert_eq!(
            LoopIntent::parse("closed_tonic"),
            Some(LoopIntent::ClosedTonic)
        );
        assert_eq!(
            LoopIntent::parse("modal_drone"),
            Some(LoopIntent::ModalDrone)
        );
    }

    #[test]
    fn voice_leading_connection_classifies_motion() {
        let c4 = SpelledPitch::parse("C4").unwrap();
        let d4 = SpelledPitch::parse("D4").unwrap();
        let g4 = SpelledPitch::parse("G4").unwrap();
        assert_eq!(
            VoiceLeadingConnection::new(c4, d4, VoiceId(0)).motion,
            MotionKind::Step
        );
        assert_eq!(
            VoiceLeadingConnection::new(c4, g4, VoiceId(0)).motion,
            MotionKind::Leap
        );
        assert_eq!(
            VoiceLeadingConnection::new(c4, c4, VoiceId(0)).motion,
            MotionKind::Static
        );
        let vl = VoiceLeadingConnection::new(g4, c4, VoiceId(2));
        assert_eq!(vl.semitones, -7);
        assert_eq!(
            VoiceLeadingConnection::from_json(&vl.to_json()).unwrap(),
            vl
        );
    }

    #[test]
    fn relative_motion_classification() {
        assert_eq!(RelativeMotion::classify(0, 0), RelativeMotion::Static);
        assert_eq!(RelativeMotion::classify(0, 2), RelativeMotion::Oblique);
        assert_eq!(RelativeMotion::classify(2, 2), RelativeMotion::Parallel);
        assert_eq!(RelativeMotion::classify(2, 3), RelativeMotion::Similar);
        assert_eq!(RelativeMotion::classify(2, -2), RelativeMotion::Contrary);
    }
}
