//! Ported from `libs/platform/telemetry/test/metrics_test.cpp`.

mod support;

use bc_telemetry::{
    AttributeValue, Attributes, MetricKind, Parent, SpanKind, current_span_context, record_counter,
    record_gauge, record_histogram, start_span,
};

use support::with_in_memory;

#[test]
fn record_counter_exports_a_counter_point() {
    with_in_memory(|exporter| {
        record_counter(Parent::current(), "requests", 3, Vec::new());
        let metrics = exporter.metrics();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "requests");
        assert_eq!(metrics[0].kind, MetricKind::Counter);
        assert_eq!(metrics[0].value, AttributeValue::Int64(3));
    });
}

#[test]
fn record_histogram_exports_a_histogram_point() {
    with_in_memory(|exporter| {
        record_histogram(Parent::current(), "latency_ms", 12.5, Vec::new());
        let metrics = exporter.metrics();
        assert_eq!(metrics[0].kind, MetricKind::Histogram);
        assert_eq!(metrics[0].value, AttributeValue::Float64(12.5));
    });
}

#[test]
fn record_gauge_exports_a_gauge_point() {
    with_in_memory(|exporter| {
        record_gauge(Parent::current(), "queue_depth", 4.0, Vec::new());
        let metrics = exporter.metrics();
        assert_eq!(metrics[0].kind, MetricKind::Gauge);
        assert_eq!(metrics[0].value, AttributeValue::Float64(4.0));
    });
}

#[test]
fn a_metric_captures_the_active_span_context() {
    with_in_memory(|exporter| {
        // Compared against *the active* context, not merely checked for
        // validity. `is_valid()` alone would pass if the recorder attached a
        // freshly generated context instead of the enclosing span's — which is
        // exactly the defect this case is named for.
        let active = {
            let _outer = start_span(
                Parent::current(),
                "outer",
                SpanKind::Internal,
                Attributes::new(),
            );
            record_counter(Parent::current(), "inside", 1, Vec::new());
            current_span_context().expect("a span is active inside its guard")
        };
        let metrics = exporter.metrics();
        let context = metrics[0]
            .span_context
            .expect("a metric recorded inside a span carries its context");
        assert_eq!(context, active);
    });
}

#[test]
fn an_increment_of_one_is_what_the_cpp_default_produced() {
    // Rewritten from `CounterDefaultIncrement`: Rust has no default arguments,
    // so the increment is always explicit. What the C++ case actually asserted
    // — that the common increment is 1 and lands as an `Int64` — is what this
    // keeps.
    with_in_memory(|exporter| {
        record_counter(Parent::current(), "hits", 1, Vec::new());
        assert_eq!(exporter.metrics()[0].value, AttributeValue::Int64(1));
    });
}

#[test]
fn recording_a_metric_with_no_pipeline_reaches_no_exporter() {
    support::with_uninstalled_exporter(|exporter| {
        record_counter(Parent::current(), "orphan", 1, Vec::new());
        record_histogram(Parent::current(), "orphan", 1.0, Vec::new());
        record_gauge(Parent::current(), "orphan", 1.0, Vec::new());
        // The observation, not merely the absence of a panic: the exporter that
        // was installed a moment ago received none of the three.
        assert!(exporter.metrics().is_empty(), "{:?}", exporter.metrics());
    });
}
