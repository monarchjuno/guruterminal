#!/bin/sh
set -eu

TARGET=aarch64-apple-darwin
MACOS_MINIMUM_VERSION=13.0
PI_VERSION=0.84.2
PI_ARCHIVE_SHA256=c996e888b7f7dce44bcf24f69176ac646c44139d3916bd49a6b28e5a8c5e3a65
DENO_VERSION=2.9.5
DENO_ARCHIVE_SHA256=b796aadd131f6930560c1ee040cf0d6f53933fbb987464e9ff46bd7ea4830615
PYODIDE_VERSION=314.0.3

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
REPO_ROOT=$(CDPATH= cd -- "$APP_ROOT/../.." && pwd)
TAURI_ROOT="$APP_ROOT/src-tauri"
PYTHON_BIN="$APP_ROOT/python/.venv/bin/python"
BINARY_DIR="$TAURI_ROOT/binaries"
RUNTIME_DIR="$TAURI_ROOT/resources/pi-runtime"
PI_BINARY="$RUNTIME_DIR/guruterminal-pi"
CORE_BINARY="$BINARY_DIR/guruterminal-core-$TARGET"
FINANCE_BINARY="$RUNTIME_DIR/finance-worker/guruterminal-finance"
COMPUTE_RUNTIME="$RUNTIME_DIR/compute-worker"
COMPUTE_BINARY="$COMPUTE_RUNTIME/guruterminal-compute"
OPENBB_RUNTIME="$RUNTIME_DIR/openbb"
OPENBB_BINARY="$OPENBB_RUNTIME/guruterminal-openbb"
CORE_VERSION=$(
    sed -n 's/^version = "\([^"]*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -n 1
)

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    echo "package prerequisite checks require macOS arm64." >&2
    exit 1
fi
for command in node file shasum find; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "required command is missing: $command" >&2
        exit 1
    fi
done
if [ ! -x "$PYTHON_BIN" ] || [ "$("$PYTHON_BIN" -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")')" != 3.12 ]; then
    echo "the pinned Python 3.12 staging environment is missing: $PYTHON_BIN" >&2
    exit 1
fi

for executable in "$PI_BINARY" "$CORE_BINARY" "$FINANCE_BINARY" "$COMPUTE_BINARY" "$OPENBB_BINARY"; do
    if [ ! -x "$executable" ]; then
        echo "required staged executable is missing: $executable" >&2
        exit 1
    fi
done
for obsolete in \
    "$RUNTIME_DIR/pi" \
    "$RUNTIME_DIR/openbb-runtime" \
    "$BINARY_DIR/guruterminal-pi-$TARGET"; do
    if [ -e "$obsolete" ]; then
        echo "obsolete runtime packaging path is present: $obsolete" >&2
        exit 1
    fi
done
if [ -n "$(find "$RUNTIME_DIR" -type f -path '*.dist-info/direct_url.json' -print -quit)" ]; then
    echo "staged runtime contains local build-path metadata." >&2
    exit 1
fi
if [ -n "$(find "$RUNTIME_DIR" -type l -print -quit)" ]; then
    echo "staged runtime contains a symbolic link." >&2
    exit 1
fi
for required in \
    "$RUNTIME_DIR/package.json" \
    "$RUNTIME_DIR/.pi-version" \
    "$RUNTIME_DIR/.pi-archive.sha256" \
    "$RUNTIME_DIR/.pi-executable.sha256" \
    "$RUNTIME_DIR/finance-worker/_internal" \
    "$COMPUTE_RUNTIME/bootstrap.mjs" \
    "$COMPUTE_RUNTIME/javascript-host.mjs" \
    "$COMPUTE_RUNTIME/contract.mjs" \
    "$COMPUTE_RUNTIME/runtime-manifest.json" \
    "$COMPUTE_RUNTIME/pyodide/pyodide.asm.wasm" \
    "$COMPUTE_RUNTIME/.deno-version" \
    "$COMPUTE_RUNTIME/.deno-archive.sha256" \
    "$COMPUTE_RUNTIME/.pyodide-version" \
    "$COMPUTE_RUNTIME/.compute-manifest.sha256" \
    "$COMPUTE_RUNTIME/.compute-executable.sha256" \
    "$OPENBB_RUNTIME/_internal" \
    "$OPENBB_RUNTIME/_internal/guruterminal_openbb/runtime-manifest.json" \
    "$OPENBB_RUNTIME/runtime-manifest.json" \
    "$OPENBB_RUNTIME/THIRD_PARTY_LICENSES/python-distributions.json" \
    "$OPENBB_RUNTIME/uv.lock" \
    "$APP_ROOT/agent/guruterminal-extension.mjs" \
    "$APP_ROOT/agent/broker-client.mjs" \
    "$APP_ROOT/agent/workbench-tools.mjs" \
    "$APP_ROOT/agent/model-run-controls.mjs" \
    "$APP_ROOT/agent/guruterminal-native-search.mjs" \
    "$APP_ROOT/agent/native-search/common.mjs" \
    "$APP_ROOT/agent/native-search/codex.mjs" \
    "$APP_ROOT/agent/native-search/anthropic.mjs" \
    "$APP_ROOT/agent/native-search/xai.mjs" \
    "$APP_ROOT/agent/guruterminal-provider-extension.mjs" \
    "$APP_ROOT/agent/SYSTEM.md" \
    "$APP_ROOT/agent/skills/research/SKILL.md" \
    "$APP_ROOT/agent/skills/wiki/SKILL.md" \
    "$APP_ROOT/agent/skills/lens/SKILL.md" \
    "$APP_ROOT/THIRD_PARTY_NOTICES.md" \
    "$REPO_ROOT/LICENSE" \
    "$REPO_ROOT/NOTICE" \
    "$TAURI_ROOT/PiEntitlements.plist"; do
    if [ ! -e "$required" ]; then
        echo "required staged asset is missing: $required" >&2
        exit 1
    fi
