---
name: decision
description: Use when the user asks for an explicit investment judgment, recommendation, stance, or allocation choice rather than research alone.
---

# Decision

Use only for an explicit investment judgment. Do not activate merely because an
answer contains analysis, comparison, or risks.

## Retrieval

For a new judgment, build the current-evidence view and consult Memory that can
materially change it. For a recalled, revised, or continued judgment, begin by
exact-reading the prior Decision and refresh claims that require current
evidence. Use `learned_index` and `memory_search` adaptively, exact-read before
relying on a record, and retrieve semantically relevant learned state even when
surface terms differ. Follow only authored relationships material to the
choice; the host does not expand them automatically.

## Judgment contract

State the entity, cutoff, horizon, constraints, choice, calibrated confidence,
strongest counterevidence, risks, and measurable invalidation conditions.
Separate observed evidence, calculations, assumptions, and Memory. Abstain when
the available current-turn results cannot support a judgment. Create Evidence
for the decisive claims with `evidence_create`, writing a readable markdown
body and citing the delivered `result_ref` values actually used. A non-abstain
`decision_submit` accepts only Evidence IDs created in this turn. An abstain
submission may omit Evidence but
must preserve its reason and uncertainty. List only exact-read Wiki or Lens IDs
in `uses_ids`; a prior Decision is context, not fresh evidence.

With Update memory on, propose only source-grounded Wiki or Lens learning that
can improve a later relevant task. The Decision is experience; a future-usable
Wiki or Lens change is learning.

## Deliver

Lead with the stance and confidence. Give the decisive evidence, strongest
counterevidence, assumptions, risks, and invalidation conditions, then briefly
name any durable lesson proposed for later work.
