//! Stage 7 — realising chords as voicings, connected across time.
//!
//! Voicings come from `knowledge/voicings.json`: a template names the degrees
//! it stacks, which of them are required, which may be dropped, the register it
//! belongs in and the profiles that use it. The engine's job is to choose one
//! template and one octave per chord such that the whole chain moves as little
//! as it can while staying inside the range, under the melody, and inside the
//! instrument's spacing limits.
//!
//! That choice is a dynamic program over `(template, octave)` states, not a
//! greedy walk: a template that is cheap here and expensive everywhere after is
//! rejected by the DP, which is the point of assigning voices across time.

use crate::error::HarmonyError;
use crate::params::VoicingParams;
use crate::voiceleading::parallel_penalty_scale;
use music_domain::prelude::*;
use theory_kb::{InstrumentProfile, KnowledgeBase, ResolvedProfile, VoicingTemplate};

/// Octave offsets a template may be tried at.
const OCTAVE_OFFSETS: [i32; 3] = [-1, 0, 1];

/// Templates considered per chord, after profile and chord filtering.
const MAX_TEMPLATES: usize = 10;

/// Realisations kept per chord after ranking, which bounds the DP at
/// `MAX_REALISATIONS^2` transitions per slot.
const MAX_REALISATIONS: usize = 28;

/// Rotations of a template's upper voices considered per template.
///
/// The bass is fixed by the harmony, so only the voices above it are rotated.
/// This is what lets four-part writing keep its upper voices near where they
/// were instead of moving the whole shape in parallel.
const MAX_LAYOUT_VARIANTS: usize = 5;

/// Stage 7, as the frozen contract declares it.
pub fn voice_progression(
    kb: &KnowledgeBase,
    prof: &ResolvedProfile,
    chords: &[ChordEvent],
    melody: Option<&NoteSet>,
    vp: &VoicingParams,
) -> Result<Vec<Voicing>, HarmonyError> {
    vp.validate()?;
    if chords.is_empty() {
        return Ok(Vec::new());
    }
    let instrument = vp
        .instrument_profile
        .as_deref()
        .and_then(|id| kb.instrument_profile(id));
    let (low, high) = window(vp, instrument);
    // How much this style charges for parallel perfect intervals. The voicing
    // DP has to know, or it will happily minimise motion by moving every voice
    // in parallel and leave the audit to complain afterwards.
    let parallels = parallel_penalty_scale(prof);

    // Per chord: every realisation worth considering.
    let mut layers: Vec<Vec<Candidate>> = Vec::with_capacity(chords.len());
    for chord in chords {
        let ceiling = melody_ceiling(melody, chord, vp, high);
        let mut options = realisations(kb, prof, chord, vp, low, high, ceiling, instrument);
        if options.is_empty() {
            options.push(fallback(chord, vp, low, ceiling));
        }
        layers.push(options);
    }

    // Dynamic program: minimise total motion plus per-voicing cost.
    let mut best: Vec<Vec<(f64, Option<usize>)>> = Vec::with_capacity(layers.len());
    for (i, layer) in layers.iter().enumerate() {
        let mut row = Vec::with_capacity(layer.len());
        for candidate in layer.iter() {
            if i == 0 {
                row.push((candidate.cost, None));
                continue;
            }
            let previous = &best[i - 1];
            let mut choice: Option<(f64, usize)> = None;
            for (j, (accumulated, _)) in previous.iter().enumerate() {
                let motion =
                    motion_cost(&layers[i - 1][j].pitches, &candidate.pitches, vp, parallels);
                let total = accumulated + motion + candidate.cost;
                let better = match choice {
                    None => true,
                    Some((best_total, best_j)) => {
                        total < best_total - 1e-9
                            || ((total - best_total).abs() <= 1e-9
                                && layers[i - 1][j].key < layers[i - 1][best_j].key)
                    }
                };
                if better {
                    choice = Some((total, j));
                }
            }
            match choice {
                Some((total, j)) => row.push((total, Some(j))),
                None => row.push((candidate.cost, None)),
            }
        }
        best.push(row);
    }

    // Trace back deterministically.
    let last = best.len() - 1;
    let mut index = 0usize;
    let mut best_total = f64::INFINITY;
    for (i, (total, _)) in best[last].iter().enumerate() {
        if *total < best_total - 1e-9
            || ((*total - best_total).abs() <= 1e-9
                && layers[last][i].key < layers[last][index].key)
        {
            best_total = *total;
            index = i;
        }
    }
    let mut chosen = vec![0usize; layers.len()];
    chosen[last] = index;
    for i in (1..layers.len()).rev() {
        chosen[i - 1] = best[i][chosen[i]].1.unwrap_or(0);
    }

    Ok(chosen
        .into_iter()
        .enumerate()
        .map(|(i, pick)| layers[i][pick].voicing.clone())
        .collect())
}

