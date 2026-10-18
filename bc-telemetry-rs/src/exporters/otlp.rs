//! OTLP over HTTP, with a JSON payload.
//!
//! **New surface — the C++ original has no network exporter.** The OTLP
//! specification defines a JSON encoding alongside protobuf, and JSON is what
//! this uses: the family already owns a strict JSON writer (bc-json-rs), while
//! protobuf would mean a schema compiler, a build step, and a dependency
//! closure an order of magnitude larger for a wire format nothing here reads
//! back.
//!
//! Feature-gated (`otlp`) and off by default, so the transport dependency
//! reaches only a consumer that asks for it.
//!
//! # Failure is a drop, not an error
//!
//! Every method is infallible by signature, like the rest of the crate. An
//! unreachable collector, a refused request, a timeout — each is a counted
//! drop. That is the one behaviour a telemetry exporter must have: the process
//! it instruments does not care that the collector is down.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bc_json::{JsonReader, JsonWriter};

use crate::exporter::TelemetryExporter;
use crate::log_record::{LogRecord, Severity};
use crate::metrics::{MetricKind, MetricPoint};
use crate::types::{AttributeValue, Attributes, Span, SpanContext, SpanKind, StatusCode};

/// Where an [`OtlpExporter`] sends, and how patiently.
#[derive(Clone, Debug)]
pub struct OtlpExporterConfig {
    /// The collector's base URL, e.g. `http://127.0.0.1:4318`.
    ///
    /// The signal paths — `/v1/traces`, `/v1/metrics`, `/v1/logs` — are
    /// appended, as the OTLP/HTTP specification defines them.
    pub endpoint: String,
    /// How long to wait for a response before giving up and counting a drop.
    pub timeout: Duration,
    /// How long to keep retrying a collector that asked us to back off.
    ///
    /// Zero by default, and that is the safe answer rather than a lazy one: an
    /// export runs on the caller's own thread unless a
    /// [`BatchedExporter`](crate::BatchedExporter) is in front of it, so a
    /// retry budget here is time the instrumented work spends sleeping. A drop
    /// is already counted and visible; a stall is not. Raise it with
    /// [`with_retry_budget`](OtlpExporterConfig::with_retry_budget) for a
    /// batched pipeline, where the wait falls on the drain task instead.
    pub retry_budget: Duration,
    /// The `service.name` resource attribute every payload carries.
    pub service_name: String,
}

impl OtlpExporterConfig {
    /// A configuration pointing at `endpoint`, with a five-second timeout.
    #[must_use]
    pub fn new(endpoint: impl Into<String>, service_name: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout: Duration::from_secs(5),
            retry_budget: Duration::ZERO,
            service_name: service_name.into(),
        }
    }

    /// Spends up to `budget` retrying a collector that is asking us to wait.
    #[must_use]
    pub const fn with_retry_budget(mut self, budget: Duration) -> Self {
        self.retry_budget = budget;
        self
    }
}

/// The first back-off, doubled on each further attempt.
const RETRY_BASE: Duration = Duration::from_millis(250);

/// How long to wait before retrying, or `None` to give up now.
///
/// `retry_after` is the collector's own hint, from the header of the same name.
/// Only the delay-seconds form is honoured: the HTTP-date form would need a date
/// parser here to answer a question the exponential fallback already answers, and
/// a collector under load sends seconds.
///
/// A hint longer than the budget is a refusal, not a reason to overshoot it —
/// the point of a budget is that it is the whole of what this may spend. A hint
/// *shorter* than the base back-off is floored at it: `Retry-After: 0` is a
/// collector saying "now", and honouring it literally spun this loop as fast as
/// the collector could answer for the whole budget.
fn retry_delay(retry_after: Option<&str>, attempt: u32, remaining: Duration) -> Option<Duration> {
    if remaining.is_zero() {
        return None;
    }
    let hinted = retry_after
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .map(|hint| hint.max(RETRY_BASE));
    let delay = hinted.unwrap_or_else(|| RETRY_BASE.saturating_mul(1_u32 << attempt.min(8)));
    (delay <= remaining).then_some(delay)
}

