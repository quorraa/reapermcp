//! Stage 1 — selection normalisation and melody extraction.
//!
//! This stage answers one question: *which notes are the line we are going to
//! harmonise, and how sure are we?* The second half of that question is the
//! reason this module exists at all. A harmoniser that silently flattens a
//! two-voice invention into "the top notes" and then reports a melody with no
//! caveat is lying to the user, and every downstream confidence is built on
//! that lie.
//!
//! So [`extract`] always reports three things together: the mode it actually
//! used, the assumptions it made in plain language, and a confidence that drops
//! when it had to guess. When the guess is a voice separation over genuinely
//! polyphonic material it also raises an [`AMBIGUOUS_MELODY`] warning.
//!
//! [`AMBIGUOUS_MELODY`]: crate::selection::AMBIGUOUS_MELODY

use crate::error::AnalysisError;
use crate::util::{id_array, num, str_array, warning_array};
use crate::voices::{separate_voices, DEFAULT_MAX_VOICES};
use music_domain::prelude::*;
use qjson::{Json, JsonMap};

/// Warning code raised when the melody had to be guessed out of polyphony.
pub const AMBIGUOUS_MELODY: &str = "AMBIGUOUS_MELODY";

/// Warning code raised when the requested line was empty.
pub const EMPTY_MELODY: &str = "EMPTY_MELODY";

/// Warning code raised when muted notes were dropped from the melody.
pub const MUTED_NOTES_EXCLUDED: &str = "MUTED_NOTES_EXCLUDED";

/// Confidence attached to a line the caller chose explicitly.
const CONFIDENCE_EXPLICIT: f64 = 0.95;
/// Confidence attached to genuinely monophonic material.
const CONFIDENCE_MONOPHONIC: f64 = 0.92;
/// Confidence attached to a selection that narrowed a take down to one line.
const CONFIDENCE_SELECTED_SUBSET: f64 = 0.88;
/// Confidence attached to a top line taken out of real polyphony.
const CONFIDENCE_SEPARATED: f64 = 0.42;

/// How the melody is to be taken out of the selection.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum ExtractionMode {
    /// Prefer the selection, then obvious monophony, then voice separation.
    #[default]
    Auto,
    /// Use exactly the notes the host reports as selected.
    SelectedNotes,
    /// The skyline: the highest note sounding at each onset.
    HighestVoice,
    /// The bass line: the lowest note sounding at each onset.
    LowestVoice,
    /// Everything on one MIDI channel.
    MidiChannel,
    /// The single most prominent separated voice.
    MonophonicVoice,
    /// No melody at all: treat the whole texture as harmony.
    AllNotesAsHarmony,
}

impl ExtractionMode {
    /// Stable identifier used on the wire.
    pub fn id(self) -> &'static str {
        match self {
            ExtractionMode::Auto => "auto",
            ExtractionMode::SelectedNotes => "selected_notes",
            ExtractionMode::HighestVoice => "highest_voice",
            ExtractionMode::LowestVoice => "lowest_voice",
            ExtractionMode::MidiChannel => "midi_channel",
            ExtractionMode::MonophonicVoice => "monophonic_voice",
            ExtractionMode::AllNotesAsHarmony => "all_notes_as_harmony",
        }
    }

    /// Reads the wire form.
    pub fn parse(s: &str) -> Option<ExtractionMode> {
        ExtractionMode::all().iter().copied().find(|m| m.id() == s)
    }

    /// Every mode, in declaration order.
    pub fn all() -> &'static [ExtractionMode] {
        &[
            ExtractionMode::Auto,
            ExtractionMode::SelectedNotes,
            ExtractionMode::HighestVoice,
            ExtractionMode::LowestVoice,
            ExtractionMode::MidiChannel,
            ExtractionMode::MonophonicVoice,
            ExtractionMode::AllNotesAsHarmony,
        ]
    }
}

/// A melody-extraction request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractionRequest {
    /// Which line to take.
    pub mode: ExtractionMode,
    /// The channel, required by [`ExtractionMode::MidiChannel`] and ignored
    /// otherwise.
    pub channel: Option<u8>,
}

impl ExtractionRequest {
    /// A request in `auto` mode.
    pub fn auto() -> ExtractionRequest {
        ExtractionRequest {
            mode: ExtractionMode::Auto,
            channel: None,
        }
    }

    /// A request in an explicit mode with no channel.
    pub fn mode(mode: ExtractionMode) -> ExtractionRequest {
        ExtractionRequest {
            mode,
            channel: None,
        }
    }

