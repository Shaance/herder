#!/bin/sh
# Print the Homebrew formula for one release, filled in from the archive's .sha256 file.
# Usage: render-formula.sh VERSION DIR   (DIR holds herder-macos-universal.tar.gz.sha256)
# The release workflow pushes the output to Shaance/homebrew-tap as Formula/herder.rb.
set -eu
version=$1
dir=$2
archive=herder-macos-universal.tar.gz
printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || { echo "error: version must be X.Y.Z, got '$version'" >&2; exit 1; }
read -r sha name <"$dir/$archive.sha256"
printf '%s\n' "$sha" | grep -Eq '^[0-9a-f]{64}$' || { echo "error: malformed checksum in $dir/$archive.sha256" >&2; exit 1; }
[ "$name" = "$archive" ] || { echo "error: checksum names '$name', expected $archive" >&2; exit 1; }
sed -e "s/@VERSION@/$version/g" -e "s/@SHA256@/$sha/g" "$(dirname "$0")/herder.rb"
