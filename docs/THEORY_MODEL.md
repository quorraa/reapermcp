# Theory model

How the QLabs knowledge bundle represents music theory, and how the engine executes it.

This document is the contract between the knowledge data in `knowledge/` and the Rust code in
`crates/theory-kb`, `crates/harmony-engine`, `crates/arrangement-engine` and
`crates/loop-engine`. If the two disagree, the data is authoritative for *what* the rules say
and this document is authoritative for *how* they are evaluated.

---

## 1. Design premise: there is no single correct harmony

The engine does not implement one definition of correct. It implements a shared rule base plus
ten style profiles that decide which rules are active, how much each one counts, and what to
prefer when rules disagree. The same sonority can be a fault in one profile, a colour in
another and the whole point in a third:

- Parallel fifths: a major fault in `strict_counterpoint`, disabled in `modal_ambient`, the
  intended sound in `pop_rock` power-chord voicings.
- A tonic dominant-seventh chord: an unresolved dissonance in `common_practice`, a stable
  primary sonority in `blues`.
- A natural 11 a semitone above a major third: penalised in close register, bypassed entirely
  when the 11 is in the melody, the third is omitted, the voices are wide, the profile wants
  clusters, or the context is quartal or planing.

Every result therefore reports the active profile, the assumptions, the confidence, the rules
that fired, the exceptions that bypassed them, the score components and the source ids. A
style-dependent choice is never described as an absolute law.

---

## 2. Rule kinds

Eight kinds, ordered from binding to advisory. The kind determines *when* a rule is evaluated,
not merely how much it scores.

| Kind | Enforced | Meaning | Count |
| --- | --- | --- | --- |
| `mathematical_invariant` | before scoring | Arithmetic or representational facts: MIDI 0–127, positive duration, exact loop length. Violation is impossible output, not bad output. | 3 |
| `hard_integrity` | before scoring | Product and data guarantees: no source mutation, no notes outside their item, no overlaps in a monophonic part, chord identity survives voicing omissions. | 12 |
| `strong_theory_principle` | soft, high weight | Principles a competent musician in the declared profiles would not argue with: tendency-tone resolution, applied dominants needing their target, destination-sensitive diminished spelling. | 22 |
| `theory_default` | soft | Standard practice with known exceptions, all listed in the rule. The largest group. | 46 |
| `style_sensitive_preference` | soft, profile-scoped | Rules that are genuinely contradicted by other profiles. Every one of these is listed in the disputed-rules table in `docs/RESEARCH_AND_PROVENANCE.md`. | 28 |
| `arrangement_heuristic` | soft | Register, density, doubling and masking guidance. Sourced, but closer to craft than to theory. | 16 |
| `loop_integrity` | soft, high weight | Loop-boundary correctness: hanging notes, pickups, bass and pedal continuity across the wrap. | 8 |
| `implementation_heuristic` | soft, low weight | Honest engineering judgement. **These carry an empty `source_refs` array by contract** — they are not given a citation they do not deserve. | 12 |

Of 147 rules, **135 are source-backed and 12 are labelled heuristics.** The validator enforces
the biconditional: non-empty `source_refs` if and only if the kind is not
`implementation_heuristic`.

### Hard versus soft constraint handling

```
candidate generation
        │
        ▼
[1] hard filter        mathematical_invariant + hard_integrity
        │              violation  ->  candidate REJECTED, never scored
        ▼
[2] soft scoring       every other kind
        │              each firing rule adds effect.score_delta to ONE named
        │              score component, after the profile multiplier
        ▼
[3] weighted total     profile score_weights, normalised by the engine
```

Rules with `score_delta <= -1000` mark the hard tier in data as well as in kind, so a loader
that only reads numbers still refuses them. Soft rules never silently reject a candidate; they
push it down the ranking and say why. That is what keeps unconventional but valid material
reachable.

### Rule evaluation order for one candidate

1. Match `trigger.event` against the engine event being processed.
2. Match the optional trigger selectors (`chord_family`, `contains_degrees`, `role`, …).
3. Check the profile list; if the active profile (or an ancestor, see §4) is absent, status is
   `not_applicable`.
4. Apply the profile's `rule_overrides`. `"disabled"` short-circuits to `not_applicable`.
5. Evaluate `exceptions`. Any one holding gives status `bypassed`, delta 0, and the matched
   exception names are recorded.
