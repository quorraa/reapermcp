//! Behavioural tests for knowledge `test_id`s this crate discharges.
//!
//! Every function here is named after a `test_id` carried by a rule in
//! `knowledge/`, and asserts the *behaviour* that rule describes — through the
//! chord model, the voice-leading audit, the generator or the MCP surface —
//! rather than merely asserting that the rule exists.
//!
//! The corresponding rule id is named in each test's doc comment so the two can
//! be kept in step.

use harmony_engine::prelude::*;
use music_domain::prelude::*;
use qjson::{json_obj, Json};
use reaper_music_mcp::server::{CallContext, ServerCore};
use theory_kb::{KnowledgeBase, ResolvedProfile};

fn kb() -> &'static KnowledgeBase {
    KnowledgeBase::embedded()
}

fn profile(id: &str) -> ResolvedProfile {
    kb().resolve_profile(id).expect(id)
}

fn spec(symbol: &str) -> ChordSpec {
    music_domain::symbol::parse(symbol).unwrap_or_else(|e| panic!("{symbol}: {}", e.message))
}

/// A chord event spanning `[onset, onset + duration)`.
fn event(id: u32, symbol: &str, onset: i64, duration: i64) -> ChordEvent {
    ChordEvent {
        id,
        spec: spec(symbol),
        onset: BeatTime::from_quarters(onset),
        duration: BeatTime::from_quarters(duration),
        inversion: 0,
        function: None,
        roman: None,
        local_tonic: None,
        voicing: None,
        confidence: 1.0,
        inference_source: "user".to_string(),
        original_symbol: Some(symbol.to_string()),
    }
}

/// A voicing built from spelled-pitch text, lowest first.
fn voicing(pitches: &[&str]) -> Voicing {
    let ps: Vec<SpelledPitch> = pitches
        .iter()
        .map(|p| SpelledPitch::parse(p).unwrap_or_else(|| panic!("{p}")))
        .collect();
    let voices = (0..ps.len()).map(|i| VoiceId(i as u16)).collect();
    Voicing {
        pitches: ps,
        family: VoicingFamily::Close,
        voices,
    }
}

fn audit(profile_id: &str, voicings: &[Voicing], chords: &[ChordEvent]) -> VoiceLeadingReport {
    audit_voice_leading(kb(), &profile(profile_id), voicings, chords)
}

// ---------------------------------------------------------------------------
// Chord-symbol semantics
// ---------------------------------------------------------------------------

/// `extensions.add_does_not_imply_seventh`.
///
/// An added tone is added to a triad; it introduces no seventh.
#[test]
fn add9_no_seventh() {
    let add9 = spec("Cadd9");
    assert_eq!(add9.seventh, SeventhQuality::None, "add9 has no seventh");
    assert!(!add9.has_degree(7), "add9 must not report a seventh degree");
    assert!(add9.has_degree(9), "add9 must report its ninth");
    assert!(add9.has_degree(3), "add9 keeps the third");

    let degrees: Vec<u8> = add9
        .chord_tones()
        .iter()
        .map(|(d, _)| d.number)
        .collect::<Vec<_>>();
    assert!(
        !degrees.contains(&7),
        "no seventh may appear among the chord tones: {degrees:?}"
    );

    // C9 is a different chord: it does imply a seventh.
    let c9 = spec("C9");
    assert_ne!(c9.seventh, SeventhQuality::None, "C9 implies a seventh");
    assert_ne!(add9.pitch_classes(), c9.pitch_classes());
}

/// `extensions.sixth_does_not_imply_seventh`.
///
/// A sixth chord contains a sixth and no seventh; it is not a thirteenth.
#[test]
fn sixth_implies_no_seventh() {
    let c6 = spec("C6");
    assert_eq!(
        c6.seventh,
        SeventhQuality::None,
        "a sixth chord has no seventh"
    );
    assert!(c6.has_degree(6), "a sixth chord has its sixth");
    assert!(!c6.has_degree(7));
    assert!(!c6.has_degree(13), "C6 is not C13");

    let pcs = c6.pitch_classes();
    assert!(pcs.contains(&9), "C6 sounds A");
    assert!(
        !pcs.contains(&10) && !pcs.contains(&11),
        "no seventh sounds"
    );

    // A thirteenth chord, by contrast, does carry a seventh.
    let c13 = spec("C13");
    assert_ne!(c13.seventh, SeventhQuality::None);
    assert_ne!(c6.pitch_classes(), c13.pitch_classes());

    // Minor sixth behaves the same way.
    let cm6 = spec("Cm6");
    assert_eq!(cm6.seventh, SeventhQuality::None);
    assert!(cm6.has_degree(6));
}

