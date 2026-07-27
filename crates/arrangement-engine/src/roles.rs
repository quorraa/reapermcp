//! Roles: which pattern realises them, on which instrument, in which register.
//!
//! Nothing here invents a range, a register or a rhythm. A role is matched to
//! an [`ArrangementPattern`] out of `knowledge/arrangement_patterns.json`, that
//! pattern is matched to an [`InstrumentProfile`] out of
//! `knowledge/instrument_profiles.json`, and the register window is the
//! intersection of the two records' ranges. The only judgement this module
//! contributes is *which* record to prefer when several fit, and — for the four
//! roles the catalogue does not name directly — which role to substitute.

use music_domain::prelude::*;
use theory_kb::{ArrangementPattern, InstrumentProfile, KnowledgeBase, ResolvedProfile};

/// How prominent a part is meant to be. Lower sorts earlier and wins contested
/// register.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// The listener is meant to follow this part.
    Foreground,
    /// Audible and characterful, but not the subject.
    Midground,
    /// Supports without competing.
    Background,
}

impl Priority {
    /// Reads the `priority` field of an arrangement pattern.
    ///
    /// An unrecognised value is treated as midground rather than rejected: the
    /// catalogue is editable data and a new word there should degrade, not
    /// crash.
    pub fn parse(s: &str) -> Priority {
        match s {
            "foreground" => Priority::Foreground,
            "background" => Priority::Background,
            _ => Priority::Midground,
        }
    }

    /// Stable identifier.
    pub fn id(self) -> &'static str {
        match self {
            Priority::Foreground => "foreground",
            Priority::Midground => "midground",
            Priority::Background => "background",
        }
    }

    /// The rank the frozen [`crate::RoleAssignment::priority`] field carries.
    pub fn rank(self) -> u8 {
        match self {
            Priority::Foreground => 0,
            Priority::Midground => 1,
            Priority::Background => 2,
        }
    }
}

/// Roles the pattern catalogue does not name, and what stands in for them.
///
/// Role substitution is one of the contrast levers the brief lists, and it is
/// also the honest answer to a role with no pattern: rather than fabricate a
/// rhythm, the engine borrows a catalogued pattern from a musically adjacent
/// role and says so in the assignment's rationale. The first substitute that
/// resolves wins, so the order is the preference order.
pub const ROLE_SUBSTITUTES: &[(&str, &[&str])] = &[
    ("pulse", &["ostinato", "bass", "comping"]),
    ("percussion", &["pulse", "ostinato", "riff"]),
    ("ornament", &["counterlead", "riff", "lead"]),
    ("ear_candy", &["texture", "ornament", "counterlead"]),
];

/// The substitute chain for a role, most preferred first. Empty when the
/// catalogue names the role itself.
pub fn substitutes(role: ArrangementRole) -> &'static [&'static str] {
    ROLE_SUBSTITUTES
        .iter()
        .find(|(id, _)| *id == role.id())
        .map(|(_, subs)| *subs)
        .unwrap_or(&[])
}

/// Every pattern whose `role` field matches, in catalogue order.
pub fn patterns_for_role(
    kb: &KnowledgeBase,
    role: ArrangementRole,
) -> Vec<&ArrangementPattern> {
    kb.arrangement_patterns()
        .iter()
        .filter(|p| p.role == role.id())
        .collect()
}

/// The pool a role draws from: its own patterns, then its substitutes'.
///
/// The returned vector is in a deterministic order — catalogue order within
/// each tier, tiers in preference order — so selection never depends on hash
/// iteration.
pub fn pattern_pool(kb: &KnowledgeBase, role: ArrangementRole) -> Vec<&ArrangementPattern> {
    let mut pool = patterns_for_role(kb, role);
    if pool.is_empty() {
        for sub in substitutes(role) {
            for p in kb.arrangement_patterns().iter().filter(|p| p.role == *sub) {
                pool.push(p);
            }
            if !pool.is_empty() {
                break;
            }
        }
    }
    pool
}

/// How well a pattern suits a request, higher is better.
///
/// Five terms, all reading data: whether the profile chain names the pattern,
/// how close the pattern's own density sits to the requested density, how close
/// its energy contribution sits to the section's energy, whether the section
/// role is one the pattern participates in, and how much its grid collides with
/// the onsets already spoken for — `contested` holds positions within a bar.
pub fn pattern_fit(
    pattern: &ArrangementPattern,
    profile: &ResolvedProfile,
    density_target: f64,
    energy_target: f64,
    section_role: Option<&str>,
    contested: &[BeatTime],
) -> f64 {
    let style = if pattern
        .style_profiles
        .iter()
        .any(|s| profile.chain.iter().any(|c| c == s))
    {
        1.0
    } else {
        0.0
    };
    let participates = match section_role {
        Some(r) if !pattern.section_participation.is_empty() => {
            if pattern.section_participation.iter().any(|s| s == r) {
                1.0
            } else {
                0.0
            }
        }
        _ => 0.5,
    };
    let density_fit = 1.0 - (pattern.density - density_target).abs();
    let energy_fit = 1.0 - (pattern.energy_contribution - energy_target).abs();
    // Contrary rhythmic activity, chosen rather than repaired: a pattern whose
    // grid lands where the lead already plays is worth less than one that fits
    // between the lead's attacks.
    let conflict = crate::patterns::onset_conflict(pattern, contested);
    2.0 * style + participates + density_fit + 0.75 * energy_fit - 1.5 * conflict
}

