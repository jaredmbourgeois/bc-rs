#!/usr/bin/env bash
#
# This member's full verification pipeline, stated once.
#
# bc-rs's root scripts/verify.sh runs it for every CI run and for the tracked RustRover run configurations, so "green
# locally" and "green in CI" cannot drift apart by editing one and forgetting the other. The steps every member shares
# — its command line, the test runner, the test-collection guard and the verdict — are bc-rs's
# scripts/lib/member-pipeline.sh; what is here is this member's own.
#
# Three packages with different contracts: the port, whose dependency set is pinned below and which names no TOML
# parser; `bc-telemetry-gen` in crates/gen, a generator that is not a port and has its own pinned set; and
# `bc-telemetry-dashboard` in crates/dashboard, the dashboard model the generator renders through, pinned to the
# family crates alone. Steps that just have to be green run over the three packages (`member_packages`, which
# scripts/lib/member-pipeline.sh reads from Cargo); each dependency guard names the package it pins.
#
# Usage: scripts/verify.sh [--quick] [--accept-skip <tier>=<why>]...   (scripts/lib/member-pipeline.sh says what each
# takes). Some tiers run only where what they need is here, and say by name when they do not: typescript, grafana,
# test-collection, and release-tests under --quick. The run then ends INCOMPLETE (exit 3)
# unless every skip was accepted, and "all checks passed" only when nothing was skipped (scripts/skip-verdict.sh).
# There is no default list: CI's workflow names the skips it accepts.

set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
# The bc-rs workspace this member sits in, whose shared scripts every member's pipeline uses.
workspace_root="$(cd "$repository_root/.." && pwd -P)"
# shellcheck source=../../scripts/lib/member-pipeline.sh
. "$workspace_root/scripts/lib/member-pipeline.sh"
member_pipeline_begin "$repository_root" 'typescript grafana test-collection' "$@"
cd "$repository_root"

# Every test invocation shares these flags, and so does the test-collection
# guard below — which is the point. `--all-features` is not optional: without it
# a `#![cfg(feature = "...")]` integration test compiles under clippy and is
# silently dropped from the run. That is load-bearing here: the `otlp`,
# `postgres` and `batched` suites each run only because of it.
#
# `--lib --tests` rather than `--all-targets`: every library's unit tests and
# every integration suite, and not the generator's binary, which is argv handed
# to the library's `run` and holds no test — `--all-targets` would compile it a
# harness that collects nothing, which the guard below rightly refuses. The
# binary still compiles under every lint and build step, which do take
# `--all-targets`.
test_args=(--locked "${member_packages[@]}" --lib --tests --all-features)

step 'Formatting'
cargo fmt --check "${member_packages[@]}"

step 'Host programs'
# The PostgreSQL suites start servers of their own, from programs that are often
# not on PATH; scripts/check-test-host-programs.sh says where they are found. It
# names anything missing before a test starts, and the pinned PostgreSQL
# major's programs then go first on PATH, so the suite runs the version it
# reported.
scripts/check-test-host-programs.sh
postgresql_bin="$(scripts/check-test-host-programs.sh --postgresql-bin)"
export PATH="$postgresql_bin:$PATH"

step 'Tests (debug)'
run_tests "${test_args[@]}"

if ! release_tests_skipped_by_quick; then
    step 'Tests (release)'
    # Debug and release must agree: the property tests walk wide signed epoch
    # ranges through multiply-and-add arithmetic whose overflow behavior differs
    # between the two profiles.
    run_tests "${test_args[@]}" --release
fi

step 'TypeScript query handles'
# The generator's TypeScript proof builds a dashboard with Grafana's Foundation
# SDK from the generated handles (crates/gen/tests/suite/typescript.rs). It needs
# Node and the SDK, which scripts/fetch-typescript-sdk.sh fetches from a
# committed lockfile, so the suite ignores it and this step runs it where both
# are here. Where they are not it says so by name: a skipped proof is reported,
# never passed. And a run that ran nothing is a failure, not a pass.
typescript_sdk=crates/gen/tests/typescript/node_modules/@grafana/grafana-foundation-sdk
if command -v node >/dev/null 2>&1 && [[ -d "$typescript_sdk" ]]; then
    echo "node $(node --version)"
    typescript_output="$(cargo test --locked -p bc-telemetry-gen --test suite -- --ignored typescript:: 2>&1)" || {
        printf '%s\n' "$typescript_output" >&2
        exit 1
    }
    printf '%s\n' "$typescript_output" | grep 'test result'
    if ! grep -q 'test result: ok. 1 passed' <<< "$typescript_output"; then
        echo 'error: the TypeScript proof did not run exactly one test' >&2
        exit 1
    fi