6. Evaluate `conditions`. All must hold, otherwise `not_applicable`.
7. Apply: status `applied` (or `violated` for a negative delta on a hard rule), delta is
   `effect.score_delta * override_multiplier`, added to `effect.score_component`.

Steps 5 and 6 are deliberately in that order: an exception bypasses the rule even when the
conditions would have matched, and the trace shows the bypass rather than silence.

---

## 3. The predicate vocabulary

`conditions` and `exceptions` are drawn from a **closed** list of predicate identifiers. The
loader rejects any predicate the engine cannot evaluate, which makes an unimplemented predicate
a startup failure rather than a silently-never-firing rule.

Predicates take no arguments. Anything parametric is either encoded in the name
(`melody_leap_exceeds_fifth`) or read from profile and instrument data at evaluation time
(`spacing_violates_instrument_low_interval_limit` consults the active instrument profile
rather than a constant). This keeps the evaluator a simple dispatch over a `&str` and keeps the
data readable.

#### Chord content

*Inspects: the realised `Voicing` and its `ChordSpec`.*

| Predicate id | Meaning | Rules using it |
| --- | --- | --- |
| `third_is_omitted` | The chord's third is absent from the realised voicing. | 2 |
| `fifth_is_omitted` | The chord's fifth is absent from the realised voicing. | 2 |
| `root_is_omitted` | The chord's root is absent from the realised voicing. | 1 |
| `third_is_suspended` | The third is replaced by a fourth or a second (sus identity). | 4 |
| `third_is_not_suspended` | The chord retains a real third — it is not a sus sonority. Exact logical negation of `third_is_suspended`. | 1 |
| `seventh_is_present` | A chordal seventh is sounding. | 9 |
| `chord_has_altered_tones` | At least one altered degree is sounding. | 7 |
| `chord_is_rootless_voicing` | The voicing family is rootless. | 1 |
| `chord_is_symmetric_collection` | The chord divides the octave evenly (dim7, aug, whole-tone). | 5 |
| `guide_tones_present` | Both the third (or its substitute) and the seventh are sounding. | 2 |
| `bass_supplies_root` | Another part is sounding the chord root in the bass register. | 1 |
| `doubling_is_on_tendency_tone` | A doubled voice lands on a leading tone, chordal seventh or altered degree. | 2 |

#### Register and spacing

*Inspects: sounding MIDI pitches plus the active instrument profile.*

| Predicate id | Meaning | Rules using it |
| --- | --- | --- |
| `third_and_eleventh_are_in_adjacent_or_close_registers` | The natural eleventh is within an octave-plus-a-second above the third. | 1 |
| `voices_are_widely_spaced` | Every adjacent voice pair is at least a fifth apart. | 6 |
| `spacing_violates_instrument_low_interval_limit` | An adjacent voice pair is closer than the active instrument profile's limit for that register. | 1 |
| `register_is_low` | The sounding pitches sit below MIDI 48. | 3 |
| `register_is_high` | The sounding pitches sit above MIDI 79. | 1 |
| `voice_crossing_present` | A voice moves above or below a neighbouring voice's previous pitch. | 1 |
| `voice_overlap_present` | A voice moves past the previous pitch of an adjacent voice. | 1 |
| `outer_voice_span_exceeds_two_octaves` | Soprano and bass are more than 24 semitones apart. | 2 |
| `voice_exceeds_instrument_range` | A pitch falls outside the part's instrument profile range. | 3 |

#### Melody

*Inspects: the melody `NoteSet`, its phrase segmentation and its salience scores.*

