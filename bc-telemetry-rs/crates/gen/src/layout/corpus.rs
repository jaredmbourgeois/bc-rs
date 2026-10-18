//! The conformance corpus: fixture records, the rows each must become, and the
//! counts the ingest step must return.
//!
//! Every runtime writes the layout from its own exporter, and what holds them to
//! one behaviour is this file's output: a writer's suite exports the corpus's
//! records into a real PostgreSQL and compares what landed. The corpus is
//! derived from the tree, so it holds a record of every declared item — which is
//! also what lets a dashboard's every panel be run against it in a real Grafana
//! and expected to return something.
//!
//! This module is the layout's reference writer, in three steps a reader can
//! check one at a time:
//!
//! 1. [`fixtures`] builds the records, as a runtime carries them.
//! 2. [`stage`] turns one record into the row a writer stages — the lifting of
//!    identity, tenancy and the module's version out of the attributes — or
//!    refuses it.
//! 3. [`Store::ingest`] applies the ingest step's rules to staged rows.
//!
//! The SQL that does step 3 for real is generated beside the corpus; a test
//! runs both and compares, so neither is its own oracle.

use std::collections::{BTreeMap, BTreeSet};

use bc_date_time::{DateFormatBuilder, DateFormatter, Epoch};
use bc_json::{JsonWriteError, JsonWriter};
use bc_telemetry::MetricKind;

use crate::layout::LAYOUT_VERSION;
use crate::layout::views::{self, ViewShape, quoted_identifier};
use crate::model::{ErrorClass, ItemKind, Keyword, Module, ScalarType, Tree};

/// The instant the corpus's maintenance runs at: 2026-01-15T12:00:00Z. Every
/// record's time is fixed relative to it, so the corpus is the same bytes on
/// every run.
const INSTANT_MS: i64 = 1_768_478_400_000;
const DAYS_AHEAD: i64 = 1;
const DAYS_KEPT: i64 = 2;
const MILLISECONDS_PER_DAY: i64 = 86_400_000;
const MILLISECONDS_PER_MINUTE: i64 = 60_000;

/// The service a writer under test is configured as, for a record that names
/// none.
const SERVICE_NAME: &str = "corpus";
const ORGANIZATION: &str = "corpus-org";
const KEPT_CUSTOMER: &str = "customer-kept";
const ERASED_CUSTOMER: &str = "customer-erased";
/// When the erased customer was erased: an hour before the instant.
const ERASED_AT_MS: i64 = INSTANT_MS - 60 * MILLISECONDS_PER_MINUTE;

/// The producer every metric and log record of the corpus is observed by.
const PRODUCER: &str = "0b5e0000000000000000000000000001";

/// Text a writer must carry through its staging encoding unharmed: the
/// separators and escapes of every encoding on the way, and nothing at all.
const AWKWARD_TEXT: &str = "tab\there, newline\nhere, return\rhere, backslash \\ and \\N, \
                            quote \" and ' and ünïcödé";

// The attributes a writer lifts into columns, under the names
// bourgeois_concern's runtime already gives them.
const SERVICE_ATTRIBUTE: &str = "service.name";
const ORGANIZATION_ATTRIBUTE: &str = "organization.identifier";
const CUSTOMER_ATTRIBUTE: &str = "customer.id";
// The reserved attributes a generated emitter appends: the module's version on
// every record, and a log's qualified name, since its body is its description.
const VERSION_ATTRIBUTE: &str = "bc.telemetry.schema_version";
const LOG_NAME_ATTRIBUTE: &str = "bc.telemetry.log";

// The separators of a check's line: between fields, between attributes, and
// between a span's events. Control characters no fixture value holds.
const FIELD_SEPARATOR: char = '\u{1f}';
const ATTRIBUTE_SEPARATOR: char = '\u{1e}';
const EVENT_SEPARATOR: char = '\u{1d}';
/// How a check's line spells a null field.
const NULL_FIELD: &str = "\\N";

/// An attribute's value, as a runtime carries one.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

/// Attributes in the order a record holds them. A key may repeat: the runtime
/// permits it, and what a writer does about it is one of the rules.
type Attributes = Vec<(String, Value)>;

/// The three kinds of record, which are the three tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    Spans,
    Metrics,
    Logs,
}

impl Signal {
    const ALL: [Self; 3] = [Self::Spans, Self::Metrics, Self::Logs];

    /// The table's name, which is also how the corpus names the kind.
    fn table(self) -> &'static str {
        match self {
            Self::Spans => "spans",
            Self::Metrics => "metrics",
            Self::Logs => "logs",
        }
    }

    /// The table's columns, in the order a staged row lists them.
    fn columns(self) -> Vec<&'static str> {
        let mut columns = vec![
            "time",
            "record_id",
            "module",
            "name",
            "schema_version",
            "service_name",
            "organization",
            "customer",
            "trace_id",
            "span_id",
            "attributes",
        ];
        columns.extend_from_slice(match self {
            Self::Spans => &[
                "end_time",
                "parent_span_id",
                "kind",
                "status",
                "status_message",
                "events",
            ],
            Self::Metrics => &["kind", "value"],
            Self::Logs => &["severity", "body"],
        });
        columns
    }
}

/// A span's place in a trace.
#[derive(Debug, Clone)]
struct SpanIdentity {
    trace_id: String,
    span_id: String,
}

/// What identifies a metric point or a log record: the producer that observed
/// it and that producer's count. Valid when the producer is not all zeros and
/// the sequence is not zero.
#[derive(Debug, Clone)]
struct Observation {
    producer: String,
    sequence: u64,
}

#[derive(Debug, Clone)]
struct SpanRecord {
    name: String,
    identity: SpanIdentity,
    parent_span_id: Option<String>,
    kind: &'static str,
    status: &'static str,
    status_message: String,
    start_ms: i64,
    end_ms: i64,
    attributes: Attributes,
    events: Vec<EventRecord>,
}

#[derive(Debug, Clone)]
struct EventRecord {
    name: String,
    time_ms: i64,
    attributes: Attributes,
}

#[derive(Debug, Clone)]
struct MetricRecord {
    name: String,
    kind: &'static str,
    value: Value,
    time_ms: i64,
    attributes: Attributes,
    observation: Observation,
    /// The span in force when the point was recorded, if any.
    within: Option<SpanIdentity>,
}

#[derive(Debug, Clone)]
struct LogRecord {
    time_ms: i64,
    severity: &'static str,
    body: String,
    attributes: Attributes,
    observation: Observation,
    /// The span in force when the record was made, if any.
    within: Option<SpanIdentity>,
}

#[derive(Debug, Clone)]
enum Record {
    Span(SpanRecord),
    Metric(MetricRecord),
    Log(LogRecord),
}

/// Records a writer exports in one call.
struct Batch {
    name: String,
    signal: Signal,
    records: Vec<Record>,
}

/// One stored — or staged — row: the columns every table carries, and its
/// table's own.
#[derive(Debug, Clone)]
struct Row {
    time_ms: i64,
    record_id: String,
    module: Option<String>,
    name: String,
    schema_version: Option<i64>,
    service_name: String,
    organization: Option<String>,
    customer: Option<String>,
    trace_id: Option<String>,
    span_id: Option<String>,
    attributes: BTreeMap<String, Value>,
    tail: Tail,
}

#[derive(Debug, Clone)]
enum Tail {
    Span {
        end_ms: i64,
        parent_span_id: Option<String>,
        kind: &'static str,
        status: &'static str,
        status_message: String,
        events: Vec<StoredEvent>,
    },
    Metric {
        kind: &'static str,
        value: f64,
    },
    Log {
        severity: &'static str,
        body: String,
    },
}

#[derive(Debug, Clone)]
struct StoredEvent {
    name: String,
    time_ms: i64,
    attributes: BTreeMap<String, Value>,
}

/// What a writer makes of one record: the row it stages, or nothing when it
/// refuses the record, and how many repeated attribute keys it met.
struct Staged {
    row: Option<Row>,
    collisions: u64,
}

/// What a writer counts for a batch before the database sees it.
#[derive(Debug, Default, Clone, Copy)]
struct WriterCounts {
    /// Records the writer refused to stage: a metric with no number for a
    /// value, a record with no identity.
    refused: u64,
    /// Repeated attribute keys. The record is still written, with the last
    /// value; the collision is counted as a drop of the value it replaced.
    collisions: u64,
}

/// The four counts an ingest function returns.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct IngestCounts {
    inserted: u64,
    out_of_range: u64,
    erased: u64,
    duplicate: u64,
}

