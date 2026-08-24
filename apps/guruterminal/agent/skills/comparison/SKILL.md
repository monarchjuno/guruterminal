---
name: comparison
description: Use when aligning two or more entities, periods, or peer multiples. Skip a single-name lookup.
---

<!--
Adapted from HKUDS/Vibe-Trading `valuation-model` relative-valuation notes,
`financial-statement` GAAP/IFRS alignment, and `deep-company-series` peer
fairness checks (MIT). https://github.com/HKUDS/Vibe-Trading
-->

# Comparison

Refine `research`. Do not replace it.

1. Align period, currency, accounting basis (GAAP, IFRS, or local; reported vs
   adjusted), consolidation, and share-count or per-share basis before ranking.
2. Peers must share industry, scale, and stage. Do not apply a leader multiple
   to a smaller or earlier-stage name, and do not compare a stripped core
   multiple with an unstripped peer multiple.
3. Keep an unaligned or missing observation as a gap. Never coerce it to zero.
4. When two enabled sources differ by more than a rounding tolerance, state
   both, name the definition gap, and say which figure the answer uses.
5. Compute aligned figures from explicit inputs with `finance_calculate` or
   `compute_run`. Any successfully delivered read result is usable; identify
   its provider, warnings, and definition limits.

Lead with the aligned comparison, then name leftover gaps.
