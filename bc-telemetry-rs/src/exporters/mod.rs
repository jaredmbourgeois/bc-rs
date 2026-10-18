//! The exporters that ship in the default build.
//!
//! Three are ported from the C++ — [`InMemoryExporter`], [`StderrExporter`],
//! and [`CompositeExporter`] — and one, [`FileExporter`], is new surface the
//! original never had: a line-delimited JSON file, versioned so that a reader
//! tailing it can evolve with it.

#[cfg(feature = "batched")]
mod batched;
use bc_json::JsonWriter;

use crate::metrics::MetricKind;
#[cfg(any(feature = "otlp", feature = "postgres"))]
use crate::types::AttributeValue;
use crate::types::{SpanKind, StatusCode};

/// A metric's value as a double, or `None` when it is not a measurement.
///
/// Shared because both exporters that write a numeric column need exactly this
/// answer, and they used to disagree about it: this returned `Some(0.0)` for a
/// string in one and `None` in the other, so the same point was a counted drop
/// in Postgres and a fabricated zero in OTLP. A reader cannot tell a fabricated
/// zero from a measured one, so a fabricated zero is a hidden drop, and this
/// crate permits a drop but never hides one. So `None` is the answer, and the
/// caller counts the point it cannot send.
///
/// A value beyond 2^53 loses precision here. That is accepted rather than
/// hidden: an integer counter travels exactly through OTLP's `asInt` decimal
/// string, and this serves only the paths whose values are doubles already.
#[cfg(any(feature = "otlp", feature = "postgres"))]
#[allow(clippy::cast_precision_loss)]
pub(crate) fn numeric_of(value: &AttributeValue) -> Option<f64> {
    match value {
        AttributeValue::Int64(value) => Some(*value as f64),
        AttributeValue::Float64(value) => Some(*value),
        AttributeValue::Boolean(value) => Some(f64::from(u8::from(*value))),
        AttributeValue::String(_) => None,
    }
}

/// The wire word for a span kind, a status, and a metric kind.
///
/// Shared for the reason `numeric_of` above is shared, and with the same
/// history one step earlier: the file and Postgres exporters each carried a
/// byte-identical private copy of all three, in a crate that had already
/// hoisted a shared answer *because* two exporters disagreed about one. Two
/// tables for one vocabulary is the same hazard whether or not they currently
/// agree — a rename lands in one and the two surfaces then name one span kind
/// two ways, with each exporter's own tests passing.
pub(crate) const fn span_kind_name(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Internal => "internal",
        SpanKind::Server => "server",
        SpanKind::Client => "client",
        SpanKind::Producer => "producer",
        SpanKind::Consumer => "consumer",
    }
}

/// See [`span_kind_name`].
pub(crate) const fn status_name(status: StatusCode) -> &'static str {
    match status {
        StatusCode::Unset => "unset",
        StatusCode::Ok => "ok",
        StatusCode::Error => "error",
    }
}

/// See [`span_kind_name`].
pub(crate) const fn metric_kind_name(kind: MetricKind) -> &'static str {
    match kind {
        MetricKind::Counter => "counter",
        MetricKind::Histogram => "histogram",
        MetricKind::Gauge => "gauge",
    }
}

/// Writes one element of the enclosing array, as an object.
///
/// `JsonWriter::array_value` writes a separator and then calls its body, so the
/// body has to write a whole value — a bare sequence of `field` calls produces
/// `[,"key":…]`, which is not JSON. This pairs the two calls once so no caller
/// has to remember.
pub(crate) fn object_element(json: &mut JsonWriter, body: impl FnOnce(&mut JsonWriter)) {
    json.array_value(|json| {
        json.object(body);
    });
}

mod composite;
mod file;
mod in_memory;
#[cfg(feature = "otlp")]
mod otlp;
#[cfg(feature = "postgres")]
mod postgres;
mod stderr;

#[cfg(feature = "batched")]
pub use batched::BatchedExporter;
pub use composite::CompositeExporter;
pub use file::{FileExporter, FileExporterConfig, LINE_SCHEMA_VERSION};
pub use in_memory::InMemoryExporter;
#[cfg(feature = "otlp")]
pub use otlp::{OtlpExporter, OtlpExporterConfig};
#[cfg(feature = "postgres")]
pub use postgres::{PostgresBatchOutcome, PostgresExporter, PostgresWriteError};
pub use stderr::{StderrExporter, StderrExporterScope};
