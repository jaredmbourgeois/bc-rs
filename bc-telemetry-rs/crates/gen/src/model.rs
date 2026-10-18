//! The model a tree of definitions lowers into: the vocabulary this crate's
//! README describes under "The definition file".
//!
//! Every adapter, dialect and export receives this model and never the TOML or
//! JSON it came from. The kinds a metric or a log can be are
//! bc-telemetry-rs's own `MetricKind` and `Severity`, so the vocabulary cannot
//! name a kind the runtime lacks; everything else here is a declaration, which
//! the runtime has no type for.
//!
//! Maps are keyed by name, so every consumer iterates modules and items in
//! name order whatever order the files declared them in — that is what makes
//! generation deterministic rather than merely repeatable.

use std::collections::{BTreeMap, BTreeSet};

use bc_telemetry::{MetricKind, Severity, SpanKind};

use crate::source::{Span, Spanned};

/// Every module found under the root, by name.
#[derive(Debug, Default)]
pub(crate) struct Tree {
    pub(crate) modules: BTreeMap<String, Module>,
}

/// One module: a directory's definition, merged across its files.
#[derive(Debug)]
pub(crate) struct Module {
    /// The module's directory relative to the root's, `/`-separated; `.` is the
    /// root's own.
    pub(crate) directory: String,
    /// Where its definition file names it, for a finding about the module as a
    /// whole.
    pub(crate) name_span: Span,
    /// Spanned so a breaking change can point at the number it failed to bump.
    pub(crate) schema_version: Spanned<u32>,
    pub(crate) items: Items,
}

/// Everything a module declares, by kind and then by name.
#[derive(Debug, Default)]
pub(crate) struct Items {
    pub(crate) spans: BTreeMap<String, SpanDeclaration>,
    pub(crate) metrics: BTreeMap<String, MetricDeclaration>,
    pub(crate) logs: BTreeMap<String, LogDeclaration>,
    pub(crate) errors: BTreeMap<String, ErrorDeclaration>,
    pub(crate) queries: BTreeMap<String, Query>,
    pub(crate) dashboards: BTreeMap<String, Dashboard>,
}

#[derive(Debug)]
pub(crate) struct SpanDeclaration {
    pub(crate) description: String,
    /// What the span is to a trace: its own work, a request served or made, a
    /// message produced or consumed. It describes, so it may change at any
    /// version.
    pub(crate) kind: SpanKind,
    pub(crate) attributes: BTreeMap<String, ScalarType>,
    pub(crate) events: BTreeSet<String>,
}

#[derive(Debug)]
pub(crate) struct MetricDeclaration {
    pub(crate) description: String,
    pub(crate) kind: MetricKind,
    pub(crate) unit: Unit,
    pub(crate) attributes: BTreeMap<String, ScalarType>,
}

#[derive(Debug)]
pub(crate) struct LogDeclaration {
    pub(crate) description: String,
    pub(crate) severity: Severity,
    pub(crate) fields: BTreeMap<String, ScalarType>,
}

/// A condition the module's code reports, as a bc-error kind whose category is
/// the module's name and whose value is the declared `code`.
#[derive(Debug)]
pub(crate) struct ErrorDeclaration {
    pub(crate) description: String,
    pub(crate) class: ErrorClass,
    /// The error's identity within its module, from 0 to [`MAXIMUM_ERROR_CODE`].
    /// Spanned so a finding about the code points at the number.
    pub(crate) code: Spanned<u32>,
    pub(crate) fields: BTreeMap<String, ScalarType>,
}

/// The largest code an error may declare: bc-error's kind value is a signed
/// 32-bit integer.
pub(crate) const MAXIMUM_ERROR_CODE: u32 = 2_147_483_647;

/// A named declarative query.
#[derive(Debug)]
pub(crate) struct Query {
    pub(crate) description: String,
    pub(crate) measure: Spanned<ItemRef>,
    /// Spanned so an aggregation its measure does not admit points at the word.
    pub(crate) aggregation: Spanned<Aggregation>,
    pub(crate) window_seconds: u64,
    /// In declared order: it is the order of the generated grouping columns.
    pub(crate) group_by: Vec<Spanned<String>>,
}