/// An object from attributes: each key once, the last value written under it,
/// and how many values were replaced on the way.
fn attribute_object(attributes: &Attributes) -> (BTreeMap<String, Value>, u64) {
    let mut object = BTreeMap::new();
    let mut collisions = 0;
    for (key, value) in attributes {
        if object.insert(key.clone(), value.clone()).is_some() {
            collisions += 1;
        }
    }
    (object, collisions)
}

/// Takes a text attribute out of `object`, leaving any other type where it is:
/// only text is ever a service's, a tenant's or a customer's name.
fn lift_text(object: &mut BTreeMap<String, Value>, key: &str) -> Option<String> {
    if !matches!(object.get(key), Some(Value::Text(_))) {
        return None;
    }
    match object.remove(key) {
        Some(Value::Text(text)) => Some(text),
        _ => None,
    }
}

/// The columns a record's attributes say it belongs under.
struct Lifted {
    module: Option<String>,
    name: String,
    schema_version: Option<i64>,
    service_name: String,
    organization: Option<String>,
    customer: Option<String>,
}

/// Lifts identity and tenancy out of `object`. `wire_name` is the
/// name the record carries — a span's or a metric's own, a log's reserved
/// attribute.
///
/// A record is *declared* when it carries the version attribute as an integer
/// and its name is `<module>.<item>`; its module, name and version then become
/// columns. Any other record is one no definition declared: its name is kept
/// whole under a null module, and whatever it carried stays in its attributes.
fn lift(object: &mut BTreeMap<String, Value>, wire_name: &str) -> Lifted {
    let service_name = lift_text(object, SERVICE_ATTRIBUTE);
    let organization = lift_text(object, ORGANIZATION_ATTRIBUTE);
    let customer = lift_text(object, CUSTOMER_ATTRIBUTE);

    let version = match object.get(VERSION_ATTRIBUTE) {
        Some(Value::Int(version)) => Some(*version),
        _ => None,
    };
    let qualified = wire_name
        .split_once('.')
        .filter(|(module, item)| !module.is_empty() && !item.is_empty() && !item.contains('.'));
    let (module, name, schema_version) = match (version, qualified) {
        (Some(version), Some((module, item))) => {
            object.remove(VERSION_ATTRIBUTE);
            (Some(module.to_string()), item.to_string(), Some(version))
        }
        _ => (None, wire_name.to_string(), None),
    };
    Lifted {
        module,
        name,
        schema_version,
        service_name: match service_name {
            Some(service_name) => service_name,
            None => SERVICE_NAME.to_string(),
        },
        organization,
        customer,
    }
}

fn observation_is_valid(observation: &Observation) -> bool {
    observation.sequence != 0 && observation.producer.chars().any(|digit| digit != '0')
}

/// Whether any of `texts` holds a NUL, which no PostgreSQL `text` or `jsonb`
/// value can store. The writer refuses such a record rather than alter it: a
/// customer's id with its NUL replaced is another customer's, whose erasure
/// would then miss it.
fn holds_nul<'text>(mut texts: impl Iterator<Item = &'text str>) -> bool {
    texts.any(|text| text.contains('\0'))
}

/// Every text an attribute list carries: each key, and each value that is
/// text.
fn attribute_texts(attributes: &Attributes) -> impl Iterator<Item = &str> {
    attributes.iter().flat_map(|(key, value)| {
        let text = match value {
            Value::Text(text) => Some(text.as_str()),
            Value::Bool(_) | Value::Int(_) | Value::Float(_) => None,
        };
        std::iter::once(key.as_str()).chain(text)
    })
}

impl Staged {
    /// A record the writer refuses: no row, and nothing counted against it.
    fn refused() -> Self {
        Self {
            row: None,
            collisions: 0,
        }
    }
}

/// The row a writer stages for `record`, or its refusal.
fn stage(record: &Record) -> Staged {
    match record {
        Record::Span(span) => stage_span(span),
        Record::Metric(metric) => stage_metric(metric),
        Record::Log(log) => stage_log(log),
    }
}

fn stage_span(span: &SpanRecord) -> Staged {
    let texts = [span.name.as_str(), span.status_message.as_str()]
        .into_iter()
        .chain(attribute_texts(&span.attributes))
        .chain(span.events.iter().flat_map(|event| {
            std::iter::once(event.name.as_str()).chain(attribute_texts(&event.attributes))
        }));
    if holds_nul(texts) {
        return Staged::refused();
    }
    let (mut object, mut collisions) = attribute_object(&span.attributes);
    let lifted = lift(&mut object, &span.name);
    let events = span
        .events
        .iter()
        .map(|event| {
            let (attributes, event_collisions) = attribute_object(&event.attributes);
            collisions += event_collisions;
            StoredEvent {
                name: event.name.clone(),
                time_ms: event.time_ms,
                attributes,
            }
        })
        .collect();
    let row = Row {
        time_ms: span.start_ms,
        record_id: format!("{}-{}", span.identity.trace_id, span.identity.span_id),
        module: lifted.module,
        name: lifted.name,
        schema_version: lifted.schema_version,
        service_name: lifted.service_name,
        organization: lifted.organization,
        customer: lifted.customer,
        trace_id: Some(span.identity.trace_id.clone()),
        span_id: Some(span.identity.span_id.clone()),
        attributes: object,
        tail: Tail::Span {
            end_ms: span.end_ms,
            parent_span_id: span.parent_span_id.clone(),
            kind: span.kind,
            status: span.status,
            status_message: span.status_message.clone(),
            events,
        },
    };
    Staged {
        row: Some(row),
        collisions,
    }
}

fn stage_metric(metric: &MetricRecord) -> Staged {
    // A metric's value is a number. A boolean counts as one and zero, as every
    // runtime has always stored it; text is no number, and neither is a float
    // that is not finite, which the column would take and every sum would then
    // carry.
    let value = match &metric.value {
        Value::Int(value) => Some(int_as_float(*value)),
        Value::Float(value) if value.is_finite() => Some(*value),
        Value::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        Value::Float(_) | Value::Text(_) => None,
    };
    let (Some(value), true) = (value, observation_is_valid(&metric.observation)) else {
        return Staged::refused();
    };
    if holds_nul(std::iter::once(metric.name.as_str()).chain(attribute_texts(&metric.attributes))) {
        return Staged::refused();
    }
    let (mut object, collisions) = attribute_object(&metric.attributes);
    let lifted = lift(&mut object, &metric.name);
    let row = Row {
        time_ms: metric.time_ms,
        record_id: record_id_of(&metric.observation),
        module: lifted.module,
        name: lifted.name,
        schema_version: lifted.schema_version,
        service_name: lifted.service_name,
        organization: lifted.organization,
        customer: lifted.customer,
        trace_id: metric.within.as_ref().map(|span| span.trace_id.clone()),
        span_id: metric.within.as_ref().map(|span| span.span_id.clone()),
        attributes: object,
        tail: Tail::Metric {
            kind: metric.kind,
            value,
        },
    };
    Staged {
        row: Some(row),
        collisions,
    }
}

fn stage_log(log: &LogRecord) -> Staged {
    if !observation_is_valid(&log.observation)
        || holds_nul(std::iter::once(log.body.as_str()).chain(attribute_texts(&log.attributes)))
    {
        return Staged::refused();
    }
    let (mut object, collisions) = attribute_object(&log.attributes);
    // A log's name rides in a reserved attribute, since its body is its
    // description; a log no definition declared has none.
    let wire_name = match object.get(LOG_NAME_ATTRIBUTE) {
        Some(Value::Text(name)) => name.clone(),
        _ => String::new(),
    };
    let lifted = lift(&mut object, &wire_name);
    if lifted.module.is_some() {
        object.remove(LOG_NAME_ATTRIBUTE);
    }
    let row = Row {
        time_ms: log.time_ms,
        record_id: record_id_of(&log.observation),
        module: lifted.module,
        name: lifted.name,
        schema_version: lifted.schema_version,
        service_name: lifted.service_name,
        organization: lifted.organization,
        customer: lifted.customer,
        trace_id: log.within.as_ref().map(|span| span.trace_id.clone()),
        span_id: log.within.as_ref().map(|span| span.span_id.clone()),
        attributes: object,
        tail: Tail::Log {
            severity: log.severity,
            body: log.body.clone(),
        },
    };
    Staged {
        row: Some(row),
        collisions,
    }
}

fn record_id_of(observation: &Observation) -> String {
    format!("{}-{}", observation.producer, observation.sequence)
}

/// A counter's increment as the `value` column holds it. The corpus's integers
/// are small, so none is changed by becoming a float.
#[expect(
    clippy::cast_precision_loss,
    reason = "every integer the corpus holds is far inside the range a float holds exactly"
)]
fn int_as_float(value: i64) -> f64 {
    value as f64
}

