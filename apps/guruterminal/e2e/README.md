# Guru Terminal native E2E

Guru Terminal's native UI boundary is the real Tauri webview exposed through
`tauri-plugin-wdio-webdriver`. Repository scripts attach with locked
`webdriverio`.

Install the locked E2E dependencies once:

```bash
cd apps/guruterminal/e2e
npm ci
```

## Shared development window

Everyday iteration and agent-driven inspection use the same debug window.

```bash
cd apps/guruterminal
npm run tauri dev
```

or, from the repository root:

```bash
apps/guruterminal/e2e/up.sh
```

`npm run tauri dev` enables Vite and Rust watch and writes a loopback WebDriver
session. `up.sh` attaches to that window when it is live, waits if Rust is
rebuilding, or starts the same debug session in a new process session so the
app stays up after the calling shell exits. Both use the development app-data
subdirectory, not the installed app and not the isolated E2E identifier.

Debug builds keep state in a `development/` child of the platform app-data
directory. If that local database is an older schema, the app discards it and
creates a fresh current-schema store instead of crashing. Installed-app data is never
opened or migrated. WebDriver-enabled development sessions also keep connector
credentials process-local, so launching or hot-reloading the shared window does
not open the macOS Keychain or Windows Credential Manager.

Drive the window from the repository root with:

```bash
node apps/guruterminal/e2e/agent-driver.mjs inspect
node apps/guruterminal/e2e/agent-driver.mjs wait-chat-idle
node apps/guruterminal/e2e/agent-driver.mjs --help
```

The bounded CLI supports inspection, stable-selector clicks and text entry,
exact option selection, a small keyboard allowlist, screenshots inside
`e2e/artifacts/`, a Chat-settle wait of up to ten minutes (enough for
`luna-max` plus `$research`), a Work-progress dump of the visible tool
trace, and the `luna-max` preset. `wait-chat-idle` is idle when Stop is
gone and no assistant article has `ChatMessage.status` `streaming`
(`article.message.assistant.streaming`). Message status classes are
`streaming`, `complete`, `aborted`, and `error`. It does not expose arbitrary
JavaScript, navigation, cookies, uploads, browser launch, or cloud sessions.

Before any real-model Chat action, run:

```bash
node apps/guruterminal/e2e/agent-driver.mjs luna-max
```

This selects and visibly verifies the exact `GPT-5.6 Luna` entry and `max`
thinking. Do not trust a persisted model selection. Offline inspection and the
deterministic native smoke do not require credentials or invoke a model.

React changes apply through Vite. A Rust rebuild restarts the native process
and drops WebDriver until the new process binds the development port
(`14440` by default) and `GET /status` reports `ready: true`. If that port is
occupied by a non-WebDriver listener, the launcher chooses a free loopback
port and records it with `profile: development` in
`e2e/artifacts/current-session.json`. Launchers wait until that endpoint
belongs to the new process tree. Retry `up.sh` or inspect; do not start a
second window.

Leave the window up while you or an agent are still using it. Stop it with:

```bash
apps/guruterminal/e2e/down.sh
```

The project skill `.agents/skills/guruterminal-native-e2e/SKILL.md` is the
mandatory entry point whenever an agent launches, inspects, operates, debugs,
or verifies the visible desktop UI.

## Isolated suites

Isolated runners keep `--no-watch` and the `com.monarchjuno.guruterminal.e2e`
identifier so product-contract tests do not mutate development state. They
cannot share Vite port 1420 with the development window. Tell the user before
closing a visible window, then stop only that session; never use broad `pkill`
cleanup. Never delete or rewrite a development or persistent profile to resolve
an E2E failure.

For an intentionally fresh database outside those suites, launch `run-app.sh`
directly. It creates temporary app state and removes it at exit. A
caller-supplied absolute `GURUTERMINAL_E2E_STATE_DIR` is retained instead.

## Deterministic native suites

Run the credential-free core smoke against fresh state:

```bash
cd apps/guruterminal/e2e
npm run test:native
```

It covers onboarding, Agent configuration including Wiki and Lens, Chat
lifecycle, destructive confirmation, and Marketplace remaining available after
the last Agent is deleted.

Run the longer chrome suite when the change is native layout, Marketplace
filtering, Settings, Memory empty-state, or minimum-window behavior:

```bash
cd apps/guruterminal/e2e
npm run test:native:full
```

When a CI job stages bundled runtimes, set
`GURUTERMINAL_E2E_REQUIRE_STAGED_RUNTIMES=1`; the full suite then requires the
OpenBB Platform Marketplace card to report `Ready`. Local development keeps the
fallback assertion that the card reports either `Ready` or `Runtime unavailable`.

Run the two-launch persistence contract separately:

```bash
cd apps/guruterminal/e2e
npm run test:persistence
```

It creates isolated state, persists an Agent and Chat through the UI, imports a
validated Wiki/Lens/Evidence/Decision fixture through the real Memory import
action, restarts the native app, verifies the same state and imported records,
then removes only its own test state.

Model, Core, and finance flows require the normal staging step:

```bash
apps/guruterminal/scripts/stage-macos-arm64.sh
```

## Explicit live suites

Live suites require an explicit absolute disposable Pi profile. They never
copy, enumerate, print, or write that profile into artifacts.

```bash
cd apps/guruterminal/e2e
GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR=/absolute/path/to/disposable/pi npm run test:live-chat
```

`test:live-chat` verifies a visible Luna/max streamed assistant delta, Finance
Core capability discovery/load plus a deterministic percentage calculation,
Work progress for compute / artifact / evidence / chart / decision, Memory
`$wiki` and `$lens` teach-then-apply, and a restarted new Chat that semantically
searches and exact-reads both records rather than relying on the prior
transcript. It also covers Memory view titles, Stop, session compaction without
error, a follow-up turn after the long sequence, and restart persistence.
Deterministic Rust and native tests remain authoritative for Memory write
integrity, Wiki/Lens policy enforcement, recovery, concurrency, and
release-only boundaries.

## Security boundary

- The WebDriver plugin is compiled only with Cargo features `webdriver` or
  `e2e`; release builds reject both.
- Debug `tauri dev` / `up.sh` attach WebDriver to the development identifier
  and the `development/` app-data subdirectory. Isolated suites use
  `com.monarchjuno.guruterminal.e2e` and an explicit state directory.
- The unauthenticated endpoint binds only to `127.0.0.1` and exists only while
  the debug or isolated launcher runs. The development window uses port `14440`
  by default; isolated runners pick a random port.
- Launchers use private permissions and a minimal environment. E2E builds ignore
  provider credential environment variables and keep connector credentials
  process-local.
- `webdriverio` and every transitive dependency are locked, and `.npmrc`
  disables dependency lifecycle scripts.
- `artifacts/`, dependencies, databases, credentials, screenshots, and local
  profiles are ignored or kept outside the repository.
- The loopback WebDriver endpoint is privileged local debug infrastructure. It
  must never be enabled in production or pointed at the installed app profile.
