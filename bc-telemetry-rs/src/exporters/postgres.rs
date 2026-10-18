//! The PostgreSQL exporter: a writer of the bc-telemetry storage layout.
//!
//! **New surface — the C++ original's exporter writes a layout of its own.**
//! Feature-gated (`postgres`) and off by default, so the driver's dependency
//! closure reaches only a consumer that asks for it.
//!
//! The layout belongs to `bc-telemetry-gen`, which renders its DDL, its ingest
//! functions and a conformance corpus (`crates/gen/README.md`, "The PostgreSQL
//! layout"). This exporter owns no DDL: a database is brought to the layout by
//! applying the generated `layout.sql` and `functions.sql`, which is
//! deployment's job and never a side effect of exporting. What it owns is the
//! writer's half, and the corpus holds it to the same behaviour as every other
//! writer.
//!
//! # Only to a database on this host
//!
//! No TLS stack is built into this exporter, so it writes only to a database
//! it reaches over a Unix socket or a loopback address: a record carries a
//! customer's identity, and a host anywhere else would receive it in the
//! clear. A connection string naming another host, or an `sslmode` asking for
//! TLS, is refused when the exporter first connects — every batch a counted
//! drop, its reason in [`PostgresExporter::last_failure`] — rather than
//! negotiated down to plaintext, which is what `sslmode=prefer`, the driver's
//! default, does silently.
//!
//! # A batch never waits long
//!
//! The session sets [`SESSION_SETTINGS`] once connected: a statement is bounded,
//! and a lock it waits on is bounded more tightly, so a batch behind
//! `telemetry.maintain()`'s partition work fails and is counted rather than
//! holding the writer — and, through the client's mutex, every recording thread
//! that shares it.
//!
//! # One batch, one ingest
//!
//! Each export call is one batch, written in one transaction: the session's
//! staging tables are opened (`telemetry.open_staging()`), the batch is staged
//! with one `COPY`, and the kind's generated ingest function moves it into the
//! table. That function holds the rules every writer shares — a row whose day
//! has no partition, a row about a customer erased after it was made, a row
//! already stored — and answers with what it did. [`PostgresBatchOutcome`] is
//! that answer, beside what this writer refused before staging.
//!
//! # What a record becomes
//!
//! A row's columns are lifted out of the record's attributes:
//!
//! - `service.name`, `organization.identifier` and `customer.id`, when they are
//!   text, become `service_name`, `organization` and `customer`; a record with
//!   no service name is filed under the one this exporter was built with.
//! - A record whose name is `<module>.<item>` and which carries an integer
//!   `bc.telemetry.schema_version` is *declared*: its module, item name and
//!   version become columns. A log carries its name in `bc.telemetry.log`,
//!   since its body is its description. Any other record keeps its name whole,
//!   under a null module, with whatever it carried left in its attributes.
//! - What remains is an object, each key once. A record that repeats a key
//!   keeps the last value, and each value replaced is counted as a drop.
//!
//! A metric point whose value is no number — text, or a float that is not
//! finite — is refused, as is a point or a log with no valid
//! [`ObservationId`]: its record id is what lets a retried batch store it once.
//! So is any record whose text holds a NUL — its name, a message or body, an
//! attribute's key or text value, an event's — since no PostgreSQL `text` or
//! `jsonb` value can store one. It is refused whole rather than altered: one
//! record costs one record, never the batch a `COPY` would fail, and a
//! customer's id is never rewritten into another customer's.
//!
//! # Failure is a drop, not an error
//!
//! The [`TelemetryExporter`] methods are infallible by signature, like every
//! exporter's here: a closed connection, a refused statement, a database
//! without the layout — each is a counted drop of the whole batch, and
//! [`PostgresExporter::last_failure`] says why. A process does not stop working
//! because its telemetry database is down. The `write_*` methods are the same
//! work with the outcome returned, for a caller that wants it.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use bc_date_time::{DateFormatBuilder, DateFormatter, Epoch};
use bc_json::{JsonWriteError, JsonWriter};
use postgres::config::{Config, Host, SslMode};
use postgres::{Client, NoTls};

use crate::exporter::TelemetryExporter;
use crate::exporters::{metric_kind_name, numeric_of, span_kind_name, status_name};
use crate::log_record::{LogRecord, Severity};
use crate::metrics::MetricPoint;
use crate::types::{AttributeValue, Attributes, ObservationId, Span, SpanContext, SpanEvent};

