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
# takes). The tiers this run may skip: wasm32 test-collection bindings-swift bindings-kotlin, and release-tests under --quick.

set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
# The bc-rs workspace this member sits in, whose shared scripts every member's pipeline uses.
workspace_root="$(cd "$repository_root/.." && pwd -P)"
# shellcheck source=../../scripts/lib/member-pipeline.sh
. "$workspace_root/scripts/lib/member-pipeline.sh"
member_pipeline_begin "$repository_root" 'wasm32 test-collection bindings-swift bindings-kotlin' "$@"
cd "$repository_root"

# Every test invocation shares these flags, and so does the test-collection
# guard below — which is the point. `--all-features` is not optional: without it
# a `#![cfg(feature = "...")]` integration test compiles under clippy and is
# silently dropped from the run. That is load-bearing now, not precautionary:
# `tests/uniffi_bindings.rs` is gated on `uniffi` and runs only because of it.
test_args=(--locked --all-targets --all-features)

step 'Formatting'
cargo fmt --check

step 'Tests (debug)'
run_tests "${test_args[@]}"

if ! release_tests_skipped_by_quick; then
    step 'Tests (release)'
    # Debug and release must agree: the property tests walk wide signed epoch
    # ranges through multiply-and-add arithmetic whose overflow behavior differs
    # between the two profiles.
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
# `--all-targets` on both polarities, not just the library. Every other step
# above runs with `--all-features` by design (see test_args), so without this
# line nothing would ever compile the tests, benches, or examples with the
# feature OFF — and a `use` or a helper that silently depends on it would reach
# a consumer who never enables it.
cargo build --locked --all-targets --no-default-features
cargo build --locked --all-targets --all-features

step 'wasm32 build'
# README's Verification section lists this build and says verify.sh runs
# everything below. Nothing else in the pipeline would catch a std call that is
# unavailable on wasm32; it compiles fine on the host.
#
# `system_epoch_source` is the one caller of the system clock, and on
# wasm32-unknown-unknown std's SystemTime::now() panics rather than failing to
# compile — which is exactly why the EpochSource seam exists. This step proves
# the crate *builds* there; the README's "Behaviour worth knowing" says what happens at
# run time.
#
# It builds with NO features. `uniffi` is excluded deliberately, and the
# exclusion is a scope decision rather than a workaround: the scaffolding does
# compile for wasm32. But what this step claims is that *this crate* compiles
# for the target its README names, and compiling the scaffolding's dependency
# closure neither strengthens that claim nor could fail for a reason about this
# crate.
#
# The feature set is asserted rather than tracked, so a second feature cannot
# quietly inherit that decision — adding one fails here until someone says
# whether wasm32 should build it.
#
# Read through `cargo metadata`, NOT by scanning the [features] table: an
# optional dependency creates a real feature with no [features] row at all, and
# that is the ordinary way a crate grows one. Measured on a scratch crate
# carrying `libc = { optional = true }` plus a `uniffi` row — the table scan this
# replaces reported `uniffi`, and cargo reports `libc uniffi`. The scan was
# blind to exactly the case the guard exists to catch.
#
# Two lists rather than one, because a single "excluded" list has to hold every
# feature to be compared against what the crate declares — which made the error
# message tell you to add an *included* feature to a list named "excluded".
# `wasm32_features` is what the build passes; `wasm32_skipped_features` is what
# it deliberately leaves out. Their union must be everything declared.
wasm32_features=''
wasm32_skipped_features='uniffi'
# Selected by name, and refused unless exactly one package carries it: inside bc-rs, `cargo metadata` lists every
# member's package, and the first of them is some other crate's.
if ! cargo_features="$(cargo metadata --no-deps --format-version 1 |
    jq -r --arg name bc-date-time-rs \
        '[.packages[] | select(.name == $name)] | if length == 1 then .[0].features | keys[] else error("expected one package named \($name)") end')"; then
    echo 'error: could not read the feature list from cargo metadata, so the' >&2
    echo '       wasm32 feature decision below verified nothing.' >&2
    exit 1
