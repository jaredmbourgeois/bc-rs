//! The generated PostgreSQL layout, run in a real PostgreSQL: the DDL applies,
//! the ingest step returns the counts the corpus says it must, the tables and
//! every typed view hold the rows the corpus says they must, and the
//! maintenance and erasure functions do what their names say.
//!
//! The corpus is the generator's own reference, computed in Rust; the layout is
//! SQL. Neither is the other's oracle: this suite is where the two are held
//! together.
#![cfg(unix)]

use std::path::{Path, PathBuf};

use bc_json::JsonReader;

use crate::postgres_cluster::{Cluster, FIELD_SEPARATOR};
use crate::support::fixture;

/// A short directory for a cluster's socket: its path has a hard limit that a
/// cargo target directory can pass on its own.
pub(crate) const SOCKET_ROOT: &str = "/tmp";

fn start_a_cluster() -> Cluster {
    let cluster = Cluster::start(
        Path::new(env!("CARGO_TARGET_TMPDIR")),
        Path::new(SOCKET_ROOT),
    );
    println!("PostgreSQL {}", cluster.version());
    cluster
}

fn generated(file: &str) -> PathBuf {
    fixture("canonical")
        .join("generated/telemetry/postgresql")
        .join(file)
}

/// The layout's data and its functions, with no views.
fn with_the_data_and_functions() -> Cluster {
    let cluster = start_a_cluster();
    cluster.apply(&generated("layout.sql"));
    cluster.apply(&generated("functions.sql"));
    cluster
}

fn with_the_layout() -> Cluster {
    let cluster = with_the_data_and_functions();
    cluster.apply(&generated("views.sql"));
    cluster
}

/// A cluster holding the layout and every record of the corpus, for a suite
/// that reads what the corpus wrote.
pub(crate) fn with_the_corpus() -> Cluster {
    let cluster = with_the_layout();
    let text = std::fs::read_to_string(generated("corpus.json")).expect("read the corpus");
    let corpus = JsonReader::new(bc_json::validate_json(&text).expect("the corpus is JSON"));
    load_the_corpus(&cluster, &corpus);
    cluster
}

