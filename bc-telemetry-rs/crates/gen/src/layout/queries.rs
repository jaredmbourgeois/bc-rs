//! The queries over the layout, in PostgreSQL.
//!
//! One statement per named query, over its measure's typed view, and one per
//! log and per failure error that reads its records newest first — what a
//! `render = "logs"` panel shows. Every statement is written for
//! Grafana's PostgreSQL data source, whose macros `$__timeFilter` and
//! `$__timeGroupAlias` expand to a dashboard's time range and interval.
//!
//! A module's statements go to one file, each preceded by a comment naming it,
//! describing it and, for a query, saying what unit its result is in, so the
//! file reads standalone.

use std::collections::BTreeMap;

use bc_telemetry_dashboard::ResultUnit;

use crate::layout::RenderError;
use crate::layout::views::{quoted_identifier, view_name_of};
use crate::model::{
    Aggregation, ErrorClass, ItemKind, ItemRef, Keyword, Measure, Query, Tree, Unit,
};

/// The name a records statement gives a record's severity: the one Grafana's
/// logs panel reads it by. A PostgreSQL result reaches that panel as an untyped
/// frame, which it reads as its first time column for the timestamp, its first
/// text column for the line, and a column named `level` for the severity
/// (`public/app/features/logs/legacyLogsFrame.ts`, Grafana 13.2.3) — so a records
/// statement puts `body` before every other text column, names the severity
/// this, and no field of a log or an error may take this name.
const SEVERITY_AS_GRAFANA_READS_IT: &str = "level";

/// Whether a field of `kind` may not be named `name` because its records
/// statement gives a column that name: only logs and errors have one.
pub(crate) fn is_a_column_the_records_statement_adds(kind: ItemKind, name: &str) -> bool {
    match kind {
        ItemKind::Log | ItemKind::Error => name == SEVERITY_AS_GRAFANA_READS_IT,
        ItemKind::Span | ItemKind::Metric => false,
    }
}

/// What a query's result is in: its measure's unit for a sum, an
/// average, an extreme or a percentile, nothing for a count, and per second for
/// a rate. A counter's rate is its total per second; every other rate counts
/// what it measures, so it is a plain number per second.
pub(crate) fn result_unit(
    aggregation: Aggregation,
    measure: Measure,
    measure_unit: Unit,
) -> ResultUnit {
    match aggregation {
        Aggregation::Count => ResultUnit::Of(Unit::Unitless),
        Aggregation::Rate if measure == Measure::Counter => ResultUnit::PerSecond(measure_unit),
        Aggregation::Rate => ResultUnit::PerSecond(Unit::Unitless),
        Aggregation::Sum
        | Aggregation::Avg
        | Aggregation::Min
        | Aggregation::Max
        | Aggregation::P50
        | Aggregation::P90
        | Aggregation::P95
        | Aggregation::P99 => ResultUnit::Of(measure_unit),
    }
}

/// A result's unit as a statement's comment says it, in the definition file's
/// words: `ms`, `none`, `per second`, `bytes per second`.
pub(crate) fn describe(unit: ResultUnit) -> String {
    match unit {
        ResultUnit::Of(unit) => unit.keyword().to_string(),
        ResultUnit::PerSecond(Unit::Unitless) => "per second".to_string(),
        ResultUnit::PerSecond(unit) => format!("{} per second", unit.keyword()),
    }
}

/// What a query reads: what its measure is, and the unit its values are in.
struct Measured {
    measure: Measure,
    unit: Unit,
}

/// The measure `item` names, as validation resolved it. `None` only for a
/// reference validation should have refused.
fn measured(tree: &Tree, item: &ItemRef) -> Option<Measured> {
    let items = &tree.modules.get(&item.module)?.items;
    match item.kind {
        // A span's measure is its duration, in milliseconds.
        ItemKind::Span => items.spans.get(&item.name).map(|_| Measured {
            measure: Measure::Span,
            unit: Unit::Milliseconds,
        }),
        ItemKind::Metric => items.metrics.get(&item.name).map(|metric| Measured {
            measure: Measure::of_metric(metric.kind),
            unit: metric.unit,
        }),
        ItemKind::Log => items.logs.get(&item.name).map(|_| Measured {
            measure: Measure::Records,
            unit: Unit::Unitless,
        }),
        ItemKind::Error => items
            .errors
            .get(&item.name)
            .filter(|error| error.class == ErrorClass::Failure)
            .map(|_| Measured {
                measure: Measure::Records,
                unit: Unit::Unitless,
            }),
    }
}

