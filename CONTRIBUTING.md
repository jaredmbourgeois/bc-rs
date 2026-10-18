# Contributing to bc-rs

bc-rs does not take outside contributions yet, and it has no private channel for security reports yet; when either
opens, this file will say how. Until then it states how the workspace is built and checked, for anyone reading or
building the code.

## Working rules

- **One pipeline.** `scripts/verify.sh` runs the workspace's own checks, then every member's own `scripts/verify.sh`
  in dependency order — the members and their order read from Cargo by `scripts/workspace-members.py`, never kept by
  hand — and ends with one verdict line per check and per member and one for the run: FAIL when any failed,
  INCOMPLETE when one skipped a tier nobody accepted, PASS only when all passed. CI runs it on the minimum supported
  Rust version and one pinned stable release, and the tracked `.idea/runConfigurations/` run it too. `--quick` leaves
  out the release-mode test runs, and the verdicts say so.

  ```sh
  scripts/verify.sh                                  # every member, every tier this host can run
  scripts/verify.sh --quick                          # without the release-mode test runs
  scripts/verify.sh --accept-skip bindings-kotlin='no Kotlin toolchain here'
  ```

- **One member pipeline, stated once.** The steps every member shares — its command line, the test runner, the
  test-collection guard, the wasm32 probe, the dependency reader and the verdict — are `scripts/lib/member-pipeline.sh`;
  a member's `scripts/verify.sh` holds only what is its own.
- **A skip is never a pass.** A tier that cannot run here — the wasm32 target, cargo-nextest, a Swift or Kotlin
  toolchain, a member's own tools — says so by name, and its member's run is INCOMPLETE (exit 3) unless the caller
  accepted that tier with `--accept-skip <tier>=<why>` (`scripts/skip-verdict.sh`). The root pipeline refuses an
  acceptance no member's tiers name, and routes each to the members that have it.
- **One toolchain, one policy.** `rust-toolchain.toml` pins the minimum supported Rust version that the root
  `[workspace.package]` declares, and the root `[workspace.lints]` is the one lint policy every member inherits.
  `unsafe` is forbidden in every member. A member reaches another only through `{ workspace = true }`, so its own
  manifest names no path out of its directory, and every version stays its own.

## The scripts' rules

Every script here holds them:

- **The bash floor is 3.2.** The scripts are `#!/usr/bin/env bash`, which takes whatever bash is first on `PATH`;
  macOS ships 3.2, and a GUI git client or the IDE reaches it first. So no `declare -A` (walk parallel indexed arrays
  by index), and no `"${array[@]}"` on an array that may be empty (3.2 reads it as unbound under `set -u`; count it
  first, or pass it as arguments, since `"$@"` is safe). On a host whose `/bin/bash` is 3.x, `scripts/verify.sh` runs
  itself and everything it starts under it, and says which bash it reached (`scripts/lib/bash-floor.sh`).
- **Under `pipefail`, a pipeline's reader reads to the end.** `grep -q`, `head` and `grep -m` stop early, and the
  writer then dies of SIGPIPE, which pipefail reports as the pipeline failing. Hand such a reader a here-string or a
  file.
- **No loop over a process substitution.** `done < <(command)` loses the command's exit status, so a reader that
  fails feeds the loop nothing and every check inside it passes. Capture the output with its status, then read it
  through a here-string.
- **No default inside a function.** A function's caller passes every argument; a required one is `"${1:?name}"`. A
  script's own top-level argument parsing is its command line and may probe `${1:-}`.
- **Readable by a non-specialist.** No `awk` programs, no parameter-expansion golf, named pipeline stages, and an
  explicit commented sequence over a clever one-liner. Structured data — JSON, a syntax to parse — is Python's, with
  its exit status checked where the shell reads it.