/// The four counts an ingest function answered with, as the corpus words them.
fn counts_of(row: &str) -> [(&'static str, i64); 4] {
    let numbers: Vec<i64> = row
        .split(FIELD_SEPARATOR)
        .map(|number| number.parse().expect("an ingest count is a number"))
        .collect();
    let [inserted, out_of_range, erased, duplicate] = numbers[..] else {
        panic!("an ingest function answers with four counts, got `{row}`");
    };
    [
        ("inserted", inserted),
        ("out_of_range", out_of_range),
        ("erased", erased),
        ("duplicate", duplicate),
    ]
}

/// Loads every batch of the corpus through the ingest step, asserting each
/// batch's counts.
fn load_the_corpus(cluster: &Cluster, corpus: &JsonReader<'_>) {
    for statement in corpus.required_string_array("setup").expect("setup") {
        let _ = cluster.rows(&statement);
    }
    let statements = corpus.find_object("statements").expect("statements");
    for batch in corpus.required_object_array("batches").expect("batches") {
        let name = batch.required_string("name").expect("a batch's name");
        let kind = batch.required_string("kind").expect("a batch's kind");
        let of_kind = statements
            .find_object(&kind)
            .expect("the kind's statements");
        // COPY's data is one line per row, ended by a line holding `\.`.
        let mut copy_data = String::new();
        for line in batch.required_string_array("staged").expect("staged lines") {
            copy_data.push_str(&line);
            copy_data.push('\n');
        }
        // One session: the staging tables are the session's own.
        let script = format!(
            "DO $$ BEGIN PERFORM telemetry.open_staging(); END $$;\n{};\n{copy_data}\\.\n{};\n",
            of_kind.required_string("copy").expect("the copy statement"),
            of_kind
                .required_string("ingest")
                .expect("the ingest statement"),
        );
        let answer = cluster.session(&script);
        let [row] = &answer[..] else {
            panic!("batch `{name}`: one row of counts, got {answer:?}");
        };
        let expected = batch.find_object("counts").expect("a batch's counts");
        for (count, value) in counts_of(row) {
            assert_eq!(
                value,
                expected.required_integer(count).expect("a count"),
                "batch `{name}`: `{count}`"
            );
        }
    }
}

/// The corpus, through the generated SQL: every count and every row.
#[test]
fn the_ingest_step_and_the_views_hold_the_corpus() {
    let cluster = with_the_layout();
    let text = std::fs::read_to_string(generated("corpus.json")).expect("read the corpus");
    let corpus = JsonReader::new(bc_json::validate_json(&text).expect("the corpus is JSON"));
    load_the_corpus(&cluster, &corpus);

    let checks = corpus.required_object_array("checks").expect("checks");
    assert!(
        checks.len() > 3,
        "the corpus checks the three tables and no view"
    );
    for check in checks {
        let name = check.required_string("name").expect("a check's name");
        let query = check.required_string("query").expect("a check's query");
        let expected = check.required_string_array("rows").expect("a check's rows");
        assert_eq!(cluster.rows(&query), expected, "check `{name}`");
        // A view of a declared item holds that item's records: an empty one
        // would make every panel over it return nothing.
        assert!(!expected.is_empty(), "check `{name}` expects no rows");
    }
}

/// The layout is one transaction that brings an empty database to version 1:
/// applied where the schema already is, it changes nothing.
#[test]
fn the_layout_applies_once_and_says_which_version_is_live() {
    let cluster = start_a_cluster();
    cluster.apply(&generated("layout.sql"));
    assert_eq!(cluster.rows("SELECT telemetry.layout_version()"), ["1"]);
    // The functions are code: applied over themselves, they replace what is
    // there and touch no data.
    cluster.apply(&generated("functions.sql"));
    let _ = cluster.rows(
        "SELECT * FROM telemetry.maintain('2026-01-15T12:00:00Z', days_ahead => 1, days_kept => 2)",
    );
    cluster.apply(&generated("functions.sql"));

    assert!(
        !cluster.applies(&generated("layout.sql")),
        "the layout applied over itself"
    );
    assert_eq!(
        cluster.rows("SELECT count(*) FROM telemetry.live_days"),
        ["4"],
        "a second application disturbed what the first left"
    );
    assert_eq!(cluster.rows("SELECT telemetry.layout_version()"), ["1"]);

    // The views are the opposite: replaced whole, as often as they are applied,
    // and with them any view an earlier tree left behind.
    let _ = cluster.rows("CREATE VIEW telemetry.left_behind AS SELECT 1 AS one");
    cluster.apply(&generated("views.sql"));
    cluster.apply(&generated("views.sql"));
    assert_eq!(
        cluster.rows(
            "SELECT count(*) FROM pg_views WHERE schemaname = 'telemetry' \
             AND viewname = 'left_behind'"
        ),
        ["0"]
    );
    assert_eq!(
        cluster.rows(
            "SELECT count(*) FROM pg_views WHERE schemaname = 'telemetry' \
             AND viewname = 'metric__store_server__request_duration'"
        ),
        ["1"]
    );
}

/// A writer's session outlives a migration: its staging tables were made in
/// the old layout's shape, and an ingest reading a column the migration added
/// would fail every batch on it. `open_staging()` remakes them when the layout
/// version it made them at is no longer the live one.
#[test]
fn a_sessions_staging_tables_follow_a_migration_it_outlived() {
    let cluster = with_the_layout();
    let answer = cluster.session(
        "DO $$ BEGIN PERFORM telemetry.open_staging(); END $$;
         ALTER TABLE telemetry.spans ADD COLUMN migrated text;
         CREATE OR REPLACE FUNCTION telemetry.layout_version() RETURNS integer
             LANGUAGE sql IMMUTABLE AS $f$ SELECT 2 $f$;
         DO $$ BEGIN PERFORM telemetry.open_staging(); END $$;
         SELECT count(*) FROM pg_attribute
         WHERE attrelid = 'pg_temp.bc_telemetry_staged_spans'::regclass
           AND attname = 'migrated' AND NOT attisdropped;
",
    );
    assert_eq!(
        answer,
        ["1"],
        "the staging table kept the shape it had before the migration"
    );
}

fn joined(fields: &[&str]) -> String {
    fields.join(&FIELD_SEPARATOR.to_string())
}

/// Maintenance keeps a window of days around the instant it is handed: it
/// creates what is missing, drops what fell out, and forgets the erasures only
/// the dropped days could have matched.
#[test]
fn maintenance_keeps_a_window_of_days_around_the_instant_it_is_given() {
    let cluster = with_the_data_and_functions();
    let days = || cluster.rows("SELECT day FROM telemetry.live_days ORDER BY day");

    // Two days kept, today, one day ahead.
    assert_eq!(
        cluster.rows("SELECT * FROM telemetry.maintain('2026-01-15T12:00:00Z', days_ahead => 1, days_kept => 2)"),
        [joined(&["4", "0", "0"])]
    );
    assert_eq!(
        days(),
        ["2026-01-13", "2026-01-14", "2026-01-15", "2026-01-16"]
    );
    // Run again at the same instant, it has nothing to do.
    assert_eq!(
        cluster.rows("SELECT * FROM telemetry.maintain('2026-01-15T23:59:59.999Z', days_ahead => 1, days_kept => 2)"),
        [joined(&["0", "0", "0"])]
    );

    // The day is the instant's UTC day, whatever zone the instant is written in.
    let _ = cluster.rows(
        "SELECT * FROM telemetry.erase_customers('org', ARRAY['early', 'late'], \
         '2026-01-13T23:59:59.999Z')",
    );
    let _ = cluster.rows(
        "SELECT * FROM telemetry.erase_customers('org', ARRAY['late'], '2026-01-14T00:00:00Z')",
    );
    assert_eq!(
        cluster.rows("SELECT * FROM telemetry.maintain('2026-01-15T19:00:00-05:00', days_ahead => 1, days_kept => 2)"),
        [joined(&["1", "1", "1"])]
    );
    assert_eq!(
        days(),
        ["2026-01-14", "2026-01-15", "2026-01-16", "2026-01-17"]
    );
    // Each kept day has its three partitions, and the dropped day has none.
    assert_eq!(
        cluster.rows(
            "SELECT count(*) FROM pg_class WHERE relname ~ '^(spans|metrics|logs)_[0-9]{8}$'"
        ),
        ["12"]
    );
    assert_eq!(
        cluster.rows(
            "SELECT count(*) FROM pg_class WHERE relname IN \
             ('spans_20260113', 'metrics_20260113', 'logs_20260113')"
        ),
        ["0"]
    );
    // An erasure is forgotten once it is older than every kept day, and kept
    // while a kept day could still receive a record it must refuse.
    assert_eq!(
        cluster.rows("SELECT customer FROM telemetry.erasures ORDER BY customer"),
        ["late"]
    );

    // A day ahead, once created, is kept until it ages out: narrowing the
    // window drops what fell behind it — the 14th, and the erasure only the
    // 14th could still have matched — and nothing in front.
    assert_eq!(
        cluster.rows("SELECT * FROM telemetry.maintain('2026-01-15T12:00:00Z', days_ahead => 0, days_kept => 0)"),
        [joined(&["0", "1", "1"])]
    );
    assert_eq!(days(), ["2026-01-15", "2026-01-16", "2026-01-17"]);
    assert_eq!(
        cluster.rows("SELECT count(*) FROM telemetry.erasures"),
        ["0"]
    );
}

/// Neither horizon may be negative: a window that ends before it starts would
/// drop every partition and create none.
#[test]
fn maintenance_refuses_a_negative_horizon() {
    let cluster = with_the_data_and_functions();
    for arguments in [
        "days_ahead => -1, days_kept => 2",
        "days_ahead => 1, days_kept => -1",
    ] {
        assert!(
            cluster.refuses(&format!(
                "SELECT * FROM telemetry.maintain('2026-01-15T12:00:00Z', {arguments})"
            )),
            "maintain({arguments}) was accepted"
        );
    }
    assert_eq!(
        cluster.rows("SELECT count(*) FROM telemetry.live_days"),
        ["0"]
    );
}

/// A day's partition runs from one UTC midnight to the next whatever session
/// makes it. Its bounds are written as text and read back, and a session that
/// spells time its own way — a date style that names zones by abbreviation,
/// and a zone, India's, whose abbreviation the default set reads as Israel's —
/// would otherwise move both bounds by hours.
#[test]
fn a_partitions_bounds_are_utc_midnights_in_any_session() {
    let cluster = with_the_data_and_functions();
    let created = cluster.session(
        "SET TimeZone = 'Asia/Kolkata';\n\
         SET DateStyle = 'SQL, DMY';\n\
         SELECT days_created FROM telemetry.maintain('2026-01-15T12:00:00Z', \
         days_ahead => 0, days_kept => 0);\n",
    );
    assert_eq!(created, ["1"]);
    assert_eq!(
        cluster.session(
            "SET TimeZone = 'UTC';\n\
             SET DateStyle = 'ISO, YMD';\n\
             SELECT pg_get_expr(relpartbound, oid) FROM pg_class \
             WHERE relname = 'spans_20260115';\n"
        ),
        ["FOR VALUES FROM ('2026-01-15 00:00:00+00') TO ('2026-01-16 00:00:00+00')"]
    );
}

/// An erasure deletes what is stored about a customer, in one tenant, and
/// records itself so that what is still on its way is refused.
#[test]
fn an_erasure_deletes_a_customers_records_and_refuses_the_ones_in_flight() {
    let cluster = with_the_layout();
    let text = std::fs::read_to_string(generated("corpus.json")).expect("read the corpus");
    let corpus = JsonReader::new(bc_json::validate_json(&text).expect("the corpus is JSON"));
    load_the_corpus(&cluster, &corpus);

    let about = |customer: &str| {
        cluster.rows(&format!(
            "SELECT (SELECT count(*) FROM telemetry.spans WHERE customer = '{customer}'), \
                    (SELECT count(*) FROM telemetry.metrics WHERE customer = '{customer}'), \
                    (SELECT count(*) FROM telemetry.logs WHERE customer = '{customer}')"
        ))
    };
    let stored = about("customer-kept");
    assert_ne!(
        stored,
        [joined(&["0", "0", "0"])],
        "the corpus stores nothing about the customer this erases"
    );

    // Another tenant's customer of the same name is another customer.
    assert_eq!(
        cluster.rows(
            "SELECT * FROM telemetry.erase_customers('another-org', ARRAY['customer-kept'], \
             '2026-01-15T12:00:00Z')"
        ),
        [joined(&["0", "0", "0"])]
    );
    assert_eq!(about("customer-kept"), stored);

    assert_eq!(
        cluster.rows(
            "SELECT * FROM telemetry.erase_customers('corpus-org', ARRAY['customer-kept'], \
             '2026-01-15T12:00:00Z')"
        ),
        stored
    );
    assert_eq!(about("customer-kept"), [joined(&["0", "0", "0"])]);

    // Erased again later, the later instant is the one that holds.
    let _ = cluster.rows(
        "SELECT * FROM telemetry.erase_customers('corpus-org', ARRAY['customer-kept'], \
         '2026-01-15T11:00:00Z')",
    );
    assert_eq!(
        cluster.rows(
            "SELECT to_char(erased_at AT TIME ZONE 'UTC', 'HH24:MI') FROM telemetry.erasures \
             WHERE organization = 'corpus-org' AND customer = 'customer-kept'"
        ),
        ["12:00"]
    );
}

// --- The queries ----------------------------------------------------------------

/// The range a dashboard would ask for, standing in for `$__timeFilter`: the
/// corpus's day.
const TIME_FILTER: &str = "time BETWEEN '2026-01-15T00:00:00Z' AND '2026-01-16T00:00:00Z'";

/// A window as Grafana's interval syntax spells it, in seconds.
fn interval_seconds(interval: &str) -> u64 {
    let (number, unit) = interval.split_at(interval.len() - 1);
    let number: u64 = number.parse().expect("an interval's number");
    let unit_seconds = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        other => panic!("an interval unit Grafana does not have: {other}"),
    };
    number * unit_seconds
}

