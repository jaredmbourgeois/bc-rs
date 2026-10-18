//! Ported from `libs/platform/telemetry/test/log_record_test.cpp`.

mod support;

use bc_telemetry::{
    AttributeValue, Attributes, Parent, Severity, SpanKind, current_span_context, record_log,
    start_span,
};

use support::with_in_memory;

#[test]
fn record_log_routes_through_the_pipeline() {
    with_in_memory(|exporter| {
        record_log(
            Parent::current(),
            Severity::Warn,
            "disk is filling",
            Vec::new(),
        );
        let logs = exporter.logs();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].body, "disk is filling");
        assert_eq!(logs[0].severity, Severity::Warn);
    });
}

#[test]
fn record_log_carries_attributes() {
    with_in_memory(|exporter| {
        record_log(
            Parent::current(),
            Severity::Info,
            "deployed",
            vec![("version".to_owned(), AttributeValue::from("1.2.3"))],
        );
        let logs = exporter.logs();
        assert_eq!(logs[0].attributes.len(), 1);
        assert_eq!(logs[0].attributes[0].0, "version");
    });
}

#[test]
fn record_log_captures_the_active_span_context() {
    with_in_memory(|exporter| {
        // Compared against *the active* context, for the same reason as the
        // metrics case: an `is_valid()` assertion cannot tell the enclosing
        // span's context from a newly minted one.
        let active = {
            let _request = start_span(
                Parent::current(),
                "request",
                SpanKind::Internal,
                Attributes::new(),
            );
            record_log(Parent::current(), Severity::Error, "failed", Vec::new());
            current_span_context().expect("a span is active inside its guard")
        };
        let logs = exporter.logs();
        let context = logs[0]
            .span_context
            .expect("a log recorded inside a span carries its context");
        assert_eq!(context, active);
    });
}

#[test]
fn recording_a_log_with_no_pipeline_reaches_no_exporter() {
    support::with_uninstalled_exporter(|exporter| {
        record_log(
            Parent::current(),
            Severity::Fatal,
            "nobody is listening",
            Vec::new(),
        );
        assert!(exporter.logs().is_empty(), "{:?}", exporter.logs());
    });
}

/// The uppercase label is a wire spelling — the file exporter's `severity` and
/// OTLP's `severityText` carry it — so each of the five is pinned.
#[test]
fn every_severity_has_its_uppercase_label() {
    let labels: Vec<&str> = [
        Severity::Debug,
        Severity::Info,
        Severity::Warn,
        Severity::Error,
        Severity::Fatal,
    ]
    .into_iter()
    .map(Severity::label)
    .collect();
    assert_eq!(labels, ["DEBUG", "INFO", "WARN", "ERROR", "FATAL"]);
}