else
    skipped typescript 'Node or the SDK is absent here; scripts/fetch-typescript-sdk.sh fetches the SDK'
fi

step 'Grafana in the loop'
# The generator's integration tier (crates/gen/tests/suite/grafana_in_the_loop.rs):
# every generated dashboard, and the TypeScript proof's, provisioned into a real
# Grafana beside a real PostgreSQL, and every panel's query run through it. It
# needs the pinned Grafana and data source, which scripts/fetch-grafana.sh fetches
# into this cache; this step never fetches, and where nothing is fetched it says so
# by name. A run that ran nothing is a failure, not a pass.
grafana_cache="$HOME/.cache/bc-telemetry-rs"
if grafana_home="$(scripts/fetch-grafana.sh --fetched "$grafana_cache")"; then
    grafana_output="$(BC_TELEMETRY_GRAFANA_HOME="$grafana_home" \
        cargo test --locked -p bc-telemetry-gen --test suite -- --ignored --nocapture grafana_in_the_loop:: 2>&1)" || {
        printf '%s\n' "$grafana_output" >&2
        exit 1
    }
    grep -E '^(Grafana|PostgreSQL) |test result' <<< "$grafana_output"
    if ! grep -q 'test result: ok. 1 passed' <<< "$grafana_output"; then
        echo 'error: the Grafana tier did not run exactly one test' >&2
        exit 1
    fi
else
    skipped grafana "the pinned Grafana is not fetched into $grafana_cache; scripts/fetch-grafana.sh $grafana_cache fetches it"
fi

step 'Test-collection guard'
collection_guard every "${test_args[@]}"

step 'Doctests'
# Includes every ```rust block in README.md, wired in via src/lib.rs.
cargo test --locked "${member_packages[@]}" --doc
cargo test --locked "${member_packages[@]}" --doc --all-features

step 'Lints'
cargo clippy --locked "${member_packages[@]}" --all-targets -- -D warnings
cargo clippy --locked "${member_packages[@]}" --all-targets --all-features -- -D warnings

step 'Documentation'
RUSTDOCFLAGS='-D warnings' cargo doc --locked "${member_packages[@]}" --no-deps
RUSTDOCFLAGS='-D warnings' cargo doc --locked "${member_packages[@]}" --no-deps --all-features

step 'Feature matrix'
# `--all-targets` on both polarities, not just the library. Every other step
# above runs with `--all-features` by design (see test_args), so without this
# line nothing would ever compile the tests, benches, or examples with the
# feature OFF — and a `use` or a helper that silently depends on it would reach
# a consumer who never enables it.
cargo build --locked "${member_packages[@]}" --all-targets --no-default-features
cargo build --locked "${member_packages[@]}" --all-targets --all-features

# There is deliberately no wasm32 build step, unlike bc-date-time-rs. That
# crate claims to need nothing from a host, and the step is how
# that claim is checked. This crate makes the opposite claim: it reads the OS
# CSPRNG for trace ids and a filesystem for the file exporter, so it serves a
# process with an operating system beneath it. `getrandom` does not even
# compile for wasm32-unknown-unknown without an explicit backend, because that target has no entropy source, and adding one to
# satisfy a step nothing asks for would be inventing a consumer. bc-error-rs
# carries no wasm32 step either, for its own reasons; the step is per-member,
# not a family fixture.

