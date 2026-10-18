#!/bin/sh
# Fetches the pinned Grafana (scripts/tool-pins.sh) for this host's platform from
# Grafana's own release tarball, and the pinned release of its PostgreSQL data source
# from Grafana's plugin catalog; checks each against the hash pinned beside it; and
# unpacks them as one Grafana home under the directory given. Prints that home.
#
# Run it once, by hand or by the CI runner's provisioning; no test fetches anything.
# The data source goes into the home's `plugins-bundled`, which Grafana loads at
# start, because Grafana 13's tarball does not carry it: left to itself Grafana
# downloads the catalog's newest release at every start, and the generator's
# Grafana tier turns that off (crates/gen/tests/suite/grafana_in_the_loop.rs).
#
# The tarball is never a package manager's: an upgrade there would move the Grafana
# the tests run on, and the CI runner would run another one. It is about 450 MB, so it
# is fetched once into a cache and kept, and a second run with the pinned home
# already unpacked fetches nothing.
#
# Usage:
#   scripts/fetch-grafana.sh <cache-directory>             fetch, then print the home
#   scripts/fetch-grafana.sh --fetched <cache-directory>   print the home only if it
#                                                          is already fetched, else
#                                                          fail; never fetches

set -eu

usage() {
    echo "usage: $0 [--fetched] <cache-directory>" >&2
    exit 2
}

only_answer=no
if [ "$#" -eq 2 ] && [ "$1" = --fetched ]; then
    only_answer=yes
    cache="$2"
elif [ "$#" -eq 1 ]; then
    cache="$1"
else
    usage
fi

# shellcheck source-path=SCRIPTDIR source=tool-pins.sh
. "$(dirname "$0")/tool-pins.sh"

# This host's platform as each download spells it, and the pinned hashes for it.
case "$(uname -s) $(uname -m)" in
'Darwin arm64')
    tarball_platform=darwin_arm64
    plugin_os=darwin
    plugin_arch=arm64
    grafana_sha256="$TOOL_PIN_GRAFANA_SHA256_DARWIN_ARM64"
    plugin_sha256="$TOOL_PIN_GRAFANA_POSTGRESQL_DATASOURCE_SHA256_DARWIN_ARM64"
    ;;
'Darwin x86_64')
    tarball_platform=darwin_amd64
    plugin_os=darwin
    plugin_arch=amd64
    grafana_sha256="$TOOL_PIN_GRAFANA_SHA256_DARWIN_AMD64"
    plugin_sha256="$TOOL_PIN_GRAFANA_POSTGRESQL_DATASOURCE_SHA256_DARWIN_AMD64"
    ;;
'Linux aarch64')
    tarball_platform=linux_arm64
    plugin_os=linux
    plugin_arch=arm64
    grafana_sha256="$TOOL_PIN_GRAFANA_SHA256_LINUX_ARM64"
    plugin_sha256="$TOOL_PIN_GRAFANA_POSTGRESQL_DATASOURCE_SHA256_LINUX_ARM64"
    ;;
'Linux x86_64')
    tarball_platform=linux_amd64
    plugin_os=linux
    plugin_arch=amd64
    grafana_sha256="$TOOL_PIN_GRAFANA_SHA256_LINUX_AMD64"
    plugin_sha256="$TOOL_PIN_GRAFANA_POSTGRESQL_DATASOURCE_SHA256_LINUX_AMD64"
    ;;
*)
    echo "error: Grafana publishes no tarball for $(uname -s) $(uname -m)" >&2
    exit 1
    ;;
esac

home="$cache/grafana-$TOOL_PIN_GRAFANA-$tarball_platform"

# The unpacked home records what it was unpacked from, one line per download; a home
# whose record is the pinned one is whole, and nothing is fetched.
expected_record="grafana $grafana_sha256
grafana-postgresql-datasource $plugin_sha256"
if [ -f "$home/.fetched" ] && [ "$(cat "$home/.fetched")" = "$expected_record" ]; then
    printf '%s\n' "$home"
    exit 0
fi
if [ "$only_answer" = yes ]; then
    exit 1
fi

if ! command -v unzip >/dev/null; then
    echo 'error: the data source is a zip archive, and this host has no unzip (Debian package unzip)' >&2
    exit 1
fi

# The SHA-256 of file $1, with whichever of the two common tools this host has.
sha256_of() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | cut -d ' ' -f 1
    else
        shasum -a 256 "$1" | cut -d ' ' -f 1
    fi
}

# Downloads URL $1 to file $2 and requires its hash to be $3.
fetch_and_check() {
    echo "fetching $1" >&2
    curl --fail --location --silent --show-error --output "$2" "$1"
    actual_sha256="$(sha256_of "$2")"
    if [ "$actual_sha256" != "$3" ]; then
        rm -f "$2"
        echo "error: $1 hashes to $actual_sha256; scripts/tool-pins.sh pins $3" >&2
        exit 1
    fi
}

mkdir -p "$cache"
tarball="$cache/grafana-$TOOL_PIN_GRAFANA-$tarball_platform.tar.gz.partial"
plugin_zip="$cache/grafana-postgresql-datasource-$TOOL_PIN_GRAFANA_POSTGRESQL_DATASOURCE-$tarball_platform.zip.partial"
fetch_and_check \
    "https://dl.grafana.com/grafana/release/$TOOL_PIN_GRAFANA/grafana_${TOOL_PIN_GRAFANA}_${TOOL_PIN_GRAFANA_BUILD}_${tarball_platform}.tar.gz" \
    "$tarball" "$grafana_sha256"
fetch_and_check \
    "https://grafana.com/api/plugins/grafana-postgresql-datasource/versions/$TOOL_PIN_GRAFANA_POSTGRESQL_DATASOURCE/download?os=$plugin_os&arch=$plugin_arch" \
    "$plugin_zip" "$plugin_sha256"

# Unpacked beside the cache under a name of its own first, so a run that stops halfway
# leaves no home that looks whole.
unpacking="$home.unpacking"
rm -rf "$unpacking" "$home"
mkdir -p "$unpacking"
tar -xzf "$tarball" -C "$unpacking" --strip-components 1
mkdir -p "$unpacking/plugins-bundled"
unzip -q "$plugin_zip" -d "$unpacking/plugins-bundled"
rm -f "$tarball" "$plugin_zip"
printf '%s\n' "$expected_record" > "$unpacking/.fetched"
mv "$unpacking" "$home"
printf '%s\n' "$home"