| Predicate id | Meaning | Rules using it |
| --- | --- | --- |
| `melody_is_11` | The melody note is the eleventh of the sounding chord. | 2 |
| `melody_is_chord_tone` | The melody note is a member of the sounding chord. | 2 |
| `melody_is_extension` | The melody note is a 9, 11 or 13 of the sounding chord. | 3 |
| `melody_is_altered_tone` | The melody note is an altered degree of the sounding chord. | 2 |
| `melody_note_is_structural` | Structural salience exceeds the analysis threshold. | 4 |
| `melody_note_is_on_strong_beat` | Metric weight of the onset is at or above 0.75. | 2 |
| `melody_note_is_long` | Duration is at least twice the prevailing note value. | 4 |
| `melody_leap_exceeds_fifth` | The interval from the previous melody note is larger than P5. | 3 |
| `leap_is_followed_by_step_in_opposite_direction` | The leap is recovered by contrary step. | 1 |
| `melody_is_at_phrase_end` | The note is the final note of a phrase. | 1 |
| `melody_is_pickup` | The note precedes the first downbeat of its phrase. | 3 |
| `melody_outlines_augmented_or_tritone_interval` | Two consecutive melody notes form an augmented interval or a tritone. | 2 |
| `melody_is_repeated_pitch` | The melody note repeats the previous pitch. | 1 |
| `note_returns_to_previous_pitch` | The following note returns to the pitch that preceded this one. | 1 |
| `melody_reaches_registral_peak` | The note is the highest pitch of its phrase. | 3 |
| `melody_material_would_be_altered` | A candidate would change a melody pitch or onset that the caller asked to preserve. | 1 |

#### Motion and voice leading

*Inspects: a `VoiceLeadingConnection` pair between two adjacent voicings.*

| Predicate id | Meaning | Rules using it |
| --- | --- | --- |
| `motion_is_parallel` | Both voices move in the same direction by the same interval. | 5 |
| `motion_is_similar` | Both voices move in the same direction by different intervals. | 2 |
| `motion_is_contrary` | The two voices move in opposite directions. | 5 |
| `motion_is_oblique` | One voice holds while the other moves. | 1 |
| `interval_is_perfect_fifth_or_octave` | The arrival interval is P5, P8 or a compound of them. | 6 |
| `outer_voices_involved` | The voice pair being examined is the soprano and the bass. | 2 |
| `common_tone_available` | The two chords share at least one pitch class. | 4 |
| `common_tone_retained` | A shared pitch class is held in the same voice. | 3 |
| `stepwise_connection_available` | Every voice could move by step or hold. | 3 |
| `leading_tone_resolves_up_by_step` | The leading tone ascends a half step to the tonic. | 1 |
| `chordal_seventh_resolves_down_by_step` | The chordal seventh descends by step. | 2 |
| `altered_tone_resolves_by_step` | Each altered degree moves by step into the next chord. | 2 |
| `tendency_tone_is_unresolved` | A leading tone, seventh or alteration does not resolve. | 2 |
| `voice_leap_is_large` | A single voice moves by more than an octave. | 2 |
| `total_motion_is_minimal` | Total semitone travel is at or below the best available path. | 1 |

#### Dissonance treatment

*Inspects: the note plus its immediate neighbours and the metric grid.*

| Predicate id | Meaning | Rules using it |
| --- | --- | --- |
| `dissonance_is_prepared` | The dissonant tone is present as a consonance in the same voice immediately before. | 6 |
| `dissonance_is_on_strong_beat` | The dissonance falls on a metrically strong position. | 8 |
| `suspension_resolves_down_by_step` | The suspended tone descends by step. | 5 |
| `note_is_approached_by_step` | The previous note is a step away. | 4 |
| `note_is_left_by_step` | The following note is a step away. | 6 |
| `note_is_on_weak_beat` | Metric weight of the onset is below 0.5. | 5 |
| `note_is_chromatic_to_active_scale` | The pitch class is outside the active scale. | 4 |

#### Function and progression

*Inspects: the `ChordEvent`, its neighbours and the active `KeyRegion`.*

| Predicate id | Meaning | Rules using it |
| --- | --- | --- |
| `function_is_tonic` | The chord's function class is tonic. | 2 |
| `function_is_predominant` | The chord's function class is predominant. | 4 |
| `function_is_dominant` | The chord's function class is dominant. | 5 |
| `chord_is_diatonic_to_key` | Every chord tone belongs to the active key collection. | 1 |
| `chord_is_borrowed_from_parallel_mode` | The chord is diatonic to the parallel mode. | 6 |
| `chord_is_applied_dominant` | The chord is a dominant of a degree other than the tonic. | 4 |
| `chord_is_tritone_substitute` | The chord is a dominant a tritone from the expected dominant. | 3 |
| `target_chord_follows` | The chord the trigger expects as a destination actually follows. | 10 |
| `root_motion_is_descending_fifth` | The next root is a fifth below or a fourth above. | 1 |
| `root_motion_is_stepwise` | The next root is a second away. | 4 |
| `root_motion_is_third_related` | The next root is a third away. | 1 |
| `cadence_is_expected_at_this_slot` | Phrase analysis marks this slot as a cadential arrival. | 8 |
| `cadential_six_four_present` | A tonic-shaped chord sounds over the dominant scale degree. | 2 |
| `chord_is_in_root_position` | The bass is the chord root. | 4 |

