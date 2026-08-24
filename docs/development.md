# Development

Node.js 22.19+, Python 3.12 with `uv`, and Rust compatible with both manifests
(CI uses 1.97.1). Install from lockfiles. The clean-checkout repository gate
below installs app dependencies; focused checks assume they are already present.

## Layout

- `apps/guruterminal/` — desktop app, agent resources, OpenBB MCP sidecar, deterministic finance worker, compute worker
- `src/` — private Memory sidecar
- `tests/` — sidecar and repository checks
- `docs/` — orientation

## Tests

Prove a user-visible or security-relevant behavior by running it. Do not add phrase-lock or source-scan tests. See `AGENTS.md` § Tests.

Use the narrowest command while editing. `scripts/verify.sh` is a handoff gate, not the edit loop.

```sh
# sidecar
cargo test --test <integration-test> --locked

# renderer (from apps/guruterminal)
npm test -- <test-file>

# desktop Rust
cargo test --manifest-path apps/guruterminal/src-tauri/Cargo.toml <test-name> --locked

# finance worker
uv run --project apps/guruterminal/python --locked pytest

# OpenBB MCP sidecar
uv run --project apps/guruterminal/openbb --locked pytest apps/guruterminal/openbb/tests

# Pi tools
node --test apps/guruterminal/agent/*.test.mjs

# compute
npm test --prefix apps/guruterminal/compute
```

From `apps/guruterminal/`, `npm run check:web`, `npm run check:rust`, and
`npm run check:python` run broader scopes without reinstalling dependencies;
`npm run check` runs all three. `npm run verify` installs locked app, E2E,
compute, and Python dependencies first and is intended for clean CI.

Codex Desktop reads `.codex/environments/environment.toml`: new worktrees get
the locked app dependencies, and the top bar exposes `Run desktop`, `Check
app`, and `Verify repository` actions. The commands above remain the portable
source of truth for other agents and terminals.

Repository gate before handoff: `scripts/verify.sh`. If a broad gate fails,
reproduce with the narrowest command, then rerun that gate.

Memory, harness, and broker policy without a window:

```sh
cargo test --manifest-path apps/guruterminal/src-tauri/Cargo.toml applied_research_wiki --locked
cargo test --manifest-path apps/guruterminal/src-tauri/Cargo.toml agent_harness --locked
cargo test --manifest-path apps/guruterminal/src-tauri/Cargo.toml memory_authority --locked
```

Live Chat and native UI stay in `apps/guruterminal/e2e` (`test:native` or `test:live-chat`). The Memory sidecar is a private worker, not a public product CLI.

## Shared window

From `apps/guruterminal/`, `npm run tauri dev` is the development window (Vite/Rust watch, `development/` app data, WebDriver on `14440`). `e2e/up.sh` attaches to that window. Isolated `test:native` needs port 1420 — stop the shared window first. Tell the user before closing a visible window.

Agents operating the window must use
`.agents/skills/guruterminal-native-e2e/SKILL.md`; the E2E README remains the
canonical command and safety reference.

Never commit staged binaries, build output, credentials, local databases, or attachments.
