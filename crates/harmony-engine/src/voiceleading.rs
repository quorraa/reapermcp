//! Voice-leading analysis and audit.
//!
//! Everything here is **profile-driven**. The same pair of parallel fifths is a
//! serious fault under `strict_counterpoint`, a cost under `common_practice`, a
//! shrug under `neo_soul_rnb` and the entire idiom under `pop_rock`. The audit
//! reports the same facts in every case; what changes is the weight, which is
//! read from the profile and from the knowledge base's own rules rather than
//! from a constant in this file.
//!
//! Low-register spacing likewise comes from instrument data. There is no single
//! universal low-interval limit here, because there is no single instrument.

use crate::ctx::RULE_DELTA_SCALE;
use crate::factbuild::{sound_facts_for_voicing, sounding_degree_texts};
use music_domain::prelude::*;
use theory_kb::rules::facts;
use theory_kb::{
    InstrumentProfile, KnowledgeBase, ResolvedProfile, RuleContext, RuleEngine, RuleEvent,
};

/// One parallel or direct perfect interval.
#[derive(Clone, Debug, PartialEq)]
pub struct Parallel {
    /// The perfect interval the voices arrive on.
    pub interval: Interval,
    /// Index of the chord the motion starts from.
    pub from_index: usize,
    /// The two voices involved.
    pub voices: (VoiceId, VoiceId),
    /// True for a *direct* (hidden) perfect interval reached by similar motion,
    /// false for a true parallel.
    pub hidden: bool,
}

/// Everything the audit found.
#[derive(Clone, Debug, Default)]
pub struct VoiceLeadingReport {
    /// Every voice-to-voice connection, in time then voice order.
    pub connections: Vec<VoiceLeadingConnection>,
    /// Total semitone travel across every voice.
    pub total_motion: i32,
    /// Largest single-voice leap in semitones.
    pub max_leap: i32,
    /// Parallel and direct perfect intervals.
    pub parallels: Vec<Parallel>,
    /// Voice crossings.
    pub crossings: usize,
    /// Voice overlaps.
    pub overlaps: usize,
    /// Human-readable descriptions of tendency tones left hanging.
    pub unresolved_tendencies: Vec<String>,
    /// Raw score components plus the profile-weighted total.
    pub score: ScoreVector,
    /// Every rule consulted.
    pub rule_applications: Vec<RuleApplication>,
    /// Non-fatal findings.
    pub findings: Vec<Warning>,
}

impl VoiceLeadingReport {
    /// The true parallels, excluding direct perfect intervals.
    pub fn true_parallels(&self) -> Vec<&Parallel> {
        self.parallels.iter().filter(|p| !p.hidden).collect()
    }

    /// How many voice pairs held a common tone in the same voice.
    pub fn common_tones_retained(&self) -> usize {
        self.connections.iter().filter(|c| c.semitones == 0).count()
    }
}

/// Everything the audit needs that the frozen signature does not carry.
#[derive(Clone, Debug, Default)]
pub struct AuditOptions {
    /// The melody, when one is being harmonised under.
    pub melody: Option<NoteSet>,
    /// True when the context is quartal, planing or constant-structure.
    pub planing: bool,
    /// True when a modal centre is active.
    pub modal: bool,
    /// True when the caller explicitly asked for clusters.
    pub cluster_intent: bool,
    /// Instrument profile whose spacing limits apply.
    pub instrument_profile: Option<String>,
    /// True when the audited part is a monophonic bass.
    pub role_is_bass: bool,
}

/// How much this profile charges for a parallel perfect interval.
///
/// This is the profile's declared `parallel_motion_treatment`, not a constant:
/// `forbidden` and `penalised` pay, `neutral` pays a token, and `idiomatic`
/// pays nothing at all, because in that idiom it is the sound being asked for.
pub fn parallel_penalty_scale(prof: &ResolvedProfile) -> f64 {
    match prof.field_str("parallel_motion_treatment") {
        Some("forbidden") => 1.5,
        Some("penalised") => 1.0,
        Some("neutral") => 0.3,
        Some("idiomatic") => 0.0,
        _ => 0.8,
    }
}

