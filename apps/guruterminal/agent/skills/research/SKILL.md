---
name: research
description: Use when the answer depends on financial facts, valuation, comparison, a thesis challenge, a market move, or a simple historical rule test.
---

# Research

Use when the request depends on financial facts or an analytical workflow. Keep
Memory controls as the user set them. Do not turn a simple lookup into a full
report.

## Core loop

1. Identify the entity or market, relevant date, units or currency, and the
   question the answer should resolve. Ask only when missing information would
   materially change the result.
2. Collect current-turn results for the claims most likely to change the
   answer. Use `capability_search` when the available route is unclear and
   `capability_load` before calling a relevant unloaded component. Prefer a
   capability that directly covers the fact and use its discovery operation
   when an identifier must be resolved. If an official or primary-source route
   is enabled and covers the claim, prefer it; otherwise use the enabled
   community or vendor route. Provider identity affects attribution and source
   quality, not whether a successfully delivered read result is usable. Use the
   web when original documents, news, policy, or narrative are the direct
   source.
   Fetch a user-named public URL directly. If document extraction reports no
   text layer, tell the user instead of inventing the content.
3. When Use memory is on, choose retrieval order and breadth from the request.
   `learned_index` and `memory_search` discover candidates; exact-read a record
   before relying on it. Retrieve semantically relevant learned state even when
   the user's literal terms differ, and follow only authored relationships
   likely to change the answer. Explicit recall or continuation may start from
   Memory; new factual claims still require current evidence. Current evidence
   wins conflicts with dated Memory.
4. Separate sourced facts, calculations, assumptions, and unresolved gaps.
   Rely only on successfully delivered current-turn result data for new claims;
   capability discovery and prior-turn Tool output are context only.
   Supply explicit inputs to `finance_calculate` for deterministic valuation
   and risk math. Use `compute_run` only for custom analysis; prefer javascript
   unless a Python package is required, and list every package on the first
   Python call.
5. When source-grounded research informs the final answer, use
   `evidence_create` for the claims actually used, selecting exact values from
   delivered `result_ref` payloads with JSON Pointer. For a requested chart,
   use `chart_publish` with explicit result row and column pointers, or explicit
   inline rows and columns plus upstream result references. Call
   `decision_submit` only when the user asked for an explicit judgment.
6. With Update memory on, propose a Wiki or Lens change only when the turn
   produced reusable, source-grounded learning. Discover semantic matches and
   improve them rather than duplicate them. Treat a same-kind active title or
   alias collision as the existing page, exact-read its canonical ID, and
   revise it. Wiki holds durable facts; Lens holds scoped, falsifiable
   judgment. If nothing survives source, scope, and counterexample checks, make
   no Memory change.

## Common analyses

- Valuation: fix the date, entity, share class, currency, and value basis.
  Separate historical inputs from forecasts, use a method suited to the
  economics, and report a range when assumptions dominate precision.
- Comparison: align periods, currencies, definitions, and share-count basis.
  Keep an unaligned or unavailable observation visible as a gap, never zero.
- Move attribution: confirm the move and its window, test plausible causes
  against current evidence, and keep unexplained movement explicit.
- Thesis challenge: seek the strongest disconfirming evidence and translate the
  material risks into observable invalidation conditions.
- Historical rule tests: define the rule before calculating, avoid look-ahead,
  disclose coverage and costs, and do not present a backtest as a live signal.

## Deliver

Lead with the answer the Evidence supports. State important providers, sources,
warnings, assumptions, gaps, and what would change the conclusion. Name reused
Wiki or Lens titles and any durable learning proposed for later work.
