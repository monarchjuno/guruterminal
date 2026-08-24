<div align="center">

<img src="apps/guruterminal/assets/chimp-icon.png" alt="Guru Terminal" width="108" />

# Guru Terminal

**Vibe Investing: grow an investment Guru that learns.**

A self-improving investment agent: it turns source-grounded research into reusable knowledge, and that knowledge changes how it handles the next case.

[Install](installation.md) • [Documentation](docs/README.md) • [How it learns](docs/knowledge-model.md) • [Architecture](docs/architecture.md)

</div>

---

Most investment assistants start over with every answer. A Guru does not. It is a second investment brain: it reads primary filings, runs the valuation math itself, challenges its prior views, and turns each piece of research into knowledge it uses again — in later companies, later markets, later decisions.

Everything a Guru learns is plain Markdown in a folder you own. The app runs on your machine, on your own API key. There is no account, no sync, and no subscription that takes your Guru with it.

## Teach it once, it stays taught

**Day 1** — *"Learn how Korean holding companies trade against their net asset value."* The Guru researches primary sources and writes reusable Wiki and Lens pages in that same turn. The Memory workspace shows the mind filling in.

**Week 3** — *"Research LG Corp's holdco discount."* The Guru retrieves those pages, checks them against current filings, and shows what current evidence retains, revises, or rejects.

Guru trackers tell you what a famous investor bought, weeks late, with no reasoning attached. A Guru develops its own way of reading situations and carries lessons from earlier cases into the next one.

## What makes it a Guru

- **It carries lessons forward.** Ordinary research can improve reusable facts and investment lenses. A later relevant turn retrieves those lessons instead of starting from zero. Memory is Markdown you own; prior versions stay in the Guru's local git history.
- **Primary filings, two markets, one conversation.** OpenBB brings SEC EDGAR, FRED, keyless market data, and configured data vendors into the same chat as native OpenDART, World Bank, KRX, and the complete 146-endpoint Korea Investment read catalog. Credentials stay on the device.
- **Its own math.** Thirteen deterministic calculations — discounted cash flow and its sensitivity, IRR, WACC, enterprise-value bridge, risk metrics, and more — run in a worker with no network access. Agent-written Python runs in a sandbox with no network, filesystem, or environment.
- **Files that outlive the app.** Memory is Markdown you can open in any editor. Chart and document workspaces sit beside the conversation.

## How a Guru remembers

Each Guru keeps its own Memory, so a quality-compounder Guru and a deep-value Guru develop into different investors rather than contaminating each other. There are four kinds:

| Kind | What it holds |
| --- | --- |
| Wiki | What this Guru has learned about the world and can use again |
| Lens | How this Guru invests, including lessons, limits, and what would prove it wrong |
| Evidence | Dated claims selected from exact current-turn results, with host-recorded source receipts |
| Decision | A judgment the Guru can learn from without rewriting history |

You never file anything into these yourself — the Guru does that as part of learning. With Memory enabled, it improves Wiki and Lens during ordinary Chat, and a later relevant turn must retrieve and apply that learned state. A stored record or a retrospective review alone is not learning.

> [!NOTE]
> This is a bounded claim, not a black-box one. A Guru turns source-grounded experience into reusable knowledge that changes how it handles a later relevant case. It is not model-weight training, a promise of returns, or a single performance score.

## Install

Guru Terminal V1 is one desktop application for macOS 13 or newer on Apple Silicon and Windows x64. Download the signed installer from the latest [GitHub Release](https://github.com/monarchjuno/guruterminal/releases) and follow [installation.md](installation.md). macOS artifacts are signed and notarized; Windows artifacts are Authenticode-signed. The app accepts only updates signed by the Guru Terminal updater key.

## Security model

- React submits typed intent and has no raw filesystem, process, credential, or Memory-write authority.
- Rust owns Guru isolation, persistence, and Memory writes.
- Model, OpenBB MCP, compute, and deterministic finance workers are bounded disposable sidecars.
- Memory is the Guru's durable learned state; immutable Skill revisions define reviewed product workflows.

## Development

Requirements and commands are in [docs/development.md](docs/development.md). Product-facing language is in [docs/positioning.md](docs/positioning.md). Agent work map: [AGENTS.md](AGENTS.md). Run the complete verifier with:

```sh
scripts/verify.sh
```

Release and update behavior is documented in [docs/ci-cd.md](docs/ci-cd.md).

> [!IMPORTANT]
> Guru Terminal develops a user's investment-research Guru from source-grounded experience. It does not provide investment advice and does not execute trades.

Guru Terminal is licensed under AGPL-3.0-only.
