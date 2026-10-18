//! The OTLP/HTTP exporter.
//!
//! New surface — no ported cases. The payload is checked against a real
//! listener rather than by inspecting a string the exporter never sent: the
//! whole risk in this exporter is that what it builds and what it posts differ.
#![cfg(feature = "otlp")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use bc_date_time::Epoch;
use bc_json::validate_json;
use bc_telemetry::{
    AttributeValue, LogRecord, MetricKind, MetricPoint, ObservationId, OtlpExporter,
    OtlpExporterConfig, Severity, Span, SpanKind, StatusCode, TelemetryExporter,
};

/// Accepts exactly one request and returns its body.
///
/// The listener is bound *before* the thread starts, so the port is live the
/// moment this returns — that is the readiness signal, not a sleep.
fn one_request() -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("a bound address").port();
    let handle = thread::spawn(move || {
        // Bounded rather than blocking forever. `TcpListener::accept` has no
        // timeout of its own, so poll it against a deadline: if the exporter
        // never connects — a sandbox blocking loopback, an address-family
        // mismatch — this test fails *itself* in seconds instead of wedging the
        // whole run until an outer kill.
        listener
            .set_nonblocking(true)
            .expect("a nonblocking listener");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the exporter never connected"
                    );
                    thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        };
        stream.set_nonblocking(false).expect("a blocking stream");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .expect("a read timeout");
        let mut raw = Vec::new();
        let mut buffer = [0_u8; 4096];
        // Read until the body is complete: headers, then Content-Length bytes.
        loop {
            let read = stream.read(&mut buffer).expect("a readable stream");
            if read == 0 {
                break;
            }
            raw.extend_from_slice(&buffer[..read]);
            let text = String::from_utf8_lossy(&raw).to_string();
            if let Some(headers_end) = text.find("\r\n\r\n") {
                let declared = text
                    .to_ascii_lowercase()
                    .split("content-length:")
                    .nth(1)
                    .and_then(|rest| rest.split("\r\n").next())
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if text.len() - (headers_end + 4) >= declared {
                    break;
                }
            }
        }
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let text = String::from_utf8_lossy(&raw).to_string();
        text.split_once("\r\n\r\n")
            .map(|(_, body)| body.to_owned())
            .unwrap_or_default()
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

fn exporter_for(endpoint: &str) -> OtlpExporter {
    OtlpExporter::new(OtlpExporterConfig::new(endpoint, "example-service"))
}

#[test]
fn a_span_is_posted_as_valid_otlp_json() {
    let (endpoint, server) = one_request();
    let exporter = exporter_for(&endpoint);

    exporter.export_spans(&[Span {
        name: "fetch".to_owned(),
        kind: SpanKind::Server,
        status: StatusCode::Error,
        status_message: "upstream refused".to_owned(),
        start_time: Epoch::from_millis(1_000),
        end_time: Epoch::from_millis(1_250),
        attributes: vec![("host".to_owned(), AttributeValue::from("example-host"))],
        ..Span::default()
    }]);

    let body = server.join().expect("the listener thread finished");
    assert!(validate_json(&body).is_ok(), "not valid JSON: {body}");
    assert!(body.contains("\"resourceSpans\""), "{body}");
    assert!(body.contains("\"name\":\"fetch\""), "{body}");
    // OTLP's SpanKind is 1-based: Server is 2, not this crate's 1.
    assert!(body.contains("\"kind\":2"), "{body}");
    // Times are decimal strings in nanoseconds, not JSON numbers.
    assert!(
        body.contains("\"startTimeUnixNano\":\"1000000000\""),
        "{body}"
    );
    assert!(
        body.contains("\"endTimeUnixNano\":\"1250000000\""),
        "{body}"
    );
    assert!(body.contains("\"code\":2"), "{body}");
    assert!(body.contains("\"service.name\""), "{body}");
    assert_eq!(exporter.dropped(), 0);
}

#[test]
fn a_log_is_posted_with_its_otlp_severity_number() {
    let (endpoint, server) = one_request();
    let exporter = exporter_for(&endpoint);

    exporter.export_logs(&[LogRecord {
        timestamp: Epoch::from_millis(2_000),
        severity: Severity::Error,
        body: "it broke".to_owned(),
        attributes: Vec::new(),
        span_context: None,
        observation_id: ObservationId::default(),
    }]);

    let body = server.join().expect("the listener thread finished");
    assert!(validate_json(&body).is_ok(), "{body}");
    assert!(body.contains("\"resourceLogs\""), "{body}");
    // ERROR is 17 in the specification's table.
    assert!(body.contains("\"severityNumber\":17"), "{body}");
    assert!(body.contains("\"stringValue\":\"it broke\""), "{body}");
}

#[test]
fn a_counter_is_posted_as_a_monotonic_sum_with_an_exact_integer() {
    let (endpoint, server) = one_request();
    let exporter = exporter_for(&endpoint);

    exporter.export_metrics(&[MetricPoint {
        name: "requests".to_owned(),
        kind: MetricKind::Counter,
        value: AttributeValue::Int64(9_007_199_254_740_993), // 2^53 + 1
        attributes: Vec::new(),
        timestamp: Epoch::from_millis(3_000),
        span_context: None,
        observation_id: ObservationId::default(),
    }]);

    let body = server.join().expect("the listener thread finished");
    assert!(validate_json(&body).is_ok(), "{body}");
    assert!(body.contains("\"isMonotonic\":true"), "{body}");
    // The reason counters go as a string: a double would round this value.
    assert!(body.contains("\"asInt\":\"9007199254740993\""), "{body}");
}

#[test]
fn an_unreachable_collector_is_a_counted_drop_rather_than_a_failure() {
    // The contract that matters most: a process does not stop working because
    // its collector is down. Port 1 on loopback refuses immediately.
    let exporter = exporter_for("http://127.0.0.1:1");

    exporter.export_spans(&[Span::default(), Span::default()]);
    exporter.export_logs(&[LogRecord {
        timestamp: Epoch::ZERO,
        severity: Severity::Info,
        body: String::new(),
        attributes: Vec::new(),
        span_context: None,
        observation_id: ObservationId::default(),
    }]);

    assert_eq!(exporter.dropped(), 3);
}

#[test]
fn an_empty_batch_contacts_nothing() {
    // No listener at all: if this posted, it would have to fail, and the drop
    // counter would move.
    let exporter = exporter_for("http://127.0.0.1:1");
    exporter.export_spans(&[]);
    exporter.export_metrics(&[]);
    exporter.export_logs(&[]);
    assert_eq!(exporter.dropped(), 0);
}