/// A dashboard: panels in declared order, which is their layout order.
#[derive(Debug)]
pub(crate) struct Dashboard {
    pub(crate) title: String,
    pub(crate) panels: Vec<Panel>,
}

#[derive(Debug)]
pub(crate) struct Panel {
    pub(crate) title: String,
    pub(crate) source: Spanned<PanelSource>,
    pub(crate) render: Spanned<Render>,
}

/// What a panel shows: a declared query, or — for `render = "logs"` — the
/// records of a declared log or error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PanelSource {
    Query(QueryRef),
    Records(ItemRef),
}

impl PanelSource {
    /// The reference in its qualified spelling.
    pub(crate) fn spelled(&self) -> String {
        match self {
            Self::Query(query) => query.spelled(),
            Self::Records(item) => item.spelled(),
        }
    }
}

/// A reference to a declared item. It always names the module: a reference the
/// definition wrote as `<kind>.<name>` is resolved to the module that wrote it,
/// so two definitions that mean the same thing lower to the same model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ItemRef {
    pub(crate) kind: ItemKind,
    pub(crate) module: String,
    pub(crate) name: String,
}

impl ItemRef {
    /// `<kind>.<module>.<name>`.
    pub(crate) fn spelled(&self) -> String {
        format!("{}.{}.{}", self.kind.keyword(), self.module, self.name)
    }
}

/// A reference to a declared query, resolved to its module like an [`ItemRef`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueryRef {
    pub(crate) module: String,
    pub(crate) name: String,
}

impl QueryRef {
    /// `query.<module>.<name>`.
    pub(crate) fn spelled(&self) -> String {
        format!("query.{}.{}", self.module, self.name)
    }
}

/// A closed vocabulary spelled by keywords in the definition file.
///
/// One list of members and one member → keyword mapping per vocabulary; the
/// parse direction and the "must be one of" listing are derived from those two,
/// so they cannot disagree with each other.
pub(crate) trait Keyword: Copy + 'static {
    /// Every member, in the order a message lists them.
    const ALL: &'static [Self];

    fn keyword(self) -> &'static str;

    fn from_keyword(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|member| member.keyword() == text)
    }

    fn keywords() -> Vec<&'static str> {
        Self::ALL.iter().map(|member| member.keyword()).collect()
    }
}

/// The kinds of declared item: what a module's code emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ItemKind {
    Span,
    Metric,
    Log,
    Error,
}

impl Keyword for ItemKind {
    const ALL: &'static [Self] = &[Self::Span, Self::Metric, Self::Log, Self::Error];

    fn keyword(self) -> &'static str {
        match self {
            Self::Span => "span",
            Self::Metric => "metric",
            Self::Log => "log",
            Self::Error => "error",
        }
    }
}

/// Whether an error is ours to answer for or the caller's to correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorClass {
    Failure,
    /// Permanent, and never reported: it has no log record and so no view.
    Refusal,
}

impl Keyword for ErrorClass {
    const ALL: &'static [Self] = &[Self::Failure, Self::Refusal];

    fn keyword(self) -> &'static str {
        match self {
            Self::Failure => "failure",
            Self::Refusal => "refusal",
        }
    }
}

/// The closed set of units a metric is measured in, each one Grafana
/// unit so a panel's unit is derived rather than written. The set is the
/// dashboard model's own, so a definition cannot name a unit a dashboard has no
/// spelling for — the pattern `MetricKind` and `Severity` follow below.
pub(crate) use bc_telemetry_dashboard::Unit;

impl Keyword for Unit {
    const ALL: &'static [Self] = &[
        Self::Unitless,
        Self::Milliseconds,
        Self::Seconds,
        Self::Bytes,
        Self::Percent,
    ];

    fn keyword(self) -> &'static str {
        match self {
            Self::Unitless => "none",
            Self::Milliseconds => "ms",
            Self::Seconds => "s",
            Self::Bytes => "bytes",
            Self::Percent => "percent",
        }
    }
}

