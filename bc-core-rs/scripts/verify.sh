#!/usr/bin/env bash
#
# The crate's full verification pipeline, stated once.
#
# bc-rs's root scripts/verify.sh runs it for every CI run and for the tracked RustRover run configurations, so "green
# locally" and "green in CI" cannot drift apart by editing one and forgetting the other. The steps every member shares
# — its command line, the test runner, the test-collection guard and the verdict — are bc-rs's
# scripts/lib/member-pipeline.sh; what is here is this crate's own.
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

# Every test invocation shares these flags, and so does the test-collection
# guard below — which is the point. `--all-features` is not optional: without it
# a `#![cfg(feature = "...")]` integration test compiles under clippy and is
# silently dropped from the run. This crate has no features today; the flag
# stays so adding one cannot silently drop its tests.
test_args=(--locked --all-targets --all-features)

step 'Formatting'
cargo fmt --check

step 'Tests (debug)'
run_tests "${test_args[@]}"

if ! release_tests_skipped_by_quick; then
    step 'Tests (release)'
    # Debug and release must agree: the generation counter is the arena's whole
    # safety story and it is incremented arithmetic, whose overflow behaviour
    # differs between the two profiles.
    run_tests "${test_args[@]}" --release
fi

step 'Test-collection guard'
collection_guard every "${test_args[@]}"

step 'Doctests'
# Includes every ```rust block in README.md, wired in via src/lib.rs.
cargo test --locked --doc
cargo test --locked --doc --all-features

step 'Lints'
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --all-features -- -D warnings

step 'Documentation'
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps --all-features

step 'Feature matrix'
cargo build --locked --no-default-features
cargo build --locked --all-features

step 'wasm32 build'
# Nothing else in the pipeline would catch a std call unavailable on wasm32; it
# compiles fine on the host. This crate reaches for no platform facility at all
# — the arena is a Vec — so the step exists to keep that true rather than to
# check a known-risky caller.
#
# Where the target is absent the step is a skip, named in the verdict: a fresh
# clone still verifies, and CI installs the target, so the check always runs there.
if wasm32_target_installed; then
    cargo build --locked --target wasm32-unknown-unknown --all-features
else
    skipped wasm32 "the wasm32-unknown-unknown target is not installed; rustup target add wasm32-unknown-unknown"
fi

step 'Dependency guard'
# This crate sits underneath the rest of the family: bc-nodular-architecture-rs
# holds an arena on the hottest path it has, so an edge taken here is an edge
# taken by every consumer downstream of it. The arena is a Vec and a free list;
# nothing it does needs help. Unlike bc-json-rs there is no optional-feature
# carve-out: the guard admits nothing but this package.
#
# `normal,build` rather than `normal`: a build-script dependency compiles and
# runs code on every build of every consumer, so it is as real a supply-chain
# edge as a linked one.
unexpected="$(unexpected_dependencies 'bc-core-rs' --edges normal,build)"
if [[ -n "$unexpected" ]]; then
    echo "error: bc-core-rs gained a dependency: $unexpected" >&2
    echo '       This crate is contractually dependency-free: the rest of the family' >&2
    echo '       builds on it, so an edge taken here is taken by every consumer.' >&2
    echo '       If a dependency is genuinely needed, that is a design decision,' >&2
    echo '       not a drive-by.' >&2
    cargo tree --locked --edges normal,build >&2
    exit 1
fi
echo 'the default build carries no dependency: ok'

member_pipeline_verdict
