#!/usr/bin/env bash
# Downloads the Chewing dictionaries (CC BY 4.0) into the app's assets.
# The trie format matches the `chewing` crate pinned in core/ffi/Cargo.toml.
set -euo pipefail

version=2026.3.22
sha256=db8248f7a46be17beda41aedd94e7e846d01e3b2cfa3b45fcfae453acf9c62be
url=https://codeberg.org/chewing/libchewing-data/releases/download/v$version/libchewing-data-$version-Generic.zip

root=$(cd "$(dirname "$0")/.." && pwd)
out=$root/android/app/src/main/assets/chewing
stamp=$out/.version

if [[ -f $stamp && $(<"$stamp") == "$version" ]]; then
    echo "Chewing data $version already in place"
    exit 0
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl --fail --location --silent --show-error --retry 3 --output "$work/data.zip" "$url"
echo "$sha256  $work/data.zip" | sha256sum --check --quiet

rm -rf "$out"
mkdir -p "$out"
for name in word.dat tsi.dat symbols.dat swkb.dat; do
    unzip -q -j "$work/data.zip" "*/share/libchewing/$name" -d "$out"
done
echo "$version" > "$stamp"
ls -l "$out"
