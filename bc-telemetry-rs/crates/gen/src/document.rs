//! The positioned document tree both front ends lower into.
//!
//! TOML and JSON are equivalent by construction rather than by a serializer:
//! each syntax has a front end that turns its file into this one shape, and a
//! single strict walker reads the shape into the model. A second syntax
//! therefore costs a front end, never a second copy of the validation rules.

use crate::source::Span;

/// A value and where it was written.
#[derive(Debug)]
pub(crate) struct Node {
    pub(crate) span: Span,
    pub(crate) value: Value,
}

/// The value kinds a definition can hold.
#[derive(Debug)]
pub(crate) enum Value {
    String(String),
    Integer(i64),
    /// No key a definition defines takes a float or a boolean, so their values
    /// are not kept: they exist to be described in a type mismatch. A key that
    /// needs one adds the payload here.
    Float,
    Boolean,
    Array(Vec<Node>),
    Table(Table),
    /// A value one syntax can write and the definition has no use for — a TOML
    /// datetime, a JSON `null`. Kept as a node rather than refused by the front
    /// end, so the walker reports it as a type mismatch at the key that holds it.
    Unrepresented(&'static str),
}

impl Value {
    /// The value's kind as an error message says it: "a string", "a table".
    pub(crate) fn describe(&self) -> &'static str {
        match self {
            Self::String(_) => "a string",
            Self::Integer(_) => "an integer",
            Self::Float => "a float",
            Self::Boolean => "a boolean",
            Self::Array(_) => "an array",
            Self::Table(_) => "a table",
            Self::Unrepresented(description) => description,
        }
    }
}

/// A table's entries in the order they were written.
#[derive(Debug)]
pub(crate) struct Table {
    /// Where the table is named: its key, or the start of the file for a root.
    pub(crate) span: Span,
    pub(crate) entries: Vec<Entry>,
}

impl Table {
    pub(crate) fn get(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.key == key)
    }
}

/// One `key = value` entry.
#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) key: String,
    pub(crate) key_span: Span,
    pub(crate) value: Node,
}
