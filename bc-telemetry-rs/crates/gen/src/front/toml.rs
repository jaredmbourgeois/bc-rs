//! The TOML front end, over `toml_edit`, picked because every key and value
//! keeps its byte span.
//!
//! `toml_edit` spreads values across `Item`, `Value`, `Table`, `InlineTable` and
//! `ArrayOfTables`, each with an optional span. This module folds all of that
//! into the one [`Node`] shape, so standard tables, dotted keys, inline tables
//! and arrays of tables are indistinguishable to the walker — as they are in
//! the TOML data model.

use std::ops::Range;

use crate::diagnostics::{Code, Diagnostic};
use crate::document::{Entry, Node, Table, Value};
use crate::source::{FileId, Span};

pub(super) fn parse(file: FileId, text: &str) -> Result<Table, Diagnostic> {
    match toml_edit::Document::parse(text) {
        Ok(document) => Ok(lower_table(document.as_table(), file, Span::start_of(file))),
        Err(error) => Err(Diagnostic {
            code: Code::TomlSyntax,
            message: format!("TOML syntax error: {}", error.message().trim_end()),
            span: Some(
                error
                    .span()
                    .map_or(Span::start_of(file), |range| span(file, &range)),
            ),
        }),
    }
}

fn span(file: FileId, range: &Range<usize>) -> Span {
    Span {
        file,
        start: range.start,
        end: range.end,
    }
}

/// Lowers `table`, standing at `named_at` (the key that names it).
///
/// Every span falls back to an enclosing one when `toml_edit` gives none — it
/// gives one for everything a parse produced, so the fallback only keeps this
/// function total.
fn lower_table(table: &toml_edit::Table, file: FileId, named_at: Span) -> Table {
    let mut entries = Vec::new();
    for (key, item) in table {
        let key_span = table
            .key(key)
            .and_then(toml_edit::Key::span)
            .map_or(named_at, |range| span(file, &range));
        if let Some(value) = lower_item(item, file, key_span) {
            entries.push(Entry {
                key: key.to_string(),
                key_span,
                value,
            });
        }
    }
    Table {
        span: named_at,
        entries,
    }
}

fn lower_item(item: &toml_edit::Item, file: FileId, key_span: Span) -> Option<Node> {
    match item {
        // A placeholder `toml_edit` uses for an absent value; a parsed document
        // never hands one to iteration, and there is nothing to lower.
        toml_edit::Item::None => None,
        toml_edit::Item::Value(value) => Some(lower_value(value, file, key_span)),
        toml_edit::Item::Table(table) => Some(Node {
            span: key_span,
            value: Value::Table(lower_table(table, file, key_span)),
        }),
        toml_edit::Item::ArrayOfTables(tables) => {
            let elements = tables
                .iter()
                .map(|table| {
                    let element_span = table.span().map_or(key_span, |range| span(file, &range));
                    Node {
                        span: element_span,
                        value: Value::Table(lower_table(table, file, element_span)),
                    }
                })
                .collect();
            Some(Node {
                span: key_span,
                value: Value::Array(elements),
            })
        }
    }
}

fn lower_value(value: &toml_edit::Value, file: FileId, fallback: Span) -> Node {
    let value_span = value.span().map_or(fallback, |range| span(file, &range));
    let lowered = match value {
        toml_edit::Value::String(text) => Value::String(text.value().clone()),
        toml_edit::Value::Integer(number) => Value::Integer(*number.value()),
        toml_edit::Value::Float(_) => Value::Float,
        toml_edit::Value::Boolean(_) => Value::Boolean,
        toml_edit::Value::Datetime(_) => Value::Unrepresented("a datetime"),
        toml_edit::Value::Array(array) => Value::Array(
            array
                .iter()
                .map(|element| lower_value(element, file, value_span))
                .collect(),
        ),
        toml_edit::Value::InlineTable(inline) => {
            let mut entries = Vec::new();
            for (key, entry_value) in inline {
                let key_span = inline
                    .key(key)
                    .and_then(toml_edit::Key::span)
                    .map_or(value_span, |range| span(file, &range));
                entries.push(Entry {
                    key: key.to_string(),
                    key_span,
                    value: lower_value(entry_value, file, key_span),
                });
            }
            Value::Table(Table {
                span: value_span,
                entries,
            })
        }
    };
    Node {
        span: value_span,
        value: lowered,
    }
}
