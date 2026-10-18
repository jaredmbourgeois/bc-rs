# bc-telemetry-gen

`bc-telemetry-gen` reads a tree of telemetry definitions — each a **module**: the spans, metrics,
logs and errors the code in one directory may emit, the named queries over them, the dashboards that
show them — and generates everything downstream, so the emitting code, the queries and the
dashboards cannot drift from one another. This README says how to run it, what a definition file
declares, and what the generator writes from it.

**This crate is not a port.** It has no counterpart in `bourgeois_concern`. It lives beside the
port because its model is the port's own vocabulary: a metric's `type` is one of bc-telemetry-rs's
`MetricKind`s and a log's `level` one of its `Severity`s, so a definition cannot name a kind the
runtime lacks.

## Running it

```sh
bc-telemetry-gen [--file <path>] [--check]
```

A run starts at a **root directory**: the current one, or the one holding the file `--file` names.
The root's definition file is `bc-telemetry.toml` or `bc-telemetry.json`, and so is every module's:
the run walks every directory below the root, and each one that holds a file of that name is a
module. The two syntaxes are equivalent — each is lowered into one positioned document tree, which a
single strict walker reads — so a file means the same thing in either, and every finding names its
`file:line:column`.

The walk skips what would read as a second copy of the tree:

- a directory whose name begins with `.`;
- a directory holding a `CACHEDIR.TAG`, which build tools leave in their output (cargo's `target/`
  holds a copy of every packaged module);
- the generator's own output directories;
- a symbolic link to a directory.

A directory holding the definition in both syntaxes is refused, never picked from, and so is a file
whose name differs from a definition file's by case alone: one file system would read it and
another would not.

A run validates the whole tree before it writes anything, and reports every finding it has rather
than its first: unknown keys (with the nearest known spelling), types, names, an included file whose
`[module]` disagrees with its module's, an item declared twice, two errors under one code, a query
or panel that references nothing declared, an aggregation its measure does not admit. With a
`[generate]` table in the root file it then writes the generated tree; with `--check` it compares
the committed tree with what it would write, and writes nothing — the freshness guard a consumer's
CI runs.

| Exit status | Meaning |
|---|---|
| 0 | valid, and the tree was written or is fresh |
| 1 | findings, each on stderr — or the answer could not be written to stdout |
| 2 | a malformed command line |

The library entry point is the same function the binary calls, so a tool can run the generator
in process:

```rust
let mut stdout = Vec::new();
let mut stderr = Vec::new();
let status = bc_telemetry_gen::run(
    ["--version".to_string()],
    std::path::Path::new("."),
    &mut stdout,
    &mut stderr,
);
assert_eq!(status, bc_telemetry_gen::ExitStatus::Success);
assert!(String::from_utf8(stdout).unwrap().starts_with("bc-telemetry-gen "));
```

## Modules and references

A module's file opens with `[module]`: its `name`, unique across the tree, and its
`schema_version`. It may split its declarations over files with `include`, each of which repeats
that header; an include is a path inside the module's own directory and may not reach into a
subdirectory that is a module of its own. The root file may be a module too, and is the only file
that may carry `[generate]` and `[retired]`.

A query's `measure` and a panel's `query` name a declared item as `<kind>.<name>`, or as
`<kind>.<module>.<name>` in another module. The model always holds the second form, so two
definitions that mean the same thing generate the same bytes. A query measuring an item in another
module follows that module's version: when it bumps, the item's view starts over, since a view reads
only its module's current version, and the query reads empty until new records land.

## The definition file

Beside its header, a module declares items — each a table under its kind, keyed by its name — and
the queries and dashboards over them:

```toml
[span.checkout]
description = "One checkout flow, cart to confirmation"
kind = "server"             # internal | server | client | producer | consumer; internal if absent
attributes = { cart_id = "string", item_count = "int" }
events = ["payment_authorized"]

[metric.request_duration]
description = "Server-side request latency"
type = "histogram"          # counter | gauge | histogram
unit = "ms"                 # none | ms | s | bytes | percent
attributes = { route = "string", status = "int" }

[log.order_failed]
description = "An order that reached checkout and did not complete"
level = "error"             # debug | info | warn | error | fatal
fields = { order_id = "string", reason = "string" }

[error.authorization_declined]
description = "The processor declined the authorization"
class = "failure"           # failure | refusal
code = 1                    # the error's identity within its module, never reused
fields = { order_id = "string", processor_code = "string" }

[query.checkout-p95]
description = "95th-percentile request latency per route over five minutes"
measure = "metric.request_duration"
aggregation = "p95"
window = "5m"               # a positive whole number of s, m, h or d
group_by = ["route"]

[dashboard.checkout-health]
title = "Checkout health"

[[dashboard.checkout-health.panel]]
title = "p95 latency by route"
# A declared query, or — with render = "logs" — a declared log or failure error.
query = "query.checkout-p95"
render = "timeseries"       # timeseries | stat | table | logs
```

Every item and query carries a `description`: it becomes the generated code's doc comments, the
query file's comments and the panel's description, so the definition is the one place a signal is
explained. An attribute or a field is one of a closed set of scalars — `string`, `int`, `float`,
`bool`, `duration` (an integer count of milliseconds) and `digest` (a string) — so no record
carries a raw payload or a free-form map. Records are stored as emitted and aggregated only when
read: a counter's record is one increment, a gauge's one reading, a histogram's one observation.

**Names.** A module, an item, an attribute, a field and an event are `snake_case`; a query and a
dashboard are `kebab-case`, since neither becomes a language identifier directly. No name holds a
`.` or an empty word, so a qualified name splits one way and `__` is free to separate the parts of a
view name. A name is held to the engine that carries it: a view's name,
`<kind>__<module>__<item>`, and an attribute's or a field's, a column of that view, to PostgreSQL's
63 bytes, past which PostgreSQL would truncate it silently into a collision; a dashboard's Grafana
uid, `<module>-<dashboard>`, to Grafana's 40 characters. An error and a log of one module never share a name, since both are stored as log
records.

**Aggregations.** What an aggregation measures is fixed per kind, and a pair outside this table is
refused, since a sum of gauge readings or a percentile of counter increments means nothing:

| Measure | The value aggregated | count | sum | avg, min, max | p50, p90, p95, p99 | rate |
|---|---|---|---|---|---|---|
| counter | the increment | ✓ | ✓ total | — | — | ✓ total per second |
| gauge | the reading | ✓ | — | ✓ | ✓ | — |
| histogram | the observation | ✓ | ✓ | ✓ | ✓ | ✓ observations per second |
| span | its duration in ms | ✓ | ✓ | ✓ | ✓ | ✓ spans per second |
| log, error | — | ✓ | — | — | — | ✓ records per second |

A percentile is continuous: PostgreSQL's `percentile_cont`, interpolated between the two nearest
values. A result is in its measure's unit for a sum, an average, an extreme or a percentile, and
unitless for a count; a counter's rate is in its unit per second, and every other rate is a count per
second.

**Errors.** A declared error is a bc-error kind: its category is the module's name and its value the
`code`, an integer from 0 to 2,147,483,647, unique within the module. The code is persisted wherever
the error is, so it is the error's identity and never reused (the manifest holds that, below). A
`failure` is ours to answer for, and its report calls log it as an error-level record or, answered
past with a lesser result, a warn-level one. A `refusal` is the caller's to correct: it is recorded
on its span and never reported, since logging it would bury the errors that are ours — so it has no
view, and no query or logs panel may name it.

**On the wire.** A record's name is qualified, `<module>.<item>`. Every generated record carries its
module's version in the reserved attribute `bc.telemetry.schema_version`, and a log's or a reported
error's record carries its qualified name in `bc.telemetry.log`, since its body is its description.
A writer of the layout lifts these into the `module`, `name` and `schema_version` columns.

## The generated tree

Generation is deterministic: modules and items in name order whatever order the files declare them
in, and no timestamps, so the tree is committable and a regeneration diffs only what changed.

`[generate] output` is one path used in two places. Under the root's directory it holds the shared
artifacts and `manifest.json`; under each module's own directory it holds that module's emitting
code. **The claim is tree-wide**: with `output = "generated/telemetry"`, that path is the
generator's under the root and under every directory that holds a definition file, so pick one no
other tool writes to. To change `output`, move `manifest.json` to where the new path puts it and
regenerate: the run removes the rest from the old place.