/// One realisation of one chord.
#[derive(Clone, Debug)]
struct Candidate {
    voicing: Voicing,
    pitches: Vec<i32>,
    cost: f64,
    key: String,
}

/// The effective MIDI window: the request's, narrowed by the instrument's.
fn window(vp: &VoicingParams, instrument: Option<&InstrumentProfile>) -> (i32, i32) {
    match instrument {
        Some(p) => (vp.low.max(p.range.low_midi), vp.high.min(p.range.high_midi)),
        None => (vp.low, vp.high),
    }
}

/// The highest pitch the harmony may take under this chord.
///
/// When the melody is preserved, the harmony sits below it so the line stays
/// the top voice and nothing doubles or crosses it.
fn melody_ceiling(
    melody: Option<&NoteSet>,
    chord: &ChordEvent,
    vp: &VoicingParams,
    high: i32,
) -> i32 {
    if !vp.preserve_top {
        return high;
    }
    let Some(notes) = melody else {
        return high;
    };
    let sounding: Vec<&Note> = notes
        .notes
        .iter()
        .filter(|n| n.onset < chord.end() && n.onset + n.duration > chord.onset)
        .collect();
    match sounding.iter().map(|n| n.midi).min() {
        Some(lowest) => (lowest - 1).min(high),
        None => high,
    }
}

