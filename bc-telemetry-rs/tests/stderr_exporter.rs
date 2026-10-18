//! Ported from `libs/platform/telemetry/test/stderr_exporter_test.cpp`.
//!
//! The C++ injects a `std::ostream&`; here the seam is `to_writer`, and the
//! captured buffer below is what stands in for the test's `std::ostringstream`.

use std::io::Write;
use std::sync::{Arc, Mutex};

use bc_date_time::Epoch;
use bc_telemetry::{
    AttributeValue, LogRecord, MetricKind, MetricPoint, ObservationId, Severity, Span, SpanEvent,
    SpanKind, StatusCode, StderrExporter, StderrExporterScope, TelemetryExporter,
};

/// A writer whose bytes a case can read back.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8(bytes.clone()).expect("the exporter writes utf-8")
    }
}

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn exporter_writing_to(scope: StderrExporterScope) -> (StderrExporter, Captured) {
    let captured = Captured::default();
    let exporter = StderrExporter::to_writer(Box::new(captured.clone()), scope);
    (exporter, captured)
}

fn a_span() -> Span {
    Span {
        name: "fetch".to_owned(),
        start_time: Epoch::from_millis(1_000),
        end_time: Epoch::from_millis(1_250),
        kind: SpanKind::Client,
        ..Span::default()
    }
}

fn a_log(severity: Severity) -> LogRecord {
    LogRecord {
        timestamp: Epoch::from_millis(1_000),
        severity,
        body: "something happened".to_owned(),
        attributes: Vec::new(),
        span_context: None,
        observation_id: ObservationId::default(),
    }
}

#[test]
fn export_spans_prints_name_duration_status_and_attributes() {
    let (exporter, captured) = exporter_writing_to(StderrExporterScope::SpansAndLogs);
    let span = Span {
        status: StatusCode::Error,
        status_message: "upstream refused".to_owned(),
        attributes: vec![
            ("host".to_owned(), AttributeValue::from("example-host")),
            ("retries".to_owned(), AttributeValue::Int64(2)),
        ],
        ..a_span()
    };
    exporter.export_spans(&[span]);

    let text = captured.text();
    assert!(text.starts_with("[trace] fetch (250ms)"), "{text}");
    assert!(text.contains("ERROR: upstream refused"), "{text}");
    assert!(text.contains("host=example-host"), "{text}");
    assert!(text.contains("retries=2"), "{text}");
    assert!(text.ends_with('\n'));
}

// A span's events are where a failure's detail and a job's findings live; on a
// process whose only sink is stderr they must print, one indented line each,
// after the span's own line.
#[test]
fn export_spans_prints_each_event_under_its_span() {
    let (exporter, captured) = exporter_writing_to(StderrExporterScope::SpansAndLogs);
    let span = Span {
        name: "worker.gcode_cache_eviction".to_owned(),
        kind: SpanKind::Internal,
        status: StatusCode::Error,
        status_message: "gcode_cache:2".to_owned(),
        start_time: Epoch::from_millis(100),
        end_time: Epoch::from_millis(103),
        attributes: vec![("job.lane".to_owned(), AttributeValue::from("poll_loop"))],
        events: vec![
            SpanEvent {
                name: "error".to_owned(),
                timestamp: Epoch::from_millis(102),
                attributes: vec![
                    (
                        "error.category".to_owned(),
                        AttributeValue::from("gcode_cache"),
                    ),
                    (
                        "error.meta.detail".to_owned(),
                        AttributeValue::from(
                            "/var/cache/gcode could not be listed: Permission denied",
                        ),
                    ),
                ],
            },
            SpanEvent {
                name: "retry".to_owned(),
                timestamp: Epoch::from_millis(103),
                attributes: Vec::new(),
            },
        ],
        ..Span::default()
    };
    exporter.export_spans(&[span]);

    assert_eq!(
        captured.text(),
        "[trace] worker.gcode_cache_eviction (3ms) ERROR: gcode_cache:2 job.lane=poll_loop\n\
         \x20 event error error.category=gcode_cache \
         error.meta.detail=/var/cache/gcode could not be listed: Permission denied\n\
         \x20 event retry\n"
    );
}

