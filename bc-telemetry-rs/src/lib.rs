//! OTel-shaped telemetry: spans, metrics, and log records behind one exporter.
//!
//! A semantics-preserving Rust port of `libs/platform/telemetry` from
//! `bourgeois_concern`. Three rules carry the whole crate, in order of
//! importance:
//!
//! 1. **Telemetry never fails the caller.** Every recording call is infallible
//!    by signature. No pipeline installed means a per-call drop; an exporter
//!    that cannot write drops and counts. This is the source's load-bearing
//!    rule, kept verbatim, and it is why nothing here returns a `Result`.
//! 2. **Dropping is permitted; hiding a drop is not.** Every drop is counted,
//!    the count is readable through [`Pipeline::dropped`], and the file exporter
//!    periodically emits it as a record of its own — so a reader tailing the
//!    file sees a gap rather than a seamless lie.
//! 3. **Telemetry is exhaust, never evidence.** Nothing here is meant to be read
//!    back by a decision: telemetry may drop, so it proves nothing.
//!
//! ```
//! use std::sync::Arc;
//! use bc_telemetry::{
//!     Attributes, InMemoryExporter, Parent, PipelineBuilder, SpanKind, StatusCode, ends_ok,
//!     fixed_id_source, running_span,
//! };
//! use bc_date_time::{Epoch, fixed_epoch_source};
//!
//! let exporter = Arc::new(InMemoryExporter::new());
//! let _pipeline = PipelineBuilder::new(exporter.clone())
//!     .epoch_source(fixed_epoch_source(Epoch::from_millis(1_000)))
//!     .id_source(fixed_id_source(0xAB))
//!     .install();
//!
//! // Every choice is an argument: the parent, the kind, the attributes, and how
//! // the body's result ends the span.
//! let answer = running_span(
//!     Parent::current(),
//!     "compute",
//!     SpanKind::Internal,
//!     Attributes::new(),
//!     ends_ok,
//!     |_span| 6 * 7,
//! );
//!
//! assert_eq!(answer, 42);
//! let spans = exporter.spans();
//! assert_eq!(spans.len(), 1);
//! assert_eq!(spans[0].name, "compute");
//! assert_eq!(spans[0].status, StatusCode::Ok);
//! assert_eq!(spans[0].kind, SpanKind::Internal);
//! ```
//!
//! # What is deliberately out of scope
//!
//! Hosting a collector; a metrics aggregator (points are exported as recorded,
//! not summed); and any decision that reads telemetry back.
#![deny(missing_docs)]

mod context;
mod error_telemetry;
mod exporter;
mod exporters;
mod id_source;
mod log_record;
mod metrics;
mod pipeline;
mod propagation;
mod types;

pub use context::{
    InSpan, OpenSpan, Parent, SpanGuard, SpanStatus, add_span_event, add_span_event_if_any,
    current_span_context, ends_by_result, ends_ok, open_span, running_span, running_span_async,
    set_span_attribute, set_span_attribute_if_any, set_span_status, set_span_status_if_any,
    start_span, with_span, with_span_async, with_span_ending,
};
pub use error_telemetry::{
    NotThisError, NotThisErrorReason, error_identity_attributes, record_error_telemetry,
    report_degraded, report_error,
};
pub use exporter::{
    Sampler, SamplingDecision, TelemetryExporter, always_on_sampler, probability_sampler,
};
pub use exporters::{
    CompositeExporter, FileExporter, FileExporterConfig, InMemoryExporter, LINE_SCHEMA_VERSION,
    StderrExporter, StderrExporterScope,
};
pub use id_source::{IdSource, fixed_id_source, system_id_source};
pub use log_record::{LogRecord, Severity, record_log};
pub use metrics::{MetricKind, MetricPoint, record_counter, record_gauge, record_histogram};
pub use pipeline::{
    DropCounts, DropReason, Pipeline, PipelineBuilder, PipelineScope, drop_counts, dropped_count,
    installed_pipeline, shutdown_pipeline,
};
pub use propagation::{format_traceparent, parse_traceparent};
pub use types::{
    AttributeValue, Attributes, ObservationId, Span, SpanContext, SpanEvent, SpanId, SpanKind,
    StatusCode, TRACE_FLAG_SAMPLED, TraceId,
};

#[cfg(feature = "batched")]
pub use exporters::BatchedExporter;
#[cfg(feature = "otlp")]
pub use exporters::{OtlpExporter, OtlpExporterConfig};
#[cfg(feature = "postgres")]
pub use exporters::{PostgresBatchOutcome, PostgresExporter, PostgresWriteError};

/// Compiles every ```rust block in README.md as a doctest, so the examples the
/// README shows cannot drift from the API it documents.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeExamples;
