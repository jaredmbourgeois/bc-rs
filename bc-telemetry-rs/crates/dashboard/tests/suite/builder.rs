//! The builder and the JSON it renders, held to Grafana's own dashboard schema
//! (`crates/dashboard/schema/dashboard.jsonschema.json`).

use bc_json::JsonReader;
use bc_telemetry_dashboard::{
    Color, Dashboard, DashboardId, DashboardIdError, DataSource, Logs, MeasureQuery, RecordsQuery,
    ResultUnit, Row, Stat, Table, Thresholds, Timeseries, Unit,
};

use crate::json_schema::Schema;

const DASHBOARD_SCHEMA: &str = include_str!("../../schema/dashboard.jsonschema.json");

fn telemetry() -> DataSource {
    DataSource::postgresql("telemetry")
}

fn latency() -> MeasureQuery {
    MeasureQuery::new(
        "95th-percentile request latency",
        "SELECT $__timeGroupAlias(time, '5m'), percentile_cont(0.95) WITHIN GROUP (ORDER BY value) AS value\nFROM telemetry.\"metric__store_server__request_duration\"\nWHERE $__timeFilter(time)\nGROUP BY 1\nORDER BY 1",
        ResultUnit::Of(Unit::Milliseconds),
        telemetry(),
    )
}

fn throughput() -> MeasureQuery {
    MeasureQuery::new(
        "Bytes sent per second",
        "SELECT $__timeGroupAlias(time, '1m'), sum(value) / 60 AS value\nFROM telemetry.\"metric__payments__response_size\"\nWHERE $__timeFilter(time)\nGROUP BY 1\nORDER BY 1",
        ResultUnit::PerSecond(Unit::Bytes),
        telemetry(),
    )
}

fn failed_orders() -> RecordsQuery {
    RecordsQuery::new(
        "An order that did not complete",
        "SELECT time, severity, body, \"reason\"\nFROM telemetry.\"log__store_server__order_failed\"\nWHERE $__timeFilter(time)\nORDER BY time DESC",
        telemetry(),
    )
}

fn checkout_health() -> Dashboard {
    Dashboard::new(
        DashboardId::new("checkout-health").expect("a valid uid"),
        "Checkout health",
    )
    .panel(Stat::with_thresholds(
        "Bytes out",
        throughput(),
        Thresholds::starting(Color::Green)
            .above(1_000_000.0, Color::Orange)
            .above(5_000_000.0, Color::Red),
    ))
    .row(
        Row::new("Latency")
            .panel(Timeseries::new("p95 latency", latency()))
            .panel(Table::new("p95 latency, tabled", latency())),
    )
    .row(Row::new("Failures").panel(Logs::new("Failed orders", failed_orders())))
}

/// The panels of a rendered dashboard, each as a reader.
fn panels_of(json: &str) -> Vec<JsonReader<'_>> {
    let dashboard = JsonReader::new(bc_json::validate_json(json).expect("the dashboard is JSON"));
    dashboard.required_object_array("panels").expect("panels")
}

#[test]
fn a_built_dashboard_is_one_grafanas_schema_holds() {
    let json = checkout_health().to_json().expect("render the dashboard");
    let violations = Schema::new(DASHBOARD_SCHEMA).violations(&json);
    assert!(violations.is_empty(), "{violations:#?}\n{json}");
}

#[test]
fn a_dashboard_says_who_it_is_and_which_schema_version_it_writes() {
    let json = checkout_health().to_json().expect("render the dashboard");
    let dashboard = JsonReader::new(bc_json::validate_json(&json).expect("JSON"));
    assert_eq!(
        dashboard.required_string("uid").expect("uid"),
        "checkout-health"
    );
    assert_eq!(
        dashboard.required_string("title").expect("title"),
        "Checkout health"
    );
    assert_eq!(
        dashboard
            .required_integer("schemaVersion")
            .expect("version"),
        42
    );
    assert!(
        json.ends_with("}\n"),
        "a rendered dashboard ends in a newline"
    );
}

#[test]
fn panels_are_numbered_and_laid_out_with_the_dashboards_own_before_its_rows() {
    let json = checkout_health().to_json().expect("render the dashboard");
    let layout: Vec<(String, i64, i64, i64, i64, i64)> = panels_of(&json)
        .iter()
        .map(|panel| {
            let grid = panel.find_object("gridPos").expect("a grid position");
            let at = |key: &str| grid.required_integer(key).expect("a grid number");
            (
                panel.required_string("type").expect("a type"),
                panel.required_integer("id").expect("an id"),
                at("x"),
                at("y"),
                at("w"),
                at("h"),
            )
        })
        .collect();
    let expected: Vec<(String, i64, i64, i64, i64, i64)> = [
        ("stat", 1, 0, 0, 12, 8),
        ("row", 2, 0, 8, 24, 1),
        ("timeseries", 3, 0, 9, 12, 8),
        ("table", 4, 12, 9, 12, 8),
        ("row", 5, 0, 17, 24, 1),
        ("logs", 6, 0, 18, 24, 10),
    ]
    .iter()
    .map(|(kind, id, x, y, w, h)| ((*kind).to_string(), *id, *x, *y, *w, *h))
    .collect();
    assert_eq!(layout, expected);
}