/// What one table holds, and the two things the ingest step reads: the days
/// with partitions, and the erasures.
struct Store {
    live_days: BTreeSet<i64>,
    /// `(organization, customer)` to the instant they were erased.
    erasures: BTreeMap<(Option<String>, String), i64>,
    tables: BTreeMap<&'static str, BTreeMap<(i64, String), Row>>,
}

impl Store {
    /// A store as the corpus's setup leaves it: maintained at the instant, with
    /// one customer erased.
    fn after_setup() -> Self {
        let today = INSTANT_MS.div_euclid(MILLISECONDS_PER_DAY);
        let mut erasures = BTreeMap::new();
        erasures.insert(
            (Some(ORGANIZATION.to_string()), ERASED_CUSTOMER.to_string()),
            ERASED_AT_MS,
        );
        Self {
            live_days: (today - DAYS_KEPT..=today + DAYS_AHEAD).collect(),
            erasures,
            tables: BTreeMap::new(),
        }
    }

    /// The ingest step: each staged row is out of range, erased, a
    /// duplicate, or inserted — in that order of asking.
    fn ingest(&mut self, signal: Signal, rows: Vec<Row>) -> IngestCounts {
        let table = self.tables.entry(signal.table()).or_default();
        let mut counts = IngestCounts::default();
        for row in rows {
            let day = row.time_ms.div_euclid(MILLISECONDS_PER_DAY);
            if !self.live_days.contains(&day) {
                counts.out_of_range += 1;
                continue;
            }
            let erased_at = row.customer.as_ref().and_then(|customer| {
                self.erasures
                    .get(&(row.organization.clone(), customer.clone()))
            });
            if erased_at.is_some_and(|erased_at| row.time_ms <= *erased_at) {
                counts.erased += 1;
                continue;
            }
            let key = (row.time_ms, row.record_id.clone());
            if table.contains_key(&key) {
                counts.duplicate += 1;
                continue;
            }
            table.insert(key, row);
            counts.inserted += 1;
        }
        counts
    }

    fn rows(&self, signal: Signal) -> impl Iterator<Item = &Row> {
        self.tables
            .get(signal.table())
            .into_iter()
            .flatten()
            .map(|(_, row)| row)
    }
}

// ---------------------------------------------------------------------------
// The fixture records.
// ---------------------------------------------------------------------------

/// Hands out the identities the fixture records carry, in order, so the corpus
/// is the same on every run.
struct Identities {
    next_span: u64,
    next_observation: u64,
}

impl Identities {
    fn starting() -> Self {
        Self {
            next_span: 1,
            next_observation: 1,
        }
    }

    fn span(&mut self) -> SpanIdentity {
        let number = self.next_span;
        self.next_span += 1;
        SpanIdentity {
            trace_id: format!("c0de{number:028x}"),
            span_id: format!("5ba9{number:012x}"),
        }
    }

    fn observation(&mut self) -> Observation {
        let sequence = self.next_observation;
        self.next_observation += 1;
        Observation {
            producer: PRODUCER.to_string(),
            sequence,
        }
    }
}

/// What tells the records of one declared item apart. The corpus holds one
/// record of each item per variant.
struct Variant {
    /// The record's place among its item's, from zero.
    number: i64,
    minutes_before_the_instant: i64,
    /// A span's end status, and the message that goes with it.
    status: &'static str,
    status_message: &'static str,
    /// The level an error is reported at: two reports and one degraded report.
    error_severity: &'static str,
}

const VARIANTS: [Variant; 3] = [
    Variant {
        number: 0,
        minutes_before_the_instant: 20,
        status: "ok",
        status_message: "",
        error_severity: "error",
    },
    Variant {
        number: 1,
        minutes_before_the_instant: 15,
        status: "unset",
        status_message: "",
        error_severity: "error",
    },
    Variant {
        number: 2,
        minutes_before_the_instant: 10,
        status: "error",
        status_message: "failed",
        error_severity: "warn",
    },
];

impl Variant {
    fn time_ms(&self) -> i64 {
        INSTANT_MS - self.minutes_before_the_instant * MILLISECONDS_PER_MINUTE
    }

    /// A value of `scalar` for this variant's record. Text takes one of two
    /// values, so a query that groups by it has groups to return.
    fn value_of(&self, attribute: &str, scalar: ScalarType) -> Value {
        match scalar {
            ScalarType::String => Value::Text(format!("{attribute}-{}", self.number % 2)),
            ScalarType::Digest => Value::Text(format!("digest-{}", self.number % 2)),
            ScalarType::Int => Value::Int(self.number + 1),
            ScalarType::Duration => Value::Int(250 * (self.number + 1)),
            ScalarType::Float => Value::Float(12.5 + int_as_float(self.number)),
            ScalarType::Bool => Value::Bool(self.number % 2 == 0),
        }
    }

    /// The attributes a generated emitter gives this variant's record: the
    /// declared ones, the tenant, a customer on the first, and the reserved
    /// version.
    fn attributes(
        &self,
        declared: &BTreeMap<String, ScalarType>,
        schema_version: u32,
    ) -> Attributes {
        let mut attributes: Attributes = declared
            .iter()
            .map(|(name, scalar)| (name.clone(), self.value_of(name, *scalar)))
            .collect();
        attributes.push((
            ORGANIZATION_ATTRIBUTE.to_string(),
            Value::Text(ORGANIZATION.to_string()),
        ));
        if self.number == 0 {
            attributes.push((
                CUSTOMER_ATTRIBUTE.to_string(),
                Value::Text(KEPT_CUSTOMER.to_string()),
            ));
        }
        attributes.push((
            VERSION_ATTRIBUTE.to_string(),
            Value::Int(i64::from(schema_version)),
        ));
        attributes
    }
}

/// How the wire names an item: its module, then its own name, so two modules'
/// items never share a name on the wire.
fn qualified(module_name: &str, item: &str) -> String {
    format!("{module_name}.{item}")
}

/// A record of every item the tree declares, per variant: one batch per table.
fn declared_batches(tree: &Tree, identities: &mut Identities) -> Vec<Batch> {
    let mut spans = Vec::new();
    let mut metrics = Vec::new();
    let mut logs = Vec::new();
    for (module_name, module) in &tree.modules {
        declared_spans(module_name, module, identities, &mut spans);
        declared_metrics(module_name, module, identities, &mut metrics);
        declared_logs(module_name, module, identities, &mut logs);
        reported_errors(module_name, module, identities, &mut logs);
    }
    vec![
        Batch {
            name: "every declared span".to_string(),
            signal: Signal::Spans,
            records: spans,
        },
        Batch {
            name: "every declared metric".to_string(),
            signal: Signal::Metrics,
            records: metrics,
        },
        Batch {
            name: "every declared log and reported error".to_string(),
            signal: Signal::Logs,
            records: logs,
        },
    ]
}

fn declared_spans(
    module_name: &str,
    module: &Module,
    identities: &mut Identities,
    records: &mut Vec<Record>,
) {
    for (name, span) in &module.items.spans {
        for variant in &VARIANTS {
            let start_ms = variant.time_ms();
            records.push(Record::Span(SpanRecord {
                name: qualified(module_name, name),
                identity: identities.span(),
                parent_span_id: None,
                kind: span.kind.keyword(),
                status: variant.status,
                status_message: variant.status_message.to_string(),
                start_ms,
                end_ms: start_ms + 125,
                attributes: variant.attributes(&span.attributes, module.schema_version.value),
                events: span
                    .events
                    .iter()
                    .map(|event| EventRecord {
                        name: event.clone(),
                        time_ms: start_ms + 5,
                        attributes: Vec::new(),
                    })
                    .collect(),
            }));
        }
    }
}

fn declared_metrics(
    module_name: &str,
    module: &Module,
    identities: &mut Identities,
    records: &mut Vec<Record>,
) {
    for (name, metric) in &module.items.metrics {
        for variant in &VARIANTS {
            let value = match metric.kind {
                MetricKind::Counter => Value::Int(variant.number + 1),
                MetricKind::Gauge => Value::Float(3.5 + int_as_float(variant.number)),
                MetricKind::Histogram => Value::Float(120.5 + 10.0 * int_as_float(variant.number)),
            };
            records.push(Record::Metric(MetricRecord {
                name: qualified(module_name, name),
                kind: metric.kind.keyword(),
                value,
                time_ms: variant.time_ms(),
                attributes: variant.attributes(&metric.attributes, module.schema_version.value),
                observation: identities.observation(),
                within: None,
            }));
        }
    }
}

