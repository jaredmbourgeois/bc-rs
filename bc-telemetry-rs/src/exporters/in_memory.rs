//! The exporter tests assert against.

use std::sync::Mutex;

use crate::exporter::TelemetryExporter;
use crate::log_record::LogRecord;
use crate::metrics::MetricPoint;
use crate::types::Span;

/// Keeps everything exported, for inspection.
#[derive(Debug, Default)]
pub struct InMemoryExporter {
    // One mutex over all three, matching the C++: a test that reads spans and
    // logs wants one consistent moment, not two.
    stored: Mutex<Stored>,
}

#[derive(Debug, Default)]
struct Stored {
    spans: Vec<Span>,
    metrics: Vec<MetricPoint>,
    logs: Vec<LogRecord>,
}

impl InMemoryExporter {
    /// An empty exporter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn stored(&self) -> std::sync::MutexGuard<'_, Stored> {
        self.stored
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Every span exported so far.
    #[must_use]
    pub fn spans(&self) -> Vec<Span> {
        self.stored().spans.clone()
    }

    /// Every metric point exported so far.
    #[must_use]
    pub fn metrics(&self) -> Vec<MetricPoint> {
        self.stored().metrics.clone()
    }

    /// Every log record exported so far.
    #[must_use]
    pub fn logs(&self) -> Vec<LogRecord> {
        self.stored().logs.clone()
    }

    /// Discards everything stored.
    pub fn clear(&self) {
        let mut stored = self.stored();
        stored.spans.clear();
        stored.metrics.clear();
        stored.logs.clear();
    }
}

impl TelemetryExporter for InMemoryExporter {
    fn export_spans(&self, spans: &[Span]) {
        self.stored().spans.extend_from_slice(spans);
    }

    fn export_metrics(&self, metrics: &[MetricPoint]) {
        self.stored().metrics.extend_from_slice(metrics);
    }

    fn export_logs(&self, logs: &[LogRecord]) {
        self.stored().logs.extend_from_slice(logs);
    }

    fn flush(&self) {}

    /// Always zero: this exporter keeps every record it is given, so there is
    /// nothing to lose. Spelled out rather than inherited from a default,
    /// because "cannot drop" and "forgot to count" must not look alike.
    fn dropped(&self) -> u64 {
        0
    }
}