/// The audit, as the frozen contract declares it.
pub fn audit_voice_leading(
    kb: &KnowledgeBase,
    prof: &ResolvedProfile,
    voicings: &[Voicing],
    chords: &[ChordEvent],
) -> VoiceLeadingReport {
    audit_with(kb, prof, voicings, chords, &AuditOptions::default())
}

/// The audit with the extra musical context the caller knows about.
pub fn audit_with(
    kb: &KnowledgeBase,
    prof: &ResolvedProfile,
    voicings: &[Voicing],
    chords: &[ChordEvent],
    opts: &AuditOptions,
) -> VoiceLeadingReport {
    let engine = RuleEngine::new(kb, prof);
    let instrument = opts
        .instrument_profile
        .as_deref()
        .and_then(|id| kb.instrument_profile(id));
    let mut report = VoiceLeadingReport::default();
    let mut score = ScoreVector::new();
    for c in SCORE_COMPONENTS {
        score.set(c, 0.0);
    }

    for (index, voicing) in voicings.iter().enumerate() {
        let chord = chords.get(index);
        audit_one_voicing(
            &engine,
            voicing,
            chord,
            opts,
            instrument,
            &mut report,
            &mut score,
        );
    }

    for index in 0..voicings.len().saturating_sub(1) {
        let a = &voicings[index];
        let b = &voicings[index + 1];
        collect_connections(a, b, index, &mut report);
        collect_parallels(a, b, index, prof, &mut report);
        audit_transition(
            &engine,
            a,
            b,
            index,
            chords,
            opts,
            prof,
            &mut report,
            &mut score,
        );
    }

    report.total_motion = report.connections.iter().map(|c| c.semitones.abs()).sum();
    report.max_leap = report
        .connections
        .iter()
        .map(|c| c.semitones.abs())
        .max()
        .unwrap_or(0);

    let smoothness = if report.connections.is_empty() {
        0.5
    } else {
        let mean = report.total_motion as f64 / report.connections.len() as f64;
        (1.0 - mean / 4.0).clamp(0.0, 1.0)
    };
    score.add("voice_leading", smoothness);
    // Planing and cluster contexts are the sound being asked for, so the
    // parallels they produce are reported and cost nothing.
    let scale = if opts.planing || opts.cluster_intent {
        0.0
    } else {
        parallel_penalty_scale(prof)
    };
    score.add(
        "voice_leading",
        -0.25 * scale * report.true_parallels().len() as f64,
    );
    score.add("voice_leading", -0.15 * report.crossings as f64);
    score.add("voice_leading", -0.10 * report.overlaps as f64);
    score.add(
        "voice_leading",
        -0.12 * report.unresolved_tendencies.len() as f64,
    );

    let weights: Vec<(&str, f64)> = SCORE_COMPONENTS
        .iter()
        .map(|c| (*c, prof.weight(c)))
        .collect();
    score.recompute_total(&weights);
    report.score = score;
    report
}