fi
# `|| true` because grep exits 1 on no match, and a crate declaring only
# `default` legitimately reads empty here — the comparison below is what judges
# that, not the pipeline's exit status.
declared_features="$(printf '%s\n' "$cargo_features" \
    | grep -v '^default$' | sort | tr '\n' ' ' | sed 's/ $//' || true)"
accounted_features="$(printf '%s %s' "$wasm32_features" "$wasm32_skipped_features" \
    | tr ' ' '\n' | grep -v '^$' | sort | tr '\n' ' ' | sed 's/ $//' || true)"
if [[ "$declared_features" != "$accounted_features" ]]; then
    echo "error: this crate declares features '$declared_features', but the" >&2
    echo "       wasm32 build accounts for '$accounted_features'." >&2
    echo '       Decide where the new one belongs: wasm32_features to build it' >&2
    echo '       on wasm32, or wasm32_skipped_features with the reason. Silence' >&2
    echo '       is the one wrong answer.' >&2
    exit 1
fi

# Where the target is absent the step is a skip, named in the verdict: a fresh
# clone still verifies, and CI installs the target, so the check always runs there.
if wasm32_target_installed; then
    wasm32_build_args=(--locked --target wasm32-unknown-unknown)
    if [[ -n "$wasm32_features" ]]; then
        wasm32_build_args+=(--features "$wasm32_features")
    fi
    cargo build "${wasm32_build_args[@]}"
else
    skipped wasm32 "the wasm32-unknown-unknown target is not installed; rustup target add wasm32-unknown-unknown"
fi

step 'Dependency guard'
# This crate is the family's canonical time type: it lands in a consumer's
# persisted records and in the small runtimes a consumer ships to remote hosts,
# the most dependency-sensitive place in its build. The calendar math here IS
# the algorithm chrono and jiff implement, so a dependency would buy nothing —
# and taking one would give up the only reason to prefer this crate over them.
#
# What this measures is the DEFAULT build, and that is the whole contract: no
# feature flags are passed, so cargo resolves default features, and the `uniffi`
# optional dependency is invisible here exactly as it is to a consumer that does
# not ask for it. The steps above that pass `--all-features` do see it; this one
# deliberately does not, because "dependency-free" is a claim about what a
# consumer gets by default, not about what the crate can be asked for.
#
# `normal,build` rather than `normal`: a build-script dependency compiles and
# runs code on every build of every consumer, so it is as real a supply-chain
# edge as a linked one.
# Proc-macro mode needs no build.rs, so enabling `uniffi` adds no build edge
# either; this line still reads both because that is a property of the mode,
# not of the current dependency list.
unexpected="$(unexpected_dependencies 'bc-date-time-rs' --edges normal,build)"
if [[ -n "$unexpected" ]]; then
    echo "error: bc-date-time-rs gained a dependency: $unexpected" >&2
    echo '       Its default build is contractually dependency-free: the calendar' >&2
    echo '       math here is the algorithm chrono and jiff implement, so a' >&2
    echo '       dependency would buy nothing. If one is genuinely needed, that is' >&2
    echo '       a design decision, not a drive-by.' >&2
    cargo tree --locked --edges normal,build >&2
    exit 1
fi
echo 'the default build carries no dependency: ok'

step 'Foreign bindings smoke'
# The one step that crosses the ABI. Every other binding test here exercises the
# Rust side only, so a scaffolding-level regression -- the `#[track_caller]`
# class -- is invisible to all of them until a consumer hits it. Each language
# runs where its toolchain is here; where it is not, the tier is a skip named in
# the verdict, and a toolchain that is present and fails is a failure.
run_bindings_smoke "$repository_root" swift
run_bindings_smoke "$repository_root" kotlin

member_pipeline_verdict
