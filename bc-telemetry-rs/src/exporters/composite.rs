//! Fan-out to several exporters.

use std::sync::Arc;

use crate::exporter::TelemetryExporter;
use crate::log_record::LogRecord;
use crate::metrics::MetricPoint;
use crate::types::Span;

/// Forwards everything to each of several exporters.
///
/// **A deviation worth stating.** The C++ wraps each forward in a `try`/`catch`
/// and writes a line to `std::cerr` when one throws, because its exporter
/// methods can throw and one failing must not stop the others. Here the trait's
/// methods are infallible by signature — an exporter that cannot write drops and
/// counts instead — so there is nothing to catch and the fan-out is a plain
/// loop. The isolation the C++ bought with `catch` is bought here by the type.
#[derive(Debug)]
pub struct CompositeExporter {
    exporters: Vec<Arc<dyn TelemetryExporter>>,
}

impl CompositeExporter {
    /// Fans out to `exporters`, in order.
    #[must_use]
    pub fn new(exporters: Vec<Arc<dyn TelemetryExporter>>) -> Self {
        Self { exporters }
    }
}

impl TelemetryExporter for CompositeExporter {
    fn export_spans(&self, spans: &[Span]) {
        for exporter in &self.exporters {
            exporter.export_spans(spans);
        }
    }

    fn export_metrics(&self, metrics: &[MetricPoint]) {
        for exporter in &self.exporters {
            exporter.export_metrics(metrics);
        }
    }

    fn export_logs(&self, logs: &[LogRecord]) {
        for exporter in &self.exporters {
            exporter.export_logs(logs);
        }
    }

    fn flush(&self) {
        for exporter in &self.exporters {
            exporter.flush();
        }
    }

    /// The sum of what every wrapped exporter dropped.
    ///
    /// Saturating rather than wrapping: a drop count that rolled over to a small
    /// number would read as healthy, which is the one thing a drop counter must
    /// never do.
    fn dropped(&self) -> u64 {
        self.exporters.iter().fold(0_u64, |total, exporter| {
            total.saturating_add(exporter.dropped())
        })
    }
}