/// Chooses the pattern for a role.
///
/// An explicit `texture_pattern` wins whenever it exists and its role matches
/// the requested role — that is the caller overriding the engine, which the
/// brief requires the texture lever to be able to do. Otherwise the best
/// [`pattern_fit`] wins, with catalogue order as the tie-break so the choice is
/// deterministic and the catalogue's own ordering expresses preference.
#[allow(clippy::too_many_arguments)] // Every argument is one axis of the selection the brief specifies.
pub fn select_pattern<'k>(
    kb: &'k KnowledgeBase,
    role: ArrangementRole,
    profile: &ResolvedProfile,
    forced: Option<&str>,
    density_target: f64,
    energy_target: f64,
    section_role: Option<&str>,
    contested: &[BeatTime],
) -> Option<&'k ArrangementPattern> {
    if let Some(id) = forced {
        if let Some(p) = kb.arrangement_patterns().iter().find(|p| p.id == id) {
            if p.role == role.id() {
                return Some(p);
            }
        }
    }
    let pool = pattern_pool(kb, role);
    let mut best: Option<(&ArrangementPattern, f64)> = None;
    for p in pool {
        let fit = pattern_fit(
            p,
            profile,
            density_target,
            energy_target,
            section_role,
            contested,
        );
        match best {
            Some((_, score)) if score >= fit => {}
            _ => best = Some((p, fit)),
        }
    }
    best.map(|(p, _)| p)
}

/// How well an instrument profile suits a pattern, higher is better.
pub fn instrument_fit(
    inst: &InstrumentProfile,
    pattern: &ArrangementPattern,
    role: ArrangementRole,
) -> f64 {
    let affinity = if inst.typical_roles.iter().any(|r| r == role.id()) {
        2.0
    } else if inst.typical_roles.contains(&pattern.role) {
        1.5
    } else {
        0.0
    };
    let polyphony = if inst.polyphony >= pattern.polyphony {
        1.0
    } else {
        0.0
    };
    let overlap = overlap_semitones(
        (inst.range.low_midi, inst.range.high_midi),
        (pattern.range.low_midi, pattern.range.high_midi),
    );
    let wanted = (pattern.range.high_midi - pattern.range.low_midi).max(1);
    affinity + polyphony + (overlap as f64 / wanted as f64)
}

/// Chooses the instrument profile a role is written for.
///
/// Ties break on catalogue order, so the same request always names the same
/// instrument and the more specific profile — which is listed first — wins over
/// the general-purpose one.
pub fn select_instrument<'k>(
    kb: &'k KnowledgeBase,
    pattern: &ArrangementPattern,
    role: ArrangementRole,
) -> Option<&'k InstrumentProfile> {
    let mut best: Option<(&InstrumentProfile, f64)> = None;
    for inst in kb.instrument_profiles() {
        let fit = instrument_fit(inst, pattern, role);
        match best {
            Some((_, score)) if score >= fit => {}
            _ => best = Some((inst, fit)),
        }
    }
    best.map(|(i, _)| i)
}

/// Semitones two inclusive MIDI windows share.
pub fn overlap_semitones(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.1.min(b.1) - a.0.max(b.0) + 1).max(0)
}

/// The register window a part is written into: the pattern's range clipped to
/// what the instrument can actually play.
///
/// When the two do not intersect the instrument wins, because the instrument is
/// a physical fact and the pattern's range is a stylistic preference.
pub fn base_window(pattern: &ArrangementPattern, inst: &InstrumentProfile) -> (i32, i32) {
    let low = pattern.range.low_midi.max(inst.range.low_midi);
    let high = pattern.range.high_midi.min(inst.range.high_midi);
    if low >= high {
        (inst.range.low_midi, inst.range.high_midi)
    } else {
        (low, high)
    }
}

/// The roles to write when the caller did not name any.
///
/// Read from the profile's `default_arrangement_roles` when it declares one,
/// and otherwise the four roles that make a complete arrangement of a chord
/// progression: the tune, the bottom, the harmony and a rhythmic layer.
pub fn default_roles(profile: &ResolvedProfile) -> Vec<ArrangementRole> {
    if let Some(list) = profile
        .field("default_arrangement_roles")
        .and_then(qjson::Json::as_arr)
    {
        let roles: Vec<ArrangementRole> = list
            .iter()
            .filter_map(qjson::Json::as_str)
            .filter_map(ArrangementRole::parse)
            .collect();
        if !roles.is_empty() {
            return roles;
        }
    }
    vec![
        ArrangementRole::Lead,
        ArrangementRole::Bass,
        ArrangementRole::HarmonicBed,
        ArrangementRole::Comping,
    ]
}

