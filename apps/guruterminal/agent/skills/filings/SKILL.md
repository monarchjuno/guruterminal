---
name: filings
description: Use when reading a filing, earnings release, or three-statement set. Skip a price-only question.
---

<!--
Adapted from HKUDS/Vibe-Trading `edgar-sec-filings` and `financial-statement`
read-order (MIT). https://github.com/HKUDS/Vibe-Trading
Signals, scores, and insider/13F trading rules omitted.
-->

# Filings and statements

Refine `research`. Do not replace it.

1. Identify the document or period: form or report type, entity, and as-of.
   Use `capability_search` and `capability_load` when the relevant OpenBB filing
   or statement Tool is not active. Prefer an enabled primary-source route when
   it covers the claim, including native OpenDART for Korean disclosures;
   otherwise use an enabled vendor statement and identify its provider.
2. Read for facts the answer needs: revenue and margin trend, cash vs accrual
   earnings, leverage and liquidity, and material MD&A or risk-factor changes
   versus the prior comparable filing.
3. When three statements are present, check identity: net income versus
   retained earnings, cash bridge versus the cash-flow sum, and a large gap
   between net income and operating cash. Banks and insurers need their own
   layout; do not force a non-financial template.
4. Fetch the user-named URL or the exact filing the Tool returned. A missing
   text layer is a limit, not empty content.
5. Do not turn the read into a score, rating, or order.

Lead with the sourced facts and the gaps that remain.
