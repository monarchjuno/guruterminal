---
name: valuation
description: Use when pricing an entity or range. Skip a simple quote or lookup.
---

<!--
Adapted from HKUDS/Vibe-Trading `valuation-model` (MIT).
https://github.com/HKUDS/Vibe-Trading
Trimmed for Guru Terminal: no target price, rating, or order.
-->

# Valuation

Refine `research`. Do not replace it. Skip a simple price lookup.

1. Fix the entity, share class, as-of date, currency, and value basis. Separate
   observed inputs from forecasts.
2. Choose a method that fits the economics: DCF or DDM for a going concern with
   usable cash or dividends; SOTP for distinct segments; relative multiples when
   peers share industry, scale, and stage. Use at least two methods when both
   are supported by Evidence created from delivered current-turn results.
3. Compute from explicit inputs with `finance_calculate` or `compute_run`. Any
   successfully delivered read result is usable; identify its provider,
   warnings, and definition limits. Do not withhold the range because the
   route is not official.
4. When WACC, growth, or terminal value dominate, report a sensitivity range,
   not a single precision figure.
5. Do not issue a target, rating, or order. A valuation is an anchor, not a
   live signal.

Lead with the range the evidence supports, the methods used, and the
assumptions that would move it.
