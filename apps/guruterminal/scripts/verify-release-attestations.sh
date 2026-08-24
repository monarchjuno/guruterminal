#!/bin/bash
set -euo pipefail

if [ "$#" -ne 4 ]; then
    echo "Usage: $0 <asset-directory> <owner/repository> <source-commit> <source-ref>" >&2
    exit 2
fi

ASSET_DIRECTORY=$1
REPOSITORY=$2
SOURCE_COMMIT=$3
SOURCE_REF=$4

if [ ! -d "$ASSET_DIRECTORY" ] || [ -L "$ASSET_DIRECTORY" ]; then
    echo "Asset directory must be a real directory." >&2
    exit 1
fi
if [[ ! "$REPOSITORY" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]]; then
    echo "Repository must be an owner/name pair." >&2
    exit 1
fi
if [[ ! "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ ]]; then
    echo "Source commit must be a lowercase 40-digit SHA-1." >&2
    exit 1
fi
if [[ ! "$SOURCE_REF" =~ ^refs/tags/v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
    echo "Source ref must be a stable release tag ref." >&2
    exit 1
fi

shopt -s nullglob
ASSETS=("$ASSET_DIRECTORY"/*)
if [ "${#ASSETS[@]}" -ne 11 ]; then
    echo "Closed release set must contain exactly 11 attested files." >&2
    exit 1
fi

SIGNER_WORKFLOW="$REPOSITORY/.github/workflows/release.yml"
for asset in "${ASSETS[@]}"; do
    if [ ! -f "$asset" ] || [ -L "$asset" ]; then
        echo "Attestation subject must be a regular file: $asset" >&2
        exit 1
    fi
    gh attestation verify "$asset" \
        --repo "$REPOSITORY" \
        --signer-workflow "$SIGNER_WORKFLOW" \
        --signer-digest "$SOURCE_COMMIT" \
        --source-digest "$SOURCE_COMMIT" \
        --source-ref "$SOURCE_REF" \
        --deny-self-hosted-runners \
        >/dev/null
done

echo "Verified provenance for all ${#ASSETS[@]} closed-set release assets."
