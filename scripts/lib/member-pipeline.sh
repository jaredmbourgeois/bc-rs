# Sourced by each member's `scripts/verify.sh`: the steps every member's pipeline shares, held once.
#
# A member's pipeline is its own file, because what it checks is its own — its feature matrix, its dependency guards,
# the tiers only it has. What every member does the same way lives here: its command line, the step headings, the test
# runner, the test-collection guard, the wasm32 target probe, the dependency reader, and the verdict that names every
# tier the run skipped (scripts/skip-verdict.sh). Six copies of these once held one bash 3.2 defect six times.
#
# Each function takes what it reads as arguments. The run's own record — what it skipped, and what its caller accepted
# — is the four `pipeline_*` arrays below, which only these functions write.
#
# A member's command line, parsed by `member_pipeline_begin`:
#
#   scripts/verify.sh [--quick] [--accept-skip <tier>=<why>]...
#   scripts/verify.sh --skippable-tiers
#
#   --quick                     leave out the release-profile test run; the verdict names it, accepted by asking
#   --accept-skip <tier>=<why>  accept that <tier> does not run here, saying why
#   --skippable-tiers           print the tiers this member may skip, one per line, and exit — the root pipeline asks,
#                               so it can route each acceptance to the members that have the tier
#
# A skipped tier nobody accepted ends the run INCOMPLETE (exit 3), never as a pass.

member_pipeline_workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"

quick_mode=false
member_packages=()
pipeline_skippable_tiers=''
pipeline_skipped_tiers=()
pipeline_skipped_reasons=()
pipeline_accepted_tiers=()
pipeline_accepted_reasons=()

step() {
    printf '\n\033[1m==> %s\033[0m\n' "${1:?title}"
}

# Whether the word $1 is one of the space-separated words in $2.
is_one_of() {
    local word="${1:?word}" words="${2:?words}"
    case " $words " in
        *" $word "*) return 0 ;;
        *) return 1 ;;
    esac
}

# `member_pipeline_begin <repository-root> <skippable-tiers> [<argument>...]`: refuses a member that is not in bc-rs,
# then reads the command line. The tiers are space-separated; `release-tests`, which `--quick` skips, is every
# member's.
member_pipeline_begin() {
    # The tiers may be empty — a member with none of its own — but are always passed.
    local repository_root="${1:?repository root}" declared_tiers="${2?skippable tiers}"
    shift 2
    pipeline_skippable_tiers="release-tests $declared_tiers"

    local tier
    if [[ $# -eq 1 && "$1" == --skippable-tiers ]]; then
        for tier in $pipeline_skippable_tiers; do
            printf '%s\n' "$tier"
        done
        exit 0
    fi

    # The workspace Cargo resolves for this member, against the one these scripts came from: a member copied out of
    # bc-rs, or reached through a path that leads elsewhere, would otherwise run every tier against the wrong root.
    local resolved_workspace
    if ! resolved_workspace="$(cd "$repository_root" && cargo locate-project --workspace --message-format plain)"; then
        echo "error: cargo could not locate the workspace $repository_root belongs to." >&2
        exit 1
    fi
    if [[ "$(cd "$(dirname "$resolved_workspace")" && pwd -P)" != "$member_pipeline_workspace_root" ]]; then
        echo "error: $repository_root resolves to the workspace $resolved_workspace," >&2
        echo "       not bc-rs at $member_pipeline_workspace_root; run it from inside bc-rs." >&2
        exit 1
    fi

    # This member's packages as Cargo resolves them, as `-p <name>` arguments: for a member with more than one, so a
    # step that must cover all of them names each, where `--workspace` would mean every member of bc-rs. A list kept
    # by hand here would be a third list to fall out of step with the root manifest.
    local package_names name
    if ! package_names="$(python3 "$member_pipeline_workspace_root/scripts/workspace-members.py" packages \
        "$member_pipeline_workspace_root" "$repository_root")"; then
        echo "error: the packages of $repository_root could not be read from the workspace." >&2
        exit 1
    fi
    member_packages=()
    while IFS= read -r name; do
        [[ -n "$name" ]] && member_packages+=(-p "$name")
    done <<< "$package_names"

    local reason
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --quick)
                quick_mode=true
                shift
                ;;
            --accept-skip)
                if [[ $# -lt 2 || "$2" != *=* ]]; then
                    echo 'error: --accept-skip takes <tier>=<why>' >&2
                    exit 64
                fi
                tier="${2%%=*}"
                reason="${2#*=}"
                if ! is_one_of "$tier" "$pipeline_skippable_tiers" || [[ -z "$reason" ]]; then
                    echo "error: --accept-skip $2: the tier is one of: $pipeline_skippable_tiers; and the why is not empty" >&2
                    exit 64
                fi
                pipeline_accepted_tiers+=("$tier")
                pipeline_accepted_reasons+=("$reason")
                shift 2
                ;;
            *)
                echo "error: unknown argument '$1'; usage: scripts/verify.sh [--quick] [--accept-skip <tier>=<why>]..." >&2
                exit 64
                ;;
        esac
    done
}

# Records that tier $1 did not run, and why ($2), and says so by name.
skipped() {
    local tier="${1:?tier}" reason="${2:?reason}"
    if ! is_one_of "$tier" "$pipeline_skippable_tiers"; then
        echo "error: this pipeline skipped '$tier', which it does not declare among its skippable tiers" >&2
        exit 70
    fi
    printf 'SKIPPED: %s — %s\n' "$tier" "$reason"
    pipeline_skipped_tiers+=("$tier")
    pipeline_skipped_reasons+=("$reason")
}

# Under `--quick`, records the release-profile run as skipped and accepted by the asking, and answers yes; otherwise
# answers no, and the caller runs it.
release_tests_skipped_by_quick() {
    if [[ "$quick_mode" == true ]]; then
        skipped release-tests '--quick left out the release-profile run'
        pipeline_accepted_tiers+=(release-tests)
        pipeline_accepted_reasons+=('--quick')
        return 0
    fi
    return 1
}

# `run_tests <cargo test arguments>...`: through nextest, which gives each test a timeout (.config/nextest.toml) that
# plain `cargo test` cannot express. nextest is required in CI; without it locally the suite still runs, under cargo
# test, and the test-collection guard is what records the difference as a skip.
run_tests() {
    if command -v cargo-nextest >/dev/null 2>&1; then
        cargo nextest run "$@" --profile ci
    else
        echo "note: cargo-nextest not installed; falling back to 'cargo test'." >&2
        cargo test "$@"
    fi
}

# `collection_guard <every|integration> <cargo nextest list arguments>...` — refuses a test binary that collected zero
# tests (scripts/lib/empty-test-binaries.py says why). It is given the arguments the suite ran with, so it measures the
# binaries the suite actually ran. Without nextest it cannot list them, and the tier is a skip.
collection_guard() {
    local kinds="${1:?every or integration}"
    shift
    if ! command -v cargo-nextest >/dev/null 2>&1; then
        skipped test-collection 'cargo-nextest is not installed; cargo install --locked cargo-nextest'
        return 0
    fi
    local listing empty status
    if ! listing="$(cargo nextest list "$@" --message-format json)"; then
        echo 'error: cargo nextest could not list the test binaries, so the guard verified nothing.' >&2
        exit 1
    fi
    set +e
    empty="$(python3 "$member_pipeline_workspace_root/scripts/lib/empty-test-binaries.py" "$kinds" <<< "$listing")"
    status=$?
    set -e
    case "$status" in
        0) echo 'every compiled test binary collected at least one test' ;;
        1)
            echo 'error: these test binaries collected zero tests:' >&2
            printf '%s\n' "$empty" >&2
            echo '       Either they are gated out of the run — check the feature flags the suite ran with — or' >&2
            echo '       they hold no tests at all. Delete the binary or give it a test; there is no allowlist.' >&2
            exit 1
            ;;
        *) exit 1 ;;
    esac
}