/// Rules and findings for one realised voicing.
#[allow(clippy::too_many_arguments)] // Each argument is an independent input.
fn audit_one_voicing(
    engine: &RuleEngine<'_>,
    voicing: &Voicing,
    chord: Option<&ChordEvent>,
    opts: &AuditOptions,
    instrument: Option<&InstrumentProfile>,
    report: &mut VoiceLeadingReport,
    score: &mut ScoreVector,
) {
    let midis: Vec<i32> = voicing.pitches.iter().map(|p| p.midi()).collect();
    if midis.iter().any(|m| !(0..=127).contains(m)) {
        report.findings.push(Warning::new(
            "MIDI_OUT_OF_RANGE",
            "a realised voicing left the MIDI range",
            Severity::Major,
        ));
    }
    for pair in midis.windows(2) {
        if pair[1] <= pair[0] {
            report.crossings += 1;
        }
    }
    let Some(chord) = chord else {
        return;
    };
    let mut rc = RuleContext::new();
    rc.set_event(RuleEvent::VoicingBuilt);
    sound_facts_for_voicing(&mut rc, &chord.spec, voicing, chord.inversion);
    rc.set_str(facts::VOICING_FAMILY, voicing.family.id());
    rc.set_bool(
        "chord_is_rootless_voicing",
        voicing.family == VoicingFamily::Rootless,
    );
    rc.set_bool("intentional_cluster", opts.cluster_intent);
    rc.set_bool("cluster_intent_is_false", !opts.cluster_intent);
    rc.set_bool("modal_center_is_active", opts.modal);
    rc.set_bool(
        "quartal_or_planing_context",
        opts.planing
            || matches!(
                voicing.family,
                VoicingFamily::Quartal | VoicingFamily::Quintal | VoicingFamily::Cluster
            ),
    );
    rc.set_bool("role_is_bass", opts.role_is_bass);
    rc.set_bool(
        "voice_crossing_present",
        midis.windows(2).any(|p| p[1] <= p[0]),
    );
    rc.set_bool("voice_overlap_present", false);
    rc.set_bool(
        "doubling_is_on_tendency_tone",
        doubles_tendency(&chord.spec, &midis),
    );
    if let Some(min) = midis.first() {
        rc.set_num(facts::MIN_SOUNDING_MIDI, f64::from(*min));
    }
    if let Some(max) = midis.last() {
        rc.set_num(facts::MAX_SOUNDING_MIDI, f64::from(*max));
    }
    if let (Some(min), Some(max)) = (midis.first(), midis.last()) {
        rc.set_num(facts::OUTER_VOICE_SPAN_SEMITONES, f64::from(max - min));
    }
    if let Some(gap) = midis.windows(2).map(|p| p[1] - p[0]).min() {
        rc.set_num(facts::MIN_ADJACENT_VOICE_INTERVAL, f64::from(gap));
        if let Some(limit) = instrument.and_then(|p| p.min_spacing_at(midis[0])) {
            rc.set_num(facts::INSTRUMENT_LOW_INTERVAL_LIMIT, f64::from(limit));
        }
    }
    if let Some(instrument) = instrument {
        rc.set_num(
            facts::INSTRUMENT_LOW_MIDI,
            f64::from(instrument.range.low_midi),
        );
        rc.set_num(
            facts::INSTRUMENT_HIGH_MIDI,
            f64::from(instrument.range.high_midi),
        );
        rc.set_bool(
            "voice_exceeds_instrument_range",
            midis.iter().any(|m| !instrument.range.contains(*m)),
        );
    }
    if let Some(distance) = third_to_eleventh(&chord.spec, &midis) {
        rc.set_num(facts::THIRD_TO_ELEVENTH_SEMITONES, f64::from(distance));
    }
    rc.set_bool("melody_is_11", melody_is_eleventh(opts, chord));
    rc.set_bool("total_motion_is_minimal", false);

    let outcome = engine.evaluate(&rc);
    for (component, delta) in &outcome.deltas {
        score.add(component, delta * RULE_DELTA_SCALE);
    }
    for app in outcome.applications {
        if app.status != RuleStatus::NotApplicable {
            report.rule_applications.push(app);
        }
    }
}

