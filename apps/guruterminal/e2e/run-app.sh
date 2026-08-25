#!/bin/sh
set -eu
umask 077

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
ARTIFACT_DIR="$SCRIPT_DIR/artifacts"
SESSION_INFO="$ARTIFACT_DIR/current-session.json"
APP_PID=
CREATED_STATE_ROOT=
LIVE_PI_AGENT_DATA_DIR=
E2E_IMPORT_DIR=

terminate_process_tree() (
    target_pid=$1
    case "$target_pid" in
        *[!0-9]*|'') return ;;
    esac
    for child_pid in $(pgrep -P "$target_pid" 2>/dev/null || true); do
        terminate_process_tree "$child_pid"
    done
    if kill -0 "$target_pid" 2>/dev/null; then
        kill "$target_pid" 2>/dev/null || true
    fi
)

for command in node npm cargo pgrep; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "required command is missing: $command" >&2
        exit 1
    fi
done

NODE_BINARY=$(command -v node)
TAURI_CLI="$APP_ROOT/node_modules/@tauri-apps/cli/tauri.js"
if [ ! -f "$TAURI_CLI" ]; then
    echo "Guru Terminal dependencies are not installed. Run: (cd apps/guruterminal && npm ci)" >&2
    exit 1
fi

if [ -n "${GURUTERMINAL_E2E_STATE_DIR:-}" ]; then
    case "$GURUTERMINAL_E2E_STATE_DIR" in
        /*) ;;
        *)
            echo "GURUTERMINAL_E2E_STATE_DIR must be absolute." >&2
            exit 1
            ;;
    esac
    E2E_STATE_ROOT="$GURUTERMINAL_E2E_STATE_DIR"
    mkdir -p "$E2E_STATE_ROOT"
else
    E2E_STATE_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/guruterminal-e2e.XXXXXX")
    CREATED_STATE_ROOT="$E2E_STATE_ROOT"
fi
E2E_APP_DATA_DIR="$E2E_STATE_ROOT/app-data"
mkdir -p "$ARTIFACT_DIR"

if [ -n "${GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR:-}" ]; then
    case "$GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR" in
        /*) ;;
        *)
            echo "GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR must be absolute." >&2
            exit 1
            ;;
    esac
    if [ ! -d "$GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR" ] || [ -L "$GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR" ]; then
        echo "GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR must be a real directory." >&2
        exit 1
    fi
    LIVE_PI_AGENT_DATA_DIR="$GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR"
fi

if [ -n "${GURUTERMINAL_E2E_IMPORT_DIR:-}" ]; then
    case "$GURUTERMINAL_E2E_IMPORT_DIR" in
        /*) ;;
        *)
            echo "GURUTERMINAL_E2E_IMPORT_DIR must be absolute." >&2
            exit 1
            ;;
    esac
    if [ ! -d "$GURUTERMINAL_E2E_IMPORT_DIR" ] || [ -L "$GURUTERMINAL_E2E_IMPORT_DIR" ]; then
        echo "GURUTERMINAL_E2E_IMPORT_DIR must be a real directory." >&2
        exit 1
    fi
    E2E_IMPORT_DIR="$GURUTERMINAL_E2E_IMPORT_DIR"
fi

cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    rm -f -- "$SESSION_INFO"
    if [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2>/dev/null; then
        terminate_process_tree "$APP_PID"
        wait "$APP_PID" 2>/dev/null || true
    fi
    if [ -n "$CREATED_STATE_ROOT" ]; then
        rm -rf -- "$CREATED_STATE_ROOT"
    fi
    exit "$status"
}
trap cleanup EXIT
if [ "${GURUTERMINAL_E2E_DETACH:-}" != "1" ]; then
    trap 'exit 129' HUP
fi
trap 'exit 130' INT
trap 'exit 143' TERM

if [ -n "${GURUTERMINAL_E2E_PORT:-}" ]; then
    case "$GURUTERMINAL_E2E_PORT" in
        *[!0-9]*|'')
            echo "GURUTERMINAL_E2E_PORT must be an integer from 1024 to 65535." >&2
            exit 1
            ;;
    esac
    if [ "$GURUTERMINAL_E2E_PORT" -lt 1024 ] || [ "$GURUTERMINAL_E2E_PORT" -gt 65535 ]; then
        echo "GURUTERMINAL_E2E_PORT must be an integer from 1024 to 65535." >&2
        exit 1
    fi
    WEBDRIVER_PORT=$GURUTERMINAL_E2E_PORT
else
    WEBDRIVER_PORT=$("$NODE_BINARY" -e '
      const net = require("node:net");
      const server = net.createServer();
      server.unref();
      server.on("error", (error) => { console.error(error.message); process.exit(1); });
      server.listen(0, "127.0.0.1", () => {
        const address = server.address();
        console.log(address.port);
        server.close();
      });
    ')
fi
WEBDRIVER_PORT=$("$NODE_BINARY" "$SCRIPT_DIR/wait-session.mjs" --resolve-port "$WEBDRIVER_PORT")
export TAURI_WEBDRIVER_PORT="$WEBDRIVER_PORT"

E2E_STARTUP_TIMEOUT_MS=${GURUTERMINAL_E2E_STARTUP_TIMEOUT_MS:-300000}
case "$E2E_STARTUP_TIMEOUT_MS" in
    *[!0-9]*|'')
        echo "GURUTERMINAL_E2E_STARTUP_TIMEOUT_MS must be a positive integer." >&2
        exit 1
        ;;
esac
if [ "$E2E_STARTUP_TIMEOUT_MS" -le 0 ]; then
    echo "GURUTERMINAL_E2E_STARTUP_TIMEOUT_MS must be a positive integer." >&2
    exit 1
fi

echo "Guru Terminal E2E profile: com.monarchjuno.guruterminal.e2e"
echo "Guru Terminal E2E state: $E2E_APP_DATA_DIR"
echo "Guru Terminal E2E WebDriver: http://127.0.0.1:$WEBDRIVER_PORT"
echo "Stop this process when the test session is complete."

cd "$APP_ROOT"
# The embedded WebDriver is third-party code in the app process. Keep only the
# minimum developer runtime environment and never inherit shell credentials,
# proxies, cloud-test settings, or Node/Rust injection flags.
if [ -n "$LIVE_PI_AGENT_DATA_DIR" ]; then
    /usr/bin/env -i \
        PATH="$PATH" \
        HOME="$HOME" \
        TMPDIR="${TMPDIR:-/tmp}" \
        LANG="${LANG:-C}" \
        GURUTERMINAL_E2E_APP_DATA_DIR="$E2E_APP_DATA_DIR" \
        GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR="$LIVE_PI_AGENT_DATA_DIR" \
        GURUTERMINAL_E2E_OMIT_COLD_HISTORY="${GURUTERMINAL_E2E_OMIT_COLD_HISTORY:-}" \
        GURUTERMINAL_E2E_IMPORT_DIR="$E2E_IMPORT_DIR" \
        TAURI_WEBDRIVER_PORT="$WEBDRIVER_PORT" \
        "$NODE_BINARY" "$TAURI_CLI" dev \
        --no-watch \
        --features e2e \
        --config src-tauri/tauri.e2e.conf.json &
else
    /usr/bin/env -i \
        PATH="$PATH" \
        HOME="$HOME" \
        TMPDIR="${TMPDIR:-/tmp}" \
        LANG="${LANG:-C}" \
        GURUTERMINAL_E2E_APP_DATA_DIR="$E2E_APP_DATA_DIR" \
        GURUTERMINAL_E2E_IMPORT_DIR="$E2E_IMPORT_DIR" \
        TAURI_WEBDRIVER_PORT="$WEBDRIVER_PORT" \
        "$NODE_BINARY" "$TAURI_CLI" dev \
        --no-watch \
        --features e2e \
        --config src-tauri/tauri.e2e.conf.json &
fi
APP_PID=$!

if ! "$NODE_BINARY" "$SCRIPT_DIR/wait-session.mjs" \
    --wait-owned \
    --pid "$APP_PID" \
    --port "$WEBDRIVER_PORT" \
    --timeout-ms "$E2E_STARTUP_TIMEOUT_MS"; then
    echo "Guru Terminal E2E WebDriver did not become ready." >&2
    exit 1
fi

"$NODE_BINARY" "$SCRIPT_DIR/wait-session.mjs" \
    --write-session "$SESSION_INFO" \
    --pid "$$" \
    --port "$WEBDRIVER_PORT" \
    --profile e2e

echo "Guru Terminal E2E WebDriver is ready."
echo "Inspect it with: node apps/guruterminal/e2e/agent-driver.mjs inspect"

wait "$APP_PID"
APP_PID=