fn declared_logs(
    module_name: &str,
    module: &Module,
    identities: &mut Identities,
    records: &mut Vec<Record>,
) {
    for (name, log) in &module.items.logs {
        for variant in &VARIANTS {
            let mut attributes = variant.attributes(&log.fields, module.schema_version.value);
            attributes.push((
                LOG_NAME_ATTRIBUTE.to_string(),
                Value::Text(qualified(module_name, name)),
            ));
            records.push(Record::Log(LogRecord {
                time_ms: variant.time_ms(),
                severity: log.severity.keyword(),
                body: log.description.clone(),
                attributes,
                observation: identities.observation(),
                within: None,
            }));
        }
    }
}

/// An error is reported as a log record carrying its category and code.
fn reported_errors(
    module_name: &str,
    module: &Module,
    identities: &mut Identities,
    records: &mut Vec<Record>,
) {
    for (name, error) in &module.items.errors {
        // Nothing reports a refusal, so the corpus holds no record of one.
        if error.class == ErrorClass::Refusal {
            continue;
        }
        for variant in &VARIANTS {
            let mut attributes = variant.attributes(&error.fields, module.schema_version.value);
            attributes.push((
                "error.category".to_string(),
                Value::Text(module_name.to_string()),
            ));
            attributes.push((
                "error.kind".to_string(),
                Value::Int(i64::from(error.code.value)),
            ));
            attributes.push((
                LOG_NAME_ATTRIBUTE.to_string(),
                Value::Text(qualified(module_name, name)),
            ));
            records.push(Record::Log(LogRecord {
                time_ms: variant.time_ms(),
                severity: variant.error_severity,
                body: error.description.clone(),
                attributes,
                observation: identities.observation(),
                within: None,
            }));
        }
    }
}

/// One record that tests one rule: what it is called, when it happened, and
/// what it carries.
struct RuleCase {
    name: &'static str,
    time_ms: i64,
    attributes: Attributes,
}

fn text(key: &str, value: &str) -> (String, Value) {
    (key.to_string(), Value::Text(value.to_string()))
}

/// A version attribute, and — for a log, whose name rides in an attribute —
/// `name` as the log's name. A span or a metric keeps the log-name attribute as
/// an ordinary one.
fn named_with_a_version(name: &str) -> Attributes {
    vec![
        (VERSION_ATTRIBUTE.to_string(), Value::Int(1)),
        text(LOG_NAME_ATTRIBUTE, name),
    ]
}

/// The cases every table's ingest function is held to. None is a declared
/// item's record, so none reaches a view.
fn rule_cases() -> Vec<RuleCase> {
    vec![
        RuleCase {
            name: "corpus.rules.undeclared",
            time_ms: INSTANT_MS - 9 * MILLISECONDS_PER_MINUTE,
            attributes: vec![
                text(SERVICE_ATTRIBUTE, "another-service"),
                text("route", "/health"),
            ],
        },
        RuleCase {
            name: "corpus.rules.before_every_partition",
            time_ms: INSTANT_MS - 5 * MILLISECONDS_PER_DAY,
            attributes: Vec::new(),
        },
        RuleCase {
            name: "corpus.rules.after_every_partition",
            time_ms: INSTANT_MS + 5 * MILLISECONDS_PER_DAY,
            attributes: Vec::new(),
        },
        // At the very instant of the erasure: in flight when it committed.
        RuleCase {
            name: "corpus.rules.erased",
            time_ms: ERASED_AT_MS,
            attributes: vec![
                text(ORGANIZATION_ATTRIBUTE, ORGANIZATION),
                text(CUSTOMER_ATTRIBUTE, ERASED_CUSTOMER),
            ],
        },
        // A millisecond later: new data about a customer who came back.
        RuleCase {
            name: "corpus.rules.after_the_erasure",
            time_ms: ERASED_AT_MS + 1,
            attributes: vec![
                text(ORGANIZATION_ATTRIBUTE, ORGANIZATION),
                text(CUSTOMER_ATTRIBUTE, ERASED_CUSTOMER),
            ],
        },
        // The same customer name under no tenant is another customer.
        RuleCase {
            name: "corpus.rules.another_tenants_customer",
            time_ms: ERASED_AT_MS - 1,
            attributes: vec![text(CUSTOMER_ATTRIBUTE, ERASED_CUSTOMER)],
        },
        // Only `<module>.<item>` with an integer version is declared: a name
        // with two dots, or an empty side, is kept whole under a null module,
        // and its version stays among its attributes.
        RuleCase {
            name: "corpus.rules.three_parts",
            time_ms: INSTANT_MS - 5 * MILLISECONDS_PER_MINUTE,
            attributes: named_with_a_version("corpus.rules.three_parts"),
        },
        RuleCase {
            name: ".no_module",
            time_ms: INSTANT_MS - 4 * MILLISECONDS_PER_MINUTE,
            attributes: named_with_a_version(".no_module"),
        },
        RuleCase {
            name: "no_item.",
            time_ms: INSTANT_MS - 3 * MILLISECONDS_PER_MINUTE,
            attributes: named_with_a_version("no_item."),
        },
        RuleCase {
            name: "by_hand.declared",
            time_ms: INSTANT_MS - 2 * MILLISECONDS_PER_MINUTE,
            attributes: named_with_a_version("by_hand.declared"),
        },
        RuleCase {
            name: "corpus.rules.repeated_key",
            time_ms: INSTANT_MS - 8 * MILLISECONDS_PER_MINUTE,
            attributes: vec![text("note", "first"), text("note", "second")],
        },
        RuleCase {
            name: "corpus.rules.awkward_text",
            time_ms: INSTANT_MS - 7 * MILLISECONDS_PER_MINUTE,
            attributes: vec![
                text("note", AWKWARD_TEXT),
                text("empty", ""),
                ("ratio".to_string(), Value::Float(0.25)),
                ("whole".to_string(), Value::Float(2.0)),
                // JSON has no spelling for it, so the column holds null.
                ("not_finite".to_string(), Value::Float(f64::NAN)),
                ("count".to_string(), Value::Int(-7)),
                ("flag".to_string(), Value::Bool(false)),
            ],
        },
    ]
}

/// The rule cases as records of one table, with the writer-side refusals that
/// table has.
fn rule_batch(signal: Signal, identities: &mut Identities) -> Batch {
    // A metric or a log made inside a span carries that span's ids as columns.
    let within = Some(SpanIdentity {
        trace_id: "c0de00000000000000000000000000ff".to_string(),
        span_id: "5ba90000000000ff".to_string(),
    });
    let mut records = Vec::new();
    for case in rule_cases() {
        records.push(match signal {
            Signal::Spans => Record::Span(SpanRecord {
                name: case.name.to_string(),
                identity: identities.span(),
                parent_span_id: Some("5ba9000000000000".to_string()),
                kind: "server",
                status: "unset",
                status_message: String::new(),
                start_ms: case.time_ms,
                end_ms: case.time_ms + 40,
                attributes: case.attributes.clone(),
                events: vec![EventRecord {
                    name: "noted".to_string(),
                    time_ms: case.time_ms + 1,
                    attributes: case.attributes,
                }],
            }),
            Signal::Metrics => Record::Metric(MetricRecord {
                name: case.name.to_string(),
                kind: "gauge",
                value: Value::Float(0.5),
                time_ms: case.time_ms,
                attributes: case.attributes,
                observation: identities.observation(),
                within: within.clone(),
            }),
            Signal::Logs => Record::Log(LogRecord {
                time_ms: case.time_ms,
                severity: "info",
                body: format!("{}: {AWKWARD_TEXT}", case.name),
                attributes: case.attributes,
                observation: identities.observation(),
                within: within.clone(),
            }),
        });
    }
    records.extend(writer_rule_records(signal, identities));
    Batch {
        name: format!("the rules, for {}", signal.table()),
        signal,
        records,
    }
}

