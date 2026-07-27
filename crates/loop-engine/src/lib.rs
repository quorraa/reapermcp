//! `loop-engine` — pipeline stage 10 and brief §10.9: the loop audit.
//!
//! A loop is not simply a region with a start and an end. It is a region whose
//! **end is heard immediately before its own beginning**, over and over, and
//! everything this crate does follows from that one fact: the wrap is a real
//! musical connection and is audited as one.
//!
//! # What it audits
//!
//! Everything brief §10.9 lists: the final harmony against the initial harmony,
//! bass continuity, voice leading across the wrap, unresolved tendencies,
//! hanging notes, notes crossing the boundary, pickup placement, tail
//! requirements, percussion phase metadata where available, harmonic rhythm at
//! the wrap, and layer removal that eliminates an essential chord tone.
//!
//! # The tonic is never forced
//!
//! There is no universal "loop quality" number here. Each of the six
//! [`LoopIntent`](music_domain::structure::LoopIntent) values has its own
//! criteria, and they genuinely differ:
//!
//! | intent | what makes it good |
//! |---|---|
//! | `closed_tonic` | a dominant-to-tonic wrap — a turnaround |
//! | `open_dominant` | ending unresolved, so the standing tension is *credited* |
//! | `modal_drone` | a stable collection, a pedal or common tone, a smooth seam — **no dominant required** |
//! | `seamless_color` | shared pitch classes and stepwise motion, so the loop point is inaudible |
//! | `transition_ready` | a final chord that can move on rather than close |
//! | `one_shot_ending` | closing conclusively; wrap continuity is not scored at all |
//!
//! `modal_drone` does not merely weight the functional wrap lower — it does not
//! use it as a criterion at all, and subtracts for it, because a strong cadence
//! turns a drone into a closed tonal loop.
//!
//! # Exactness
//!
//! Loop length is compared with [`BeatTime`](music_domain::time::BeatTime)
//! equality, never with a float tolerance. A drift of one tick compounds on
//! every repeat, which is why musical time in this workspace is a normalised
//! rational.
//!
//! # Suggestions, not mutations
//!
//! [`audit`] never changes a note. [`suggest_repairs`] proposes changes, each
//! with a rationale and the `knowledge/` rule ids that justify it.
//! [`apply_boundary_policy`] is the only mutating function in the crate, and
//! its [`CarryPolicy`] is the caller's choice.
//!
//! # Determinism
//!
//! Same input, same knowledge version, same profile => byte-identical
//! [`LoopReport`](music_domain::candidate::LoopReport) JSON. No `HashMap`
//! iteration reaches output, note-id lists are sorted, and every string is
//! built from the measurements rather than from iteration order.
//!
//! # Example
//!
//! ```
//! use loop_engine::testing;
//! use music_domain::prelude::LoopIntent;
//!
//! let closed = testing::dominant_wrap(LoopIntent::ClosedTonic, "common_practice");
//! let modal = testing::modal_planing_loop(LoopIntent::ModalDrone, "modal_ambient");
//!
//! // A V-to-I wrap is excellent for a closed tonal loop …
//! assert!(closed.report().compatible);
//! // … and a modal loop with no dominant anywhere is excellent too.
//! assert!(modal.report().compatible);
//! ```

#![warn(missing_docs)]

pub mod audit;
pub mod boundary;
pub mod error;
pub mod intent;
pub mod repair;
pub mod testing;
pub mod wrap;

pub use audit::{audit, audit_detailed, LoopAudit, LoopInput, COMPATIBLE_THRESHOLD};
pub use boundary::{
    apply_boundary_policy, carried_notes, crossing_notes, entry_points, exit_points, hanging_notes,
    is_marked_carry, occupied_length, pickup_length, tail_length, CarryPolicy, LoopSpan,
    CARRY_MARK, CARRY_MARKS,
};
pub use error::LoopError;
pub use intent::{IntentFit, SeamIntegrity};
pub use repair::{suggest_repairs, LoopRepair};
pub use wrap::{KeyFrame, LayerRemoval, PercussionPhase, WrapObservation};

/// The types most callers want in scope.
pub mod prelude {
    pub use crate::audit::{audit, audit_detailed, LoopAudit, LoopInput};
    pub use crate::boundary::{
        apply_boundary_policy, crossing_notes, hanging_notes, pickup_length, tail_length,
        CarryPolicy, LoopSpan,
    };
    pub use crate::error::LoopError;
    pub use crate::intent::{IntentFit, SeamIntegrity};
    pub use crate::repair::{suggest_repairs, LoopRepair};
    pub use crate::wrap::{KeyFrame, WrapObservation};
    pub use music_domain::candidate::LoopReport;
}

#[cfg(test)]
mod contract_surface {
    //! Compile-time assertions that the frozen contract exists with the
    //! signatures the MCP server is written against.

    use crate::prelude::*;
    use music_domain::prelude::*;
    use theory_kb::{KnowledgeBase, ResolvedProfile};

    #[test]
    fn frozen_signatures() {
        let _: for<'a> fn(&KnowledgeBase, &ResolvedProfile, &LoopInput<'a>) -> LoopReport = audit;
        let _: for<'a> fn(
            &KnowledgeBase,
            &ResolvedProfile,
            &LoopInput<'a>,
            &LoopReport,
        ) -> Vec<LoopRepair> = suggest_repairs;
        let _: fn(&mut Vec<Note>, &LoopSpan, CarryPolicy) = apply_boundary_policy;
        let _: fn(&NoteSet, &LoopSpan) -> Vec<NoteId> = hanging_notes;
        let _: fn(&NoteSet, &LoopSpan) -> Vec<NoteId> = crossing_notes;
        let _: fn(&NoteSet, &LoopSpan) -> BeatTime = pickup_length;
        let _: for<'a> fn(&'a LoopSpan) -> BeatTime = LoopSpan::length;
    }

    #[test]
    fn the_cancel_flag_is_the_harmony_engine_one() {
        // The contract says to reuse it rather than define another.
        let flag = harmony_engine::params::CancelFlag::new();
        assert!(!flag.is_cancelled());
        flag.cancel();
        assert!(flag.is_cancelled());
    }
}
