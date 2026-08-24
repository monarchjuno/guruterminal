#!/bin/sh
set -eu

APP_BUNDLE=${1:?usage: check-macos-app-bundle.sh /path/to/Guru\ Terminal.app}
if [ "$#" -ne 1 ] || [ ! -d "$APP_BUNDLE" ]; then
    echo "Guru Terminal app bundle is missing: $APP_BUNDLE" >&2
    exit 1
fi

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
REPO_ROOT=$(CDPATH= cd -- "$APP_ROOT/../.." && pwd)
TAURI_ROOT="$APP_ROOT/src-tauri"
PYTHON_BIN="$APP_ROOT/python/.venv/bin/python"
CONTENTS="$APP_BUNDLE/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RESOURCE_DIR="$CONTENTS/Resources"
RUNTIME_DIR="$RESOURCE_DIR/pi-runtime"
AGENT_DIR="$RESOURCE_DIR/guruterminal-agent"
PI_BINARY="$RUNTIME_DIR/guruterminal-pi"
CORE_BINARY="$MACOS_DIR/guruterminal-core"
FINANCE_BINARY="$RUNTIME_DIR/finance-worker/guruterminal-finance"
COMPUTE_RUNTIME="$RUNTIME_DIR/compute-worker"
COMPUTE_BINARY="$COMPUTE_RUNTIME/guruterminal-compute"
OPENBB_RUNTIME="$RUNTIME_DIR/openbb"
OPENBB_BINARY="$OPENBB_RUNTIME/guruterminal-openbb"
PI_VERSION=0.84.2
DENO_VERSION=2.9.5
PYODIDE_VERSION=314.0.3
CORE_VERSION=$(
    sed -n 's/^version = "\([^"]*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -n 1
)

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    echo "packaged app inspection requires macOS arm64." >&2
    exit 1
fi
if [ ! -x "$PYTHON_BIN" ] || [ "$("$PYTHON_BIN" -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")')" != 3.12 ]; then
    echo "the pinned Python 3.12 staging environment is missing: $PYTHON_BIN" >&2
    exit 1
fi

for required in \
    "$MACOS_DIR/guruterminal" \
    "$CORE_BINARY" \
    "$PI_BINARY" \
    "$FINANCE_BINARY" \
    "$COMPUTE_BINARY" \
    "$OPENBB_BINARY" \
    "$COMPUTE_RUNTIME/bootstrap.mjs" \
    "$COMPUTE_RUNTIME/javascript-host.mjs" \
    "$COMPUTE_RUNTIME/pyodide/pyodide.asm.wasm" \
    "$OPENBB_RUNTIME/_internal/guruterminal_openbb/runtime-manifest.json" \
    "$OPENBB_RUNTIME/runtime-manifest.json" \
    "$OPENBB_RUNTIME/THIRD_PARTY_LICENSES/python-distributions.json" \
    "$OPENBB_RUNTIME/uv.lock" \
    "$AGENT_DIR/SYSTEM.md" \
    "$AGENT_DIR/guruterminal-extension.mjs" \
    "$AGENT_DIR/broker-client.mjs" \
    "$AGENT_DIR/workbench-tools.mjs" \
    "$AGENT_DIR/guruterminal-native-search.mjs" \
    "$AGENT_DIR/native-search/common.mjs" \
    "$AGENT_DIR/native-search/codex.mjs" \
    "$AGENT_DIR/native-search/anthropic.mjs" \
    "$AGENT_DIR/native-search/xai.mjs" \
    "$AGENT_DIR/guruterminal-provider-extension.mjs" \
    "$AGENT_DIR/skills/research/SKILL.md" \
    "$AGENT_DIR/skills/research/agents/openai.yaml" \
    "$AGENT_DIR/skills/wiki/SKILL.md" \
    "$AGENT_DIR/skills/wiki/agents/openai.yaml" \
    "$AGENT_DIR/skills/lens/SKILL.md" \
    "$AGENT_DIR/skills/lens/agents/openai.yaml" \
    "$RESOURCE_DIR/LICENSE" \
    "$RESOURCE_DIR/NOTICE" \
    "$RESOURCE_DIR/THIRD_PARTY_NOTICES.md"; do
    if [ ! -s "$required" ]; then
        echo "packaged asset is missing or empty: $required" >&2
        exit 1
    fi
done