    /// A request for one MIDI channel.
    pub fn channel(channel: u8) -> ExtractionRequest {
        ExtractionRequest {
            mode: ExtractionMode::MidiChannel,
            channel: Some(channel),
        }
    }

    /// Canonical JSON form, used in the analysis-id derivation.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("mode", Json::Str(self.mode.id().to_string()));
        m.insert(
            "channel",
            match self.channel {
                Some(c) => Json::Int(c as i64),
                None => Json::Null,
            },
        );
        Json::Obj(m)
    }
}

impl Default for ExtractionRequest {
    fn default() -> Self {
        ExtractionRequest::auto()
    }
}

/// The result of stage 1.
#[derive(Clone, Debug)]
pub struct Extraction {
    /// The line to harmonise.
    pub melody: NoteSet,
    /// Everything in the source that is not in [`Extraction::melody`].
    pub accompaniment: NoteSet,
    /// The mode that was actually used, which may differ from the requested
    /// one when `auto` resolved to a concrete strategy.
    pub mode_used: ExtractionMode,
    /// What had to be assumed, in the user's language.
    pub assumptions: Vec<String>,
    /// Confidence in the extracted line, `0.0..=1.0`.
    pub confidence: f64,
    /// Structured findings, including [`AMBIGUOUS_MELODY`].
    pub warnings: Vec<Warning>,
    /// How many independent lines the source was found to contain.
    pub voice_count: usize,
}

impl Extraction {
    /// JSON form.
    ///
    /// Notes are reported as id lists rather than as full note objects: the
    /// caller already holds the source note set, and repeating it here would
    /// treble the size of every analysis document.
    pub fn to_json(&self) -> Json {
        let melody_ids: Vec<NoteId> = self.melody.notes.iter().map(|n| n.id).collect();
        let acc_ids: Vec<NoteId> = self.accompaniment.notes.iter().map(|n| n.id).collect();
        let mut m = JsonMap::new();
        m.insert("mode_used", Json::Str(self.mode_used.id().to_string()));
        m.insert("voice_count", Json::Int(self.voice_count as i64));
        m.insert("confidence", num(self.confidence));
        m.insert("melody_note_count", Json::Int(melody_ids.len() as i64));
        m.insert("melody_notes", id_array(&melody_ids));
        m.insert("accompaniment_note_count", Json::Int(acc_ids.len() as i64));
        m.insert("accompaniment_notes", id_array(&acc_ids));
        m.insert("assumptions", str_array(&self.assumptions));
        m.insert("warnings", warning_array(&self.warnings));
        Json::Obj(m)
    }

    /// True when a warning with `code` is present.
    pub fn has_warning(&self, code: &str) -> bool {
        self.warnings.iter().any(|w| w.code == code)
    }
}

