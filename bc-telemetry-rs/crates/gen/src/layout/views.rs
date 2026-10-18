//! The typed views: one per declared item, named
//! `<kind>__<module>__<item>`, reading that item's rows at its module's current
//! `schema_version` and turning its attributes into typed columns.
//!
//! A view reads one version on purpose. An additive change keeps the version,
//! so the view sees every earlier row; a breaking change bumps it, and the view
//! starts over rather than cast an earlier row's attribute to a type it never
//! had.
//!
//! An error that is a `refusal` has no view: nothing reports one, since it is
//! the caller's to correct, so no row could ever be in it.

use std::collections::BTreeMap;

use crate::model::{ErrorClass, ItemKind, ItemRef, Keyword, ScalarType, Tree};
use crate::names::view_name;

/// One view, as far as anything that reads it needs to know: its name, the rows
/// it selects, and its columns.
#[derive(Debug)]
pub(crate) struct ViewShape {
    /// `<kind>__<module>__<item>`, unquoted.
    pub(crate) name: String,
    pub(crate) kind: ItemKind,
    pub(crate) module: String,
    pub(crate) item: String,
    pub(crate) schema_version: u32,
    /// The item's attributes — or, for a log or an error, its fields — in name
    /// order, which is their column order.
    pub(crate) attributes: Vec<(String, ScalarType)>,
}

impl ViewShape {
    /// The table this view reads.
    pub(crate) fn table(&self) -> &'static str {
        table_of(self.kind)
    }

    /// The columns every view of this kind carries after the common ones.
    pub(crate) fn kind_columns(&self) -> &'static [&'static str] {
        kind_columns(self.kind)
    }
}

/// The table an item kind's records are stored in. Errors are log records.
pub(crate) fn table_of(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Span => "spans",
        ItemKind::Metric => "metrics",
        ItemKind::Log | ItemKind::Error => "logs",
    }
}

/// The columns every view carries, before its kind's own.
pub(crate) const COMMON_COLUMNS: [&str; 6] = [
    "time",
    "service_name",
    "organization",
    "customer",
    "trace_id",
    "span_id",
];

/// The columns a view of `kind` carries after the common ones and before the
/// item's attributes.
pub(crate) fn kind_columns(kind: ItemKind) -> &'static [&'static str] {
    match kind {
        ItemKind::Span => &[
            "end_time",
            "duration_ms",
            "kind",
            "status",
            "status_message",
        ],
        ItemKind::Metric => &["value"],
        ItemKind::Log | ItemKind::Error => &["severity", "body"],
    }
}

/// Whether an attribute — or field — of `kind` may be named `name`: a view's
/// attribute columns sit beside the columns every view of that kind carries,
/// and one name cannot be two columns.
pub(crate) fn is_a_column_every_view_carries(kind: ItemKind, name: &str) -> bool {
    COMMON_COLUMNS.contains(&name) || kind_columns(kind).contains(&name)
}

/// The view of the item `item` names, unquoted.
pub(crate) fn view_name_of(item: &ItemRef) -> String {
    view_name(item.kind.keyword(), &item.module, &item.name)
}

/// The SQL type an attribute's column is cast to.
pub(crate) fn column_type(scalar: ScalarType) -> &'static str {
    match scalar {
        ScalarType::String | ScalarType::Digest => "text",
        ScalarType::Int | ScalarType::Duration => "bigint",
        ScalarType::Float => "double precision",
        ScalarType::Bool => "boolean",
    }
}

/// Every view the tree generates, in module order, then kind order — span,
/// metric, log, error — then item order.
pub(crate) fn shapes(tree: &Tree) -> Vec<ViewShape> {
    let mut shapes = Vec::new();
    for (module_name, module) in &tree.modules {
        let version = module.schema_version.value;
        let mut add = |kind: ItemKind, item: &str, attributes: &BTreeMap<String, ScalarType>| {
            shapes.push(ViewShape {
                name: view_name(kind.keyword(), module_name, item),
                kind,
                module: module_name.clone(),
                item: item.to_string(),
                schema_version: version,
                attributes: attributes
                    .iter()
                    .map(|(name, scalar)| (name.clone(), *scalar))
                    .collect(),
            });
        };
        for (name, span) in &module.items.spans {
            add(ItemKind::Span, name, &span.attributes);
        }
        for (name, metric) in &module.items.metrics {
            add(ItemKind::Metric, name, &metric.attributes);
        }
        for (name, log) in &module.items.logs {
            add(ItemKind::Log, name, &log.fields);
        }
        for (name, error) in &module.items.errors {
            if error.class == ErrorClass::Failure {
                add(ItemKind::Error, name, &error.fields);
            }
        }
    }
    shapes
}

