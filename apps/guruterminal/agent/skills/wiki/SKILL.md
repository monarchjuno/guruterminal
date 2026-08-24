---
name: wiki
description: Use when the user selects this Skill or asks to organize, update, or retain stable descriptive facts in Wiki.
---

# Wiki

Wiki is the Guru's reusable factual model. It stores durable facts a later turn
should start from, not a thesis, a filing paste, or a transient quarterly fact.
Use memory and Update memory are on for this turn.

## Workflow

1. Use `learned_index` and `memory_search` as needed to discover the durable
   concept. Exact-read a semantic match before changing it; discovery excerpts
   are not record authority. Treat a same-kind active title or alias collision
   as the same page: exact-read its canonical ID and revise it instead of
   creating a second record.
2. Use `evidence_create` for every factual change, selecting exact values from
   delivered current-turn `result_ref` payloads with JSON Pointer.
3. Propose a complete record with `memory_patch_propose`, a rationale, and the
   staged Evidence IDs or exact-read Memory `source_ids` it uses. Frontmatter
   must include non-empty `id`, `title`, `summary`, and `as_of` in RFC3339
   with seconds and timezone (for example `2026-08-24T00:00:00Z`). Improve a
   semantic match instead of duplicating it.
4. Keep the page focused on one durable concept. Add `entities` and `see_also`
   when those relationships help later relevant work find and apply the page.

To retire a page, set `status: revoked` and `revoked_by` to the contradicting
record. Do not delete a page merely to hide a superseded claim.

## Deliver

Return the organized facts. Name the Wiki records proposed, why the changes are
durable, and where the Guru should reuse them. If no durable change is
justified, make none.
