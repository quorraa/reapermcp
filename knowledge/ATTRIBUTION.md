# Attribution

Sources registered in `knowledge/sources.json`, what each one supports, and its verification
status. Every `license` value below is currently `unverified-reference-only` because this
bundle was authored with **no network access**; see `knowledge/LICENSE.md` and
`docs/RESEARCH_AND_PROVENANCE.md`.

Author lists are intentionally empty where authorship could not be confirmed offline. An empty
list is a statement of uncertainty, not a placeholder to be filled in with a guess.

## Registry

| Source id | Title | Publisher | Type | Usage | License status | What it backs here |
| --- | --- | --- | --- | --- | --- | --- |
| `mcp-spec-2025-11-25` | Model Context Protocol Specification, version 2025-11-25 | Model Context Protocol project | official_docs | technical-contract | unverified | Protocol version string, tool/resource/prompt surface, structured tool output. Cited by one rule about honouring caller-declared preservation constraints. |
| `mcp-rust-sdk` | Model Context Protocol Rust SDK (repository) | Model Context Protocol project | official_docs | technical-contract | unverified | Cross-check of protocol message shapes. No SDK code is vendored; the shipped server is std-only. |
| `reaper-reascript-api` | REAPER ReaScript documentation | Cockos Incorporated | official_docs | technical-contract | unverified | Which official API surface exists for the Lua bridge. |
| `reaper-reascript-help` | REAPER ReaScript API function reference | Cockos Incorporated | official_docs | technical-contract | unverified | MIDI pitch and duration bounds, item/take identity, undo semantics. Cited by the data-integrity and loop-integrity rules. |
| `open-music-theory` | Open Music Theory | VIVA / Pressbooks | open_textbook | paraphrased-rule-source | unverified | Harmony and function, non-chord tones, extensions and alterations, jazz voicings, chord-scale theory, ii-V-I behaviour, blues harmony, substitutions and planing. The single largest contributor to the harmony and extensions domains. |
| `mt21c` | Music Theory for the 21st-Century Classroom | University of Puget Sound | open_textbook | paraphrased-rule-source | unverified | Motives and phrases, melodic analysis, voice leading, counterpoint, texture and accompaniment patterns, formal contrast. The largest contributor to the melody, voice-leading and counterpoint domains. |
| `mt21c-mode-mixture` | Music Theory for the 21st-Century Classroom: Mode Mixture | University of Puget Sound | open_textbook | paraphrased-rule-source | unverified | Borrowed chords, parallel-mode mixture, minor plagal motion, Neapolitan and augmented-sixth behaviour. Registered separately because the project brief supplies its own canonical URL for it. |
| `ijcai-2024-constraint-composition` | IJCAI 2024 proceedings paper 0858 (constraint-based music composition) | International Joint Conferences on Artificial Intelligence | academic_paper | reference-only | unverified | Methodological background for the hard-constraints-then-soft-scoring search design. **Deliberately cited by no individual theory rule**, because the paper's exact title and authorship could not be confirmed offline. |
| `berklee-orchestration-2` | Orchestration 2: Writing Techniques for Full Orchestra (course page) | Berklee Online | course_material | reference-only | unverified | Bibliographic support for the arrangement domain: register separation, doubling, spacing and texture. A commercial course page, not an open text; no arrangement rule relies on it as its sole authority. |

## How citations appear in the product

Each theory rule carries `source_refs` as a list of `{source_id, locator}` pairs. When a rule
fires during generation, its `source_ids` are propagated into the candidate's `DecisionTrace`
and surfaced through `candidate.explain` and the `theory://sources` MCP resource, so a user can
always see which sources stand behind a decision.

Rules that are genuinely engineering judgement carry `kind: "implementation_heuristic"` and an
**empty** `source_refs` array. They are not dressed up with a citation they do not deserve. Of
the 147 rules in this bundle, 135 are source-backed and 12 are labelled heuristics.

## No text was copied

No sentence, table, figure, exercise or example from any source above appears in this bundle.
Every `summary`, `rationale`, `notes` and `explanation_template` field is original wording
written for machine consumption. Section locators are chapter- or topic-level titles, chosen
because they are stable and checkable; **no page numbers are recorded anywhere**, since page
numbers could not be verified and are edition-dependent.
