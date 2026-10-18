#!/usr/bin/env bash
# The workspace's one verification pipeline: the workspace's own checks, then every member's own scripts/verify.sh in
# dependency order, a verdict line for each, and one verdict for the run.
#
#   scripts/verify.sh [--quick] [--accept-skip <tier>=<why>]...
#
# `--quick` reaches every member, each of which then names its release-profile tests as skipped. `--accept-skip`
# reaches each member that declares the tier (`<member>/scripts/verify.sh --skippable-tiers` lists them), and one
# tier is this pipeline's own: `bash-floor`, the floor run below, on a host with no bash 3.2. A tier no member and no
# part of this pipeline declares is refused before anything runs.
#
# The bash floor: on a host whose /bin/bash is bash 3, this pipeline runs itself, and everything it starts, under that
# bash, and says which bash it reached (scripts/lib/bash-floor.sh says why). A host with none records it as skipped.
#
# Every workspace check and every member runs whatever an earlier one did, so one run names every verdict. The run's
# own verdict is the worst of them: FAIL when any failed, INCOMPLETE when any skipped a tier nobody accepted, and PASS
# only when every one passed — never healthier than its worst row.
#
# Exit: 0 PASS, 1 FAIL, 3 INCOMPLETE, 64 a malformed command line.

set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"

# --- the floor ---------------------------------------------------------------------------------------------------
# shellcheck source=lib/bash-floor.sh
. "$workspace_root/scripts/lib/bash-floor.sh"
floor_bash="$(bash_floor_find)"
bash_floor_enter "$floor_bash" "$workspace_root/scripts/verify.sh" "$@"
cd "$workspace_root"

# --- the command line ------------------------------------------------------------------------------------------------
quick_requested=false
accepted_tiers=()
accepted_reasons=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --quick)
            quick_requested=true
            shift
            ;;
        --accept-skip)
            if [[ $# -lt 2 || "$2" != *=* || -z "${2#*=}" ]]; then
                echo 'error: --accept-skip takes <tier>=<why>, and the why is not empty' >&2
                exit 64
            fi
            accepted_tiers+=("${2%%=*}")
            accepted_reasons+=("${2#*=}")
            shift 2
            ;;
        *)
            echo "error: unknown argument '$1'; usage: scripts/verify.sh [--quick] [--accept-skip <tier>=<why>]..." >&2
            exit 64
            ;;
    esac
done

step() {
    printf '\n\033[1m==> %s\033[0m\n' "${1:?title}"
}

# The rows of the final verdict, by index (bash 3.2 has no associative arrays): what was judged, and how it ended.
row_names=()
row_verdicts=()

# `record <name> <status>` — a row from a check's exit status: 0 PASS, 3 INCOMPLETE, anything else FAIL.
record() {
    local name="${1:?name}" status="${2:?status}"
    row_names+=("$name")
    case "$status" in
        0) row_verdicts+=(PASS) ;;
        3) row_verdicts+=(INCOMPLETE) ;;
        *) row_verdicts+=("FAIL (exit $status)") ;;
    esac
}

# `check <name> <command>...` — runs a workspace check and records its row, whatever it ends with.
check() {
    local name="${1:?name}" status
    shift
    step "$name"
    set +e
    "$@"
    status=$?
    set -e
    # A workspace check is pass or fail: exit 3 is a member's word for an incomplete run, not a check's.
    if [[ "$status" -eq 3 ]]; then
        status=1
    fi
    record "$name" "$status"
}

# --- the members, in dependency order, and the tiers each may skip -----------------------------------------------
step 'Members'
if ! member_list="$(python3 scripts/workspace-members.py order "$workspace_root")"; then
    echo 'error: the members could not be read, so no member was run.' >&2
    exit 1
fi
members=()
member_tiers=()
while IFS= read -r member; do
    [[ -z "$member" ]] && continue
    if ! tiers="$("$member/scripts/verify.sh" --skippable-tiers)"; then
        echo "error: $member/scripts/verify.sh could not say which tiers it may skip." >&2
        exit 1
    fi
    members+=("$member")
    # One line per tier, joined into one space-separated string padded at both ends, so a lookup matches a whole word.
    tier_words="$(tr '\n' ' ' <<< "$tiers")"
    member_tiers+=(" $tier_words")
    echo "$member may skip: $tier_words"
