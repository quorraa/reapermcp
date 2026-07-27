//! The seam geometry and the four carry policies.
//!
//! `apply_boundary_policy` is the only mutating function in the crate. Every
//! policy is exercised here against the same material, and every one of them
//! must leave the loop span bit-for-bit identical.

use loop_engine::prelude::*;
use loop_engine::testing;
use loop_engine::{carried_notes, entry_points, exit_points, is_marked_carry, occupied_length};
use music_domain::prelude::*;

fn span() -> LoopSpan {
    LoopSpan::new(
        BeatTime::ZERO,
        BeatTime::from_quarters(16),
        LoopIntent::SeamlessColor,
    )
}

/// A pickup, a plain interior note and an overhanging final note.
fn material() -> Vec<Note> {
    vec![
        testing::note(
            0,
            55,
            BeatTime::from_quarters(-1),
            BeatTime::from_quarters(1),
        ),
        testing::note(1, 60, BeatTime::ZERO, BeatTime::from_quarters(4)),
        testing::note(
            2,
            64,
            BeatTime::from_quarters(14),
            BeatTime::from_quarters(3),
        ),
    ]
}

fn set(notes: Vec<Note>) -> NoteSet {
    NoteSet::sorted(notes, testing::plain_time_map())
}

// ---------------------------------------------------------------------------
// span arithmetic
// ---------------------------------------------------------------------------

#[test]
fn the_span_length_is_the_difference_of_its_boundaries() {
    assert_eq!(span().length(), BeatTime::from_quarters(16));
    assert_eq!(
        LoopSpan::new(
            BeatTime::new(7, 3),
            BeatTime::new(31, 3),
            LoopIntent::ModalDrone
        )
        .length(),
        BeatTime::from_quarters(8)
    );
}

#[test]
fn a_span_reports_whether_it_contains_a_position() {
    let s = span();
    assert!(s.contains(BeatTime::ZERO));
    assert!(s.contains(BeatTime::from_quarters(15)));
    assert!(!s.contains(BeatTime::from_quarters(16)));
    assert!(!s.contains(BeatTime::from_quarters(-1)));
}

#[test]
fn an_empty_span_is_invalid() {
    let s = LoopSpan::new(BeatTime::ZERO, BeatTime::ZERO, LoopIntent::ClosedTonic);
    assert!(!s.is_valid());
    assert!(s.validate().is_err());
}

#[test]
fn the_span_json_names_its_intent() {
    let j = span().to_json();
    assert_eq!(
        j.get("intent").and_then(qjson::Json::as_str),
        Some("seamless_color")
    );
}

// ---------------------------------------------------------------------------
// seam classification
// ---------------------------------------------------------------------------

#[test]
fn the_three_note_sets_are_distinguished() {
    let s = set(material());
    assert_eq!(hanging_notes(&s, &span()), vec![2]);
    assert_eq!(
        crossing_notes(&s, &span()),
        vec![2],
        "the anacrusis stops exactly at the loop start, so it does not cross the seam"
    );
    assert!(carried_notes(&s, &span()).is_empty());
}

#[test]
fn a_note_sustaining_through_the_loop_start_does_cross() {
    let s = set(vec![testing::note(
        7,
        60,
        BeatTime::from_quarters(-2),
        BeatTime::from_quarters(4),
    )]);
    assert_eq!(crossing_notes(&s, &span()), vec![7]);
    assert!(hanging_notes(&s, &span()).is_empty());
}

#[test]
fn entry_and_exit_points_are_reported() {
    let s = set(material());
    assert_eq!(entry_points(&s, &span()), vec![1]);
    assert!(exit_points(&s, &span()).is_empty());
}

#[test]
fn the_occupied_length_grows_with_material_outside_the_span() {
    let s = set(material());
    assert_eq!(occupied_length(&s, &span()), BeatTime::from_quarters(18));
    let clean = set(vec![testing::note(
        0,
        60,
        BeatTime::ZERO,
        BeatTime::from_quarters(16),
    )]);
    assert_eq!(occupied_length(&clean, &span()), span().length());
}

#[test]
fn a_carried_note_is_excluded_from_the_occupied_length() {
    let mut notes = material();
    notes[2].articulation = Some(loop_engine::CARRY_MARK.to_string());
    notes.remove(0);
    let s = set(notes);
    assert_eq!(occupied_length(&s, &span()), span().length());
}

// ---------------------------------------------------------------------------
// every policy, exactly
// ---------------------------------------------------------------------------

