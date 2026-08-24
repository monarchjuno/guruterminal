import type { ChatThread, GuruSummary, LibraryRecord } from "./types";

const MOCK_AGENT_SKILLS = ["research", "wiki", "lens"];

export const MOCK_GURUS: GuruSummary[] = [
  {
    id: "guru-quality",
    name: "Quality Compounder",
    philosophy:
      "Own high-quality businesses for the long term at sensible prices.",
    record_count: 42,
    updated_at: "2026-08-06T09:20:00+09:00",
    accent: "#a4530c",
    enabled_skill_ids: [...MOCK_AGENT_SKILLS],
    availability: { status: "available" },
  },
  {
    id: "guru-value",
    name: "Contrarian Value",
    philosophy:
      "Prioritize the gap between price and intrinsic value, with downside protection first.",
    record_count: 31,
    updated_at: "2026-08-03T16:40:00+09:00",
    accent: "#7a5b23",
    enabled_skill_ids: [...MOCK_AGENT_SKILLS],
    availability: { status: "available" },
  },
  {
    id: "guru-cycle",
    name: "Cycle Reader",
    philosophy:
      "Read industry-cycle inflections through inventory, pricing, and capital spending.",
    record_count: 27,
    updated_at: "2026-07-29T11:15:00+09:00",
    accent: "#4d5f8e",
    enabled_skill_ids: [...MOCK_AGENT_SKILLS],
    availability: { status: "available" },
  },
];

export const MOCK_THREADS: Record<string, ChatThread[]> = {
  "guru-quality": [
    {
      id: "thread-margin",
      guru_id: "guru-quality",
      title: "How should we read the margin decline?",
      updated_at: "2026-08-07T10:42:00+09:00",
      use_memory: true,
      update_memory: true,
      messages: [
        {
          id: "msg-welcome-quality",
          role: "assistant",
          content:
            "Want to determine whether this quarter's margin decline is temporary? With Guru Memory enabled, I can compare it with the quality criteria we established earlier.",
          created_at: "2026-08-07T10:40:00+09:00",
          status: "complete",
        },
      ],
    },
    {
      id: "thread-capital",
      guru_id: "guru-quality",
      title: "Capital allocation checklist",
      updated_at: "2026-08-04T14:10:00+09:00",
      use_memory: true,
      update_memory: true,
      messages: [],
    },
  ],
  "guru-value": [
    {
      id: "thread-downside",
      guru_id: "guru-value",
      title: "Downside scenario review",
      updated_at: "2026-08-03T16:40:00+09:00",
      use_memory: true,
      update_memory: true,
      messages: [],
    },
  ],
  "guru-cycle": [
    {
      id: "thread-inventory",
      guru_id: "guru-cycle",
      title: "Reading the inventory cycle",
      updated_at: "2026-07-29T11:15:00+09:00",
      use_memory: true,
      update_memory: true,
      messages: [],
    },
  ],
};

