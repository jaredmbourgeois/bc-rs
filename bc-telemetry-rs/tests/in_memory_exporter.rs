//! Ported from `libs/platform/telemetry/test/in_memory_exporter_test.cpp`.

use std::sync::Arc;
use std::thread;

use bc_date_time::Epoch;
use bc_telemetry::{
    InMemoryExporter, LogRecord, MetricKind, MetricPoint, ObservationId, Severity, Span,
    TelemetryExporter,
};

fn a_span() -> Span {
    Span {
        name: "work".to_owned(),
        ..Span::default()
    }
}

fn a_metric() -> MetricPoint {
    MetricPoint {
        name: "hits".to_owned(),
        kind: MetricKind::Counter,
        value: bc_telemetry::AttributeValue::Int64(1),
        attributes: Vec::new(),
        timestamp: Epoch::ZERO,
        span_context: None,
        observation_id: ObservationId::default(),
    }
}

fn a_log() -> LogRecord {
    LogRecord {
        timestamp: Epoch::ZERO,
        severity: Severity::Info,
        body: "hello".to_owned(),
        attributes: Vec::new(),
        span_context: None,
        observation_id: ObservationId::default(),
    }
}

#[test]
fn collects_spans_metrics_and_logs() {
    let exporter = InMemoryExporter::new();
    exporter.export_spans(&[a_span()]);
    exporter.export_metrics(&[a_metric()]);
    exporter.export_logs(&[a_log()]);

    assert_eq!(exporter.spans().len(), 1);
    assert_eq!(exporter.metrics().len(), 1);
    assert_eq!(exporter.logs().len(), 1);
    assert_eq!(exporter.spans()[0].name, "work");
}

#[test]
fn clear_empties_all() {
    let exporter = InMemoryExporter::new();
    exporter.export_spans(&[a_span()]);
    exporter.export_metrics(&[a_metric()]);
    exporter.export_logs(&[a_log()]);

    exporter.clear();

    assert!(exporter.spans().is_empty(), "{:?}", exporter.spans());
    assert!(exporter.metrics().is_empty(), "{:?}", exporter.metrics());
    assert!(exporter.logs().is_empty(), "{:?}", exporter.logs());
}

#[test]
fn concurrent_export_and_read_is_thread_safe() {
    // Not a timing test: every thread is joined, and the assertion is the exact
    // total. A missing lock shows up as a lost write or a torn read, not as a
    // flake, so there is nothing here to wait on.
    let exporter = Arc::new(InMemoryExporter::new());
    let writers = 8;
    let per_writer = 50;

    let handles: Vec<_> = (0..writers)
        .map(|_| {
            let exporter = Arc::clone(&exporter);
            thread::spawn(move || {
                for _ in 0..per_writer {
                    exporter.export_spans(&[a_span()]);
                    // Read concurrently with the other writers, which is the
                    // half of the case that exercises the shared lock.
                    let _ = exporter.spans().len();
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("no writer panicked");
    }
    assert_eq!(exporter.spans().len(), writers * per_writer);
}
