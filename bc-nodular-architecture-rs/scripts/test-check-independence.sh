#!/usr/bin/env bash
# Self-tests for check-independence.sh: it must accept the real source tree and
# reject each violation it exists to catch.

set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
guard="$repository_root/scripts/check-independence.sh"

fixture_root="$(mktemp -d "${TMPDIR:-/tmp}/bc-nodular-independence.XXXXXX")"
trap 'rm -rf "$fixture_root"' EXIT

assert_rejects() {
    local name="$1" contents="$2" expected="$3"
    local directory="$fixture_root/$name"
    mkdir -p "$directory"
    printf '%s\n' "$contents" > "$directory/example.rs"

    local output status
    output="$("$guard" "$directory" 2>&1)" && status=0 || status=$?
    if [[ "$status" -eq 0 ]]; then
        printf 'selftest %s: the guard accepted a violation\n' "$name" >&2
        exit 1
    fi
    if [[ "$output" != *"$expected"* ]]; then
        printf 'selftest %s: missing diagnostic %q in:\n%s\n' "$name" "$expected" "$output" >&2
        exit 1
    fi
}

assert_rejects runtime-crate \
    'use tokio::sync::mpsc;' \
    'must not name an async runtime'

assert_rejects async-fn \
    'pub async fn run() {}' \
    'must stay synchronous'

assert_rejects await-point \
    'fn poll() { receiver.recv().await; }' \
    'must stay synchronous'

assert_rejects stray-lock \
    'use std::sync::Mutex;' \
    'only src/node.rs may hold a lock'

assert_rejects stray-atomic \
    'use std::sync::atomic::AtomicBool;' \
    'may synchronize threads'

assert_rejects stray-channel \
    'use std::sync::mpsc;' \
    'may synchronize threads'

# A lock inside node.rs itself is the one permitted case.
mkdir -p "$fixture_root/permitted"
printf 'use std::sync::Mutex;\n' > "$fixture_root/permitted/node.rs"
if ! "$guard" "$fixture_root/permitted" >/dev/null 2>&1; then
    printf 'selftest permitted-lock: the guard rejected node.rs holding a lock\n' >&2
    exit 1
fi

# The channel and the counter are permitted only in the files whose contracts
# need them.
mkdir -p "$fixture_root/permitted-sync"
printf 'use std::sync::mpsc::sync_channel;\n' > "$fixture_root/permitted-sync/node_loop.rs"
printf 'use std::sync::atomic::AtomicU64;\n' > "$fixture_root/permitted-sync/model.rs"
if ! "$guard" "$fixture_root/permitted-sync" >/dev/null 2>&1; then
    printf 'selftest permitted-sync: the guard rejected node_loop.rs/model.rs synchronizing\n' >&2
    exit 1
fi

if ! "$guard" >/dev/null 2>&1; then
    printf 'selftest source-tree: the guard rejected the real source tree\n' >&2
    exit 1
fi

# The default (no-argument) run also asserts what it is NOT scanning. Those
# checks key off the repository layout, so they are exercised by copying the
# guard into a fixture repository rather than by pointing it at a directory.
make_repository() {
    local name="$1"
    local repository="$fixture_root/$name"
    mkdir -p "$repository/scripts" "$repository/src" "$repository/crates/tokio/src"
    cp "$guard" "$repository/scripts/check-independence.sh"
    printf 'use std::sync::Mutex;\n' > "$repository/src/node.rs"
    printf 'pub fn step() {}\n' > "$repository/src/node_loop.rs"
    printf 'pub fn advance() {}\n' > "$repository/src/node_result.rs"
    printf 'use tokio::sync::mpsc;\n' > "$repository/crates/tokio/src/lib.rs"
    printf '%s\n' "$repository"
}

assert_repository_rejects() {
    local name="$1" repository="$2" expected="$3"
    local output status
    output="$("$repository/scripts/check-independence.sh" 2>&1)" && status=0 || status=$?
    if [[ "$status" -eq 0 ]]; then
        printf 'selftest %s: the guard accepted a violation\n' "$name" >&2
        exit 1
    fi
    if [[ "$output" != *"$expected"* ]]; then
        printf 'selftest %s: missing diagnostic %q in:\n%s\n' "$name" "$expected" "$output" >&2
        exit 1
    fi
}

intact="$(make_repository intact)"
if ! "$intact/scripts/check-independence.sh" >/dev/null 2>&1; then
    printf 'selftest intact-repository: the guard rejected a well-formed layout\n' >&2
    exit 1
fi

# A scan with nothing to look at passes every rule vacuously, which is the one
# way this guard can report success while checking nothing at all.
empty="$(make_repository empty-scan)"
rm "$empty"/src/*.rs
assert_repository_rejects empty-scan "$empty" 'found no Rust sources under'

absent="$(make_repository absent-adapter)"
rm -r "$absent/crates/tokio/src"
assert_repository_rejects absent-adapter "$absent" 'tokio adapter is missing'

# The exclusion only earns its keep while the excluded tree really is the one
# holding the runtime dependency.
vacuous="$(make_repository vacuous-exclusion)"
printf 'pub fn nothing() {}\n' > "$vacuous/crates/tokio/src/lib.rs"
assert_repository_rejects vacuous-exclusion "$vacuous" "exclusion is now vacuous"

printf '\033[1;32m==> independence selftests: all checks passed\033[0m\n'