/// A statement with Grafana's two macros expanded the way Grafana expands them
/// for a fixed range: the filter to a `BETWEEN`, the time group to the start of
/// the bucket its row falls in.
///
/// This is not the dashboards' proof, which runs every panel's query through a
/// real Grafana (`grafana_in_the_loop.rs`) and so holds the generator to
/// Grafana rather than to this copy of it. What it holds is what Grafana does
/// not decide: that the SQL around the macros is valid over the views and reads
/// the rows the corpus put there.
fn with_the_macros_expanded(statement: &str) -> String {
    let mut expanded = statement.replace("$__timeFilter(time)", TIME_FILTER);
    let opening = "$__timeGroupAlias(time, '";
    if let Some(start) = expanded.find(opening) {
        let after = start + opening.len();
        let end = after + expanded[after..].find("')").expect("the macro's closing");
        let seconds = interval_seconds(&expanded[after..end]);
        expanded.replace_range(
            start..end + 2,
            &format!(
                "to_timestamp(floor(extract(epoch FROM time) / {seconds}) * {seconds}) AS \"time\""
            ),
        );
    }
    expanded
}

/// The statements of a generated query file: every block after the file's
/// opening comment, each a comment and one statement. The generator writes no
/// blank line inside a block, so a blank line is where one ends.
fn statements_of(file: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(file).expect("read a query file");
    text.split("\n\n")
        .skip(1)
        .map(|block| block.trim_end().trim_end_matches(';').to_string())
        .collect()
}

/// Every statement the generator wrote runs over the corpus and returns rows:
/// each query's buckets with a number for a value, each log's and error's
/// records. The corpus holds a record of every declared item, so an empty
/// answer is a statement that reads the wrong thing.
#[test]
fn every_generated_query_runs_over_the_corpus_and_returns_rows() {
    let cluster = with_the_corpus();

    let mut files: Vec<PathBuf> = std::fs::read_dir(generated("queries"))
        .expect("the query files")
        .map(|entry| entry.expect("a query file").path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 3, "a query file per module: {files:?}");

    let mut statements_run = 0;
    for file in &files {
        for statement in statements_of(file) {
            let rows = cluster.rows(&with_the_macros_expanded(&statement));
            assert!(!rows.is_empty(), "no rows from:\n{statement}");
            if statement.contains("$__timeGroupAlias") {
                for row in &rows {
                    let value = row
                        .rsplit(FIELD_SEPARATOR)
                        .next()
                        .expect("a row has a value");
                    assert!(
                        value.parse::<f64>().is_ok(),
                        "`{value}` is not a number, from:\n{statement}"
                    );
                }
            }
            statements_run += 1;
        }
    }
    // Eight queries, three logs and one failure error across the three modules.
    assert_eq!(statements_run, 12);
}