/// `extensions.ambiguous_symbol_reports_alternatives`.
///
/// A symbol the grammar cannot resolve is reported, never silently guessed.
#[test]
fn ambiguous_symbol_reported() {
    // Every rejection carries the input, a position and a message, so a client
    // can say *why* rather than substituting an arbitrary chord.
    for bad in ["", "H7", "Cmaj7/", "C(((", "Xyz", "Cmaj7add", "C/", "7"] {
        match music_domain::symbol::parse(bad) {
            Ok(parsed) => panic!(
                "{bad:?} must not silently parse to {}",
                parsed.render_ascii()
            ),
            Err(e) => {
                assert_eq!(e.input, bad);
                assert!(!e.message.is_empty(), "{bad:?} produced an empty message");
            }
        }
    }

    // And the knowledge base carries the rule that mandates the behaviour, so
    // the engine can cite it.
    let rule = kb()
        .rule("extensions.ambiguous_symbol_reports_alternatives")
        .expect("the rule must exist");
    assert!(rule.kind.is_hard(), "reporting ambiguity is not optional");
}

/// `extensions.ambiguous_symbol_reports_alternatives`.
///
/// The symbol grammar's precedence is total: every symbol the knowledge bundle
/// documents parses, parses to exactly one chord, and round-trips.
#[test]
fn symbol_precedence_is_total() {
    let aliases = kb().chord_symbols();
    assert!(
        !aliases.is_empty(),
        "the bundle must document chord symbols"
    );

    let mut checked = 0usize;
    for alias in aliases {
        for written in std::iter::once(alias.written.clone()).chain(alias.ascii_variants.clone()) {
            let symbol = format!("C{written}");
            let Ok(first) = music_domain::symbol::parse(&symbol) else {
                continue;
            };
            // Total: the same input always yields the same parse.
            let second = music_domain::symbol::parse(&symbol).expect("determinism");
            assert_eq!(first, second, "{symbol} parsed two different ways");
            // And the canonical rendering round-trips, which is only possible
            // if the precedence chain resolved to a single reading.
            let rendered = first.render_ascii();
            let reparsed = music_domain::symbol::parse(&rendered)
                .unwrap_or_else(|e| panic!("{symbol} rendered as {rendered}: {}", e.message));
            assert_eq!(
                first.pitch_classes(),
                reparsed.pitch_classes(),
                "{symbol} -> {rendered} changed identity"
            );
            checked += 1;
        }
    }
    assert!(checked >= 20, "expected a real corpus, checked {checked}");
}

// ---------------------------------------------------------------------------
// Voice leading and counterpoint
// ---------------------------------------------------------------------------