/// Rules and findings for one transition.
#[allow(clippy::too_many_arguments)] // Each argument is an independent input.
fn audit_transition(
    engine: &RuleEngine<'_>,
    a: &Voicing,
    b: &Voicing,
    index: usize,
    chords: &[ChordEvent],
    opts: &AuditOptions,
    prof: &ResolvedProfile,
    report: &mut VoiceLeadingReport,
    score: &mut ScoreVector,
) {
    let from = chords.get(index);
    let n = a.pitches.len().min(b.pitches.len());
    let outer = n.saturating_sub(1);
    for i in 0..n {
        let motion = b.pitches[i].midi() - a.pitches[i].midi();
        let mut rc = RuleContext::new();
        rc.set_event(RuleEvent::VoicePairMotion);
        rc.set_num(facts::VOICE_MOTION_SEMITONES, f64::from(motion));
        rc.set_bool("voice_leap_is_large", motion.abs() > 12);
        rc.set_bool("common_tone_available", shares_tone(a, b));
        rc.set_bool("common_tone_retained", motion == 0 && shares_tone(a, b));
        rc.set_bool("outer_voices_involved", i == 0 || i == outer);
        rc.set_bool("modal_center_is_active", opts.modal);
        rc.set_bool("quartal_or_planing_context", opts.planing);
        rc.set_bool("intentional_cluster", opts.cluster_intent);
        rc.set_bool("cluster_intent_is_false", !opts.cluster_intent);
        rc.set_bool("role_is_bass", opts.role_is_bass && i == 0);
        if let Some(from) = from {
            rc.set_str(facts::CHORD_FAMILY, from.spec.family_id());
            // The exception is about the chord that *carries* the seventh: a
            // tonic seventh has nothing to resolve. It is not about where the
            // progression is going.
            rc.set_bool(
                "function_is_tonic",
                from.function == Some(HarmonicFunction::Tonic),
            );
            rc.set_bool(
                "leading_tone_resolves_up_by_step",
                is_leading_tone(&from.spec, a.pitches[i]) && motion == 1,
            );
            rc.set_bool(
                "chordal_seventh_resolves_down_by_step",
                is_chordal_seventh(&from.spec, a.pitches[i]) && (motion == -1 || motion == -2),
            );
            rc.set_bool(
                "altered_tone_resolves_by_step",
                is_altered(&from.spec, a.pitches[i]) && motion.abs() <= 2 && motion != 0,
            );
            if is_leading_tone(&from.spec, a.pitches[i]) && motion != 1 {
                report.unresolved_tendencies.push(format!(
                    "the leading tone {} in voice {i} does not rise to the tonic at {}",
                    a.pitches[i].to_ascii(),
                    from.onset.to_display()
                ));
            }
            if is_chordal_seventh(&from.spec, a.pitches[i]) && !(motion == -1 || motion == -2) {
                report.unresolved_tendencies.push(format!(
                    "the seventh {} in voice {i} does not fall by step at {}",
                    a.pitches[i].to_ascii(),
                    from.onset.to_display()
                ));
            }
        }
        // Motion is classified against the next voice up, which is the pair a
        // parallel-perfect rule is about.
        if let Some(j) = (i + 1..n).next() {
            let other = b.pitches[j].midi() - a.pitches[j].midi();
            let arrival = (b.pitches[j].midi() - b.pitches[i].midi()).abs();
            let relation = relative_motion(motion, other);
            rc.set_str(facts::MOTION, relation.id());
            rc.set_str(facts::INTERVAL, &interval_name(arrival));
            rc.set_str(facts::VOICING_FAMILY, b.family.id());
            rc.set_num(facts::ARRIVAL_INTERVAL_SEMITONES, f64::from(arrival));
            rc.set_bool("motion_is_parallel", relation == RelativeMotion::Parallel);
            rc.set_bool("motion_is_similar", relation == RelativeMotion::Similar);
            rc.set_bool("motion_is_contrary", relation == RelativeMotion::Contrary);
            rc.set_bool("motion_is_oblique", relation == RelativeMotion::Oblique);
            rc.set_bool(
                "interval_is_perfect_fifth_or_octave",
                matches!(arrival % 12, 0 | 7),
            );
        }
        let outcome = engine.evaluate(&rc);
        for (component, delta) in &outcome.deltas {
            let scale = if component == "voice_leading" {
                RULE_DELTA_SCALE * parallel_penalty_scale(prof).max(0.2)
            } else {
                RULE_DELTA_SCALE
            };
            score.add(component, delta * scale);
        }
        for app in outcome.applications {
            if app.status != RuleStatus::NotApplicable {
                report.rule_applications.push(app);
            }
        }
    }
}

