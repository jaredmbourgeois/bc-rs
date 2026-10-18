//! The Grafana export: every dashboard the canonical tree generates holds
//! Grafana's own dashboard schema, and every panel shows the statement its
//! module's query file holds — one SQL text, written once, read by both.

use std::path::{Path, PathBuf};

use bc_json::JsonReader;

use crate::json_schema::Schema;
use crate::support::fixture;

#[allow(
    dead_code,
    unreachable_pub,
    reason = "a dashboard program uses the handles it needs; this one builds a single dashboard"
)]
mod queries {
    include!("../fixtures/canonical/generated/telemetry/grafana/queries.rs");
}

const DASHBOARD_SCHEMA: &str = include_str!("../../../dashboard/schema/dashboard.jsonschema.json");

fn generated(relative: &str) -> PathBuf {
    fixture("canonical")
        .join("generated/telemetry")
        .join(relative)
}

fn dashboards() -> Vec<(PathBuf, String)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(generated("grafana/dashboards"))
        .expect("the dashboards")
        .map(|entry| entry.expect("a dashboard").path())
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|file| {
            let text = std::fs::read_to_string(&file).expect("read a dashboard");
            (file, text)
        })
        .collect()
}

fn name_of(file: &Path) -> String {
    file.file_name()
        .expect("a file name")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn every_generated_dashboard_holds_grafanas_schema() {
    let schema = Schema::new(DASHBOARD_SCHEMA);
    let dashboards = dashboards();
    let names: Vec<String> = dashboards.iter().map(|(file, _)| name_of(file)).collect();
    // A dashboard per declaration, named for its uid, `<module>-<dashboard>`.
    assert_eq!(
        names,
        [
            "payments-payments-health.json",
            "store_server-checkout-health.json"
        ]
    );
    for (file, text) in &dashboards {
        let violations = schema.violations(text);
        assert!(violations.is_empty(), "{}: {violations:#?}", name_of(file));
    }
}

/// Every statement a module's query file holds, without its comment or its `;`.
fn statements_in_query_files() -> Vec<String> {
    let mut statements = Vec::new();
    for entry in std::fs::read_dir(generated("postgresql/queries")).expect("the query files") {
        let text = std::fs::read_to_string(entry.expect("a query file").path())
            .expect("read a query file");
        for block in text.split("\n\n").skip(1) {
            let statement: Vec<&str> = block
                .lines()
                .filter(|line| !line.starts_with("--"))
                .collect();
            statements.push(statement.join("\n").trim_end_matches(';').to_string());
        }
    }
    statements
}

#[test]
fn every_panel_reads_a_statement_its_module_query_file_holds() {
    let statements = statements_in_query_files();
    let mut panels_read = 0;
    for (file, text) in dashboards() {
        let dashboard = JsonReader::new(bc_json::validate_json(&text).expect("JSON"));
        for panel in dashboard.required_object_array("panels").expect("panels") {
            let title = panel.required_string("title").expect("a title");
            let targets = panel.required_object_array("targets").expect("targets");
            let [target] = &targets[..] else {
                panic!("{}: `{title}` has one target", name_of(&file));
            };
            let sql = target.required_string("rawSql").expect("raw SQL");
            assert!(
                statements.contains(&sql),
                "{}: `{title}` reads SQL no query file holds:\n{sql}",
                name_of(&file)
            );
            let source = target.find_object("datasource").expect("a data source");
            assert_eq!(source.required_string("uid").expect("a uid"), "telemetry");
            panels_read += 1;
        }
    }
    // The canonical tree's two dashboards have six panels and four.
    assert_eq!(panels_read, 10);
}

#[test]
fn a_panel_carries_the_unit_its_query_answers_in() {
    let (_, text) = dashboards()
        .into_iter()
        .find(|(file, _)| name_of(file) == "store_server-checkout-health.json")
        .expect("the checkout dashboard");
    let dashboard = JsonReader::new(bc_json::validate_json(&text).expect("JSON"));
    let units: Vec<(String, Option<String>)> = dashboard
        .required_object_array("panels")
        .expect("panels")
        .iter()
        .map(|panel| {
            let unit = panel
                .find_object("fieldConfig")
                .and_then(|config| config.find_object("defaults"))
                .and_then(|defaults| defaults.string("unit"));
            (panel.required_string("type").expect("a type"), unit)
        })
        .collect();
    let expected: Vec<(String, Option<String>)> = [
        ("timeseries", Some("ms")),
        ("stat", Some("cps")),
        ("table", Some("none")),
        ("timeseries", Some("s")),
        ("logs", None),
        ("logs", None),
    ]
    .iter()
    .map(|(kind, unit)| ((*kind).to_string(), unit.map(str::to_string)))
    .collect();
    assert_eq!(units, expected);
}

/// A dashboard built in code: one built with the builder from the generated
/// Rust handles renders exactly what the declarative dashboard renders for the
/// same panels — one model, so one dashboard whichever way it was written, and
/// no panel naming a query that does not exist.
#[test]
fn a_dashboard_built_from_the_rust_handles_is_the_declarative_one() {
    use bc_telemetry_dashboard::{Dashboard, DashboardId, Logs, Stat, Timeseries};

    let built = Dashboard::new(
        DashboardId::new("payments-payments-health").expect("a uid within Grafana's bound"),
        "Payments health",
    )
    .panel(Timeseries::new(
        "p90 settlement time by processor",
        queries::payments::queries::settlement_p90(),
    ))
    .panel(Stat::new(
        "Response bytes",
        queries::payments::queries::response_bytes(),
    ))
    .panel(Timeseries::new(
        "Checkouts per second",
        queries::store_server::queries::checkout_rate(),
    ))
    .panel(Logs::new(
        "Processor timeouts",
        queries::payments::records::processor_timeout(),
    ));

    let declared = std::fs::read_to_string(
        fixture("canonical")
            .join("generated/telemetry/grafana/dashboards/payments-payments-health.json"),
    )
    .expect("the declarative door's dashboard is generated");
    assert_eq!(built.to_json().expect("the model renders"), declared);
}
