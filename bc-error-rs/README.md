# bc-error-rs

Structured, recursively annotated error values, stable human diagnostics, and small extraction extensions for Rust's
native `Result`.

The default build has **no third-party dependencies** and builds for `wasm32-unknown-unknown`; `serde` and `uniffi`
(Swift and Kotlin bindings) are optional features.

Depend on it by git and release tag, as the [workspace README](../README.md#depending-on-bc-rs) shows; the package is
`bc-error-rs` and the library is imported as `bc_error`.

It is a semantics-preserving port of the C++ `libs/foundation/error` library of Bourgeois Concern's codebase: the same
API and behaviour, with every case of the original's test suite carried into this crate's tests or set aside for a
recorded reason.

Licensed under Apache-2.0 ([LICENSE.txt](LICENSE.txt)); copyright 2026 Jared Bourgeois.

## Public API

- `MetadataType` has the source discriminants `Null = 0` through `Error = 8`.
- `MetadataValue` is a null-by-default recursive value. Its variant set is a public `#[non_exhaustive]` enum, so callers can `match` on it; the source-named `as_*` accessors, `r#type()`, `From` conversions, deep cloning, equality, and deterministic human stringification are all retained.
- `MetadataArray` and `MetadataDictionary` are public aliases for `Vec<MetadataValue>` and `BTreeMap<String, MetadataValue>`.
- `MetadataDictionaryExt` adds `with` (chainable) and `set` (in place) to `MetadataDictionary`, taking `impl Into<String>` keys and `impl Into<MetadataValue>` values so the `From` conversions above are reachable without naming them. `BTreeMap::insert` remains the primitive.
- `AnyErrorKind` is the descriptor pairing a category with its numeric value, defined in a module that depends on nothing else here so a domain can declare its named kinds without reaching for `AnyError`, metadata, or a `Result`. `AnyErrorKind::new` is `const`, so a registry of kinds is free; `AnyErrorKind::owned` covers a category only known at run time.
- `AnyError` owns category, kind, location, metadata, and an outermost-first context chain. It is built from a named `AnyErrorKind` through `from_kind`, `from_detail`, `from_metadata`, or `at_location` — raw positional construction is inaccessible. `matches(&kind)` compares category and value together. `with_context` and `with_metadata` are consuming builders. The payload is boxed, so the type is one pointer wide and `Result<T, AnyError>` stays cheap.
- `RefusalKind` declares a condition that is a refusal every time: every `AnyError` raised as one carries the `refused` fact. The factories and `matches` take either kind through the sealed `ErrorCondition` trait.
- `ErrorFacts` is the conversion trait; `make_any_error` copies all fields, including context. `fail(&error, "context")` is the one spelling of "this operation failed": the error as an `AnyError` with the context stamped, as an `Err`.
- `error_detail_view` borrows the reason an error states (a non-empty string under `detail`), `error_detail` returns it or the caller's fallback, and `prefixed_error_detail` joins a sentence and the reason without a dangling `": "`. `MESSAGE_METADATA_KEYS` is where an error's text lives, best key first.
- `format_error_message` is the canonical one-line diagnostic, used by `AnyError`'s `Display`: context, `category kind=<n>`, the structured facts, then the stated reason. It never emits a source position, so it is safe in a response body. `format_error_message_with_location` adds ` @ <location>` when, and only when, the error states no reason — for an operator's sink.
- `DomainErrorBase` is what a domain error type holds — its kind, location and metadata — and `DomainError` (a category and the base) gives such a type `ErrorFacts` and `CarriesFacts`.
- `error_facts` holds the five facts a caller above the store still asks about — `sqlstate`, `corrupt_row`, `commit_outcome_unknown`, `invariant_breach`, `refused` — as reserved metadata keys, with readers (`is_corrupt_row`, `sqlstate_of`, `is_permanent_failure`, …), writers (`with_corrupt_row_fact`, …, generic over `CarriesFacts`), `with_facts_of` to carry them onto a new error, and `caused_by` to build an error caused by another. `with_invariant_breach_fact` also reports the breach to the process's assertion handler (`bc-core-rs`).
- `FirstFailure` collects a batch's independent failures: the first is reported, every one is counted.
- `ResultOptionalExt` adds the checked `Result<Option<T>, E>` extraction the standard type does not supply. Everything else the source's `Result` class offered is spelled with the standard operation of the same meaning.
- `format_location` and `format_location_with_separator` capture their caller. `format_location_from` and `format_location_from_with_separator` format an explicit `std::panic::Location`.

In a consumer's `Cargo.toml`, at a release tag:

```toml
[dependencies]
bc-error-rs = { git = "https://codeberg.org/BourgeoisConcernLLC/bc-rs.git", tag = "0.1.0" }
```

Every `rust` block in this file is compiled as a doctest by `cargo test --doc`,
so an example that stops matching the API fails CI.

```rust
use bc_error::{AnyError, AnyErrorKind, MetadataValue};

// A domain declares its kinds once; `AnyErrorKind::new` is `const`, so a
// registry costs nothing and the category can never drift from the value.
const COUNTER_UNREADABLE: AnyErrorKind = AnyErrorKind::new("store", 7);

fn read_value() -> Result<u64, AnyError> {
    Err(
        AnyError::from_kind(COUNTER_UNREADABLE)
            .with_context("reading value")
            .with_metadata("key", MetadataValue::from("counter")),
    )
}

let error = read_value().unwrap_err();
assert!(error.matches(&COUNTER_UNREADABLE));
assert_eq!(error.context(), ["reading value"]);
```

Metadata is matched, not probed:

```rust
use bc_error::{AnyError, AnyErrorKind, MetadataValue};

let error = AnyError::from_kind(AnyErrorKind::new("store", 7))
    .with_metadata("attempts", MetadataValue::from(3_i64));

// `MetadataValue` is #[non_exhaustive], so a downstream match carries a
// wildcard arm; adding a variant is therefore not a breaking change.
let rendered = match &error.metadata()["attempts"] {
    MetadataValue::Int64(count) => format!("{count} attempts"),
    MetadataValue::String(text) => text.clone(),
    other => other.stringify(),
};

assert_eq!(rendered, "3 attempts");
```

## Stringify And Safety

`MetadataValue::stringify()` is human output, explicitly not JSON. It deliberately preserves the source behavior of placing strings in quotes without escaping their contents. Quotes, backslashes, control characters, and newlines in strings, dictionary keys, error fields, or context therefore appear literally and may produce invalid JSON or multi-line output. Do not feed this representation to a JSON parser or use it as a wire format. For a canonical single-line diagnostic, use `format_error_message` or `AnyError`'s `Display` implementation; both sanitize the final assembled message.

Dictionaries use source-compatible sorted-key order. Finite floats use locale-independent fixed decimal precision: six places for `stringify()` and the requested precision for `stringify_with_decimals(u8)`. Nonfinite values are deliberately normalized to lowercase `nan`, `inf`, and `-inf`. Recursive errors always render fields in `category`, `kind`, `location`, optional nonempty `context`, then `metadata` order.

For a machine-readable representation, enable the optional `serde` feature, which derives `Serialize` and `Deserialize` for `MetadataValue`, `MetadataType`, and `AnyError`. The encoding is externally tagged rather than untagged, so `Int64`, `Uint64`, and `Float64` stay distinguishable across a round trip. That is the supported wire path; `stringify()` is not.

```toml
[dependencies]
bc-error-rs = { git = "https://codeberg.org/BourgeoisConcernLLC/bc-rs.git", tag = "0.1.0", features = ["serde"] }
```

The implementation forbids `unsafe`. The default build has **no** third-party dependencies, and `scripts/verify.sh` fails if one appears: `serde` is opt-in, and `serde_json` is a dev-dependency used only to prove the feature round-trips, so neither reaches a consumer's dependency graph.

## The `uniffi` Feature

Off by default. Enabling it adds the UniFFI binding so a Swift or Kotlin application can depend on
this crate directly, and pulls dozens of transitive crates — which is why it is a feature rather
than a dependency. `src/ffi.rs` is the entire foreign contract.

The Rust API is unchanged. What the feature adds is a **boundary mirror**, because two parts of
`MetadataValue` cannot cross as they stand and both are deliberate:

- It carries a `MetadataDictionary`, which is a `BTreeMap`. UniFFI has no form for one, so it
  crosses as a **sorted sequence of key/value records** rather than being silently rehashed —
  `stringify` and `format_message` depend on that ordering. `BTreeMap` iterates in key order, so
  `ffi::entries_of` is sorted by construction rather than by a sort it performs.
- Its `Error` arm holds a `Box<AnyError>`, whose payload is private and size-pinned.

So the boundary gets `ForeignMetadataValue` and `ForeignError`, with total conversions both ways.
`AnyError` crosses as a **record, not an object**: an error is a value, and a foreign caller wants
its category, kind, and metadata rather than a handle whose lifetime it must manage.

`ErrorFacts` is the one seam a foreign caller implements, and it needed reshaping for a reason
worth stating: all five of its methods return **borrowed** data, which a callback interface cannot
produce — the foreign side owns its values and has nothing for Rust to borrow from. So
`ffi::ForeignErrorFacts` returns owned values and `ffi::CapturedFacts` stores one reading of them,
serving `ErrorFacts` from what it stored. Reading each field exactly once is not just an
optimisation: it makes the snapshot internally consistent, so a foreign object that changed
between two calls cannot yield an error whose category and metadata disagree.

`MetadataType` needed nothing. It was already `#[repr(u8)]` with written-out discriminants "so
ported code and foreign bindings keep a stable discriminant" — this is the binding that sentence
was written for.


### The bindings smoke test

`tests/uniffi_bindings.rs` exercises the Rust side of the boundary only — it round-trips the
mirror types in process and never generates a line of Swift or Kotlin. That leaves a whole class
invisible: a scaffolding-level regression is unreachable from any Rust-side test and would first
be seen by a consumer.

`bindings-smoke/` closes it. It is a test-only `cdylib` shim standing in for a foreign consumer,
deliberately *not* part of this crate — every bound crate in the family stays a plain `lib`, and
the `cdylib` a generator runs against belongs to the consumer. It has its own `[workspace]` table,
so bc-rs's workspace excludes it, and it is absent from `package.include`, so `cargo package` never
sees it. `scripts/verify.sh` drives it through bc-rs's `scripts/bindings-smoke.sh`, the one harness
every member with a shim calls: bindings are generated for Swift and Kotlin, compiled, and executed
against the real library, carrying one value across the ABI in both directions.

**Tier: dev fleet, and the tier is declared rather than inferred.** Each language runs where its
toolchain is here, and a toolchain that is present and fails is a failure. A host without one does
not get to pass on that basis — absence alone is the fail-open this step exists to prevent, and a
machine that quietly loses `swiftc` would otherwise start reporting a green it did not earn. So the
missing language is the skipped tier `bindings-swift` or `bindings-kotlin`, and the run is
INCOMPLETE unless its caller accepted that tier by name with its reason: CI's
`.forgejo/workflows/ci.yml` accepts `bindings-kotlin`, because its runner carries Swift and no
Kotlin. The generator runs on the workspace's pinned stable, 1.99.0, because `uniffi`'s `cli`
feature needs a rustc newer than the MSRV pin, which governs the library and not a build tool.

## Verification

The port is verified with one command:

```sh
scripts/verify.sh
```

That script is the single definition of the pipeline — formatting, tests in debug and
release, doctests (including every `rust` block in this file), Clippy with warnings denied,
rustdoc with warnings denied, the feature matrix, a wasm32 build, two guards — test collection
and the dependency surface — and the foreign bindings smoke test. CI and the tracked RustRover run
configurations both invoke it, so "green locally" and "green in CI" cannot drift apart by
editing one and forgetting the other. Pass `--quick` to skip the release-profile run while
iterating.

Both guards exist because what they protect is invisible to the test suite:

- **Test collection.** A test binary that collects zero tests still passes — cargo prints
  `running 0 tests` and exits 0. This crate's five `serde` tests were gated behind a feature
  the test step did not enable, so they compiled under Clippy and never ran. The guard fails
  the build on any binary that collects nothing, whatever the reason.
- **Dependency surface.** The default build stays free of third-party code; `serde` is opt-in.

The pipeline carries a `wasm32` build step. This crate is the family's error model, so it is
reached from every surface the family targets, including the wasm32 builds its siblings promise;
leaving the claim to be enforced downstream would surface a `std` call unavailable there in a
consumer rather than here. It builds with `serde` and deliberately without `uniffi` — the
scaffolding does compile for wasm32, but what the step claims is that *this crate* does, and
compiling the generator's dependency closure neither strengthens that claim nor could fail for a
reason about this crate. `verify.sh` asserts that every declared feature is accounted for as
either built or skipped, so a new one cannot quietly inherit that decision.

CI runs bc-rs's root pipeline, which runs this one, on Rust 1.85.0 and one pinned stable, 1.99.0 — never the moving channel. bc-rs's `rust-toolchain.toml` pins local builds to the
1.85.0 MSRV so the question CI's first matrix leg asks is also the one answered on a
developer's machine; CI overrides it per-leg with `RUSTUP_TOOLCHAIN`. `cargo-nextest` is
pinned to 0.9.100 and terminates any test exceeding 30 seconds; the CI job's outer bound is
bc-rs's `.forgejo/workflows/ci.yml` `timeout-minutes`. `verify.sh` falls back to `cargo test` when nextest is not installed locally,
and says so.