done
for notice_fragment in \
    'Pi coding agent' \
    '2025 Mario Zechner' \
    'Deno 2.9.5' \
    'Pyodide 314.0.3' \
    'MIT License'; do
    if ! grep -Fq "$notice_fragment" "$APP_ROOT/THIRD_PARTY_NOTICES.md"; then
        echo "Pi license notice is incomplete: $notice_fragment" >&2
        exit 1
    fi
done

for entitlement in \
    com.apple.security.cs.allow-jit \
    com.apple.security.cs.allow-unsigned-executable-memory; do
    if [ "$(/usr/libexec/PlistBuddy -c "Print :$entitlement" "$TAURI_ROOT/PiEntitlements.plist")" != true ]; then
        echo "required Pi signing entitlement is not enabled: $entitlement" >&2
        exit 1
    fi
done

if [ "$(tr -d '[:space:]' <"$RUNTIME_DIR/.pi-version")" != "$PI_VERSION" ]; then
    echo "staged Pi version marker is invalid." >&2
    exit 1
fi
if [ "$(tr -d '[:space:]' <"$RUNTIME_DIR/.pi-archive.sha256")" != "$PI_ARCHIVE_SHA256" ]; then
    echo "staged Pi archive digest marker is invalid." >&2
    exit 1
fi
if [ "$(node -p "require('$RUNTIME_DIR/package.json').version")" != "$PI_VERSION" ]; then
    echo "staged Pi package metadata has the wrong version." >&2
    exit 1
fi
RUNTIME_PI_SHA256=$(LC_ALL=C shasum -a 256 "$PI_BINARY" | awk '{print $1}')
PINNED_PI_SHA256=$(tr -d '[:space:]' <"$RUNTIME_DIR/.pi-executable.sha256")
if [ "$RUNTIME_PI_SHA256" != "$PINNED_PI_SHA256" ]; then
    echo "Pi resource binary differs from its verified digest." >&2
    exit 1
fi
if [ "$(tr -d '[:space:]' <"$COMPUTE_RUNTIME/.deno-version")" != "$DENO_VERSION" ] || \
    [ "$(tr -d '[:space:]' <"$COMPUTE_RUNTIME/.deno-archive.sha256")" != "$DENO_ARCHIVE_SHA256" ] || \
    [ "$(tr -d '[:space:]' <"$COMPUTE_RUNTIME/.pyodide-version")" != "$PYODIDE_VERSION" ]; then
    echo "staged compute runtime identity is invalid." >&2
    exit 1
fi
COMPUTE_SHA256=$(LC_ALL=C shasum -a 256 "$COMPUTE_BINARY" | awk '{print $1}')
PINNED_COMPUTE_SHA256=$(tr -d '[:space:]' <"$COMPUTE_RUNTIME/.compute-executable.sha256")
if [ "$COMPUTE_SHA256" != "$PINNED_COMPUTE_SHA256" ]; then
    echo "compute resource binary differs from its verified digest." >&2
    exit 1
fi
COMPUTE_MANIFEST_SHA256=$(LC_ALL=C shasum -a 256 "$COMPUTE_RUNTIME/runtime-manifest.json" | awk '{print $1}')
PINNED_COMPUTE_MANIFEST_SHA256=$(tr -d '[:space:]' <"$COMPUTE_RUNTIME/.compute-manifest.sha256")
if [ "$COMPUTE_MANIFEST_SHA256" != "$PINNED_COMPUTE_MANIFEST_SHA256" ]; then
    echo "compute runtime manifest differs from its verified digest." >&2
    exit 1
fi

