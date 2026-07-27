# Research and provenance

> ## ⚠ This build had no network access
>
> The knowledge bundle in `knowledge/` was authored in an environment where outbound network
> requests were unavailable. **No source page, license file, table of contents or PDF was
> fetched, opened or read during authoring.** Consequently:
>
> - Every source in `knowledge/sources.json` carries the license value
>   `unverified-reference-only`.
> - Section locators are chapter- or **topic-level titles**, not verified table-of-contents
>   entries and **never page numbers**.
> - Author lists are **empty** wherever authorship could not be confirmed. They were not
>   guessed.
> - One source (`ijcai-2024-constraint-composition`) is registered by URL alone, because its
>   title and authors could not be resolved. It is cited by **no** individual rule.
>
> **These license values and locators must be confirmed before this bundle is redistributed.**
> Until then, treat the bundle as safe to *use* (it contains no upstream text) and unsafe to
> *relicense on behalf of upstream*.

---

## 1. Source-selection policy

Sources were chosen in the priority order set by the project brief:

1. Official protocol and host API documentation, for technical contracts only.
2. Openly licensed university-level music-theory texts, for the bulk of the encoded theory.
3. Peer-reviewed research, for methodology.
4. Reputable course material, for areas the open texts under-serve (orchestration).

Deliberate exclusions:

- **Wikipedia is not used as a theory authority anywhere**, per the brief.
- No source was added merely to lengthen the bibliography. Nine sources back 147 rules; three
  of those nine do the great majority of the theoretical work.
- No source was added whose license status would have forced a share-alike claim over the
  encoded data, because none of its expression is reproduced in any case.

The registry is deliberately small. A large bibliography in which most entries are cited once
is a worse provenance signal than a small one in which every entry is genuinely load-bearing.

## 2. Source registry summary

| Source id | Role | Rules citing it |
| --- | --- | --- |
| `open-music-theory` | Primary theory source for harmony, extensions, chord-scale relationships, jazz and blues practice | large |
| `mt21c` | Primary theory source for melody, phrase structure, voice leading, counterpoint, texture, contrast | large |
| `mt21c-mode-mixture` | Mixture, borrowed chords, Neapolitan, augmented sixths | small, focused |
| `reaper-reascript-help` | MIDI and item integrity bounds | data-integrity and loop-integrity rules |
| `reaper-reascript-api` | API surface for the Lua bridge | none directly |
| `mcp-spec-2025-11-25` | Protocol contract; caller-declared constraints | one rule |
| `mcp-rust-sdk` | Protocol shape cross-check | none |
| `ijcai-2024-constraint-composition` | Search-design methodology | **none, deliberately** |
| `berklee-orchestration-2` | Orchestration background: register, doubling, spacing | arrangement rules, always alongside `mt21c` |

Full details, including per-source relevant sections and verification notes, are in
`knowledge/sources.json` and `knowledge/ATTRIBUTION.md`.

## 3. Licensing posture

Three separate license planes, deliberately not conflated:

| Plane | Covered by | Terms |
| --- | --- | --- |
| Software | repository-root `LICENSE` | permissive |
| Knowledge data | `knowledge/LICENSE.md` | CC BY 4.0, original expression |
| Upstream sources | each source's own terms | **all unverified in this build** |

The knowledge data can be licensed CC BY 4.0 because it is original expression encoding
publicly known musical facts and conventions. Facts and conventions are not copyrightable; the
wording, the structure, the predicate vocabulary, the score components, the profile weights and
the rule decompositions are ours.

Where a source license is uncertain — which, in this build, is everywhere — the policy is:

- Do not redistribute excerpts. *None are present.*
- Use the source as bibliographic support only.
- State explicitly that its expressive text was not copied. *Stated in three places.*
- Record the uncertainty. *Recorded in the `license` and `notes` fields of every record.*
- Prefer an openly licensed replacement for the encoded rule. *Applied: the two open textbooks
  carry nearly all the theory load; the proprietary and commercial sources carry only technical
  contracts and background.*

## 4. Paraphrasing policy

The rule is stronger than "do not plagiarise". It is: **write the record as if the source were
closed.**

Concretely, every `summary`, `rationale`, `notes`, `voice_leading_notes` and
`explanation_template` field in the bundle was composed as a piece of machine-consumable
engineering documentation, aimed at a reader who needs to know when a rule stops applying.
That framing has almost no overlap with how a textbook explains the same idea to a student, so
the wording diverges naturally rather than by careful avoidance.

Three additional constraints were applied:

1. **No sentence-level paraphrase.** A rule is a trigger, a predicate list, a numeric effect
   and an exception list. That decomposition does not exist in any source; it was designed here.
2. **No reproduced tables.** The interval table, degree spellings and chord-quality tables were
   generated from first principles in code (see the mode-rotation cross-check in the validator,
   which verifies every modal scale against a rotation of its parent) rather than transcribed.
3. **No page numbers, ever.** Locators are titles. This is both a verifiability measure and an
   anti-transcription measure: a title-level locator cannot function as a pointer to specific
   copied text.

## 5. How confidence values were assigned

`confidence` is **not** a probability that the rule is true. It is the authors' confidence
that the rule states its principle correctly *for the profiles it declares*. The bands used:

| Band | Meaning | Typical kinds |
| --- | --- | --- |
| 1.0 | Data or protocol invariant; not a musical claim at all | `mathematical_invariant`, `hard_integrity` |
| 0.90 – 0.95 | Uncontroversial within the declared profiles; a competent musician in that style would agree without argument | `strong_theory_principle` |
| 0.80 – 0.89 | Standard practice with well-known exceptions, all of which are listed in the rule's `exceptions` array | `theory_default` |
| 0.70 – 0.79 | Genuinely style-dependent; the same behaviour is correct elsewhere | `style_sensitive_preference` |
| 0.60 – 0.69 | Engineering judgement about how the engine should behave | `implementation_heuristic` |

A low confidence is never a reason to omit a rule. It is a reason to give the rule a smaller
score delta, more exceptions, and a narrower profile list.

## 6. Style-sensitive and disputed rules

These are the rules where the bundle deliberately refuses to state a universal law. Each is
listed with the profile that contradicts it, because that contradiction is the point.

| Rule | Says | Contradicted by |
| --- | --- | --- |
| `voice_leading.parallel_perfect_fifths` | Parallel fifths reduce voice independence | `pop_rock`, `blues`, `electronic_loop`, `drum_and_bass` (multiplier 0.2–0.25 or disabled); `modal_ambient` disables it entirely |
| `voice_leading.parallel_octaves` | Parallel octaves collapse two voices into one | Same profiles; also bypassed whenever an arrangement pattern declares octave doubling |
| `harmony.dominant_seventh_resolves_down_fifth` | A dominant seventh expects resolution | `blues` and `modal_ambient` disable it outright |
| `harmony.blues_dominant_seventh_is_stable_tonic` | A tonic seventh is stable and final | `common_practice` and `strict_counterpoint` disable it |
| `extensions.major_natural_11_close_register` | A natural 11 near a major third clashes | Five listed exceptions; `neo_soul_rnb` reduces it to 0.6 and it is inactive in modal and electronic profiles |
| `extensions.avoid_tone_is_profile_sensitive` | "Avoid notes" reduce fit | Explicitly soft; `neo_soul_rnb` drops it to 0.3 and four exceptions bypass it |
| `counterpoint.strong_beats_are_consonant` | Strong beats carry consonances | Active **only** in `strict_counterpoint`; disabled in `jazz_standard` and `pop_rock` |
| `counterpoint.dissonance_is_prepared` | Dissonance is prepared | Disabled in `pop_rock` and `modal_ambient`, reduced to 0.3 in `jazz_standard` |
| `harmony.retrogression_penalty` | V to IV weakens direction | Disabled in `blues`, `pop_rock` and `modal_ambient` |
| `harmony.root_position_at_cadence` | Cadences use root position | Disabled in `modal_ambient`, reduced to 0.4 in `neo_soul_rnb` |
| `looping.closed_tonic_prefers_dominant_to_tonic_wrap` | V-to-I across the wrap is ideal | Applies to one loop intent only; `modal_ambient` reduces it to 0.3 and prefers common-tone continuity |
| `melody.avoid_augmented_melodic_interval` | Augmented seconds are avoided | Bypassed whenever a modal centre is active — Phrygian dominant depends on the interval |
| `harmony.harmonic_rhythm_is_independent_of_tempo` | Fast tempo does not imply fast chord changes | Not disputed, but deliberately weighted 1.5–2.0 in the electronic profiles because the opposite assumption is a common generator bug |

Two structural decisions follow from this table:

- `blues`, `pop_rock` and `modal_ambient` are **root profiles with no parent**. Making them
  children of `common_practice` and then cancelling half its rules would misrepresent them as
  deviations from a norm. They are not.
- `strict_counterpoint` *is* a child of `common_practice`, because it genuinely is that
  grammar with the independence penalties turned up.

## 7. How source references surface in results

```
rule.source_refs[]              -> {source_id, locator}
        │
        ▼  rule fires during generation
RuleApplication.source_ids[]    -> ["open-music-theory", ...]
        │
        ▼  aggregated per candidate
DecisionTrace.source_ids[]
        │
        ├─▶ candidate.explain            (tool result)
        ├─▶ candidate://{id}/trace       (MCP resource)
        └─▶ theory://sources             (full registry, including license status)
```

`theory://sources` returns each record's `license` and `notes` verbatim, so a client that
surfaces provenance also surfaces the fact that the license is unverified. That is intentional:
the uncertainty travels with the citation instead of being lost at the first hop.

`theory.search` additionally returns matched rules with their `source_refs`, their `kind` and
their `confidence`, so a user asking "why" gets the rule's status alongside its content rather
than a bare assertion.

## 8. Verification checklist before redistribution

1. Fetch each URL in `knowledge/sources.json` and record the actual license.
2. Replace `unverified-reference-only` with the real identifier, or with
   `restricted-reference-only` where redistribution is prohibited.
3. Confirm each `relevant_sections[].locator` against the live table of contents; correct any
   title that has changed. Do **not** add page numbers.
4. Resolve the title and authors of `ijcai-2024-constraint-composition`, or remove the record.
5. Fill in `authors` for `open-music-theory` and `mt21c` from their title pages.
6. Update `accessed_at` to the verification date.
7. Update the unverified-source table in `knowledge/LICENSE.md` and the registry table in
   `knowledge/ATTRIBUTION.md`.
8. Re-run the knowledge validator and regenerate `manifest.json`'s `content_sha256`.