#### Context and user intent

*Inspects: the request parameters and the analysis result.*

| Predicate id | Meaning | Rules using it |
| --- | --- | --- |
| `key_is_minor` | The active key region is a minor or minor-family mode. | 3 |
| `modal_center_is_active` | Analysis chose a modal centre rather than a major/minor key. | 26 |
| `pedal_point_is_active` | A sustained pedal is sounding under the harmony. | 8 |
| `harmonic_rhythm_is_fast` | Chords change more than once per bar. | 3 |
| `harmonic_rhythm_is_slow` | Chords change less than once per bar. | 1 |
| `cluster_intent_is_false` | The request did not ask for cluster voicings. | 1 |
| `intentional_cluster` | The request or profile explicitly asks for clusters. | 18 |
| `quartal_or_planing_context` | The voicing family is quartal or quintal, or the schema is a planing schema. | 28 |
| `complexity_target_is_high` | The complexity control is at or above 0.66. | 1 |
| `chromaticism_target_is_high` | The chromaticism control is at or above 0.66. | 11 |
| `extension_density_is_high` | The extension-density control is at or above 0.66. | 1 |
| `strictness_is_common_practice` | The request strictness is common_practice_strict. | 6 |

#### Arrangement

*Inspects: the `Part` set, the arrangement pattern and the instrument profile.*

| Predicate id | Meaning | Rules using it |
| --- | --- | --- |
| `role_is_bass` | The part's arrangement role is bass. | 5 |
| `role_is_lead` | The part's arrangement role is lead. | 3 |
| `role_is_pad_or_sustained` | The part's role is pad, texture or ambience. | 6 |
| `part_density_exceeds_role_target` | Onsets per bar exceed the pattern's density budget. | 2 |
| `onsets_collide_with_higher_priority_role` | More than half of this part's onsets coincide with a foreground part's onsets. | 2 |
| `register_overlaps_another_part` | The part's tessitura overlaps another part's by more than a fourth. | 4 |
| `part_duplicates_melody_rhythm` | The part's onset pattern matches the melody's. | 1 |
| `phrase_contains_no_rest` | The part sounds continuously for a whole phrase. | 2 |
| `polyphony_exceeds_profile_limit` | Simultaneous notes exceed the instrument profile's polyphony. | 1 |
| `doubling_is_allowed_for_role` | The arrangement pattern permits this doubling interval. | 7 |
| `essential_chord_tone_only_in_this_layer` | Removing this layer would delete the last sounding third or seventh of a chord. | 2 |
| `contrast_is_dynamics_only` | Two adjacent sections differ in velocity but not in register, density, texture or harmony. | 1 |

#### Loop

*Inspects: the loop bounds and the first/last slots of the candidate.*

| Predicate id | Meaning | Rules using it |
| --- | --- | --- |
| `is_loop_wrap_boundary` | The evaluated pair spans the loop end and loop start. | 3 |
| `final_chord_is_dominant_function` | The last chord of the loop is dominant. | 4 |
| `final_chord_is_tonic_function` | The last chord of the loop is tonic. | 2 |
| `first_chord_is_tonic_function` | The first chord of the loop is tonic. | 1 |
| `note_extends_past_loop_end` | A generated note sounds beyond the loop end point. | 1 |
| `note_carry_policy_is_explicit` | The request set an explicit carry or split policy. | 3 |
| `bass_leaps_across_wrap` | The bass interval across the wrap is larger than a fifth. | 1 |
| `pickup_is_present` | Material sounds before the first downbeat of the loop. | 1 |
| `pedal_continues_across_wrap` | A pedal tone is sounding on both sides of the wrap. | 1 |
| `harmonic_rhythm_changes_at_wrap` | The slot length before the wrap differs from the one after it. | 1 |
| `loop_length_is_not_exact` | The generated span differs from the requested loop length. Exact logical negation of `loop_length_is_exact`; the length invariant needs the negative form so it fires on drift rather than on correctness. | 1 |

#### Integrity

*Inspects: the generated `Note`s and the `EditPlan`.*

