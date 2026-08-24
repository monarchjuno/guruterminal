#!/bin/sh
set -eu
umask 077

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SESSION_INFO="$SCRIPT_DIR/artifacts/current-session.json"

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

if [ ! -s "$SESSION_INFO" ]; then
    echo "No Guru Terminal E2E session is running."
    exit 0
fi

LAUNCHER_PID=$(node -e '
  const session = JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8"));
  const pid = session.launcherPid;
  if (!Number.isInteger(pid) || pid <= 0) process.exit(2);
  process.stdout.write(String(pid));
' "$SESSION_INFO") || {
    echo "Guru Terminal E2E session is missing a launcher pid." >&2
    exit 1
}

if kill -0 "$LAUNCHER_PID" 2>/dev/null; then
    terminate_process_tree "$LAUNCHER_PID"
fi

attempt=0
while [ -s "$SESSION_INFO" ] || node "$SCRIPT_DIR/wait-session.mjs" --check-port 1420; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 75 ]; then
        echo "Timed out waiting for the Guru Terminal E2E session to stop." >&2
        exit 1
    fi
    sleep 0.2
done

echo "Guru Terminal E2E stopped."