/// The records a writer refuses or converts before the database sees them —
/// the rules a table's ingest function never meets.
fn writer_rule_records(signal: Signal, identities: &mut Identities) -> Vec<Record> {
    let mut records = Vec::new();
    let refused_at = INSTANT_MS - 6 * MILLISECONDS_PER_MINUTE;
    let no_identity = Observation {
        producer: PRODUCER.to_string(),
        sequence: 0,
    };
    // Text no PostgreSQL value can hold: refused whole, never altered.
    let holds_a_nul = || vec![("note".to_string(), Value::Text("before\0after".to_string()))];
    match signal {
        Signal::Spans => {
            records.push(Record::Span(SpanRecord {
                name: "corpus.rules.text_holds_a_nul".to_string(),
                identity: identities.span(),
                parent_span_id: None,
                kind: "internal",
                status: "unset",
                status_message: String::new(),
                start_ms: refused_at,
                end_ms: refused_at + 1,
                attributes: holds_a_nul(),
                events: Vec::new(),
            }));
        }
        Signal::Metrics => {
            records.push(Record::Metric(MetricRecord {
                name: "corpus.rules.text_holds_a_nul".to_string(),
                kind: "counter",
                value: Value::Int(1),
                time_ms: refused_at,
                attributes: holds_a_nul(),
                observation: identities.observation(),
                within: None,
            }));
            records.push(Record::Metric(MetricRecord {
                name: "corpus.rules.value_is_a_boolean".to_string(),
                kind: "gauge",
                value: Value::Bool(true),
                time_ms: refused_at,
                attributes: Vec::new(),
                observation: identities.observation(),
                within: None,
            }));
            records.push(Record::Metric(MetricRecord {
                name: "corpus.rules.value_is_text".to_string(),
                kind: "gauge",
                value: Value::Text("not a number".to_string()),
                time_ms: refused_at,
                attributes: Vec::new(),
                observation: identities.observation(),
                within: None,
            }));
            records.push(Record::Metric(MetricRecord {
                name: "corpus.rules.value_is_not_finite".to_string(),
                kind: "gauge",
                value: Value::Float(f64::INFINITY),
                time_ms: refused_at,
                attributes: Vec::new(),
                observation: identities.observation(),
                within: None,
            }));
            records.push(Record::Metric(MetricRecord {
                name: "corpus.rules.no_identity".to_string(),
                kind: "counter",
                value: Value::Int(1),
                time_ms: refused_at,
                attributes: Vec::new(),
                observation: no_identity,
                within: None,
            }));
        }
        Signal::Logs => {
            records.push(Record::Log(LogRecord {
                time_ms: refused_at,
                severity: "info",
                body: "corpus.rules.no_identity".to_string(),
                attributes: Vec::new(),
                observation: no_identity,
                within: None,
            }));
            records.push(Record::Log(LogRecord {
                time_ms: refused_at,
                severity: "info",
                body: "corpus.rules.text_holds_a_nul".to_string(),
                attributes: holds_a_nul(),
                observation: identities.observation(),
                within: None,
            }));
        }
    }
    records
}

/// Every batch of the corpus, in the order a writer exports them: every
/// declared item; the same spans, metrics and logs again, as a batch retried
/// after an unknown commit outcome; then the rules.
fn fixtures(tree: &Tree) -> Vec<Batch> {
    let mut identities = Identities::starting();
    let declared = declared_batches(tree, &mut identities);
    let mut batches = Vec::new();
    let mut retried = Vec::new();
    for batch in declared {
        retried.push(Batch {
            name: format!("{}, retried", batch.name),
            signal: batch.signal,
            records: batch.records.clone(),
        });
        batches.push(batch);
    }
    batches.extend(retried);
    for signal in Signal::ALL {
        batches.push(rule_batch(signal, &mut identities));
    }
    batches
}

// ---------------------------------------------------------------------------
// Spelling rows: as a staged line, and as a check's line.
// ---------------------------------------------------------------------------

/// A float as PostgreSQL prints a `double precision`: the shortest text that
/// reads back as the same value. The corpus's floats are short decimals, which
/// both sides print alike.
fn float_text(value: f64) -> String {
    format!("{value}")
}

fn iso_time(formatter: &DateFormatter, time_ms: i64) -> String {
    formatter.format(Epoch::from_millis(time_ms))
}

/// An attribute object as the `attributes` column's JSON. A float that is not
/// finite has no JSON spelling and is written as null.
fn write_attribute_fields(writer: &mut JsonWriter, attributes: &BTreeMap<String, Value>) {
    for (key, value) in attributes {
        match value {
            Value::Bool(value) => writer.field(key, *value),
            Value::Int(value) => writer.field(key, *value),
            Value::Float(value) if value.is_finite() => writer.field(key, *value),
            Value::Float(_) => writer.field_null(key),
            Value::Text(value) => writer.field(key, value.as_str()),
        }
    }
}

fn attributes_json(attributes: &BTreeMap<String, Value>) -> Result<String, JsonWriteError> {
    let mut writer = JsonWriter::new();
    writer.object(|writer| write_attribute_fields(writer, attributes));
    writer.finish()
}

fn events_json(
    events: &[StoredEvent],
    formatter: &DateFormatter,
) -> Result<String, JsonWriteError> {
    let mut writer = JsonWriter::new();
    writer.bare_array(|writer| {
        for event in events {
            writer.array_value(|writer| {
                writer.object(|writer| {
                    writer.field("name", event.name.as_str());
                    writer.field("time", iso_time(formatter, event.time_ms));
                    writer.nested_object("attributes", |writer| {
                        write_attribute_fields(writer, &event.attributes);
                    });
                });
            });
        }
    });
    writer.finish()
}

/// One field of a staged line: PostgreSQL's `COPY` text format, in which a
/// backslash introduces an escape and a null is `\N`.
fn copy_field(value: Option<&str>) -> String {
    let Some(value) = value else {
        return NULL_FIELD.to_string();
    };
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// A row as one line of `COPY ... FROM STDIN`, its fields in the table's column
/// order.
fn staged_line(row: &Row, formatter: &DateFormatter) -> Result<String, JsonWriteError> {
    let version = row.schema_version.map(|version| version.to_string());
    let mut fields = vec![
        copy_field(Some(&iso_time(formatter, row.time_ms))),
        copy_field(Some(&row.record_id)),
        copy_field(row.module.as_deref()),
        copy_field(Some(&row.name)),
        copy_field(version.as_deref()),
        copy_field(Some(&row.service_name)),
        copy_field(row.organization.as_deref()),
        copy_field(row.customer.as_deref()),
        copy_field(row.trace_id.as_deref()),
        copy_field(row.span_id.as_deref()),
        copy_field(Some(&attributes_json(&row.attributes)?)),
    ];
    match &row.tail {
        Tail::Span {
            end_ms,
            parent_span_id,
            kind,
            status,
            status_message,
            events,
        } => {
            fields.push(copy_field(Some(&iso_time(formatter, *end_ms))));
            fields.push(copy_field(parent_span_id.as_deref()));
            fields.push(copy_field(Some(kind)));
            fields.push(copy_field(Some(status)));
            fields.push(copy_field(Some(status_message)));
            fields.push(copy_field(Some(&events_json(events, formatter)?)));
        }
        Tail::Metric { kind, value } => {
            fields.push(copy_field(Some(kind)));
            fields.push(copy_field(Some(&float_text(*value))));
        }
        Tail::Log { severity, body } => {
            fields.push(copy_field(Some(severity)));
            fields.push(copy_field(Some(body)));
        }
    }
    Ok(fields.join("\t"))
}

fn check_field(value: Option<&str>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => NULL_FIELD.to_string(),
    }
}

/// An attribute object as a check's line spells it: each attribute as
/// `key=type:text`, in key order. A number is spelled as a float prints, so
/// `2` and `2.0` are one value, as they are to every reader.
fn check_attributes(attributes: &BTreeMap<String, Value>) -> String {
    let entries: Vec<String> = attributes
        .iter()
        .map(|(key, value)| match value {
            Value::Bool(value) => format!("{key}=boolean:{value}"),
            Value::Int(value) => format!("{key}=number:{}", float_text(int_as_float(*value))),
            Value::Float(value) if value.is_finite() => {
                format!("{key}=number:{}", float_text(*value))
            }
            Value::Float(_) => format!("{key}=null:"),
            Value::Text(value) => format!("{key}=string:{value}"),
        })
        .collect();
    entries.join(&ATTRIBUTE_SEPARATOR.to_string())
}

/// A stored row as the line its table's check query returns.
fn table_check_line(row: &Row) -> String {
    let version = row.schema_version.map(|version| version.to_string());
    let mut fields = vec![
        row.time_ms.to_string(),
        row.record_id.clone(),
        check_field(row.module.as_deref()),
        row.name.clone(),
        check_field(version.as_deref()),
        row.service_name.clone(),
        check_field(row.organization.as_deref()),
        check_field(row.customer.as_deref()),
        check_field(row.trace_id.as_deref()),
        check_field(row.span_id.as_deref()),
        check_attributes(&row.attributes),
    ];
    match &row.tail {
        Tail::Span {
            end_ms,
            parent_span_id,
            kind,
            status,
            status_message,
            events,
        } => {
            fields.push(end_ms.to_string());
            fields.push(check_field(parent_span_id.as_deref()));
            fields.push((*kind).to_string());
            fields.push((*status).to_string());
            fields.push(status_message.clone());
            let events: Vec<String> = events
                .iter()
                .map(|event| {
                    format!(
                        "{}@{}{{{}}}",
                        event.name,
                        event.time_ms,
                        check_attributes(&event.attributes)
                    )
                })
                .collect();
            fields.push(events.join(&EVENT_SEPARATOR.to_string()));
        }
        Tail::Metric { kind, value } => {
            fields.push((*kind).to_string());
            fields.push(float_text(*value));
        }
        Tail::Log { severity, body } => {
            fields.push((*severity).to_string());
            fields.push(body.clone());
        }
    }
    fields.join(&FIELD_SEPARATOR.to_string())
}

