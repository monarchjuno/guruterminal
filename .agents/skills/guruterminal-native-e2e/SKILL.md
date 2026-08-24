---
name: guruterminal-native-e2e
description: Launch, inspect, operate, debug, or verify the visible Guru Terminal desktop UI. Prefer the shared development window with hot reload; use isolated suites only for product regression.
---

# Guru Terminal native UI

Read `apps/guruterminal/AGENTS.md` and `apps/guruterminal/e2e/README.md` first. The README is the canonical command and security reference.

## Shared window

- Run `apps/guruterminal/e2e/up.sh`; it attaches to or starts the shared `tauri dev` window.
- Inspect and operate it with `node apps/guruterminal/e2e/agent-driver.mjs ...`. Do not substitute a browser mock, logs, or generic computer control for native UI proof.
- Inspect before choosing selectors and after meaningful transitions. Prefer stable roles, IDs, and `aria-label` values.
- React hot-reloads. A Rust rebuild briefly drops WebDriver; retry `up.sh` or `inspect` instead of starting another window.
- Leave the window running unless the user asked to close it or an isolated suite needs port 1420.

The driver intentionally exposes no arbitrary JavaScript, navigation, cookies, uploads, or cloud sessions. Never inspect, copy, print, reset, or delete credentials or user profiles to fix a test.

## Live Chat

Immediately before a real-model Chat action, run:

```bash
node apps/guruterminal/e2e/agent-driver.mjs luna-max
```

Inspection and deterministic smoke suites remain offline. Live suites require the explicit disposable Pi profile documented in the E2E README.

## Report

Report the native flow exercised and its visible result. Run `apps/guruterminal/e2e/down.sh` only when the user asked to close the window or after warning that an isolated suite needs it stopped.