#[test]
fn every_policy_preserves_the_loop_length_exactly() {
    for policy in CarryPolicy::all() {
        let s = span();
        let before = s.length();
        let mut notes = material();
        apply_boundary_policy(&mut notes, &s, *policy);
        assert_eq!(s.length(), before, "policy {}", policy.id());
        assert_eq!(s.start, BeatTime::ZERO);
        assert_eq!(s.end, BeatTime::from_quarters(16));
    }
}

#[test]
fn every_policy_preserves_a_non_binary_loop_length_exactly() {
    for policy in CarryPolicy::all() {
        let s = LoopSpan::new(
            BeatTime::new(1, 3),
            BeatTime::new(49, 3),
            LoopIntent::ModalDrone,
        );
        let mut notes = vec![
            testing::note_at(0, 55, (-2, 3), (1, 1)),
            testing::note_at(1, 60, (46, 3), (3, 1)),
        ];
        apply_boundary_policy(&mut notes, &s, *policy);
        assert_eq!(s.length(), BeatTime::from_quarters(16), "{}", policy.id());
    }
}

#[test]
fn every_policy_leaves_positive_durations() {
    for policy in CarryPolicy::all() {
        let mut notes = material();
        apply_boundary_policy(&mut notes, &span(), *policy);
        assert!(
            notes.iter().all(|n| n.duration.is_positive()),
            "policy {}",
            policy.id()
        );
    }
}

#[test]
fn every_policy_produces_a_sorted_list() {
    for policy in CarryPolicy::all() {
        let mut notes = material();
        apply_boundary_policy(&mut notes, &span(), *policy);
        assert!(
            notes
                .windows(2)
                .all(|w| (w[0].onset, w[0].midi, w[0].id) <= (w[1].onset, w[1].midi, w[1].id)),
            "policy {}",
            policy.id()
        );
    }
}

#[test]
fn every_policy_assigns_unique_note_ids() {
    for policy in CarryPolicy::all() {
        let mut notes = material();
        apply_boundary_policy(&mut notes, &span(), *policy);
        let mut ids: Vec<NoteId> = notes.iter().map(|n| n.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "policy {} reused a note id", policy.id());
    }
}

#[test]
fn split_leaves_nothing_outside_the_span() {
    let mut notes = material();
    apply_boundary_policy(&mut notes, &span(), CarryPolicy::Split);
    assert!(notes
        .iter()
        .all(|n| n.onset >= span().start && n.end() <= span().end));
    let s = set(notes);
    assert!(hanging_notes(&s, &span()).is_empty());
    assert_eq!(pickup_length(&s, &span()), BeatTime::ZERO);
    assert_eq!(tail_length(&s, &span()), BeatTime::ZERO);
}

#[test]
fn split_keeps_the_total_sounding_duration() {
    let mut notes = material();
    let before: BeatTime = notes.iter().fold(BeatTime::ZERO, |acc, n| acc + n.duration);
    apply_boundary_policy(&mut notes, &span(), CarryPolicy::Split);
    let after: BeatTime = notes.iter().fold(BeatTime::ZERO, |acc, n| acc + n.duration);
    assert_eq!(
        before, after,
        "splitting cuts a note, it does not shorten it"
    );
}

#[test]
fn carry_moves_nothing_and_marks_the_overhang() {
    let mut notes = material();
    apply_boundary_policy(&mut notes, &span(), CarryPolicy::Carry);
    assert_eq!(notes.len(), 3);
    let overhang = notes.iter().find(|n| n.id == 2).unwrap();
    assert_eq!(overhang.end(), BeatTime::from_quarters(17));
    assert!(is_marked_carry(overhang));
    let pickup = notes.iter().find(|n| n.id == 0).unwrap();
    assert_eq!(pickup.onset, BeatTime::from_quarters(-1));
}

#[test]
fn carry_turns_a_hanging_note_into_a_crossing_note() {
    let mut notes = material();
    apply_boundary_policy(&mut notes, &span(), CarryPolicy::Carry);
    let s = set(notes);
    assert!(hanging_notes(&s, &span()).is_empty());
    assert_eq!(carried_notes(&s, &span()), vec![2]);
}

#[test]
fn carry_does_not_re_mark_an_already_marked_note() {
    let mut notes = material();
    notes[2].articulation = Some("tie".to_string());
    apply_boundary_policy(&mut notes, &span(), CarryPolicy::Carry);
    let overhang = notes.iter().find(|n| n.id == 2).unwrap();
    assert_eq!(overhang.articulation.as_deref(), Some("tie"));
}

#[test]
fn truncate_clips_everything_into_the_span() {
    let mut notes = material();
    apply_boundary_policy(&mut notes, &span(), CarryPolicy::Truncate);
    assert!(notes
        .iter()
        .all(|n| n.onset >= span().start && n.end() <= span().end));
    assert!(
        !notes.iter().any(|n| n.id == 0),
        "the anacrusis is entirely outside the span and is dropped"
    );
}

