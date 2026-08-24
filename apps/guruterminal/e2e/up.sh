#!/bin/sh
set -eu
umask 077

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ARTIFACT_DIR="$SCRIPT_DIR/artifacts"
SESSION_INFO="$ARTIFACT_DIR/current-session.json"
LAUNCHER_LOG="$ARTIFACT_DIR/launcher.log"
DEV_WEBDRIVER_PORT=${GURUTERMINAL_DEV_WEBDRIVER_PORT:-14440}

mkdir -p "$ARTIFACT_DIR"

if node "$SCRIPT_DIR/wait-session.mjs" --check "$SESSION_INFO"; then
    if TAURI_WEBDRIVER_PORT="$DEV_WEBDRIVER_PORT" node "$SCRIPT_DIR/wait-session.mjs" --is-dev-session "$SESSION_INFO"; then
        echo "Guru Terminal development window is already running."
        echo "Inspect it with: node apps/guruterminal/e2e/agent-driver.mjs inspect"
        echo "Stop it with: apps/guruterminal/e2e/down.sh"
        exit 0
    fi
    echo "An isolated E2E window owns the current session. Stop it with apps/guruterminal/e2e/down.sh first." >&2
    exit 1
fi

if TAURI_WEBDRIVER_PORT="$DEV_WEBDRIVER_PORT" node "$SCRIPT_DIR/wait-session.mjs" --recover "$SESSION_INFO"; then
    echo "Guru Terminal development window is ready again."
    echo "Inspect it with: node apps/guruterminal/e2e/agent-driver.mjs inspect"
    echo "Stop it with: apps/guruterminal/e2e/down.sh"
    exit 0
fi

if TAURI_WEBDRIVER_PORT="$DEV_WEBDRIVER_PORT" node "$SCRIPT_DIR/wait-session.mjs" --adopt-dev "$SESSION_INFO"; then
    echo "Attached to the existing Guru Terminal development window."
    echo "Inspect it with: node apps/guruterminal/e2e/agent-driver.mjs inspect"
    echo "Stop it with: apps/guruterminal/e2e/down.sh"
    exit 0
fi

if node "$SCRIPT_DIR/wait-session.mjs" --check-port 1420; then
    echo "Port 1420 is in use without the development WebDriver." >&2
    echo "Restart that window with: (cd apps/guruterminal && npm run tauri dev)" >&2
    exit 1
fi

rm -f -- "$SESSION_INFO"

# Start a new session so the app survives the calling shell's process group.
export GURUTERMINAL_E2E_DETACH=1
export TAURI_WEBDRIVER_PORT="$DEV_WEBDRIVER_PORT"
: >"$LAUNCHER_LOG"
LAUNCHER_PID=$(node "$SCRIPT_DIR/detach-launch.mjs" "$SCRIPT_DIR/run-dev-app.sh" "$LAUNCHER_LOG")

if ! node "$SCRIPT_DIR/wait-session.mjs" --pid "$LAUNCHER_PID" --session "$SESSION_INFO"; then
    echo "Guru Terminal development window failed to start." >&2
    cat "$LAUNCHER_LOG" >&2
    exit 1
fi

echo "Guru Terminal development window is ready."
echo "Inspect it with: node apps/guruterminal/e2e/agent-driver.mjs inspect"
echo "Stop it with: apps/guruterminal/e2e/down.sh"
