# License for the QLabs knowledge data

This file covers **only** the contents of the `knowledge/` directory. The software in this
repository is licensed separately; see the repository-root `LICENSE`.

## What this data is

Every record in `knowledge/` — scale and mode definitions, interval tables, chord qualities,
the chord-symbol alias table, functional tables, voicing templates, progression and cadence
schemas, arrangement patterns, instrument profiles, theory rules and style profiles — is
**original expression authored for this project**. Summaries, rationales, notes, explanation
templates and voice-leading commentary were written from scratch for machine consumption.

## What this data is not

This data is **not a redistribution of any source text.** No passage from any source listed in
`knowledge/sources.json` has been copied, excerpted, transcribed or paraphrased at the
sentence level. Musical facts (that a major scale is 2-2-1-2-2-2-1, that a dominant seventh
contains a tritone between its third and seventh) are not copyrightable; the wording used to
encode and explain them here is ours.

## License grant

The knowledge data in `knowledge/` is released under the
**Creative Commons Attribution 4.0 International license (CC BY 4.0)**:

<https://creativecommons.org/licenses/by/4.0/>

You may share and adapt it, including commercially, provided you give appropriate credit,
link to the license, and indicate whether changes were made. Suggested attribution:

> QLabs REAPER Music Intelligence MCP knowledge bundle, knowledge_version 1.0.0, CC BY 4.0.

## Upstream sources retain their own licenses

The sources registered in `knowledge/sources.json` are cited as bibliographic support and, for
the protocol and host documentation, as technical contracts. They are **not** relicensed by
this file. Anyone redistributing this bundle must respect each upstream source's own terms
independently.

## Sources whose license could NOT be verified in this build

This bundle was authored in an environment with **no network access**. Every source therefore
carries the license value `unverified-reference-only`, which means exactly what it says: the
license could not be checked against the live page and must not be assumed.

The following sources are all currently marked `unverified-reference-only`:

| Source id | Why it is unverified |
| --- | --- |
| `mcp-spec-2025-11-25` | Specification site not reachable offline. |
| `mcp-rust-sdk` | Repository license file not reachable offline. |
| `reaper-reascript-api` | Proprietary vendor documentation; terms not reachable offline. |
| `reaper-reascript-help` | Proprietary vendor documentation; terms not reachable offline. |
| `open-music-theory` | Believed to be an openly licensed textbook, but the specific license version was not confirmed. |
| `mt21c` | Believed to be an openly licensed textbook, but the specific license version was not confirmed. |
| `mt21c-mode-mixture` | Same publication as `mt21c`; same uncertainty. |
| `ijcai-2024-constraint-composition` | Proceedings license not confirmed; the paper's title and authors were also not resolvable offline and are deliberately left blank rather than guessed. |
| `berklee-orchestration-2` | Commercial course landing page; assume all rights reserved. |

Because none of these licenses is confirmed, this bundle takes the conservative route on every
one of them: **no excerpts, no quotations, no reproduced tables, no page numbers.** Section
locators are chapter- or topic-level titles only.

**Before redistributing this bundle**, re-verify each license, update the `license` and `notes`
fields in `knowledge/sources.json`, and update this file and `knowledge/ATTRIBUTION.md`. If any
source turns out to require share-alike treatment, that requirement applies to material derived
from it — but note that no material here is derived from any source's *expression*, only from
publicly known musical facts and conventions, so a share-alike obligation is not expected to
attach. Verify rather than assume.

## Practical summary

- Code: repository-root `LICENSE`.
- Knowledge data: CC BY 4.0 (this file).
- Upstream sources: their own terms, none of which were verified offline.
- Nothing in `knowledge/` reproduces upstream expressive text.