// The attributes a writer lifts into columns, under the names
// bourgeois_concern's runtime gives them.
const SERVICE_ATTRIBUTE: &str = "service.name";
const ORGANIZATION_ATTRIBUTE: &str = "organization.identifier";
const CUSTOMER_ATTRIBUTE: &str = "customer.id";
// The reserved attributes a generated emitter appends.
const VERSION_ATTRIBUTE: &str = "bc.telemetry.schema_version";
const LOG_NAME_ATTRIBUTE: &str = "bc.telemetry.log";

/// What one batch came to: what this writer refused or lost before staging,
/// and the ingest step's four counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PostgresBatchOutcome {
    /// Records not staged: a metric point whose value is no number, or a point
    /// or a log with no valid observation id.
    pub refused: u64,
    /// Attribute values replaced by a later value under the same key.
    pub collisions: u64,
    /// Rows stored.
    pub inserted: u64,
    /// Rows whose UTC day has no partition.
    pub out_of_range: u64,
    /// Rows about a customer erased at or after the row's time.
    pub erased: u64,
    /// Rows already stored — a batch retried after an unknown commit outcome.
    /// Not a drop: the record is stored once.
    pub duplicate: u64,
}

impl PostgresBatchOutcome {
    /// What this batch dropped: every record refused or not stored, and every
    /// attribute value a repeated key replaced.
    #[must_use]
    pub const fn dropped(&self) -> u64 {
        self.refused
            .saturating_add(self.collisions)
            .saturating_add(self.out_of_range)
            .saturating_add(self.erased)
    }
}

/// Why a batch was not written.
///
/// Whether any of it was stored is unknown only when the commit itself failed;
/// writing the batch again is safe either way, since the ingest step stores
/// each record once.
#[derive(Debug)]
pub struct PostgresWriteError {
    /// What was being done, as the message says it: "could not {step}".
    step: &'static str,
    cause: Box<dyn std::error::Error + Send + Sync>,
}

impl PostgresWriteError {
    fn at(step: &'static str, cause: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            step,
            cause: Box::new(cause),
        }
    }
}

impl std::fmt::Display for PostgresWriteError {
    /// The step, then the cause and every error under it: the driver's own
    /// error says only "db error", and the server's words — "canceling
    /// statement due to lock timeout" — are its source, which a reader of
    /// [`PostgresExporter::last_failure`] would otherwise never see.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "could not {}: {}", self.step, self.cause)?;
        let mut under = self.cause.source();
        while let Some(source) = under {
            write!(formatter, ": {source}")?;
            under = source.source();
        }
        Ok(())
    }
}

impl std::error::Error for PostgresWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

/// The ingest step answered with a count below zero, which no count is.
#[derive(Debug)]
struct NegativeCount(i64);

impl std::fmt::Display for NegativeCount {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the ingest step answered with the count {}",
            self.0
        )
    }
}

impl std::error::Error for NegativeCount {}

/// The three tables a batch can be for.
#[derive(Clone, Copy, Debug)]
enum Table {
    Spans,
    Metrics,
    Logs,
}

impl Table {
    /// The statement that stages a batch: the table's columns, in the order a
    /// staged line lists them.
    const fn copy_statement(self) -> &'static str {
        match self {
            Self::Spans => {
                "COPY pg_temp.bc_telemetry_staged_spans (time, record_id, module, name, \
                 schema_version, service_name, organization, customer, trace_id, span_id, \
                 attributes, end_time, parent_span_id, kind, status, status_message, events) \
                 FROM STDIN"
            }
            Self::Metrics => {
                "COPY pg_temp.bc_telemetry_staged_metrics (time, record_id, module, name, \
                 schema_version, service_name, organization, customer, trace_id, span_id, \
                 attributes, kind, value) FROM STDIN"
            }
            Self::Logs => {
                "COPY pg_temp.bc_telemetry_staged_logs (time, record_id, module, name, \
                 schema_version, service_name, organization, customer, trace_id, span_id, \
                 attributes, severity, body) FROM STDIN"
            }
        }
    }

    const fn ingest_statement(self) -> &'static str {
        match self {
            Self::Spans => {
                "SELECT inserted, out_of_range, erased, duplicate FROM telemetry.ingest_spans()"
            }
            Self::Metrics => {
                "SELECT inserted, out_of_range, erased, duplicate FROM telemetry.ingest_metrics()"
            }
            Self::Logs => {
                "SELECT inserted, out_of_range, erased, duplicate FROM telemetry.ingest_logs()"
            }
        }
    }
}

