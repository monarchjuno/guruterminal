# Guru Terminal Desktop

This directory contains the Guru Terminal React and Tauri desktop product, its bounded agent resources, read-only finance worker, isolated Deno/Pyodide compute worker, native E2E harness, app-local locks, and release packaging.

## Development

```sh
npm ci
npm run tauri dev
```

Development builds keep their app state in a `development/` child of the
platform app-data directory. They do not open the installed app's database,
credentials, or process state. An obsolete development database is discarded
and replaced with a fresh current-schema store instead of crashing. Debug `tauri dev`
also binds loopback WebDriver so `e2e/up.sh` and `agent-driver.mjs` can attach
to the same window you are clicking. If Vite reports that port 1420 is already
in use, attach with `e2e/up.sh` or stop the older development process before
starting again.

Isolated suites (`test:native`, `test:persistence`, live native E2E) still need
port 1420 and a no-watch E2E profile. Stop the shared development window before
those runners; after they finish, start `tauri dev` again only when you want
the window back. See [`e2e/README.md`](e2e/README.md).

Run app-local checks without reinstalling dependencies:

```sh
npm run check
```

`npm run verify` is the clean-checkout gate and installs every locked app
dependency before running the same checks.

Guru Terminal V1 supports macOS 13 or newer on Apple Silicon and Windows x64. Release builds require target sidecars plus the platform signing credentials described in the root `docs/ci-cd.md`.