/// The closed scalar set attributes and fields are typed by: no raw payload
/// bodies and no free-form maps. Each lowers to one of the runtime's four
/// attribute values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScalarType {
    String,
    Int,
    Float,
    Bool,
    /// Lowers to an integer count of milliseconds.
    Duration,
    /// Lowers to a string.
    Digest,
}

impl Keyword for ScalarType {
    const ALL: &'static [Self] = &[
        Self::String,
        Self::Int,
        Self::Float,
        Self::Bool,
        Self::Duration,
        Self::Digest,
    ];

    fn keyword(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Int => "int",
            Self::Float => "float",
            Self::Bool => "bool",
            Self::Duration => "duration",
            Self::Digest => "digest",
        }
    }
}

// bc-telemetry-rs's kinds, given the keywords the definition spells them with.
// Each `match` is exhaustive on purpose: a kind the runtime gains fails to
// compile here until the vocabulary says how it is written. `ALL` sits beside
// the match so the two are read — and edited — together.

impl Keyword for MetricKind {
    const ALL: &'static [Self] = &[Self::Counter, Self::Gauge, Self::Histogram];

    fn keyword(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
            Self::Histogram => "histogram",
        }
    }
}

impl Keyword for SpanKind {
    const ALL: &'static [Self] = &[
        Self::Internal,
        Self::Server,
        Self::Client,
        Self::Producer,
        Self::Consumer,
    ];

    fn keyword(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Server => "server",
            Self::Client => "client",
            Self::Producer => "producer",
            Self::Consumer => "consumer",
        }
    }
}

impl Keyword for Severity {
    const ALL: &'static [Self] = &[
        Self::Debug,
        Self::Info,
        Self::Warn,
        Self::Error,
        Self::Fatal,
    ];

    fn keyword(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
            Self::Fatal => "fatal",
        }
    }
}

/// A query's aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Aggregation {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    P50,
    P90,
    P95,
    P99,
    Rate,
}

impl Keyword for Aggregation {
    const ALL: &'static [Self] = &[
        Self::Count,
        Self::Sum,
        Self::Avg,
        Self::Min,
        Self::Max,
        Self::P50,
        Self::P90,
        Self::P95,
        Self::P99,
        Self::Rate,
    ];

    fn keyword(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Sum => "sum",
            Self::Avg => "avg",
            Self::Min => "min",
            Self::Max => "max",
            Self::P50 => "p50",
            Self::P90 => "p90",
            Self::P95 => "p95",
            Self::P99 => "p99",
            Self::Rate => "rate",
        }
    }
}

/// A storage engine the generator renders a layout for, and the query
/// language that goes with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Dialect {
    Postgresql,
}

impl Keyword for Dialect {
    const ALL: &'static [Self] = &[Self::Postgresql];

    fn keyword(self) -> &'static str {
        match self {
            Self::Postgresql => "postgresql",
        }
    }
}

/// Where the generator exports dashboards to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Export {
    Grafana,
}

impl Keyword for Export {
    const ALL: &'static [Self] = &[Self::Grafana];

    fn keyword(self) -> &'static str {
        match self {
            Self::Grafana => "grafana",
        }
    }
}

/// What a query aggregates, as the README's aggregation table keys it: a
/// metric by its type, a span by its duration, and a log or an error by its
/// records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Measure {
    Counter,
    Gauge,
    Histogram,
    Span,
    Records,
}

impl Measure {
    /// The measure as a message names it: "a counter".
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::Counter => "a counter",
            Self::Gauge => "a gauge",
            Self::Histogram => "a histogram",
            Self::Span => "a span",
            Self::Records => "a log or an error",
        }
    }

    pub(crate) fn of_metric(kind: MetricKind) -> Self {
        match kind {
            MetricKind::Counter => Self::Counter,
            MetricKind::Gauge => Self::Gauge,
            MetricKind::Histogram => Self::Histogram,
        }
    }

    /// Every aggregation this measure admits, in the order a message lists
    /// them.
    pub(crate) fn admitted(self) -> Vec<Aggregation> {
        Aggregation::ALL
            .iter()
            .copied()
            .filter(|aggregation| aggregation.admits(self))
            .collect()
    }
}

