# bc-date-time-rs

One canonical way to say *when*: a UTC epoch type in milliseconds, an injectable clock, and the Gregorian calendar
math and ISO 8601 formatting and parsing that go with them.

The default build has **no dependencies** and builds for `wasm32-unknown-unknown`; the optional `uniffi` feature
generates Swift and Kotlin bindings.

Depend on it by git and release tag, as the [workspace README](../README.md#depending-on-bc-rs) shows; the package is
`bc-date-time-rs` and the library is imported as `bc_date_time`.

It is a semantics-preserving port of the C++ `libs/foundation/date_time` library of Bourgeois Concern's codebase: the
same API and behaviour, with every case of the original's test suite carried into this crate's tests or set aside for
a recorded reason.

Licensed under Apache-2.0 ([LICENSE.txt](LICENSE.txt)); copyright 2026 Jared Bourgeois.

## Three ideas carry the crate

**`Epoch` is the only way to say when.** Milliseconds since 1970-01-01T00:00:00Z, UTC, as an `i64`.
Integer milliseconds have no floating-point comparison hazards, `i64` covers ±292 million years,
and the unit matches JavaScript's `Date.now()` and most database timestamp columns. The point is
that consumers *stop growing local timestamp types* — one consumer's journal had one, and it is
now a render of this one.

**The clock is injected, never read.** Nothing here calls the system clock except
`system_epoch_source`, which exists so a composition root can hand one down. That is what lets
retention windows, dedup horizons, and rotation decisions be pure functions of their input, tested
without a real clock anywhere in sight.

**UTC, and only UTC.** No timezone database, no local-time concept. The parsers reject an offset
suffix rather than interpreting it, because silently treating `+05:00` as UTC is a wrong answer
where a refusal is a correct one.

```rust
use bc_date_time::{
    DateFormatBuilder, Epoch, EpochDelta, fixed_epoch_source, format_iso_timestamp,
    parse_iso_timestamp,
};

// A clock a test controls completely.
let clock = fixed_epoch_source(Epoch::from_millis(1_771_338_645_123));
let now = clock.now();

assert_eq!(format_iso_timestamp(now), "2026-02-17 14:30:45 UTC");
assert_eq!(DateFormatBuilder::us_long().format(now), "February 17, 2026");

// Formatting and parsing are inverses over the ISO 8601 form.
let rendered = DateFormatBuilder::iso8601().format(now);
assert_eq!(rendered, "2026-02-17T14:30:45.123Z");
assert_eq!(parse_iso_timestamp(&rendered), Some(now));

// Spans are their own type, so one instant cannot be added to another.
let deadline = now + EpochDelta::from_days(3);
assert_eq!(deadline.delta_since(now), EpochDelta::from_hours(72));
```

## Public API

- **`Epoch`** — a `#[repr(transparent)]` newtype over `i64` milliseconds. `Epoch::ZERO`,
  `from_millis` / `millis`, `delta_since`, and `truncated_to_seconds` (which uses `div_euclid`, so
  a pre-1970 instant truncates *backwards* rather than jumping forward). Ordered, hashable, `Copy`.
- **`EpochDelta`** — a `#[repr(transparent)]` signed span in milliseconds, with
  `from_{millis,seconds,minutes,hours,days}` constructors and `millis` / `whole_seconds` /
  `whole_days` accessors. `Epoch - Epoch` yields one; `Epoch ± EpochDelta` moves an instant. All
  arithmetic saturates, so debug and release builds agree.
- **`EpochSource`** — the clock. `EpochSource::new(closure)`, `.now()`, `Clone`, and the two
  factories `system_epoch_source()` and `fixed_epoch_source(epoch)`. Any `Fn() -> Epoch` converts
  via `From`.
- **`Weekday`** (Sunday = 1 … Saturday = 7) and **`Month`** (January = 1 … December = 12), both
  `#[repr(u8)]` with written-out discriminants. Each carries `ALL`, `number`, `zero_indexed`,
  `from_number`, `name`, and `abbr`; `Weekday` adds `is_weekend`.
- **`EpochRange`** — a half-open `[start, end)` interval with `contains` and `duration`.
- **`DateComponents`** — the decomposed instant. Build with `DateComponents::new(..)`, which
  derives `weekday`; convert with `epoch_to_components` and `components_to_epoch`.
- **`IsoWeek`** plus `iso_week_of`, `days_in_month`, and `is_leap_year`.
- **`DateFormatBuilder`** — a typed part list, not a format string. `year`/`month`/`day`/`weekday`/
  `hour`/`minute`/`second`/`millisecond`/`ampm`/`literal`, each with an `_styled` sibling taking an
  explicit style, plus eight static factories (`iso8601`, `us_short`, `us_long`,
  `us_long_with_weekday`, `us_datetime`, `us_datetime_seconds`, `iso_date`, `iso_datetime_minute`).
  `build(&self)` snapshots the current parts into a **`DateFormatter`**, which renders with
  `.format(epoch)`, is `Clone`, and can be held in an ordinary `#[derive(Debug)]` struct.
  `DateFormatter::new(closure)` builds one directly, for a format the builder cannot spell.
- **One-shot formatters** — `format_iso_date`, `format_iso_timestamp`, `format_iso_week_version`.
- **Parsers** — `parse_iso_date` (`YYYY-MM-DD`) and `parse_iso_timestamp`
  (`YYYY-MM-DDTHH:MM:SS[.fff][Z]`), both returning `Option<Epoch>`.
- **`add_business_days`** and **`format_time_ago`**.

### Which spelling to persist

Converging on one `Epoch` type is only half the job; a consumer that invents its own way to *write
it down* re-creates the duplication one layer up. So:

> **A new persisted timestamp uses `DateFormatBuilder::iso8601()` to write and
> `parse_iso_timestamp` to read.** That pair is millisecond-precision, strictly UTC, and mutually
> inverse — the property tests assert it directly.

Narrower spellings exist and are legitimate, but they are *local compromises, not defaults*.
One consumer's journal writes a second-precision variant because its bytes predate this crate and a
byte already persisted is never silently reinterpreted; that renderer is deliberately private to
the journal so it cannot be reached for by accident. If you need a narrower form, own it where it
is written and say why — do not copy someone else's.

## Behaviour worth knowing

This crate keeps its C++ original's behaviour, rough edges included, so these are deliberate:

- **`components_to_epoch` ignores `DateComponents::weekday`** and does not validate its fields.
  Out-of-range values normalize the way `<chrono>` normalizes them — second 60 becomes second 0 of
  the next minute, month 13 becomes next January. `DateComponents::new` derives a correct weekday
  so a caller cannot build an inconsistent struct by accident, but the conversion itself is 1:1.
- **`parse_iso_timestamp` accepts `second == 60`.** A leap second is legal ISO 8601 text; this
  crate has no leap-second table and normalizes rather than pretending to represent one.
- **The trailing `Z` is optional** on `parse_iso_timestamp`, and a bare `.5` fraction means 500ms.
- **`add_business_days` skips only weekends.** No holiday calendar — holidays are jurisdictional
  and this crate has no locale.
- **Years outside 1000–9999 have no ISO 8601 spelling.** They are representable as an `Epoch` and
  render through the builder; the wire syntax simply has a four-digit year field and no
  expanded-year form. The property tests state that domain explicitly.

And one platform limit: `system_epoch_source().now()` **panics on `wasm32-unknown-unknown`**, because
std has no clock there. The crate still *builds* for that target, which `scripts/verify.sh` checks —
and a wasm consumer injects a different `EpochSource`, which is precisely what the seam is for.

## Why the default build has no dependencies

The default build pulls in nothing, and `scripts/verify.sh` fails if that ever changes. This crate
lands in a consumer's persisted records and in the small runtimes a consumer ships to remote hosts,
the most dependency-sensitive place in its build, so the guard is the point rather than a nicety.
The one optional feature below is off by default and invisible to a consumer that does not ask for
it — which is what the guard measures.

That is affordable because the calendar math here *is* the algorithm `chrono` and `jiff` implement.
The C++ original reaches for C++20 `<chrono>`'s calendar types, which are Howard Hinnant's
`days_from_civil` / `civil_from_days` standardized; Rust's std has no calendar, so `src/civil.rs`
writes those two functions out — about forty lines, with the era-shift reasoning in comments.
Taking a dependency would buy nothing this crate does not already have, and would give up the only
reason to prefer it.

## The `uniffi` feature

Off by default. Enabling it adds the UniFFI binding so a Swift or Kotlin application can depend on
this crate directly, and pulls dozens of transitive crates — which is why it is a feature rather
than a dependency. `src/ffi.rs` is the entire foreign contract; read it there rather than hunting
attributes across the crate.

Every bound crate in the family splits its binding into three layers, and the split is what keeps
the Rust API unchanged:

1. **Values cross verbatim.** `Epoch` and `EpochDelta` are custom newtypes over `i64`, `Month` and
   `Weekday` are enums (their written-out `#[repr(u8)]` discriminants were always meant to be part
   of a boundary contract), and `DateComponents`, `IsoWeek`, and `EpochRange` are records.
2. **Closures stay Rust's.** `EpochSource::new` and `DateFormatter::new` still take `impl Fn`. A
   Rust caller never sees a trait, and the family's "a one-method trait is ceremony" rule is
   untouched.
3. **Foreign implementors get a trait.** `ffi::ForeignEpochSource` and `ffi::ForeignDateFormatter`
   exist for callers that cannot hand Rust a closure, and each `from_foreign` constructor collapses
   one into the *same* stored closure layer 2 already holds — one representation, never two paths.

Three things deliberately do not cross. `DateFormatBuilder`'s seventeen part methods take
`&mut self` and chain, a shape UniFFI has no form for, so its eight canonical formats are exported
as free functions instead and the six style enums stay Rust-only. Arithmetic on `Epoch` and
`EpochDelta` stays Rust-side, because they arrive as integers a foreign caller adds to natively.
And this crate stays a plain `lib`: a foreign consumer supplies the thin `cdylib` shim that
`uniffi-bindgen` runs against, rather than every Rust crate that consumes it linking a dynamic
library for a feature they do not enable.

Two boundary facts, probed rather than recalled (uniffi 0.32.0, Rust 1.85):

- **`default-features = false` is load-bearing.** uniffi's default `cargo-metadata` feature pulls
  `cargo_metadata`, which requires rustc 1.86 and hard-fails this crate's 1.85 pin. `askama` and
  `cargo-platform` genuinely do belong to the *generator*, which runs on its own toolchain — and
  generalising from them to this crate is exactly what hid it. `cargo_metadata` is in the runtime
  graph.
- **`unsafe_code = "forbid"` stays.** The scaffolding's `#[unsafe(no_mangle)]` does not trip it,
  because lints do not fire inside an external macro's expansion — verified against a planted
  `unsafe` block, which `forbid` does still reject. The `allow(unsafe_code)` escape hatch would in
  fact be a hard error next to a `forbid` (E0453), so this is the only shape that works, and it is
  also the strictest one.

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

`scripts/verify.sh` is this crate's pipeline; bc-rs's root `scripts/verify.sh` runs it with every
other member's, and CI and the tracked `.idea/runConfigurations/` invoke that one, so "green locally"
and "green in CI" cannot drift by editing one and forgetting the other. Pass `--quick` to skip the
release-profile test run.

```sh
scripts/verify.sh
```

It runs, in order: `cargo fmt --check` · tests in debug · tests in release · the test-collection
guard · doctests · clippy · rustdoc · the feature matrix · a `wasm32-unknown-unknown` build · the
dependency guard · the foreign bindings smoke test.

Four of those exist for reasons worth stating:

- **Tests run under both profiles** because `EpochDelta` arithmetic saturates; a plain `+` would
  panic in debug and wrap in release, and only running one profile would hide the difference.
- **The test-collection guard** fails a compiled test binary that collected zero tests. Both "dead
  code" and "gated out of the run" pass green otherwise — cargo prints `running 0 tests` and exits
  0. There is deliberately no allowlist.
- **The dependency guard** fails if `cargo tree` shows anything but this package. It reads the
  *default* build — no feature flags — so the optional `uniffi` dependency is invisible to it
  exactly as it is to a consumer that does not ask for it. "Dependency-free" is a claim about what
  a consumer gets by default, not about what the crate can be asked for.
- **The wasm32 build takes no features**, and asserts that `uniffi` is the only one declared. That
  step's claim is that *this library* needs no host clock; the scaffolding does compile for wasm32,
  but building it there would neither strengthen the claim nor fail for a reason about this crate.
  The assertion exists so a second feature cannot quietly inherit that decision.