/// The SQL expression that spells an attribute object as [`check_attributes`]
/// does.
fn check_attributes_sql(object: &str) -> String {
    format!(
        "(SELECT coalesce(string_agg(entry.key || '=' || jsonb_typeof(entry.value) || ':' || \
         CASE jsonb_typeof(entry.value) \
         WHEN 'number' THEN ((entry.value #>> '{{}}')::double precision)::text \
         WHEN 'null' THEN '' \
         ELSE entry.value #>> '{{}}' END, \
         E'\\x1e' ORDER BY entry.key COLLATE \"C\"), '') \
         FROM jsonb_each({object}) AS entry)"
    )
}

/// The SQL expression that spells an instant as its milliseconds since 1970.
fn milliseconds_sql(instant: &str) -> String {
    format!("(extract(epoch FROM {instant}) * 1000)::bigint")
}

/// A check's query: one line per row of `relation`, its fields joined, the
/// lines in byte order — the order the corpus lists them in, whatever
/// collation the database was created with.
fn lines_query(fields: &[String], relation: &str) -> String {
    format!(
        "SELECT line FROM (SELECT concat_ws(E'\\x1f', {}) AS line FROM telemetry.{relation}) \
         AS lines ORDER BY line COLLATE \"C\"",
        fields.join(", ")
    )
}

fn nullable_sql(column: &str) -> String {
    format!("coalesce({column}, '\\N')")
}

/// The query whose rows are [`table_check_line`]s, for every row of `signal`'s
/// table.
fn table_check_query(signal: Signal) -> String {
    let mut fields = vec![
        milliseconds_sql("time"),
        "record_id".to_string(),
        nullable_sql("module"),
        "name".to_string(),
        nullable_sql("schema_version::text"),
        "service_name".to_string(),
        nullable_sql("organization"),
        nullable_sql("customer"),
        nullable_sql("trace_id"),
        nullable_sql("span_id"),
        check_attributes_sql("attributes"),
    ];
    match signal {
        Signal::Spans => {
            fields.push(milliseconds_sql("end_time"));
            fields.push(nullable_sql("parent_span_id"));
            fields.push("kind".to_string());
            fields.push("status".to_string());
            fields.push("status_message".to_string());
            fields.push(format!(
                "(SELECT coalesce(string_agg((listed.event->>'name') || '@' || {} || '{{' || {} \
                 || '}}', E'\\x1d' ORDER BY listed.position), '') \
                 FROM jsonb_array_elements(events) WITH ORDINALITY AS listed(event, position))",
                milliseconds_sql("(listed.event->>'time')::timestamptz"),
                check_attributes_sql("listed.event->'attributes'")
            ));
        }
        Signal::Metrics => {
            fields.push("kind".to_string());
            fields.push("value::text".to_string());
        }
        Signal::Logs => {
            fields.push("severity".to_string());
            fields.push("body".to_string());
        }
    }
    lines_query(&fields, signal.table())
}

/// A row as the line `view`'s check query returns, or `None` when the view
/// does not select it.
fn view_check_line(view: &ViewShape, row: &Row) -> Option<String> {
    let selected = row.module.as_deref() == Some(view.module.as_str())
        && row.name == view.item
        && row.schema_version == Some(i64::from(view.schema_version));
    if !selected {
        return None;
    }
    let mut fields = vec![
        row.time_ms.to_string(),
        row.service_name.clone(),
        check_field(row.organization.as_deref()),
        check_field(row.customer.as_deref()),
        check_field(row.trace_id.as_deref()),
        check_field(row.span_id.as_deref()),
    ];
    match &row.tail {
        Tail::Span {
            end_ms,
            kind,
            status,
            status_message,
            ..
        } => {
            fields.push(end_ms.to_string());
            fields.push(float_text(int_as_float(end_ms - row.time_ms)));
            fields.push((*kind).to_string());
            fields.push((*status).to_string());
            fields.push(status_message.clone());
        }
        Tail::Metric { value, .. } => fields.push(float_text(*value)),
        Tail::Log { severity, body } => {
            fields.push((*severity).to_string());
            fields.push(body.clone());
        }
    }
    for (attribute, scalar) in &view.attributes {
        // A column is its attribute cast to the declared type; a record written
        // before the attribute was added has none, and reads as null.
        let column = match (row.attributes.get(attribute), scalar) {
            (Some(Value::Text(value)), ScalarType::String | ScalarType::Digest) => {
                Some(value.clone())
            }
            (Some(Value::Int(value)), ScalarType::Int | ScalarType::Duration) => {
                Some(value.to_string())
            }
            (Some(Value::Float(value)), ScalarType::Float) => Some(float_text(*value)),
            (Some(Value::Bool(value)), ScalarType::Bool) => Some(value.to_string()),
            _ => None,
        };
        fields.push(check_field(column.as_deref()));
    }
    Some(fields.join(&FIELD_SEPARATOR.to_string()))
}

/// The query whose rows are [`view_check_line`]s, for every row `view` selects.
fn view_check_query(view: &ViewShape) -> String {
    let mut fields = vec![
        milliseconds_sql("time"),
        "service_name".to_string(),
        nullable_sql("organization"),
        nullable_sql("customer"),
        nullable_sql("trace_id"),
        nullable_sql("span_id"),
    ];
    for column in view.kind_columns() {
        fields.push(match *column {
            "end_time" => milliseconds_sql("end_time"),
            "duration_ms" | "value" => format!("{column}::text"),
            other => other.to_string(),
        });
    }
    for (attribute, _) in &view.attributes {
        fields.push(nullable_sql(&format!(
            "{}::text",
            quoted_identifier(attribute)
        )));
    }
    lines_query(&fields, &quoted_identifier(&view.name))
}

// ---------------------------------------------------------------------------
// The corpus file.
// ---------------------------------------------------------------------------

fn write_attributes(writer: &mut JsonWriter, attributes: &Attributes) {
    writer.array("attributes", |writer| {
        for (key, value) in attributes {
            writer.array_value(|writer| {
                writer.object(|writer| {
                    writer.field("key", key.as_str());
                    match value {
                        Value::Bool(value) => writer.field("bool", *value),
                        Value::Int(value) => writer.field("int", *value),
                        Value::Float(value) if value.is_finite() => writer.field("float", *value),
                        Value::Float(value) => {
                            writer.field("not_finite", not_finite_name(*value));
                        }
                        // JSON cannot carry a NUL through bc-json, so the corpus
                        // writes the text as its pieces between NULs, as it
                        // names a float JSON cannot hold.
                        Value::Text(value) if value.contains('\0') => {
                            let pieces: Vec<String> =
                                value.split('\0').map(str::to_owned).collect();
                            writer.string_array("nul_separated", &pieces);
                        }
                        Value::Text(value) => writer.field("string", value.as_str()),
                    }
                });
            });
        }
    });
}

fn write_observation(writer: &mut JsonWriter, observation: &Observation) {
    writer.field("producer", observation.producer.as_str());
    writer.field("sequence", observation.sequence);
}

/// The span a metric or a log was made in, when there was one.
fn write_within(writer: &mut JsonWriter, within: Option<&SpanIdentity>) {
    if let Some(span) = within {
        writer.nested_object("within", |writer| {
            writer.field("trace_id", span.trace_id.as_str());
            writer.field("span_id", span.span_id.as_str());
        });
    }
}

/// How the corpus names a float JSON cannot hold.
fn not_finite_name(value: f64) -> &'static str {
    if value.is_nan() {
        "nan"
    } else if value.is_sign_positive() {
        "infinity"
    } else {
        "-infinity"
    }
}