/// Writes telemetry into a database that holds the bc-telemetry layout.
pub struct PostgresExporter {
    client: Mutex<Option<Client>>,
    connection_string: String,
    service_name: String,
    formatter: DateFormatter,
    dropped: AtomicU64,
    last_failure: Mutex<Option<String>>,
}

impl std::fmt::Debug for PostgresExporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The connection string carries a password; it is never printed.
        formatter
            .debug_struct("PostgresExporter")
            .field("service_name", &self.service_name)
            .field("dropped", &self.dropped.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// What every session this exporter opens sets before its first batch: a
/// statement may run 15 seconds and wait 5 for a lock, so a stalled database
/// costs one counted batch, never a writer held indefinitely.
const SESSION_SETTINGS: &str = "SET statement_timeout = '15s'; SET lock_timeout = '5s'";

/// A connection this exporter will not make: one that would carry records off
/// this host in the clear, or ask for the TLS this exporter does not build.
#[derive(Debug)]
struct TransportRefused(String);

impl std::fmt::Display for TransportRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for TransportRefused {}

/// Why `config` names a connection this exporter refuses, if it does: TLS
/// asked for, or a host that is neither a Unix socket nor a loopback address.
fn refused_transport(config: &Config) -> Option<TransportRefused> {
    match config.get_ssl_mode() {
        SslMode::Disable | SslMode::Prefer => {}
        // `Require`, and any stronger mode a later driver adds.
        _ => {
            return Some(TransportRefused(
                "the connection string asks for TLS, which this exporter does not build; it \
                 writes only to a database on this host, over a Unix socket or a loopback address"
                    .to_owned(),
            ));
        }
    }
    let off_this_host = |host: &str| {
        TransportRefused(format!(
            "`{host}` is not this host, and with no TLS built a record would cross the network in \
             the clear; this exporter writes only over a Unix socket or a loopback address"
        ))
    };
    for host in config.get_hosts() {
        match host {
            #[cfg(unix)]
            Host::Unix(_) => {}
            Host::Tcp(name) => {
                if !is_loopback_name(name) {
                    return Some(off_this_host(name));
                }
            }
        }
    }
    for address in config.get_hostaddrs() {
        if !address.is_loopback() {
            return Some(off_this_host(&address.to_string()));
        }
    }
    None
}

/// Whether a TCP host names this machine: `localhost`, or a loopback address.
fn is_loopback_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("localhost")
        || name
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// A session to the database `connection_string` names, its timeouts set, when
/// it is one this exporter may write to.
fn connect(connection_string: &str) -> Result<Client, PostgresWriteError> {
    let config: Config = connection_string
        .parse()
        .map_err(|error| PostgresWriteError::at("read the connection string", error))?;
    if let Some(refusal) = refused_transport(&config) {
        return Err(PostgresWriteError::at("connect to the database", refusal));
    }
    let mut client = config
        .connect(NoTls)
        .map_err(|error| PostgresWriteError::at("connect to the database", error))?;
    client
        .batch_execute(SESSION_SETTINGS)
        .map_err(|error| PostgresWriteError::at("bound the session's waits", error))?;
    Ok(client)
}

/// Seconds allowed for a connection attempt when the connection string names
/// no limit.
///
/// Short on purpose: this is a telemetry sink, and a caller waiting on it is
/// already paying for instrumentation it did not ask to block on.
const CONNECT_TIMEOUT_SECONDS: u32 = 5;

/// Ensures the connection string carries a `connect_timeout`.
///
/// Without one, `Client::connect` blocks for the operating system's TCP connect
/// timeout — around 75 seconds on Linux and macOS — whenever the host *drops*
/// packets rather than refusing them, which is what a firewall or a dead route
/// does. It blocks while holding the mutex every other recording thread needs,
/// so one partitioned database stalls the whole process. A caller who names
/// their own value keeps it.
fn with_connect_timeout(connection_string: String) -> String {
    if connection_string.contains("connect_timeout") {
        return connection_string;
    }
    // Two spellings are valid, and they take different separators: a URI
    // (`postgresql://host/db?k=v`) and libpq keyword/value pairs
    // (`host=... dbname=...`).
    if connection_string.starts_with("postgres://")
        || connection_string.starts_with("postgresql://")
    {
        let separator = if connection_string.contains('?') {
            '&'
        } else {
            '?'
        };
        format!("{connection_string}{separator}connect_timeout={CONNECT_TIMEOUT_SECONDS}")
    } else {
        format!("{connection_string} connect_timeout={CONNECT_TIMEOUT_SECONDS}")
    }
}

impl PostgresExporter {
    /// An exporter for the database `connection_string` names, filing a record
    /// that carries no `service.name` under `service_name`.
    ///
    /// Connects lazily: nothing is contacted until something is exported.
    /// Deliberately not a fallible constructor. A telemetry sink that refuses to
    /// build when the database is briefly unreachable would make installing
    /// instrumentation a failure path in `main`.
    #[must_use]
    pub fn new(connection_string: impl Into<String>, service_name: impl Into<String>) -> Self {
        Self {
            client: Mutex::new(None),
            connection_string: with_connect_timeout(connection_string.into()),
            service_name: service_name.into(),
            formatter: DateFormatBuilder::iso8601(),
            dropped: AtomicU64::new(0),
            last_failure: Mutex::new(None),
        }
    }

    /// Why the most recent batch that failed was not written, if one has.
    ///
    /// Every failure is also a counted drop. This is the reason beside the
    /// count, so a reader who sees drops climbing can find out why without
    /// attaching a debugger to a process that is working as designed.
    #[must_use]
    pub fn last_failure(&self) -> Option<String> {
        lock(&self.last_failure).clone()
    }

    /// Writes `spans` as one batch.
    ///
    /// # Errors
    ///
    /// The batch could not be written: no connection, or a statement the
    /// database refused — a database without the layout among them.
    pub fn write_spans(&self, spans: &[Span]) -> Result<PostgresBatchOutcome, PostgresWriteError> {
        let mut staged = Staged::default();
        for span in spans {
            match self.span_line(span).map_err(encoding)? {
                Some(line) => staged.add(&line),
                None => staged.refused += 1,
            }
        }
        self.write(Table::Spans, &staged)
    }

    /// Writes `metrics` as one batch.
    ///
    /// # Errors
    ///
    /// As [`PostgresExporter::write_spans`].
    pub fn write_metrics(
        &self,
        metrics: &[MetricPoint],
    ) -> Result<PostgresBatchOutcome, PostgresWriteError> {
        let mut staged = Staged::default();
        for point in metrics {
            match self.metric_line(point).map_err(encoding)? {
                Some(line) => staged.add(&line),
                None => staged.refused += 1,
            }
        }
        self.write(Table::Metrics, &staged)
    }

    /// Writes `logs` as one batch.
    ///
    /// # Errors
    ///
    /// As [`PostgresExporter::write_spans`].
    pub fn write_logs(
        &self,
        logs: &[LogRecord],
    ) -> Result<PostgresBatchOutcome, PostgresWriteError> {
        let mut staged = Staged::default();
        for log in logs {
            match self.log_line(log).map_err(encoding)? {
                Some(line) => staged.add(&line),
                None => staged.refused += 1,
            }
        }
        self.write(Table::Logs, &staged)
    }

    /// Stages what `staged` holds and ingests it, in one transaction.
    fn write(
        &self,
        table: Table,
        staged: &Staged,
    ) -> Result<PostgresBatchOutcome, PostgresWriteError> {
        let mut outcome = PostgresBatchOutcome {
            refused: staged.refused,
            collisions: staged.collisions,
            ..PostgresBatchOutcome::default()
        };
        if staged.lines.is_empty() {
            // Everything was refused: no row to stage, so nothing to ask of the
            // database.
            return Ok(outcome);
        }
        let mut slot = lock(&self.client);
        let client = match &mut *slot {
            Some(client) => client,
            empty => empty.insert(connect(&self.connection_string)?),
        };
        let ingested = ingest(client, table, &staged.lines);
        // A broken connection stays broken until it is dropped, so a failed
        // batch retires the client and the next one reconnects.
        if client.is_closed() {
            *slot = None;
        }
        let [inserted, out_of_range, erased, duplicate] = ingested?;
        outcome.inserted = inserted;
        outcome.out_of_range = out_of_range;
        outcome.erased = erased;
        outcome.duplicate = duplicate;
        Ok(outcome)
    }

    /// Counts what a batch dropped, and remembers why a failed one failed.
    fn settle(&self, records: usize, written: Result<PostgresBatchOutcome, PostgresWriteError>) {
        let dropped = match written {
            Ok(outcome) => outcome.dropped(),
            Err(error) => {
                *lock(&self.last_failure) = Some(error.to_string());
                records as u64
            }
        };
        self.dropped.fetch_add(dropped, Ordering::Relaxed);
    }

    fn time(&self, instant: Epoch) -> String {
        self.formatter.format(instant)
    }

    /// The line for `span`, or `None` when it is refused.
    fn span_line(&self, span: &Span) -> Result<Option<StagedLine>, JsonWriteError> {
        let texts = [
            self.service_name.as_str(),
            span.name.as_str(),
            span.status_message.as_str(),
        ]
        .into_iter()
        .chain(attribute_texts(&span.attributes))
        .chain(span.events.iter().flat_map(|event| {
            std::iter::once(event.name.as_str()).chain(attribute_texts(&event.attributes))
        }));
        if holds_nul(texts) {
            return Ok(None);
        }
        let (mut object, mut collisions) = attribute_object(&span.attributes);
        let lifted = lift(&mut object, &span.name, &self.service_name);
        let mut events = JsonWriter::new();
        events.bare_array(|events| {
            for event in &span.events {
                collisions += self.write_event(events, event);
            }
        });
        let record_id = format!(
            "{}-{}",
            span.context.trace_id.to_hex(),
            span.context.span_id.to_hex()
        );
        let mut fields = self.common_fields(
            span.start_time,
            &record_id,
            &lifted,
            Some(&span.context),
            &object,
        )?;
        let parent_span_id = span.parent_span_id.map(|parent| parent.to_hex());
        fields.push(copy_field(Some(&self.time(span.end_time))));
        fields.push(copy_field(parent_span_id.as_deref()));
        fields.push(copy_field(Some(span_kind_name(span.kind))));
        fields.push(copy_field(Some(status_name(span.status))));
        fields.push(copy_field(Some(&span.status_message)));
        fields.push(copy_field(Some(&events.finish()?)));
        Ok(Some(StagedLine { fields, collisions }))
    }

    /// Writes one event as an element of the `events` array, and says how many
    /// of its attribute values a repeated key replaced.
    fn write_event(&self, events: &mut JsonWriter, event: &SpanEvent) -> u64 {
        let (attributes, collisions) = attribute_object(&event.attributes);
        events.array_value(|events| {
            events.object(|events| {
                events.field("name", event.name.as_str());
                events.field("time", self.time(event.timestamp));
                events.nested_object("attributes", |events| {
                    write_attribute_fields(events, &attributes);
                });
            });
        });
        collisions
    }

    /// The line for `point`, or `None` when it is refused.
    fn metric_line(&self, point: &MetricPoint) -> Result<Option<StagedLine>, JsonWriteError> {
        // A float that is not finite is no measurement, and the column would
        // take it: every sum over the item would then be one too.
        let value = numeric_of(&point.value).filter(|value| value.is_finite());
        let (Some(value), true) = (value, point.observation_id.is_valid()) else {
            return Ok(None);
        };
        let texts = [self.service_name.as_str(), point.name.as_str()]
            .into_iter()
            .chain(attribute_texts(&point.attributes));
        if holds_nul(texts) {
            return Ok(None);
        }
        let (mut object, collisions) = attribute_object(&point.attributes);
        let lifted = lift(&mut object, &point.name, &self.service_name);
        let mut fields = self.common_fields(
            point.timestamp,
            &record_id_of(point.observation_id),
            &lifted,
            point.span_context.as_ref(),
            &object,
        )?;
        fields.push(copy_field(Some(metric_kind_name(point.kind))));
        fields.push(copy_field(Some(&value.to_string())));
        Ok(Some(StagedLine { fields, collisions }))
    }

    /// The line for `log`, or `None` when it is refused.
    fn log_line(&self, log: &LogRecord) -> Result<Option<StagedLine>, JsonWriteError> {
        let texts = [self.service_name.as_str(), log.body.as_str()]
            .into_iter()
            .chain(attribute_texts(&log.attributes));
        if !log.observation_id.is_valid() || holds_nul(texts) {
            return Ok(None);
        }
        let (mut object, collisions) = attribute_object(&log.attributes);
        let wire_name = match object.get(LOG_NAME_ATTRIBUTE) {
            Some(AttributeValue::String(name)) => name.clone(),
            _ => String::new(),
        };
        let lifted = lift(&mut object, &wire_name, &self.service_name);
        if lifted.module.is_some() {
            object.remove(LOG_NAME_ATTRIBUTE);
        }
        let mut fields = self.common_fields(
            log.timestamp,
            &record_id_of(log.observation_id),
            &lifted,
            log.span_context.as_ref(),
            &object,
        )?;
        fields.push(copy_field(Some(severity_name(log.severity))));
        fields.push(copy_field(Some(&log.body)));
        Ok(Some(StagedLine { fields, collisions }))
    }

    /// The columns every table carries, staged.
    fn common_fields(
        &self,
        time: Epoch,
        record_id: &str,
        lifted: &Lifted,
        within: Option<&SpanContext>,
        attributes: &BTreeMap<&str, &AttributeValue>,
    ) -> Result<Vec<String>, JsonWriteError> {
        let schema_version = lifted.schema_version.map(|version| version.to_string());
        let trace_id = within.map(|context| context.trace_id.to_hex());
        let span_id = within.map(|context| context.span_id.to_hex());
        Ok(vec![
            copy_field(Some(&self.time(time))),
            copy_field(Some(record_id)),
            copy_field(lifted.module.as_deref()),
            copy_field(Some(&lifted.name)),
            copy_field(schema_version.as_deref()),
            copy_field(Some(&lifted.service_name)),
            copy_field(lifted.organization.as_deref()),
            copy_field(lifted.customer.as_deref()),
            copy_field(trace_id.as_deref()),
            copy_field(span_id.as_deref()),
            copy_field(Some(&attributes_json(attributes)?)),
        ])
    }
}

/// Opens the staging tables, stages `lines`, ingests them and commits, and
/// returns the ingest step's counts: inserted, out of range, erased, duplicate.
fn ingest(client: &mut Client, table: Table, lines: &str) -> Result<[u64; 4], PostgresWriteError> {
    let mut transaction = client
        .transaction()
        .map_err(|error| PostgresWriteError::at("begin a transaction", error))?;
    transaction
        .batch_execute("SELECT telemetry.open_staging()")
        .map_err(|error| PostgresWriteError::at("open the staging tables", error))?;
    let mut copy = transaction
        .copy_in(table.copy_statement())
        .map_err(|error| PostgresWriteError::at("stage the batch", error))?;
    copy.write_all(lines.as_bytes())
        .map_err(|error| PostgresWriteError::at("stage the batch", error))?;
    copy.finish()
        .map_err(|error| PostgresWriteError::at("stage the batch", error))?;
    let row = transaction
        .query_one(table.ingest_statement(), &[])
        .map_err(|error| PostgresWriteError::at("ingest the batch", error))?;
    let mut counts = [0_u64; 4];
    for (column, count) in counts.iter_mut().enumerate() {
        let value: i64 = row
            .try_get(column)
            .map_err(|error| PostgresWriteError::at("read the ingest step's counts", error))?;
        *count = u64::try_from(value).map_err(|_| {
            PostgresWriteError::at("read the ingest step's counts", NegativeCount(value))
        })?;
    }
    transaction
        .commit()
        .map_err(|error| PostgresWriteError::at("commit the batch", error))?;
    Ok(counts)
}

fn encoding(error: JsonWriteError) -> PostgresWriteError {
    PostgresWriteError::at("encode the batch", error)
}

/// Takes a lock, whether or not a thread panicked holding it: what each lock
/// here guards — a connection slot, a message — is whole after any panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One record's staged fields, and how many of its attribute values a repeated
/// key replaced.
struct StagedLine {
    fields: Vec<String>,
    collisions: u64,
}

/// A batch on its way to the staging table.
#[derive(Default)]
struct Staged {
    /// `COPY`'s text: one line per row, fields separated by tabs.
    lines: String,
    refused: u64,
    collisions: u64,
}

impl Staged {
    fn add(&mut self, line: &StagedLine) {
        self.lines.push_str(&line.fields.join("\t"));
        self.lines.push('\n');
        self.collisions += line.collisions;
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

/// Whether any of `texts` holds a NUL, which no PostgreSQL `text` or `jsonb`
/// value can store: `COPY` refuses the whole batch over one in a column, and
/// the JSON writer would replace one in an attribute with U+FFFD. The record is
/// refused instead — never altered.
fn holds_nul<'text>(mut texts: impl Iterator<Item = &'text str>) -> bool {
    texts.any(|text| text.contains('\0'))
}

/// Every text an attribute list carries: each key, and each value that is
/// text.
fn attribute_texts(attributes: &Attributes) -> impl Iterator<Item = &str> {
    attributes.iter().flat_map(|(key, value)| {
        let text = match value {
            AttributeValue::String(text) => Some(text.as_str()),
            AttributeValue::Boolean(_) | AttributeValue::Int64(_) | AttributeValue::Float64(_) => {
                None
            }
        };
        std::iter::once(key.as_str()).chain(text)
    })
}

/// An object from attributes: each key once, the last value written under it,
/// and how many values were replaced on the way.
fn attribute_object(attributes: &Attributes) -> (BTreeMap<&str, &AttributeValue>, u64) {
    let mut object = BTreeMap::new();
    let mut collisions = 0;
    for (key, value) in attributes {
        if object.insert(key.as_str(), value).is_some() {
            collisions += 1;
        }
    }
    (object, collisions)
}

/// Takes a text attribute out of `object`, leaving any other type where it is:
/// only text is ever a service's, a tenant's or a customer's name.
fn lift_text(object: &mut BTreeMap<&str, &AttributeValue>, key: &str) -> Option<String> {
    let Some(AttributeValue::String(text)) = object.get(key) else {
        return None;
    };
    let text = text.clone();
    object.remove(key);
    Some(text)
}

/// Lifts identity and tenancy out of `object`. `wire_name` is the name the
/// record carries: a span's or a point's own, a log's reserved attribute.
fn lift(
    object: &mut BTreeMap<&str, &AttributeValue>,
    wire_name: &str,
    service_name: &str,
) -> Lifted {
    let lifted_service_name = lift_text(object, SERVICE_ATTRIBUTE);
    let organization = lift_text(object, ORGANIZATION_ATTRIBUTE);
    let customer = lift_text(object, CUSTOMER_ATTRIBUTE);

    let version = match object.get(VERSION_ATTRIBUTE) {
        Some(AttributeValue::Int64(version)) => Some(*version),
        _ => None,
    };
    // `<module>.<item>`: exactly one dot, with a name on each side of it.
    let qualified = wire_name
        .split_once('.')
        .filter(|(module, item)| !module.is_empty() && !item.is_empty() && !item.contains('.'));
    let (module, name, schema_version) = match (version, qualified) {
        (Some(version), Some((module, item))) => {
            object.remove(VERSION_ATTRIBUTE);
            (Some(module.to_owned()), item.to_owned(), Some(version))
        }
        _ => (None, wire_name.to_owned(), None),
    };
    Lifted {
        module,
        name,
        schema_version,
        service_name: match lifted_service_name {
            Some(lifted) => lifted,
            None => service_name.to_owned(),
        },
        organization,
        customer,
    }
}

/// A point's or a log's record id: its producer, then that producer's count.
fn record_id_of(observation: ObservationId) -> String {
    format!("{}-{}", observation.producer.to_hex(), observation.sequence)
}

/// A log's severity as the layout stores it: the definition file's lowercase
/// words, not the runtime's labels.
const fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Debug => "debug",
        Severity::Info => "info",
        Severity::Warn => "warn",
        Severity::Error => "error",
        Severity::Fatal => "fatal",
    }
}

