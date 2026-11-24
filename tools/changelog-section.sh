#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Print the body of one section of CHANGELOG.md, without its heading (8.10).
# Use: tools/changelog-section.sh VERSION [FILE]
#   VERSION is x.y.z (the heading "## x.y.z - YYYY-MM-DD") or the word Unreleased.
#   FILE is CHANGELOG.md of the repository by default.
# Exit code: 0 with the text on stdout; 1 when the section is absent or empty; 2 for a bad call.
set -eu
[ "$#" -ge 1 ] && [ "$#" -le 2 ] || { echo "use: tools/changelog-section.sh VERSION [FILE]" >&2; exit 2; }
version=$1
file=${2:-"$(dirname "$0")/../CHANGELOG.md"}
[ -f "$file" ] || { echo "error: $file does not exist" >&2; exit 2; }

body=$(awk -v v="$version" '
    /^## / {
        if (on) exit
        if (v == "Unreleased") { if ($0 == "## Unreleased") { on = 1; next } }
        else if (index($0, "## " v " - ") == 1) { on = 1; next }
        next
    }
    on { print }' "$file")
# Remove the blank lines at the start and at the end.
body=$(printf '%s\n' "$body" | sed -e '/./,$!d' | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}')
if [ -z "$body" ]; then
    echo "error: the section '$version' of $file is absent or empty" >&2
    exit 1
fi
printf '%s\n' "$body"
