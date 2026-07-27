//! Generated file - do not edit by hand.
//!
//! Regenerate with `cargo run -p xtask -- regen-embedded`. The list is checked
//! against `knowledge/` on disk by `tests/embedded_parity.rs`, so it can never
//! silently drift from the tree it was generated from.

/// Every file under `knowledge/`, as `(relative path, contents)` pairs.
///
/// Paths use forward slashes and are sorted byte-wise ascending. The layout is
/// fixed by the generator, so `rustfmt` is asked to leave it alone.
#[rustfmt::skip]
pub static EMBEDDED_FILES: &[(&str, &str)] = &[
    ("ATTRIBUTION.md", include_str!("../../../knowledge/ATTRIBUTION.md")),
    ("LICENSE.md", include_str!("../../../knowledge/LICENSE.md")),
    ("arrangement_patterns.json", include_str!("../../../knowledge/arrangement_patterns.json")),
    ("cadences.json", include_str!("../../../knowledge/cadences.json")),
    ("chord_qualities.json", include_str!("../../../knowledge/chord_qualities.json")),
    ("chord_symbols.json", include_str!("../../../knowledge/chord_symbols.json")),
    ("functions.json", include_str!("../../../knowledge/functions.json")),
    ("instrument_profiles.json", include_str!("../../../knowledge/instrument_profiles.json")),
    ("intervals.json", include_str!("../../../knowledge/intervals.json")),
    ("manifest.json", include_str!("../../../knowledge/manifest.json")),
    ("modes.json", include_str!("../../../knowledge/modes.json")),
    ("profiles/blues.json", include_str!("../../../knowledge/profiles/blues.json")),
    ("profiles/cinematic.json", include_str!("../../../knowledge/profiles/cinematic.json")),
    ("profiles/common_practice.json", include_str!("../../../knowledge/profiles/common_practice.json")),
    ("profiles/drum_and_bass.json", include_str!("../../../knowledge/profiles/drum_and_bass.json")),
    ("profiles/electronic_loop.json", include_str!("../../../knowledge/profiles/electronic_loop.json")),
    ("profiles/jazz_standard.json", include_str!("../../../knowledge/profiles/jazz_standard.json")),
    ("profiles/modal_ambient.json", include_str!("../../../knowledge/profiles/modal_ambient.json")),
    ("profiles/neo_soul_rnb.json", include_str!("../../../knowledge/profiles/neo_soul_rnb.json")),
    ("profiles/pop_rock.json", include_str!("../../../knowledge/profiles/pop_rock.json")),
    ("profiles/strict_counterpoint.json", include_str!("../../../knowledge/profiles/strict_counterpoint.json")),
    ("progressions.json", include_str!("../../../knowledge/progressions.json")),
    ("rules/arrangement.json", include_str!("../../../knowledge/rules/arrangement.json")),
    ("rules/counterpoint.json", include_str!("../../../knowledge/rules/counterpoint.json")),
    ("rules/extensions.json", include_str!("../../../knowledge/rules/extensions.json")),
    ("rules/harmony.json", include_str!("../../../knowledge/rules/harmony.json")),
    ("rules/looping.json", include_str!("../../../knowledge/rules/looping.json")),
    ("rules/melody.json", include_str!("../../../knowledge/rules/melody.json")),
    ("rules/voice_leading.json", include_str!("../../../knowledge/rules/voice_leading.json")),
    ("scales.json", include_str!("../../../knowledge/scales.json")),
    ("sources.json", include_str!("../../../knowledge/sources.json")),
    ("voicings.json", include_str!("../../../knowledge/voicings.json")),
];
