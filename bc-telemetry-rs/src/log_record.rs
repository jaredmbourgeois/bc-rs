//! Span-correlated log records.

use std::panic::{self, AssertUnwindSafe};

use bc_date_time::Epoch;

use crate::context::Parent;
use crate::pipeline::{DropReason, installed_pipeline};
use crate::types::{Attributes, ObservationId, SpanContext};

/// A log record's severity.
///
/// **A deviation from the C++, and a deliberate one.** The original's
/// `LogRecord` uses `bc::LogLevel` from the log library directly. This family
/// has no log library — telemetry owns logging — so the severity is this
/// crate's own enum, with the same five levels and the same discriminants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Severity {
    /// Diagnostic detail.
    Debug = 0,
    /// Normal operational information.
    #[default]
    Info = 1,
    /// A recoverable concern.
    Warn = 2,
    /// An operation failed.
    Error = 3,
    /// An unrecoverable condition.
    Fatal = 4,
}

impl Severity {
    /// The uppercase label, matching the C++ log library's.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Fatal => "FATAL",
        }
    }
}

/// One log entry routed through the telemetry pipeline.
#[derive(Clone, Debug, PartialEq)]
pub struct LogRecord {
    /// When it was recorded.
    pub timestamp: Epoch,
    /// Its severity.
    pub severity: Severity,
    /// The message body.
    pub body: String,
    /// Attributes attached to the record.
    pub attributes: Attributes,
    /// The span in force when it was recorded, if any.
    pub span_context: Option<SpanContext>,
    /// What identifies this record, so a writer stores it once.
    pub observation_id: ObservationId,
}

/// Routes a log entry under `parent` through the installed pipeline.
///
/// A no-op when no pipeline is installed — telemetry never fails the caller.
pub fn record_log(
    parent: Parent,
    severity: Severity,
    body: impl Into<String>,
    attributes: Attributes,
) {
    let Some(pipeline) = installed_pipeline() else {
        return;
    };
    let Some(observation_id) = pipeline.next_observation() else {
        // A record no writer could store exactly once: counted, not exported.
        pipeline.record_drop(DropReason::Identity);
        return;
    };
    // The clock and the exporter are caller-supplied: a panic in either costs
    // the record and a count, never the caller.
    let Ok(timestamp) = panic::catch_unwind(AssertUnwindSafe(|| pipeline.now())) else {
        pipeline.record_drop(DropReason::Failed);
        return;
    };
    let record = LogRecord {
        timestamp,
        severity,
        body: body.into(),
        attributes,
        span_context: parent.context(),
        observation_id,
    };
    let exported = panic::catch_unwind(AssertUnwindSafe(|| {
        pipeline.exporter().export_logs(&[record]);
    }));
    if exported.is_err() {
        pipeline.record_drop(DropReason::Failed);
    }
}
