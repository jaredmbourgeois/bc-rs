//! The PostgreSQL exporter.
//!
//! New surface, so none of these is a ported case. Two halves:
//!
//! - **With no database**, the contract a running process depends on when its
//!   telemetry database is down: a counted drop, never a failure.
//! - **Against a real one** — a throwaway cluster per test — the conformance
//!   corpus `bc-telemetry-gen` publishes beside the layout: every batch through
//!   this writer, every count it answers with, and every row the tables and
//!   views then hold. A writer with no corpus run is not a writer of the
//!   layout.
#![cfg(feature = "postgres")]

use bc_date_time::Epoch;
use bc_telemetry::{
    AttributeValue, LogRecord, MetricKind, MetricPoint, ObservationId, PostgresExporter, Severity,
    Span, TelemetryExporter, TraceId,
};

/// A connection string that cannot succeed: port 1 on loopback refuses.
const UNREACHABLE: &str = "postgresql://nobody@127.0.0.1:1/telemetry?connect_timeout=1";

fn an_observation(sequence: u64) -> ObservationId {
    ObservationId {
        producer: TraceId { bytes: [0x0b; 16] },
        sequence,
    }
}

fn a_log(observation_id: ObservationId) -> LogRecord {
    LogRecord {
        timestamp: Epoch::ZERO,
        severity: Severity::Info,
        body: "hello".to_owned(),
        attributes: Vec::new(),
        span_context: None,
        observation_id,
    }
}

fn a_metric(observation_id: ObservationId) -> MetricPoint {
    MetricPoint {
        name: "hits".to_owned(),
        kind: MetricKind::Counter,
        value: AttributeValue::Int64(1),
        attributes: Vec::new(),
        timestamp: Epoch::ZERO,
        span_context: None,
        observation_id,
    }
}

#[test]
fn an_unreachable_database_is_a_counted_drop_with_its_reason() {
    // The contract that matters most: a process does not stop working because
    // its telemetry database is down, and it does not silently pretend either.
    let exporter = PostgresExporter::new(UNREACHABLE, "tests");
    assert_eq!(exporter.last_failure(), None);

    exporter.export_spans(&[Span::default(), Span::default()]);
    exporter.export_metrics(&[a_metric(an_observation(1))]);
    exporter.export_logs(&[a_log(an_observation(2))]);
    exporter.flush();

    assert_eq!(exporter.dropped(), 4);
    let reason = exporter.last_failure().expect("a reason for the drops");
    assert!(
        reason.starts_with("could not connect to the database"),
        "{reason}"
    );
}

#[test]
fn an_empty_batch_contacts_nothing() {
    let exporter = PostgresExporter::new(UNREACHABLE, "tests");
    exporter.export_spans(&[]);
    exporter.export_metrics(&[]);
    exporter.export_logs(&[]);
    assert_eq!(exporter.dropped(), 0);
    assert_eq!(exporter.last_failure(), None);
}

#[test]
fn a_record_no_writer_could_store_once_is_refused_before_the_database() {
    // An observation id of zero is the one a record gets when its pipeline
    // could issue none. Refused, it never reaches the connection, so the
    // unreachable database below is never asked.
    let exporter = PostgresExporter::new(UNREACHABLE, "tests");
    let refused = exporter
        .write_metrics(&[a_metric(ObservationId::default())])
        .expect("nothing to write, so nothing to fail");
    assert_eq!(refused.refused, 1);
    assert_eq!(refused.dropped(), 1);

    let mut text_valued = a_metric(an_observation(1));
    text_valued.value = AttributeValue::from("seven");
    let mut not_finite = a_metric(an_observation(2));
    not_finite.value = AttributeValue::Float64(f64::NAN);
    exporter.export_metrics(&[text_valued, not_finite]);
    exporter.export_logs(&[a_log(ObservationId::default())]);
    assert_eq!(exporter.dropped(), 3);
    assert_eq!(exporter.last_failure(), None);
}

