#!/bin/sh
# The host programs this workspace's tests execute, and the Debian package each is in.
#
# Readers:
#   - CI runs it (no argument) as the step after bc-rs's checkout, so a
#     runner missing a program fails in seconds with the package named.
#   - scripts/verify.sh runs it the same way, then reads `--postgresql-bin` and puts
#     that directory first on PATH for the suite.
#   - `--packages` prints the Debian packages, for the runner's provisioning list.
#
# The PostgreSQL suites need most of these: the generator's layout suite and the
# exporter's, each starting a throwaway server of its own
# (tests/support/postgres_cluster.rs). The generator's Grafana tier adds curl, which
# it talks to Grafana through, and kill, which stops a killed run's leftover Grafana
# (crates/gen/tests/suite/grafana_in_the_loop.rs); Grafana itself is no host program —
# scripts/fetch-grafana.sh fetches the pinned one, and verify.sh names its home. They
# run on the pinned PostgreSQL major and on no other, so a host with only an older or
# newer one is refused by name rather than tested on a version nobody chose
# (scripts/tool-pins.sh).
#
# PostgreSQL's server programs are often not on PATH — Debian keeps them in
# /usr/lib/postgresql/<version>/bin, and a Homebrew keg-only formula in its keg — so
# they are found in the pinned major's directory among the places those two put them,
# and PATH's own. Everything else is found on PATH, as the tests find it.

set -eu

# shellcheck source-path=SCRIPTDIR source=tool-pins.sh
. "$(dirname "$0")/tool-pins.sh"

# Each line is "<program> <package> <where>":
#   where  postgresql — in PostgreSQL's program directory; path — on PATH.
host_programs() {
    printf '%s\n' \
        "initdb postgresql-$TOOL_PIN_POSTGRESQL_MAJOR postgresql" \
        "pg_ctl postgresql-$TOOL_PIN_POSTGRESQL_MAJOR postgresql" \
        "postgres postgresql-$TOOL_PIN_POSTGRESQL_MAJOR postgresql" \
        "psql postgresql-client-$TOOL_PIN_POSTGRESQL_MAJOR postgresql" \
        'ps procps path' \
        'kill procps path' \
        'curl curl path'
}

# The major version of the PostgreSQL whose programs are in directory $1, or nothing
# when its `initdb` does not say. It prints, for example,
# "initdb (PostgreSQL) 17.2 (Debian 17.2-1)": the major version is the number
# before the first dot.
major_version_of() {
    version_line="$("$1/initdb" --version)" || return 0
    printf '%s\n' "$version_line" | sed -n 's/^initdb (PostgreSQL) \([0-9][0-9]*\).*$/\1/p'
}

# Every PostgreSQL program directory this host has, one per line as
# "<major> <directory>".
postgresql_directories() {
    on_path=''
    if initdb_on_path="$(command -v initdb)"; then
        on_path="$(dirname "$initdb_on_path")"
    fi
    # An unmatched pattern stays as written and fails the test below, so a host
    # without Homebrew or without Debian's layout simply contributes nothing.
    for directory in \
        /opt/homebrew/opt/postgresql*/bin \
        /usr/local/opt/postgresql*/bin \
        /usr/lib/postgresql/*/bin \
        "$on_path"; do
        if [ ! -x "$directory/initdb" ]; then
            continue
        fi
        major="$(major_version_of "$directory")"
        if [ -z "$major" ]; then
            echo "error: $directory/initdb did not say which PostgreSQL it is" >&2
            exit 1
        fi
        printf '%s %s\n' "$major" "$directory"
    done
}

# The directory of the pinned PostgreSQL major's programs, or nothing.
pinned_postgresql_directory() {
    postgresql_directories | while read -r major directory; do
        if [ "$major" = "$TOOL_PIN_POSTGRESQL_MAJOR" ]; then
            printf '%s\n' "$directory"
            break
        fi
    done
}

# What to say when the pinned major is not here: what is, and how to install it.
refuse_without_the_pinned_postgresql() {
    found="$(postgresql_directories | while read -r major directory; do
        printf ' %s (in %s)' "$major" "$directory"
    done)"
    echo "error: the tests run on PostgreSQL $TOOL_PIN_POSTGRESQL_MAJOR, the pinned major (scripts/tool-pins.sh), and this host has none" >&2
    if [ -n "$found" ]; then
        echo "       found:$found" >&2
    fi
    echo "       install: Debian postgresql-$TOOL_PIN_POSTGRESQL_MAJOR, macOS brew install postgresql@$TOOL_PIN_POSTGRESQL_MAJOR" >&2
    exit 1
}

# Whether $1 is a program on PATH. `command -v` answers about a shell builtin when
# one has the name, rather than the file a test executes; no program this list
# looks for on PATH is a builtin.
on_path() {
    command -v "$1" >/dev/null
}

case "${1:-}" in
--packages)
    host_programs | while read -r _program package _where; do
        printf '%s\n' "$package"
    done | sort -u
    exit 0
    ;;
--postgresql-bin)
    directory="$(pinned_postgresql_directory)"
    if [ -z "$directory" ]; then
        refuse_without_the_pinned_postgresql
    fi
    printf '%s\n' "$directory"
    exit 0
    ;;
'') ;;
*)
    echo "usage: $0 [--packages | --postgresql-bin]" >&2
    exit 2
    ;;
esac

postgresql_bin="$(pinned_postgresql_directory)"
if [ -z "$postgresql_bin" ]; then
    refuse_without_the_pinned_postgresql
fi
missing=''
# The list is read from a here-document rather than a pipe so the loop runs in this
# shell, and what it records in `missing` is still there after it.
while read -r program package where; do
    if [ "$where" = postgresql ]; then
        if [ -x "$postgresql_bin/$program" ]; then
            printf 'found %s in %s\n' "$program" "$postgresql_bin"
            continue
        fi
    elif on_path "$program"; then
        printf 'found %s on PATH\n' "$program"
        continue
    fi
    echo "missing: $program (Debian package $package)" >&2
    missing="$missing $package"
done <<EOF
$(host_programs)
EOF

if [ -n "$missing" ]; then
    echo "error: the tests run programs this host lacks; install:$missing" >&2
    echo "       (macOS: brew install postgresql@$TOOL_PIN_POSTGRESQL_MAJOR for the PostgreSQL ones)" >&2
    exit 1
fi
echo "every host program the tests run is here (PostgreSQL $(major_version_of "$postgresql_bin"), the pinned major)"