for binary in "$PI_BINARY" "$CORE_BINARY" "$FINANCE_BINARY" "$COMPUTE_BINARY" "$OPENBB_BINARY"; do
    if ! LC_ALL=C file "$binary" | grep -q 'Mach-O 64-bit executable arm64'; then
        echo "staged artifact is not a macOS arm64 executable: $binary" >&2
        exit 1
    fi
done
find "$OPENBB_RUNTIME" -type f -exec sh -c '
    for binary do
        description=$(LC_ALL=C file -b "$binary")
        case "$description" in
            *Mach-O*)
                case "$description" in
                    *arm64*) ;;
                    *)
                        echo "OpenBB runtime contains a Mach-O without arm64 support: $binary" >&2
                        exit 1
                        ;;
                esac
                ;;
        esac
    done
' sh {} +
sh "$SCRIPT_DIR/check-macos-minimum-version.sh" \
    "$MACOS_MINIMUM_VERSION" \
    "$RUNTIME_DIR" \
    "$CORE_BINARY"

node --check "$APP_ROOT/agent/guruterminal-extension.mjs"
node --check "$APP_ROOT/agent/broker-client.mjs"
node --check "$APP_ROOT/agent/workbench-tools.mjs"
node --check "$APP_ROOT/agent/model-run-controls.mjs"
node --check "$APP_ROOT/agent/guruterminal-native-search.mjs"
node --check "$APP_ROOT/agent/native-search/common.mjs"
node --check "$APP_ROOT/agent/native-search/codex.mjs"
node --check "$APP_ROOT/agent/native-search/anthropic.mjs"
node --check "$APP_ROOT/agent/native-search/xai.mjs"
node --check "$APP_ROOT/agent/guruterminal-provider-extension.mjs"
node --test "$APP_ROOT"/agent/*.test.mjs
"$PYTHON_BIN" -m json.tool "$TAURI_ROOT/tauri.conf.json" >/dev/null
"$PYTHON_BIN" -m json.tool "$TAURI_ROOT/tauri.release.conf.json" >/dev/null
"$PYTHON_BIN" -m json.tool "$TAURI_ROOT/tauri.package-smoke.conf.json" >/dev/null
"$PYTHON_BIN" "$SCRIPT_DIR/check-macos-bundle-version.py" \
    --tauri-config "$TAURI_ROOT/tauri.conf.json" \
    --source-plist "$TAURI_ROOT/Info.plist"
TAURI_MACOS_MINIMUM=$("$PYTHON_BIN" -c '
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    print(json.load(source)["bundle"]["macOS"]["minimumSystemVersion"])
' "$TAURI_ROOT/tauri.conf.json")
if [ "$TAURI_MACOS_MINIMUM" != "$MACOS_MINIMUM_VERSION" ]; then
    echo "Tauri and sidecar macOS minimum versions differ." >&2
    exit 1
fi
"$PYTHON_BIN" "$SCRIPT_DIR/check-sidecars.py" \
    --pi "$PI_BINARY" \
    --pi-version "$PI_VERSION" \
    --pi-runtime "$RUNTIME_DIR" \
    --provider-extension "$APP_ROOT/agent/guruterminal-provider-extension.mjs" \
    --core "$CORE_BINARY" \
    --core-version "$CORE_VERSION" \
    --finance "$FINANCE_BINARY" \
    --compute "$COMPUTE_BINARY" \
    --compute-runtime "$COMPUTE_RUNTIME" \
    --deno-version "$DENO_VERSION" \
    --pyodide-version "$PYODIDE_VERSION" \
    --openbb "$OPENBB_BINARY" \
    --openbb-runtime "$OPENBB_RUNTIME"

if [ "${GURUTERMINAL_REQUIRE_DISTRIBUTION_SIGNING:-0}" = 1 ]; then
    if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then
        echo "distribution signing was required but APPLE_SIGNING_IDENTITY is absent." >&2
        exit 1
    fi
    for command in codesign plutil; do
        if ! command -v "$command" >/dev/null 2>&1; then
            echo "required signing command is missing: $command" >&2
            exit 1
        fi
    done
    find "$RUNTIME_DIR" -type f -exec sh -c '
        for binary do
            if LC_ALL=C file "$binary" | grep -q "Mach-O"; then
                codesign --verify --strict --verbose=2 "$binary"
            fi
        done
    ' sh {} +
    codesign --verify --strict --verbose=2 "$CORE_BINARY"
    for entitlement in \
        com.apple.security.cs.allow-jit \
        com.apple.security.cs.allow-unsigned-executable-memory; do
        if ! codesign --display --entitlements :- "$PI_BINARY" 2>/dev/null \
            | plutil -extract "$entitlement" raw -o - - \
            | grep -qx true; then
            echo "signed Pi resource is missing entitlement $entitlement" >&2
            exit 1
        fi
    done
else
    echo "Unsigned/ad-hoc staging validated. Signing and notarization were not asserted."
fi

echo "Guru Terminal macOS arm64 package prerequisites passed."
