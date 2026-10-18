//! The line-delimited JSON file exporter.
//!
//! **New surface — the C++ original has no file exporter.** Its lines are a
//! versioned surface and a dashboard tails them, so the lifecycle matters as
//! much as the format: one append-only file per exporter, rotated at a size cap
//! by rename-and-reopen (a tailing reader re-tails on rotation), with rotated
//! files bounded by a count so the live-file cap alone cannot grow the
//! directory forever.
//!
//! Every line is one JSON object carrying a `schema_version`, so a file that
//! spans upgrades holds mixed line schemas and a reader evolves additively: an
//! unknown schema is skipped and counted, never a parse failure.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use bc_date_time::Epoch;
use bc_json::JsonWriter;

use crate::exporter::TelemetryExporter;
use crate::exporters::{metric_kind_name, span_kind_name, status_name};
use crate::log_record::LogRecord;
use crate::metrics::MetricPoint;
use crate::types::{AttributeValue, Attributes, Span, SpanContext};

/// The schema version every line carries.
///
/// Bumped when the meaning of an existing key changes; a genuinely new key is
/// an addition and needs no bump.
pub const LINE_SCHEMA_VERSION: u32 = 1;

/// How a [`FileExporter`] rotates.
#[derive(Clone, Debug)]
pub struct FileExporterConfig {
    /// Where the live file lives.
    pub path: PathBuf,
    /// Rotate once the live file reaches this many bytes.
    pub max_bytes: u64,
    /// How many rotated files to keep.
    ///
    /// **Zero discards the live file's contents** rather than keeping them
    /// unrotated: with nowhere to move the full file, rotation unlinks it and
    /// starts a new one. A reader already tailing the old inode still reads it
    /// to the end, but nothing new can open it. Set this to at least 1 to keep
    /// the previous file.
    pub max_rotated: usize,
    /// Emit a drop-count record every this many written lines.
    ///
    /// Zero disables the periodic record. The counter is still readable through
    /// [`TelemetryExporter::dropped`] — this only controls whether a *tailing*
    /// reader is told, which is the half that keeps a drop from being hidden.
    pub drop_report_interval: u64,
}

impl FileExporterConfig {
    /// A configuration with the default caps: 64 MiB per file, eight rotated
    /// files kept, a drop record every 1024 lines.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            max_bytes: 64 * 1024 * 1024,
            max_rotated: 8,
            drop_report_interval: 1024,
        }
    }
}

/// Appends telemetry to a line-delimited JSON file.
pub struct FileExporter {
    config: FileExporterConfig,
    state: Mutex<Option<OpenFile>>,
    dropped: AtomicU64,
    written: AtomicU64,
}

#[derive(Debug)]
struct OpenFile {
    handle: File,
    bytes: u64,
}

