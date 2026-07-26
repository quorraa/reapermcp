//! Shared fixture plumbing for the integration tests.

// This module is compiled once per integration-test binary, and neither binary
// uses all of it; the alternative is a duplicate copy per test file.
#![allow(dead_code)]

use music_domain::prelude::*;
use std::path::{Path, PathBuf};

/// The workspace root, found from this crate's manifest directory.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root must exist")
}

/// Every committed fixture, in a stable order.
pub fn fixtures() -> Vec<Fixture> {
    let mut out = Vec::new();
    for dir in ["melodies", "loops", "progressions"] {
        let d = repo_root().join("fixtures").join(dir);
        let mut files: Vec<PathBuf> = std::fs::read_dir(&d)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", d.display()))
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();
        for f in files {
            out.push(Fixture::from_path(&f).unwrap_or_else(|e| panic!("{}: {e}", f.display())));
        }
    }
    out
}

/// One fixture by id, e.g. `"melodies/dorian_vamp_d"`.
pub fn fixture(id: &str) -> Fixture {
    fixtures()
        .into_iter()
        .find(|f| f.id == id)
        .unwrap_or_else(|| panic!("no fixture {id}"))
}

/// The note set an analysis runs over.
///
/// A progression-only fixture carries no notes, so its chords are realised as
/// close-position block voicings from C3 upwards. That is test material, not a
/// musical claim — it exists so chord detection and the polyphonic evidence
/// sources are exercised over the committed progressions instead of being
/// skipped.
pub fn analysis_notes(f: &Fixture) -> NoteSet {
    let ns = f.note_set();
    if !ns.notes.is_empty() {
        return ns;
    }
    let mut notes = Vec::new();
    let mut id: NoteId = 0;
    for ev in f.chord_events() {
        let mut previous = 47; // one semitone below C3
        for pc in ev.spec.pitch_classes() {
            let mut midi = 48 + pc.rem_euclid(12);
            while midi <= previous {
                midi += 12;
            }
            previous = midi;
            let mut n = Note::new(
                id,
                SpelledPitch::from_midi(midi, None),
                ev.onset,
                ev.duration,
            );
            n.selected = true;
            notes.push(n);
            id += 1;
        }
    }
    NoteSet::sorted(notes, f.time_map())
}

/// True when the fixture's own notes were used, as opposed to realised chords.
pub fn has_own_notes(f: &Fixture) -> bool {
    !f.notes.is_empty()
}

/// The loop span a fixture declares.
pub fn loop_span(f: &Fixture) -> Option<(BeatTime, BeatTime)> {
    f.loop_region.as_ref().map(|l| (l.start_qn, l.end_qn))
}