#[test]
fn a_panel_reads_its_query_as_raw_sql_in_the_unit_the_query_answers_in() {
    let json = checkout_health().to_json().expect("render the dashboard");
    let panels = panels_of(&json);
    let target_of = |panel: &JsonReader<'_>| {
        let targets = panel.required_object_array("targets").expect("targets");
        let [target] = &targets[..] else {
            panic!("one target per panel");
        };
        (
            target.required_string("format").expect("format"),
            target.required_string("rawSql").expect("raw SQL"),
            target.required_boolean("rawQuery").expect("raw query"),
        )
    };
    let unit_of = |panel: &JsonReader<'_>| {
        panel
            .find_object("fieldConfig")
            .and_then(|config| config.find_object("defaults"))
            .and_then(|defaults| defaults.string("unit"))
    };

    let (format, sql, raw) = target_of(&panels[0]);
    assert_eq!(format, "time_series");
    assert!(sql.contains("response_size") && raw, "{sql}");
    assert_eq!(unit_of(&panels[0]).as_deref(), Some("binBps"));

    assert_eq!(target_of(&panels[2]).0, "time_series");
    assert_eq!(unit_of(&panels[2]).as_deref(), Some("ms"));
    assert_eq!(target_of(&panels[3]).0, "table");

    let (format, sql, _) = target_of(&panels[5]);
    assert_eq!(format, "table");
    assert!(sql.ends_with("ORDER BY time DESC"), "{sql}");
    assert_eq!(unit_of(&panels[5]), None, "records have no unit");
    let source = panels[5].find_object("datasource").expect("a data source");
    assert_eq!(
        source.required_string("type").expect("type"),
        "grafana-postgresql-datasource"
    );
    assert_eq!(source.required_string("uid").expect("uid"), "telemetry");
}

#[test]
fn thresholds_start_from_their_base_and_step_up_in_the_order_given() {
    let json = checkout_health().to_json().expect("render the dashboard");
    let stat = &panels_of(&json)[0];
    let thresholds = stat
        .find_object("fieldConfig")
        .and_then(|config| config.find_object("defaults"))
        .and_then(|defaults| defaults.find_object("thresholds"))
        .expect("thresholds");
    assert_eq!(
        thresholds.required_string("mode").expect("mode"),
        "absolute"
    );
    let steps = thresholds.required_object_array("steps").expect("steps");
    let spelled: Vec<(bool, String)> = steps
        .iter()
        .map(|step| {
            (
                step.is_null("value"),
                step.required_string("color").expect("a color"),
            )
        })
        .collect();
    assert_eq!(
        spelled,
        [
            (true, "green".to_string()),
            (false, "orange".to_string()),
            (false, "red".to_string())
        ]
    );
    assert_eq!(steps[2].floating("value"), Some(5_000_000.0));
    // A panel built without thresholds has none.
    assert!(
        panels_of(&json)[2]
            .find_object("fieldConfig")
            .and_then(|config| config.find_object("defaults"))
            .and_then(|defaults| defaults.find_object("thresholds"))
            .is_none()
    );
}

#[test]
fn a_threshold_json_cannot_hold_is_a_rendering_error() {
    let dashboard =
        Dashboard::new(DashboardId::new("bad").expect("uid"), "Bad").panel(Stat::with_thresholds(
            "Never",
            latency(),
            Thresholds::starting(Color::Blue).above(f64::NAN, Color::Purple),
        ));
    assert!(dashboard.to_json().is_err());
}

#[test]
fn a_uid_is_what_grafana_stores_or_is_refused() {
    let forty = "a".repeat(40);
    assert_eq!(
        DashboardId::new(forty.clone()).expect("forty").as_str(),
        forty
    );
    assert!(DashboardId::new("Store_server-checkout-9").is_ok());
    assert_eq!(DashboardId::new(""), Err(DashboardIdError::Empty));
    let forty_one = "a".repeat(41);
    assert_eq!(
        DashboardId::new(forty_one.clone()),
        Err(DashboardIdError::TooLong {
            uid: forty_one.clone()
        })
    );
    assert_eq!(
        DashboardId::new("checkout health"),
        Err(DashboardIdError::Character {
            uid: "checkout health".to_string(),
            character: ' '
        })
    );
    assert_eq!(
        DashboardIdError::TooLong { uid: forty_one }.to_string(),
        format!(
            "the dashboard uid `{}` is 41 characters, past Grafana's 40",
            "a".repeat(41)
        )
    );
    assert_eq!(
        DashboardIdError::Character {
            uid: "a.b".to_string(),
            character: '.'
        }
        .to_string(),
        "the dashboard uid `a.b` holds `.`; a uid is letters, digits, `-` and `_`"
    );
    assert_eq!(
        DashboardIdError::Empty.to_string(),
        "a dashboard uid is empty"
    );
}

#[test]
fn grafanas_schema_refuses_what_this_model_must_never_write() {
    // The oracle is live: a dashboard broken the ways a mistake here would break
    // one is refused, so the green above means something.
    let json = checkout_health().to_json().expect("render the dashboard");
    let schema = Schema::new(DASHBOARD_SCHEMA);
    for (broken, why) in [
        (
            json.replacen("\"schemaVersion\"", "\"schemaVersions\"", 1),
            "a misspelled key",
        ),
        (
            json.replacen("\"w\": 12", "\"w\": 25", 1),
            "a panel wider than the grid",
        ),
        (
            json.replacen("\"collapsed\": false", "\"collapsed\": \"no\"", 1),
            "a mistyped value",
        ),
        (
            json.replacen("\"type\": \"row\"", "\"type\": \"rows\"", 1),
            "a row that is not one",
        ),
    ] {
        assert!(
            !schema.violations(&broken).is_empty(),
            "the schema held {why}"
        );
    }
}