/// How many records a collector said it rejected, from an OTLP response body.
///
/// Parsed with the family's own reader rather than scanned. The scan this
/// replaces was justified in a comment claiming a JSON reader would mean a new
/// dependency; bc-json-rs is an unconditional dependency of this crate — the
/// same one the writer above comes from — so the justification was simply
/// false, and hand-rolling a second JSON primitive next to the one already
/// linked in is how a codebase grows divergent parsers.
///
/// `key` is the signal's own count field: OTLP names them `rejectedSpans`,
/// `rejectedDataPoints`, and `rejectedLogRecords`, and a response carries only
/// the one belonging to the endpoint it answers.
///
/// An unreadable body reads as zero, which is the same answer as a body that
/// never mentioned a count. Validation is what makes the read unambiguous:
/// a duplicate key would otherwise make "the" value a question of scan order.
fn rejected_records(payload: &str, key: &str) -> u64 {
    let Ok(document) = bc_json::validate_json(payload) else {
        return 0;
    };
    let Some(partial) = JsonReader::new(document).find_object("partialSuccess") else {
        return 0;
    };
    // proto3 renders a uint64 as a string, but collectors emit both forms.
    partial
        .integer(key)
        .and_then(|count| u64::try_from(count).ok())
        .or_else(|| partial.string(key).and_then(|count| count.parse().ok()))
        .unwrap_or(0)
}

/// Ships telemetry to an OTLP/HTTP collector.
#[derive(Debug)]
pub struct OtlpExporter {
    config: OtlpExporterConfig,
    /// Built once and reused. `ureq::post` constructs a fresh `Agent` per call,
    /// and an agent owns the connection pool — so every export was paying a DNS
    /// lookup, a TCP handshake, and for an `https` endpoint a full TLS
    /// handshake, on a path that runs once per batch.
    agent: ureq::Agent,
    dropped: AtomicU64,
}

impl OtlpExporter {
    /// Builds an exporter. Nothing is contacted until something is exported.
    #[must_use]
    pub fn new(config: OtlpExporterConfig) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(config.timeout))
            // A status is data here, not an error: 429 and 503 mean "ask again",
            // which is a different outcome from 400 "this payload is wrong", and
            // the default of collapsing every non-2xx into one `Err` cannot tell
            // them apart.
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            config,
            agent,
            dropped: AtomicU64::new(0),
        }
    }

    /// Posts `body` to `path`, counting `records` as dropped on any failure.
    ///
    /// A 2xx is not the end of the question. OTLP/HTTP answers a partially
    /// accepted export with **200 OK** plus a `partialSuccess` object naming how
    /// many records it rejected, so reading only the status code reports a
    /// clean export while the collector threw records away — precisely the
    /// hidden drop this crate forbids.
    ///
    /// 429 and 503 are the two statuses that mean "later", and the only ones
    /// retried; everything else non-2xx is a drop this export cannot argue with.
    fn post(&self, path: &str, body: &str, rejected_key: &str, records: usize) {
        let url = format!("{}{path}", self.config.endpoint.trim_end_matches('/'));
        let deadline = Instant::now() + self.config.retry_budget;
        let mut attempt = 0_u32;

        loop {
            let outcome = self
                .agent
                .post(&url)
                .header("content-type", "application/json")
                .send(body);
            let Ok(mut response) = outcome else {
                // A transport failure, not a status: nothing was acknowledged.
                self.dropped.fetch_add(records as u64, Ordering::Relaxed);
                return;
            };

            let status = response.status().as_u16();
            if matches!(status, 429 | 503) {
                // The collector is up and asking for time. Retrying is the only
                // way these records survive, and the budget is what stops that
                // from becoming an unbounded stall in the caller's work.
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let remaining = deadline.saturating_duration_since(Instant::now());
                if let Some(delay) = retry_delay(retry_after.as_deref(), attempt, remaining) {
                    std::thread::sleep(delay);
                    attempt += 1;
                    continue;
                }
                self.dropped.fetch_add(records as u64, Ordering::Relaxed);
                return;
            }
            if !(200..300).contains(&status) {
                // A refusal this export cannot fix by asking again.
                self.dropped.fetch_add(records as u64, Ordering::Relaxed);
                return;
            }

            let Ok(payload) = response.body_mut().read_to_string() else {
                // Delivered, but the acknowledgement is unreadable, so whether
                // the collector kept the records is unknown. Unknown is not
                // "fine".
                self.dropped.fetch_add(records as u64, Ordering::Relaxed);
                return;
            };
            let rejected = rejected_records(&payload, rejected_key);
            if rejected > 0 {
                self.dropped.fetch_add(rejected, Ordering::Relaxed);
            }
            return;
        }
    }

    fn write_resource(&self, json: &mut JsonWriter) {
        json.nested_object("resource", |json| {
            json.array("attributes", |json| {
                super::object_element(json, |json| {
                    json.field("key", "service.name");
                    json.nested_object("value", |json| {
                        json.field("stringValue", self.config.service_name.as_str());
                    });
                });
            });
        });
    }
}