/// Records every voice-to-voice connection of one transition.
fn collect_connections(a: &Voicing, b: &Voicing, index: usize, report: &mut VoiceLeadingReport) {
    let n = a.pitches.len().min(b.pitches.len());
    for i in 0..n {
        let from = a.pitches[i];
        let to = b.pitches[i];
        let semitones = to.midi() - from.midi();
        report.connections.push(VoiceLeadingConnection {
            from,
            to,
            voice: VoiceId(i as u16),
            motion: match semitones.abs() {
                0 => MotionKind::Static,
                1 | 2 => MotionKind::Step,
                _ => MotionKind::Leap,
            },
            semitones,
        });
        // Overlap: this voice has moved past where its neighbour was.
        if i > 0 && to.midi() < a.pitches[i - 1].midi() {
            report.overlaps += 1;
        }
        if i + 1 < n && to.midi() > a.pitches[i + 1].midi() {
            report.overlaps += 1;
        }
    }
    let _ = index;
}

/// Records parallel and direct perfect intervals of one transition.
fn collect_parallels(
    a: &Voicing,
    b: &Voicing,
    index: usize,
    prof: &ResolvedProfile,
    report: &mut VoiceLeadingReport,
) {
    let n = a.pitches.len().min(b.pitches.len());
    let outer = n.saturating_sub(1);
    for i in 0..n {
        for j in (i + 1)..n {
            let before = (a.pitches[j].midi() - a.pitches[i].midi()).abs();
            let after = (b.pitches[j].midi() - b.pitches[i].midi()).abs();
            let mi = b.pitches[i].midi() - a.pitches[i].midi();
            let mj = b.pitches[j].midi() - a.pitches[j].midi();
            let perfect_after = matches!(after % 12, 0 | 7) && after != 0;
            if !perfect_after {
                continue;
            }
            let interval = Interval::from_semitones_default(after % 12);
            let same_direction = mi.signum() == mj.signum() && mi != 0;
            if matches!(before % 12, 0 | 7) && before % 12 == after % 12 && mi == mj && mi != 0 {
                report.parallels.push(Parallel {
                    interval,
                    from_index: index,
                    voices: (VoiceId(i as u16), VoiceId(j as u16)),
                    hidden: false,
                });
            } else if same_direction && i == 0 && j == outer && mj.abs() > 2 {
                // A direct perfect interval only matters between the outer
                // voices, and only in a profile that polices it.
                if parallel_penalty_scale(prof) >= 1.0 {
                    report.parallels.push(Parallel {
                        interval,
                        from_index: index,
                        voices: (VoiceId(i as u16), VoiceId(j as u16)),
                        hidden: true,
                    });
                }
            }
        }
    }
}

/// The short interval name a trigger selector matches, e.g. `"P5"`, `"P8"`.
///
/// Compound intervals are reduced, but a real octave keeps its own name
/// because that is what the parallel-octave rule selects on.
pub fn interval_name(semitones: i32) -> String {
    let simple = semitones.rem_euclid(12);
    if simple == 0 {
        return if semitones == 0 { "P1" } else { "P8" }.to_string();
    }
    Interval::from_semitones_default(simple).name()
}

/// The relation between two voices' motions.
pub fn relative_motion(a: i32, b: i32) -> RelativeMotion {
    match (a, b) {
        (0, 0) => RelativeMotion::Static,
        (0, _) | (_, 0) => RelativeMotion::Oblique,
        _ if a == b => RelativeMotion::Parallel,
        _ if a.signum() == b.signum() => RelativeMotion::Similar,
        _ => RelativeMotion::Contrary,
    }
}

/// True when the two voicings share any pitch class.
fn shares_tone(a: &Voicing, b: &Voicing) -> bool {
    a.pitches
        .iter()
        .any(|p| b.pitches.iter().any(|q| q.pitch_class() == p.pitch_class()))
}

/// True when the pitch is the chord's leading tone: a major third of a
/// dominant-family sonority.
fn is_leading_tone(spec: &ChordSpec, pitch: SpelledPitch) -> bool {
    if !spec.is_dominant_family() {
        return false;
    }
    let third = (spec.root_pc() + 4).rem_euclid(12);
    pitch.pitch_class() == third
}

