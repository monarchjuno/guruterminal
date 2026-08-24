#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
PYTHON_ROOT="$APP_ROOT/python"
OPENBB_ROOT="$APP_ROOT/openbb"

for command in node npm cargo rustc uv; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "required command is missing: $command" >&2
        exit 1
    fi
done

node -e '
const [major, minor] = process.versions.node.split(".").map(Number);
if (major < 22 || (major === 22 && minor < 19)) {
  console.error(`Guru Terminal requires Node 22.19 or newer; found ${process.version}`);
  process.exit(1);
}
'

cd "$APP_ROOT"

# CI starts from a clean checkout. Local agents that already have dependencies
# should use scripts/check.sh so their edit loop does not reinstall them.
npm ci
(cd e2e && npm ci)
(cd compute && npm ci --ignore-scripts)
uv sync --project "$PYTHON_ROOT" --locked --python 3.12
uv sync --project "$OPENBB_ROOT" --locked --python 3.12

"$SCRIPT_DIR/check.sh" all

echo "Guru Terminal app-local verification passed."
