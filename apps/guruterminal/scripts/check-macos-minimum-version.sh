#!/bin/sh
set -eu

if [ "$#" -lt 2 ]; then
    echo "usage: $0 MAXIMUM_VERSION PATH..." >&2
    exit 2
fi

MAXIMUM_VERSION=$1
shift

for command in file otool awk find grep; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "required deployment-target command is missing: $command" >&2
        exit 1
    fi
done
for root in "$@"; do
    if [ ! -e "$root" ]; then
        echo "deployment-target input is missing: $root" >&2
        exit 1
    fi
done

find "$@" -type f -exec sh -c '
    maximum=$1
    shift
    for binary do
        if ! LC_ALL=C file "$binary" | grep -q "Mach-O"; then
            continue
        fi
        minimum=$(
            otool -l "$binary" | awk '\''
                $1 == "cmd" {
                    deployment = ($2 == "LC_BUILD_VERSION" || $2 == "LC_VERSION_MIN_MACOSX")
                    next
                }
                deployment && ($1 == "minos" || $1 == "version") {
                    print $2
                    exit
                }
            '\''
        )
        if [ -z "$minimum" ]; then
            echo "Mach-O deployment target is missing: $binary" >&2
            exit 1
        fi
        if ! awk -v observed="$minimum" -v maximum="$maximum" '\''
            function valid(version, parts, count, position) {
                count = split(version, parts, ".")
                if (count < 2 || count > 3) {
                    return 0
                }
                for (position = 1; position <= count; position++) {
                    if (parts[position] !~ /^[0-9]+$/) {
                        return 0
                    }
                }
                return 1
            }
            BEGIN {
                if (!valid(observed) || !valid(maximum)) {
                    exit 2
                }
                observed_count = split(observed, observed_parts, ".")
                maximum_count = split(maximum, maximum_parts, ".")
                for (position = 1; position <= 3; position++) {
                    observed_part = position <= observed_count ? observed_parts[position] + 0 : 0
                    maximum_part = position <= maximum_count ? maximum_parts[position] + 0 : 0
                    if (observed_part < maximum_part) {
                        exit 0
                    }
                    if (observed_part > maximum_part) {
                        exit 1
                    }
                }
                exit 0
            }
        '\'' </dev/null; then
            echo "Mach-O requires macOS $minimum, above supported $maximum: $binary" >&2
            exit 1
        fi
    done
' sh "$MAXIMUM_VERSION" {} +