/// An attribute object's fields. A float that is not finite has no JSON
/// spelling and is written as null.
fn write_attribute_fields(json: &mut JsonWriter, attributes: &BTreeMap<&str, &AttributeValue>) {
    for (key, value) in attributes {
        match value {
            AttributeValue::Boolean(value) => json.field(key, *value),
            AttributeValue::Int64(value) => json.field(key, *value),
            AttributeValue::Float64(value) if value.is_finite() => json.field(key, *value),
            AttributeValue::Float64(_) => json.field_null(key),
            AttributeValue::String(value) => json.field(key, value.as_str()),
        }
    }
}

/// The `attributes` column: an object, each key once.
fn attributes_json(attributes: &BTreeMap<&str, &AttributeValue>) -> Result<String, JsonWriteError> {
    let mut json = JsonWriter::new();
    json.object(|json| write_attribute_fields(json, attributes));
    json.finish()
}

/// One field of a staged line, in PostgreSQL's `COPY` text format: a backslash
/// introduces an escape, a tab ends the field, a newline ends the row, and a
/// null is `\N`.
fn copy_field(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "\\N".to_owned();
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

impl TelemetryExporter for PostgresExporter {
    fn export_spans(&self, spans: &[Span]) {
        if spans.is_empty() {
            return;
        }
        self.settle(spans.len(), self.write_spans(spans));
    }

    fn export_metrics(&self, metrics: &[MetricPoint]) {
        if metrics.is_empty() {
            return;
        }
        self.settle(metrics.len(), self.write_metrics(metrics));
    }

    fn export_logs(&self, logs: &[LogRecord]) {
        if logs.is_empty() {
            return;
        }
        self.settle(logs.len(), self.write_logs(logs));
    }

    /// Each batch is written as it arrives, so nothing is buffered here.
    fn flush(&self) {}

    fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Config, NegativeCount, PostgresWriteError, copy_field, refused_transport,
        with_connect_timeout,
    };

    /// What `connection_string` is refused for, if it is.
    fn refusal(connection_string: &str) -> Option<String> {
        let config: Config = connection_string
            .parse()
            .unwrap_or_else(|error| panic!("`{connection_string}` parses: {error}"));
        refused_transport(&config).map(|refused| refused.to_string())
    }

    /// With no TLS built, the exporter writes to a database on this host and to
    /// no other: a Unix socket or a loopback address, in either spelling of a
    /// connection string, and never one that asks for TLS it cannot give.
    #[test]
    fn only_a_socket_or_a_loopback_address_is_written_to() {
        for allowed in [
            "host=/var/run/postgresql dbname=telemetry",
            "postgresql://writer@localhost/telemetry",
            "postgresql://writer@127.0.0.1:5432/telemetry",
            "postgresql://writer@[::1]/telemetry",
            "host=localhost hostaddr=127.0.0.1 dbname=telemetry",
            "postgresql://writer@localhost/telemetry?sslmode=disable",
        ] {
            assert_eq!(refusal(allowed), None, "{allowed}");
        }
        for (refused, says) in [
            (
                "postgresql://writer@db.example/telemetry",
                "`db.example` is not this host",
            ),
            (
                "host=192.0.2.10 dbname=telemetry",
                "`192.0.2.10` is not this host",
            ),
            (
                "host=localhost hostaddr=192.0.2.10 dbname=telemetry",
                "`192.0.2.10` is not this host",
            ),
            (
                "host=/var/run/postgresql,db.example dbname=telemetry",
                "`db.example` is not this host",
            ),
            (
                "postgresql://writer@localhost/telemetry?sslmode=require",
                "asks for TLS",
            ),
        ] {
            let found = refusal(refused);
            assert!(
                found.as_deref().is_some_and(|text| text.contains(says)),
                "{refused}: {found:?}"
            );
        }
    }

    #[test]
    fn a_write_error_says_what_failed_and_keeps_its_cause() {
        let error = PostgresWriteError::at("read the ingest step's counts", NegativeCount(-1));
        assert_eq!(
            error.to_string(),
            "could not read the ingest step's counts: the ingest step answered with the count -1"
        );
        let cause = std::error::Error::source(&error).expect("the cause is kept");
        assert_eq!(
            cause.to_string(),
            "the ingest step answered with the count -1"
        );
    }

    #[test]
    fn a_uri_gains_a_timeout_with_the_right_separator() {
        assert_eq!(
            with_connect_timeout("postgresql://h/db".to_owned()),
            "postgresql://h/db?connect_timeout=5"
        );
        assert_eq!(
            with_connect_timeout("postgresql://h/db?sslmode=require".to_owned()),
            "postgresql://h/db?sslmode=require&connect_timeout=5"
        );
    }

    #[test]
    fn keyword_value_pairs_gain_a_space_separated_timeout() {
        assert_eq!(
            with_connect_timeout("host=h dbname=db".to_owned()),
            "host=h dbname=db connect_timeout=5"
        );
    }

    #[test]
    fn a_caller_supplied_timeout_is_left_alone() {
        for supplied in [
            "postgresql://h/db?connect_timeout=30",
            "host=h dbname=db connect_timeout=30",
        ] {
            assert_eq!(with_connect_timeout(supplied.to_owned()), supplied);
        }
    }

    #[test]
    fn a_staged_field_escapes_what_copy_reads_as_structure() {
        assert_eq!(copy_field(None), "\\N");
        assert_eq!(copy_field(Some("\\N")), "\\\\N");
        assert_eq!(copy_field(Some("a\tb\nc\rd\\e")), "a\\tb\\nc\\rd\\\\e");
        assert_eq!(copy_field(Some("")), "");
    }
}
