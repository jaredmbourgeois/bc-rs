//! The line-delimited JSON file exporter.
//!
//! New surface — the C++ original has no file exporter — so none of these is a
//! ported case. The lifecycle matters as much as the format, which is why
//! rotation and the drop counter are tested here rather than left to
//! inspection.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use bc_date_time::Epoch;
use bc_json::validate_json;
use bc_telemetry::{
    AttributeValue, FileExporter, FileExporterConfig, LINE_SCHEMA_VERSION, LogRecord, MetricKind,
    MetricPoint, ObservationId, Severity, Span, SpanKind, TelemetryExporter,
};

/// A directory unique to this process and this case, removed on drop.
///
/// A fixed shared name would collide across parallel test processes, across
/// runs, and across users on a shared temp dir — invisibly, on a clean
/// single-user box.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = format!(
            "bc-telemetry-rs-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path).expect("a fresh temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn a_span(name: &str) -> Span {
    Span {
        name: name.to_owned(),
        start_time: Epoch::from_millis(1_000),
        end_time: Epoch::from_millis(1_250),
        kind: SpanKind::Server,
        attributes: vec![("host".to_owned(), AttributeValue::from("example-host"))],
        events: vec![bc_telemetry::SpanEvent {
            name: "cache.miss".to_owned(),
            timestamp: Epoch::from_millis(1_100),
            attributes: vec![("key".to_owned(), AttributeValue::from("orders"))],
        }],
        ..Span::default()
    }
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_written_line_is_valid_json_carrying_its_schema_version() {
    let dir = TempDir::new();
    let path = dir.path().join("telemetry.ldjson");
    let exporter = FileExporter::new(FileExporterConfig::new(&path));

    exporter.export_spans(&[a_span("fetch")]);
    exporter.export_metrics(&[MetricPoint {
        name: "hits".to_owned(),
        kind: MetricKind::Counter,
        value: AttributeValue::Int64(3),
        attributes: Vec::new(),
        timestamp: Epoch::from_millis(1_000),
        span_context: None,
        observation_id: ObservationId::default(),
    }]);
    exporter.export_logs(&[LogRecord {
        timestamp: Epoch::from_millis(1_000),
        severity: Severity::Warn,
        body: "disk is filling".to_owned(),
        attributes: Vec::new(),
        span_context: None,
        observation_id: ObservationId::default(),
    }]);
    exporter.flush();

    let written = lines(&path);
    assert_eq!(written.len(), 3);
    for line in &written {
        // Checked with this family's own validator rather than by eye: the file
        // is a versioned surface, and "it looked like JSON" is not the claim.
        assert!(validate_json(line).is_ok(), "not valid JSON: {line}");
        assert!(
            line.contains(&format!("\"schema_version\":{LINE_SCHEMA_VERSION}")),
            "no schema version: {line}"
        );
    }
    assert!(written[0].contains("\"kind\":\"span\""));
    assert!(written[0].contains("\"duration_ms\":250"));
    // An event is an object inside an array — the shape that was silently
    // producing invalid JSON until the OTLP round trip exposed it.
    assert!(written[0].contains("\"cache.miss\""), "{}", written[0]);
    assert!(written[1].contains("\"kind\":\"metric\""));
    assert!(written[2].contains("\"kind\":\"log\""));
}

#[test]
fn a_body_needing_escapes_stays_valid_json() {
    // The reason this exporter writes through bc-json-rs instead of formatting
    // strings: a quote, a backslash, a newline, and a control character in a log
    // body must not be able to break the line format.
    let dir = TempDir::new();
    let path = dir.path().join("telemetry.ldjson");
    let exporter = FileExporter::new(FileExporterConfig::new(&path));

    exporter.export_logs(&[LogRecord {
        timestamp: Epoch::ZERO,
        severity: Severity::Error,
        body: "he said \"hi\"\\ then\n\u{7} left".to_owned(),
        attributes: Vec::new(),
        span_context: None,
        observation_id: ObservationId::default(),
    }]);

    let written = lines(&path);
    assert_eq!(
        written.len(),
        1,
        "an embedded newline did not split the line"
    );
    assert!(validate_json(&written[0]).is_ok(), "{}", written[0]);
}

#[test]
fn the_live_file_rotates_at_the_size_cap() {
    let dir = TempDir::new();
    let path = dir.path().join("telemetry.ldjson");
    let exporter = FileExporter::new(FileExporterConfig {
        max_bytes: 400,
        max_rotated: 3,
        drop_report_interval: 0,
        ..FileExporterConfig::new(&path)
    });

    for index in 0..20 {
        exporter.export_spans(&[a_span(&format!("span-{index}"))]);
    }
    exporter.flush();

    let rotated_one = dir.path().join("telemetry.ldjson.1");
    assert!(path.exists(), "the live file was reopened");
    assert!(rotated_one.exists(), "the first rotation happened");
    // Rename-and-reopen, not truncate: the rotated file keeps its content.
    assert!(
        !lines(&rotated_one).is_empty(),
        "the rotated file keeps its content"
    );
    for line in lines(&rotated_one) {
        assert!(
            validate_json(&line).is_ok(),
            "rotation split a line: {line}"
        );
    }
}

#[test]
fn rotated_files_are_bounded_by_the_configured_count() {
    let dir = TempDir::new();
    let path = dir.path().join("telemetry.ldjson");
    let exporter = FileExporter::new(FileExporterConfig {
        max_bytes: 200,
        max_rotated: 2,
        drop_report_interval: 0,
        ..FileExporterConfig::new(&path)
    });

    for index in 0..40 {
        exporter.export_spans(&[a_span(&format!("span-{index}"))]);
    }
    exporter.flush();

    assert!(dir.path().join("telemetry.ldjson.1").exists());
    assert!(dir.path().join("telemetry.ldjson.2").exists());
    assert!(
        !dir.path().join("telemetry.ldjson.3").exists(),
        "the live-file cap alone would grow the directory forever"
    );
}

#[test]
fn a_write_that_cannot_land_is_counted_rather_than_raised() {
    // The path is a *directory*, so opening it as a file fails on every
    // attempt. What the crate promises is that this neither panics nor returns
    // an error — it drops, and says how many.
    let dir = TempDir::new();
    let blocked = dir.path().join("occupied");
    std::fs::create_dir_all(&blocked).expect("a directory in the way");

    let exporter = FileExporter::new(FileExporterConfig::new(&blocked));
    exporter.export_spans(&[a_span("one")]);
    exporter.export_spans(&[a_span("two")]);
    exporter.flush();

    assert_eq!(exporter.dropped(), 2);
}

#[test]
fn the_drop_count_is_emitted_into_the_file_a_reader_tails() {
    // The half that matters to a reader tailing the file: a gap it can see
    // beats a seamless lie. Arranged so the exporter genuinely drops first and
    // then recovers — a directory in the way, removed once the drops are
    // counted — because a case that never reaches the emission path would pass
    // without proving it exists.
    let dir = TempDir::new();
    let path = dir.path().join("telemetry.ldjson");
    std::fs::create_dir_all(&path).expect("a directory standing where the file goes");

    let exporter = FileExporter::new(FileExporterConfig {
        max_bytes: u64::MAX,
        max_rotated: 0,
        drop_report_interval: 2,
        ..FileExporterConfig::new(&path)
    });

    exporter.export_spans(&[a_span("lost-one")]);
    exporter.export_spans(&[a_span("lost-two")]);
    assert_eq!(exporter.dropped(), 2, "the blocked path dropped both");

    // Clear the obstruction; the next write re-opens rather than staying dead.
    std::fs::remove_dir(&path).expect("the obstruction is removable");
    exporter.export_spans(&[a_span("kept-one")]);
    exporter.export_spans(&[a_span("kept-two")]);
    exporter.flush();

    let written = lines(&path);
    let drop_records: Vec<&String> = written
        .iter()
        .filter(|line| line.contains("\"kind\":\"drops\""))
        .collect();
    assert_eq!(drop_records.len(), 1, "one drop record, in {written:?}");
    assert!(validate_json(drop_records[0]).is_ok());
    assert!(
        drop_records[0].contains("\"dropped\":2"),
        "the record names the count: {}",
        drop_records[0]
    );
    assert_eq!(
        exporter.dropped(),
        2,
        "reporting does not reset the counter"
    );
}

#[test]
fn duplicate_attribute_keys_survive_as_valid_json() {
    // `Attributes` is a `Vec` whose contract permits duplicate keys, and
    // `set_span_attribute` appends without de-duplicating. Rendering them as a
    // JSON object keyed by attribute name poisoned the writer with
    // `DuplicateKey` in a debug build — losing the entire span line — and
    // emitted JSON this crate's own validator rejects in a release build.
    let directory = TempDir::new();
    let path = directory.path().join("telemetry.ldjson");
    let exporter = FileExporter::new(FileExporterConfig::new(path.clone()));

    let span = Span {
        name: "retrying".to_owned(),
        attributes: vec![
            ("retry".to_owned(), AttributeValue::Int64(1)),
            ("retry".to_owned(), AttributeValue::Int64(2)),
        ],
        ..Span::default()
    };
    exporter.export_spans(&[span]);
    exporter.flush();

    let written = std::fs::read_to_string(&path).expect("the exporter wrote a file");
    let lines: Vec<&str> = written.lines().filter(|line| !line.is_empty()).collect();
    // The record survived at all — under the object rendering it was dropped.
    assert_eq!(lines.len(), 1, "the span line must not be dropped");
    assert!(validate_json(lines[0]).is_ok(), "line must be valid JSON");
    // Both attributes are present, in the order the caller appended them.
    assert!(lines[0].contains("\"key\":\"retry\",\"value\":1"));
    assert!(lines[0].contains("\"key\":\"retry\",\"value\":2"));
    assert_eq!(exporter.dropped(), 0);
}
