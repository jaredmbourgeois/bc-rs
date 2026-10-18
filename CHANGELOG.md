# Changelog

What each release changed, newest first. Every crate in a release is released together under the release's tag.
Versions follow semantic versioning: before 1.0, a minor version may change an API, and its entry here says how.

## 0.1.0

The first public release, every package at 0.1.0:

- `bc-core-rs` — the generational arena, the contiguous map, hex, URL, XML and percent encoding, and the assertion
  handler.
- `bc-date-time-rs` — `Epoch`, the `EpochSource` clock, Gregorian calendar math and ISO 8601 formatting and parsing,
  with Swift and Kotlin bindings behind the `uniffi` feature.
- `bc-error-rs` — structured errors and their diagnostics, with serde and UniFFI behind features.
- `bc-json-rs` — strict RFC 8259 validation, the zero-copy reader and the writer, with a serde format behind the
  `serde` feature.
- `bc-nodular-architecture-rs` — the runtime-neutral data-flow core, and `bc-nodular-tokio`, its tokio driver.
- `bc-telemetry-rs` — spans, metrics, log records and their exporters; `bc-telemetry-gen`, the schema, query and
  dashboard generator; and `bc-telemetry-dashboard`, the dashboard model it renders through.
