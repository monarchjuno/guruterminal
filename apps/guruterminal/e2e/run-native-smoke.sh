#!/bin/sh
set -eu
umask 077

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SESSION_INFO="$SCRIPT_DIR/artifacts/current-session.json"
RUN_LOG=$(mktemp "${TMPDIR:-/tmp}/guruterminal-native-e2e.XXXXXX")
APP_PID=
SMOKE_ARGS=
E2E_STARTUP_TIMEOUT_MS=${GURUTERMINAL_E2E_STARTUP_TIMEOUT_MS:-300000}

case "${1:-}" in
    --full)
        SMOKE_ARGS=--full
        ;;
    "" )
        ;;
    *)
        echo "usage: $0 [--full]" >&2
        exit 1
        ;;
esac

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

cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    if [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2>/dev/null; then
        kill "$APP_PID" 2>/dev/null || true
        wait "$APP_PID" 2>/dev/null || true
    fi
    rm -f -- "$RUN_LOG"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

rm -f -- "$SESSION_INFO"
"$SCRIPT_DIR/run-app.sh" >"$RUN_LOG" 2>&1 &
APP_PID=$!

if ! node "$SCRIPT_DIR/wait-session.mjs" \
    --pid "$APP_PID" \
    --session "$SESSION_INFO" \
    --timeout-ms "$E2E_STARTUP_TIMEOUT_MS"; then
    cat "$RUN_LOG" >&2
    echo "Guru Terminal exited before the native E2E session was ready." >&2
    exit 1
fi

node "$SCRIPT_DIR/native-smoke.mjs" "$SESSION_INFO" $SMOKE_ARGS
