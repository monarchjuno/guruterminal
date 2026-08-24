# Positioning and voice

The promise, UI wording, and tone. Behavior lives in code.

## The promise

**Vibe Investing: grow an investment Guru that learns.**

Most investment assistants start over with every answer. A Guru does not. It is a second investment brain: you teach it a domain in ordinary Chat, it compiles reusable Wiki and Lens pages from the research in that same turn, and the Memory workspace shows the mind filling in. Later outcome-tracking still exists, but it is not the only way the Guru learns.

## What a Guru is

A Guru is an investment agent that develops through experience. It reads primary filings, runs valuation math itself, challenges its prior views against current evidence, and improves the way it researches and reasons. What it learns persists as plain Markdown in a folder you own and is retrieved in later relevant work. Each Guru has its own Memory, so a quality-compounder Guru and a deep-value Guru develop into different investors rather than contaminating each other.

`Guru` is the product promise, not a persona label. If the agent only answers questions, stores records, or reviews past calls without developing future investment behavior, it has not become a Guru.

Gurus are the unit a user builds, names, and — in a later release — shares. Product copy treats a Guru as expertise grown through research, decisions, and correction, never as a thing that gives advice.

## Who this is for

A serious individual investor who reads filings, wants primary sources rather than a vendor's summary, and wants an investment agent whose judgment develops instead of resetting with every conversation. Single user, one machine, supported provider API key or account sign-in.

## Language and market

Guru Terminal is a global product. English is the source language for the interface, bundled Skills, product copy, and documentation. Users may chat, search, and write Memory in their own language. Retrieval should work across aliases and scripts, and the Guru should answer in the user's language when practical. Korean is one supported multilingual search case, not the product language. New user-facing copy stays in English unless it is added through an intentional localization system or reproduces user- or provider-authored content.

## What we are against

The category we reject is the generic research chatbot: it can produce an impressive answer, but its investment judgment does not develop from one case to the next. The rented research subscription compounds that problem by keeping accumulated work inside a vendor's cloud.

Two fair contrasts:

- Guru trackers show what a famous investor bought, from quarterly holdings disclosures filed weeks late, with no reasoning attached. A Guru here develops its own way of reading situations and carries lessons from earlier cases into the next one.
- Black-box trading agents call a rising backtest score self-improvement. Guru Terminal makes a different, bounded claim: a Guru turns source-grounded experience into reusable knowledge, and that knowledge must change how it handles a later relevant case. This is not model-weight training, a promise of returns, or a single performance score.

## The three proof points

Copy may lead with any of these. All three are shipped behavior, not roadmap.

1. **A Guru that carries lessons forward.** With Memory enabled, ordinary research or a "learn about" prompt can write reusable facts and investment lenses in that same turn. The Memory workspace is the visible mind. A later relevant turn must retrieve those lessons and show what current evidence retains, revises, or rejects.
2. **Primary filings, two markets, one conversation.** OpenBB brings SEC EDGAR,
   FRED, keyless market data, and configured data vendors into the same chat as
   native OpenDART, World Bank, KRX, and the Korea Investment read catalog.
   Credentials stay on the device.
3. **Files that outlive the app.** Memory is plain Markdown in a folder the user owns. The app runs locally with a supported provider API key or account sign-in, with no Guru Terminal account and no sync.

## Memory kinds in user-facing language

The canonical kinds are Wiki, Lens, Evidence, and Decision, and those names stay. Never describe them as a schema, and never instruct the user to file things into them — the Guru does that as part of learning.

| Kind | User-facing description |
| --- | --- |
| Wiki | What this Guru has learned about the world and can use again |
| Lens | How this Guru invests, including lessons, limits, and what would prove it wrong |
| Evidence | Dated claims from a research theme, each tied to the exact sourced data used |
| Decision | A judgment the Guru can learn from without rewriting history |

## Voice

Write to a competent investor who is short on time and allergic to being sold to.

- Lead with what the user gets, not with what the screen contains.
- Prefer the concrete number over the adjective.
- Name the tension before naming the feature.
- Keep the reading level plain. One idea per sentence.

## Do not write

- Investment advice, predicted returns, or any implication of profit.
- `powerful`, `seamless`, `unlock`, `supercharge`, `revolutionary`, `AI-powered`, `insights`, `leverage` as a verb.
- Schema vocabulary aimed at the user: `record kind`, `canonical`, `provenance-bound`, `atomic`, `artifact` as a noun the user must understand.
- Instructions that make the user responsible for Memory hygiene.
- Claims that self-improvement means model-weight training, guaranteed returns, or an opaque composite score.

## Color

Green and red belong to price direction and to nothing else. The default accent is burnt amber `#a4530c`. It stays distinct from the ochre `--warning` used on stale and superseded Memory badges.

## Legal boundary

Guru Terminal develops a user's investment-research Guru from source-grounded experience. It does not provide investment advice and does not execute trades. Copy must not imply either.
