#!/usr/bin/env bash
# Guards the invariants the tests cannot see: the library mandates no async
# runtime, it holds exactly one lock, in the one type whose contract is shared
# mutable state, and every other thread-coordination primitive is confined to
# the one file whose contract needs it.
#
# Takes the directory to scan so the self-test can point it at fixtures.

set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scan_directory="${1:-$repository_root/src}"

failures=0

fail() {
    printf '  \033[31m[FAIL]\033[0m %s\n' "$1"
    failures=$((failures + 1))
}

# `matches_of <directory> <extended-regex>` — every line under <directory>'s Rust sources that matches, as
# `<file>:<line>:<text>`; nothing when none does. grep's exit 1 is that answer; its exit 2 is a tree it could not
# read, which stops the guard rather than reading as "no match" (a loop fed by `< <(grep … || true)` read it so).
matches_of() {
    local directory="${1:?directory}" pattern="${2:?pattern}" found status
    set +e
    found="$(grep -rnE "$pattern" "$directory" --include='*.rs')"
    status=$?
    set -e
    if [[ "$status" -gt 1 ]]; then
        printf 'independence: could not scan %s\n' "$directory" >&2
        exit 1
    fi
    printf '%s' "$found"
}

# Runtime independence: no runtime crate, and no async surface of any kind.
# `async` and `await` are matched as whole words so ordinary prose in a doc
# comment does not trip the guard.
runtime_matches="$(matches_of "$scan_directory" 'tokio|async-std|smol')"
while IFS= read -r match; do
    [[ -z "$match" ]] && continue
    fail "src must not name an async runtime: $match"
done <<< "$runtime_matches"

async_matches="$(matches_of "$scan_directory" '\basync fn\b|\.await\b|\basync move\b')"
while IFS= read -r match; do
    [[ -z "$match" ]] && continue
    fail "src must stay synchronous: $match"
done <<< "$async_matches"

# Lock discipline: `Node` owns the crate's only lock, because sharing one
# nodule's state across threads is its whole contract. Anything else is a
# design regression, not a detail.
lock_matches="$(matches_of "$scan_directory" '\b(Mutex|RwLock)\b')"
while IFS= read -r match; do
    case "$match" in
        '') ;;
        */node.rs:*) ;;
        *) fail "only src/node.rs may hold a lock: $match" ;;
    esac
done <<< "$lock_matches"

# Synchronization discipline, the wider form of the same rule: every other
# primitive that coordinates threads is likewise confined to the file whose
# contract needs it. `node_loop.rs` owns the thread runner's bounded channel,
# `model.rs` owns the event-sequence counter, and nothing else in the port may
# name an atomic, a channel, or a condition variable. A stray one is either a
# new cross-thread contract (which belongs in this list, with a reason) or a
# design regression. The `mpsc` module name is matched as a path segment so
# that a `sync_channel` alias and a bare `mpsc::` import are both caught.
synchronization_matches="$(matches_of "$scan_directory" \
    '\b(Atomic(Bool|Usize|Isize|U8|U16|U32|U64|I8|I16|I32|I64|Ptr)|Condvar|Barrier|sync_channel|OnceLock|LazyLock)\b|\bmpsc\b')"
while IFS= read -r match; do
    case "$match" in
        '') ;;
        */node_loop.rs:*) ;;
        */model.rs:*) ;;
        *) fail "only src/node_loop.rs and src/model.rs may synchronize threads: $match" ;;
    esac
done <<< "$synchronization_matches"

# The scan deliberately covers the ported crate's `src` and nothing else, so the
# workspace's tokio adapter is invisible to it. That exclusion is only safe while
# it is deliberate, so the default run proves three things about it: the scan has
# something to look at, the adapter exists where it is expected, and the adapter
# genuinely contains what is being excluded. If the adapter ever stops naming
# tokio, this exclusion has become vacuous and should be removed rather than left
# as decoration.
if [[ "$#" -eq 0 ]]; then
    if [[ -z "$(find "$scan_directory" -name '*.rs' -print -quit 2>/dev/null)" ]]; then
        fail "the scan found no Rust sources under $scan_directory; it is checking nothing"
    fi

    adapter="$repository_root/crates/tokio/src"
    if [[ ! -d "$adapter" ]]; then
        fail "the tokio adapter is missing from crates/tokio/src"
    elif ! grep -rq 'tokio' "$adapter" --include='*.rs'; then
        fail "crates/tokio no longer names tokio; this guard's exclusion is now vacuous"
    fi
fi

if [[ "$failures" -ne 0 ]]; then
    printf '\033[1;31m==> independence: %d check(s) failed\033[0m\n' "$failures"
    exit 1
fi

printf '\033[1;32m==> independence: all checks passed\033[0m\n'