# Whether the wasm32-unknown-unknown target is installed. Read into a variable and matched as a here-string: under
# pipefail a `| grep -q` stops reading at its match and fails the writer with SIGPIPE.
wasm32_target_installed() {
    local installed
    if ! installed="$(rustup target list --installed 2>/dev/null)"; then
        return 1
    fi
    grep -qx 'wasm32-unknown-unknown' <<< "$installed"
}

# `dependency_names <cargo tree arguments>...` — the crate names a `cargo tree` resolves, one per line, sorted and
# without repeats. A tree cargo could not resolve stops the pipeline, never reads as an empty graph.
dependency_names() {
    local tree line name names=''
    if ! tree="$(cargo tree --locked "$@" --prefix none)"; then
        echo "error: cargo tree $* failed, so the dependency guard resolved nothing." >&2
        return 1
    fi
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        name="${line%% *}"
        names="$names$name"$'\n'
    done <<< "$tree"
    if [[ -z "$names" ]]; then
        echo "error: cargo tree $* resolved no crate at all, not even the package it was asked about." >&2
        return 1
    fi
    printf '%s' "$names" | LC_ALL=C sort -u
}

# `unexpected_dependencies <allowed-names> <cargo tree arguments>...` — the resolved crates outside the
# space-separated allowed names, one per line; nothing when the graph keeps to them.
unexpected_dependencies() {
    local allowed="${1:?allowed names}" names name
    shift
    names="$(dependency_names "$@")" || return 1
    while IFS= read -r name; do
        [[ -z "$name" ]] && continue
        if ! is_one_of "$name" "$allowed"; then
            printf '%s\n' "$name"
        fi
    done <<< "$names"
}

# `run_bindings_smoke <repository-root> <swift|kotlin>` — the one step that crosses the ABI, in one language
# (scripts/bindings-smoke.sh). A host without the language's toolchain records the tier `bindings-<language>` as
# skipped; a toolchain that is present and fails is a failure.
run_bindings_smoke() {
    local repository_root="${1:?repository root}" language="${2:?language}" status
    set +e
    "$member_pipeline_workspace_root/scripts/bindings-smoke.sh" "$repository_root" "$language"
    status=$?
    set -e
    case "$status" in
        0) ;;
        3) skipped "bindings-$language" "this host has no $language toolchain" ;;
        *) exit 1 ;;
    esac
}

# The run's verdict: every tier it skipped, each accepted by name or not (scripts/skip-verdict.sh). It ends the run.
# The leading `--` keeps the argument list non-empty when nothing was skipped or accepted.
member_pipeline_verdict() {
    step 'Verdict'
    local verdict_arguments=(--) index
    for ((index = 0; index < ${#pipeline_skipped_tiers[@]}; index++)); do
        verdict_arguments+=(--skipped "${pipeline_skipped_tiers[index]}" "${pipeline_skipped_reasons[index]}")
    done
    for ((index = 0; index < ${#pipeline_accepted_tiers[@]}; index++)); do
        verdict_arguments+=(--accepted "${pipeline_accepted_tiers[index]}" "${pipeline_accepted_reasons[index]}")
    done
    exec "$member_pipeline_workspace_root/scripts/skip-verdict.sh" "${verdict_arguments[@]}"
}