impl std::fmt::Debug for FileExporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileExporter")
            .field("path", &self.config.path)
            .field("dropped", &self.dropped.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl FileExporter {
    /// Opens (or creates) the live file.
    ///
    /// Infallible by design, like every other path in this crate: if the file
    /// cannot be opened, every write is a counted drop rather than an error the
    /// caller has to handle. A telemetry sink that refuses to construct would
    /// make instrumentation a failure path.
    #[must_use]
    pub fn new(config: FileExporterConfig) -> Self {
        let state = open_live(&config.path);
        Self {
            config,
            state: Mutex::new(state),
            dropped: AtomicU64::new(0),
            written: AtomicU64::new(0),
        }
    }

    /// Writes one already-rendered line, rotating first if it would not fit.
    ///
    /// Every line goes through here, the drop record included. It used to have
    /// its own copy of this logic that skipped rotation, discarded write errors,
    /// and added the bytes anyway — so a failing descriptor stayed installed and
    /// the size accounting drifted upward from writes that never landed.
    fn write_line(&self, line: &str) {
        // Only a line that actually landed counts toward the drop-report
        // interval: counting attempts would let a run of failures trigger a
        // report that then fails too, counting a further drop for the record
        // that was meant to announce the drops.
        if self.write_raw_line(line) {
            self.maybe_report_drops();
        }
    }

    /// The one write path: re-open, rotate, append, account.
    ///
    /// Returns whether the line reached the file.
    fn write_raw_line(&self, line: &str) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Re-open if a previous write failed or the file never opened, so a
        // transient failure (a full disk that was cleared) recovers by itself.
        if state.is_none() {
            *state = open_live(&self.config.path);
        }
        let Some(open) = state.as_mut() else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        };

        let bytes = line.len() as u64 + 1;
        if open.bytes > 0 && open.bytes + bytes > self.config.max_bytes {
            let Some(reopened) = rotate(&self.config) else {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            };
            *open = reopened;
        }

        // One `write_all` of line-plus-newline rather than `writeln!`, which
        // formats and so issues at least two writes. Within this process the
        // mutex serializes them either way; across processes appending to the
        // same file, `O_APPEND` makes each individual write atomic but not the
        // pair, so a two-call write can interleave and tear an LDJSON record.
        let mut record = String::with_capacity(line.len() + 1);
        record.push_str(line);
        record.push('\n');
        if open.handle.write_all(record.as_bytes()).is_err() {
            // Drop the handle so the next write re-opens rather than writing
            // into a file descriptor that has already failed.
            *state = None;
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        open.bytes += bytes;
        true
    }

    /// Emits a drop-count line every `drop_report_interval` written lines.
    ///
    /// This is what tells a tailing reader about a drop: a gap it can see beats
    /// a seamless lie. The record goes through `write_raw_line`, so it
    /// rotates and accounts like any other line — but not through `write_line`,
    /// which would count it toward the interval that produced it and recurse.
    fn maybe_report_drops(&self) {
        let interval = self.config.drop_report_interval;
        if interval == 0 {
            return;
        }
        let written = self.written.fetch_add(1, Ordering::Relaxed) + 1;
        // `u64::is_multiple_of` is unstable on the pinned 1.85.
        if written % interval != 0 {
            return;
        }
        let dropped = self.dropped.load(Ordering::Relaxed);
        if dropped == 0 {
            return;
        }
        let mut json = JsonWriter::new();
        json.object(|json| {
            json.field("schema_version", LINE_SCHEMA_VERSION);
            json.field("kind", "drops");
            json.field("dropped", dropped);
        });
        if let Ok(line) = json.finish() {
            self.write_raw_line(&line);
        }
    }
}

fn open_live(path: &Path) -> Option<OpenFile> {
    // Nested rather than a let-chain: those stabilized after this family's
    // pinned 1.85.
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let handle = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    let bytes = handle.metadata().map_or(0, |meta| meta.len());
    Some(OpenFile { handle, bytes })
}

/// Renames the live file aside and opens a fresh one.
///
/// Rename-and-reopen rather than truncate: a reader tailing the old inode keeps
/// reading it to its end instead of seeing its content vanish mid-line.
fn rotate(config: &FileExporterConfig) -> Option<OpenFile> {
    if config.max_rotated == 0 {
        let _ = std::fs::remove_file(&config.path);
        return open_live(&config.path);
    }
    // Shift `.N` up, dropping the oldest, then move the live file to `.1`.
    for index in (1..config.max_rotated).rev() {
        let from = rotated_path(&config.path, index);
        let to = rotated_path(&config.path, index + 1);
        if from.exists() {
            let _ = std::fs::rename(&from, &to);
        }
    }
    let _ = std::fs::rename(&config.path, rotated_path(&config.path, 1));
    open_live(&config.path)
}

fn rotated_path(path: &Path, index: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}