done <<< "$member_list"

# This pipeline's own tier.
workspace_tiers=' bash-floor '

# Every accepted tier is one some member, or this pipeline, can skip: a typo would otherwise be accepted by nobody and
# reported, five pipelines later, as a member's usage error.
index=0
while [[ "$index" -lt "${#accepted_tiers[@]}" ]]; do
    tier="${accepted_tiers[$index]}"
    known=false
    [[ "$workspace_tiers" == *" $tier "* ]] && known=true
    member_index=0
    while [[ "$member_index" -lt "${#members[@]}" ]]; do
        [[ "${member_tiers[$member_index]}" == *" $tier "* ]] && known=true
        member_index=$((member_index + 1))
    done
    if [[ "$known" == false ]]; then
        echo "error: --accept-skip $tier: no member, and nothing in this pipeline, has a tier by that name" >&2
        exit 64
    fi
    index=$((index + 1))
done

# --- the workspace's own checks ----------------------------------------------------------------------------------
step 'The bash floor'
workspace_skipped=(--)
if [[ -n "$floor_bash" ]]; then
    echo "every script below runs under $floor_bash $BASH_VERSION, first on PATH"
else
    reason="this host has no bash 3 at /bin/bash, so the floor runs where one does (macOS)"
    echo "SKIPPED: bash-floor — $reason"
    workspace_skipped+=(--skipped bash-floor "$reason")
fi
index=0
while [[ "$index" -lt "${#accepted_tiers[@]}" ]]; do
    if [[ "$workspace_tiers" == *" ${accepted_tiers[$index]} "* ]]; then
        workspace_skipped+=(--accepted "${accepted_tiers[$index]}" "${accepted_reasons[$index]}")
    fi
    index=$((index + 1))
done

check 'Skip verdict selftest' scripts/skip-verdict.sh --selftest

step 'The workspace skips'
set +e
scripts/skip-verdict.sh "${workspace_skipped[@]}"
workspace_skip_status=$?
set -e
record 'Workspace skips' "$workspace_skip_status"

# --- every member ------------------------------------------------------------------------------------------------
member_index=0
while [[ "$member_index" -lt "${#members[@]}" ]]; do
    member="${members[$member_index]}"
    # The member's arguments: --quick if asked, and each accepted tier this member declares. The leading `--quick`
    # word, or the routed ones, are counted before the array is expanded (bash 3.2 reads an empty one as unset).
    arguments=()
    if [[ "$quick_requested" == true ]]; then
        arguments+=(--quick)
    fi
    index=0
    while [[ "$index" -lt "${#accepted_tiers[@]}" ]]; do
        if [[ "${member_tiers[$member_index]}" == *" ${accepted_tiers[$index]} "* ]]; then
            arguments+=(--accept-skip "${accepted_tiers[$index]}=${accepted_reasons[$index]}")
        fi
        index=$((index + 1))
    done
    step "Member: $member"
    set +e
    if [[ "${#arguments[@]}" -gt 0 ]]; then
        "$member/scripts/verify.sh" "${arguments[@]}"
    else
        "$member/scripts/verify.sh"
    fi
    member_status=$?
    set -e
    record "$member" "$member_status"
    member_index=$((member_index + 1))
done

# --- the verdict -----------------------------------------------------------------------------------------------------
step 'Verdict'
run_verdict=PASS
index=0
while [[ "$index" -lt "${#row_names[@]}" ]]; do
    printf '  %-32s %s\n' "${row_names[$index]}" "${row_verdicts[$index]}"
    case "${row_verdicts[$index]}" in
        FAIL*)
            run_verdict=FAIL
            ;;
        INCOMPLETE)
            if [[ "$run_verdict" == PASS ]]; then
                run_verdict=INCOMPLETE
            fi
            ;;
    esac
    index=$((index + 1))
done

case "$run_verdict" in
    PASS)
        printf '\n\033[1;32m==> bc-rs verify: every check and every member passed\033[0m\n'
        ;;
    INCOMPLETE)
        printf '\n\033[1;33m==> bc-rs verify: INCOMPLETE — a tier was skipped and nobody accepted that\033[0m\n'
        exit 3
        ;;
    FAIL)
        printf '\n\033[1;31m==> bc-rs verify: FAIL\033[0m\n'
        exit 1
        ;;
esac
