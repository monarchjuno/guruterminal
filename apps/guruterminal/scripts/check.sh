#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
PYTHON_ROOT="$APP_ROOT/python"
OPENBB_ROOT="$APP_ROOT/openbb"
TAURI_MANIFEST="$APP_ROOT/src-tauri/Cargo.toml"
SCOPE=${1:-all}

usage() {
    echo "usage: $0 [all|web|rust|python]" >&2
}

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "required command is missing: $1" >&2
        exit 1
    fi
}

require_node_modules() {
    if [ ! -f "$1/node_modules/.package-lock.json" ]; then
        echo "dependencies are not installed in $1; run npm ci there first" >&2
        exit 1
    fi
}

check_node_version() {
    node -e '
const [major, minor] = process.versions.node.split(".").map(Number);
if (major < 22 || (major === 22 && minor < 19)) {
  console.error(`Guru Terminal requires Node 22.19 or newer; found ${process.version}`);
  process.exit(1);
}
'
}

check_web() {
    require_command node
    require_command npm
    check_node_version
    require_node_modules "$APP_ROOT"
    require_node_modules "$APP_ROOT/e2e"
    require_node_modules "$APP_ROOT/compute"

    cd "$APP_ROOT"
    sh -n \
        e2e/run-app.sh \
        e2e/run-dev-app.sh \
        e2e/up.sh \
        e2e/down.sh \
        e2e/run-native-live-chat.sh \
        e2e/run-native-persistence.sh \
        e2e/run-native-smoke.sh \
        scripts/check-macos-app-bundle.sh \
        scripts/check-macos-minimum-version.sh \
        scripts/check-package-prerequisites.sh \
        scripts/verify-published-release.sh \
        scripts/stage-compute-macos-arm64.sh \
        scripts/stage-finance-macos-arm64.sh \
        scripts/stage-openbb-macos-arm64.sh
    node -e 'JSON.parse(require("node:fs").readFileSync("src-tauri/tauri.e2e.conf.json", "utf8"));'
    node --check e2e/native-persistence.mjs
    node --check e2e/agent-driver.mjs
    node --check e2e/native-live-chat.mjs
    node --check e2e/work-progress.mjs
    node --test e2e/work-progress.test.mjs
    node --check e2e/native-smoke.mjs
    node --check e2e/wait-session.mjs
    node --check e2e/wait-session-lib.mjs
    node --check e2e/detach-launch.mjs
    node --check scripts/tauri.mjs
    node --test e2e/detach-launch.test.mjs
    node e2e/wait-session.test.mjs
    (cd e2e && npm run verify-security)
    node --check agent/guruterminal-extension.mjs
    node --check agent/broker-client.mjs
    node --check agent/workbench-tools.mjs
    node --check agent/model-run-controls.mjs
    node --check agent/guruterminal-native-search.mjs
    node --check agent/native-search/common.mjs
    node --check agent/native-search/codex.mjs
    node --check agent/native-search/anthropic.mjs
    node --check agent/native-search/xai.mjs
    node --check agent/guruterminal-provider-extension.mjs
    node --check scripts/verify-updater-signatures.mjs
    node --test agent/*.test.mjs
    (cd compute && npm test)
    npm test
    npm run build
}

check_rust() {
    require_command cargo
    require_command rustc

    cargo fmt --manifest-path "$TAURI_MANIFEST" -- --check
    cargo clippy \
        --manifest-path "$TAURI_MANIFEST" \
        --locked \
        --all-targets \
        -- \
        -D warnings
    cargo test \
        --manifest-path "$TAURI_MANIFEST" \
        --locked \
        --all-targets
}

check_python() {
    require_command uv

    uv run --project "$PYTHON_ROOT" --locked --no-sync \
        python "$APP_ROOT/scripts/check-macos-bundle-version.py" \
        --tauri-config "$APP_ROOT/src-tauri/tauri.conf.json" \
        --source-plist "$APP_ROOT/src-tauri/Info.plist"
    uv run --project "$PYTHON_ROOT" --locked --no-sync python -c '
import sys
assert sys.version_info[:2] == (3, 12), sys.version
'
    (
        cd "$PYTHON_ROOT"
        uv run --locked --no-sync ruff check . "$APP_ROOT/scripts/check-sidecars.py"
        uv run --locked --no-sync ruff format --check . "$APP_ROOT/scripts/check-sidecars.py"
        uv run --locked --no-sync pytest
    )

    uv run --project "$OPENBB_ROOT" --locked --no-sync python -c '
import sys
assert sys.version_info[:2] == (3, 12), sys.version
'
    (
        cd "$OPENBB_ROOT"
        uv run --locked --no-sync ruff check .
        uv run --locked --no-sync ruff format --check .
        uv run --locked --no-sync pytest
    )
    uv run --project "$OPENBB_ROOT" --locked --no-sync \
        python "$OPENBB_ROOT/build_sidecar.py" --check
}

case "$SCOPE" in
    all)
        check_web
        check_rust
        check_python
        ;;
    web)
        check_web
        ;;
    rust)
        check_rust
        ;;
    python)
        check_python
        ;;
    *)
        usage
        exit 2
        ;;
esac

echo "Guru Terminal $SCOPE checks passed."
