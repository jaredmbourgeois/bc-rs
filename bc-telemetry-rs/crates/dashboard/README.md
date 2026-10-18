# bc-telemetry-dashboard

A typed model of Grafana dashboards over the bc-telemetry storage layout, and the builder that writes
them. It renders Grafana's classic dashboard JSON — the dashboard, its rows, and timeseries, stat,
table and logs panels whose one target is a raw SQL query on Grafana's PostgreSQL data source —
through the family's JSON writer, for Grafana's file provisioning to read.

It is not a port: nothing in `bourgeois_concern` draws dashboards.

## Two front doors, one model

- **Declarative:** `bc-telemetry-gen` lowers a module's `[dashboard.*]` tables into this model and
  writes each to `<output>/grafana/dashboards/<uid>.json`.
- **Programmatic:** the builder below, for whatever the declarative tables do not say — rows,
  thresholds. Its panels take query handles, so a panel shows a query the generator wrote rather
  than SQL typed by hand.

```rust
use bc_telemetry_dashboard::{
    Color, Dashboard, DashboardId, DataSource, MeasureQuery, ResultUnit, Row, Thresholds,
    Timeseries, Unit,
};

let telemetry = DataSource::postgresql("telemetry");
let checkout_p95 = MeasureQuery::new(
    "95th-percentile request latency per route over five minutes",
    "SELECT $__timeGroupAlias(time, '5m'), ...",
    ResultUnit::Of(Unit::Milliseconds),
    telemetry,
);
let dashboard = Dashboard::new(DashboardId::new("checkout-health")?, "Checkout health").row(
    Row::new("Latency").panel(Timeseries::with_thresholds(
        "p95 latency by route",
        checkout_p95,
        Thresholds::starting(Color::Green).above(500.0, Color::Red),
    )),
);
let json = dashboard.to_json()?;
assert!(json.contains("\"uid\": \"checkout-health\""));
# Ok::<(), Box<dyn std::error::Error>>(())
```

The constructors take every value the model requires and the methods only add — a panel, a row, a
threshold step — so no later call can override what a constructor chose. A query that measures
(`MeasureQuery`) and one that reads records (`RecordsQuery`) are different types, so a logs panel
cannot be handed a measurement or a chart a log. A uid is refused past Grafana's 40 characters, or
holding anything but letters, digits, `-` and `_`, whichever door named it.

A dashboard's own panels are laid out before its first row, two measure panels to a line and a logs
panel to a line of its own: Grafana places a panel that follows a row inside that row.

## The oracle

Every dashboard this crate renders is validated in its tests against Grafana's own dashboard JSON
Schema, vendored at `schema/dashboard.jsonschema.json`:

| | |
|---|---|
| Source | `https://raw.githubusercontent.com/grafana/grafana-foundation-sdk/b3d52a7cbaffff15aeada9a7dbc55be1a417bdab/jsonschema/dashboard.jsonschema.json` |
| Revision | grafana-foundation-sdk `b3d52a7` ("Prepare v0.0.18 release", 2026-06-12), the last commit to touch the file when it was vendored on 2026-10-12 |
| SHA-256 | `67451cd5b288f97e3f68eccd52ffa4f75151fc87c669b8cd93c051056b294265` |
| Draft | JSON Schema draft-07, self-contained (`definitions`, local `$ref`s only) |
| Pinned with | Grafana 13.2.3, whose newest dashboard `schemaVersion` is the 42 this model writes, and Foundation SDK `0.0.20`, which carries this file byte for byte (`scripts/tool-pins.sh`) |
| Re-verify | a move of the Grafana pin in `scripts/tool-pins.sh`, or a change to the file upstream |

The validator is the workspace's own, in `tests/support/json_schema.rs`, over the keywords this
schema uses. The two JSON Schema crates measured on 2026-10-12 each brought a closure of 56 and 79
crates, ICU and `syn` among them, for a test-only check; the keyword set a self-contained draft-07
schema needs is small enough to hold, and each keyword's refusal is a planted test.

## Dependencies, and why each is here

- **bc-json-rs** — the family's JSON writer renders every dashboard.
- **bc-error-rs** and **bc-core-rs** — not named here; they arrive through bc-json-rs, whose
  errors are the family's error vocabulary, which reports an invariant breach to bc-core-rs's
  assertion handler. Both are first-party and third-party-free.
