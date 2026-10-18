//! The console exporter.

use std::io::Write;
use std::sync::Mutex;

use crate::exporter::TelemetryExporter;
use crate::log_record::{LogRecord, Severity};
use crate::metrics::MetricPoint;
use crate::types::{AttributeValue, Span, StatusCode};

/// What a [`StderrExporter`] writes.
///
/// `SpansAndLogs` is the development shape: nothing else is installed, so the
/// console is the only telemetry surface and carries everything. `LogsOnly` is
/// for a process that *also* ships to a collector. There the collector owns the
/// spans — a line per request span would bury the journal — but the log records
/// stay on the console, because `journalctl -u <app>` is where an operator reads
/// why a process refused to start, and that must not depend on a collector being
/// reachable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StderrExporterScope {
    /// Write spans and log records.
    #[default]
    SpansAndLogs,
    /// Write only log records.
    LogsOnly,
}

/// Writes finished spans and log records to a stream.
///
/// The C++ takes `std::ostream&` defaulting to `std::cerr`; here the sink is an
/// owned boxed writer, because Rust has no defaulted arguments and a borrowed
/// stream would put a lifetime on a type the pipeline stores behind an `Arc`.
pub struct StderrExporter {
    out: Mutex<Box<dyn Write + Send>>,
    scope: StderrExporterScope,
    /// Metrics this exporter discarded. Counted rather than assumed zero: it
    /// really does drop every metric, and this crate never hides a drop, even
    /// one that is deliberate and documented.
    dropped: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for StderrExporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StderrExporter")
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl StderrExporter {
    /// Writes spans and logs to the process's standard error.
    #[must_use]
    pub fn new() -> Self {
        Self::to_writer(
            Box::new(std::io::stderr()),
            StderrExporterScope::SpansAndLogs,
        )
    }

    /// Writes only log records to the process's standard error.
    ///
    /// The console half of a process that also ships to a collector. Named
    /// rather than a second argument so a caller selects it without naming a
    /// process stream.
    #[must_use]
    pub fn logs_only() -> Self {
        Self::to_writer(Box::new(std::io::stderr()), StderrExporterScope::LogsOnly)
    }

    /// Which record kinds this exporter writes.
    ///
    /// Public so a caller — and a test — can ask what a named constructor
    /// selected, rather than inferring it from observed behaviour.
    #[must_use]
    pub const fn scope(&self) -> StderrExporterScope {
        self.scope
    }

    /// Writes to any sink — the seam a test uses to read what was written.
    #[must_use]
    pub fn to_writer(out: Box<dyn Write + Send>, scope: StderrExporterScope) -> Self {
        Self {
            out: Mutex::new(out),
            scope,
            dropped: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn with_out(&self, write: impl FnOnce(&mut dyn Write)) {
        let mut out = self
            .out
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        write(out.as_mut());
    }
}

impl Default for StderrExporter {
    fn default() -> Self {
        Self::new()
    }
}

/// The lowercase severity label the C++ exporter writes.
///
/// Deliberately not [`Severity::label`], which is the uppercase spelling bc's
/// log library uses: this is the console format the source defines, and the two
/// are allowed to differ because they answer to different contracts.
const fn console_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Debug => "debug",
        Severity::Info => "info",
        Severity::Warn => "warn",
        Severity::Error => "error",
        Severity::Fatal => "fatal",
    }
}

fn write_attribute(out: &mut dyn Write, value: &AttributeValue) {
    // Every write here is best-effort: a console that cannot be written to is
    // not a reason to fail the caller.
    let _ = match value {
        AttributeValue::Boolean(value) => write!(out, "{}", if *value { "true" } else { "false" }),
        AttributeValue::Int64(value) => write!(out, "{value}"),
        AttributeValue::Float64(value) => write!(out, "{value}"),
        AttributeValue::String(value) => write!(out, "{value}"),
    };
}

impl TelemetryExporter for StderrExporter {
    fn export_spans(&self, spans: &[Span]) {
        if self.scope == StderrExporterScope::LogsOnly {
            return;
        }
        self.with_out(|out| {
            for span in spans {
                let duration = span.end_time.delta_since(span.start_time).millis();
                let _ = write!(out, "[trace] {} ({duration}ms)", span.name);
                if span.status == StatusCode::Error {
                    let _ = write!(out, " ERROR");
                    if !span.status_message.is_empty() {
                        let _ = write!(out, ": {}", span.status_message);
                    }
                }
                for (key, value) in &span.attributes {
                    let _ = write!(out, " {key}=");
                    write_attribute(out, value);
                }
                let _ = writeln!(out);
                // Each event on its own indented line under its span. An event
                // is where a failure's detail lives and where a job's findings
                // ride; on a process whose only sink is stderr, a failed run
                // otherwise reads `ERROR: <kind>` with no reason anywhere.
                for event in &span.events {
                    let _ = write!(out, "  event {}", event.name);
                    for (key, value) in &event.attributes {
                        let _ = write!(out, " {key}=");
                        write_attribute(out, value);
                    }
                    let _ = writeln!(out);
                }
            }
        });
    }

    /// Metrics are deliberately not written: the C++ exporter drops them too,
    /// because a console line per counter increment is noise rather than signal.
    ///
    /// Deliberate, documented — and still counted. A reader asking `dropped()`
    /// gets the truth rather than a zero that happens to match a design choice.
    fn export_metrics(&self, metrics: &[MetricPoint]) {
        self.dropped
            .fetch_add(metrics.len() as u64, std::sync::atomic::Ordering::Relaxed);
    }

    fn dropped(&self) -> u64 {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn export_logs(&self, logs: &[LogRecord]) {
        self.with_out(|out| {
            for log in logs {
                let _ = write!(out, "[log {}] {}", console_label(log.severity), log.body);
                for (key, value) in &log.attributes {
                    let _ = write!(out, " {key}=");
                    write_attribute(out, value);
                }
                let _ = writeln!(out);
            }
        });
    }

    fn flush(&self) {
        self.with_out(|out| {
            let _ = out.flush();
        });
    }
}
