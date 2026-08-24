#!/bin/sh
set -eu

TARGET=aarch64-apple-darwin
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
COMPUTE_ROOT="$APP_ROOT/compute"
TAURI_ROOT="$APP_ROOT/src-tauri"
RUNTIME_ROOT="$TAURI_ROOT/resources/pi-runtime"
WORKER_RESOURCE="$RUNTIME_ROOT/compute-worker"
MANIFEST="$COMPUTE_ROOT/runtime-manifest.json"

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    echo "compute worker staging requires macOS arm64." >&2
    exit 1
fi
for command in curl file mktemp node npm shasum unzip; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "required command is missing: $command" >&2
        exit 1
    fi
done
if [ ! -d "$RUNTIME_ROOT" ]; then
    echo "stage Pi before staging the compute worker." >&2
    exit 1
fi

npm ci --prefix "$COMPUTE_ROOT" --ignore-scripts
DENO_VERSION=$(node -p "require('$MANIFEST').deno.version")
PYODIDE_VERSION=$(node -p "require('$MANIFEST').pyodide.version")
DENO_ARCHIVE=$(node -p "require('$MANIFEST').deno.archives['$TARGET'].file")
DENO_ARCHIVE_SHA256=$(node -p "require('$MANIFEST').deno.archives['$TARGET'].sha256")
if [ "$(node -p "require('$COMPUTE_ROOT/node_modules/pyodide/package.json').version")" != "$PYODIDE_VERSION" ]; then
    echo "installed Pyodide does not match the compute runtime manifest." >&2
    exit 1
fi

TEMP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/guruterminal-compute.XXXXXX")
NEW_WORKER=$(mktemp -d "$RUNTIME_ROOT/.compute-worker.XXXXXX")
OLD_WORKER="$RUNTIME_ROOT/.compute-worker-old.$$"
cleanup() {
    if [ -d "$OLD_WORKER" ] && [ ! -d "$WORKER_RESOURCE" ]; then
        mv "$OLD_WORKER" "$WORKER_RESOURCE"
    fi
    rm -rf -- "$TEMP_ROOT" "$NEW_WORKER"
}
trap cleanup EXIT HUP INT TERM

ARCHIVE="$TEMP_ROOT/$DENO_ARCHIVE"
curl \
    --fail \
    --location \
    --proto '=https' \
    --tlsv1.2 \
    --retry 3 \
    --silent \
    --show-error \
    "https://github.com/denoland/deno/releases/download/v$DENO_VERSION/$DENO_ARCHIVE" \
    --output "$ARCHIVE"
ACTUAL_ARCHIVE_SHA256=$(LC_ALL=C shasum -a 256 "$ARCHIVE" | awk '{print $1}')
if [ "$ACTUAL_ARCHIVE_SHA256" != "$DENO_ARCHIVE_SHA256" ]; then
    echo "Deno archive checksum mismatch." >&2
    exit 1
fi
ARCHIVE_LIST="$TEMP_ROOT/archive-list.txt"
LC_ALL=C unzip -Z1 "$ARCHIVE" >"$ARCHIVE_LIST"
if [ "$(wc -l <"$ARCHIVE_LIST" | tr -d '[:space:]')" != 1 ] || ! grep -qx 'deno' "$ARCHIVE_LIST"; then
    echo "Deno archive contains an unexpected payload." >&2
    exit 1
fi
unzip -q "$ARCHIVE" -d "$TEMP_ROOT/deno"
if [ ! -x "$TEMP_ROOT/deno/deno" ]; then
    echo "Deno executable is missing after extraction." >&2
    exit 1
fi
if [ "$("$TEMP_ROOT/deno/deno" --version | awk 'NR == 1 { print $2 }')" != "$DENO_VERSION" ]; then
    echo "Deno executable version does not match the pinned version." >&2
    exit 1
fi
if ! LC_ALL=C file "$TEMP_ROOT/deno/deno" | grep -q 'Mach-O 64-bit executable arm64'; then
    echo "compute runtime is not a macOS arm64 executable." >&2
    exit 1
fi

cp "$TEMP_ROOT/deno/deno" "$NEW_WORKER/guruterminal-compute"
chmod 755 "$NEW_WORKER/guruterminal-compute"
cp "$COMPUTE_ROOT/bootstrap.mjs" "$COMPUTE_ROOT/javascript-host.mjs" "$COMPUTE_ROOT/contract.mjs" "$MANIFEST" "$NEW_WORKER/"
mkdir -p "$NEW_WORKER/pyodide"
for asset in pyodide.asm.mjs pyodide.asm.wasm pyodide.mjs pyodide-lock.json python_stdlib.zip; do
    cp "$COMPUTE_ROOT/node_modules/pyodide/$asset" "$NEW_WORKER/pyodide/$asset"
done

node -e '
const manifest = require(process.argv[1]);
for (const pkg of manifest.pyodide.packages) {
  process.stdout.write(`${pkg.file}\t${pkg.sha256}\n`);
}
' "$MANIFEST" | while IFS="	" read -r file digest; do
    target="$NEW_WORKER/pyodide/$file"
    curl \
        --fail \
        --location \
        --proto '=https' \
        --tlsv1.2 \
        --retry 3 \
        --silent \
        --show-error \
        "https://cdn.jsdelivr.net/pyodide/v$PYODIDE_VERSION/full/$file" \
        --output "$target"
    actual=$(LC_ALL=C shasum -a 256 "$target" | awk '{print $1}')
    if [ "$actual" != "$digest" ]; then
        echo "Pyodide package checksum mismatch: $file" >&2
        exit 1
    fi
done

printf '%s\n' "$DENO_VERSION" >"$NEW_WORKER/.deno-version"
printf '%s\n' "$DENO_ARCHIVE_SHA256" >"$NEW_WORKER/.deno-archive.sha256"
printf '%s\n' "$PYODIDE_VERSION" >"$NEW_WORKER/.pyodide-version"
LC_ALL=C shasum -a 256 "$MANIFEST" | awk '{print $1}' >"$NEW_WORKER/.compute-manifest.sha256"

if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    codesign \
        --force \
        --entitlements "$TAURI_ROOT/PiEntitlements.plist" \
        --options runtime \
        --timestamp \
        --sign "$APPLE_SIGNING_IDENTITY" \
        "$NEW_WORKER/guruterminal-compute"
fi
LC_ALL=C shasum -a 256 "$NEW_WORKER/guruterminal-compute" | awk '{print $1}' >"$NEW_WORKER/.compute-executable.sha256"

if [ -e "$OLD_WORKER" ]; then
    echo "refusing to replace unexpected staging path: $OLD_WORKER" >&2
    exit 1
fi
if [ -d "$WORKER_RESOURCE" ]; then
    mv "$WORKER_RESOURCE" "$OLD_WORKER"
fi
mv "$NEW_WORKER" "$WORKER_RESOURCE"
NEW_WORKER="$TEMP_ROOT/already-moved"
rm -rf -- "$OLD_WORKER"

echo "Staged the Deno/Pyodide compute worker for $TARGET."