/// Orders roles for allocation: foreground first, then catalogue order.
///
/// Foreground parts choose their register before anything else, which is how
/// `arrangement.foreground_role_wins_contested_register` is actually honoured
/// rather than merely scored.
pub fn allocation_order(roles: &[(ArrangementRole, Priority)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..roles.len()).collect();
    order.sort_by(|a, b| {
        roles[*a]
            .1
            .cmp(&roles[*b].1)
            .then_with(|| role_index(roles[*a].0).cmp(&role_index(roles[*b].0)))
    });
    order
}

/// The role's position in [`ArrangementRole::all`], used as a stable tie-break.
pub fn role_index(role: ArrangementRole) -> usize {
    ArrangementRole::all()
        .iter()
        .position(|r| *r == role)
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use theory_kb::KnowledgeBase;

    fn kb() -> &'static KnowledgeBase {
        KnowledgeBase::embedded()
    }

    #[test]
    fn every_role_resolves_to_a_pattern() {
        for role in ArrangementRole::all() {
            assert!(
                !pattern_pool(kb(), *role).is_empty(),
                "no pattern pool for {}",
                role.id()
            );
        }
    }

    #[test]
    fn only_four_roles_need_substitution() {
        let needing: Vec<&str> = ArrangementRole::all()
            .iter()
            .filter(|r| patterns_for_role(kb(), **r).is_empty())
            .map(|r| r.id())
            .collect();
        assert_eq!(
            needing,
            vec!["pulse", "percussion", "ornament", "ear_candy"]
        );
    }

    #[test]
    fn every_role_resolves_to_an_instrument() {
        let profile = kb().resolve_profile("pop_rock").expect("profile");
        for role in ArrangementRole::all() {
            let pattern = select_pattern(kb(), *role, &profile, None, 0.5, 0.5, None, &[])
                .unwrap_or_else(|| panic!("pattern for {}", role.id()));
            let inst = select_instrument(kb(), pattern, *role)
                .unwrap_or_else(|| panic!("instrument for {}", role.id()));
            let (lo, hi) = base_window(pattern, inst);
            assert!(lo < hi, "{} has an empty window", role.id());
            assert!(inst.range.contains(lo) && inst.range.contains(hi));
        }
    }

    #[test]
    fn selection_is_deterministic() {
        let profile = kb().resolve_profile("jazz_standard").expect("profile");
        for role in ArrangementRole::all() {
            let a = select_pattern(kb(), *role, &profile, None, 0.4, 0.6, Some("verse"), &[]);
            let b = select_pattern(kb(), *role, &profile, None, 0.4, 0.6, Some("verse"), &[]);
            assert_eq!(a.map(|p| &p.id), b.map(|p| &p.id));
        }
    }

    #[test]
    fn a_forced_texture_wins_when_the_role_matches() {
        let profile = kb().resolve_profile("pop_rock").expect("profile");
        let chosen = select_pattern(
            kb(),
            ArrangementRole::HarmonicBed,
            &profile,
            Some("arr_chorale_voicing"),
            0.9,
            0.9,
            None,
            &[],
        )
        .expect("a pattern");
        assert_eq!(chosen.id, "arr_chorale_voicing");
    }

    #[test]
    fn a_forced_texture_for_the_wrong_role_is_ignored() {
        let profile = kb().resolve_profile("pop_rock").expect("profile");
        let chosen = select_pattern(
            kb(),
            ArrangementRole::Bass,
            &profile,
            Some("arr_sustained_pad"),
            0.5,
            0.5,
            None,
            &[],
        )
        .expect("a pattern");
        assert_eq!(chosen.role, "bass");
    }

    #[test]
    fn priority_parses_and_ranks() {
        assert_eq!(Priority::parse("foreground"), Priority::Foreground);
        assert_eq!(Priority::parse("background"), Priority::Background);
        assert_eq!(Priority::parse("anything else"), Priority::Midground);
        assert!(Priority::Foreground < Priority::Background);
        assert_eq!(Priority::Foreground.rank(), 0);
        assert_eq!(Priority::Foreground.id(), "foreground");
    }

    #[test]
    fn overlap_is_inclusive_and_never_negative() {
        assert_eq!(overlap_semitones((60, 72), (60, 72)), 13);
        assert_eq!(overlap_semitones((60, 72), (73, 80)), 0);
        assert_eq!(overlap_semitones((60, 72), (72, 80)), 1);
    }

    #[test]
    fn foreground_allocates_first() {
        let roles = vec![
            (ArrangementRole::Pad, Priority::Background),
            (ArrangementRole::Lead, Priority::Foreground),
            (ArrangementRole::Comping, Priority::Midground),
        ];
        assert_eq!(allocation_order(&roles), vec![1, 2, 0]);
    }

    #[test]
    fn default_roles_are_a_complete_arrangement() {
        let profile = kb().resolve_profile("pop_rock").expect("profile");
        let roles = default_roles(&profile);
        assert!(roles.contains(&ArrangementRole::Lead));
        assert!(roles.contains(&ArrangementRole::Bass));
    }
}