#[test]
fn export_logs_prints_severity_body_and_attributes() {
    let (exporter, captured) = exporter_writing_to(StderrExporterScope::SpansAndLogs);
    exporter.export_logs(&[LogRecord {
        attributes: vec![("unit".to_owned(), AttributeValue::from("store_server"))],
        ..a_log(Severity::Warn)
    }]);

    let text = captured.text();
    assert!(text.starts_with("[log warn] something happened"), "{text}");
    assert!(text.contains("unit=store_server"), "{text}");
}

#[test]
fn export_logs_handles_all_severity_levels() {
    let (exporter, captured) = exporter_writing_to(StderrExporterScope::SpansAndLogs);
    for severity in [
        Severity::Debug,
        Severity::Info,
        Severity::Warn,
        Severity::Error,
        Severity::Fatal,
    ] {
        exporter.export_logs(&[a_log(severity)]);
    }

    let text = captured.text();
    for label in ["debug", "info", "warn", "error", "fatal"] {
        assert!(
            text.contains(&format!("[log {label}]")),
            "missing {label}: {text}"
        );
    }
    assert_eq!(text.lines().count(), 5);
}

#[test]
fn export_logs_handles_all_attribute_value_types() {
    let (exporter, captured) = exporter_writing_to(StderrExporterScope::SpansAndLogs);
    exporter.export_logs(&[LogRecord {
        attributes: vec![
            ("flag".to_owned(), AttributeValue::Boolean(true)),
            ("off".to_owned(), AttributeValue::Boolean(false)),
            ("count".to_owned(), AttributeValue::Int64(-3)),
            ("ratio".to_owned(), AttributeValue::Float64(0.5)),
            ("name".to_owned(), AttributeValue::from("text")),
        ],
        ..a_log(Severity::Info)
    }]);

    let text = captured.text();
    for expected in [
        "flag=true",
        "off=false",
        "count=-3",
        "ratio=0.5",
        "name=text",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
}

#[test]
fn empty_batches_produce_no_output() {
    let (exporter, captured) = exporter_writing_to(StderrExporterScope::SpansAndLogs);
    exporter.export_spans(&[]);
    exporter.export_logs(&[]);
    exporter.export_metrics(&[]);
    assert!(captured.text().is_empty(), "{:?}", captured.text());
}

#[test]
fn logs_only_drops_spans_and_keeps_log_records() {
    let (exporter, captured) = exporter_writing_to(StderrExporterScope::LogsOnly);
    exporter.export_spans(&[a_span()]);
    exporter.export_logs(&[a_log(Severity::Info)]);

    let text = captured.text();
    assert!(!text.contains("[trace]"), "{text}");
    assert!(text.contains("[log info]"), "{text}");
}

#[test]
fn logs_only_is_what_the_named_constructor_selects() {
    // Rust-added: the C++ `logs_only()` factory has a default argument this port
    // cannot carry, so the two constructors are checked to agree on scope.
    // The named constructor itself, which this case used to construct and then
    // discard without asserting anything about — so it passed unchanged even if
    // `logs_only()` returned the wrong scope.
    assert_eq!(
        StderrExporter::logs_only().scope(),
        StderrExporterScope::LogsOnly
    );
    assert_eq!(
        StderrExporter::new().scope(),
        StderrExporterScope::default()
    );

    // …and that the scope it names actually suppresses spans.
    let (exporter, captured) = exporter_writing_to(StderrExporterScope::LogsOnly);
    exporter.export_spans(&[a_span()]);
    assert!(captured.text().is_empty(), "{:?}", captured.text());
}

/// Metrics are not printed — a console line per counter increment is noise —
/// and still counted, so `dropped` tells the truth about them.
#[test]
fn metrics_are_not_printed_and_are_counted_as_dropped() {
    let (exporter, captured) = exporter_writing_to(StderrExporterScope::SpansAndLogs);
    let point = MetricPoint {
        name: "requests".to_owned(),
        kind: MetricKind::Counter,
        value: AttributeValue::Int64(1),
        attributes: Vec::new(),
        timestamp: Epoch::from_millis(1_000),
        span_context: None,
        observation_id: ObservationId::default(),
    };
    exporter.export_metrics(&[point.clone(), point]);

    assert_eq!(captured.text(), "");
    assert_eq!(exporter.dropped(), 2);
}