/// The column a query aggregates: a metric's value, a span's duration. A log's
/// or an error's records are counted, never summed, so they have none.
fn value_column(measure: Measure) -> Option<&'static str> {
    match measure {
        Measure::Counter | Measure::Gauge | Measure::Histogram => Some("value"),
        Measure::Span => Some("duration_ms"),
        Measure::Records => None,
    }
}

/// A continuous percentile's fraction, as SQL spells it.
fn percentile_fraction(aggregation: Aggregation) -> Option<&'static str> {
    match aggregation {
        Aggregation::P50 => Some("0.5"),
        Aggregation::P90 => Some("0.9"),
        Aggregation::P95 => Some("0.95"),
        Aggregation::P99 => Some("0.99"),
        Aggregation::Count
        | Aggregation::Sum
        | Aggregation::Avg
        | Aggregation::Min
        | Aggregation::Max
        | Aggregation::Rate => None,
    }
}

/// The SQL that aggregates one time bucket, or `None` for an aggregation the
/// measure does not admit, which validation refuses.
pub(crate) fn aggregate_sql(
    aggregation: Aggregation,
    measure: Measure,
    window_seconds: u64,
) -> Option<String> {
    if !aggregation.admits(measure) {
        return None;
    }
    if let Some(fraction) = percentile_fraction(aggregation) {
        let value = value_column(measure)?;
        return Some(format!(
            "percentile_cont({fraction}) WITHIN GROUP (ORDER BY {value})"
        ));
    }
    match aggregation {
        Aggregation::Count => Some("count(*)".to_string()),
        Aggregation::Sum => Some(format!("sum({})", value_column(measure)?)),
        Aggregation::Avg => Some(format!("avg({})", value_column(measure)?)),
        Aggregation::Min => Some(format!("min({})", value_column(measure)?)),
        Aggregation::Max => Some(format!("max({})", value_column(measure)?)),
        // A counter's rate is its total per second; every other measure's
        // counts what happened per second. The count is cast so the division
        // is not an integer one.
        Aggregation::Rate if measure == Measure::Counter => {
            Some(format!("sum(value) / {window_seconds}"))
        }
        Aggregation::Rate => Some(format!("count(*)::double precision / {window_seconds}")),
        // Rendered above, with their fraction.
        Aggregation::P50 | Aggregation::P90 | Aggregation::P95 | Aggregation::P99 => None,
    }
}

/// A window as Grafana's interval syntax spells it, in the largest whole unit:
/// `5m`, `1h`, `90s`.
pub(crate) fn interval(window_seconds: u64) -> String {
    for (unit_seconds, suffix) in [(86_400, "d"), (3_600, "h"), (60, "m")] {
        if window_seconds % unit_seconds == 0 {
            return format!("{}{suffix}", window_seconds / unit_seconds);
        }
    }
    format!("{window_seconds}s")
}

/// A statement as everything downstream of the generator takes it — the query
/// file, a dashboard's panel, a query handle: what it says it is, its SQL, and
/// the unit of what it answers. The SQL carries no terminating `;`, since a
/// panel's target is one statement.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Handle {
    /// How the statement's comment names it: the query's own name, or
    /// `records of <kind> <item>`.
    pub(crate) heading: String,
    pub(crate) description: String,
    pub(crate) sql: String,
    /// `None` for an item's records: a logs panel shows records, not a measure.
    pub(crate) unit: Option<ResultUnit>,
}

impl Handle {
    /// The comment above the statement: its heading and description, each line
    /// of the description a `-- ` line of its own so a line break cannot end the
    /// comment early, then the unit.
    fn comment_lines(&self) -> Vec<String> {
        let mut description_lines = self.description.lines();
        let mut lines = vec![match description_lines.next() {
            Some(first) => format!("-- {}: {first}", self.heading),
            None => format!("-- {}", self.heading),
        }];
        for line in description_lines {
            lines.push(format!("--   {line}"));
        }
        if let Some(unit) = self.unit {
            lines.push(format!("-- unit: {}", describe(unit)));
        }
        lines
    }
}