| Predicate id | Meaning | Rules using it |
| --- | --- | --- |
| `midi_pitch_out_of_range` | A pitch is outside 0..127. | 1 |
| `duration_is_not_positive` | A note duration is zero or negative. | 1 |
| `notes_overlap_in_monophonic_part` | Two notes sound at once in a part declared monophonic. | 1 |
| `note_outside_item_bounds` | A note falls outside the generated media item. | 1 |
| `source_material_would_be_mutated` | The plan would modify the user's source item or take. | 1 |
| `chord_symbol_is_ambiguous` | More than one parse of the symbol has equal precedence. | 1 |
| `omission_changes_semantic_identity` | A voicing omission would alter the parsed chord identity rather than only the played notes. | 1 |

#### Search and diversity

*Inspects: the set of already-selected candidates.*

| Predicate id | Meaning | Rules using it |
| --- | --- | --- |
| `candidate_duplicates_existing_strategy` | A candidate matches an already-selected candidate on root motion, functional path, modal source and bass contour. | 1 |

**Total: 116 predicates, all of them used by at least one rule (395 total references).**

This count is enforced, not asserted: `RuleEngine::known_predicates()` must equal the set
of predicates the rule data actually uses, in both directions, and a predicate appearing in
`knowledge/` that the engine does not implement is a validation failure rather than a silent
no-op. See `crates/theory-kb/tests/embedded_parity.rs`.

### Trigger events

The trigger's `event` selects the point in the pipeline at which a rule is considered. Fourteen
events, all closed:

| Event | Fired when |
| --- | --- |
| `symbol_parsed` | A chord symbol has been parsed into a `ChordSpec`. |
| `note_emitted` | A single generated note is about to be written into an edit plan. |
| `melody_note` | One melody note is being analysed. |
| `melody_phrase` | A complete melodic phrase has been segmented. |
| `nct_hypothesis` | A non-chord-tone classification is being scored. |
| `harmonic_grid` | The harmonic-rhythm grid is being chosen. |
| `chord_selected` | A single chord candidate is being scored in one slot. |
| `chord_pair` | Two adjacent chords are being scored together. |
| `progression_path` | A complete candidate progression is being scored. |
| `voicing_built` | A realised voicing for one chord is being scored. |
| `voice_pair_motion` | Two voices moving between two chords are being scored. |
| `part_generated` | A complete generated part is being scored. |
| `arrangement_plan` | The whole multi-part plan is being scored. |
| `loop_boundary` | The wrap point of a loop is being audited. |

Optional trigger selectors narrow the match further: `chord_family`, `contains_degrees`,
`function_class`, `role`, `scale_family`, `voicing_family`, `cadence_kind`, `loop_intent`,
`note_role`, `motion`, `interval`, `progression_tag`. Their permitted values are fixed by
`schemas/theory-rule.schema.json`.

---

## 4. Profiles: inheritance and override semantics

Ten profiles, four inheritance roots:

```
common_practice
├── strict_counterpoint
├── jazz_standard
│   └── neo_soul_rnb
└── cinematic

pop_rock
└── electronic_loop
    └── drum_and_bass

blues            (root, no parent)
modal_ambient    (root, no parent)
```

`blues` and `modal_ambient` are roots on purpose. Making them children of `common_practice`
and then cancelling half its rules would encode them as deviations from a norm, which is a
claim about music the bundle declines to make. `strict_counterpoint` *is* a child, because it
genuinely is common-practice grammar with the independence penalties turned up.

**Resolution order.** A profile is resolved by walking from the root down to the leaf and
merging, so the child always wins:

1. Start from the root ancestor's field values, `score_weights` and `rule_overrides`.
2. For each descendant in order, overwrite any scalar field the descendant declares, replace
   `score_weights` entries the descendant declares, and merge `rule_overrides` key by key.
3. The result is a fully materialised profile with no unresolved references.

The loader rejects cycles and unknown parents. In this bundle every profile declares every
field explicitly, so inheritance affects `rule_overrides` in practice more than scalars — that
is intentional, because a fully declared profile is readable on its own.

**Override values.** A `rule_overrides` entry is either a non-negative multiplier applied to
`effect.score_delta`, or the literal string `"disabled"`. A multiplier of `0` and `"disabled"`
differ in the trace: `0` reports the rule as `applied` with no effect, `"disabled"` reports it
as `not_applicable`. Prefer `"disabled"` when the rule is conceptually wrong for the style, and
a small multiplier when it is merely less important.

