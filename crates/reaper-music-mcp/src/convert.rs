//! Translating bridge snapshots into the domain model.
//!
//! The bridge speaks floating-point quarter notes because that is what REAPER
//! hands it; the analysis engine speaks [`BeatTime`], an exact rational. This
//! module is the single place that crosses between them, so the rounding
//! decision is made once and is testable.
//!
//! Positions are kept in the **project** quarter-note domain rather than the
//! item-relative one. That matters downstream: the edit plan places new items
//! at project quarter notes, so keeping one domain end to end means a generated
//! chord lands under the melody note it was written for without any further
//! arithmetic.

use crate::error::ToolError;
use music_domain::prelude::*;
use reaper_ipc::Snapshot;

/// The grid a floating-point quarter note is snapped to.
///
/// 1920 = 2^7 · 3 · 5 covers 128th notes, triplets and quintuplets exactly,
/// which is every subdivision REAPER's own grid offers.
pub const QN_GRID: i64 = 1920;

/// Builds the time map a snapshot implies.
///
/// The bridge always reports at least one marker (a synthetic one when the
/// project has no explicit tempo map), so the result is always usable.
pub fn time_map_of(snapshot: &Snapshot) -> TimeMap {
    let mut tempos = Vec::new();
    let mut meters = Vec::new();
    for m in &snapshot.tempo_markers {
        let qn = BeatTime::from_f64_grid(m.qn, QN_GRID);
        if m.bpm.is_finite() && m.bpm > 0.0 {
            tempos.push(TempoEvent::new(qn, m.bpm, m.linear));
        }
        if m.timesig_num > 0 && m.timesig_den > 0 {
            meters.push(MeterEvent::new(
                qn,
                TimeSignature::new(m.timesig_num as u16, m.timesig_den as u16),
                0,
            ));
        }
    }
    if tempos.is_empty() && meters.is_empty() {
        return TimeMap::constant(120.0, TimeSignature::new(4, 4));
    }
    TimeMap::new(tempos, meters)
}

/// Converts a snapshot's note list into a [`NoteSet`].
///
/// Ids are the snapshot's own `index` values, so a note in an analysis, a
/// decision trace and a loop report all refer to the same note the bridge
/// reported.
pub fn note_set_of(snapshot: &Snapshot) -> Result<NoteSet, ToolError> {
    let time_map = time_map_of(snapshot);
    let mut notes = Vec::with_capacity(snapshot.notes.len());
    for n in &snapshot.notes {
        if !(0..=127).contains(&n.pitch) {
            return Err(ToolError::invalid_argument(format!(
                "the bridge reported a note with pitch {} outside 0..=127",
                n.pitch
            )));
        }
        let onset = BeatTime::from_f64_grid(n.start_qn, QN_GRID);
        let mut end = BeatTime::from_f64_grid(n.end_qn, QN_GRID);
        if end <= onset {
            end = onset + BeatTime::new(1, QN_GRID);
        }
        let midi = n.pitch as i32;
        let mut note = Note::new(
            n.index.max(0) as NoteId,
            SpelledPitch::from_midi(midi, None),
            onset,
            end - onset,
        );
        note.midi = midi;
        note.velocity = n.velocity.clamp(1, 127) as u8;
        note.channel = n.channel.clamp(0, 15) as u8;
        note.muted = n.muted;
        note.selected = n.selected;
        notes.push(note);
    }
    let mut set = NoteSet::sorted(notes, time_map);
    set.origin = NoteOrigin {
        take_guid: snapshot.take_guid.clone(),
        item_guid: snapshot.item_guid.clone(),
        track_guid: snapshot.track_guid.clone(),
    };
    Ok(set)
}

/// The item's bounds in the project quarter-note domain.
pub fn item_bounds(snapshot: &Snapshot) -> (f64, f64) {
    let start = snapshot.item_position_qn;
    let end = if snapshot.item_end_qn > start {
        snapshot.item_end_qn
    } else {
        start + snapshot.item_length_qn.max(1.0)
    };
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qjson::Json;

    fn scene() -> Snapshot {
        Snapshot::from_json(&reaper_ipc::testing::scene_snapshot()).expect("scene parses")
    }

    #[test]
    fn the_scene_snapshot_becomes_a_note_set() {
        let s = scene();
        let notes = note_set_of(&s).expect("conversion");
        assert_eq!(notes.notes.len(), s.notes.len());
        assert!(notes.notes.iter().all(|n| n.duration.is_positive()));
        assert_eq!(notes.origin.take_guid, s.take_guid);
    }

    #[test]
    fn note_ids_follow_the_snapshot_index() {
        let s = scene();
        let notes = note_set_of(&s).unwrap();
        let mut ids: Vec<NoteId> = notes.notes.iter().map(|n| n.id).collect();
        ids.sort_unstable();
        let expected: Vec<NoteId> = (0..s.notes.len() as NoteId).collect();
        assert_eq!(ids, expected);
    }

    #[test]
    fn positions_stay_in_the_project_domain() {
        let s = scene();
        let notes = note_set_of(&s).unwrap();
        let first = &notes.notes[0];
        assert!((first.onset.as_f64() - s.notes[0].start_qn).abs() < 1e-6);
    }

    #[test]
    fn a_zero_length_note_is_widened_not_dropped() {
        let mut s = scene();
        s.notes[0].end_qn = s.notes[0].start_qn;
        let notes = note_set_of(&s).unwrap();
        assert_eq!(notes.notes.len(), s.notes.len());
        assert!(notes.notes.iter().all(|n| n.duration.is_positive()));
    }

    #[test]
    fn an_out_of_range_pitch_is_rejected() {
        let mut s = scene();
        s.notes[0].pitch = 200;
        assert!(note_set_of(&s).is_err());
    }

    #[test]
    fn the_time_map_reflects_the_markers() {
        let s = scene();
        let tm = time_map_of(&s);
        assert!((tm.tempo_at(BeatTime::ZERO) - 120.0).abs() < 1e-9);
        assert_eq!(tm.meter_at(BeatTime::ZERO), TimeSignature::new(4, 4));
    }

    #[test]
    fn an_empty_marker_list_falls_back_to_a_constant_map() {
        let mut s = scene();
        s.tempo_markers.clear();
        let tm = time_map_of(&s);
        assert!((tm.tempo_at(BeatTime::ZERO) - 120.0).abs() < 1e-9);
    }

    #[test]
    fn item_bounds_are_ordered() {
        let s = scene();
        let (a, b) = item_bounds(&s);
        assert!(b > a);
    }

    #[test]
    fn conversion_is_deterministic() {
        let s = scene();
        let a = note_set_of(&s).unwrap();
        let b = note_set_of(&s).unwrap();
        assert_eq!(a.hash_hex(), b.hash_hex());
    }

    #[test]
    fn a_note_that_would_collapse_keeps_one_grid_step() {
        let mut s = scene();
        s.notes[0].start_qn = 1.0;
        s.notes[0].end_qn = 1.0;
        let notes = note_set_of(&s).unwrap();
        let widened = notes
            .notes
            .iter()
            .find(|n| n.onset == BeatTime::new(1, 1))
            .expect("the widened note");
        assert_eq!(widened.duration, BeatTime::new(1, QN_GRID));
        assert_ne!(Json::Null, Json::Bool(true));
    }
}
