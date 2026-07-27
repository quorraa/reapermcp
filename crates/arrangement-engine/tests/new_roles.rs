//! The four roles that used to have no catalogued pattern.
//!
//! Each one is checked for the property that makes it that role rather than a
//! neighbour: a pulse marks time without colouring the harmony, percussion
//! carries no harmony at all, an ornament stays out of the lead's way, and
//! ear candy stays rare.

use arrangement_engine::{roles, ArrangementParams};
use music_domain::structure::ArrangementRole;
use theory_kb::KnowledgeBase;

fn kb() -> &'static KnowledgeBase {
    KnowledgeBase::embedded()
}

fn plan_for(role: ArrangementRole, profile: &str) -> arrangement_engine::ArrangementPlan {
    arrangement_engine::testing::harness("melodies/eight_bar_c_major", profile)
        .arrange(&ArrangementParams::default().with_roles(&[role]))
        .expect("a plan")
}

#[test]
fn all_four_roles_now_use_their_own_pattern() {
    for role in [
        ArrangementRole::Pulse,
        ArrangementRole::Percussion,
        ArrangementRole::Ornament,
        ArrangementRole::EarCandy,
    ] {
        let own = roles::patterns_for_role(kb(), role);
        assert!(
            !own.is_empty(),
            "{} still has no catalogued pattern",
            role.id()
        );
        let plan = plan_for(role, "electronic_loop");
        let a = plan.assignment(role).expect("an assignment");
        assert!(
            own.iter().any(|p| p.id == a.pattern_id),
            "{} chose {}, which is not one of its own patterns",
            role.id(),
            a.pattern_id
        );
        assert!(
            !a.rationale.contains("substituted"),
            "{} reported a substitution it did not make",
            role.id()
        );
        assert!(!plan
            .warnings
            .iter()
            .any(|w| w.code == "ROLE_SUBSTITUTED" && w.message.contains(role.id())));
    }
}

#[test]
fn a_pulse_states_only_the_root() {
    let pattern = roles::patterns_for_role(kb(), ArrangementRole::Pulse)
        .into_iter()
        .next()
        .expect("a pulse pattern");
    assert_eq!(
        pattern.harmonic_responsibility, "root_only",
        "a pulse that voices the chord competes with the harmonic bed"
    );
    assert_eq!(pattern.polyphony, 1, "a pulse is a single line");

    let plan = plan_for(ArrangementRole::Pulse, "pop_rock");
    let part = plan
        .parts
        .iter()
        .find(|p| p.role == ArrangementRole::Pulse)
        .expect("a pulse part");
    assert!(!part.notes.is_empty());
    assert!(!part.polyphonic);
    // One sounding pitch at a time: no note starts before the previous ends.
    let mut sorted = part.notes.clone();
    sorted.sort_by_key(|a| a.onset);
    for w in sorted.windows(2) {
        assert!(
            w[0].end() <= w[1].onset,
            "pulse notes overlap at {}",
            w[1].onset
        );
    }
}

#[test]
fn percussion_carries_no_harmony() {
    let pattern = roles::patterns_for_role(kb(), ArrangementRole::Percussion)
        .into_iter()
        .next()
        .expect("a percussion pattern");
    assert_eq!(
        pattern.harmonic_responsibility, "none",
        "a percussion part must not be given harmonic duty"
    );
    let plan = plan_for(ArrangementRole::Percussion, "drum_and_bass");
    let part = plan
        .parts
        .iter()
        .find(|p| p.role == ArrangementRole::Percussion)
        .expect("a percussion part");
    assert!(!part.notes.is_empty());
    // Narrow band: this is articulation selection, not melody.
    let lo = part.notes.iter().map(|n| n.midi).min().unwrap();
    let hi = part.notes.iter().map(|n| n.midi).max().unwrap();
    assert!(
        hi - lo <= 24,
        "percussion spread {lo}..{hi} is too wide to read as a rhythm part"
    );
}

#[test]
fn an_ornament_is_sparser_than_the_lead() {
    let plan = arrangement_engine::testing::harness("melodies/eight_bar_c_major", "jazz_standard")
        .arrange(
            &ArrangementParams::default()
                .with_roles(&[ArrangementRole::Lead, ArrangementRole::Ornament]),
        )
        .expect("a plan");
    let lead = plan
        .parts
        .iter()
        .find(|p| p.role == ArrangementRole::Lead)
        .expect("a lead");
    let orn = plan
        .parts
        .iter()
        .find(|p| p.role == ArrangementRole::Ornament)
        .expect("an ornament");
    assert!(
        orn.notes.len() < lead.notes.len(),
        "ornament wrote {} notes against a lead of {} — that is a countermelody",
        orn.notes.len(),
        lead.notes.len()
    );
}

#[test]
fn ear_candy_stays_rare() {
    let plan = plan_for(ArrangementRole::EarCandy, "electronic_loop");
    let part = plan
        .parts
        .iter()
        .find(|p| p.role == ArrangementRole::EarCandy)
        .expect("an ear candy part");
    let span = (plan.span.1 - plan.span.0).as_f64();
    let per_qn = part.notes.len() as f64 / span.max(1.0);
    assert!(
        per_qn <= 0.5,
        "ear candy wrote {} notes over {span} QN ({per_qn:.2}/qn) — that is an ostinato",
        part.notes.len()
    );
    assert!(!part.notes.is_empty(), "ear candy wrote nothing at all");
}
