#!/usr/bin/env bash
#
# This member's full verification pipeline, stated once.
#
# Two packages with different contracts: the ported crate, which must stay runtime-free and depend on bc-core-rs and
# bc-date-time-rs alone, and the tokio adapter, which exists precisely to depend on a runtime. Steps that assert the
# port's independence are therefore scoped to `-p bc-nodular-architecture-rs`; steps that just have to be green run
# over both packages (`member_packages`, which scripts/lib/member-pipeline.sh reads from Cargo). bc-rs's root
# scripts/verify.sh runs this for CI and for the tracked RustRover run configurations.
#
# Usage: scripts/verify.sh [--quick] [--accept-skip <tier>=<why>]...   (scripts/lib/member-pipeline.sh says what each
# takes). The tiers this run may skip: wasm32, test-collection, and release-tests under --quick.

set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
# The bc-rs workspace this member sits in, whose shared scripts every member's pipeline uses.
workspace_root="$(cd "$repository_root/.." && pwd -P)"
# shellcheck source=../../scripts/lib/member-pipeline.sh
. "$workspace_root/scripts/lib/member-pipeline.sh"
member_pipeline_begin "$repository_root" 'wasm32 test-collection' "$@"
cd "$repository_root"

# The libraries' unit tests and every integration suite, over both packages.
test_args=(--locked "${member_packages[@]}" --lib --tests)

step 'Formatting'
cargo fmt --check "${member_packages[@]}"

step 'Tests (default features)'
run_tests "${test_args[@]}"

step 'Tests (all features)'
run_tests "${test_args[@]}" --all-features

if ! release_tests_skipped_by_quick; then
    step 'Tests (release, default features)'
    run_tests "${test_args[@]}" --release

    step 'Tests (release, all features)'
    run_tests "${test_args[@]}" --release --all-features
fi

step 'Test-collection guard'
# The widest of the invocations above, so nothing the suite ran goes unmeasured. Restricted to the `tests/` suites:
# neither crate here has a `#[cfg(test)]` module — every test lives in `tests/` — so cargo compiles a unit-test binary
# per lib that holds nothing and structurally always will. That is this member's layout, not the dead-or-gated-out
# tests the guard is for, and excusing it by name would need the allowlist the guard refuses to have. The day either
# lib grows a `#[cfg(test)]` module, measure `every` binary: from then on an empty lib binary is the defect rather than
# the layout.
collection_guard integration "${test_args[@]}" --all-features

step 'Doctests'
cargo test --locked "${member_packages[@]}" --doc
cargo test --locked "${member_packages[@]}" --doc --all-features

step 'Lints'
cargo clippy --locked "${member_packages[@]}" --all-targets -- -D warnings
cargo clippy --locked "${member_packages[@]}" --all-targets --all-features -- -D warnings

step 'Documentation'
RUSTDOCFLAGS='-D warnings' cargo doc --locked "${member_packages[@]}" --no-deps
RUSTDOCFLAGS='-D warnings' cargo doc --locked "${member_packages[@]}" --no-deps --all-features

step 'Feature matrix'
cargo build --locked "${member_packages[@]}" --no-default-features
cargo build --locked "${member_packages[@]}" --all-features

step 'wasm32 build'
if wasm32_target_installed; then
    # The ported crate only: the adapter targets a runtime that does not build
    # for wasm32, which is exactly why the port keeps it at arm's length.
    cargo build --locked -p bc-nodular-architecture-rs --target wasm32-unknown-unknown --no-default-features
    cargo build --locked -p bc-nodular-architecture-rs --target wasm32-unknown-unknown --all-features
else
    skipped wasm32 "the wasm32-unknown-unknown target is not installed; rustup target add wasm32-unknown-unknown"
fi

step 'Dependency graph'
for feature_args in --no-default-features --all-features; do
    unexpected="$(unexpected_dependencies 'bc-nodular-architecture-rs bc-core-rs bc-date-time-rs' \
        -p bc-nodular-architecture-rs --edges normal,build "$feature_args")"
    if [[ -n "$unexpected" ]]; then
        echo "error: the $feature_args graph contains a dependency beyond bc-core-rs and bc-date-time-rs:" >&2
        echo "$unexpected" >&2
        exit 1
    fi
done
echo 'the port depends on bc-core-rs and bc-date-time-rs alone, with and without features: ok'

step 'Adapter dependency graph'
# The adapter is allowed exactly one runtime and the crate it drives. Its direct
# edges are pinned so a convenience dependency cannot arrive unnoticed; tokio's
# own transitive tree is tokio's business, so only direct edges are checked.
unexpected="$(unexpected_dependencies 'bc-nodular-tokio bc-nodular-architecture-rs tokio' \
    -p bc-nodular-tokio --edges normal --depth 1)"
if [[ -n "$unexpected" ]]; then
    echo 'error: the adapter has a direct dependency beyond the core crate and tokio:' >&2
    echo "$unexpected" >&2
    exit 1
fi
echo 'the adapter depends on the port and tokio alone: ok'

step 'Runtime and lock independence'
scripts/test-check-independence.sh
scripts/check-independence.sh

step 'Benchmarks compile'
cargo bench --locked -p bc-nodular-architecture-rs --no-run

member_pipeline_verdict