The manifest records every module's lowered vocabulary and directory, and every file the run
produced. It is what the next run reads back to:

- **own its directories and nothing else in them.** A file the manifest lists and this run no
  longer produces is removed; a file no run produced is refused, and nothing is written. A listed
  file outside the directories the manifest itself names is refused, so an edited manifest cannot
  direct a removal elsewhere under the root.
- **hold the additive rule, per module.** Removing or renaming an item, removing or retyping an
  attribute, removing an event, changing a metric's type or unit, and removing, renaming or
  reclassifying an error are refused unless that module's `schema_version` is bumped past the
  committed one; the version never falls, and a bump in one module admits nothing in another.
- **keep what is spent, spent.** An error's `code` is its identity: a rename keeps it, an error
  that keeps its name never changes it, and a removed error's code is retired — recorded here, and
  refused if any later version declares it. A module that leaves the tree is refused until the
  root's `[retired] modules` names it; that list is permanent, and a retired name is never declared
  again.

**The retired codes are recorded in the manifest and nowhere else.** A manifest that is deleted, or
merged badly, forgets them, and no later run can notice. Review it as source.

No declared name can contain a `.`, which makes a qualified name split one way and keeps the
reserved `bc.telemetry.*` attributes every generated emitter stamps on its records out of reach of
every declaration.

## The PostgreSQL layout

With `dialects = ["postgresql"]` in `[generate]`, the run writes these under `<output>/postgresql/`:

- **`layout.sql`** — the layout's data, the same for every tree: the `telemetry` schema, the three
  tables partitioned by day, the days and erasures the ingest step reads, and
  `telemetry.layout_version()`. It is one transaction that brings an empty database to the layout's
  version and refuses to apply over itself; it is a file of this crate (`src/layout/postgresql_v1.sql`),
  written out as it is.
- **`functions.sql`** — the layout's code: the maintenance, erasure, staging and ingest functions, every
  one `CREATE OR REPLACE` in one transaction, applied after `layout.sql` and again whenever it changes,
  so a function's new body reaches a database without a new layout version (a new signature is one).
  An ingest and an erasure of the same customer hold each other off on one advisory lock, and an
  erasure is refused outside a READ COMMITTED transaction. A file of this crate
  (`src/layout/postgresql_functions.sql`), written out as it is.
- **`views.sql`** — one typed view per declared item, `<kind>__<module>__<item>`, at its module's
  current `schema_version`. It drops every view in the schema and creates the tree's, so it is applied
  again whenever the definitions change. An attribute may not take the name of a column its view
  already carries, nor a log's or an error's field `level`, which its records statement adds
  (`reserved-name`).
- **`queries/<module>.sql`** — each module's queries: a statement per named query over its
  measure's view, and one per log and failure error that reads its records newest first — `time`,
  `body`, the severity as `level`, then its fields, the order and names Grafana's logs panel reads —
  each commented with its name, description and the unit of what it answers. They are written for
  Grafana's PostgreSQL data source and its `$__timeFilter` and `$__timeGroupAlias` macros; the suite
  runs every one over the corpus with the macros expanded for a fixed range.
- **`corpus.json`** — the conformance corpus: a record of every declared item, the same batches
  retried, and records that test each rule of the ingest step and of a writer, with the lines a writer
  stages, the counts that must come back and the rows every table and view must then hold. It is
  computed by this crate's own reference writer (`src/layout/corpus.rs`); the suite runs it through
  the generated SQL in a real PostgreSQL (`tests/suite/layout.rs`), and the port's exporter is held to
  it the same way, so neither the reference nor the SQL is its own oracle.

## The Grafana export

With `exports = ["grafana"]` and `[generate.grafana] datasource = "<uid>"` — the uid of the Grafana
data source that reads the layout — the run writes under `<output>/grafana/`:

- **`dashboards/<module>-<dashboard>.json`** — each declarative dashboard, lowered into
  `bc-telemetry-dashboard`'s model and rendered as Grafana's classic dashboard JSON. Each panel's
  target is the statement its query file holds, in the unit its query answers in. The suite holds
  every one to Grafana's own dashboard schema.