/// OTLP renders a 64-bit time as a decimal **string**, not a number, because
/// JSON numbers cannot carry the full range exactly.
fn nanos(millis: i64) -> String {
    (i128::from(millis) * 1_000_000).to_string()
}

fn write_attributes(json: &mut JsonWriter, attributes: &Attributes) {
    json.array("attributes", |json| {
        for (key, value) in attributes {
            super::object_element(json, |json| {
                json.field("key", key.as_str());
                json.nested_object("value", |json| match value {
                    AttributeValue::Boolean(value) => json.field("boolValue", *value),
                    // Also a string, for the reason above.
                    AttributeValue::Int64(value) => {
                        json.field("intValue", value.to_string().as_str());
                    }
                    AttributeValue::Float64(value) => json.field("doubleValue", *value),
                    AttributeValue::String(value) => json.field("stringValue", value.as_str()),
                });
            });
        }
    });
}

fn write_span_ids(json: &mut JsonWriter, context: Option<&SpanContext>) {
    if let Some(context) = context {
        json.field("traceId", context.trace_id.to_hex().as_str());
        json.field("spanId", context.span_id.to_hex().as_str());
    }
}

/// OTLP's `SpanKind` is 1-based with an `UNSPECIFIED` at zero, so every value
/// is this crate's plus one. Written out rather than computed: the numbers are
/// a wire contract, and `as u8 + 1` would hide that behind arithmetic.
const fn otlp_span_kind(kind: SpanKind) -> u8 {
    match kind {
        SpanKind::Internal => 1,
        SpanKind::Server => 2,
        SpanKind::Client => 3,
        SpanKind::Producer => 4,
        SpanKind::Consumer => 5,
    }
}

/// OTLP severity numbers, from the specification's table.
const fn otlp_severity_number(severity: Severity) -> u8 {
    match severity {
        Severity::Debug => 5,
        Severity::Info => 9,
        Severity::Warn => 13,
        Severity::Error => 17,
        Severity::Fatal => 21,
    }
}

const fn otlp_status_code(status: StatusCode) -> u8 {
    match status {
        StatusCode::Unset => 0,
        StatusCode::Ok => 1,
        StatusCode::Error => 2,
    }
}

/// The `scope` block every signal repeats.
fn write_scope(json: &mut JsonWriter) {
    json.nested_object("scope", |json| {
        json.field("name", "bc-telemetry-rs");
        json.field("version", env!("CARGO_PKG_VERSION"));
    });
}

/// One span, in OTLP's shape.
///
/// A free function rather than five levels of closure inside `export_spans`:
/// the envelope is deep enough on its own, and a reader looking for what a span
/// serialises to should not have to find it inside one.
fn write_span(json: &mut JsonWriter, span: &Span) {
    write_span_ids(json, Some(&span.context));
    if let Some(parent) = span.parent_span_id {
        json.field("parentSpanId", parent.to_hex().as_str());
    }
    json.field("name", span.name.as_str());
    json.field("kind", otlp_span_kind(span.kind));
    json.field(
        "startTimeUnixNano",
        nanos(span.start_time.millis()).as_str(),
    );
    json.field("endTimeUnixNano", nanos(span.end_time.millis()).as_str());
    write_attributes(json, &span.attributes);
    json.array("events", |json| {
        for event in &span.events {
            super::object_element(json, |json| {
                json.field("name", event.name.as_str());
                json.field("timeUnixNano", nanos(event.timestamp.millis()).as_str());
                write_attributes(json, &event.attributes);
            });
        }
    });
    json.nested_object("status", |json| {
        json.field("code", otlp_status_code(span.status));
        if !span.status_message.is_empty() {
            json.field("message", span.status_message.as_str());
        }
    });
}