/// The handle of the query `query_name` of module `module_name`.
///
/// # Errors
///
/// Its measure does not resolve, or does not admit its aggregation: validation
/// refuses both, so either is a defect here.
pub(crate) fn query_handle(
    tree: &Tree,
    module_name: &str,
    query_name: &str,
    query: &Query,
) -> Result<Handle, RenderError> {
    let measure_ref = &query.measure.value;
    let unvalidated = || {
        RenderError::Unvalidated(format!(
            "query `{module_name}.{query_name}` over `{}`",
            measure_ref.spelled()
        ))
    };
    let measured = measured(tree, measure_ref).ok_or_else(unvalidated)?;
    let aggregation = query.aggregation.value;
    let aggregate = aggregate_sql(aggregation, measured.measure, query.window_seconds)
        .ok_or_else(unvalidated)?;
    let groups: Vec<String> = query
        .group_by
        .iter()
        .map(|group| quoted_identifier(&group.value))
        .collect();

    let mut select = vec![format!(
        "$__timeGroupAlias(time, '{}')",
        interval(query.window_seconds)
    )];
    select.extend(groups.iter().cloned());
    select.push(format!("{aggregate} AS value"));
    let mut group_by = vec!["1".to_string()];
    group_by.extend(groups);

    Ok(Handle {
        heading: query_name.to_string(),
        description: query.description.clone(),
        sql: format!(
            "SELECT {}\nFROM telemetry.{}\nWHERE $__timeFilter(time)\nGROUP BY {}\nORDER BY 1",
            select.join(",\n       "),
            quoted_identifier(&view_name_of(measure_ref)),
            group_by.join(", ")
        ),
        unit: Some(result_unit(aggregation, measured.measure, measured.unit)),
    })
}

/// The handle that reads `item`'s records newest first, with its fields as
/// columns, in the order and under the names Grafana's logs panel reads them
/// by ([`SEVERITY_AS_GRAFANA_READS_IT`]).
pub(crate) fn records_handle<'fields>(
    item: &ItemRef,
    description: &str,
    fields: impl Iterator<Item = &'fields String>,
) -> Handle {
    let mut columns = vec![
        "time".to_string(),
        "body".to_string(),
        format!("severity AS {SEVERITY_AS_GRAFANA_READS_IT}"),
    ];
    columns.extend(fields.map(|field| quoted_identifier(field)));
    Handle {
        heading: format!("records of {} {}", item.kind.keyword(), item.name),
        description: description.to_string(),
        sql: format!(
            "SELECT {}\nFROM telemetry.{}\nWHERE $__timeFilter(time)\nORDER BY time DESC",
            columns.join(", "),
            quoted_identifier(&view_name_of(item))
        ),
        unit: None,
    }
}

/// How a module's file opens.
fn opening(module_name: &str) -> String {
    format!(
        "-- The queries of module `{module_name}`, for the bc-telemetry layout on PostgreSQL.\n\
         --\n\
         -- Generated by bc-telemetry-gen. Do not edit.\n\
         -- Each statement is written for Grafana's PostgreSQL data source: $__timeFilter and\n\
         -- $__timeGroupAlias are its macros, expanded to a dashboard's time range and interval.\n"
    )
}

