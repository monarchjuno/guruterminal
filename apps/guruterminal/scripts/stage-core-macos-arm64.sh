#!/bin/sh
set -eu

TARGET=aarch64-apple-darwin
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
REPO_ROOT=$(CDPATH= cd -- "$APP_ROOT/../.." && pwd)
BINARY_DIR="$APP_ROOT/src-tauri/binaries"
STAGED_BINARY="$BINARY_DIR/guruterminal-core-$TARGET"
EXPECTED_VERSION=$(
    sed -n 's/^version = "\([^"]*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -n 1
)

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    echo "Guru Terminal Core staging requires macOS arm64." >&2
    exit 1
fi
for command in cargo file; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "required command is missing: $command" >&2
        exit 1
    fi
done

if [ -n "${GURUTERMINAL_CORE_BIN:-}" ]; then
    SOURCE_BINARY=$GURUTERMINAL_CORE_BIN
else
    cargo build \
        --manifest-path "$REPO_ROOT/Cargo.toml" \
        --release \
        --locked \
        --target "$TARGET"
    SOURCE_BINARY="$REPO_ROOT/target/$TARGET/release/guruterminal-core"
fi

if [ ! -x "$SOURCE_BINARY" ]; then
    echo "Guru Terminal Core release binary is missing or not executable." >&2
    exit 1
fi
if [ "$("$SOURCE_BINARY" --version)" != "$EXPECTED_VERSION" ]; then
    echo "Guru Terminal Core binary version does not match Cargo.toml." >&2
    exit 1
fi
if ! LC_ALL=C file "$SOURCE_BINARY" | grep -q 'Mach-O 64-bit executable arm64'; then
    echo "Guru Terminal Core release binary is not a macOS arm64 binary." >&2
    exit 1
fi

mkdir -p "$BINARY_DIR"
TEMP_BINARY="$BINARY_DIR/.guruterminal-core-$TARGET.$$"
cp "$SOURCE_BINARY" "$TEMP_BINARY"
chmod 755 "$TEMP_BINARY"
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    codesign \
        --force \
        --options runtime \
        --timestamp \
        --sign "$APPLE_SIGNING_IDENTITY" \
        "$TEMP_BINARY"
fi
mv -f "$TEMP_BINARY" "$STAGED_BINARY"

echo "Staged Guru Terminal Core v$EXPECTED_VERSION for $TARGET."
