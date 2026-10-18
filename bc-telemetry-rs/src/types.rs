//! The OTel-shaped value model: identifiers, span context, attributes, spans.

use bc_core::encode_hex;
use bc_date_time::Epoch;

/// A 16-byte trace identifier.
///
/// All-zero is the "unset" value the W3C spec calls invalid, which is why
/// [`TraceId::is_valid`] exists rather than callers checking bytes themselves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TraceId {
    /// The raw bytes, big-endian as they appear in a `traceparent` header.
    pub bytes: [u8; 16],
}

impl TraceId {
    /// Whether any byte is non-zero.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.bytes.iter().any(|byte| *byte != 0)
    }

    /// The lowercase hex rendering, 32 characters.
    #[must_use]
    pub fn to_hex(&self) -> String {
        encode_hex(&self.bytes)
    }
}

/// An 8-byte span identifier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SpanId {
    /// The raw bytes, big-endian as they appear in a `traceparent` header.
    pub bytes: [u8; 8],
}

impl SpanId {
    /// Whether any byte is non-zero.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.bytes.iter().any(|byte| *byte != 0)
    }

    /// The lowercase hex rendering, 16 characters.
    #[must_use]
    pub fn to_hex(&self) -> String {
        encode_hex(&self.bytes)
    }
}

/// What identifies a metric point or a log record: the producer that observed
/// it, and that producer's count.
///
/// A span is identified by its trace and span ids; a point or a log has no ids
/// of its own, and a writer that retries a batch after an unknown commit
/// outcome needs one to store each record once. The producer is drawn once per
/// pipeline from its [`crate::IdSource`], so ids from two processes collide
/// only as two trace ids would; within one producer, every nonzero sequence is
/// issued at most once.
///
/// **New surface beside the C++ trunk**, in the shape the C++ gives it
/// (`ObservationId` in `types.hpp`), so the two runtimes write one record id:
/// `<producer hex>-<sequence>`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ObservationId {
    /// The pipeline that observed the record.
    pub producer: TraceId,
    /// That pipeline's count, from one.
    pub sequence: u64,
}

impl ObservationId {
    /// Whether the producer is set and the sequence is not zero. An invalid id
    /// is one no writer can store exactly once.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.producer.is_valid() && self.sequence != 0
    }
}

/// The `sampled` bit of a span context's trace flags.
pub const TRACE_FLAG_SAMPLED: u8 = 0x01;

/// The identity a span carries across a process boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SpanContext {
    /// The trace this span belongs to.
    pub trace_id: TraceId,
    /// This span's own identifier.
    pub span_id: SpanId,
    /// W3C trace flags; see [`TRACE_FLAG_SAMPLED`].
    pub trace_flags: u8,
}

impl SpanContext {
    /// Whether the sampled bit is set.
    #[must_use]
    pub const fn is_sampled(&self) -> bool {
        (self.trace_flags & TRACE_FLAG_SAMPLED) != 0
    }
}

/// One attribute value.
///
/// A closed set, deliberately: attributes are allowlisted by shape, which is
/// what keeps secrets out of telemetry. The allowlist — ids, names, digests,
/// durations, counts, and outcome enums — is expressible in these four, and a
/// variant for arbitrary bytes would be the hole a secret eventually goes
/// through.
#[derive(Clone, Debug, PartialEq)]
pub enum AttributeValue {
    /// A Boolean.
    Boolean(bool),
    /// A signed 64-bit integer.
    Int64(i64),
    /// A 64-bit float.
    Float64(f64),
    /// A UTF-8 string.
    String(String),
}

impl From<bool> for AttributeValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<i64> for AttributeValue {
    fn from(value: i64) -> Self {
        Self::Int64(value)
    }
}

impl From<f64> for AttributeValue {
    fn from(value: f64) -> Self {
        Self::Float64(value)
    }
}

impl From<String> for AttributeValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for AttributeValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

/// An ordered list of key/value attributes.
///
/// A sequence rather than a map, matching the C++ and the OTel model: order is
/// the order they were recorded, and duplicate keys are the caller's business.
pub type Attributes = Vec<(String, AttributeValue)>;

/// What kind of operation a span represents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SpanKind {
    /// Work internal to this process.
    #[default]
    Internal = 0,
    /// Handling an inbound request.
    Server = 1,
    /// Making an outbound request.
    Client = 2,
    /// Producing onto a queue.
    Producer = 3,
    /// Consuming from a queue.
    Consumer = 4,
}

/// A span's outcome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum StatusCode {
    /// Nothing was said about the outcome.
    #[default]
    Unset = 0,
    /// The operation succeeded.
    Ok = 1,
    /// The operation failed.
    Error = 2,
}

/// A timestamped event recorded inside a span.
#[derive(Clone, Debug, PartialEq)]
pub struct SpanEvent {
    /// The event's name.
    pub name: String,
    /// When it happened.
    pub timestamp: Epoch,
    /// Attributes attached to the event.
    pub attributes: Attributes,
}

/// A finished span, as handed to an exporter.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    /// The span's name.
    pub name: String,
    /// Its identity.
    pub context: SpanContext,
    /// The enclosing span, if any.
    pub parent_span_id: Option<SpanId>,
    /// What kind of operation it represents.
    pub kind: SpanKind,
    /// Its outcome.
    pub status: StatusCode,
    /// A message accompanying a non-`Unset` status.
    pub status_message: String,
    /// When it started.
    pub start_time: Epoch,
    /// When it ended.
    pub end_time: Epoch,
    /// Attributes attached to the span.
    pub attributes: Attributes,
    /// Events recorded inside it, in order.
    pub events: Vec<SpanEvent>,
}

impl Default for Span {
    fn default() -> Self {
        Self {
            name: String::new(),
            context: SpanContext::default(),
            parent_span_id: None,
            kind: SpanKind::default(),
            status: StatusCode::default(),
            status_message: String::new(),
            start_time: Epoch::ZERO,
            end_time: Epoch::ZERO,
            attributes: Attributes::new(),
            events: Vec::new(),
        }
    }
}
