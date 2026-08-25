#!/bin/sh
set -eu

TARGET=aarch64-apple-darwin
MACOS_MINIMUM_VERSION=13.0
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
PYTHON_ROOT="$APP_ROOT/python"
TAURI_ROOT="$APP_ROOT/src-tauri"
RUNTIME_ROOT="$TAURI_ROOT/resources/pi-runtime"
WORKER_RESOURCE="$RUNTIME_ROOT/finance-worker"
if [ -z "${UV_CACHE_DIR:-}" ]; then
    UV_CACHE_DIR="${TMPDIR:-/tmp}/guruterminal-uv-cache"
    export UV_CACHE_DIR
fi

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    echo "finance worker staging requires macOS arm64." >&2
    exit 1
fi
for command in uv file mktemp; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "required command is missing: $command" >&2
        exit 1
    fi
done
if [ ! -d "$RUNTIME_ROOT" ]; then
    echo "stage Pi before staging the finance worker." >&2
    exit 1
fi

uv sync --project "$PYTHON_ROOT" --locked --python 3.12
PYTHON_BIN="$PYTHON_ROOT/.venv/bin/python"
if [ ! -x "$PYTHON_BIN" ]; then
    echo "uv did not create the pinned Python 3.12 environment." >&2
    exit 1
fi
COMPATIBLE_WHEELS=$(
    "$PYTHON_BIN" - "$PYTHON_ROOT/uv.lock" "$MACOS_MINIMUM_VERSION" <<'PY'
import re
import sys
import tomllib
from pathlib import Path
from urllib.parse import unquote, urlparse

lock_path = Path(sys.argv[1])
maximum = tuple(int(part) for part in sys.argv[2].split("."))
lock = tomllib.loads(lock_path.read_text(encoding="utf-8"))
for package_name in ("numpy", "scipy"):
    packages = [
        package for package in lock["package"] if package["name"] == package_name
    ]
    if len(packages) != 1:
        raise SystemExit(f"uv.lock must contain exactly one {package_name} package")
    package = packages[0]
    pattern = re.compile(
        rf"{package_name}-{re.escape(package['version'])}-cp312-cp312-"
        r"macosx_(?P<major>[0-9]+)_(?P<minor>[0-9]+)_arm64[.]whl"
    )
    candidates = []
    for wheel in package.get("wheels", []):
        parsed = urlparse(wheel["url"])
        filename = unquote(Path(parsed.path).name)
        match = pattern.fullmatch(filename)
        version = (
            int(match.group("major")),
            int(match.group("minor")),
        ) if match else None
        if (
            version is not None
            and version <= maximum
            and parsed.scheme == "https"
            and parsed.hostname == "files.pythonhosted.org"
            and re.fullmatch(r"sha256:[0-9a-f]{64}", wheel.get("hash", ""))
        ):
            candidates.append((version, wheel))
    if not candidates:
        raise SystemExit(
            f"uv.lock has no hashed macOS 13-compatible {package_name} arm64 wheel"
        )
    selected_version = max(version for version, _ in candidates)
    selected = [wheel for version, wheel in candidates if version == selected_version]
    if len(selected) != 1:
        raise SystemExit(
            f"uv.lock has an ambiguous macOS-compatible {package_name} wheel"
        )
    wheel = selected[0]
    print(f"{wheel['url']}#sha256={wheel['hash'].removeprefix('sha256:')}")
PY
)
# The selector only emits hashed files.pythonhosted.org URLs, which cannot contain spaces.
# shellcheck disable=SC2086
uv pip install \
    --python "$PYTHON_BIN" \
    --reinstall \
    --no-deps \
    $COMPATIBLE_WHEELS
uv pip check --python "$PYTHON_BIN"
TEMP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/guruterminal-finance.XXXXXX")
NEW_WORKER=$(mktemp -d "$RUNTIME_ROOT/.finance-worker.XXXXXX")
OLD_WORKER="$RUNTIME_ROOT/.finance-worker-old.$$"
cleanup() {
    if [ -d "$OLD_WORKER" ] && [ ! -d "$WORKER_RESOURCE" ]; then
        mv "$OLD_WORKER" "$WORKER_RESOURCE"
    fi
    rm -rf -- "$TEMP_ROOT" "$NEW_WORKER"
}
trap cleanup EXIT HUP INT TERM