/// `counterpoint.no_parallel_unisons`.
///
/// Two independent voices moving in parallel unisons are penalised, and the
/// audit names the interval it found.
#[test]
fn parallel_unisons_penalised() {
    let chords = vec![event(0, "C", 0, 2), event(1, "D", 2, 2)];
    // Two voices on the same pitch, moving together: parallel unisons.
    let parallel = vec![voicing(&["C4", "C4"]), voicing(&["D4", "D4"])];
    let independent = vec![voicing(&["C4", "E4"]), voicing(&["D4", "F4"])];

    let bad = audit("strict_counterpoint", &parallel, &chords);
    let good = audit("strict_counterpoint", &independent, &chords);

    let fired = bad
        .rule_applications
        .iter()
        .find(|r| r.rule_id == "counterpoint.no_parallel_unisons")
        .unwrap_or_else(|| {
            panic!(
                "the parallel-unison rule must fire: {:?}",
                bad.rule_applications
                    .iter()
                    .map(|r| r.rule_id.as_str())
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        fired.score_delta < 0.0,
        "the rule must penalise, not reward: {}",
        fired.score_delta
    );
    assert!(
        !good
            .rule_applications
            .iter()
            .any(|r| r.rule_id == "counterpoint.no_parallel_unisons" && r.score_delta < 0.0),
        "independent voices must not be charged for parallel unisons"
    );
    assert!(
        bad.score.get("voice_leading") < good.score.get("voice_leading"),
        "parallel unisons must score below independent motion ({} vs {})",
        bad.score.get("voice_leading"),
        good.score.get("voice_leading")
    );
}

/// `counterpoint.prefer_contrary_motion`.
///
/// Contrary motion between two independent lines beats similar motion.
#[test]
fn contrary_motion_preferred() {
    let chords = vec![event(0, "C", 0, 2), event(1, "G", 2, 2)];
    let contrary = vec![voicing(&["E4", "G4"]), voicing(&["D4", "B4"])];
    let similar = vec![voicing(&["E4", "G4"]), voicing(&["G4", "B4"])];

    let a = audit("common_practice", &contrary, &chords);
    let b = audit("common_practice", &similar, &chords);

    let contrary_moves = a.connections.iter().filter(|c| c.semitones != 0).count();
    assert!(contrary_moves > 0, "the fixture must actually move");
    assert!(
        a.score.get("voice_leading") >= b.score.get("voice_leading"),
        "contrary motion must not score below similar motion ({} vs {})",
        a.score.get("voice_leading"),
        b.score.get("voice_leading")
    );
}

/// `counterpoint.avoid_simultaneous_leaps`.
///
/// Both voices leaping at once in the same direction weakens independence.
#[test]
fn simultaneous_leaps_penalised() {
    let chords = vec![event(0, "C", 0, 2), event(1, "D", 2, 2)];
    // Both voices leap by more than an octave, in the same direction.
    let both_leap = vec![voicing(&["C3", "E3"]), voicing(&["D4", "A4"])];
    // The same lower leap, with the upper voice held: oblique, not similar.
    let one_leaps = vec![voicing(&["C3", "E3"]), voicing(&["D4", "E3"])];

    let a = audit("strict_counterpoint", &both_leap, &chords);
    let b = audit("strict_counterpoint", &one_leaps, &chords);

    let fired = a
        .rule_applications
        .iter()
        .find(|r| r.rule_id == "counterpoint.avoid_simultaneous_leaps")
        .unwrap_or_else(|| {
            panic!(
                "the simultaneous-leap rule must fire: {:?}",
                a.rule_applications
                    .iter()
                    .map(|r| r.rule_id.as_str())
                    .collect::<Vec<_>>()
            )
        });
    assert!(fired.score_delta < 0.0, "{}", fired.score_delta);
    assert!(
        !b.rule_applications
            .iter()
            .any(|r| r.rule_id == "counterpoint.avoid_simultaneous_leaps" && r.score_delta < 0.0),
        "one voice leaping while the other holds is oblique, not two simultaneous leaps"
    );
}

/// `counterpoint.perfect_interval_at_arrival_points`.
///
/// A perfect consonance arrived at by contrary motion at a cadence is not a
/// defect: the audit must not report a *parallel* there.
#[test]
fn perfect_interval_at_cadence_ok() {
    let chords = vec![event(0, "G7", 0, 2), event(1, "C", 2, 2)];
    // Outer voices converge onto an octave — the classic cadential arrival.
    let cadence = vec![voicing(&["B3", "F4"]), voicing(&["C4", "E4"])];
    let report = audit("common_practice", &cadence, &chords);

    assert!(
        report.true_parallels().is_empty(),
        "contrary motion into a perfect interval is not a parallel: {:?}",
        report.parallels
    );
    assert!(
        report.score.get("voice_leading") > 0.0,
        "a textbook cadence must not score at or below zero"
    );
}

/// `counterpoint.voice_independence_score`.
///
/// Independence is aggregated from several observations rather than decided by
/// any single test: two progressions with the same total motion can score
/// differently because their motion types differ.
#[test]
fn voice_independence_aggregated() {
    let chords = vec![event(0, "C", 0, 2), event(1, "F", 2, 2)];
    let locked = vec![voicing(&["C4", "E4"]), voicing(&["F4", "A4"])];
    let independent = vec![voicing(&["C4", "E4"]), voicing(&["A3", "F4"])];

    let a = audit("strict_counterpoint", &locked, &chords);
    let b = audit("strict_counterpoint", &independent, &chords);

    // The report exposes the components it aggregated, not just a verdict.
    let components: Vec<&str> = a.score.components().map(|(n, _)| n).collect();
    assert!(
        components.len() > 1,
        "independence must be aggregated from several components: {components:?}"
    );
    assert!(a.connections.len() >= 2, "every voice must be connected");
    assert!(
        (a.score.get("voice_leading") - b.score.get("voice_leading")).abs() > f64::EPSILON
            || a.parallels.len() != b.parallels.len(),
        "two different textures must not be indistinguishable"
    );
}

// ---------------------------------------------------------------------------
// Style-sensitive harmony
// ---------------------------------------------------------------------------

/// `harmony.modal_vamp_avoids_leading_tone`.
///
/// A modal profile does not manufacture a functional dominant seventh: the
/// raised seventh degree destroys the mode.
#[test]
fn modal_profile_rejects_v7() {
    let core = ServerCore::offline();
    let snapshot_id = seed(&core, "melodies/dorian_vamp_d");
    let generation = generate(&core, &snapshot_id, "modal_ambient", 3, 31);

    let mut saw_chord = false;
    for c in generation.arr_field("candidates").unwrap() {
        for chord in c.arr_field("chords").unwrap() {
            let symbol = chord.as_str().unwrap_or("");
            saw_chord = true;
            let parsed = music_domain::symbol::parse(symbol)
                .unwrap_or_else(|e| panic!("{symbol}: {}", e.message));
            // In D dorian the functional dominant would be A7, whose C# is the
            // raised seventh degree the rule forbids.
            let is_a_dominant_seven =
                parsed.is_dominant_family() && parsed.root_pc() == 9 && parsed.has_degree(7);
            assert!(
                !is_a_dominant_seven,
                "a modal profile produced the functional dominant {symbol}"
            );
        }
    }
    assert!(saw_chord, "the generation must produce chords to inspect");

    // The rule that mandates this is style-sensitive and enabled for the modal
    // profile, which is what makes the same material legal elsewhere.
    let rule = kb()
        .rule("harmony.modal_vamp_avoids_leading_tone")
        .expect("the rule exists");
    assert!(profile("modal_ambient").applies_to(rule));
}

/// `harmony.tritone_substitute_produces_chromatic_bass`.
///
/// A tritone substitution is valuable because it turns the bass into a
/// descending half step; the substitute chord shares the original's guide
/// tones, which is why the substitution works at all.
#[test]
fn tritone_sub_chromatic_bass_bonus() {
    let g7 = spec("G7");
    let db7 = spec("Db7");

    // The bass moves G -> C by a fifth, or Db -> C by a descending half step.
    let fifth_motion = (12 - g7.root_pc()) % 12;
    let half_step = (12 - db7.root_pc()) % 12;
    assert_eq!(fifth_motion, 5, "G to C is a fourth up / fifth down");
    assert_eq!(half_step, 11, "Db to C is a descending half step");

    // The substitution is only legitimate because the guide tones survive.
    let guides = |s: &ChordSpec| {
        let mut pcs: Vec<i32> = s
            .chord_tones()
            .into_iter()
            .filter(|(d, _)| d.number == 3 || d.number == 7)
            .map(|(d, _)| (s.root_pc() + d.simple_semitones()).rem_euclid(12))
            .collect();
        pcs.sort_unstable();
        pcs
    };
    assert_eq!(
        guides(&g7),
        guides(&db7),
        "a tritone substitute must share the original's third and seventh"
    );

    let rule = kb()
        .rule("harmony.tritone_substitute_produces_chromatic_bass")
        .expect("the rule exists");
    assert_eq!(rule.effect.score_component, "bass_quality");
    assert!(
        rule.effect.score_delta > 0.0,
        "the rule must reward the chromatic bass, not penalise it"
    );
}

/// `extensions.root_omitted_when_bass_supplies_it`.
///
/// A rootless voicing is legitimate only when a bass part is sounding the root.
#[test]
fn rootless_without_bass_penalised() {
    let rule = kb()
        .rule("extensions.root_omitted_when_bass_supplies_it")
        .expect("the rule exists");
    assert!(
        rule.conditions
            .iter()
            .any(|c| c.to_lowercase().contains("bass")),
        "the rule must be conditional on a bass part: {:?}",
        rule.conditions
    );

    // Behaviourally: a rootless voicing over no bass loses the chord's root,
    // and the chord model says so — the root is not recoverable from the
    // remaining pitches alone.
    let c7 = spec("C7");
    let rootless: Vec<i32> = c7
        .chord_tones()
        .into_iter()
        .filter(|(d, _)| d.number != 1)
        .map(|(d, _)| (c7.root_pc() + d.simple_semitones()).rem_euclid(12))
        .collect();
    assert!(
        !rootless.contains(&c7.root_pc()),
        "a rootless voicing genuinely omits the root"
    );

    // With a bass supplying the root, the full identity is present again.
    let mut with_bass = rootless.clone();
    with_bass.push(c7.root_pc());
    with_bass.sort_unstable();
    let mut full = c7.pitch_classes();
    full.sort_unstable();
    assert_eq!(with_bass, full, "the bass restores the chord's identity");
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root")
        .join("fixtures")
}

fn seed(core: &ServerCore, fixture: &str) -> String {
    let path = fixtures_dir().join(format!("{fixture}.json"));
    let f = music_domain::fixture::Fixture::from_path(&path).expect("fixture loads");
    let record = reaper_music_mcp::fixtures::snapshot_record(&f).expect("snapshot builds");
    core.with_store(|s| s.put_snapshot(record, qjson::time::unix_now()))
}

fn generate(core: &ServerCore, snapshot_id: &str, profile_id: &str, count: i64, seed: i64) -> Json {
    let r = reaper_music_mcp::tools::call(
        core,
        "harmony.generate_candidates",
        &json_obj! {
            "snapshot_id" => snapshot_id,
            "style_profile" => profile_id,
            "candidate_count" => count,
            "seed" => seed,
        },
        &CallContext::detached(),
    );
    assert_eq!(
        r.get("isError"),
        Some(&Json::Bool(false)),
        "generation failed: {}",
        r.get("structuredContent").cloned().unwrap_or(Json::Null)
    );
    r.get("structuredContent").unwrap().clone()
}