fn write_record(writer: &mut JsonWriter, record: &Record) {
    match record {
        Record::Span(span) => {
            writer.field("name", span.name.as_str());
            writer.field("trace_id", span.identity.trace_id.as_str());
            writer.field("span_id", span.identity.span_id.as_str());
            writer.optional_field("parent_span_id", span.parent_span_id.as_deref());
            writer.field("kind", span.kind);
            writer.field("status", span.status);
            writer.field("status_message", span.status_message.as_str());
            writer.field("start_ms", span.start_ms);
            writer.field("end_ms", span.end_ms);
            write_attributes(writer, &span.attributes);
            writer.array("events", |writer| {
                for event in &span.events {
                    writer.array_value(|writer| {
                        writer.object(|writer| {
                            writer.field("name", event.name.as_str());
                            writer.field("time_ms", event.time_ms);
                            write_attributes(writer, &event.attributes);
                        });
                    });
                }
            });
        }
        Record::Metric(metric) => {
            writer.field("name", metric.name.as_str());
            writer.field("kind", metric.kind);
            match &metric.value {
                Value::Bool(value) => writer.field("value_bool", *value),
                Value::Int(value) => writer.field("value_int", *value),
                Value::Float(value) if value.is_finite() => writer.field("value_float", *value),
                // JSON has no spelling for these, so the corpus names them.
                Value::Float(value) => writer.field("value_not_finite", not_finite_name(*value)),
                Value::Text(value) => writer.field("value_string", value.as_str()),
            }
            writer.field("time_ms", metric.time_ms);
            write_attributes(writer, &metric.attributes);
            write_observation(writer, &metric.observation);
            write_within(writer, metric.within.as_ref());
        }
        Record::Log(log) => {
            writer.field("time_ms", log.time_ms);
            writer.field("severity", log.severity);
            writer.field("body", log.body.as_str());
            write_attributes(writer, &log.attributes);
            write_observation(writer, &log.observation);
            write_within(writer, log.within.as_ref());
        }
    }
}

/// A fixed instant as the SQL that names it.
fn instant_sql(time_ms: i64, formatter: &DateFormatter) -> String {
    format!("'{}'::timestamptz", iso_time(formatter, time_ms))
}

/// The corpus for `tree`, as pretty-printed JSON.
///
/// - `setup`: statements that bring a database with the layout applied to the
///   state the batches assume;
/// - `statements`: what a writer runs per batch;
/// - `batches`: in order, each with its records, the lines a writer stages for
///   them, and the counts that must come back;
/// - `checks`: queries, and the rows each must return once every batch is in.
pub(crate) fn render(tree: &Tree) -> Result<String, JsonWriteError> {
    let formatter = DateFormatBuilder::iso8601();
    let mut store = Store::after_setup();

    // Everything is computed before anything is written, so a rendering error
    // is returned rather than buried in half a document.
    let batches = fixtures(tree);
    let mut rendered = Vec::new();
    for batch in &batches {
        rendered.push(run_batch(batch, &mut store, &formatter)?);
    }

    let shapes = views::shapes(tree);
    let mut writer = JsonWriter::pretty();
    writer.object(|writer| {
        writer.field("layout_version", LAYOUT_VERSION);
        writer.field("service_name", SERVICE_NAME);
        writer.string_array("setup", setup_statements(&formatter));
        write_statements(writer);
        write_batches(writer, &rendered);
        write_checks(writer, &store, &shapes);
    });
    let mut text = writer.finish()?;
    text.push('\n');
    Ok(text)
}

/// One batch through the reference writer and the reference ingest step.
fn run_batch<'batch>(
    batch: &'batch Batch,
    store: &mut Store,
    formatter: &DateFormatter,
) -> Result<RenderedBatch<'batch>, JsonWriteError> {
    let mut rows = Vec::new();
    let mut staged = Vec::new();
    let mut writer = WriterCounts::default();
    for record in &batch.records {
        let outcome = stage(record);
        writer.collisions += outcome.collisions;
        match outcome.row {
            Some(row) => {
                staged.push(staged_line(&row, formatter)?);
                rows.push(row);
            }
            None => writer.refused += 1,
        }
    }
    let ingest = store.ingest(batch.signal, rows);
    Ok(RenderedBatch {
        batch,
        staged,
        writer,
        ingest,
    })
}

/// What brings a database with the layout applied to the state the batches
/// assume: the partitions around the instant, and one customer erased.
fn setup_statements(formatter: &DateFormatter) -> [String; 2] {
    [
        format!(
            "SELECT * FROM telemetry.maintain({}, days_ahead => {DAYS_AHEAD}, \
             days_kept => {DAYS_KEPT})",
            instant_sql(INSTANT_MS, formatter)
        ),
        format!(
            "SELECT * FROM telemetry.erase_customers('{ORGANIZATION}', \
             ARRAY['{ERASED_CUSTOMER}'], {})",
            instant_sql(ERASED_AT_MS, formatter)
        ),
    ]
}

/// What a writer runs for a batch: open the staging tables, copy the batch
/// into its kind's, and ingest it.
fn write_statements(writer: &mut JsonWriter) {
    writer.nested_object("statements", |writer| {
        writer.field("open_staging", "SELECT telemetry.open_staging()");
        for signal in Signal::ALL {
            writer.nested_object(signal.table(), |writer| {
                writer.field(
                    "copy",
                    format!(
                        "COPY pg_temp.bc_telemetry_staged_{} ({}) FROM STDIN",
                        signal.table(),
                        signal.columns().join(", ")
                    ),
                );
                writer.field(
                    "ingest",
                    format!(
                        "SELECT inserted, out_of_range, erased, duplicate FROM \
                         telemetry.ingest_{}()",
                        signal.table()
                    ),
                );
            });
        }
    });
}

fn write_batches(writer: &mut JsonWriter, rendered: &[RenderedBatch<'_>]) {
    writer.array("batches", |writer| {
        for batch in rendered {
            writer.array_value(|writer| {
                writer.object(|writer| {
                    writer.field("name", batch.batch.name.as_str());
                    writer.field("kind", batch.batch.signal.table());
                    writer.array("records", |writer| {
                        for record in &batch.batch.records {
                            writer.array_value(|writer| {
                                writer.object(|writer| write_record(writer, record));
                            });
                        }
                    });
                    writer.string_array("staged", &batch.staged);
                    writer.nested_object("writer", |writer| {
                        writer.field("refused", batch.writer.refused);
                        writer.field("collisions", batch.writer.collisions);
                    });
                    writer.nested_object("counts", |writer| {
                        writer.field("inserted", batch.ingest.inserted);
                        writer.field("out_of_range", batch.ingest.out_of_range);
                        writer.field("erased", batch.ingest.erased);
                        writer.field("duplicate", batch.ingest.duplicate);
                    });
                });
            });
        }
    });
}

/// What the three tables and every view must hold once every batch is in.
fn write_checks(writer: &mut JsonWriter, store: &Store, shapes: &[ViewShape]) {
    writer.array("checks", |writer| {
        for signal in Signal::ALL {
            let mut rows: Vec<String> = store.rows(signal).map(table_check_line).collect();
            rows.sort();
            write_check(
                writer,
                &format!("telemetry.{}", signal.table()),
                &table_check_query(signal),
                &rows,
            );
        }
        for view in shapes {
            let signal = match view.kind {
                ItemKind::Span => Signal::Spans,
                ItemKind::Metric => Signal::Metrics,
                ItemKind::Log | ItemKind::Error => Signal::Logs,
            };
            let mut rows: Vec<String> = store
                .rows(signal)
                .filter_map(|row| view_check_line(view, row))
                .collect();
            rows.sort();
            write_check(
                writer,
                &format!("telemetry.{}", view.name),
                &view_check_query(view),
                &rows,
            );
        }
    });
}

/// One batch, with what the reference writer and the reference ingest step made
/// of it.
struct RenderedBatch<'batch> {
    batch: &'batch Batch,
    staged: Vec<String>,
    writer: WriterCounts,
    ingest: IngestCounts,
}