/// A record whose text holds a NUL is refused whole wherever the NUL sits — no
/// PostgreSQL value can store one, and altering the text would make it another
/// record — and refused before the database, so it costs that record alone.
#[test]
fn a_record_whose_text_holds_a_nul_is_refused_before_the_database() {
    use bc_telemetry::SpanEvent;

    let exporter = PostgresExporter::new(UNREACHABLE, "tests");
    let nul = || "before\0after".to_owned();

    // One span per place a NUL can sit in one.
    let spans = [
        Span {
            name: nul(),
            ..Span::default()
        },
        Span {
            status_message: nul(),
            ..Span::default()
        },
        Span {
            attributes: vec![(nul(), AttributeValue::Int64(1))],
            ..Span::default()
        },
        Span {
            attributes: vec![("note".to_owned(), AttributeValue::String(nul()))],
            ..Span::default()
        },
        Span {
            events: vec![SpanEvent {
                name: nul(),
                timestamp: Epoch::ZERO,
                attributes: Vec::new(),
            }],
            ..Span::default()
        },
        Span {
            events: vec![SpanEvent {
                name: "noted".to_owned(),
                timestamp: Epoch::ZERO,
                attributes: vec![("note".to_owned(), AttributeValue::String(nul()))],
            }],
            ..Span::default()
        },
    ];
    let spans_refused = exporter
        .write_spans(&spans)
        .expect("nothing to write, so nothing to fail");
    assert_eq!(spans_refused.refused, 6);

    let mut metric = a_metric(an_observation(1));
    metric.name = nul();
    let mut log = a_log(an_observation(2));
    log.body = nul();
    exporter.export_metrics(&[metric]);
    exporter.export_logs(&[log]);
    assert_eq!(exporter.dropped(), 2);
    assert_eq!(
        exporter.last_failure(),
        None,
        "the database was never asked"
    );

    let misnamed = PostgresExporter::new(UNREACHABLE, "service\0name");
    let refused = misnamed
        .write_logs(&[a_log(an_observation(3))])
        .expect("nothing to write, so nothing to fail");
    assert_eq!(
        refused.refused, 1,
        "a configured service name is stored too"
    );
}

#[test]
fn the_debug_rendering_never_prints_the_connection_string() {
    // It carries a password.
    let exporter = PostgresExporter::new("postgresql://user:hunter2@db.example/telemetry", "tests");
    let rendered = format!("{exporter:?}");
    assert!(!rendered.contains("hunter2"), "{rendered}");
    assert!(!rendered.contains("db.example"), "{rendered}");
    assert_eq!(
        rendered,
        r#"PostgresExporter { service_name: "tests", dropped: 0, .. }"#
    );
}

/// With no TLS built, a database on another host is never written to: the
/// batch is a counted drop whose reason names why, and no connection is tried.
#[test]
fn a_database_off_this_host_is_refused_and_counted() {
    let exporter = PostgresExporter::new("postgresql://writer@db.example/telemetry", "tests");
    exporter.export_logs(&[a_log(an_observation(1))]);
    assert_eq!(exporter.dropped(), 1);
    let failure = exporter.last_failure().expect("the refusal is recorded");
    assert!(
        failure.contains("`db.example` is not this host"),
        "{failure}"
    );
}

#[test]
fn a_connection_string_without_a_timeout_gains_one() {
    // Not a style preference: without `connect_timeout`, a host that drops
    // packets rather than refusing them makes `Client::connect` block for the
    // OS TCP timeout (~75 s) while holding the mutex every recording thread
    // needs. The exporter must cap that itself, because the caller who writes
    // `postgresql://db.example/telemetry` has no idea it matters.
    let exporter = PostgresExporter::new("postgresql://nobody@127.0.0.1:1/telemetry", "tests");
    let started = std::time::Instant::now();
    exporter.export_logs(&[a_log(an_observation(1))]);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "export blocked for {:?}, so no connect timeout was applied",
        started.elapsed()
    );
    assert_eq!(exporter.dropped(), 1);
}

#[cfg(unix)]
#[path = "support/postgres_cluster.rs"]
mod postgres_cluster;

#[cfg(unix)]
mod against_a_real_database {
    use std::path::{Path, PathBuf};

    use bc_date_time::Epoch;
    use bc_json::{JsonReader, NullReads};
    use bc_telemetry::{
        AttributeValue, Attributes, LogRecord, MetricKind, MetricPoint, ObservationId,
        PostgresBatchOutcome, PostgresExporter, Severity, Span, SpanContext, SpanEvent, SpanId,
        SpanKind, StatusCode, TelemetryExporter, TraceId,
    };

    use crate::postgres_cluster::Cluster;