/// One log record, in OTLP's shape.
fn write_log(json: &mut JsonWriter, log: &LogRecord) {
    json.field("timeUnixNano", nanos(log.timestamp.millis()).as_str());
    json.field("severityNumber", otlp_severity_number(log.severity));
    json.field("severityText", log.severity.label());
    json.nested_object("body", |json| {
        json.field("stringValue", log.body.as_str());
    });
    write_attributes(json, &log.attributes);
    write_span_ids(json, log.span_context.as_ref());
}

impl OtlpExporter {
    /// Builds one OTLP envelope: `resource<Signal>` → `scope<Signal>` → items.
    ///
    /// The three signals differ only in three key names and what each item
    /// writes, so the envelope is built once here rather than three times.
    fn envelope<T>(
        &self,
        resource_key: &str,
        scope_key: &str,
        items_key: &str,
        items: &[T],
        write_item: impl Fn(&mut JsonWriter, &T),
    ) -> Result<String, bc_json::JsonWriteError> {
        let mut json = JsonWriter::new();
        json.object(|json| {
            json.array(resource_key, |json| {
                super::object_element(json, |json| {
                    self.write_resource(json);
                    json.array(scope_key, |json| {
                        super::object_element(json, |json| {
                            write_scope(json);
                            json.array(items_key, |json| {
                                for item in items {
                                    super::object_element(json, |json| write_item(json, item));
                                }
                            });
                        });
                    });
                });
            });
        });
        json.finish()
    }
}

impl TelemetryExporter for OtlpExporter {
    fn export_spans(&self, spans: &[Span]) {
        if spans.is_empty() {
            return;
        }
        match self.envelope("resourceSpans", "scopeSpans", "spans", spans, write_span) {
            Ok(body) => self.post("/v1/traces", &body, "rejectedSpans", spans.len()),
            Err(_) => {
                self.dropped
                    .fetch_add(spans.len() as u64, Ordering::Relaxed);
            }
        }
    }

    fn export_metrics(&self, metrics: &[MetricPoint]) {
        if metrics.is_empty() {
            return;
        }
        // A point this exporter cannot express is counted here and left out of
        // the payload, rather than sent as the zero it is not.
        let (writable, skipped) = measurable(metrics);
        if skipped > 0 {
            self.dropped.fetch_add(skipped as u64, Ordering::Relaxed);
        }
        if writable.is_empty() {
            return;
        }
        match self.envelope(
            "resourceMetrics",
            "scopeMetrics",
            "metrics",
            &writable,
            |json, (point, value)| write_metric(json, point, *value),
        ) {
            Ok(body) => self.post("/v1/metrics", &body, "rejectedDataPoints", writable.len()),
            Err(_) => {
                self.dropped
                    .fetch_add(writable.len() as u64, Ordering::Relaxed);
            }
        }
    }

    fn export_logs(&self, logs: &[LogRecord]) {
        if logs.is_empty() {
            return;
        }
        match self.envelope("resourceLogs", "scopeLogs", "logRecords", logs, write_log) {
            Ok(body) => self.post("/v1/logs", &body, "rejectedLogRecords", logs.len()),
            Err(_) => {
                self.dropped.fetch_add(logs.len() as u64, Ordering::Relaxed);
            }
        }
    }

    /// Nothing is buffered: each batch is posted as it arrives, so there is
    /// nothing left to push. Batching belongs to the `batched` feature, which
    /// wraps this rather than duplicating it.
    fn flush(&self) {}

    fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}
/// Splits a batch into the points this exporter can express and a count of
/// those it cannot.
///
/// Each writable point carries its value already resolved, so nothing
/// downstream re-derives it and no `unwrap_or(0.0)` exists to fall back to.
fn measurable(metrics: &[MetricPoint]) -> (Vec<(&MetricPoint, f64)>, usize) {
    let writable: Vec<(&MetricPoint, f64)> = metrics
        .iter()
        .filter_map(|point| super::numeric_of(&point.value).map(|value| (point, value)))
        .collect();
    let skipped = metrics.len() - writable.len();
    (writable, skipped)
}