/// Runs stage 1.
///
/// # Auto
///
/// 1. If any note is selected, the working set is the selected notes. When that
///    is a proper subset of the take the narrowing is recorded as an
///    assumption.
/// 2. If the working set is monophonic it *is* the melody, at high confidence.
/// 3. Otherwise the working set is separated into voices, the top line is
///    taken, the confidence drops to [`CONFIDENCE_SEPARATED`], the assumption
///    is spelled out, and an [`AMBIGUOUS_MELODY`] warning is raised.
///
/// Step 3 is the whole point: polyphonic material never comes back looking like
/// a melody someone typed in.
///
/// # Errors
///
/// * `NO_MIDI_SOURCE` when the source holds no notes.
/// * `INVALID_ARGUMENT` when `midi_channel` is requested without a channel.
/// * `AMBIGUOUS_MELODY` when the requested mode selects no notes at all —
///   `selected_notes` with nothing selected, or a channel nothing is on.
pub fn extract(src: &NoteSet, req: &ExtractionRequest) -> Result<Extraction, AnalysisError> {
    if src.notes.is_empty() {
        return Err(AnalysisError::no_midi_source(
            "the selection contains no notes to analyse",
        ));
    }

    let mut assumptions: Vec<String> = Vec::new();
    let mut warnings: Vec<Warning> = Vec::new();

    let audible: Vec<&Note> = src.notes.iter().filter(|n| !n.muted).collect();
    let muted = src.notes.len() - audible.len();
    if muted > 0 {
        assumptions.push(format!(
            "{muted} muted note(s) were kept in the accompaniment and excluded from the melody, \
             because a muted note does not sound"
        ));
        warnings.push(Warning::new(
            MUTED_NOTES_EXCLUDED,
            format!("{muted} muted note(s) were excluded from the melody"),
            Severity::Info,
        ));
    }
    let audible_set = subset(src, &audible.iter().map(|n| n.id).collect::<Vec<_>>());
    let voice_count = separate_voices(&audible_set, DEFAULT_MAX_VOICES).len();

    let (ids, mode_used, confidence) = match req.mode {
        ExtractionMode::Auto => auto(src, &audible_set, &mut assumptions, &mut warnings),
        ExtractionMode::SelectedNotes => {
            let ids: Vec<NoteId> = audible
                .iter()
                .filter(|n| n.selected)
                .map(|n| n.id)
                .collect();
            if ids.is_empty() {
                return Err(AnalysisError::ambiguous_melody(
                    "mode 'selected_notes' was requested but no audible note is selected",
                ));
            }
            assumptions.push(format!(
                "the {} explicitly selected note(s) are the melody",
                ids.len()
            ));
            let mono = subset(src, &ids).is_monophonic();
            if !mono {
                assumptions.push(
                    "the selected notes still overlap, so the melody is polyphonic as requested"
                        .to_string(),
                );
                warnings.push(ambiguity_warning(
                    "the explicitly selected notes are not a single line",
                ));
            }
            let c = if mono {
                CONFIDENCE_EXPLICIT
            } else {
                CONFIDENCE_SEPARATED
            };
            (ids, ExtractionMode::SelectedNotes, c)
        }
        ExtractionMode::HighestVoice => {
            let ids: Vec<NoteId> = audible_set.highest_line().iter().map(|n| n.id).collect();
            assumptions.push(
                "the highest sounding note at each onset is the melody, as requested".to_string(),
            );
            (ids, ExtractionMode::HighestVoice, CONFIDENCE_EXPLICIT)
        }
        ExtractionMode::LowestVoice => {
            let ids: Vec<NoteId> = audible_set.lowest_line().iter().map(|n| n.id).collect();
            assumptions.push(
                "the lowest sounding note at each onset is the melody, as requested".to_string(),
            );
            (ids, ExtractionMode::LowestVoice, CONFIDENCE_EXPLICIT)
        }
        ExtractionMode::MidiChannel => {
            let Some(ch) = req.channel else {
                return Err(AnalysisError::invalid_argument(
                    "mode 'midi_channel' requires a channel number",
                ));
            };
            let ids: Vec<NoteId> = audible
                .iter()
                .filter(|n| n.channel == ch)
                .map(|n| n.id)
                .collect();
            if ids.is_empty() {
                return Err(AnalysisError::ambiguous_melody(format!(
                    "mode 'midi_channel' was requested for channel {ch}, which carries no notes"
                )));
            }
            assumptions.push(format!(
                "the {} note(s) on MIDI channel {ch} are the melody",
                ids.len()
            ));
            let mono = subset(src, &ids).is_monophonic();
            if !mono {
                warnings.push(ambiguity_warning(&format!(
                    "MIDI channel {ch} carries overlapping notes"
                )));
            }
            let c = if mono {
                CONFIDENCE_EXPLICIT
            } else {
                CONFIDENCE_SEPARATED
            };
            (ids, ExtractionMode::MidiChannel, c)
        }
        ExtractionMode::MonophonicVoice => {
            let lines = separate_voices(&audible_set, DEFAULT_MAX_VOICES);
            let ids = most_prominent(&audible_set, &lines);
            if lines.len() > 1 {
                assumptions.push(format!(
                    "the source separates into {} voices; the single most prominent line was taken",
                    lines.len()
                ));
                warnings.push(ambiguity_warning(
                    "a single voice was taken out of polyphonic material",
                ));
            } else {
                assumptions.push("the source is already a single voice".to_string());
            }
            let c = if lines.len() > 1 {
                CONFIDENCE_SEPARATED
            } else {
                CONFIDENCE_MONOPHONIC
            };
            (ids, ExtractionMode::MonophonicVoice, c)
        }
        ExtractionMode::AllNotesAsHarmony => {
            assumptions.push(
                "no melody was extracted: the whole texture is treated as harmony, as requested"
                    .to_string(),
            );
            (
                Vec::new(),
                ExtractionMode::AllNotesAsHarmony,
                CONFIDENCE_EXPLICIT,
            )
        }
    };

    if ids.is_empty() && req.mode != ExtractionMode::AllNotesAsHarmony {
        warnings.push(Warning::new(
            EMPTY_MELODY,
            "melody extraction produced no notes",
            Severity::Major,
        ));
    }

    let melody = subset(src, &ids);
    let rest: Vec<NoteId> = src
        .notes
        .iter()
        .filter(|n| !ids.contains(&n.id))
        .map(|n| n.id)
        .collect();
    let accompaniment = subset(src, &rest);

    Ok(Extraction {
        melody,
        accompaniment,
        mode_used,
        assumptions,
        confidence,
        warnings,
        voice_count,
    })
}

