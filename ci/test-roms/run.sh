#!/usr/bin/env bash
# Fetches the test ROMs listed in the manifest (checking each against its
# sha256), runs them headlessly and compares the final frame's hash with
# the expected one. Exits non-zero if any ROM fails.
#
#   ci/test-roms/run.sh [path/to/tuiba]
#
# Every ROM runs on the built-in BIOS replacement. When the manifest
# names a BIOS image, ROMs not marked `hle` run on it as well — once
# skipping its intro, once through it — and must show the same frame.
#
# Files are cached in $TUIBA_TEST_ROMS (default: target/test-roms) and
# never committed.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
tuiba=${1:-target/release/tuiba}
cache=${TUIBA_TEST_ROMS:-target/test-roms}
mkdir -p "$cache"

# fetch <cache path> <sha256> <url>: download unless cached and intact.
fetch() {
    local file=$1 sum=$2 url=$3
    if [[ ! -f $file ]] || ! echo "$sum  $file" | sha256sum -c --status; then
        mkdir -p "$(dirname "$file")"
        curl -fsSL --retry 3 -o "$file" "$url"
    fi
    echo "$sum  $file" | sha256sum -c --status
}

# check <label> <want> <tuiba args...>: run and compare the frame hash.
check() {
    local label=$1 want=$2 summary got
    shift 2
    summary=$("$tuiba" "$@")
    got=$(grep -o 'frame=[0-9a-f]*' <<<"$summary" | cut -d= -f2)
    if [[ $got == "$want" ]]; then
        echo "ok   $label"
        passed=$((passed + 1))
    else
        echo "FAIL $label: frame $got, expected $want"
        echo "     $summary"
        failed=$((failed + 1))
    fi
}

declare -A prefix
bios=
failed=0
passed=0
while read -r kind a b c d e f; do
    case $kind in
    source)
        prefix[$a]=$c
        ;;
    bios)
        name=$a url=$c sum=$d
        bios=$cache/bios/$name.bin
        if ! fetch "$bios" "$sum" "$url"; then
            echo "FAIL bios $name: sha256 mismatch after download"
            exit 1
        fi
        ;;
    rom)
        name=$a path=$b sum=$c frames=$d want=$e only=$f
        file=$cache/$name/$path
        if ! fetch "$file" "$sum" "${prefix[$name]}/$path"; then
            echo "FAIL $name/$path: sha256 mismatch after download"
            failed=$((failed + 1))
            continue
        fi
        check "$name/$path" "$want" "$file" --frames "$frames" --no-bios
        if [[ -n $bios && $only != hle ]]; then
            check "$name/$path (bios)" "$want" \
                "$file" --frames "$frames" --bios "$bios" --skip-intro
            check "$name/$path (bios, intro)" "$want" \
                "$file" --frames "$((frames + 300))" --bios "$bios"
        fi
        ;;
    esac
done < <(sed 's/#.*//' "$here/manifest")

echo "$passed passed, $failed failed"
[[ $failed -eq 0 ]]