/// One metric point, in OTLP's shape for its kind.
fn write_metric(json: &mut JsonWriter, point: &MetricPoint, value: f64) {
    json.field("name", point.name.as_str());
    let timestamp = nanos(point.timestamp.millis());

    match point.kind {
        MetricKind::Counter => json.nested_object("sum", |json| {
            json.field("aggregationTemporality", 2); // DELTA
            json.field("isMonotonic", true);
            write_number_data_point(json, point, &timestamp, value);
        }),
        MetricKind::Gauge => json.nested_object("gauge", |json| {
            write_number_data_point(json, point, &timestamp, value);
        }),
        // A single observation, not an aggregate: this crate records points
        // rather than summarising them, so the histogram carries one count and
        // no bucket bounds. A collector aggregates; inventing buckets here
        // would be reporting a distribution nobody measured.
        MetricKind::Histogram => json.nested_object("histogram", |json| {
            json.field("aggregationTemporality", 2);
            json.array("dataPoints", |json| {
                super::object_element(json, |json| {
                    json.field("startTimeUnixNano", timestamp.as_str());
                    json.field("timeUnixNano", timestamp.as_str());
                    json.field("count", 1);
                    json.field("sum", value);
                    json.array("bucketCounts", |json| json.array_element(1));
                    json.array("explicitBounds", |_| {});
                    write_attributes(json, &point.attributes);
                });
            });
        }),
    }
}

