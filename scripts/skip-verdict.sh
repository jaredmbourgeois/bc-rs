#!/usr/bin/env bash
#
# The verdict of a verify run that may not have run every tier.
#
# Every member's scripts/verify.sh runs some tiers only where what they need is here —
# the wasm32 target, cargo-nextest, a Swift or Kotlin toolchain, Node and the TypeScript
# SDK, the pinned Grafana — and says by name when one did not run
# (scripts/lib/member-pipeline.sh). A run that skipped a tier has not passed it, so
# "all checks passed" is said only when nothing was skipped. A caller may accept a
# skip by saying why: CI's workflow names each skip it accepts and why, and a local run
# names its own. There is no default list, so a skip nobody accepted is never read as
# a pass.
#
# Usage:
#   scripts/skip-verdict.sh -- [--skipped <tier> <reason>]... [--accepted <tier> <reason>]...
#   scripts/skip-verdict.sh --selftest
#
# The leading `--` is always passed, so a run that skipped and accepted nothing still
# hands this script a non-empty argument list (bash 3.2 reads an empty array as unset).
#
# Exit codes:
#   0  nothing was skipped, or every skip was accepted — each named in the verdict
#   1  a tier was accepted as skipped and ran anyway: the acceptance is stale, drop it
#   3  a tier was skipped and nobody accepted that: the run is incomplete, never a pass
#   64 a malformed command line

set -euo pipefail

usage() {
    echo 'usage: scripts/skip-verdict.sh -- [--skipped <tier> <reason>]... [--accepted <tier> <reason>]...' >&2
    echo '       scripts/skip-verdict.sh --selftest' >&2
    exit 64
}

# Whether the tier $1 is among the words after it.
is_among() {
    local wanted="${1:?tier}"
    shift
    local candidate
    for candidate in "$@"; do
        if [[ "$candidate" == "$wanted" ]]; then
            return 0
        fi
    done
    return 1
}

judge() {
    # Parallel indexed arrays, walked by index: bash 3.2 has no associative ones.
    local skipped_tiers=() skipped_reasons=() accepted_tiers=() accepted_reasons=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --skipped | --accepted)
                if [[ $# -lt 3 ]]; then
                    usage
                fi
                if [[ "$1" == --skipped ]]; then
                    skipped_tiers+=("$2")
                    skipped_reasons+=("$3")
                else
                    accepted_tiers+=("$2")
                    accepted_reasons+=("$3")
                fi
                shift 3
                ;;
            *) usage ;;
        esac
    done

    # A sentinel ahead of each list, so `"${list[@]}"` is never an empty array.
    local skipped_words=(-) accepted_words=(-)
    local index
    for ((index = 0; index < ${#skipped_tiers[@]}; index++)); do
        skipped_words+=("${skipped_tiers[index]}")
    done
    for ((index = 0; index < ${#accepted_tiers[@]}; index++)); do
        accepted_words+=("${accepted_tiers[index]}")
    done

    local unaccepted=0 stale=0
    for ((index = 0; index < ${#skipped_tiers[@]}; index++)); do
        if ! is_among "${skipped_tiers[index]}" "${accepted_words[@]}"; then
            unaccepted=$((unaccepted + 1))
        fi
    done
    for ((index = 0; index < ${#accepted_tiers[@]}; index++)); do
        if ! is_among "${accepted_tiers[index]}" "${skipped_words[@]}"; then
            stale=$((stale + 1))
        fi
    done

    if [[ $unaccepted -gt 0 ]]; then
        printf '\n\033[1;31m==> verify: INCOMPLETE — these tiers did not run, and nobody accepted that:\033[0m\n'
        for ((index = 0; index < ${#skipped_tiers[@]}; index++)); do
            if ! is_among "${skipped_tiers[index]}" "${accepted_words[@]}"; then
                printf '    %s — %s\n' "${skipped_tiers[index]}" "${skipped_reasons[index]}"
            fi
        done
        echo '    A tier that cannot run here is accepted with --accept-skip <tier>=<why>.'
    fi
    if [[ $stale -gt 0 ]]; then
        printf '\n\033[1;31m==> verify: an accepted skip ran — drop its --accept-skip:\033[0m\n'
        for ((index = 0; index < ${#accepted_tiers[@]}; index++)); do
            if ! is_among "${accepted_tiers[index]}" "${skipped_words[@]}"; then
                printf '    %s — accepted because %s\n' "${accepted_tiers[index]}" "${accepted_reasons[index]}"
            fi
        done
    fi
    if [[ $unaccepted -gt 0 ]]; then
        return 3
    fi
    if [[ $stale -gt 0 ]]; then
        return 1
    fi

    if [[ ${#skipped_tiers[@]} -eq 0 ]]; then
        printf '\n\033[1;32m==> verify: all checks passed\033[0m\n'
        return 0
    fi
    printf '\n\033[1;33m==> verify: passed, with accepted skips:\033[0m\n'
    for ((index = 0; index < ${#skipped_tiers[@]}; index++)); do
        local tier="${skipped_tiers[index]}" reason=''
        local accepted_index
        for ((accepted_index = 0; accepted_index < ${#accepted_tiers[@]}; accepted_index++)); do
            if [[ "${accepted_tiers[accepted_index]}" == "$tier" ]]; then
                reason="${accepted_reasons[accepted_index]}"
            fi
        done
        printf '    %s — %s (accepted: %s)\n' "$tier" "${skipped_reasons[index]}" "$reason"
    done
    return 0
}

# Each verdict reached once, by its exit code and its words; a case that does not
# reach its own fails the selftest, so no branch above is unproven.
selftest() {
    local failures=0
    expect() {
        local name="${1:?name}" wanted_status="${2:?status}" wanted_words="${3:?words}"
        shift 3
        local output status=0
        output="$(judge "$@" 2>&1)" || status=$?
        if [[ "$status" == "$wanted_status" ]] && grep -qF -- "$wanted_words" <<< "$output"; then
            echo "  [ok] $name"
        else
            echo "  [FAIL] $name: exit $status, wanted $wanted_status and \"$wanted_words\"; said:"
            printf '%s\n' "$output" | sed 's/^/         /'
            failures=$((failures + 1))
        fi
    }
    expect 'nothing skipped is a pass' 0 'all checks passed'
    expect 'an accepted skip passes, named with why' 0 'grafana — not fetched (accepted: the runner has none)' \
        --skipped grafana 'not fetched' --accepted grafana 'the runner has none'
    expect 'a skip nobody accepted is incomplete' 3 'INCOMPLETE' \
        --skipped bindings-swift 'swiftc is absent'
    expect 'an incomplete run names the unaccepted tier alone' 3 'bindings-swift — swiftc is absent' \
        --skipped grafana 'not fetched' --accepted grafana 'owed' --skipped bindings-swift 'swiftc is absent'
    expect 'an acceptance whose tier ran is stale' 1 'grafana — accepted because owed' \
        --accepted grafana 'owed'
    expect 'an incomplete run outranks a stale acceptance' 3 'an accepted skip ran' \
        --skipped bindings-swift 'absent' --accepted grafana 'owed'
    expect 'a malformed command line is a usage error' 64 'usage:' --skipped grafana
    if [[ $failures -gt 0 ]]; then
        echo "skip-verdict selftest: $failures case(s) failed" >&2
        return 1
    fi
    echo 'skip-verdict selftest: passed'
}

case "${1:-}" in
    --selftest) selftest ;;
    --)
        shift
        judge "$@"
        ;;
    *) usage ;;
esac
