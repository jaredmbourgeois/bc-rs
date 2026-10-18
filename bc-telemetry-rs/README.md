# bc-telemetry-rs

OTel-shaped telemetry — spans, metrics, and span-correlated log records — behind one exporter interface, and errors
recorded and reported as telemetry. It is deliberately small, where the official OpenTelemetry SDK is not: a value
model, a span stack, a pipeline, and four exporters in the default build (three more behind features).

It needs no async runtime unless its `batched` exporter is enabled, which is the one feature that brings tokio. Its
sibling crates are the schema tooling: [`bc-telemetry-gen`](crates/gen/README.md) generates typed SDKs, the PostgreSQL layout and its
queries, and Grafana dashboards from one definition file, and [`bc-telemetry-dashboard`](crates/dashboard/README.md)
is the dashboard model it renders through.

Depend on it by git and release tag, as the [workspace README](../README.md#depending-on-bc-rs) shows; the package is
`bc-telemetry-rs` and the library is imported as `bc_telemetry`.

It is a semantics-preserving port of the C++ `libs/platform/telemetry` library, with `libs/platform/error_telemetry`
beside it, of Bourgeois Concern's codebase: the same API and behaviour, with every case of the original's test suites
carried into this crate's tests or set aside for a recorded reason.

Licensed under Apache-2.0 ([LICENSE.txt](LICENSE.txt)); copyright 2026 Jared Bourgeois.

## Three rules carry the crate

1. **Telemetry never fails the caller.** The source's load-bearing rule, kept verbatim. Every
   recording call is infallible *by signature* — nothing here returns a `Result`. No pipeline
   installed means a per-call drop; an exporter that cannot write drops and counts.
2. **Dropping is permitted; hiding a drop is not.** Every drop is counted, readable through
   `Pipeline::dropped`, and the file exporter periodically emits the count as a record of its own —
   so a reader tailing the file sees a gap rather than a seamless lie.
3. **Telemetry is exhaust, never evidence.** Nothing here is meant to be read back by a decision:
   telemetry may drop, so it proves nothing.

```rust
use std::sync::Arc;
use bc_telemetry::{
    Attributes, InMemoryExporter, Parent, PipelineBuilder, SpanKind, StatusCode, ends_ok,
    running_span,
};

let exporter = Arc::new(InMemoryExporter::new());
let _pipeline = PipelineBuilder::new(exporter.clone()).install();

// Every choice is an argument: the parent, the kind, the attributes, and how
// the body's result ends the span.
let answer = running_span(
    Parent::current(),
    "compute",
    SpanKind::Internal,
    Attributes::new(),
    ends_ok,
    |_span| 6 * 7,
);

assert_eq!(answer, 42);
assert_eq!(exporter.spans()[0].name, "compute");
assert_eq!(exporter.spans()[0].status, StatusCode::Ok);
```

## Public API

- **`TraceId` / `SpanId` / `SpanContext`** — 16 and 8 bytes, `to_hex`, `is_valid`, and the
  `TRACE_FLAG_SAMPLED` bit. All-zero is invalid per the W3C spec.
- **`Span` / `SpanEvent` / `SpanKind` / `StatusCode` / `AttributeValue` / `Attributes`** — the value
  model an exporter receives. `Attributes` is an ordered `Vec` of pairs, not a map, matching the
  source and the OTel model.
- **Spans** — a `Parent` names what a new span, a log record or a metric point belongs to:
  `Parent::current()`, `Parent::span(&open)`, `Parent::remote(context)` or `Parent::none()`.
  `start_span` returns a `SpanGuard`, current on this thread until it ends; `open_span` returns an
  `OpenSpan`, held by whoever has it and current nowhere until `with_span` lends it. `running_span`
  and `with_span_ending` end a span with the status an end strategy gives the body's result
  (`ends_ok`, `ends_by_result`, or the caller's own), as a `SpanStatus`. The current span's
  mutators — `add_span_event`, `set_span_status`, `set_span_attribute` — count and report a misuse
  with none current; their `*_if_any` forms do neither. `current_span_context` reads it.
  Asynchronous code uses `with_span_async` (an `InSpan` future) and `running_span_async`.
- **`IdSource`** — where trace and span ids come from. `system_id_source()` and `fixed_id_source()`.
- **`Sampler`** — `always_on_sampler()` and `probability_sampler(ratio)`, plus `SamplingDecision`.
- **Metrics** — `record_counter`, `record_histogram`, `record_gauge`, over `MetricPoint`/`MetricKind`.
- **Logs** — `record_log` over `LogRecord`/`Severity`.
- **Error telemetry** — `record_error_telemetry` records an error as an event on the current span
  and marks it errored; `report_error` and `report_degraded` also write an error- or warn-level log of
  the error's identity (`error_identity_attributes`: category, kind, location, context chain and the
  five structured facts — never its free-text metadata). `NotThisError` (with `NotThisErrorReason`)
  is what a generated error type's `try_from_error` hands back for an error that is not it.
- **`ObservationId`** — what identifies a metric point or a log record: its pipeline's producer and
  that producer's count. Every `MetricPoint` and `LogRecord` carries one.
- **Pipeline** — `PipelineBuilder`, `PipelineScope`, `installed_pipeline`, `shutdown_pipeline`, and
  `Pipeline::next_observation`. `drop_counts()` and `dropped_count()` read what the installed
  pipeline dropped, by `DropReason`.
- **Exporters** — `InMemoryExporter`, `StderrExporter` (with `StderrExporterScope`),
  `CompositeExporter`, and `FileExporter` (with `FileExporterConfig`, `LINE_SCHEMA_VERSION`).
  Behind features: `BatchedExporter` (`batched`), `OtlpExporter` + `OtlpExporterConfig` (`otlp`),
  and `PostgresExporter` with `PostgresBatchOutcome` and `PostgresWriteError` (`postgres`). The
  PostgreSQL exporter builds no TLS, so it writes only to a
  database on its own host (a Unix socket or a loopback address) and refuses any other, or an
  `sslmode` asking for TLS, as a counted drop naming why; its session bounds a statement at 15 s and
  a lock wait at 5 s. A `BatchedExporter` built outside a tokio runtime drops and counts every record
  rather than writing on the caller's thread.

## Where async is, and is not

**Synchronous flush-on-drop is the default export mode, and that is load-bearing rather than a
simplification.** A one-shot process — a command-line tool's single invocation — exits after one
operation and cannot host a batching task, so a crate whose only export path was batched-async would
be unusable there. `PipelineScope` flushes on drop, so `install` at the top of `main` covers every
exit path including an early return.

Batched asynchronous export is the **`batched`** feature, which is the only thing that pulls tokio.
It stays off by default for a second reason too: a consumer that does not run tokio never compiles
it.

## Dependencies, and why each is here

The default build takes five, all but one from this family:

- **bc-date-time-rs** — `Epoch` and the injectable `EpochSource`. Not optional: a pipeline holds a
  clock and every record carries an instant.
- **bc-core-rs** — `encode_hex` / `decode_hex_into`. The C++ `types.cpp` and `propagation.cpp` reach
  for the same primitive by the same name; writing a second hex codec for trace ids would duplicate
  something this family already owns. And the process's assertion handler, which a misuse of the
  span API is reported to, as the C++ `context.cpp` reports through `bc::core`'s.
- **bc-json-rs** — the file exporter's line-delimited JSON is a versioned surface that readers
  depend on, and a second JSON escaper is exactly the primitive that must not be rewritten.
- **bc-error-rs** — the family's one error vocabulary, which `error_telemetry` records: an error's
  identity and its five structured facts, as the C++ `error_telemetry` reads them from `bc_error`.
  bc-json-rs's strict read tier carries it too. It is third-party-free, so the third-party closure is
  unchanged.
- **getrandom** — trace and span ids. The C++ `random.cpp` fills them from the OS CSPRNG
  (`arc4random_buf` / `getrandom` / `BCryptGenRandom`); Rust cannot reach those without `unsafe`,
  which this crate forbids. It is the crate the ecosystem's randomness bottoms out in, and it sits
  behind the injectable `IdSource`, so no test depends on it.

Optional: **tokio** (`batched`), **ureq** (`otlp`), **postgres** (`postgres`).

## Verification

`scripts/verify.sh` is this member's pipeline; bc-rs's root `scripts/verify.sh` runs it with every
other member's, passing on `--quick` and each `--accept-skip` whose tier this member has, and CI and
the tracked `.idea/runConfigurations/` invoke that one. Pass `--quick` to leave out the
release-profile test run; the verdict names it as a skip.

```sh
scripts/verify.sh
```

**A run that skipped a tier has not passed it.** The tiers below that need something beyond Rust —
`typescript`, `grafana`, and `test-collection` without cargo-nextest — each say `SKIPPED` with the
reason where it is absent, and the run then ends INCOMPLETE, exit 3, unless the caller accepted every
skip with `--accept-skip <tier>=<why>`. It says "all checks passed" only when nothing was skipped,
names each accepted skip with its why otherwise, and fails an accepted skip whose tier ran anyway, so
the list cannot outlive its reason (bc-rs's `scripts/skip-verdict.sh`, which every member's verdict
goes through, and whose `--selftest` the root pipeline runs). There is no default list. CI's workflow
accepts the TypeScript proof and the Grafana tier, each with the reason its runner cannot run it, and
runs everything else.

**The versions of the programs the tests run that are not Rust crates live in
`scripts/tool-pins.sh`**, each spelled there and nowhere else; what a pin fixes beside it — the
dashboard model's `schemaVersion`, the Foundation SDK the TypeScript proof installs, the vendored
schema's hash — is listed with it there. A pin is not a floor: a floor is the oldest version the code
relies on, a pin the one version the tests run on, here and on the CI runner alike. Moving a pin is
one edit there.

The PostgreSQL suites — the generator's layout and this crate's exporter — start a throwaway server
per test, listening on a private socket only, so the host needs PostgreSQL's programs, of the pinned
major: every PostgreSQL test runs on that one major and on no other.
`scripts/check-test-host-programs.sh` declares every program the tests run, finds the pinned major's,
which `verify.sh` then puts first on `PATH`, and refuses a host with only another major by name,
with the line that installs the pinned one; CI runs it before anything is built. The programs come
from the host's own packages at that major — Homebrew's `postgresql@<major>` formula on a Mac,
Debian's `postgresql-<major>` package on the CI runner — so the patch release moves with the host's
updates and the major never does.

The generator's TypeScript proof — a dashboard built with Grafana's Foundation SDK from the
generated query handles — needs Node, from the pinned floor, and the SDK, which
`scripts/fetch-typescript-sdk.sh` fetches from a committed lockfile. `verify.sh` runs it where both are
present and prints `SKIPPED` with the reason where they are not; nothing fetches during a test.

The generator's Grafana tier — every generated dashboard provisioned into a real Grafana beside a real
PostgreSQL, and every panel's query run through it — needs the pinned Grafana and its PostgreSQL data
source, which `scripts/fetch-grafana.sh ~/.cache/bc-telemetry-rs` fetches from Grafana's own release
and plugin catalog, each checked against its pinned hash (about 450 MB, once). `verify.sh` runs the
tier where they are fetched there and prints `SKIPPED` with the reason where they are not.

### Workspace members

This member is three packages of bc-rs's workspace. Its top package is the port, and everything above
this section is about it. `crates/gen` holds `bc-telemetry-gen`, the telemetry-as-code generator: it
reads a tree of telemetry modules and generates what is downstream of them, with this port's
`MetricKind` and `Severity` as the kinds its vocabulary is closed over; `crates/gen/README.md` says
what it reads and what it writes. `crates/dashboard` holds `bc-telemetry-dashboard`, the typed model
of Grafana dashboards the generator renders through and a program links to build dashboards of its
own. Neither is a port, and its README says so; the generator is a consumer of the port, never a
dependency of it.

The guards say which package each rule covers. `scripts/verify.sh` runs formatting, tests,
doctests, lints and documentation across the three packages and pins each one's dependency set
separately — the port's unchanged, the generator's adding only `toml_edit` and its closure, the
dashboard model's the family's JSON writer alone.