/// Writes attributes as an ordered array of key/value objects.
///
/// Not a JSON object keyed by attribute name, though that reads more naturally:
/// `Attributes` is a `Vec` whose own documentation says duplicate keys are the
/// caller's business, and `set_span_attribute` appends without de-duplicating.
/// Rendered as an object, two attributes sharing a name poison the writer with
/// `DuplicateKey` in a debug build — dropping the whole record — and emit JSON
/// that this crate's own validator rejects in a release build. An array mirrors
/// the type that actually exists, keeps caller order, and is the shape the OTLP
/// exporter already uses, so all three exporters now agree.
fn write_attributes(json: &mut JsonWriter, attributes: &Attributes) {
    json.array("attributes", |json| {
        for (key, value) in attributes {
            super::object_element(json, |json| {
                json.field("key", key.as_str());
                match value {
                    AttributeValue::Boolean(value) => json.field("value", *value),
                    AttributeValue::Int64(value) => json.field("value", *value),
                    AttributeValue::Float64(value) => json.field("value", *value),
                    AttributeValue::String(value) => json.field("value", value.as_str()),
                }
            });
        }
    });
}

fn write_span_context(json: &mut JsonWriter, context: Option<&SpanContext>) {
    if let Some(context) = context {
        json.field("trace_id", context.trace_id.to_hex().as_str());
        json.field("span_id", context.span_id.to_hex().as_str());
    } else {
        json.field_null("trace_id");
        json.field_null("span_id");
    }
}

fn epoch_millis(epoch: Epoch) -> i64 {
    epoch.millis()
}

impl TelemetryExporter for FileExporter {
    fn export_spans(&self, spans: &[Span]) {
        for span in spans {
            let mut json = JsonWriter::new();
            json.object(|json| {
                json.field("schema_version", LINE_SCHEMA_VERSION);
                json.field("kind", "span");
                json.field("name", span.name.as_str());
                write_span_context(json, Some(&span.context));
                json.optional_field(
                    "parent_span_id",
                    span.parent_span_id.map(|parent| parent.to_hex()),
                );
                json.field("span_kind", span_kind_name(span.kind));
                json.field("status", status_name(span.status));
                json.field("status_message", span.status_message.as_str());
                json.field("start_time", epoch_millis(span.start_time));
                json.field("end_time", epoch_millis(span.end_time));
                json.field(
                    "duration_ms",
                    span.end_time.delta_since(span.start_time).millis(),
                );
                write_attributes(json, &span.attributes);
                json.array("events", |json| {
                    for event in &span.events {
                        super::object_element(json, |json| {
                            json.field("name", event.name.as_str());
                            json.field("timestamp", epoch_millis(event.timestamp));
                            write_attributes(json, &event.attributes);
                        });
                    }
                });
            });
            match json.finish() {
                Ok(line) => self.write_line(&line),
                Err(_) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    fn export_metrics(&self, metrics: &[MetricPoint]) {
        for point in metrics {
            let mut json = JsonWriter::new();
            json.object(|json| {
                json.field("schema_version", LINE_SCHEMA_VERSION);
                json.field("kind", "metric");
                json.field("name", point.name.as_str());
                json.field("metric_kind", metric_kind_name(point.kind));
                match &point.value {
                    AttributeValue::Boolean(value) => json.field("value", *value),
                    AttributeValue::Int64(value) => json.field("value", *value),
                    AttributeValue::Float64(value) => json.field("value", *value),
                    AttributeValue::String(value) => json.field("value", value.as_str()),
                }
                json.field("timestamp", epoch_millis(point.timestamp));
                write_span_context(json, point.span_context.as_ref());
                write_attributes(json, &point.attributes);
            });
            match json.finish() {
                Ok(line) => self.write_line(&line),
                Err(_) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    fn export_logs(&self, logs: &[LogRecord]) {
        for log in logs {
            let mut json = JsonWriter::new();
            json.object(|json| {
                json.field("schema_version", LINE_SCHEMA_VERSION);
                json.field("kind", "log");
                json.field("severity", log.severity.label());
                json.field("body", log.body.as_str());
                json.field("timestamp", epoch_millis(log.timestamp));
                write_span_context(json, log.span_context.as_ref());
                write_attributes(json, &log.attributes);
            });
            match json.finish() {
                Ok(line) => self.write_line(&line),
                Err(_) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    fn flush(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(open) = state.as_mut() {
            let _ = open.handle.flush();
        }
    }

    fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}
