#!/bin/sh
set -eu

TARGET=aarch64-apple-darwin
MACOS_MINIMUM_VERSION=13.0
MACOSX_DEPLOYMENT_TARGET=$MACOS_MINIMUM_VERSION
export MACOSX_DEPLOYMENT_TARGET
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
OPENBB_ROOT="$APP_ROOT/openbb"
TAURI_ROOT="$APP_ROOT/src-tauri"
RUNTIME_ROOT="$TAURI_ROOT/resources/pi-runtime"
SIDECAR_RESOURCE="$RUNTIME_ROOT/openbb"
if [ -z "${UV_CACHE_DIR:-}" ]; then
    UV_CACHE_DIR="${TMPDIR:-/tmp}/guruterminal-uv-cache"
    export UV_CACHE_DIR
fi

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    echo "OpenBB staging requires macOS arm64." >&2
    exit 1
fi
for command in uv file mktemp; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "required command is missing: $command" >&2
        exit 1
    fi
done
if [ ! -d "$RUNTIME_ROOT" ]; then
    echo "stage Pi before staging OpenBB." >&2
    exit 1
fi

# Reinstall under the deployment target so uv selects compatible arm64 wheels
# instead of host-optimized macOS 14 variants from the same lockfile.
uv sync \
    --project "$OPENBB_ROOT" \
    --locked \
    --python 3.12 \
    --python-platform aarch64-apple-darwin \
    --reinstall-package numpy \
    --reinstall-package scipy
PYTHON_BIN="$OPENBB_ROOT/.venv/bin/python"
uv pip check --python "$PYTHON_BIN"
TEMP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/guruterminal-openbb.XXXXXX")
NEW_RESOURCE=$(mktemp -d "$RUNTIME_ROOT/.openbb.XXXXXX")
OLD_RESOURCE="$RUNTIME_ROOT/.openbb-old.$$"
cleanup() {
    if [ -d "$OLD_RESOURCE" ] && [ ! -d "$SIDECAR_RESOURCE" ]; then
        mv "$OLD_RESOURCE" "$SIDECAR_RESOURCE"
    fi
    rm -rf -- "$TEMP_ROOT" "$NEW_RESOURCE"
}
trap cleanup EXIT HUP INT TERM

set -- "$PYTHON_BIN" "$OPENBB_ROOT/build_sidecar.py" --distpath "$TEMP_ROOT/dist"
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    set -- "$@" --codesign-identity "$APPLE_SIGNING_IDENTITY"
else
    echo "Building an unsigned/ad-hoc OpenBB staging runtime."
fi
"$@"

BUILT_RUNTIME="$TEMP_ROOT/dist/guruterminal-openbb"
BUILT_EXECUTABLE="$BUILT_RUNTIME/guruterminal-openbb"
if [ ! -x "$BUILT_EXECUTABLE" ] || \
    [ ! -d "$BUILT_RUNTIME/_internal" ] || \
    [ ! -s "$BUILT_RUNTIME/_internal/random_user_agent/data/user_agents.zip" ] || \
    [ ! -s "$BUILT_RUNTIME/THIRD_PARTY_LICENSES/python-distributions.json" ]; then
    echo "PyInstaller did not create a complete OpenBB runtime." >&2
    exit 1
fi
if ! LC_ALL=C file "$BUILT_EXECUTABLE" | grep -q 'Mach-O 64-bit executable arm64'; then
    echo "OpenBB runtime is not a macOS arm64 binary." >&2
    exit 1
fi
"$PYTHON_BIN" "$OPENBB_ROOT/materialize_bundle_symlinks.py" "$BUILT_RUNTIME"
if find "$BUILT_RUNTIME" -type l -print -quit | grep -q .; then
    echo "OpenBB runtime still contains a symlink after materialization." >&2
    exit 1
fi
sh "$SCRIPT_DIR/check-macos-minimum-version.sh" \
    "$MACOS_MINIMUM_VERSION" \
    "$BUILT_RUNTIME"

cp -R "$BUILT_RUNTIME/." "$NEW_RESOURCE/"
if [ -e "$OLD_RESOURCE" ]; then
    echo "refusing to replace unexpected staging path: $OLD_RESOURCE" >&2
    exit 1
fi
if [ -d "$SIDECAR_RESOURCE" ]; then
    mv "$SIDECAR_RESOURCE" "$OLD_RESOURCE"
fi
mv "$NEW_RESOURCE" "$SIDECAR_RESOURCE"
NEW_RESOURCE="$TEMP_ROOT/already-moved"
rm -rf -- "$OLD_RESOURCE"

echo "Staged the OpenBB runtime for $TARGET."
