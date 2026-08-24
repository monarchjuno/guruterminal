#!/bin/sh
set -eu
umask 077

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SESSION_INFO="$SCRIPT_DIR/artifacts/current-session.json"
STATE_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/guruterminal-persistence-e2e.XXXXXX")
RUN_LOG=$(mktemp "${TMPDIR:-/tmp}/guruterminal-persistence-log.XXXXXX")
APP_PID=

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
      socket.once("connect", () => {
        socket.destroy();
        resolve(true);
      });
      const closed = () => {
        socket.destroy();
        resolve(false);
      };
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
    rm -f -- "$RUN_LOG"
    rm -rf -- "$STATE_ROOT"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

run_phase() {
    phase=$1
    : >"$RUN_LOG"
    rm -f -- "$SESSION_INFO"
    GURUTERMINAL_E2E_STATE_DIR="$STATE_ROOT" \
        "$SCRIPT_DIR/run-app.sh" >"$RUN_LOG" 2>&1 &
    APP_PID=$!

    if ! node "$SCRIPT_DIR/wait-session.mjs" --pid "$APP_PID" --session "$SESSION_INFO"; then
        cat "$RUN_LOG" >&2
        echo "Guru Terminal exited before persistence phase $phase was ready." >&2
        exit 1
    fi

    node "$SCRIPT_DIR/native-persistence.mjs" "$SESSION_INFO" "$phase"
    cleanup_app
    wait_for_dev_server_exit
}

run_phase seed
run_phase verify
echo "Guru Terminal native persistence smoke passed."
