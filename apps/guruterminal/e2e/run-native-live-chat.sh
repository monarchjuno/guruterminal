#!/bin/sh
set -eu
umask 077

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SESSION_INFO="$SCRIPT_DIR/artifacts/current-session.json"
STATE_ROOT=
RUN_LOG=
APP_PID=
REQUESTED_PHASE=${1:-all}

case "$REQUESTED_PHASE" in
    all|smoke) ;;
    *)
        echo "usage: $0 [smoke]" >&2
        exit 2
        ;;
esac

if [ -z "${GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR:-}" ]; then
    echo "GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR is required for the live native Chat E2E." >&2
    exit 1
fi

cleanup_app() {
    if [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2>/dev/null; then
        kill "$APP_PID" 2>/dev/null || true
        wait "$APP_PID" 2>/dev/null || true
    fi
    APP_PID=
    rm -f -- "$SESSION_INFO"
}

wait_for_dev_server_exit() {
    node <<'NODE'
import net from "node:net";

const deadline = Date.now() + 15_000;
while (Date.now() < deadline) {
  let open = false;
  for (const host of ["127.0.0.1", "::1"]) {
    if (await new Promise((resolve) => {
      const socket = net.createConnection({ host, port: 1420 });
      socket.setTimeout(200);
      socket.once("connect", () => { socket.destroy(); resolve(true); });
      const closed = () => { socket.destroy(); resolve(false); };
      socket.once("error", closed);
      socket.once("timeout", closed);
    })) {
      open = true;
      break;
    }
  }
  if (!open) process.exit(0);
  await new Promise((resolve) => setTimeout(resolve, 100));
}
throw new Error("Guru Terminal dev server did not exit within 15 seconds");
NODE
}

cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    cleanup_app
    if [ -n "$RUN_LOG" ]; then
        if [ "$status" -ne 0 ]; then
            cat "$RUN_LOG" >&2
        fi
        rm -f -- "$RUN_LOG"
    fi
    if [ -n "$STATE_ROOT" ]; then
        rm -rf -- "$STATE_ROOT"
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

STATE_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/guruterminal-live-chat-e2e.XXXXXX")
RUN_LOG=$(mktemp "${TMPDIR:-/tmp}/guruterminal-live-chat-log.XXXXXX")

run_phase() {
    phase=$1
    : >"$RUN_LOG"
    rm -f -- "$SESSION_INFO"
    GURUTERMINAL_E2E_STATE_DIR="$STATE_ROOT" \
        GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR="$GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR" \
        "$SCRIPT_DIR/run-app.sh" >"$RUN_LOG" 2>&1 &
    APP_PID=$!

    if ! node "$SCRIPT_DIR/wait-session.mjs" --pid "$APP_PID" --session "$SESSION_INFO"; then
        cat "$RUN_LOG" >&2
        echo "Guru Terminal exited before live Chat phase $phase was ready." >&2
        exit 1
    fi

    node "$SCRIPT_DIR/native-live-chat.mjs" "$SESSION_INFO" "$phase"
    cleanup_app
    wait_for_dev_server_exit
}

if [ "$REQUESTED_PHASE" = "smoke" ]; then
    run_phase smoke
    echo "Guru Terminal native Luna max Chat smoke passed."
else
    run_phase run
    run_phase verify
    echo "Guru Terminal native Luna max Chat E2E passed."
fi
