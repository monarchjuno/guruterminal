#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

"$SCRIPT_DIR/stage-pi-macos-arm64.sh"
"$SCRIPT_DIR/stage-core-macos-arm64.sh"
"$SCRIPT_DIR/stage-finance-macos-arm64.sh"
"$SCRIPT_DIR/stage-compute-macos-arm64.sh"
"$SCRIPT_DIR/stage-openbb-macos-arm64.sh"
"$SCRIPT_DIR/check-package-prerequisites.sh"

echo "Guru Terminal macOS arm64 staging is complete."
