# bc-rs

Rust libraries ported case for case from a production C++ codebase: a generational arena and the encoding primitives
around it, one UTC time type with an injectable clock, structured errors, a strict JSON validator, reader and writer,
a runtime-neutral unidirectional data-flow core, and OTel-shaped telemetry with a generator for its schema, queries
and dashboards.

**Why these:** each crate is a semantics-preserving port of one of that codebase's C++ libraries — the same public API
and observable behaviour, written as idiomatic Rust where Rust forces it or where it turns a misuse the C++ detects at
run time into one that does not compile. Every case of the original's test suite was carried into the port, or
set aside with a recorded reason, so the behaviour you get is the one that suite pinned. No crate contains `unsafe`.

**Proof:** 1,200 unit and integration tests across the nine packages, plus every README example compiled as a
doctest, run on two Rust toolchains on every push — all but two, which need a pinned Grafana or the TypeScript SDK
and run where those are present. `bc-json-rs` runs the whole JSONTestSuite parsing corpus: it
refuses all 188 must-refuse cases, accepts every must-accept case but the four it refuses on purpose, and records
its outcome for each of the 35 implementation-defined ones ([`tests/conformance.rs`](bc-json-rs/tests/conformance.rs)).
The count is `cargo nextest list --workspace --all-features --lib --tests` at this release.

**Get it:** pin a tag from `https://codeberg.org/BourgeoisConcernLLC/bc-rs.git` with Cargo (below).

## Depending on bc-rs

The crates are not on crates.io. Depend on each one you use by this repository and a release tag; Cargo finds the
package by name inside the workspace, and `Cargo.lock` records the exact commit the tag named.

```toml
[dependencies]
bc-json-rs = { git = "https://codeberg.org/BourgeoisConcernLLC/bc-rs.git", tag = "0.1.0" }
bc-date-time-rs = { git = "https://codeberg.org/BourgeoisConcernLLC/bc-rs.git", tag = "0.1.0" }
```

Every crate in one release is released together, so name the same tag for each. A bump is a deliberate edit of the
tag and a `cargo update -p <crate>`; nothing moves under you between them.

## What it does

| Crate | Import | What it is |
|---|---|---|
| [`bc-core-rs`](bc-core-rs/README.md) | `bc_core` | A generational arena and a contiguous map; hex, URL, XML and percent encoding; the process's assertion handler. No dependencies. |
| [`bc-date-time-rs`](bc-date-time-rs/README.md) | `bc_date_time` | `Epoch`, UTC milliseconds as one canonical time type; an injectable `EpochSource` clock; Gregorian calendar math; ISO 8601 formatting and parsing. No dependencies by default. |
| [`bc-error-rs`](bc-error-rs/README.md) | `bc_error` | Structured, recursively annotated error values with stable human diagnostics, and extraction helpers for `Result`. |
| [`bc-json-rs`](bc-json-rs/README.md) | `bc_json` | Strict RFC 8259 validation, a lazy zero-copy reader and a fluent writer that cannot emit invalid JSON, bridged to serde. |
| [`bc-nodular-architecture-rs`](bc-nodular-architecture-rs/README.md) | `bc_nodular_architecture` | A runtime-neutral unidirectional data-flow core: nodules, nodes and a stepping loop, with a tokio driver in `bc-nodular-tokio`. |
| [`bc-telemetry-rs`](bc-telemetry-rs/README.md) | `bc_telemetry` | Spans, metrics and span-correlated log records behind one exporter interface, and errors reported as telemetry. |
| [`bc-telemetry-gen`](bc-telemetry-rs/crates/gen/README.md) | `bc_telemetry_gen` | Generates typed telemetry SDKs, PostgreSQL layout and queries, and Grafana dashboards from one definition file. |

Each crate's README is its documentation: its API, its examples (compiled as doctests) and its design.

## Platforms and requirements

The minimum supported Rust version is the root `Cargo.toml`'s `rust-version`, which `rust-toolchain.toml` pins for
local builds; CI also builds and tests on one pinned newer stable release. Every crate builds for
`wasm32-unknown-unknown` except `bc-telemetry-rs`, whose trace ids need an operating-system random source. The
optional `uniffi` feature of `bc-date-time-rs` and `bc-error-rs` generates Swift and Kotlin bindings.

## Stability

The crates are pre-1.0: a minor version may change an API, and each release says what changed in
[CHANGELOG.md](CHANGELOG.md). Pin a tag, as above, and move it deliberately.

## Build from source

```sh
git clone https://codeberg.org/BourgeoisConcernLLC/bc-rs.git
cd bc-rs
scripts/verify.sh
```

`scripts/verify.sh` runs every crate's formatting, tests (through cargo-nextest when it is installed), doctests,
clippy, rustdoc and feature matrix, and its wasm32 build, and ends with one verdict per crate. A tier this machine
cannot run — the wasm32 target, cargo-nextest, a Swift or Kotlin toolchain — is reported by name, and the run is
INCOMPLETE rather than passed unless you accept the skip; [CONTRIBUTING.md](CONTRIBUTING.md) says how.

## Contributing and security

bc-rs does not take outside contributions yet, and it has no private channel for security reports yet; both will be
announced here when they open. [CONTRIBUTING.md](CONTRIBUTING.md) describes how the workspace is built and checked.

## License

Licensed under the Apache License, Version 2.0 (SPDX `Apache-2.0`); the text is in [LICENSE.txt](LICENSE.txt).
Copyright 2026 Jared Bourgeois.

`bc-json-rs` vendors the [JSONTestSuite](https://github.com/nst/JSONTestSuite) parsing corpus, copyright 2016 Nicolas
Seriot, under the MIT License, with that licence and the upstream revision beside it in
[`bc-json-rs/tests/fixtures/JSONTestSuite/`](bc-json-rs/tests/fixtures/JSONTestSuite/).