fn write_number_data_point(
    json: &mut JsonWriter,
    point: &MetricPoint,
    timestamp: &str,
    value: f64,
) {
    json.array("dataPoints", |json| {
        super::object_element(json, |json| {
            json.field("startTimeUnixNano", timestamp);
            json.field("timeUnixNano", timestamp);
            match &point.value {
                // An integer travels exactly, as OTLP's decimal string.
                AttributeValue::Int64(exact) => json.field("asInt", exact.to_string().as_str()),
                _ => json.field("asDouble", value),
            }
            write_attributes(json, &point.attributes);
        });
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        OtlpExporter, OtlpExporterConfig, RETRY_BASE, measurable, rejected_records, retry_delay,
        write_metric,
    };
    use crate::metrics::{MetricKind, MetricPoint};
    use crate::types::{AttributeValue, Attributes, ObservationId};

    /// The traces endpoint's count field, which most of these exercise.
    const SPANS: &str = "rejectedSpans";

    fn gauge(name: &str, value: AttributeValue) -> MetricPoint {
        MetricPoint {
            name: name.to_owned(),
            kind: MetricKind::Gauge,
            value,
            attributes: Attributes::new(),
            timestamp: bc_date_time::Epoch::from_millis(0),
            span_context: None,
            observation_id: ObservationId::default(),
        }
    }

    #[test]
    fn a_string_valued_metric_is_skipped_rather_than_sent_as_a_fabricated_zero() {
        // `asDouble: 0.0` is a data point nothing measured, and a reader cannot
        // tell it from a real zero. Dropping it is permitted and hiding it is
        // not, so it is counted and left out of the payload — which is what the
        // Postgres exporter was already corrected to do with the same value.
        let exporter = OtlpExporter::new(OtlpExporterConfig::new("http://127.0.0.1:1", "test"));
        let points = [
            gauge("real", AttributeValue::Float64(1.5)),
            gauge("bogus", AttributeValue::String("n/a".to_owned())),
        ];

        let (writable, skipped) = measurable(&points);
        assert_eq!(skipped, 1, "the point it cannot send must still be counted");

        let body = exporter
            .envelope(
                "resourceMetrics",
                "scopeMetrics",
                "metrics",
                &writable,
                |json, (point, value)| write_metric(json, point, *value),
            )
            .expect("the envelope is writable");

        assert!(body.contains("\"real\""), "the measured point must be sent");
        assert!(
            !body.contains("\"bogus\""),
            "a string-valued point reached the collector as a fabricated number"
        );
    }

    #[test]
    fn no_budget_means_no_retry() {
        // The default. A drop is counted and visible; a stall in the caller's
        // work is not, so nothing is spent without someone asking for it.
        assert_eq!(retry_delay(Some("1"), 0, Duration::ZERO), None);
    }

    #[test]
    fn the_collectors_hint_wins_when_it_fits_the_budget() {
        assert_eq!(
            retry_delay(Some("2"), 0, Duration::from_secs(5)),
            Some(Duration::from_secs(2))
        );
        // Whitespace is allowed around a header value.
        assert_eq!(
            retry_delay(Some(" 3 "), 4, Duration::from_secs(5)),
            Some(Duration::from_secs(3))
        );
    }

    #[test]
    fn a_hint_of_zero_still_backs_off() {
        // `Retry-After: 0` parsed to no delay at all, passed the budget check,
        // and spun the retry loop as fast as the collector could answer for the
        // whole budget. A collector asking for "now" gets the base back-off.
        assert_eq!(
            retry_delay(Some("0"), 0, Duration::from_secs(5)),
            Some(RETRY_BASE)
        );
    }

    #[test]
    fn a_hint_longer_than_the_budget_gives_up_rather_than_overshooting() {
        // The budget is the whole of what an export may spend waiting; honouring
        // a 60-second hint inside a 5-second budget would make it advisory.
        assert_eq!(retry_delay(Some("60"), 0, Duration::from_secs(5)), None);
    }

    #[test]
    fn without_a_usable_hint_the_wait_backs_off_exponentially() {
        // An HTTP-date hint lands here too: unparsed, so the fallback answers.
        let budget = Duration::from_secs(60);
        assert_eq!(retry_delay(None, 0, budget), Some(RETRY_BASE));
        assert_eq!(retry_delay(None, 1, budget), Some(RETRY_BASE * 2));
        assert_eq!(retry_delay(None, 3, budget), Some(RETRY_BASE * 8));
        assert_eq!(
            retry_delay(Some("Wed, 21 Oct 2026 07:28:00 GMT"), 0, budget),
            Some(RETRY_BASE)
        );
    }

    #[test]
    fn the_back_off_stops_doubling_before_it_can_overflow() {
        // `1 << attempt` is undefined past the word size; the shift is clamped,
        // and the budget check is what actually ends the loop.
        assert_eq!(retry_delay(None, 99, Duration::from_secs(60)), None);
        assert_eq!(
            retry_delay(None, 99, Duration::from_secs(600)),
            Some(RETRY_BASE * 256)
        );
    }

    #[test]
    fn a_body_with_no_partial_success_rejects_nothing() {
        assert_eq!(rejected_records("{}", SPANS), 0);
        assert_eq!(rejected_records(r#"{"partialSuccess":{}}"#, SPANS), 0);
    }

    #[test]
    fn a_rejected_count_is_read_whether_quoted_or_bare() {
        // proto3 JSON renders a uint64 as a string; collectors vary.
        assert_eq!(
            rejected_records(r#"{"partialSuccess":{"rejectedSpans":"2"}}"#, SPANS),
            2
        );
        assert_eq!(
            rejected_records(r#"{"partialSuccess":{"rejectedSpans":2}}"#, SPANS),
            2
        );
    }

    #[test]
    fn every_rejected_key_in_the_schema_counts() {
        assert_eq!(
            rejected_records(
                r#"{"partialSuccess":{"rejectedDataPoints":"3"}}"#,
                "rejectedDataPoints"
            ),
            3
        );
        assert_eq!(
            rejected_records(
                r#"{"partialSuccess":{"rejectedLogRecords":"4"}}"#,
                "rejectedLogRecords"
            ),
            4
        );
    }

    #[test]
    fn a_count_quoted_inside_an_error_message_is_not_counted() {
        // The control case for parsing rather than scanning. The scan this
        // replaced looked for the substring `"rejected` anywhere in the body,
        // so a collector that named the field in its own prose was read as
        // having rejected that many records. Parsing sees a string value.
        assert_eq!(
            rejected_records(
                r#"{"partialSuccess":{"errorMessage":"\"rejectedSpans\": 99 exceeded"}}"#,
                SPANS
            ),
            0
        );
    }

    #[test]
    fn an_unparseable_body_reports_no_rejections() {
        // Same answer as a body that never mentioned a count. The delivery
        // itself is judged by the status code, not by this.
        assert_eq!(rejected_records("not json at all", SPANS), 0);
        assert_eq!(rejected_records(r#"{"partialSuccess":"#, SPANS), 0);
    }

    #[test]
    fn an_error_message_alongside_the_count_does_not_confuse_it() {
        assert_eq!(
            rejected_records(
                r#"{"partialSuccess":{"rejectedSpans":"2","errorMessage":"attribute limit"}}"#,
                SPANS
            ),
            2
        );
    }
}
