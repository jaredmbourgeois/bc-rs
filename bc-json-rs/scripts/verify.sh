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
# takes). The tiers this run may skip: wasm32 test-collection, and release-tests under --quick.

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
# silently dropped from the run.
test_args=(--locked --all-targets --all-features)

step 'Formatting'
cargo fmt --check

step 'Tests (debug)'
run_tests "${test_args[@]}"

if ! release_tests_skipped_by_quick; then
    step 'Tests (release)'
    # Debug and release must agree: the property tests lean on arithmetic whose
    # overflow behavior differs between the two profiles.
    run_tests "${test_args[@]}" --release
fi

step 'Test-collection guard'
collection_guard every "${test_args[@]}"

step 'Doctests'
# Includes every ```rust block in README.md, wired in via src/lib.rs, and the
# compile_fail examples that are the typestate's only proof.
cargo test --locked --doc
cargo test --locked --doc --all-features

step 'Lints'
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --all-features -- -D warnings

step 'Documentation'
# Twice, like the lint and build steps: the feature-gated modules are not
# compiled by the default run, so their intra-doc links would go unchecked.
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps --all-features

step 'Feature matrix'
cargo build --locked --no-default-features
cargo build --locked --all-features

step 'wasm32 build'
# README's Verification section lists this build and says verify.sh runs
# "everything below" — until this step it did not. Nothing else in the pipeline
# would catch a std call that is unavailable on wasm32; it compiles fine on the
# host.
#
# Where the target is absent the step is a skip, named in the verdict: a fresh
# clone still verifies, and CI installs the target, so the check always runs there.
if wasm32_target_installed; then
    cargo build --locked --target wasm32-unknown-unknown --all-features
else
    skipped wasm32 "the wasm32-unknown-unknown target is not installed; rustup target add wasm32-unknown-unknown"
fi

step 'Dependency guard'
# This crate exists because a consumer reads JSON in its core library AND in the
# small runtime it ships to remote hosts, the most dependency-sensitive place in
# its build. A default build that pulls in THIRD-PARTY code gives up the only
# reason to prefer it over serde_json.
#
# Third-party is the claim, and now it is also the check. It used to refuse any
# dependency at all, which said something stricter than the rationale just
# above it -- and the two disagreed the moment `bc-error-rs` stopped being
# optional. A sibling `bc-*-rs` is not a supply-chain edge in the sense this
# guard exists to police: the family is one codebase, one workspace, each
# member third-party-free in its own right, so a sibling edge adds nothing to a
# consumer's third-party closure. The allowlist is explicit and small, so an
# unexpected sibling still fails here rather than being waved through by a
# pattern match on the name.
#
# `normal,build` rather than `normal`: a build-script dependency compiles and
# runs code on every build of every consumer, so it is as real a supply-chain
# edge as a linked one.
#
# bc-core-rs arrives through bc-error-rs, which reports an invariant breach to
# its assertion handler; it is first-party and itself dependency-free.
#
# The reader refuses a `cargo tree` that failed or resolved nothing, so the
# comparison is never made against a graph it did not see.
unexpected="$(unexpected_dependencies 'bc-json-rs bc-error-rs bc-core-rs' --edges normal,build)"
if [[ -n "$unexpected" ]]; then
    echo 'error: bc-json-rs gained a non-optional dependency outside the' >&2
    echo "       sibling allowlist: $unexpected" >&2
    echo '       The default build must stay free of third-party code; put a' >&2
    echo '       new third-party dependency behind a feature, as `serde` is.' >&2
    echo '       A new *sibling* edge is a deliberate decision: add it to the' >&2
    echo '       allowlist above, with the reason.' >&2
    cargo tree --locked --edges normal,build >&2
    exit 1
fi
echo 'the default build carries the family crates it names and nothing else: ok'

step 'Conformance corpus licence guard'
# The corpus's completeness is asserted in tests/conformance.rs, which counts the
# cases per class — a half-vendored corpus fails there, so counting them again
# here would only be a second copy to keep in step. What no Rust test can express
# is the redistribution obligation: the licence has to travel with the cases
# covered by it, and nothing in a test run would notice it going missing.
corpus='tests/fixtures/JSONTestSuite'
if [[ ! -f "$corpus/LICENSE" || ! -f "$corpus/REVISION" ]]; then
    echo "error: $corpus is missing its LICENSE or REVISION." >&2
    echo '       The corpus is MIT-licensed third-party data; both travel with it.' >&2
    exit 1
fi

member_pipeline_verdict