/// The `auto` decision tree. Returns the melody ids, the mode it resolved to
/// and the confidence that mode earns.
fn auto(
    src: &NoteSet,
    audible: &NoteSet,
    assumptions: &mut Vec<String>,
    warnings: &mut Vec<Warning>,
) -> (Vec<NoteId>, ExtractionMode, f64) {
    // 1: prefer explicitly selected notes.
    let selected: Vec<NoteId> = audible
        .notes
        .iter()
        .filter(|n| n.selected)
        .map(|n| n.id)
        .collect();
    let narrowed = !selected.is_empty() && selected.len() < audible.notes.len();
    let working = if selected.is_empty() {
        audible.clone()
    } else {
        subset(src, &selected)
    };
    if narrowed {
        assumptions.push(format!(
            "the {} explicitly selected note(s) were preferred over the {} in the take",
            selected.len(),
            audible.notes.len()
        ));
    } else if !selected.is_empty() {
        assumptions.push("every note in the take is selected, so nothing was narrowed".to_string());
    } else {
        assumptions.push("no note is selected, so the whole take was considered".to_string());
    }

    // 2: detect obvious monophony.
    if working.is_monophonic() {
        assumptions.push(
            "the material is monophonic — no two notes ever sound together — so it is the melody \
             as written"
                .to_string(),
        );
        let mode = if narrowed {
            ExtractionMode::SelectedNotes
        } else {
            ExtractionMode::MonophonicVoice
        };
        let confidence = if narrowed {
            CONFIDENCE_SELECTED_SUBSET
        } else {
            CONFIDENCE_MONOPHONIC
        };
        let ids = working.notes.iter().map(|n| n.id).collect();
        return (ids, mode, confidence);
    }

    // 3: separate voices and take the top line, saying so.
    let lines = separate_voices(&working, DEFAULT_MAX_VOICES);
    let ids: Vec<NoteId> = lines.first().cloned().unwrap_or_default();
    let mut ordered = ids.clone();
    ordered.sort_unstable();
    assumptions.push(format!(
        "the material is polyphonic (up to {} simultaneous notes, {} separated voices); the top \
         line was assumed to be the melody and the remaining {} note(s) were treated as \
         accompaniment",
        working.max_polyphony(),
        lines.len(),
        working.notes.len().saturating_sub(ordered.len())
    ));
    assumptions.push(
        "this is an assumption, not a reading of the score: another voice may be the intended \
         melody"
            .to_string(),
    );
    warnings.push(ambiguity_warning(&format!(
        "the selection is polyphonic ({} voices); the top line was assumed to be the melody",
        lines.len()
    )));
    (ordered, ExtractionMode::HighestVoice, CONFIDENCE_SEPARATED)
}

/// The line carrying the most sounding time, then the highest register, then
/// the lowest index — a total order, so the choice never wobbles.
fn most_prominent(src: &NoteSet, lines: &[Vec<NoteId>]) -> Vec<NoteId> {
    let mut best: Option<(usize, (i64, i64))> = None;
    for (i, ids) in lines.iter().enumerate() {
        let mut total = BeatTime::ZERO;
        let mut sum_midi = 0i64;
        for id in ids {
            if let Some(n) = src.notes.iter().find(|n| n.id == *id) {
                total = total + n.duration;
                sum_midi += n.midi as i64;
            }
        }
        // Compare durations exactly, as scaled integers, so no float creeps in.
        let key = (total.num() * 1920 / total.den().max(1), sum_midi);
        if best.is_none_or(|(_, b)| key > b) {
            best = Some((i, key));
        }
    }
    best.map(|(i, _)| lines[i].clone()).unwrap_or_default()
}

/// Builds an `AMBIGUOUS_MELODY` warning.
fn ambiguity_warning(detail: &str) -> Warning {
    Warning::new(
        AMBIGUOUS_MELODY,
        format!("{detail}; review the extraction before trusting the harmonisation"),
        Severity::Moderate,
    )
}