    /// A short directory for a cluster's socket: its path has a hard limit
    /// that a cargo target directory can pass on its own.
    const SOCKET_ROOT: &str = "/tmp";

    fn start_a_cluster() -> Cluster {
        let cluster = Cluster::start(
            Path::new(env!("CARGO_TARGET_TMPDIR")),
            Path::new(SOCKET_ROOT),
        );
        println!("PostgreSQL {}", cluster.version());
        cluster
    }

    /// One of the files the generator renders for its canonical fixture: the
    /// layout every writer is held to, and the corpus that holds it.
    fn generated(file: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("crates/gen/tests/fixtures/canonical/generated/telemetry/postgresql")
            .join(file)
    }

    fn with_the_layout() -> Cluster {
        let cluster = start_a_cluster();
        cluster.apply(&generated("layout.sql"));
        cluster.apply(&generated("functions.sql"));
        cluster.apply(&generated("views.sql"));
        cluster
    }

    // --- The corpus's records, as this runtime carries them ------------------

    fn trace_id(hex: &str) -> TraceId {
        let mut id = TraceId::default();
        assert!(
            bc_core::decode_hex_into(hex, &mut id.bytes),
            "a trace id: {hex}"
        );
        id
    }

    fn span_id(hex: &str) -> SpanId {
        let mut id = SpanId::default();
        assert!(
            bc_core::decode_hex_into(hex, &mut id.bytes),
            "a span id: {hex}"
        );
        id
    }

    /// A float JSON cannot hold, as the corpus names it.
    fn not_finite(named: &str) -> f64 {
        match named {
            "nan" => f64::NAN,
            "infinity" => f64::INFINITY,
            "-infinity" => f64::NEG_INFINITY,
            other => panic!("a float the corpus should not name: {other}"),
        }
    }

    fn attributes_of(holder: &JsonReader<'_>) -> Attributes {
        let mut attributes = Attributes::new();
        for attribute in holder
            .required_object_array("attributes")
            .expect("attributes")
        {
            let key = attribute
                .required_string("key")
                .expect("an attribute's key");
            let value = if attribute.has_field("bool") {
                AttributeValue::Boolean(attribute.required_boolean("bool").expect("a bool"))
            } else if attribute.has_field("int") {
                AttributeValue::Int64(attribute.required_integer("int").expect("an int"))
            } else if attribute.has_field("float") {
                AttributeValue::Float64(attribute.required_floating("float").expect("a float"))
            } else if attribute.has_field("not_finite") {
                AttributeValue::Float64(not_finite(
                    &attribute.required_string("not_finite").expect("a name"),
                ))
            } else if attribute.has_field("nul_separated") {
                // Text JSON cannot carry, as its pieces between NULs.
                AttributeValue::String(
                    attribute
                        .required_string_array("nul_separated")
                        .expect("the pieces")
                        .join("\0"),
                )
            } else {
                AttributeValue::String(attribute.required_string("string").expect("a string"))
            };
            attributes.push((key, value));
        }
        attributes
    }

    fn time_of(holder: &JsonReader<'_>, key: &str) -> Epoch {
        Epoch::from_millis(
            holder
                .required_integer(key)
                .expect("a time in milliseconds"),
        )
    }

    fn observation_of(record: &JsonReader<'_>) -> ObservationId {
        let sequence = record.required_integer("sequence").expect("a sequence");
        ObservationId {
            producer: trace_id(&record.required_string("producer").expect("a producer")),
            sequence: u64::try_from(sequence).expect("a sequence is not negative"),
        }
    }

    fn within_of(record: &JsonReader<'_>) -> Option<SpanContext> {
        let within = record.find_object("within")?;
        Some(SpanContext {
            trace_id: trace_id(&within.required_string("trace_id").expect("a trace id")),
            span_id: span_id(&within.required_string("span_id").expect("a span id")),
            trace_flags: 0,
        })
    }