/// A declared name as an SQL identifier. Every identifier a declaration
/// supplies is quoted, so an attribute named `order` or `user` is a column
/// like any other.
pub(crate) fn quoted_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Text as an SQL string literal.
pub(crate) fn string_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// How the views file opens: what it is, and the statement that drops the views
/// an earlier run created.
const OPENING: &str = "\
-- The typed views of this tree's telemetry, for the bc-telemetry layout on PostgreSQL.
--
-- Generated by bc-telemetry-gen. Do not edit.
-- Apply it after the layout, and again whenever the definitions change: it drops every view
-- in the `telemetry` schema and creates this tree's, in one transaction. A view reads its
-- item's rows at its module's current schema_version.

BEGIN;

DO $$
DECLARE
    existing record;
BEGIN
    FOR existing IN SELECT viewname FROM pg_views WHERE schemaname = 'telemetry' LOOP
        EXECUTE format('DROP VIEW telemetry.%I', existing.viewname);
    END LOOP;
END;
$$;
";

/// The file that drops every view in the `telemetry` schema and creates the
/// tree's, in one transaction: views hold no data, so they are replaced whole
/// rather than altered.
pub(crate) fn render(tree: &Tree) -> String {
    let mut sql = String::from(OPENING);
    for shape in shapes(tree) {
        sql.push('\n');
        sql.push_str(&view_statement(&shape));
    }
    sql.push_str("\nCOMMIT;\n");
    sql
}

fn view_statement(shape: &ViewShape) -> String {
    let mut columns: Vec<String> = COMMON_COLUMNS.iter().map(ToString::to_string).collect();
    for column in shape.kind_columns() {
        columns.push(if *column == "duration_ms" {
            // The value a span's queries aggregate: its length in milliseconds.
            "(extract(epoch FROM (end_time - time)) * 1000)::double precision AS duration_ms"
                .to_string()
        } else {
            (*column).to_string()
        });
    }
    for (attribute, scalar) in &shape.attributes {
        columns.push(format!(
            "(attributes->>{})::{} AS {}",
            string_literal(attribute),
            column_type(*scalar),
            quoted_identifier(attribute)
        ));
    }
    format!(
        "CREATE VIEW telemetry.{} AS\nSELECT {}\nFROM telemetry.{}\nWHERE module = {} AND name = {} AND schema_version = {};\n",
        quoted_identifier(&shape.name),
        columns.join(",\n       "),
        shape.table(),
        string_literal(&shape.module),
        string_literal(&shape.item),
        shape.schema_version
    )
}

#[cfg(test)]
mod tests {
    use super::{is_a_column_every_view_carries, quoted_identifier, string_literal};
    use crate::model::ItemKind;

    #[test]
    fn an_identifier_and_a_literal_are_quoted_whatever_they_hold() {
        assert_eq!(quoted_identifier("order"), "\"order\"");
        assert_eq!(quoted_identifier("a\"b"), "\"a\"\"b\"");
        assert_eq!(string_literal("payments"), "'payments'");
        assert_eq!(string_literal("it's"), "'it''s'");
    }

    #[test]
    fn a_column_is_reserved_for_the_kinds_whose_views_carry_it() {
        for kind in [
            ItemKind::Span,
            ItemKind::Metric,
            ItemKind::Log,
            ItemKind::Error,
        ] {
            assert!(is_a_column_every_view_carries(kind, "time"));
            assert!(is_a_column_every_view_carries(kind, "customer"));
            assert!(!is_a_column_every_view_carries(kind, "route"));
        }
        // `status` is a span's column and an ordinary attribute anywhere else.
        assert!(is_a_column_every_view_carries(ItemKind::Span, "status"));
        assert!(!is_a_column_every_view_carries(ItemKind::Metric, "status"));
        assert!(is_a_column_every_view_carries(ItemKind::Metric, "value"));
        assert!(!is_a_column_every_view_carries(ItemKind::Log, "value"));
        assert!(is_a_column_every_view_carries(ItemKind::Error, "body"));
        assert!(!is_a_column_every_view_carries(ItemKind::Span, "body"));
    }
}