/// True when the pitch is the chord's seventh.
fn is_chordal_seventh(spec: &ChordSpec, pitch: SpelledPitch) -> bool {
    match spec.seventh.alter() {
        Some(alter) => {
            let pc = (spec.root_pc() + 11 + i32::from(alter)).rem_euclid(12);
            pitch.pitch_class() == pc
        }
        None => false,
    }
}

/// True when the pitch is one of the chord's altered degrees.
fn is_altered(spec: &ChordSpec, pitch: SpelledPitch) -> bool {
    spec.alterations
        .iter()
        .any(|d| (spec.root_pc() + d.simple_semitones()).rem_euclid(12) == pitch.pitch_class())
}

/// True when a tendency tone is doubled in the voicing.
fn doubles_tendency(spec: &ChordSpec, midis: &[i32]) -> bool {
    let mut tendency: Vec<i32> = Vec::new();
    if spec.is_dominant_family() {
        tendency.push((spec.root_pc() + 4).rem_euclid(12));
    }
    if let Some(alter) = spec.seventh.alter() {
        tendency.push((spec.root_pc() + 11 + i32::from(alter)).rem_euclid(12));
    }
    tendency
        .iter()
        .any(|pc| midis.iter().filter(|m| m.rem_euclid(12) == *pc).count() > 1)
}

/// Distance in semitones from the sounding third up to the sounding natural
/// eleventh, when both are present.
pub fn third_to_eleventh(spec: &ChordSpec, midis: &[i32]) -> Option<i32> {
    if spec.is_suspended() || spec.omits(3) {
        return None;
    }
    let third = (spec.root_pc() + 4).rem_euclid(12);
    if !matches!(spec.triad, TriadQuality::Major | TriadQuality::Augmented) {
        return None;
    }
    let eleventh = (spec.root_pc() + 5).rem_euclid(12);
    let third_midi = midis.iter().find(|m| m.rem_euclid(12) == third)?;
    let eleventh_midi = midis.iter().find(|m| m.rem_euclid(12) == eleventh)?;
    Some((eleventh_midi - third_midi).abs())
}

/// True when the melody is sounding the chord's eleventh over this chord.
fn melody_is_eleventh(opts: &AuditOptions, chord: &ChordEvent) -> bool {
    let Some(melody) = &opts.melody else {
        return false;
    };
    let eleventh = (chord.spec.root_pc() + 5).rem_euclid(12);
    melody
        .notes
        .iter()
        .filter(|n| n.onset < chord.end() && n.onset + n.duration > chord.onset)
        .any(|n| n.midi.rem_euclid(12) == eleventh)
}

/// Renders the report as JSON for a decision trace.
pub fn report_json(report: &VoiceLeadingReport) -> qjson::Json {
    let mut m = qjson::JsonMap::new();
    m.insert(
        "total_motion",
        qjson::Json::Int(i64::from(report.total_motion)),
    );
    m.insert("max_leap", qjson::Json::Int(i64::from(report.max_leap)));
    m.insert("crossings", qjson::Json::Int(report.crossings as i64));
    m.insert("overlaps", qjson::Json::Int(report.overlaps as i64));
    m.insert(
        "parallels",
        qjson::Json::Arr(
            report
                .parallels
                .iter()
                .map(|p| {
                    let mut pm = qjson::JsonMap::new();
                    pm.insert("interval", qjson::Json::Str(p.interval.name()));
                    pm.insert("from_index", qjson::Json::Int(p.from_index as i64));
                    pm.insert("hidden", qjson::Json::Bool(p.hidden));
                    qjson::Json::Obj(pm)
                })
                .collect(),
        ),
    );
    m.insert(
        "unresolved_tendencies",
        qjson::Json::Arr(
            report
                .unresolved_tendencies
                .iter()
                .map(|t| qjson::Json::Str(t.clone()))
                .collect(),
        ),
    );
    m.insert("score", report.score.to_json());
    qjson::Json::Obj(m)
}