    fn span_of(record: &JsonReader<'_>) -> Span {
        let kind = match record.required_string("kind").expect("a kind").as_str() {
            "internal" => SpanKind::Internal,
            "server" => SpanKind::Server,
            "client" => SpanKind::Client,
            "producer" => SpanKind::Producer,
            "consumer" => SpanKind::Consumer,
            other => panic!("a span kind the corpus should not hold: {other}"),
        };
        let status = match record.required_string("status").expect("a status").as_str() {
            "unset" => StatusCode::Unset,
            "ok" => StatusCode::Ok,
            "error" => StatusCode::Error,
            other => panic!("a status the corpus should not hold: {other}"),
        };
        let parent = record
            .optional_string("parent_span_id", NullReads::Refused)
            .expect("a parent span id, or none");
        Span {
            name: record.required_string("name").expect("a name"),
            context: SpanContext {
                trace_id: trace_id(&record.required_string("trace_id").expect("a trace id")),
                span_id: span_id(&record.required_string("span_id").expect("a span id")),
                trace_flags: 0,
            },
            parent_span_id: parent.as_deref().map(span_id),
            kind,
            status,
            status_message: record
                .required_string("status_message")
                .expect("a status message"),
            start_time: time_of(record, "start_ms"),
            end_time: time_of(record, "end_ms"),
            attributes: attributes_of(record),
            events: record
                .required_object_array("events")
                .expect("events")
                .iter()
                .map(|event| SpanEvent {
                    name: event.required_string("name").expect("an event's name"),
                    timestamp: time_of(event, "time_ms"),
                    attributes: attributes_of(event),
                })
                .collect(),
        }
    }

    fn metric_of(record: &JsonReader<'_>) -> MetricPoint {
        let kind = match record.required_string("kind").expect("a kind").as_str() {
            "counter" => MetricKind::Counter,
            "gauge" => MetricKind::Gauge,
            "histogram" => MetricKind::Histogram,
            other => panic!("a metric kind the corpus should not hold: {other}"),
        };
        let value = if record.has_field("value_bool") {
            AttributeValue::Boolean(record.required_boolean("value_bool").expect("a bool"))
        } else if record.has_field("value_int") {
            AttributeValue::Int64(record.required_integer("value_int").expect("an int"))
        } else if record.has_field("value_float") {
            AttributeValue::Float64(record.required_floating("value_float").expect("a float"))
        } else if record.has_field("value_not_finite") {
            AttributeValue::Float64(not_finite(
                &record.required_string("value_not_finite").expect("a name"),
            ))
        } else {
            AttributeValue::String(record.required_string("value_string").expect("a string"))
        };
        MetricPoint {
            name: record.required_string("name").expect("a name"),
            kind,
            value,
            attributes: attributes_of(record),
            timestamp: time_of(record, "time_ms"),
            span_context: within_of(record),
            observation_id: observation_of(record),
        }
    }

    fn log_of(record: &JsonReader<'_>) -> LogRecord {
        let severity = match record
            .required_string("severity")
            .expect("a severity")
            .as_str()
        {
            "debug" => Severity::Debug,
            "info" => Severity::Info,
            "warn" => Severity::Warn,
            "error" => Severity::Error,
            "fatal" => Severity::Fatal,
            other => panic!("a severity the corpus should not hold: {other}"),
        };
        LogRecord {
            timestamp: time_of(record, "time_ms"),
            severity,
            body: record.required_string("body").expect("a body"),
            attributes: attributes_of(record),
            span_context: within_of(record),
            observation_id: observation_of(record),
        }
    }

    // --- The corpus run --------------------------------------------------------