export const MOCK_LIBRARY: Record<string, LibraryRecord[]> = {
  "guru-quality": [
    {
      id: "lens:quality/earnings-quality",
      kind: "Lens",
      title: "Earnings quality review",
      excerpt:
        "Break margin changes into pricing, mix, one-time costs, and reinvestment.",
      as_of: "2026-08-06T09:20:00+09:00",
      markdown: `# Earnings quality review

Do not conclude that business quality has changed simply because quarterly margins moved.

## Review sequence

- Changes in pricing and product mix
- Normalization of raw-material and logistics costs
- Intentional spending on growth investments
- Nonrecurring one-time items

## Decision rule

If the cause is explainable and returns on reinvestment remain intact, do not treat a short-term margin decline as damage to business quality.

> Before focusing on the number itself, assess how persistent the cause is and how management allocates capital.`,
      relationships: [
        {
          relation: "uses",
          target_id: "lens:quality/durable-moat",
          target_title: "Durable moat lens",
          target_title_source: "record",
        },
        {
          relation: "supports",
          target_id: "evidence:sample/margin-bridge",
          target_title: "Margin bridge example",
          target_title_source: "record",
        },
      ],
    },
    {
      id: "lens:quality/durable-moat",
      kind: "Lens",
      title: "Durable moat lens",
      excerpt:
        "Look at customer behavior and competitor responses before high profitability.",
      as_of: "2026-08-01T13:05:00+09:00",
      markdown: `# Durable moat lens

High margins may result from a moat, but they are not a moat by themselves.

## Questions

- Why do customers choose not to switch?
- Is the advantage structurally difficult for competitors to match, even if they lower prices?
- Does customer value increase as the business scales?
- Is more capital directed toward growth than maintenance?`,
      relationships: [],
    },
    {
      id: "wiki:quality/roic",
      kind: "Wiki",
      title: "Principles for interpreting ROIC",
      excerpt:
        "Review the capital base and normalized earnings together, without separating them from growth.",
      as_of: "2026-07-25T17:25:00+09:00",
      markdown: `# Principles for interpreting ROIC

ROIC is not a point-in-time score. It is a tool for assessing how efficiently a company deploys incremental capital.

## Watchouts

- Treat acquisition goodwill consistently.
- Do not mistake peak-cycle earnings for normalized earnings.
- Analyze capital turnover separately from operating margin.`,
      relationships: [],
    },
    {
      id: "evidence:sample/margin-bridge",
      kind: "Evidence",
      title: "Margin bridge example",
      excerpt:
        "Pricing +1.8 pp, logistics normalization +0.6 pp, growth investment -1.2 pp.",
      as_of: "2026-08-06T08:55:00+09:00",
      markdown: `# Margin bridge example

## Observations

- Pricing and mix: +1.8 pp
- Logistics cost normalization: +0.6 pp
- Investment in a new region: -1.2 pp
- One-time recall: -0.9 pp

This is an illustrative example. Verify the original filing before using it in an actual investment decision.`,
      relationships: [
        {
          relation: "supports",
          target_id: "lens:quality/earnings-quality",
          target_title: "Earnings quality review",
          target_title_source: "record",
        },
      ],
    },
    {
      id: "decision:sample/quality-thesis",
      kind: "Decision",
      title: "Defer the quality-impairment call",
      excerpt:
        "Underlying margins appear intact after excluding growth investments and one-time costs.",
      as_of: "2026-08-07T09:15:00+09:00",
      markdown: `# Defer the quality-impairment call

The current evidence does not support declaring long-term damage to business quality.

## Revisit if

- Underlying margins decline for two consecutive quarters
- Customer retention moves outside its historical range
- Growth investments take longer than planned to pay back`,
      relationships: [
        {
          relation: "uses",
          target_id: "lens:quality/earnings-quality",
          target_title: "Earnings quality review",
          target_title_source: "record",
        },
      ],
    },
  ],
  "guru-value": [
    {
      id: "lens:value/downside-first",
      kind: "Lens",
      title: "Start with the downside",
      excerpt:
        "Estimate recoverable value and impairment risk before an optimistic price target.",
      as_of: "2026-08-03T16:40:00+09:00",
      markdown: `# Start with the downside

Before forecasting precisely, estimate the loss if the thesis is wrong.

## Sequence

- Verify net cash and noncore assets
- Set a conservative range for normalized cash flow
- Define a structural-impairment scenario
- Estimate how long the company can endure without a catalyst`,
      relationships: [],
    },
  ],
  "guru-cycle": [
    {
      id: "lens:cycle/inventory",
      kind: "Lens",
      title: "Inventory inflection lens",
      excerpt:
        "Track inventory relative to sales alongside the direction of order lead times.",
      as_of: "2026-07-29T11:15:00+09:00",
      markdown: `# Inventory inflection lens

Rising inventory is neither inherently good nor bad. Consider demand, supply constraints, and order cancellations together.

## Leading observations

- Inventory relative to sales
- Customer inventory days
- Order lead times
- Utilization and new-capacity schedules`,
      relationships: [],
    },
  ],
};
