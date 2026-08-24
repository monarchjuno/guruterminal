---
name: lens
description: Use when the user selects this Skill or asks to review a Decision, update an interpretive lens, or record a falsifiable hypothesis.
---

# Lens

Lens is how this Guru invests: reusable interpretations, habits, limits, and
what would prove a view wrong. A single anecdote is not a Lens. Use memory and
Update memory are on for this turn. Standing investment philosophy belongs on
the reserved page `lens:charter`. Exact-read it first and create or revise that
page when the user is teaching how they invest.

## Decision review

When a past Decision is in scope, exact-read the original and only the linked
Wiki, Lens, or Evidence records material to the review. Gather the realized
outcome through delivered current-turn results and create Evidence for the
exact values used. Compare it with the original expectations and invalidation
conditions, and separate reasoning quality from luck. Never rewrite the
original Decision; any surviving future-usable lesson belongs in a Wiki or Lens
change.

## Research-driven Lens

When no Decision is in scope, discover the Lens that new result data would
change and exact-read it before patching. Create Evidence from the exact
current-turn values used. Update only what that Evidence justifies, keep the
lesson scoped and falsifiable, and prefer improving a semantic match to
creating a duplicate. A same-kind active title or alias collision is the same
Lens: exact-read its canonical ID and revise it.

## Memory contract

Propose a complete record with `memory_patch_propose`, a rationale, and exact
staged Evidence IDs or exact-read Memory `source_ids`. Frontmatter must include
non-empty `id`, `title`, `summary`, and `as_of` in RFC3339 with seconds and
timezone (for example `2026-08-24T00:00:00Z`). Each of these headings must be
present and non-empty: Scope, Assumptions, Counterexamples, Limits, and
Invalidation conditions.

## Deliver

Return what changed, why it is reusable, where it applies, and what would
invalidate it. If no durable lesson survives the evidence and scope checks,
make no change.
