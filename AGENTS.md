# Guru Terminal

Work map for agents. Desktop details: `apps/guruterminal/AGENTS.md`. Orientation: `docs/README.md`.
Commands and paths below are relative to the repository root unless noted.

## Product

Local-first Tauri app: Chat researches, Memory compounds. Memory is Markdown the user owns (Wiki, Lens, Evidence, Decision). The app does not give investment advice or trade.

Current shape: one desktop app under `apps/guruterminal/`. Chat is the only model-consuming session. Sidecars are private workers, not a public CLI.

## Layout

| Path | Owns |
| --- | --- |
| `apps/guruterminal/src/` | React UI and typed bridge calls |
| `apps/guruterminal/src-tauri/src/` | Authority, SQLite, workers, Memory writes |
| `apps/guruterminal/agent/` | Pi prompt, Skills, broker extensions |
| `apps/guruterminal/python/` | Read-only finance worker |
| `src/` | Memory sidecar (`guruterminal-core`) |
| `docs/` | Current-product orientation |

Do not hand-edit generated Tauri schemas or build output.

## Authority

React submits typed intent. Rust owns credentials, persistence, and Memory writes. Prefer existing dependencies over custom infrastructure. Pre-1.0: delete obsolete paths instead of migrating them.

Docs describe the current product. They are not a lock on future architecture. Change the product in code, then update the matching page.

## How to work

1. Check `git status --short`. Preserve unrelated changes.
2. Trace desktop behavior through `apps/guruterminal/src/bridge/commands.ts` to the Rust command. Put authority checks in Rust.
3. Run the narrowest relevant test.
4. Do not commit, push, or release unless asked.
5. Never store or print credentials.

| Change | Start | Verify |
| --- | --- | --- |
| Sessions / authority | `docs/architecture.md`, `apps/guruterminal/src-tauri/src/` | `cargo test --manifest-path apps/guruterminal/src-tauri/Cargo.toml <test> --locked` |
| Memory / Decisions | `docs/knowledge-model.md`, `src/`, `apps/guruterminal/src-tauri/src/commands/` | `cargo test --test guru_cli --locked` |
| Marketplace / credentials | `docs/bundled-capabilities.md`, `apps/guruterminal/marketplace/` | `cargo test --manifest-path apps/guruterminal/src-tauri/Cargo.toml marketplace --locked` |
| Desktop UI | `apps/guruterminal/AGENTS.md` | `npm test -- <test-file>` from `apps/guruterminal` |
| Sidecar | `src/`, `tests/` | `cargo test --test <name> --locked` |
| Finance worker | `apps/guruterminal/python/` | `cd apps/guruterminal/python && uv run --locked pytest` |

`scripts/verify.sh` is the repository handoff gate, not the edit loop. Use `npm run tauri dev` from `apps/guruterminal/` for the shared window. Debug builds use a development app-data directory and must not touch installed-app state.

Node 22.19+, Python 3.12 through `uv`, Rust compatible with both manifests. Respect lockfiles.

## Tests

Prove user-visible or security-relevant behavior by running it. Do not lock documentation wording, prompt copy, source identifiers, helper counts, or file layout. Source-scan tests are only for release pins (versions, checksums, action SHAs) or security invariants (credentials, process isolation).
