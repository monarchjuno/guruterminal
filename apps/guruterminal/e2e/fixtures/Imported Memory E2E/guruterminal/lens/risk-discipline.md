---
id: lens:import/risk-discipline
title: Imported Risk Discipline
summary: A downside-first review rule for the Native Memory import fixture.
as_of: 2026-08-20T00:00:00Z
entities:
  - Native Import Co.
tags:
  - import-fixture
  - risk
---

# Scope

Apply the imported risk discipline before changing a position size.

# Assumptions

Cash conversion is measured consistently across reporting periods.

# Counterexamples

A temporary working-capital build can obscure otherwise durable conversion.

# Limits

This fixture rule does not replace current evidence.

# Invalidation conditions

Two consecutive periods of confirmed cash deterioration invalidate the rule.