/// The sub-set of `src` holding exactly `ids`, keeping the source time map,
/// origin and note order.
fn subset(src: &NoteSet, ids: &[NoteId]) -> NoteSet {
    let notes: Vec<Note> = src
        .notes
        .iter()
        .filter(|n| ids.contains(&n.id))
        .cloned()
        .collect();
    let mut out = NoteSet::sorted(notes, src.time_map.clone());
    out.origin = src.origin.clone();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{note_set, note_set_selected};

    fn poly() -> NoteSet {
        note_set(&[
            ("E5", "0", "1"),
            ("C4", "0", "2"),
            ("F5", "1", "1"),
            ("B3", "2", "2"),
            ("G5", "2", "2"),
            ("A3", "4", "2"),
            ("F5", "4", "1"),
        ])
    }

    #[test]
    fn mode_ids_round_trip() {
        for m in ExtractionMode::all() {
            assert_eq!(ExtractionMode::parse(m.id()), Some(*m));
        }
        assert_eq!(ExtractionMode::parse("nope"), None);
        assert_eq!(ExtractionMode::all().len(), 7);
        assert_eq!(ExtractionMode::default(), ExtractionMode::Auto);
    }

    #[test]
    fn auto_on_monophonic_material_is_confident_and_quiet() {
        let ns = note_set(&[("C4", "0", "1"), ("D4", "1", "1"), ("E4", "2", "2")]);
        let e = extract(&ns, &ExtractionRequest::auto()).expect("extracts");
        assert_eq!(e.melody.notes.len(), 3);
        assert_eq!(e.mode_used, ExtractionMode::MonophonicVoice);
        assert!(e.confidence > 0.9);
        assert!(!e.has_warning(AMBIGUOUS_MELODY));
        assert!(e.accompaniment.notes.is_empty());
    }

    #[test]
    fn auto_on_polyphony_takes_the_top_line_and_says_so() {
        let e = extract(&poly(), &ExtractionRequest::auto()).expect("extracts");
        assert_eq!(e.mode_used, ExtractionMode::HighestVoice);
        assert!(e.confidence < 0.5, "confidence is {}", e.confidence);
        assert!(e.has_warning(AMBIGUOUS_MELODY));
        assert!(
            e.assumptions.iter().any(|a| a.contains("polyphonic")),
            "{:?}",
            e.assumptions
        );
        assert!(!e.accompaniment.notes.is_empty());
        assert!(e.melody.notes.iter().all(|n| n.midi >= 72));
    }

    #[test]
    fn auto_never_claims_polyphony_is_monophonic() {
        let e = extract(&poly(), &ExtractionRequest::auto()).expect("extracts");
        assert_ne!(e.mode_used, ExtractionMode::MonophonicVoice);
        assert!(e.voice_count >= 2);
    }

    #[test]
    fn auto_prefers_an_explicit_selection() {
        let ns = note_set_selected(&[
            ("C4", "0", "1", true),
            ("E4", "1", "1", true),
            ("G5", "0", "4", false),
        ]);
        let e = extract(&ns, &ExtractionRequest::auto()).expect("extracts");
        assert_eq!(e.mode_used, ExtractionMode::SelectedNotes);
        assert_eq!(e.melody.notes.len(), 2);
        assert!(e.assumptions.iter().any(|a| a.contains("preferred")));
    }

    #[test]
    fn melody_and_accompaniment_partition_the_source() {
        let e = extract(&poly(), &ExtractionRequest::auto()).expect("extracts");
        let mut ids: Vec<NoteId> = e
            .melody
            .notes
            .iter()
            .chain(e.accompaniment.notes.iter())
            .map(|n| n.id)
            .collect();
        ids.sort_unstable();
        let mut expected: Vec<NoteId> = poly().notes.iter().map(|n| n.id).collect();
        expected.sort_unstable();
        assert_eq!(ids, expected);
    }

    #[test]
    fn highest_and_lowest_voice_modes_differ() {
        let hi = extract(
            &poly(),
            &ExtractionRequest::mode(ExtractionMode::HighestVoice),
        )
        .expect("extracts");
        let lo = extract(
            &poly(),
            &ExtractionRequest::mode(ExtractionMode::LowestVoice),
        )
        .expect("extracts");
        assert!(hi.melody.notes.iter().all(|n| n.midi >= 72));
        assert!(lo.melody.notes.iter().all(|n| n.midi <= 60));
        assert_eq!(hi.mode_used, ExtractionMode::HighestVoice);
        assert_eq!(lo.mode_used, ExtractionMode::LowestVoice);
    }

    #[test]
    fn selected_notes_mode_needs_a_selection() {
        let ns = note_set_selected(&[("C4", "0", "1", false), ("D4", "1", "1", false)]);
        let e = extract(&ns, &ExtractionRequest::mode(ExtractionMode::SelectedNotes));
        assert_eq!(e.unwrap_err().code, "AMBIGUOUS_MELODY");
    }

    #[test]
    fn midi_channel_mode_needs_a_channel() {
        let ns = note_set(&[("C4", "0", "1")]);
        let e = extract(&ns, &ExtractionRequest::mode(ExtractionMode::MidiChannel));
        assert_eq!(e.unwrap_err().code, "INVALID_ARGUMENT");
    }

    #[test]
    fn midi_channel_mode_filters_by_channel() {
        let mut ns = note_set(&[("C4", "0", "1"), ("E4", "1", "1"), ("G4", "2", "1")]);
        ns.notes[1].channel = 3;
        let e = extract(&ns, &ExtractionRequest::channel(3)).expect("extracts");
        assert_eq!(e.melody.notes.len(), 1);
        assert_eq!(e.melody.notes[0].midi, 64);
        assert_eq!(e.accompaniment.notes.len(), 2);
        assert_eq!(e.mode_used, ExtractionMode::MidiChannel);
    }

    #[test]
    fn an_empty_channel_is_an_error_not_an_empty_melody() {
        let ns = note_set(&[("C4", "0", "1")]);
        let e = extract(&ns, &ExtractionRequest::channel(9));
        assert_eq!(e.unwrap_err().code, "AMBIGUOUS_MELODY");
    }

    #[test]
    fn all_notes_as_harmony_leaves_no_melody() {
        let e = extract(
            &poly(),
            &ExtractionRequest::mode(ExtractionMode::AllNotesAsHarmony),
        )
        .expect("extracts");
        assert!(e.melody.notes.is_empty());
        assert_eq!(e.accompaniment.notes.len(), poly().notes.len());
        assert!(
            !e.has_warning(EMPTY_MELODY),
            "harmony mode is not a failure"
        );
    }

    #[test]
    fn monophonic_voice_mode_takes_the_most_prominent_line() {
        let e = extract(
            &poly(),
            &ExtractionRequest::mode(ExtractionMode::MonophonicVoice),
        )
        .expect("extracts");
        assert!(e.melody.is_monophonic());
        assert!(e.has_warning(AMBIGUOUS_MELODY));
    }

    #[test]
    fn muted_notes_never_enter_the_melody() {
        let mut ns = note_set(&[("C4", "0", "1"), ("D4", "1", "1")]);
        ns.notes[1].muted = true;
        let e = extract(&ns, &ExtractionRequest::auto()).expect("extracts");
        assert_eq!(e.melody.notes.len(), 1);
        assert!(e.has_warning(MUTED_NOTES_EXCLUDED));
        assert_eq!(e.accompaniment.notes.len(), 1);
    }

    #[test]
    fn an_empty_source_is_no_midi_source() {
        let e = extract(&NoteSet::default(), &ExtractionRequest::auto());
        assert_eq!(e.unwrap_err().code, "NO_MIDI_SOURCE");
    }

    #[test]
    fn extraction_json_reports_the_assumption_and_confidence() {
        let e = extract(&poly(), &ExtractionRequest::auto()).expect("extracts");
        let j = e.to_json();
        assert_eq!(
            j.get("mode_used").and_then(Json::as_str),
            Some("highest_voice")
        );
        assert!(j.get("assumptions").and_then(Json::as_arr).unwrap().len() >= 2);
        assert!(!j.get("warnings").and_then(Json::as_arr).unwrap().is_empty());
        assert!(j.get("confidence").and_then(Json::as_f64).unwrap() < 0.5);
    }

    #[test]
    fn extraction_is_deterministic() {
        let a = extract(&poly(), &ExtractionRequest::auto()).expect("extracts");
        let b = extract(&poly(), &ExtractionRequest::auto()).expect("extracts");
        assert_eq!(
            a.to_json().to_canonical_string(),
            b.to_json().to_canonical_string()
        );
    }

    #[test]
    fn request_json_is_canonical() {
        let j = ExtractionRequest::channel(2).to_json();
        assert_eq!(
            j.to_canonical_string(),
            "{\"channel\":2,\"mode\":\"midi_channel\"}"
        );
    }
}