    /// What a batch must come to, as the corpus words it.
    fn expected_outcome(batch: &JsonReader<'_>) -> PostgresBatchOutcome {
        let writer = batch.find_object("writer").expect("the writer's counts");
        let counts = batch.find_object("counts").expect("the ingest counts");
        let count = |holder: &JsonReader<'_>, key: &str| {
            u64::try_from(holder.required_integer(key).expect("a count"))
                .expect("a count is not negative")
        };
        PostgresBatchOutcome {
            refused: count(&writer, "refused"),
            collisions: count(&writer, "collisions"),
            inserted: count(&counts, "inserted"),
            out_of_range: count(&counts, "out_of_range"),
            erased: count(&counts, "erased"),
            duplicate: count(&counts, "duplicate"),
        }
    }

    /// Writes one corpus batch through `exporter`.
    fn write_batch(exporter: &PostgresExporter, batch: &JsonReader<'_>) -> PostgresBatchOutcome {
        let name = batch.required_string("name").expect("a batch's name");
        let records = batch.required_object_array("records").expect("records");
        let written = match batch.required_string("kind").expect("a kind").as_str() {
            "spans" => exporter.write_spans(&records.iter().map(span_of).collect::<Vec<_>>()),
            "metrics" => exporter.write_metrics(&records.iter().map(metric_of).collect::<Vec<_>>()),
            "logs" => exporter.write_logs(&records.iter().map(log_of).collect::<Vec<_>>()),
            other => panic!("a batch of a kind the corpus should not hold: {other}"),
        };
        written.unwrap_or_else(|error| panic!("batch `{name}` was not written: {error}"))
    }

    /// The corpus through this writer: every batch's counts, then every row of
    /// the tables and of every view.
    #[test]
    fn this_writer_holds_the_corpus() {
        let cluster = with_the_layout();
        let text = std::fs::read_to_string(generated("corpus.json")).expect("read the corpus");
        let corpus = JsonReader::new(bc_json::validate_json(&text).expect("the corpus is JSON"));
        for statement in corpus.required_string_array("setup").expect("setup") {
            let _setup_rows = cluster.rows(&statement);
        }
        let exporter = PostgresExporter::new(
            cluster.connection_string(),
            corpus
                .required_string("service_name")
                .expect("the service name"),
        );

        let mut dropped = 0;
        for batch in corpus.required_object_array("batches").expect("batches") {
            let name = batch.required_string("name").expect("a batch's name");
            let outcome = write_batch(&exporter, &batch);
            assert_eq!(outcome, expected_outcome(&batch), "batch `{name}`");
            dropped += outcome.dropped();
        }
        assert!(dropped > 0, "the corpus exercised no drop");

        let checks = corpus.required_object_array("checks").expect("checks");
        assert!(checks.len() > 3, "the corpus checks the tables and no view");
        for check in checks {
            let name = check.required_string("name").expect("a check's name");
            let query = check.required_string("query").expect("a check's query");
            let expected = check.required_string_array("rows").expect("a check's rows");
            assert_eq!(cluster.rows(&query), expected, "check `{name}`");
        }
    }

    /// A database the layout was never applied to refuses the first statement
    /// of every batch: each one a counted drop, with the reason kept.
    #[test]
    fn a_database_without_the_layout_is_a_counted_drop_with_its_reason() {
        let cluster = start_a_cluster();
        let exporter = PostgresExporter::new(cluster.connection_string(), "tests");
        let mut span = Span::default();
        span.context.trace_id.bytes[0] = 1;
        span.context.span_id.bytes[0] = 1;
        exporter.export_spans(&[span.clone(), span]);
        assert_eq!(exporter.dropped(), 2);
        let reason = exporter.last_failure().expect("a reason for the drops");
        assert!(
            reason.starts_with("could not open the staging tables"),
            "{reason}"
        );
    }

    /// A connection the server closes is retired after the batch it failed, and
    /// the next batch connects again.
    #[test]
    fn a_closed_connection_is_retired_and_the_next_batch_reconnects() {
        let cluster = with_the_layout();
        // 2026-01-15T12:00:00Z: a fixed instant, and partitions around it.
        let instant = Epoch::from_millis(1_768_478_400_000);
        let _maintained =
            cluster.rows("SELECT * FROM telemetry.maintain('2026-01-15T12:00:00Z', 1, 1)");
        let exporter = PostgresExporter::new(cluster.connection_string(), "tests");
        let span_at = |byte: u8| {
            let mut span = Span::default();
            span.context.trace_id.bytes[0] = byte;
            span.context.span_id.bytes[0] = byte;
            span.start_time = instant;
            span.end_time = instant;
            span
        };
        let first = exporter
            .write_spans(&[span_at(1)])
            .expect("the first batch");
        assert_eq!(first.inserted, 1);

        let _terminated = cluster.rows(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE backend_type = 'client backend' AND pid <> pg_backend_pid()",
        );
        assert!(
            exporter.write_spans(&[span_at(2)]).is_err(),
            "a batch on a closed connection was written"
        );
        let third = exporter
            .write_spans(&[span_at(3)])
            .expect("the batch after a closed connection");
        assert_eq!(third.inserted, 1);
        assert_eq!(cluster.rows("SELECT count(*) FROM telemetry.spans"), ["2"]);
    }

    // --- An erasure and an ingest at the same time ---

    /// The customer both sessions are about, and the instant its erasure
    /// holds from: after the staged span's time, so the span is erased.
    const ERASE: &str = "SELECT spans_deleted FROM telemetry.erase_customers('race-org', \
                         ARRAY['race-customer'], '2026-01-15T13:00:00Z')";

    fn session(cluster: &Cluster) -> postgres::Client {
        postgres::Client::connect(&cluster.connection_string(), postgres::NoTls)
            .expect("connect a session")
    }

    fn backend_of(client: &mut postgres::Client) -> i32 {
        client
            .query_one("SELECT pg_backend_pid()", &[])
            .expect("ask a session's backend")
            .get(0)
    }

    /// Stages one span about the customer and ingests it, in whatever
    /// transaction `client` is in; the ingest step's `[inserted, erased]`.
    fn ingest_one_span(client: &mut impl postgres::GenericClient) -> [i64; 2] {
        client
            .batch_execute(
                "SELECT telemetry.open_staging(); \
                 INSERT INTO pg_temp.bc_telemetry_staged_spans (time, record_id, name, \
                     service_name, organization, customer, trace_id, span_id, attributes, \
                     end_time, kind, status, status_message, events) \
                 VALUES ('2026-01-15T12:00:00Z', 'race-record', 'race', 'tests', 'race-org', \
                     'race-customer', 'race-trace', 'race-span', '{}', '2026-01-15T12:00:00Z', \
                     'internal', 'unset', '', '[]')",
            )
            .expect("stage a span");
        let counts = client
            .query_one("SELECT inserted, erased FROM telemetry.ingest_spans()", &[])
            .expect("ingest the staged span");
        [counts.get(0), counts.get(1)]
    }

    /// Waits until the session on `backend` is waiting on a lock, or `work`
    /// has finished without waiting — the two outcomes a race can have, read
    /// from the server and the thread, never guessed from a duration.
    fn until_waiting_or_done<T>(
        observer: &mut postgres::Client,
        backend: i32,
        work: &std::thread::JoinHandle<T>,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let waiting: i64 = observer
                .query_one(
                    "SELECT count(*) FROM pg_locks WHERE pid = $1 AND NOT granted",
                    &[&backend],
                )
                .expect("read pg_locks")
                .get(0);
            if waiting > 0 || work.is_finished() {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the session neither waited nor finished"
            );
            // The poll's interval, not a synchronization: the loop's exit is
            // the condition above.
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn spans_about_the_customer(cluster: &Cluster) -> Vec<String> {
        cluster.rows("SELECT count(*) FROM telemetry.spans WHERE customer = 'race-customer'")
    }

    /// An ingest whose statement starts while an erasure of its customer is
    /// still uncommitted does not slip past it: it waits for the erasure and
    /// then refuses the record as erased.
    #[test]
    fn an_ingest_during_an_erasure_waits_for_it_and_refuses_the_erased_record() {
        let cluster = with_the_layout();
        let _maintained =
            cluster.rows("SELECT * FROM telemetry.maintain('2026-01-15T12:00:00Z', 1, 1)");
        let mut observer = session(&cluster);

        let mut erasing = session(&cluster);
        let mut erasure = erasing.transaction().expect("begin the erasure");
        let deleted: i64 = erasure
            .query_one(ERASE, &[])
            .expect("erase the customer")
            .get(0);
        assert_eq!(deleted, 0, "nothing was stored about the customer yet");

        let mut ingesting = session(&cluster);
        let backend = backend_of(&mut ingesting);
        let ingest = std::thread::spawn(move || ingest_one_span(&mut ingesting));
        until_waiting_or_done(&mut observer, backend, &ingest);
        erasure.commit().expect("commit the erasure");

        let [inserted, erased] = ingest.join().expect("the ingest thread");
        assert_eq!(
            [inserted, erased],
            [0, 1],
            "the ingest refuses the erased customer's record"
        );
        assert_eq!(spans_about_the_customer(&cluster), ["0"]);
    }

    /// An erasure that starts while an ingest of its customer's record is
    /// still uncommitted does not miss it: it waits for the ingest and then
    /// deletes what it stored.
    #[test]
    fn an_erasure_during_an_ingest_waits_for_it_and_deletes_what_it_stored() {
        let cluster = with_the_layout();
        let _maintained =
            cluster.rows("SELECT * FROM telemetry.maintain('2026-01-15T12:00:00Z', 1, 1)");
        let mut observer = session(&cluster);

        let mut ingesting = session(&cluster);
        let mut batch = ingesting.transaction().expect("begin the ingest");
        assert_eq!(ingest_one_span(&mut batch), [1, 0], "the span is stored");

        let mut erasing = session(&cluster);
        let backend = backend_of(&mut erasing);
        let erase = std::thread::spawn(move || -> i64 {
            erasing
                .query_one(ERASE, &[])
                .expect("erase the customer")
                .get(0)
        });
        until_waiting_or_done(&mut observer, backend, &erase);
        batch.commit().expect("commit the ingest");

        assert_eq!(
            erase.join().expect("the erasure thread"),
            1,
            "the erasure deletes the span the ingest stored"
        );
        assert_eq!(spans_about_the_customer(&cluster), ["0"]);
    }

    /// An erasure in a transaction that reads from one snapshot throughout is
    /// refused by name, and records nothing: that snapshot may predate the
    /// lock, and would miss what an ingest committed in between.
    #[test]
    fn an_erasure_outside_read_committed_is_refused_and_records_nothing() {
        let cluster = with_the_layout();
        let mut erasing = session(&cluster);
        for (isolation, level) in [
            ("REPEATABLE READ", postgres::IsolationLevel::RepeatableRead),
            ("SERIALIZABLE", postgres::IsolationLevel::Serializable),
        ] {
            let mut transaction = erasing
                .build_transaction()
                .isolation_level(level)
                .start()
                .expect("begin the transaction");
            let refused = match transaction.query_one(ERASE, &[]) {
                Ok(_) => panic!("an erasure under {isolation} was run"),
                Err(error) => error.as_db_error().map(|db| db.message().to_owned()),
            };
            assert_eq!(
                refused.as_deref(),
                Some(
                    format!(
                        "telemetry.erase_customers: run it in a READ COMMITTED transaction, \
                         not {isolation}"
                    )
                    .as_str()
                )
            );
            transaction.rollback().expect("end the refused transaction");
        }
        assert_eq!(
            cluster.rows("SELECT count(*) FROM telemetry.erasures"),
            ["0"]
        );
    }

    /// A maintenance run never waits long behind another session's lock: an
    /// exclusive request that waits is waited behind by every ingest after it,
    /// so `maintain()` gives up at its own five seconds — ended by
    /// `lock_timeout`'s SQLSTATE, not by its caller's longer bound.
    #[test]
    fn maintenance_gives_up_rather_than_queue_every_ingest_behind_it() {
        let cluster = with_the_layout();
        let mut holding = session(&cluster);
        let mut held = holding.transaction().expect("begin the holder");
        held.batch_execute("LOCK TABLE telemetry.spans IN ACCESS SHARE MODE")
            .expect("hold the table");

        let mut maintaining = session(&cluster);
        maintaining
            .batch_execute("SET statement_timeout = '20s'")
            .expect("bound the caller");
        let refused = maintaining
            .query(
                "SELECT * FROM telemetry.maintain('2030-06-01T00:00:00Z', 1, 1)",
                &[],
            )
            .expect_err("maintenance waited out the holder");
        assert_eq!(
            refused.code().map(postgres::error::SqlState::code),
            Some("55P03"),
            "{refused}"
        );
        held.rollback().expect("release the table");
    }

    /// A writer never waits long on the database: a batch behind another
    /// session's exclusive lock gives up at the session's lock bound, a
    /// counted drop naming why, instead of holding the writer. The holder's
    /// own idle bound, past the writer's, is what would end an unbounded wait.
    #[test]
    fn a_batch_behind_a_held_lock_gives_up_and_is_counted() {
        let cluster = with_the_layout();
        let mut holding = session(&cluster);
        holding
            .batch_execute("SET idle_in_transaction_session_timeout = '20s'")
            .expect("bound the holder");
        let mut held = holding.transaction().expect("begin the holder");
        held.batch_execute("LOCK TABLE telemetry.logs IN ACCESS EXCLUSIVE MODE")
            .expect("hold the table");

        let exporter = PostgresExporter::new(cluster.connection_string(), "tests");
        exporter.export_logs(&[super::a_log(super::an_observation(1))]);
        let failure = exporter
            .last_failure()
            .expect("the batch failed, and says why");
        assert!(failure.contains("lock timeout"), "{failure}");
        assert_eq!(exporter.dropped(), 1, "the batch's record is counted");
        held.rollback().expect("release the table");
    }
}