**Score weights.** Each profile carries a weight for all thirteen `SCORE_COMPONENTS`. Weights
are non-negative and need not sum to one; the engine normalises. A weight of `0` removes a
component from the total while leaving its raw value visible in the trace, which is how
`strict_counterpoint` can show an extension score it does not act on.

---

## 5. Chord semantics

A chord is never a bare pitch-class set. `ChordSpec` carries root, triad quality, seventh
quality, extensions, added tones, alterations, omissions, bass and an `alt` flag, and the
knowledge bundle backs it with three files:

- `chord_qualities.json` — 51 semantic qualities, each with its `degrees`, `extensions`,
  `added`, `alterations`, `omissible_degrees`, `implies_seventh` flag, family, typical function
  and typical chord scales.
- `chord_symbols.json` — 65 parser tokens mapping written suffixes to qualities, with ASCII and
  Unicode variants, canonical render forms, and an integer `precedence` so the longest and most
  specific spelling always wins. Every token string is unique across the table and every
  precedence value is distinct, so parsing is deterministic and a genuine tie is a data defect.
- `functions.json` — 45 functional entries covering diatonic major and minor, applied and
  secondary chords, mixture, the Neapolitan, all three augmented sixths, chromatic mediants,
  tritone substitution, the backdoor dominant, passing and common-tone diminished chords, the
  cadential six-four and pedal harmony.

Distinctions the data preserves, each pinned by a `hard_integrity` rule:

| Pair | Difference |
| --- | --- |
| `Cadd9` vs `C9` | `add9` has no seventh; `9` implies a flat seventh. |
| `C6` vs `C13` | A sixth is a chord member; a thirteenth implies a seventh. |
| `Csus4` vs `C11` | Sus **replaces** the third; `C11` has a third that a voicing may omit. |
| `CmMaj7` vs `Cm7` | The seventh rises rather than falls. |
| `C7b9` vs `Cadd b9` | One is a dominant alteration; the other is a colour over a triad. |
| `F#7` vs `Gb7` | Same sound, different spelling, different resolution. |
| Semantic identity vs played notes | A voicing omission never rewrites what the chord *is*. |

`alt` is a **family, not a collection**. `extensions.alt_expands_to_explicit_alternatives` is a
hard rule precisely because substituting one fixed altered scale throws away the choice the
notation deliberately left open. The engine must select and spell the specific alterations from
the melody, the destination and the voice-leading path.

### Scales and modes

`scales.json` holds 35 collections with `semitones`, `degree_spelling`, characteristic degrees,
parent, `mode_of` as `[parent_id, index]`, common chords, tension degrees and family. Every
modal entry is verified by the build validator to be an exact rotation of its declared parent.

`modes.json` holds 22 **character** records that reference scale ids and add what interval data
cannot express: brightness rank, characteristic degree, typical tonic chord, typical cadential
gesture and what the mode contrasts with. It deliberately duplicates no interval data.

Melodic minor's ascending/descending question is handled in `modes.json` rather than by two
scale records: analysis treats a descending natural 6 and 7 as natural-minor degrees rather
than a modulation, while generation over a minor-major seventh chord uses the fixed ascending
collection.

Tension degrees are **profile-sensitive guidance**, never exclusions —
`extensions.avoid_tone_is_profile_sensitive` is a soft rule with four exceptions, because the
natural fourth over a major chord is an avoid note in bebop and an essential colour in sus and
quartal writing.

---

## 6. Non-chord-tone model

Classification uses surrounding motion and metric position, **never pitch membership alone**.
Nine hypothesis rules in `rules/melody.json` cover passing, neighbour, suspension, retardation,
appoggiatura, escape tone, anticipation and chromatic approach; anything unmatched is a
decorative tone.

The discriminating features are exactly three:

| | Approached by | Left by | Metric position |
| --- | --- | --- | --- |
| Passing | step | step, same direction | weak |
| Neighbour | step | step, returns to origin | weak |
| Suspension | tie/repeat (prepared) | step down | strong |
| Retardation | tie/repeat (prepared) | step up | strong |
| Appoggiatura | leap | step | strong |
| Escape tone | step | leap, opposite direction | weak |
| Anticipation | any | repeats into next chord | weak |
| Chromatic approach | chromatic step | half step | weak |