set -- "$PYTHON_BIN" "$PYTHON_ROOT/build_worker.py" \
    --distpath "$TEMP_ROOT/dist"
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    set -- "$@" --codesign-identity "$APPLE_SIGNING_IDENTITY"
else
    echo "Building an unsigned/ad-hoc staging worker; distribution signing is not asserted."
fi
"$@"

BUILT_WORKER="$TEMP_ROOT/dist/guruterminal-finance"
BUILT_EXECUTABLE="$BUILT_WORKER/guruterminal-finance"
if [ ! -x "$BUILT_EXECUTABLE" ] || [ ! -d "$BUILT_WORKER/_internal" ]; then
    echo "PyInstaller did not create a complete one-directory worker." >&2
    exit 1
fi
if ! LC_ALL=C file "$BUILT_EXECUTABLE" | grep -q 'Mach-O 64-bit executable arm64'; then
    echo "finance worker is not a macOS arm64 binary." >&2
    exit 1
fi
"$PYTHON_BIN" - "$BUILT_WORKER" <<'PY'
import os
import shutil
import stat
import sys
import uuid
from pathlib import Path

root = Path(sys.argv[1]).resolve(strict=True)
while True:
    links: list[Path] = []
    for directory, directory_names, file_names in os.walk(root, followlinks=False):
        base = Path(directory)
        for name in [*directory_names, *file_names]:
            candidate = base / name
            if candidate.is_symlink():
                links.append(candidate)
    if not links:
        break

    for link in sorted(links):
        try:
            target = link.resolve(strict=True)
            target.relative_to(root)
        except (OSError, ValueError) as error:
            raise SystemExit(f"finance worker symlink escapes its bundle: {link}") from error
        target_metadata = target.stat()
        temporary = link.with_name(f".{link.name}.{uuid.uuid4().hex}.materialized")
        try:
            if stat.S_ISREG(target_metadata.st_mode):
                with target.open("rb") as source, temporary.open("xb") as destination:
                    shutil.copyfileobj(source, destination)
                    destination.flush()
                    os.fsync(destination.fileno())
                os.chmod(temporary, stat.S_IMODE(target_metadata.st_mode), follow_symlinks=False)
            elif stat.S_ISDIR(target_metadata.st_mode):
                shutil.copytree(target, temporary, symlinks=True)
                os.chmod(temporary, stat.S_IMODE(target_metadata.st_mode), follow_symlinks=False)
                if not link.is_symlink():
                    raise SystemExit(
                        f"finance worker symlink disappeared before materialization: {link}"
                    )
                # macOS resolves a directory symlink passed as the destination to
                # rename(2), so replacing it directly fails with ENOTDIR. Unlink
                # the validated link before installing its materialized copy.
                link.unlink()
            else:
                raise SystemExit(f"finance worker symlink target is not a file or directory: {link}")
            os.replace(temporary, link)
        finally:
            if temporary.is_dir():
                shutil.rmtree(temporary)
            else:
                temporary.unlink(missing_ok=True)

remaining = [
    candidate
    for directory, directory_names, file_names in os.walk(root, followlinks=False)
    for candidate in [
        *(Path(directory) / name for name in directory_names),
        *(Path(directory) / name for name in file_names),
    ]
    if candidate.is_symlink()
]
if remaining:
    raise SystemExit("finance worker still contains a symlink after materialization")
PY
sh "$SCRIPT_DIR/check-macos-minimum-version.sh" \
    "$MACOS_MINIMUM_VERSION" \
    "$BUILT_WORKER"

cp -R "$BUILT_WORKER/." "$NEW_WORKER/"
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

echo "Staged the PyInstaller one-directory finance worker for $TARGET."