fn write_check(writer: &mut JsonWriter, name: &str, query: &str, rows: &[String]) {
    writer.array_value(|writer| {
        writer.object(|writer| {
            writer.field("name", name);
            writer.field("query", query);
            writer.string_array("rows", rows);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(entries: &[(&str, Value)]) -> BTreeMap<String, Value> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect()
    }

    fn text_value(value: &str) -> Value {
        Value::Text(value.to_string())
    }

    #[test]
    fn a_repeated_key_keeps_its_last_value_and_counts_what_it_replaced() {
        let attributes = vec![
            text("note", "first"),
            text("other", "kept"),
            text("note", "second"),
            text("note", "third"),
        ];
        let (object, collisions) = attribute_object(&attributes);
        assert_eq!(object.get("note"), Some(&text_value("third")));
        assert_eq!(object.get("other"), Some(&text_value("kept")));
        assert_eq!(collisions, 2);
    }

    #[test]
    fn a_declared_record_gives_its_module_name_and_version_to_columns() {
        let mut attributes = object(&[
            (VERSION_ATTRIBUTE, Value::Int(3)),
            (SERVICE_ATTRIBUTE, text_value("edge")),
            (ORGANIZATION_ATTRIBUTE, text_value("org")),
            (CUSTOMER_ATTRIBUTE, text_value("someone")),
            ("route", text_value("/cart")),
        ]);
        let lifted = lift(&mut attributes, "payments.checkout");
        assert_eq!(lifted.module.as_deref(), Some("payments"));
        assert_eq!(lifted.name, "checkout");
        assert_eq!(lifted.schema_version, Some(3));
        assert_eq!(lifted.service_name, "edge");
        assert_eq!(lifted.organization.as_deref(), Some("org"));
        assert_eq!(lifted.customer.as_deref(), Some("someone"));
        // What was lifted is a column now, and no longer an attribute.
        assert_eq!(attributes, object(&[("route", text_value("/cart"))]));
    }

    #[test]
    fn a_record_no_definition_declared_keeps_its_name_whole_and_its_attributes() {
        // No version: the dots are the name's own.
        let mut undeclared = object(&[("route", text_value("/health"))]);
        let lifted = lift(&mut undeclared, "http.server.request");
        assert_eq!(lifted.module, None);
        assert_eq!(lifted.name, "http.server.request");
        assert_eq!(lifted.schema_version, None);
        assert_eq!(lifted.service_name, SERVICE_NAME);

        // A version, and a name that is not `<module>.<item>`: not declared
        // either, and the version stays where it was written.
        for name in ["checkout", "a.b.c", ".checkout", "payments."] {
            let mut attributes = object(&[(VERSION_ATTRIBUTE, Value::Int(1))]);
            let lifted = lift(&mut attributes, name);
            assert_eq!(lifted.module, None, "{name}");
            assert_eq!(lifted.name, name);
            assert_eq!(lifted.schema_version, None, "{name}");
            assert!(attributes.contains_key(VERSION_ATTRIBUTE), "{name}");
        }

        // A version that is not an integer is not a version.
        let mut attributes = object(&[(VERSION_ATTRIBUTE, text_value("1"))]);
        assert_eq!(lift(&mut attributes, "payments.checkout").module, None);
    }

    #[test]
    fn only_text_is_lifted_as_a_name() {
        let mut attributes = object(&[(SERVICE_ATTRIBUTE, Value::Int(7))]);
        let lifted = lift(&mut attributes, "undeclared");
        assert_eq!(lifted.service_name, SERVICE_NAME);
        assert_eq!(attributes.get(SERVICE_ATTRIBUTE), Some(&Value::Int(7)));
    }

    fn metric(value: Value, sequence: u64) -> Record {
        Record::Metric(MetricRecord {
            name: "payments.requests".to_string(),
            kind: "counter",
            value,
            time_ms: INSTANT_MS,
            attributes: vec![(VERSION_ATTRIBUTE.to_string(), Value::Int(1))],
            observation: Observation {
                producer: PRODUCER.to_string(),
                sequence,
            },
            within: None,
        })
    }

    #[test]
    fn a_float_json_cannot_hold_is_named() {
        assert_eq!(not_finite_name(f64::NAN), "nan");
        assert_eq!(not_finite_name(f64::INFINITY), "infinity");
        assert_eq!(not_finite_name(f64::NEG_INFINITY), "-infinity");
    }

    #[test]
    fn a_metric_with_no_number_or_no_identity_is_refused() {
        assert!(stage(&metric(Value::Int(2), 1)).row.is_some());
        assert!(stage(&metric(Value::Float(0.5), 1)).row.is_some());
        assert!(stage(&metric(text_value("two"), 1)).row.is_none());
        assert!(stage(&metric(Value::Float(f64::NAN), 1)).row.is_none());
        assert!(stage(&metric(Value::Float(f64::INFINITY), 1)).row.is_none());
        assert!(stage(&metric(Value::Bool(true), 1)).row.is_some());
        assert!(stage(&metric(Value::Int(2), 0)).row.is_none());
        let no_producer = Record::Metric(MetricRecord {
            name: "payments.requests".to_string(),
            kind: "counter",
            value: Value::Int(1),
            time_ms: INSTANT_MS,
            attributes: Vec::new(),
            observation: Observation {
                producer: "0".repeat(32),
                sequence: 1,
            },
            within: None,
        });
        assert!(stage(&no_producer).row.is_none());
    }

    fn row_at(time_ms: i64, record_id: &str, customer: Option<(&str, Option<&str>)>) -> Row {
        Row {
            time_ms,
            record_id: record_id.to_string(),
            module: None,
            name: "undeclared".to_string(),
            schema_version: None,
            service_name: SERVICE_NAME.to_string(),
            organization: customer.and_then(|(_, organization)| organization.map(str::to_string)),
            customer: customer.map(|(customer, _)| customer.to_string()),
            trace_id: None,
            span_id: None,
            attributes: BTreeMap::new(),
            tail: Tail::Metric {
                kind: "counter",
                value: 1.0,
            },
        }
    }

    /// The four counts, one row at a time and each at its boundary.
    #[test]
    fn the_ingest_step_inserts_what_is_live_unerased_and_new() {
        let today = INSTANT_MS.div_euclid(MILLISECONDS_PER_DAY) * MILLISECONDS_PER_DAY;
        let first_kept = today - DAYS_KEPT * MILLISECONDS_PER_DAY;
        let past_the_last = today + (DAYS_AHEAD + 1) * MILLISECONDS_PER_DAY;
        let erased = Some((ERASED_CUSTOMER, Some(ORGANIZATION)));
        let mut store = Store::after_setup();

        let counts = store.ingest(
            Signal::Metrics,
            vec![
                // The first and last milliseconds that have a partition, and
                // the ones on either side.
                row_at(first_kept, "first-kept", None),
                row_at(first_kept - 1, "before", None),
                row_at(past_the_last - 1, "last-ahead", None),
                row_at(past_the_last, "after", None),
                // At the erasure and a millisecond past it.
                row_at(ERASED_AT_MS, "erased", erased),
                row_at(ERASED_AT_MS + 1, "came-back", erased),
                // The same customer name under another tenant, and under none.
                row_at(
                    ERASED_AT_MS,
                    "other-tenant",
                    Some((ERASED_CUSTOMER, Some("another"))),
                ),
                row_at(ERASED_AT_MS, "no-tenant", Some((ERASED_CUSTOMER, None))),
                // The same key twice in one batch.
                row_at(INSTANT_MS, "twice", None),
                row_at(INSTANT_MS, "twice", None),
                // The same id at another time is another record.
                row_at(INSTANT_MS + 1, "twice", None),
            ],
        );
        assert_eq!(
            counts,
            IngestCounts {
                inserted: 7,
                out_of_range: 2,
                erased: 1,
                duplicate: 1,
            }
        );
        // A second table is another table: the same keys are new to it.
        let other_table = store.ingest(Signal::Logs, vec![row_at(INSTANT_MS, "twice", None)]);
        assert_eq!(other_table.inserted, 1);
        // And a retry of what is stored is all duplicates.
        let retried = store.ingest(
            Signal::Metrics,
            vec![row_at(first_kept, "first-kept", None)],
        );
        assert_eq!(retried.duplicate, 1);
        assert_eq!(retried.inserted, 0);
    }

    #[test]
    fn a_staged_field_escapes_what_copy_reads_as_structure() {
        assert_eq!(copy_field(None), "\\N");
        assert_eq!(copy_field(Some("plain")), "plain");
        assert_eq!(copy_field(Some("")), "");
        assert_eq!(copy_field(Some("a\tb\nc\rd\\e")), "a\\tb\\nc\\rd\\\\e");
        // Text that spells a null is text: its backslash is escaped.
        assert_eq!(copy_field(Some("\\N")), "\\\\N");
    }

    #[test]
    fn a_checks_line_spells_numbers_as_a_float_prints_them() {
        let attributes = object(&[
            ("whole", Value::Float(2.0)),
            ("ratio", Value::Float(0.25)),
            ("count", Value::Int(-7)),
            ("flag", Value::Bool(false)),
            ("empty", text_value("")),
            ("unsayable", Value::Float(f64::NAN)),
        ]);
        assert_eq!(
            check_attributes(&attributes),
            "count=number:-7\u{1e}empty=string:\u{1e}flag=boolean:false\u{1e}ratio=number:0.25\
             \u{1e}unsayable=null:\u{1e}whole=number:2"
        );
    }
}