/// The degrees a voicing sounds, exposed for tracing.
pub fn voicing_degrees(spec: &ChordSpec, voicing: &Voicing) -> Vec<String> {
    sounding_degree_texts(spec, voicing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb() -> &'static KnowledgeBase {
        KnowledgeBase::embedded()
    }

    fn voicing(pitches: &[&str]) -> Voicing {
        let ps: Vec<SpelledPitch> = pitches
            .iter()
            .map(|p| SpelledPitch::parse(p).expect("pitch"))
            .collect();
        let mut v = Voicing::new(ps, VoicingFamily::Close);
        v.voices = (0..v.pitches.len()).map(|i| VoiceId(i as u16)).collect();
        v
    }

    fn chord(id: u32, sym: &str, onset: i64) -> ChordEvent {
        let mut e = ChordEvent::new(
            id,
            symbol::parse(sym).expect("symbol"),
            BeatTime::from_quarters(onset),
            BeatTime::from_quarters(4),
        );
        e.function = Some(if sym.starts_with('G') {
            HarmonicFunction::Dominant
        } else {
            HarmonicFunction::Tonic
        });
        e
    }

    #[test]
    fn parallel_fifths_are_detected() {
        let a = voicing(&["C4", "G4"]);
        let b = voicing(&["D4", "A4"]);
        let report = audit_voice_leading(
            kb(),
            &kb().resolve_profile("strict_counterpoint").expect("p"),
            &[a, b],
            &[chord(0, "C", 0), chord(1, "D", 4)],
        );
        assert_eq!(report.true_parallels().len(), 1);
        assert_eq!(report.true_parallels()[0].interval.semitones(), 7);
    }

    #[test]
    fn parallel_octaves_are_detected() {
        let a = voicing(&["C4", "C5"]);
        let b = voicing(&["D4", "D5"]);
        let report = audit_voice_leading(
            kb(),
            &kb().resolve_profile("common_practice").expect("p"),
            &[a, b],
            &[chord(0, "C", 0), chord(1, "D", 4)],
        );
        assert_eq!(report.true_parallels().len(), 1);
    }

    #[test]
    fn parallel_fifths_cost_in_strict_counterpoint_and_not_in_pop_rock() {
        let a = voicing(&["C3", "G3"]);
        let b = voicing(&["D3", "A3"]);
        let chords = [chord(0, "C", 0), chord(1, "D", 4)];
        let strict = audit_voice_leading(
            kb(),
            &kb().resolve_profile("strict_counterpoint").expect("p"),
            &[a.clone(), b.clone()],
            &chords,
        );
        let power = audit_voice_leading(
            kb(),
            &kb().resolve_profile("pop_rock").expect("p"),
            &[a, b],
            &chords,
        );
        assert_eq!(strict.true_parallels().len(), 1);
        assert_eq!(power.true_parallels().len(), 1);
        assert!(
            strict.score.get("voice_leading") < power.score.get("voice_leading"),
            "strict {} should be worse than pop_rock {}",
            strict.score.get("voice_leading"),
            power.score.get("voice_leading")
        );
        assert_eq!(
            parallel_penalty_scale(&kb().resolve_profile("pop_rock").unwrap()),
            0.0
        );
    }

    #[test]
    fn contrary_motion_beats_parallel_motion_in_strict_profiles() {
        let prof = kb().resolve_profile("strict_counterpoint").expect("p");
        let chords = [chord(0, "C", 0), chord(1, "G", 4)];
        let parallel = audit_voice_leading(
            kb(),
            &prof,
            &[voicing(&["C4", "G4"]), voicing(&["D4", "A4"])],
            &chords,
        );
        let contrary = audit_voice_leading(
            kb(),
            &prof,
            &[voicing(&["C4", "G4"]), voicing(&["B3", "A4"])],
            &chords,
        );
        assert!(contrary.score.get("voice_leading") > parallel.score.get("voice_leading"));
    }

    #[test]
    fn common_tone_retention_is_reported() {
        let report = audit_voice_leading(
            kb(),
            &kb().resolve_profile("common_practice").expect("p"),
            &[voicing(&["C4", "E4", "G4"]), voicing(&["C4", "F4", "A4"])],
            &[chord(0, "C", 0), chord(1, "F", 4)],
        );
        assert_eq!(report.common_tones_retained(), 1);
        assert_eq!(report.total_motion, 3);
    }

    #[test]
    fn crossings_and_overlaps_are_counted() {
        let mut crossed = voicing(&["C4", "E4"]);
        crossed.pitches = vec![
            SpelledPitch::parse("E4").unwrap(),
            SpelledPitch::parse("C4").unwrap(),
        ];
        let report = audit_voice_leading(
            kb(),
            &kb().resolve_profile("common_practice").expect("p"),
            &[crossed],
            &[chord(0, "C", 0)],
        );
        assert_eq!(report.crossings, 1);
    }

    #[test]
    fn tendency_tones_are_tracked_in_g7_to_c() {
        // B rises to C, F falls to E.
        let report = audit_voice_leading(
            kb(),
            &kb().resolve_profile("common_practice").expect("p"),
            &[voicing(&["G3", "B3", "F4"]), voicing(&["C4", "C4", "E4"])],
            &[chord(0, "G7", 0), chord(1, "C", 4)],
        );
        assert!(
            report.unresolved_tendencies.is_empty(),
            "B->C and F->E resolve: {:?}",
            report.unresolved_tendencies
        );
    }

    #[test]
    fn unresolved_tendencies_are_named() {
        let report = audit_voice_leading(
            kb(),
            &kb().resolve_profile("common_practice").expect("p"),
            &[voicing(&["G3", "B3", "F4"]), voicing(&["C4", "G3", "A4"])],
            &[chord(0, "G7", 0), chord(1, "C", 4)],
        );
        assert!(!report.unresolved_tendencies.is_empty());
        assert!(report
            .unresolved_tendencies
            .iter()
            .any(|t| t.contains("leading tone")));
    }

    #[test]
    fn report_json_is_stable() {
        let report = audit_voice_leading(
            kb(),
            &kb().resolve_profile("common_practice").expect("p"),
            &[voicing(&["C4", "E4"]), voicing(&["D4", "F4"])],
            &[chord(0, "C", 0), chord(1, "Dm", 4)],
        );
        let a = report_json(&report).to_canonical_string();
        let b = report_json(&report).to_canonical_string();
        assert_eq!(a, b);
        assert!(a.contains("total_motion"));
    }

    #[test]
    fn relative_motion_is_classified() {
        assert_eq!(relative_motion(0, 0), RelativeMotion::Static);
        assert_eq!(relative_motion(0, 2), RelativeMotion::Oblique);
        assert_eq!(relative_motion(2, 2), RelativeMotion::Parallel);
        assert_eq!(relative_motion(1, 3), RelativeMotion::Similar);
        assert_eq!(relative_motion(2, -2), RelativeMotion::Contrary);
    }

    #[test]
    fn every_cited_rule_exists() {
        let report = audit_voice_leading(
            kb(),
            &kb().resolve_profile("strict_counterpoint").expect("p"),
            &[voicing(&["C3", "G3", "E4"]), voicing(&["D3", "A3", "F4"])],
            &[chord(0, "C", 0), chord(1, "Dm", 4)],
        );
        assert!(!report.rule_applications.is_empty());
        for app in &report.rule_applications {
            assert!(
                kb().rule(&app.rule_id).is_some(),
                "unknown rule {}",
                app.rule_id
            );
        }
    }

    #[test]
    fn third_to_eleventh_only_applies_over_a_real_major_third() {
        let spec = symbol::parse("Cadd11").expect("Cadd11");
        assert_eq!(third_to_eleventh(&spec, &[60, 64, 65]), Some(1));
        let sus = symbol::parse("Csus4").expect("Csus4");
        assert_eq!(third_to_eleventh(&sus, &[60, 65, 67]), None);
        let minor = symbol::parse("Cm11").expect("Cm11");
        assert_eq!(third_to_eleventh(&minor, &[60, 63, 65]), None);
    }
}
