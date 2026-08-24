#!/bin/sh
set -eu

PI_VERSION=0.84.2
PI_ARCHIVE_SHA256=c996e888b7f7dce44bcf24f69176ac646c44139d3916bd49a6b28e5a8c5e3a65
PI_ARCHIVE_URL="https://github.com/earendil-works/pi/releases/download/v${PI_VERSION}/pi-darwin-arm64.tar.gz"
TARGET=aarch64-apple-darwin

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
TAURI_ROOT="$APP_ROOT/src-tauri"
RESOURCE_PARENT="$TAURI_ROOT/resources"
RUNTIME_DIR="$RESOURCE_PARENT/pi-runtime"

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    echo "Pi staging requires macOS arm64." >&2
    exit 1
fi
for command in curl shasum tar mktemp file; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "required command is missing: $command" >&2
        exit 1
    fi
done

TEMP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/guruterminal-pi.XXXXXX")
NEW_RUNTIME=$(mktemp -d "$RESOURCE_PARENT/.pi-runtime.XXXXXX")
OLD_RUNTIME="$RESOURCE_PARENT/.pi-runtime-old.$$"
cleanup() {
    if [ -d "$OLD_RUNTIME" ] && [ ! -d "$RUNTIME_DIR" ]; then
        mv "$OLD_RUNTIME" "$RUNTIME_DIR"
    fi
    rm -rf -- "$TEMP_ROOT" "$NEW_RUNTIME"
}
trap cleanup EXIT HUP INT TERM

ARCHIVE="$TEMP_ROOT/pi-darwin-arm64.tar.gz"
if [ -n "${GURUTERMINAL_PI_ARCHIVE:-}" ]; then
    if [ ! -f "$GURUTERMINAL_PI_ARCHIVE" ]; then
        echo "GURUTERMINAL_PI_ARCHIVE does not name a file." >&2
        exit 1
    fi
    cp "$GURUTERMINAL_PI_ARCHIVE" "$ARCHIVE"
else
    curl \
        --fail \
        --location \
        --proto '=https' \
        --tlsv1.2 \
        --retry 3 \
        --silent \
        --show-error \
        "$PI_ARCHIVE_URL" \
        --output "$ARCHIVE"
fi

ACTUAL_SHA256=$(LC_ALL=C shasum -a 256 "$ARCHIVE" | awk '{print $1}')
if [ "$ACTUAL_SHA256" != "$PI_ARCHIVE_SHA256" ]; then
    echo "Pi archive checksum mismatch." >&2
    exit 1
fi

ARCHIVE_LIST="$TEMP_ROOT/archive-list.txt"
LC_ALL=C tar -tzf "$ARCHIVE" >"$ARCHIVE_LIST"
if awk '
    /^\// { bad = 1 }
    /(^|\/)\.\.($|\/)/ { bad = 1 }
    END { exit bad ? 0 : 1 }
' "$ARCHIVE_LIST"; then
    echo "Pi archive contains an unsafe path." >&2
    exit 1
fi
if ! grep -qx 'pi/pi' "$ARCHIVE_LIST" || ! grep -qx 'pi/package.json' "$ARCHIVE_LIST"; then
    echo "Pi archive is missing its executable or package metadata." >&2
    exit 1
fi

EXTRACT_ROOT="$TEMP_ROOT/extracted"
mkdir -p "$EXTRACT_ROOT"
LC_ALL=C tar -xzf "$ARCHIVE" -C "$EXTRACT_ROOT"
if [ ! -x "$EXTRACT_ROOT/pi/pi" ]; then
    echo "Pi executable is missing after extraction." >&2
    exit 1
fi
if [ "$("$EXTRACT_ROOT/pi/pi" --version)" != "$PI_VERSION" ]; then
    echo "Pi executable version does not match the pinned version." >&2
    exit 1
fi
if ! LC_ALL=C file "$EXTRACT_ROOT/pi/pi" | grep -q 'Mach-O 64-bit executable arm64'; then
    echo "Pi executable is not a macOS arm64 binary." >&2
    exit 1
fi

cp -R "$EXTRACT_ROOT/pi/." "$NEW_RUNTIME/"
mv "$NEW_RUNTIME/pi" "$NEW_RUNTIME/guruterminal-pi"
chmod 755 "$NEW_RUNTIME/guruterminal-pi"
printf '%s\n' "$PI_VERSION" >"$NEW_RUNTIME/.pi-version"
printf '%s\n' "$PI_ARCHIVE_SHA256" >"$NEW_RUNTIME/.pi-archive.sha256"
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    ENTITLEMENTS="$TAURI_ROOT/PiEntitlements.plist"
    if [ ! -f "$ENTITLEMENTS" ]; then
        echo "Pi signing entitlements are missing: $ENTITLEMENTS" >&2
        exit 1
    fi
    if ! command -v codesign >/dev/null 2>&1; then
        echo "codesign is required for distribution staging." >&2
        exit 1
    fi
    find "$NEW_RUNTIME" -type f -exec sh -c '
            identity=$1
            runtime=$2
            shift 2
            for candidate do
                if [ "$candidate" = "$runtime/guruterminal-pi" ]; then
                    continue
                fi
                if LC_ALL=C file "$candidate" | grep -q "Mach-O"; then
                    codesign \
                        --force \
                        --options runtime \
                        --timestamp \
                        --sign "$identity" \
                        "$candidate"
                fi
            done
        ' sh "$APPLE_SIGNING_IDENTITY" "$NEW_RUNTIME" {} +
    codesign \
        --force \
        --entitlements "$ENTITLEMENTS" \
        --options runtime \
        --timestamp \
        --sign "$APPLE_SIGNING_IDENTITY" \
        "$NEW_RUNTIME/guruterminal-pi"
fi
PI_EXECUTABLE_SHA256=$(LC_ALL=C shasum -a 256 "$NEW_RUNTIME/guruterminal-pi" | awk '{print $1}')
printf '%s\n' "$PI_EXECUTABLE_SHA256" >"$NEW_RUNTIME/.pi-executable.sha256"

if [ -e "$OLD_RUNTIME" ]; then
    echo "refusing to replace unexpected staging path: $OLD_RUNTIME" >&2
    exit 1
fi
if [ -d "$RUNTIME_DIR" ]; then
    mv "$RUNTIME_DIR" "$OLD_RUNTIME"
fi
mv "$NEW_RUNTIME" "$RUNTIME_DIR"
NEW_RUNTIME="$TEMP_ROOT/already-moved"
rm -rf -- "$OLD_RUNTIME"

echo "Staged Pi v$PI_VERSION for $TARGET."