/// Every realisation of one chord worth putting into the dynamic program.
#[allow(clippy::too_many_arguments)] // Each argument is an independent constraint.
fn realisations(
    kb: &KnowledgeBase,
    prof: &ResolvedProfile,
    chord: &ChordEvent,
    vp: &VoicingParams,
    low: i32,
    high: i32,
    ceiling: i32,
    instrument: Option<&InstrumentProfile>,
) -> Vec<Candidate> {
    let templates = usable_templates(kb, prof, chord, vp);
    let mut out = Vec::new();
    for template in templates {
        for (variant, layout) in layout_variants(template).into_iter().enumerate() {
            for offset in OCTAVE_OFFSETS {
                let base = (template.register_hint.low_midi + offset * 12).clamp(low, high);
                let Some(pitches) = realize(&chord.spec, template, &layout, base, chord.inversion)
                else {
                    continue;
                };
                let midis: Vec<i32> = pitches.iter().map(|p| p.midi()).collect();
                if !bass_is_as_declared(&chord.spec, chord.inversion, template, &midis) {
                    continue;
                }
                if midis.iter().any(|m| *m < low || *m > high.min(ceiling)) {
                    continue;
                }
                if midis.iter().any(|m| !(0..=127).contains(m)) {
                    continue;
                }
                let family = VoicingFamily::parse(&template.family).unwrap_or(VoicingFamily::Close);
                let mut voicing = Voicing::new(pitches, family);
                voicing.voices = (0..voicing.pitches.len())
                    .map(|i| VoiceId(i as u16))
                    .collect();
                let cost = voicing_cost(template, &chord.spec, &midis, vp, ceiling, instrument);
                out.push(Candidate {
                    pitches: midis,
                    cost,
                    key: format!("{}:{variant}:{offset}", template.id),
                    voicing,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.cost
            .partial_cmp(&b.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.key.cmp(&b.key))
    });
    out.truncate(MAX_REALISATIONS);
    out
}

/// The templates this profile, chord and request admit.
fn usable_templates<'k>(
    kb: &'k KnowledgeBase,
    prof: &ResolvedProfile,
    chord: &ChordEvent,
    vp: &VoicingParams,
) -> Vec<&'k VoicingTemplate> {
    let mut out: Vec<&VoicingTemplate> = kb
        .voicing_templates()
        .iter()
        .filter(|t| {
            t.style_profiles
                .iter()
                .any(|p| prof.chain.iter().any(|c| c == p))
        })
        .filter(|t| {
            vp.families.is_empty()
                || VoicingFamily::parse(&t.family)
                    .map(|f| vp.families.contains(&f))
                    .unwrap_or(false)
        })
        .filter(|t| t.voice_count as usize <= vp.voice_count + 1)
        .filter(|t| has_required(&chord.spec, t))
        .collect();
    out.sort_by(|a, b| {
        let da = (a.voice_count - vp.voice_count as i64).abs();
        let db = (b.voice_count - vp.voice_count as i64).abs();
        da.cmp(&db).then_with(|| a.id.cmp(&b.id))
    });
    out.truncate(MAX_TEMPLATES);
    out
}

/// True when the chord actually contains every degree the template requires.
fn has_required(spec: &ChordSpec, template: &VoicingTemplate) -> bool {
    template
        .required_degrees
        .iter()
        .all(|d| find_tone(spec, d).is_some())
}

/// Locates a chord tone by a template degree string.
///
/// A degree written without an accidental matches the chord's own spelling of
/// that number, so `3` finds the minor third of a minor chord. A degree written
/// with one must match exactly, so `b7` never silently becomes a major seventh.
fn find_tone(spec: &ChordSpec, degree_text: &str) -> Option<(ChordDegree, (Letter, Accidental))> {
    let wanted = ChordDegree::parse(degree_text)?;
    let tones = spec.chord_tones();
    let explicit = degree_text.starts_with('b') || degree_text.starts_with('#');
    let simple = |n: u8| if n > 7 { n - 7 } else { n };
    tones
        .iter()
        .find(|(d, _)| *d == wanted || (d.number == wanted.number && d.alter == wanted.alter))
        .or_else(|| {
            if explicit {
                None
            } else {
                tones
                    .iter()
                    .find(|(d, _)| simple(d.number) == simple(wanted.number))
            }
        })
        .copied()
}

/// Stacks a template's degrees into concrete pitches from `base` upwards.
fn realize(
    spec: &ChordSpec,
    template: &VoicingTemplate,
    layout: &[String],
    base: i32,
    inversion: u8,
) -> Option<Vec<SpelledPitch>> {
    let mut layout: Vec<String> = layout.to_vec();
    // An inversion asks for a specific chord tone underneath; a template that
    // already starts on it needs no help.
    if inversion > 0 {
        if let Some(bottom) = inversion_degree(spec, inversion) {
            if layout.first().map(String::as_str) != Some(bottom.as_str()) {
                layout.insert(0, bottom);
            }
        }
    }
    let mut out: Vec<SpelledPitch> = Vec::new();
    let mut floor = base;
    for degree_text in &layout {
        let Some((_, (letter, accidental))) = find_tone(spec, degree_text) else {
            if template.required_degrees.iter().any(|d| d == degree_text) {
                return None;
            }
            continue;
        };
        let pc = (letter.natural_pc() + i32::from(accidental.0)).rem_euclid(12);
        let mut midi = floor + (pc - floor).rem_euclid(12);
        if let Some(previous) = out.last() {
            if midi <= previous.midi() {
                midi += 12;
            }
        }
        if !(0..=127).contains(&midi) {
            return None;
        }
        let octave = (midi - letter.natural_pc() - i32::from(accidental.0)) / 12 - 1;
        let pitch = SpelledPitch::new(letter, accidental, octave);
        if pitch.midi() != midi {
            return None;
        }
        out.push(pitch);
        floor = midi + 1;
    }
    if out.len() < 2 {
        return None;
    }
    Some(out)
}

/// True when the realisation puts the chord's declared bass note underneath.
///
/// Rootless and upper-structure voicings are the deliberate exception: they
/// exist precisely because another part is carrying the root, so their bottom
/// voice is not the chord's bass and is not expected to be.
fn bass_is_as_declared(
    spec: &ChordSpec,
    inversion: u8,
    template: &VoicingTemplate,
    midis: &[i32],
) -> bool {
    if matches!(template.family.as_str(), "rootless" | "upper_structure") {
        return true;
    }
    let want = if spec.bass.is_some() {
        spec.bass_pc()
    } else {
        let tones = spec.chord_tones();
        if tones.is_empty() {
            return true;
        }
        let (_, (letter, accidental)) = tones[(inversion as usize).min(tones.len() - 1)];
        (letter.natural_pc() + i32::from(accidental.0)).rem_euclid(12)
    };
    midis.first().map(|m| m.rem_euclid(12)) == Some(want)
}

/// The layouts a template is tried at: the one it declares, plus rotations of
/// everything above its bass note.
fn layout_variants(template: &VoicingTemplate) -> Vec<Vec<String>> {
    let layout = &template.degree_layout;
    let mut out = vec![layout.clone()];
    if layout.len() > 2 {
        let head = layout[0].clone();
        let tail = &layout[1..];
        for shift in 1..tail.len().min(MAX_LAYOUT_VARIANTS) {
            let mut rotated = vec![head.clone()];
            rotated.extend(tail[shift..].iter().cloned());
            rotated.extend(tail[..shift].iter().cloned());
            if !out.contains(&rotated) {
                out.push(rotated);
            }
        }
    }
    out
}

/// The degree that belongs in the bass for an inversion.
fn inversion_degree(spec: &ChordSpec, inversion: u8) -> Option<String> {
    let tones = spec.chord_tones();
    tones.get(inversion as usize).map(|(d, _)| d.to_string())
}

/// What one realisation costs on its own, before motion is considered.
#[allow(clippy::too_many_arguments)] // Each argument is an independent constraint.
fn voicing_cost(
    template: &VoicingTemplate,
    spec: &ChordSpec,
    midis: &[i32],
    vp: &VoicingParams,
    ceiling: i32,
    instrument: Option<&InstrumentProfile>,
) -> f64 {
    let mut cost = 0.0;
    // Guide tones carry the chord's quality: dropping one changes what the
    // listener hears the chord to be, which is a different thing from thinning
    // the texture. The fifth, by contrast, is the cheapest thing to lose.
    let sounding: Vec<i32> = midis.iter().map(|m| m.rem_euclid(12)).collect();
    for degree in spec.guide_tones() {
        let pc = (spec.root_pc() + degree.simple_semitones()).rem_euclid(12);
        if !sounding.contains(&pc) {
            cost += 8.0;
        }
    }
    for degree in &spec.extensions {
        let pc = (spec.root_pc() + degree.simple_semitones()).rem_euclid(12);
        if !sounding.contains(&pc) {
            cost += 1.5;
        }
    }
    for degree in &spec.alterations {
        let pc = (spec.root_pc() + degree.simple_semitones()).rem_euclid(12);
        if !sounding.contains(&pc) {
            cost += 2.5;
        }
    }
    // Asking for four voices in a window that cannot hold four voices is a
    // request the register overrules: a thinner texture is charged for, but far
    // less than cramming would cost in spacing and parallel motion.
    let room = (ceiling - midis.first().copied().unwrap_or(60)).max(0);
    let crowding = if room < 12 * (vp.voice_count as i32 - 1) {
        0.6
    } else {
        1.5
    };
    cost += (midis.len() as f64 - vp.voice_count as f64).abs() * crowding;
    let bottom = *midis.first().unwrap_or(&60);
    let top = *midis.last().unwrap_or(&60);
    if bottom < template.register_hint.low_midi {
        cost += (template.register_hint.low_midi - bottom) as f64 * 0.05;
    }
    if top > template.register_hint.high_midi {
        cost += (top - template.register_hint.high_midi) as f64 * 0.05;
    }
    if top > ceiling {
        cost += (top - ceiling) as f64 * 2.0;
    }
    // Low-register congestion, from instrument data rather than one universal
    // limit; a template that declares its own limit wins over the instrument's.
    for pair in midis.windows(2) {
        let gap = pair[1] - pair[0];
        let limit = template
            .min_spacing_at(pair[0])
            .or_else(|| instrument.and_then(|p| p.min_spacing_at(pair[0])));
        if let Some(min) = limit {
            if gap < min {
                cost += (min - gap) as f64 * 0.6;
            }
        }
    }
    cost
}

/// The cost of moving from one voicing to the next.
fn motion_cost(prev: &[i32], next: &[i32], vp: &VoicingParams, parallels: f64) -> f64 {
    let n = prev.len().min(next.len());
    if n == 0 {
        return 0.0;
    }
    let mut cost = 0.0;
    for i in 0..n {
        let d = (next[i] - prev[i]).abs();
        cost += d as f64 * 0.35;
        if d > vp.max_leap {
            cost += (d - vp.max_leap) as f64 * 1.2;
        }
    }
    if parallels > 0.0 {
        cost += parallels * 5.0 * count_parallel_perfects(prev, next) as f64;
    }
    cost += (prev.len() as i32 - next.len() as i32).abs() as f64 * 0.8;
    cost / n as f64
}

/// Parallel perfect fifths and octaves between two realisations.
pub fn count_parallel_perfects(prev: &[i32], next: &[i32]) -> usize {
    let n = prev.len().min(next.len());
    let mut found = 0;
    for i in 0..n {
        for j in (i + 1)..n {
            let before = (prev[j] - prev[i]).abs();
            let after = (next[j] - next[i]).abs();
            let motion = next[i] - prev[i];
            if motion == 0 || motion != next[j] - prev[j] {
                continue;
            }
            if matches!(before % 12, 0 | 7) && before % 12 == after % 12 {
                found += 1;
            }
        }
    }
    found
}

/// A close-position realisation for a chord no template fits.
///
/// It stacks chord tones upwards from the floor and stops when the ceiling is
/// reached, so the result is always strictly ascending: a voicing whose voices
/// are out of order is a voice crossing the caller never asked for.
fn fallback(chord: &ChordEvent, vp: &VoicingParams, low: i32, ceiling: i32) -> Candidate {
    let tones = chord.spec.chord_tones();
    let floor = low.clamp(0, 127);
    let ceiling = ceiling.clamp(floor + 1, 127);
    let mut pitches: Vec<SpelledPitch> = Vec::new();
    let mut cursor = floor;
    for (_, (letter, accidental)) in tones.iter().take(vp.voice_count) {
        let pc = (letter.natural_pc() + i32::from(accidental.0)).rem_euclid(12);
        let midi = cursor + (pc - cursor).rem_euclid(12);
        if midi > ceiling {
            break;
        }
        let octave = (midi - letter.natural_pc() - i32::from(accidental.0)).div_euclid(12) - 1;
        let pitch = SpelledPitch::new(*letter, *accidental, octave);
        if pitch.midi() != midi || !pitch.is_valid_midi() {
            break;
        }
        pitches.push(pitch);
        cursor = midi + 1;
    }
    if pitches.is_empty() {
        // Nothing fits the window: place the root as close to it as possible.
        let root = chord.spec.root;
        let pc = (root.0.natural_pc() + i32::from(root.1 .0)).rem_euclid(12);
        let midi = place_in(pc, floor, ceiling);
        let octave = (midi - root.0.natural_pc() - i32::from(root.1 .0)).div_euclid(12) - 1;
        pitches.push(SpelledPitch::new(root.0, root.1, octave));
    }
    let midis: Vec<i32> = pitches.iter().map(|p| p.midi()).collect();
    let mut voicing = Voicing::new(pitches, VoicingFamily::Close);
    voicing.voices = (0..voicing.pitches.len())
        .map(|i| VoiceId(i as u16))
        .collect();
    Candidate {
        pitches: midis,
        cost: 6.0,
        key: "fallback:close".to_string(),
        voicing,
    }
}

/// The lowest pitch of a class at or above `floor`, folded down while it
/// exceeds `ceiling` and a lower octave still exists.
fn place_in(pc: i32, floor: i32, ceiling: i32) -> i32 {
    let mut midi = floor + (pc - floor).rem_euclid(12);
    while midi > ceiling && midi - 12 >= 0 {
        midi -= 12;
    }
    midi.clamp(0, 127)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidates::build_pools;
    use crate::params::{CancelFlag, SearchConfig};
    use crate::search::search_in;
    use crate::testing;

    fn chords(fixture: &str, profile: &str) -> (testing::Harness, Vec<ChordEvent>) {
        let h = testing::harness(fixture, profile);
        let chords = {
            let ctx = h.context();
            let pools = build_pools(&ctx).expect("pools");
            let paths = search_in(
                &ctx,
                &pools,
                &SearchConfig::default(),
                &CancelFlag::new(),
                &mut |_, _| {},
            )
            .expect("paths");
            paths[0].chords.clone()
        };
        (h, chords)
    }

    #[test]
    fn every_chord_gets_a_voicing_inside_the_range() {
        for profile in testing::PROFILE_IDS {
            let (h, events) = chords("melodies/eight_bar_c_major", profile);
            let vp = VoicingParams::default();
            let voicings = voice_progression(h.kb, &h.profile, &events, Some(&h.notes), &vp)
                .unwrap_or_else(|e| panic!("{profile}: {e}"));
            assert_eq!(voicings.len(), events.len());
            for v in &voicings {
                assert!(!v.pitches.is_empty());
                for p in &v.pitches {
                    assert!(p.is_valid_midi(), "{profile} wrote {}", p.to_ascii());
                    assert!(p.midi() >= vp.low && p.midi() <= vp.high);
                }
            }
        }
    }

    #[test]
    fn voicings_stay_under_a_preserved_melody() {
        let (h, events) = chords("melodies/eight_bar_c_major", "jazz_standard");
        let vp = VoicingParams {
            preserve_top: true,
            ..VoicingParams::default()
        };
        let voicings =
            voice_progression(h.kb, &h.profile, &events, Some(&h.notes), &vp).expect("v");
        for (chord, voicing) in events.iter().zip(&voicings) {
            let melody_low = h
                .notes
                .notes
                .iter()
                .filter(|n| n.onset < chord.end() && n.onset + n.duration > chord.onset)
                .map(|n| n.midi)
                .min();
            if let (Some(low), Some(top)) = (melody_low, voicing.top()) {
                assert!(
                    top.midi() < low,
                    "harmony {} reaches the melody at {}",
                    top.to_ascii(),
                    chord.onset.to_display()
                );
            }
        }
    }

    #[test]
    fn voice_assignment_is_deterministic() {
        let (h, events) = chords("melodies/eight_bar_c_major", "jazz_standard");
        let vp = VoicingParams::default();
        let a = voice_progression(h.kb, &h.profile, &events, Some(&h.notes), &vp).expect("a");
        let b = voice_progression(h.kb, &h.profile, &events, Some(&h.notes), &vp).expect("b");
        let ja: Vec<String> = a
            .iter()
            .map(|v| v.to_json().to_canonical_string())
            .collect();
        let jb: Vec<String> = b
            .iter()
            .map(|v| v.to_json().to_canonical_string())
            .collect();
        assert_eq!(ja, jb);
    }

    #[test]
    fn voices_are_numbered_bottom_up() {
        let (h, events) = chords("melodies/eight_bar_c_major", "common_practice");
        let voicings =
            voice_progression(h.kb, &h.profile, &events, None, &VoicingParams::default())
                .expect("v");
        for v in &voicings {
            assert_eq!(v.voices.len(), v.pitches.len());
            for (i, id) in v.voices.iter().enumerate() {
                assert_eq!(*id, VoiceId(i as u16));
            }
            for pair in v.pitches.windows(2) {
                assert!(pair[0].midi() < pair[1].midi(), "voicing is not ascending");
            }
        }
    }

    #[test]
    fn minimal_motion_is_preferred_across_time() {
        // Two chords a tone apart should be connected by small motion, not by
        // both landing in their own template's default register.
        let kb = h_kb();
        let prof = kb.resolve_profile("common_practice").expect("profile");
        let events = vec![
            event(0, "C", 0, 4),
            event(1, "F", 4, 4),
            event(2, "G", 8, 4),
            event(3, "C", 12, 4),
        ];
        let voicings =
            voice_progression(kb, &prof, &events, None, &VoicingParams::default()).expect("v");
        let mut total = 0;
        for pair in voicings.windows(2) {
            let n = pair[0].pitches.len().min(pair[1].pitches.len());
            for i in 0..n {
                total += (pair[1].pitches[i].midi() - pair[0].pitches[i].midi()).abs();
            }
        }
        let voices: usize = voicings
            .windows(2)
            .map(|p| p[0].pitches.len().min(p[1].pitches.len()))
            .sum();
        let per_voice = total as f64 / voices.max(1) as f64;
        assert!(
            per_voice <= 2.5,
            "average voice motion {per_voice:.2} semitones is not minimal (total {total})"
        );
    }

    #[test]
    fn chord_identity_survives_voicing() {
        let (h, events) = chords("melodies/eight_bar_c_major", "jazz_standard");
        let voicings =
            voice_progression(h.kb, &h.profile, &events, None, &VoicingParams::default())
                .expect("v");
        for (chord, voicing) in events.iter().zip(&voicings) {
            let guide = chord.spec.guide_tones();
            let sounding: Vec<i32> = voicing.pitches.iter().map(|p| p.pitch_class()).collect();
            for degree in guide {
                let pc = (chord.spec.root_pc() + degree.simple_semitones()).rem_euclid(12);
                assert!(
                    sounding.contains(&pc),
                    "voicing of {} dropped guide tone {}",
                    chord.spec.render_ascii(),
                    degree.to_string()
                );
            }
        }
    }

    #[test]
    fn range_is_enforced_even_when_narrow() {
        let kb = h_kb();
        let prof = kb.resolve_profile("common_practice").expect("profile");
        let events = vec![event(0, "Cmaj7", 0, 4), event(1, "G7", 4, 4)];
        let vp = VoicingParams {
            low: 55,
            high: 72,
            ..VoicingParams::default()
        };
        let voicings = voice_progression(kb, &prof, &events, None, &vp).expect("v");
        for v in &voicings {
            for p in &v.pitches {
                assert!((55..=72).contains(&p.midi()), "{} escaped", p.to_ascii());
            }
        }
    }

    #[test]
    fn bad_range_is_rejected() {
        let kb = h_kb();
        let prof = kb.resolve_profile("common_practice").expect("profile");
        let vp = VoicingParams {
            low: 90,
            high: 40,
            ..VoicingParams::default()
        };
        let e =
            voice_progression(kb, &prof, &[event(0, "C", 0, 4)], None, &vp).expect_err("bad range");
        assert_eq!(e.code, crate::error::INVALID_ARGUMENT);
    }

    #[test]
    fn empty_input_yields_empty_output() {
        let kb = h_kb();
        let prof = kb.resolve_profile("common_practice").expect("profile");
        assert!(
            voice_progression(kb, &prof, &[], None, &VoicingParams::default())
                .expect("ok")
                .is_empty()
        );
    }

    #[test]
    fn quartal_templates_only_fit_chords_that_have_a_fourth() {
        let kb = h_kb();
        let quartal = kb
            .voicing_templates()
            .iter()
            .find(|t| t.id == "quartal_three_voice")
            .expect("quartal template");
        assert!(!has_required(&symbol::parse("C").unwrap(), quartal));
        assert!(has_required(&symbol::parse("C7sus4").unwrap(), quartal));
    }

    #[test]
    fn degree_lookup_respects_spelling() {
        let minor = symbol::parse("Cm7").expect("Cm7");
        let (d, _) = find_tone(&minor, "3").expect("a third");
        assert_eq!(
            d.alter, -1,
            "an unaccidented 3 must find the chord's own third"
        );
        assert!(find_tone(&minor, "#11").is_none());
        let sharp = symbol::parse("C7#11").expect("C7#11");
        assert!(find_tone(&sharp, "#11").is_some());
    }

    fn h_kb() -> &'static KnowledgeBase {
        KnowledgeBase::embedded()
    }

    fn event(id: u32, sym: &str, onset: i64, dur: i64) -> ChordEvent {
        ChordEvent::new(
            id,
            symbol::parse(sym).expect("symbol"),
            BeatTime::from_quarters(onset),
            BeatTime::from_quarters(dur),
        )
    }
}
