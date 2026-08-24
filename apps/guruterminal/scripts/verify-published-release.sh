#!/bin/sh
set -eu

if [ "$#" -ne 1 ] || [ -z "$1" ]; then
    echo "usage: verify-published-release.sh TAG" >&2
    exit 2
fi

tag=$1
attempt=1
max_attempts=12
retry_delay_seconds=5
while [ "$attempt" -le "$max_attempts" ]; do
    if gh release verify "$tag"; then
        exit 0
    fi
    if [ "$attempt" -eq "$max_attempts" ]; then
        break
    fi
    echo "Published release verification is not ready (attempt $attempt/$max_attempts); retrying." >&2
    sleep "$retry_delay_seconds"
    attempt=$((attempt + 1))
done

echo "Published release verification failed after $max_attempts attempts: $tag" >&2
exit 1