- **`queries.ts`** — every query's handle, and every log's and failure error's records handle, for
  a TypeScript program that builds dashboards with Grafana's Foundation SDK: each is a target the
  SDK's panels take, `.withTarget(handle)`. `tests/typescript/checkout-health.ts` is such a program;
  the suite type-checks it against the SDK, runs it and holds what it prints to the same schema and
  to the committed `checkout-health.expected.json`, where Node and the SDK are present
  (`bc-telemetry-rs`'s README, §Verification, says how).
- **`queries.rs`** — the same handles for a Rust program that builds dashboards with
  `bc-telemetry-dashboard`'s builder, brought in with `include!`: per module,
  `queries::<module>::queries::<query>()` returns a `MeasureQuery` and
  `queries::<module>::records::<item>()` a `RecordsQuery`, each reading `data_source()`, so a panel
  is handed a query that exists or does not compile. A name that is a Rust keyword ends in `_`. The
  suite builds a declarative dashboard's twin from them and holds it to the generated JSON
  (`tests/suite/grafana.rs`).

Every one of those dashboards then meets a real Grafana (`tests/suite/grafana_in_the_loop.rs`):
the generated ones and the TypeScript program's golden are provisioned into the pinned Grafana beside
a PostgreSQL holding the corpus, fetched back from its API, and every panel's query runs through
Grafana's own data source and must answer with rows — a logs panel's, as a frame Grafana's logs panel
reads as log lines.

The export reads the PostgreSQL layout, so it needs `dialects = ["postgresql"]`. A `grafana` export
without its table, or the table without the export, is refused rather than guessed at.

## The C++ adapter

With `languages = ["cpp"]`, every module gets one header in its own directory,
`<module dir>/<output>/cpp/<module>_telemetry.hpp`, emitting onto `bourgeois_concern`'s telemetry
and error-telemetry libraries. Its basename and its namespace,
`<module>_telemetry`, are unique across the tree, so a build can put every module's header on one
include path and reach each from inside any `bc::` namespace.

```cpp
store_server_telemetry::spans::checkout(cart_id, item_count)
    .run(bc::telemetry::Parent::current(), end, [](const auto& span) { span.payment_authorized(); });
store_server_telemetry::metrics::request_duration(route, status).record(bc::telemetry::Parent::current(), elapsed);
store_server_telemetry::logs::order_failed(order_id, reason).emit(bc::telemetry::Parent::current());

auto error = payments_telemetry::errors::authorization_declined(order_id, processor_code);
payments_telemetry::report::authorization_declined(error);   // or report_degraded::…, or record_error(error.error())
```

Every verb takes its choices as arguments, the parent first; a span's `start`, `open` and `run`
yield handles with one method per declared event, recorded on that span. An error's kind is in
`error_kinds` — a `bc::RefusalKind` for a refusal — and its constructor in `errors` returns the
error's own class, `errors::AuthorizationDeclined`, holding the `bc::AnyError` (`error()`,
`into_error()`); only a failure has report calls, and they take only that class. An error that
arrives as a `bc::AnyError` becomes its class through `from_error`, a `bc::Result` whose error is
the header's `errors::NotThisError`, holding the error unchanged and the declared field it lacks, if
that is why. Every call into the runtime spells every argument, so the header survives
`bourgeois_concern` losing its default arguments. A name C++ cannot use — a keyword, an event called
`guard` or `span`, an error field called `location`, a span called `scope`, `held` or `running` (the
handle classes nested in it), an error called `not_this_error` — is refused where it is declared.

The header is all the adapter writes: C++20, every definition in it inline, so nothing is compiled
on the generator's side. A consumer adds each module's `<module dir>/<output>/cpp` directory to its
target's include path and links the error-telemetry library, the build target
`bc::error_telemetry`, which brings the telemetry runtime, the error library and the logger with
it. The header includes those libraries' headers by bare name — `telemetry.hpp`,
`record_error_telemetry.hpp`, `log.hpp`, `error.hpp`, `any_error_kind.hpp` and `result.hpp` — and
linking their targets puts them on the include path. The consumer commits the headers and runs
`--check` in its CI to keep them fresh, as for every generated file.

## The Rust adapter

With `languages = ["rust"]`, every module gets one source file in its own directory,
`<module dir>/<output>/rust/<module>_telemetry.rs`, emitting onto
bc-telemetry-rs and bc-error-rs, which the consuming crate depends on. The file holds the module's
items, not a module: the consumer names it where it brings it in, and through `include!` the file is
the generator's to format — `rustfmt` does not follow it — while `--check` keeps it fresh.