#[test]
fn truncate_keeps_a_note_that_straddles_the_loop_start() {
    let mut notes = vec![testing::note(
        0,
        60,
        BeatTime::from_quarters(-2),
        BeatTime::from_quarters(4),
    )];
    apply_boundary_policy(&mut notes, &span(), CarryPolicy::Truncate);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].onset, BeatTime::ZERO);
    assert_eq!(notes[0].duration, BeatTime::from_quarters(2));
}

#[test]
fn rearticulate_reattacks_the_overhang_at_the_loop_start() {
    let mut notes = material();
    apply_boundary_policy(&mut notes, &span(), CarryPolicy::Rearticulate);
    let head = notes
        .iter()
        .find(|n| n.onset == BeatTime::ZERO && n.midi == 64)
        .expect("the note is re-attacked at the loop start");
    assert_eq!(
        head.duration,
        BeatTime::from_quarters(3),
        "a re-attack restores the original length, unlike a split remainder"
    );
}

#[test]
fn rearticulate_duplicates_the_anacrusis_rather_than_moving_it() {
    let mut notes = material();
    apply_boundary_policy(&mut notes, &span(), CarryPolicy::Rearticulate);
    let copies: Vec<&Note> = notes.iter().filter(|n| n.midi == 55).collect();
    assert_eq!(copies.len(), 2, "the first pass keeps its own upbeat");
    assert!(copies
        .iter()
        .any(|n| n.onset == BeatTime::from_quarters(-1)));
    assert!(copies
        .iter()
        .any(|n| n.onset == BeatTime::from_quarters(15)));
}

#[test]
fn rearticulate_caps_a_reattacked_note_at_the_loop_length() {
    let mut notes = vec![testing::note(
        0,
        60,
        BeatTime::from_quarters(12),
        BeatTime::from_quarters(40),
    )];
    apply_boundary_policy(&mut notes, &span(), CarryPolicy::Rearticulate);
    let head = notes.iter().find(|n| n.onset == BeatTime::ZERO).unwrap();
    assert_eq!(head.duration, span().length());
}

#[test]
fn a_policy_applied_to_clean_material_changes_nothing_audible() {
    let clean = vec![
        testing::note(0, 60, BeatTime::ZERO, BeatTime::from_quarters(8)),
        testing::note(
            1,
            64,
            BeatTime::from_quarters(8),
            BeatTime::from_quarters(8),
        ),
    ];
    for policy in CarryPolicy::all() {
        let mut notes = clean.clone();
        apply_boundary_policy(&mut notes, &span(), *policy);
        assert_eq!(notes.len(), 2, "policy {}", policy.id());
        assert_eq!(notes[0].duration, BeatTime::from_quarters(8));
        assert_eq!(notes[1].duration, BeatTime::from_quarters(8));
    }
}

#[test]
fn a_muted_note_is_not_treated_as_material_at_the_seam() {
    let mut n = testing::note(
        0,
        60,
        BeatTime::from_quarters(15),
        BeatTime::from_quarters(4),
    );
    n.muted = true;
    let s = set(vec![n]);
    assert!(hanging_notes(&s, &span()).is_empty());
    assert!(crossing_notes(&s, &span()).is_empty());
    assert_eq!(tail_length(&s, &span()), BeatTime::ZERO);
    assert_eq!(occupied_length(&s, &span()), span().length());
}

#[test]
fn the_audit_never_mutates_its_input() {
    let h = testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice");
    let before = h.notes.hash_hex();
    let _ = h.report();
    let _ = h.report();
    assert_eq!(h.notes.hash_hex(), before);
}

#[test]
fn applying_a_policy_after_an_audit_fixes_what_the_audit_reported() {
    let h = testing::pickup_and_hanging_note(LoopIntent::ClosedTonic, "common_practice");
    let before = h.report();
    assert!(!before.hanging_notes.is_empty());
    assert!(before.pickup_qn.is_positive());

    let mut notes = h.notes.notes.clone();
    apply_boundary_policy(&mut notes, &h.span, CarryPolicy::Split);
    let fixed = h.with_notes(notes);
    let after = fixed.report();
    assert!(after.hanging_notes.is_empty());
    assert_eq!(after.pickup_qn, BeatTime::ZERO);
    assert!(after.score > before.score);
}

#[test]
fn the_carry_policy_ids_round_trip() {
    for p in CarryPolicy::all() {
        assert_eq!(CarryPolicy::parse(p.id()), Some(*p));
    }
    assert_eq!(CarryPolicy::default(), CarryPolicy::Split);
}