impl Aggregation {
    /// Whether this aggregation means anything over `measure` — the README's
    /// aggregation table, one row per measure. A sum of gauge readings, a
    /// percentile of counter increments or an average of a log means nothing.
    pub(crate) fn admits(self, measure: Measure) -> bool {
        let is_percentile = matches!(self, Self::P50 | Self::P90 | Self::P95 | Self::P99);
        let is_average_or_extreme = matches!(self, Self::Avg | Self::Min | Self::Max);
        match measure {
            Measure::Counter => matches!(self, Self::Count | Self::Sum | Self::Rate),
            Measure::Gauge => self == Self::Count || is_average_or_extreme || is_percentile,
            Measure::Histogram | Measure::Span => true,
            Measure::Records => matches!(self, Self::Count | Self::Rate),
        }
    }
}

/// How a panel renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Render {
    Timeseries,
    Stat,
    Table,
    Logs,
}

impl Keyword for Render {
    const ALL: &'static [Self] = &[Self::Timeseries, Self::Stat, Self::Table, Self::Logs];

    fn keyword(self) -> &'static str {
        match self {
            Self::Timeseries => "timeseries",
            Self::Stat => "stat",
            Self::Table => "table",
            Self::Logs => "logs",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trips<T: Keyword + PartialEq + std::fmt::Debug>() {
        let keywords = T::keywords();
        let distinct: BTreeSet<&str> = keywords.iter().copied().collect();
        assert_eq!(distinct.len(), keywords.len(), "a keyword is spelled twice");
        for member in T::ALL {
            assert_eq!(T::from_keyword(member.keyword()), Some(*member));
        }
    }

    #[test]
    fn every_vocabulary_parses_back_what_it_spells() {
        round_trips::<ItemKind>();
        round_trips::<ErrorClass>();
        round_trips::<Dialect>();
        round_trips::<Unit>();
        round_trips::<ScalarType>();
        round_trips::<MetricKind>();
        round_trips::<Severity>();
        round_trips::<Aggregation>();
        round_trips::<Render>();
    }

    #[test]
    fn a_word_outside_the_vocabulary_parses_to_nothing() {
        assert_eq!(ScalarType::from_keyword("strng"), None);
        assert_eq!(Severity::from_keyword("ERROR"), None);
    }

    /// The README's aggregation table, written out cell by cell: a row
    /// per measure, a column per aggregation in `Aggregation::ALL`'s order —
    /// count, sum, avg, min, max, p50, p90, p95, p99, rate.
    #[test]
    fn each_measure_admits_exactly_the_aggregations_of_the_design_table() {
        const YES: bool = true;
        const NO: bool = false;
        let table = [
            (
                Measure::Counter,
                [YES, YES, NO, NO, NO, NO, NO, NO, NO, YES],
            ),
            (
                Measure::Gauge,
                [YES, NO, YES, YES, YES, YES, YES, YES, YES, NO],
            ),
            (
                Measure::Histogram,
                [YES, YES, YES, YES, YES, YES, YES, YES, YES, YES],
            ),
            (
                Measure::Span,
                [YES, YES, YES, YES, YES, YES, YES, YES, YES, YES],
            ),
            (Measure::Records, [YES, NO, NO, NO, NO, NO, NO, NO, NO, YES]),
        ];
        for (measure, row) in table {
            for (aggregation, admitted) in Aggregation::ALL.iter().zip(row) {
                assert_eq!(
                    aggregation.admits(measure),
                    admitted,
                    "`{}` over {}",
                    aggregation.keyword(),
                    measure.describe()
                );
            }
        }
    }

    #[test]
    fn a_measure_lists_what_it_admits_in_keyword_order() {
        let keywords: Vec<&str> = Measure::Records
            .admitted()
            .into_iter()
            .map(Keyword::keyword)
            .collect();
        assert_eq!(keywords, ["count", "rate"]);
    }
}