```rust
use bc_telemetry::{Parent, ends_ok};

// The suite's canonical tree; a consumer names the path to its own.
mod store_server {
    include!("tests/fixtures/canonical/generated/telemetry/rust/store_server_telemetry.rs");
}
mod payments {
    include!("tests/fixtures/canonical/services/payments/generated/telemetry/rust/payments_telemetry.rs");
}

store_server::spans::checkout("cart-1", 3)
    .run(Parent::current(), ends_ok, |span| span.payment_authorized());
store_server::metrics::request_duration("/cart", 200).record(Parent::current(), 12.5);
store_server::logs::order_failed("order-2", "declined").emit(Parent::current());

let error = payments::errors::authorization_declined("order-3", "51");
payments::report::authorization_declined(&error); // or report_degraded::…, or record_error(error.error())
let propagated: bc_error::AnyError = error.into(); // what a `Result<_, AnyError>` carries on
```

Every verb takes its choices as arguments, the parent first: a span's `start`, `open`, `run` and
`run_async`, a metric's `record`, a log's `emit`. An item's types live in a module of its name —
`spans::checkout::{Span, Held, Running}`, `errors::authorization_declined::Error` — so no two
items' types collide whatever they are called. `start` returns the runtime's own `SpanGuard`, so
`clippy.toml`'s `await-holding-invalid-types` entry for it refuses one held across an `.await` (a
consumer adds the same entry); its events are `spans::checkout::Running::of(guard.span())`'s. The
held and running spans have one method per declared event, recorded on that span, beside the
runtime's own handle (`span()`, `into_span()`). An error's kind is a constant in `error_kinds` — a
`RefusalKind` for a refusal — and its `#[track_caller]` constructor in `errors` returns the error's
own type, holding the `AnyError` (`error()`, `into_error()`, `From`); only a failure has report
calls, and they take only that type, so no report names another error. An error that arrives as an
`AnyError` becomes its type through `try_from_error`, which hands back any other — of another
kind, or of this kind without a declared field — unchanged in a `bc_telemetry::NotThisError`. A
name Rust cannot use — a keyword, or an event called for one of the handles' own methods — is
refused where it is declared. Every runtime path is spelled from the crate root
(`::bc_telemetry::…`), so nothing the consumer has in scope changes what the file means.

The suite's smoke (`tests/suite/rust_adapter.rs`) compiles the canonical tree's three files into the
suite the way a consumer does and drives every kind of item through the runtime into an in-memory
exporter; it needs nothing beyond this workspace, so it runs everywhere the suite does.

## What is implemented

Discovery, validation across modules, the merge of included files, errors with declared codes, the
closed unit set, the aggregation table, the manifest, `--check`, the evolution rules, the
PostgreSQL layout, the Grafana export, and the C++ and Rust adapters above. The one `[generate]` key
that selects what else to generate, `routing`, is recognized and refused as not yet implemented: the
initialization entry point it would generate, installing the exporter it names, has no consumer yet,
and a consumer that installs its own exporter needs none.

## Dependencies, and why each is here

- **bc-telemetry-rs** — the kinds the vocabulary is closed over.
- **bc-telemetry-dashboard** — the dashboard model the declarative dashboards lower into, and the
  unit set a metric's `unit` names.
- **bc-date-time-rs** — the corpus writes its instants as ISO-8601 with the family's formatter, the
  one every writer of the layout uses.
- **bc-json-rs** — the JSON front end reads through the family's strict validator, which refuses
  duplicate keys and reports byte positions; the manifest is written with its writer.
- **`toml_edit`**, parse feature only — every key and value keeps its byte span, which a
  `file:line:column` finding needs. Its closure (`toml_parser`, `toml_datetime`, `winnow`,
  `indexmap`, `hashbrown`, `equivalent`) is this crate's alone.

bc-telemetry-rs's `scripts/verify.sh` runs this crate's tests, lints and documentation with the
port's, and pins its dependency set. The layout suite needs PostgreSQL's programs on the host;
bc-telemetry-rs's README, in its Verification section, says how they are found.