step 'Dependency guard'
# Unlike its siblings, this crate is not dependency-free, and the reason is in
# the port rather than in a preference: the C++ `random.cpp` fills trace ids
# from the OS CSPRNG, which Rust cannot reach without `unsafe` — forbidden
# here — so `getrandom` is that call, and it brings `cfg-if` and `libc`. The
# family crates carry the epoch type, the hex codec, and the JSON writer the
# file exporter's versioned lines need, and bc-error-rs, the family's one error
# vocabulary, which the error telemetry records (bc-json-rs's strict read tier
# carries it too). Each is third-party-free, so the third-party closure is
# exactly the three named below.
#
# So the guard pins the exact set. A new dependency is a decision, and this is
# where it has to be made rather than noticed later.
#
# `normal,build` rather than `normal`: a build-script dependency compiles and
# runs code on every build of every consumer, so it is as real a supply-chain
# edge as a linked one.
#
# It reads the DEFAULT build. The optional features — `batched`, `otlp`,
# `postgres` — each bring their own closure by design, and a
# consumer that does not enable one never sees it.
expected_dependencies='bc-core-rs
bc-date-time-rs
bc-error-rs
bc-json-rs
bc-telemetry-rs
cfg-if
getrandom
libc'
actual_dependencies="$(dependency_names -p bc-telemetry-rs --edges normal,build)"
if [[ "$actual_dependencies" != "$expected_dependencies" ]]; then
    echo 'error: the default dependency set changed.' >&2
    echo '       Expected:' >&2
    printf '%s\n' "$expected_dependencies" | sed 's/^/         /' >&2
    echo '       Found:' >&2
    printf '%s\n' "$actual_dependencies" | sed 's/^/         /' >&2
    echo '       A dependency here is a decision, not a drive-by — see the' >&2
    echo '       comment above and README.md "Dependencies, and why each is here".' >&2
    exit 1
fi
printf '%s\n' 'the default build carries exactly the expected dependencies: ok'

step 'Generator dependency guard'
# The generator's set is pinned the same way and for the same reason: a new
# dependency is a decision. Beyond the port it lowers onto, it takes exactly
# one third-party crate, `toml_edit` with only its parse feature, and that
# crate's closure. crates/gen/README.md says why each is there.
expected_generator_dependencies='bc-core-rs
bc-date-time-rs
bc-error-rs
bc-json-rs
bc-telemetry-dashboard
bc-telemetry-gen
bc-telemetry-rs
cfg-if
equivalent
getrandom
hashbrown
indexmap
libc
toml_datetime
toml_edit
toml_parser
winnow'
actual_generator_dependencies="$(dependency_names -p bc-telemetry-gen --edges normal,build)"
if [[ "$actual_generator_dependencies" != "$expected_generator_dependencies" ]]; then
    echo "error: the generator's dependency set changed." >&2
    echo '       Expected:' >&2
    printf '%s\n' "$expected_generator_dependencies" | sed 's/^/         /' >&2
    echo '       Found:' >&2
    printf '%s\n' "$actual_generator_dependencies" | sed 's/^/         /' >&2
    echo '       See crates/gen/README.md "Dependencies, and why each is here".' >&2
    exit 1
fi
printf '%s\n' 'the generator carries exactly the expected dependencies: ok'

step 'Dashboard model dependency guard'
# The dashboard model is linked by any program that builds a dashboard, so it
# carries the family's JSON writer and nothing else: no third-party crate at all.
# crates/dashboard/README.md says why, and why its tests' schema validator is
# the workspace's own rather than a crate's.
expected_dashboard_dependencies='bc-core-rs
bc-error-rs
bc-json-rs
bc-telemetry-dashboard'
actual_dashboard_dependencies="$(dependency_names -p bc-telemetry-dashboard --edges normal,build)"
if [[ "$actual_dashboard_dependencies" != "$expected_dashboard_dependencies" ]]; then
    echo "error: the dashboard model's dependency set changed." >&2
    echo '       Expected:' >&2
    printf '%s\n' "$expected_dashboard_dependencies" | sed 's/^/         /' >&2
    echo '       Found:' >&2
    printf '%s\n' "$actual_dashboard_dependencies" | sed 's/^/         /' >&2
    echo '       See crates/dashboard/README.md "Dependencies, and why each is here".' >&2
    exit 1
fi
printf '%s\n' 'the dashboard model carries exactly the expected dependencies: ok'

member_pipeline_verdict
