---
name: rule-test
description: Use when testing a stated historical rule, screen, or simple backtest. Skip a live quote or open-ended research.
---

<!--
Adapted from HKUDS/Vibe-Trading `factor-research` look-ahead rule and
`research-discipline` recency check (MIT).
https://github.com/HKUDS/Vibe-Trading
Not a trading-bot or repair playbook.
-->

# Historical rule test

Refine `research`. Do not replace it. This is hygiene for a rule the user
already stated, not a strategy generator.

1. Write the rule, universe, window, and rebalance before any return is
   calculated.
2. Use only information visible at each decision date. Forward returns must
   start after that date. Do not fill a statement or estimate into an earlier
   bar.
3. Compute the test with `compute_run` or `finance_calculate`, supplying the
   current-turn series explicitly. Any successfully delivered read result is
   usable; identify its provider, warnings, and coverage.
4. Disclose sample size, missing bars, costs or frictions if they would change
   the result, and any look-ahead you could not remove.
5. Do not present the result as a live signal, rating, or order.

Lead with whether the stated rule held in the tested window, then the limits.