A note may carry **several** hypotheses with confidence values;
`melody.ambiguous_tone_keeps_multiple_hypotheses` exists to prevent the engine from hiding a
genuine ambiguity behind a single label. Structural salience is a transparent weighted sum of
metric weight, duration, phrase position, leap-target status and registral extremity, and it is
exposed in the trace so a user can disagree with it.

Crucially, `melody.short_weak_note_does_not_force_chord_change` blocks the classic naive
harmoniser failure: the harmonic grid is chosen **before** chords are, and decorative notes are
explained as non-chord tones rather than given their own chord.

---

## 7. Voice-leading model

A progression is optimised as a **connected path**, not by picking the best chord per slot
(`voice_leading.path_is_optimized_globally`). Voice identity is maintained across chords;
chords are never treated as unordered pitch sets.

Modelled, each by its own rule: common-tone retention, stepwise connection, total semitone
travel, maximum leap, voice crossing, voice overlap, register drift, contrary/oblique/similar/
parallel motion, parallel and direct perfect intervals, tendency-tone resolution, chordal-
seventh resolution, altered-tone resolution, doubling, spacing, low-register congestion, range
and inner-voice shape.

**Low interval limits are data, not law.** `instrument_profiles.json` gives each of the 14
part profiles a `low_interval_limits` list of `{below_midi, min_semitones}` pairs, and
`voicings.json` gives each of the 38 templates a
`min_spacing_semitones_by_register` table. A sine sub, a string section and a distorted guitar
tolerate completely different intervals at the same pitch, so there is deliberately no single
universal constant anywhere in the bundle.

Thirteen voicing families are supported — close, open, drop 2, drop 3, shell, rootless, spread,
quartal, quintal, cluster, power, upper structure and pedal — and the family participates in
rule evaluation: `voice_leading.planing_suppresses_parallel_penalty` and
`voice_leading.power_chord_parallels_are_idiomatic` zero the parallel rules rather than merely
reducing them, so a quartal or riff candidate is not quietly outscored by a non-parallel one.

Retention priority when voices are scarce: guide tones (3 and 7) first, then the root if no
bass supplies it, then extensions, then the fifth. An **altered** fifth is a colour tone and is
explicitly not omissible.

---

## 8. Arrangement model

`arrangement_patterns.json` holds 28 patterns, each declaring role, register, range, density,
rhythmic activity, harmonic responsibility, foreground/background priority, allowed doubling,
polyphony, articulation tendency, note-length tendency, section participation, energy
contribution and loop behaviour — plus a machine-usable `rhythm` block:

```json
"rhythm": {
  "bar_length_qn": "4",
  "grid_qn": "1/2",
  "onsets": ["0", "1/2", "3/2"],
  "sustain": "grid | to_next | full_slot | mixed",
  "velocity_curve": [96, 76, 88]
}
```

Positions are **rational strings in quarter notes**, parsed directly by `BeatTime`, so no
floating-point rounding enters the arrangement layer. `onsets` are offsets within one bar.

Planning happens at three levels: phrase (entrances, answers, fills, cadential support, rests),
section (density, register, layer count, texture, harmonic complexity, contrast) and whole-loop
(energy curve, maximum-density point, strategic silence, tonal return, open versus closed
ending).

Masking is treated as a register problem before a level problem, which is the only honest
position for a MIDI-only product with no timbre control:
`arrangement.roles_occupy_separate_registers`,
`arrangement.background_parts_avoid_lead_onsets`, `arrangement.density_matches_role_target` and
`arrangement.contrast_beyond_dynamics` together push contrast onto register, rhythm, density,
texture, harmony, articulation and silence rather than velocity.

---

## 9. Loop model

Loop awareness is present from analysis onward, not bolted on at the end. Six intents:

| Intent | Wrap is judged on |
| --- | --- |
| `closed_tonic` | Dominant-to-tonic motion across the wrap, i.e. the wrap is an authentic cadence. |
| `open_dominant` | Ending unresolved is **correct**; the unresolved-tendency penalty is suppressed. |
| `modal_drone` | Common tones and pedal continuity. Dominant motion is not required or preferred. |
| `seamless_color` | Shared pitch classes so the seam is inaudible; the listener should not be able to locate the loop point. |
| `transition_ready` | Ending on a chord that can move on rather than one that closes. |
| `one_shot_ending` | Excluded from wrap scoring entirely. |

