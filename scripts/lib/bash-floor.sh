# shellcheck shell=bash
# Sourced by a pipeline entry point (scripts/verify.sh, and the working line's internal checks): run that script, and
# everything it starts, under the bash floor.
#
# THE BASH FLOOR. Every script here is `#!/usr/bin/env bash`, which takes whatever bash is first on PATH; macOS ships
# 3.2 at /bin/bash, and a GUI git client's PATH or the IDE reaches it first. A newer bash on a developer's PATH would
# hide every 3.2 break where the scripts are usually run (CONTRIBUTING.md says which). So on a host whose /bin/bash is
# bash 3, an entry point runs itself, and everything it starts, under that bash — first on PATH — and says which bash
# it reached. A host with none (a Linux runner) records the floor as skipped.

# `bash_floor_find` — prints /bin/bash when it is bash 3, and nothing on a host without one.
bash_floor_find() {
    local floor_major
    if [[ -x /bin/bash ]] && floor_major="$(/bin/bash -c 'printf %s "${BASH_VERSINFO[0]}"')" \
        && [[ "$floor_major" == 3 ]]; then
        printf '%s' /bin/bash
    fi
}

# `bash_floor_enter <floor-bash-or-empty> <entry-script> [<argument>...]` — when there is a floor and this run is not
# under it yet, re-runs <entry-script> under it with a directory holding only `bash` first on PATH, so everything the
# script starts takes the floor too. The run that made the directory removes it when it exits: `exec` keeps the
# process id, so the owner is recorded before it, and an entry point another entry point starts under the floor
# leaves its parent's directory alone.
bash_floor_enter() {
    local floor="${1?the floor bash, or empty}" entry_script="${2:?entry script}" floor_directory
    shift 2
    if [[ -n "$floor" && -z "${BC_RS_FLOOR_PATH:-}" ]]; then
        floor_directory="$(mktemp -d "${TMPDIR:-/tmp}/bc-rs-floor-bash.XXXXXX")"
        ln -s "$floor" "$floor_directory/bash"
        export BC_RS_FLOOR_PATH="$floor_directory"
        export BC_RS_FLOOR_OWNER="$$"
        export PATH="$floor_directory:$PATH"
        exec "$floor" "$entry_script" "$@"
    fi
    if [[ -n "${BC_RS_FLOOR_PATH:-}" && "${BC_RS_FLOOR_OWNER:-}" == "$$" ]]; then
        trap 'rm -rf "$BC_RS_FLOOR_PATH"' EXIT
    fi
}
