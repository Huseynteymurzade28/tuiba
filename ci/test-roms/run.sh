#!/usr/bin/env bash
# Fetches the test ROMs listed in the manifest (checking each against its
# sha256), runs them headlessly and compares the final frame's hash with
# the expected one. Exits non-zero if any ROM fails.
#
#   ci/test-roms/run.sh [path/to/tuiba]
#
# ROMs are cached in $TUIBA_TEST_ROMS (default: target/test-roms) and
# never committed.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
tuiba=${1:-target/release/tuiba}
cache=${TUIBA_TEST_ROMS:-target/test-roms}
mkdir -p "$cache"

declare -A prefix
failed=0
passed=0
while read -r kind a b c d e f; do
    case $kind in
    source)
        prefix[$a]=$c
        ;;
    rom)
        name=$a path=$b sum=$c frames=$d want=$e
        file=$cache/$name/$path
        if [[ ! -f $file ]] || ! echo "$sum  $file" | sha256sum -c --status; then
            mkdir -p "$(dirname "$file")"
            curl -fsSL --retry 3 -o "$file" "${prefix[$name]}/$path"
        fi
        if ! echo "$sum  $file" | sha256sum -c --status; then
            echo "FAIL $name/$path: sha256 mismatch after download"
            failed=$((failed + 1))
            continue
        fi
        summary=$("$tuiba" "$file" --frames "$frames")
        got=$(grep -o 'frame=[0-9a-f]*' <<<"$summary" | cut -d= -f2)
        if [[ $got == "$want" ]]; then
            echo "ok   $name/$path"
            passed=$((passed + 1))
        else
            echo "FAIL $name/$path: frame $got, expected $want"
            echo "     $summary"
            failed=$((failed + 1))
        fi
        ;;
    esac
done < <(sed 's/#.*//' "$here/manifest")

echo "$passed passed, $failed failed"
[[ $failed -eq 0 ]]