`looping.modal_drone_does_not_require_dominant` exists specifically so the engine never forces
every loop to end on the tonic. Meanwhile
`looping.closed_tonic_prefers_dominant_to_tonic_wrap` gives V-to-I across the wrap a strong
bonus for the one intent where it belongs.

The wrap is scored as an **ordinary chord pair** (`looping.voice_leading_smooth_across_wrap`),
which is what keeps `loop.audit` and the harmony engine in agreement. Loop-integrity rules also
cover hanging notes past the loop end, pickup handling, exact loop length (an exact rational
comparison, never a float epsilon), bass continuity, pedal continuity, harmonic rhythm stability
at the wrap, and layer removal that would delete the last sounding guide tone.

---

## 10. Decision traces

Every candidate carries a `DecisionTrace`. Each rule that was considered contributes a
`RuleApplication`:

```json
{
  "rule_id": "extensions.major_natural_11_close_register",
  "status": "bypassed",
  "score_delta": 0.0,
  "matched_conditions": ["third_and_eleventh_are_in_adjacent_or_close_registers"],
  "matched_exceptions": ["melody_is_11"],
  "source_ids": ["open-music-theory"],
  "explanation": "The natural 11 is the melody note, so the close-register penalty does not apply."
}
```

Four statuses: `applied`, `bypassed` (an exception matched), `not_applicable` (trigger, profile
or conditions did not match, or the profile disabled the rule) and `violated` (a hard rule
failed). Bypasses are reported rather than hidden — a rule that did not fire *because of a named
exception* is exactly the information a user needs.

Explanations are generated from the structured decision, never invented. `explanation_template`
fields on progression and cadence schemas provide the wording with `{key}` and `{slotN}`
placeholders filled from the actual candidate. An MCP client may rephrase an explanation, but
the server always supplies the factual trace, and large traces are exposed as
`candidate://{id}/trace` so tool responses stay small.

The raw score components are always retained alongside the weighted total. A single opaque
number is never the only thing returned.

---

## 11. How to add a new rule safely

1. **Decide the kind honestly.** If it is your engineering judgement, it is an
   `implementation_heuristic` and it gets an empty `source_refs`. Do not attach a citation to
   make it look authoritative. Forty source-backed rules and sixty honest heuristics beat one
   hundred with fabricated provenance.
2. **Choose the domain and file.** The rule id must be `<domain>.<snake_case_name>` and the
   domain must match the file name. The validator enforces both.
3. **Pick an existing trigger event.** If none fits, you are adding an engine hook, not a rule;
   do that first and document it in the events table above.
4. **Reuse predicates.** If you need a new one, add it to the vocabulary table in this
   document, implement it in the evaluator, and check whether an existing predicate already
   expresses it. The vocabulary is closed on purpose. Resist parameter-encoding sprawl.
5. **Write the exceptions before the conditions.** Ask where the rule is wrong. If you cannot
   name a context in which it does not apply, it is probably a `hard_integrity` rule — or it is
   overstated.
6. **Scope the profiles.** List only the profiles where the rule is genuinely active. A rule
   listed in all ten profiles and then disabled in four should have been listed in six.
7. **Choose one score component.** A rule contributes to exactly one. If it feels like two, it
   is two rules.
8. **Size the delta against its neighbours.** Roughly: `±1.5` minor, `±2.5` moderate, `±4`
   major, `-1000` hard rejection. Check what comparable rules in the same domain use.
9. **Set confidence by the bands in `docs/RESEARCH_AND_PROVENANCE.md` §5.** Low confidence
   means smaller delta, more exceptions and fewer profiles — not omission.
10. **Name at least one test id** in snake_case and write that test. Rules without behavioural
    tests drift.
11. **Add profile overrides where a profile disagrees**, and add the rule to the
    disputed-rules table in `docs/RESEARCH_AND_PROVENANCE.md` if it is style-sensitive.
12. **Set `version: 1`.** Increment it on any later change that alters the rule's meaning.
13. **Run the knowledge validator and regenerate the manifest.** Duplicate ids, unknown
    profiles, unknown sources, out-of-vocabulary predicates, non-snake_case test ids, profile
    cycles and mismatched counts are all caught there.
14. **Confirm nothing regressed.** Changing a shared rule's delta re-ranks every candidate in
    every profile that uses it; the golden tests exist to make that visible.
