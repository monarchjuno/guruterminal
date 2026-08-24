# Guru Terminal desktop

Repository map: `../../AGENTS.md`. Orientation: `../../docs/README.md`.
Run verification commands below from this directory unless the command uses a repository-root path.

## Layout

| Path | Owns |
| --- | --- |
| `src/` | React UI, typed bridge, renderer run registries |
| `src-tauri/src/` | Authority, SQLite, workers, Memory writes |
| `agent/` | Pi prompt, Skills, broker extensions |
| `python/` | Read-only finance worker |
| `compute/` | Isolated Deno/Pyodide worker |
| `marketplace/` | Official plugin marketplace (`marketplace.json` + `plugins/`) |
| `e2e/` | Native WebDriver helpers |

Keep lockfiles app-local. Do not hand-edit `src-tauri/gen/`, staged sidecars, `dist/`, `target/`, `.venv/`, or E2E dependencies.

## Authority

React is untrusted and submits typed user intent. Rust owns Guru isolation, credentials, process supervision, persistence, and Memory writes. Canonical Memory changes only through Rust. The finance worker is read-only. Agent-authored Python runs only in the compute worker (no network, host filesystem, or Memory authority).

Renderer command names must match `src-tauri/src/lib.rs`.

## Verify

Use one relevant test while editing. `npm run check:web`, `check:rust`, and
`check:python` run broader scopes without reinstalling dependencies. `npm run
verify` is the clean-checkout/CI gate.

| Change | Start | Verify |
| --- | --- | --- |
| UI or bridge type | `src/`, `src/bridge/commands.ts` | `npm test -- <test-file>` |
| Native command or storage | `src-tauri/src/lib.rs`, `commands.rs`, `store/` | `cargo test --manifest-path src-tauri/Cargo.toml <test>` |
| Chat execution | `commands/chat_runtime.rs`, `chat_execution_session.rs` | `cargo test --manifest-path src-tauri/Cargo.toml chat_runtime --locked` |
| Live Chat progress | `chat_progress.rs` | `npm test -- chat-streaming.test.tsx` |
| Pi prompt or broker tool | `agent/`, `broker.rs` | `node --test agent/*.test.mjs` |
| Finance worker | `python/`, `finance_data.rs` | `uv run --project python --locked pytest` |
| Compute worker | `compute/`, `compute.rs` | `npm test --prefix compute` |
| Native UI | `../../.agents/skills/guruterminal-native-e2e/SKILL.md` | invoke that skill |
| Packaging | `scripts/`, Tauri configs | the relevant script check |

`npm run tauri dev` is the shared development window. Isolated `test:native` suites use a no-watch E2E profile and cannot share port 1420. A browser mock is not native UI proof.