for executable in "$MACOS_DIR/guruterminal" "$CORE_BINARY" "$PI_BINARY" "$FINANCE_BINARY" "$COMPUTE_BINARY" "$OPENBB_BINARY"; do
    if [ ! -x "$executable" ] || ! LC_ALL=C file "$executable" | grep -Fq 'Mach-O 64-bit executable arm64'; then
        echo "packaged executable is not macOS arm64: $executable" >&2
        exit 1
    fi
done

"$PYTHON_BIN" "$SCRIPT_DIR/check-macos-bundle-version.py" \
    --tauri-config "$TAURI_ROOT/tauri.conf.json" \
    --source-plist "$TAURI_ROOT/Info.plist" \
    --bundle-plist "$CONTENTS/Info.plist"
if [ "$(plutil -extract CFBundleIdentifier raw -o - "$CONTENTS/Info.plist")" != com.monarchjuno.guruterminal ] || \
    [ "$(plutil -extract CFBundleExecutable raw -o - "$CONTENTS/Info.plist")" != guruterminal ] || \
    [ "$(plutil -extract CFBundleDisplayName raw -o - "$CONTENTS/Info.plist")" != 'Guru Terminal' ] || \
    [ "$(plutil -extract LSMinimumSystemVersion raw -o - "$CONTENTS/Info.plist")" != 13.0 ]; then
    echo "packaged Info.plist identity is invalid." >&2
    exit 1
fi

diff -qr "$APP_ROOT/agent" "$AGENT_DIR" >/dev/null
cmp -s "$REPO_ROOT/LICENSE" "$RESOURCE_DIR/LICENSE"
cmp -s "$REPO_ROOT/NOTICE" "$RESOURCE_DIR/NOTICE"
cmp -s "$APP_ROOT/THIRD_PARTY_NOTICES.md" "$RESOURCE_DIR/THIRD_PARTY_NOTICES.md"
if [ -e "$RUNTIME_DIR/openbb-runtime" ]; then
    echo "packaged app contains the obsolete OpenBB runtime path." >&2
    exit 1
fi
if [ -n "$(find "$RUNTIME_DIR" -type l -print -quit)" ]; then
    echo "packaged Pi runtime contains a symbolic link." >&2
    exit 1
fi
if [ -n "$(find "$RUNTIME_DIR" -type f -path '*.dist-info/direct_url.json' -print -quit)" ]; then
    echo "packaged Pi runtime contains local build-path metadata." >&2
    exit 1
fi

PACKAGED_PI_SHA256=$(LC_ALL=C shasum -a 256 "$PI_BINARY" | awk '{print $1}')
PINNED_PI_SHA256=$(tr -d '[:space:]' <"$RUNTIME_DIR/.pi-executable.sha256")
if [ "$PACKAGED_PI_SHA256" != "$PINNED_PI_SHA256" ]; then
    echo "packaged Pi binary differs from its pinned digest." >&2
    exit 1
fi

sh "$SCRIPT_DIR/check-macos-minimum-version.sh" 13.0 "$CONTENTS"
find "$OPENBB_RUNTIME" -type f -exec sh -c '
    for binary do
        description=$(LC_ALL=C file -b "$binary")
        case "$description" in
            *Mach-O*)
                case "$description" in
                    *arm64*) ;;
                    *)
                        echo "packaged OpenBB runtime contains a Mach-O without arm64 support: $binary" >&2
                        exit 1
                        ;;
                esac
                codesign --verify --strict --verbose=2 "$binary"
                ;;
        esac
    done
' sh {} +
"$PYTHON_BIN" "$SCRIPT_DIR/check-sidecars.py" \
    --pi "$PI_BINARY" \
    --pi-version "$PI_VERSION" \
    --pi-runtime "$RUNTIME_DIR" \
    --provider-extension "$AGENT_DIR/guruterminal-provider-extension.mjs" \
    --core "$CORE_BINARY" \
    --core-version "$CORE_VERSION" \
    --finance "$FINANCE_BINARY" \
    --compute "$COMPUTE_BINARY" \
    --compute-runtime "$COMPUTE_RUNTIME" \
    --deno-version "$DENO_VERSION" \
    --pyodide-version "$PYODIDE_VERSION" \
    --openbb "$OPENBB_BINARY" \
    --openbb-runtime "$OPENBB_RUNTIME"

echo "Guru Terminal packaged macOS app contents passed."