/// Every module's query file, by module name. A module with no query, no log
/// and no failure error has none.
///
/// # Errors
///
/// A query whose measure does not resolve, or whose aggregation its measure
/// does not admit: validation refuses both, so either is a defect here.
pub(crate) fn render(tree: &Tree) -> Result<BTreeMap<String, String>, RenderError> {
    let mut files = BTreeMap::new();
    for (module_name, module) in &tree.modules {
        let mut handles = Vec::new();
        for (query_name, query) in &module.items.queries {
            handles.push(query_handle(tree, module_name, query_name, query)?);
        }
        let item = |kind: ItemKind, name: &str| ItemRef {
            kind,
            module: module_name.clone(),
            name: name.to_string(),
        };
        for (name, log) in &module.items.logs {
            handles.push(records_handle(
                &item(ItemKind::Log, name),
                &log.description,
                log.fields.keys(),
            ));
        }
        for (name, error) in &module.items.errors {
            // A refusal is never reported, so it has no records to read.
            if error.class == ErrorClass::Failure {
                handles.push(records_handle(
                    &item(ItemKind::Error, name),
                    &error.description,
                    error.fields.keys(),
                ));
            }
        }
        if handles.is_empty() {
            continue;
        }
        let mut file = opening(module_name);
        for handle in handles {
            file.push('\n');
            for line in handle.comment_lines() {
                file.push_str(&line);
                file.push('\n');
            }
            file.push_str(&handle.sql);
            file.push_str(";\n");
        }
        files.insert(module_name.clone(), file);
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use bc_telemetry_dashboard::ResultUnit;

    use super::{Handle, aggregate_sql, describe, interval, result_unit};
    use crate::model::{Aggregation, Keyword, Measure, Unit};

    #[test]
    fn every_admitted_aggregation_has_its_sql_and_no_other_does() {
        let expected = [
            (Aggregation::Count, Measure::Records, Some("count(*)")),
            (Aggregation::Sum, Measure::Counter, Some("sum(value)")),
            (Aggregation::Sum, Measure::Span, Some("sum(duration_ms)")),
            (Aggregation::Avg, Measure::Gauge, Some("avg(value)")),
            (Aggregation::Min, Measure::Histogram, Some("min(value)")),
            (Aggregation::Max, Measure::Span, Some("max(duration_ms)")),
            (
                Aggregation::P50,
                Measure::Gauge,
                Some("percentile_cont(0.5) WITHIN GROUP (ORDER BY value)"),
            ),
            (
                Aggregation::P90,
                Measure::Span,
                Some("percentile_cont(0.9) WITHIN GROUP (ORDER BY duration_ms)"),
            ),
            (
                Aggregation::P95,
                Measure::Histogram,
                Some("percentile_cont(0.95) WITHIN GROUP (ORDER BY value)"),
            ),
            (
                Aggregation::P99,
                Measure::Histogram,
                Some("percentile_cont(0.99) WITHIN GROUP (ORDER BY value)"),
            ),
            (Aggregation::Rate, Measure::Counter, Some("sum(value) / 60")),
            (
                Aggregation::Rate,
                Measure::Histogram,
                Some("count(*)::double precision / 60"),
            ),
            (
                Aggregation::Rate,
                Measure::Records,
                Some("count(*)::double precision / 60"),
            ),
            (Aggregation::Sum, Measure::Gauge, None),
            (Aggregation::P95, Measure::Counter, None),
            (Aggregation::Avg, Measure::Records, None),
        ];
        for (aggregation, measure, sql) in expected {
            assert_eq!(
                aggregate_sql(aggregation, measure, 60).as_deref(),
                sql,
                "{} over {}",
                aggregation.keyword(),
                measure.describe()
            );
        }
    }

    #[test]
    fn a_results_unit_follows_its_aggregation() {
        let ms = Unit::Milliseconds;
        let bytes = Unit::Bytes;
        assert_eq!(
            result_unit(Aggregation::P95, Measure::Histogram, ms),
            ResultUnit::Of(ms)
        );
        assert_eq!(
            result_unit(Aggregation::Sum, Measure::Counter, bytes),
            ResultUnit::Of(bytes)
        );
        assert_eq!(
            result_unit(Aggregation::Count, Measure::Histogram, ms),
            ResultUnit::Of(Unit::Unitless)
        );
        assert_eq!(
            result_unit(Aggregation::Rate, Measure::Counter, bytes),
            ResultUnit::PerSecond(bytes)
        );
        assert_eq!(
            result_unit(Aggregation::Rate, Measure::Histogram, ms),
            ResultUnit::PerSecond(Unit::Unitless)
        );
        assert_eq!(describe(ResultUnit::Of(ms)), "ms");
        assert_eq!(
            describe(ResultUnit::PerSecond(Unit::Unitless)),
            "per second"
        );
        assert_eq!(describe(ResultUnit::PerSecond(bytes)), "bytes per second");
    }

    #[test]
    fn a_window_is_spelled_in_its_largest_whole_unit() {
        assert_eq!(interval(30), "30s");
        assert_eq!(interval(90), "90s");
        assert_eq!(interval(300), "5m");
        assert_eq!(interval(3_600), "1h");
        assert_eq!(interval(5_400), "90m");
        assert_eq!(interval(86_400), "1d");
        assert_eq!(interval(172_800), "2d");
    }

    #[test]
    fn a_description_never_ends_its_comment_early() {
        let handle = Handle {
            heading: "q".to_string(),
            description: "first line\nSELECT 1;".to_string(),
            sql: "SELECT 2".to_string(),
            unit: Some(ResultUnit::Of(Unit::Bytes)),
        };
        assert_eq!(
            handle.comment_lines(),
            ["-- q: first line", "--   SELECT 1;", "-- unit: bytes"]
        );
    }
}
