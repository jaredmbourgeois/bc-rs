//! Metric points and the free functions that record them.

use std::panic::{self, AssertUnwindSafe};

use bc_date_time::Epoch;

use crate::context::Parent;
use crate::pipeline::{DropReason, installed_pipeline};
use crate::types::{AttributeValue, Attributes, ObservationId, SpanContext};

/// What a metric measures.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MetricKind {
    /// A monotonically increasing count.
    #[default]
    Counter = 0,
    /// A distribution of observed values.
    Histogram = 1,
    /// A point-in-time reading.
    Gauge = 2,
}

/// One recorded measurement.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricPoint {
    /// The metric's name.
    pub name: String,
    /// What it measures.
    pub kind: MetricKind,
    /// The observed value.
    pub value: AttributeValue,
    /// Attributes attached to the point.
    pub attributes: Attributes,
    /// When it was observed.
    pub timestamp: Epoch,
    /// The span in force when it was observed, if any.
    pub span_context: Option<SpanContext>,
    /// What identifies this point, so a writer stores it once.
    pub observation_id: ObservationId,
}

/// Records one point under `parent` through the installed pipeline, or drops
/// it.
fn record_metric(
    parent: Parent,
    name: String,
    kind: MetricKind,
    value: AttributeValue,
    attributes: Attributes,
) {
    let Some(pipeline) = installed_pipeline() else {
        // No pipeline is a per-call drop, counted nowhere because there is
        // nothing installed to count it on. The drop is permitted; what is not
        // is an *installed* pipeline hiding one.
        return;
    };
    let Some(observation_id) = pipeline.next_observation() else {
        // A point no writer could store exactly once: counted, not exported.
        pipeline.record_drop(DropReason::Identity);
        return;
    };
    // The clock and the exporter are caller-supplied: a panic in either costs
    // the point and a count, never the caller.
    let Ok(timestamp) = panic::catch_unwind(AssertUnwindSafe(|| pipeline.now())) else {
        pipeline.record_drop(DropReason::Failed);
        return;
    };
    let point = MetricPoint {
        name,
        kind,
        value,
        attributes,
        timestamp,
        span_context: parent.context(),
        observation_id,
    };
    let exported = panic::catch_unwind(AssertUnwindSafe(|| {
        pipeline.exporter().export_metrics(&[point]);
    }));
    if exported.is_err() {
        pipeline.record_drop(DropReason::Failed);
    }
}

/// Records an increment to a counter, under `parent`.
pub fn record_counter(
    parent: Parent,
    name: impl Into<String>,
    increment: i64,
    attributes: Attributes,
) {
    record_metric(
        parent,
        name.into(),
        MetricKind::Counter,
        AttributeValue::Int64(increment),
        attributes,
    );
}

/// Records an observation into a histogram, under `parent`.
pub fn record_histogram(
    parent: Parent,
    name: impl Into<String>,
    value: f64,
    attributes: Attributes,
) {
    record_metric(
        parent,
        name.into(),
        MetricKind::Histogram,
        AttributeValue::Float64(value),
        attributes,
    );
}

/// Records a gauge reading, under `parent`.
pub fn record_gauge(parent: Parent, name: impl Into<String>, value: f64, attributes: Attributes) {
    record_metric(
        parent,
        name.into(),
        MetricKind::Gauge,
        AttributeValue::Float64(value),
        attributes,
    );
}
