//! The strict table walker: the one place that reads a key, checks its type,
//! and reports keys nobody read.
//!
//! Every accessor *consumes* its key; [`finish`] reports whatever was not
//! consumed as an unknown field with its nearest known spellings, so a typo'd
//! `atributes` is a finding instead of a silently generated nothing. Wrong
//! types accumulate as findings and read as absent, so one run reports every
//! problem in a table.
//!
//! [`finish`]: TableWalker::finish

use std::collections::BTreeSet;

use crate::diagnostics::{Code, Diagnostics, candidates_for, did_you_mean};
use crate::document::{Entry, Node, Table, Value};
use crate::source::{Span, Spanned};

pub(crate) struct TableWalker<'doc, 'sink> {
    table: &'doc Table,
    /// What the table is, as a message names it: "span `checkout`".
    context: String,
    consumed: BTreeSet<&'doc str>,
    diagnostics: &'sink mut Diagnostics,
}

impl<'doc, 'sink> TableWalker<'doc, 'sink> {
    pub(crate) fn new(
        table: &'doc Table,
        context: impl Into<String>,
        diagnostics: &'sink mut Diagnostics,
    ) -> Self {
        Self {
            table,
            context: context.into(),
            consumed: BTreeSet::new(),
            diagnostics,
        }
    }

    pub(crate) fn context(&self) -> &str {
        &self.context
    }

    /// The sink, for a nested walker or a check that is not about one key.
    pub(crate) fn diagnostics(&mut self) -> &mut Diagnostics {
        self.diagnostics
    }

    /// Whether the table carries `key`. Does not consume it.
    pub(crate) fn has(&self, key: &str) -> bool {
        self.table.get(key).is_some()
    }

    /// The raw entry for `key`, consuming it.
    pub(crate) fn take(&mut self, key: &str) -> Option<&'doc Entry> {
        let entry = self.table.get(key)?;
        self.consumed.insert(&entry.key);
        Some(entry)
    }

    /// Marks `key` read without yielding it — for a key another pass reads and
    /// reports on, which [`Self::finish`] would otherwise call unknown.
    pub(crate) fn consume(&mut self, key: &str) {
        if let Some(entry) = self.table.get(key) {
            self.consumed.insert(&entry.key);
        }
    }

    /// A string `key`, consuming it.
    pub(crate) fn optional_string(&mut self, key: &str) -> Option<Spanned<String>> {
        let entry = self.take(key)?;
        match &entry.value.value {
            Value::String(text) => self.writable(key, text, entry.value.span),
            other => {
                self.type_mismatch(key, "a string", other, entry.value.span);
                None
            }
        }
    }

    /// `text`, unless it holds what no generated file can carry — which is a
    /// finding at `span`, and reads as absent: a NUL, which no generated file
    /// can carry, or a carriage return no line feed follows, which would end a
    /// generated `--` or `//` comment early and make the rest of its line code.
    fn writable(&mut self, key: &str, text: &str, span: Span) -> Option<Spanned<String>> {
        let Some(what) = unwritable(text) else {
            return Some(Spanned {
                value: text.to_owned(),
                span,
            });
        };
        let message = format!("`{key}` on {} holds {what}", self.context);
        self.diagnostics.at(Code::InvalidValue, span, message);
        None
    }

    /// A string `key` the table must carry.
    pub(crate) fn required_string(&mut self, key: &str) -> Option<Spanned<String>> {
        if self.table.get(key).is_none() {
            self.missing(key);
            return None;
        }
        self.optional_string(key)
    }

    /// A required string that must not be empty — a description, a title.
    pub(crate) fn required_text(&mut self, key: &str) -> Option<Spanned<String>> {
        let text = self.required_string(key)?;
        if text.value.trim().is_empty() {
            let message = format!("`{key}` on {} must not be empty", self.context);
            self.diagnostics.at(Code::InvalidValue, text.span, message);
            return None;
        }
        Some(text)
    }

    /// An integer `key` the table must carry.
    pub(crate) fn required_integer(&mut self, key: &str) -> Option<Spanned<i64>> {
        let Some(entry) = self.take(key) else {
            self.missing(key);
            return None;
        };
        match &entry.value.value {
            Value::Integer(number) => Some(Spanned {
                value: *number,
                span: entry.value.span,
            }),
            other => {
                self.type_mismatch(key, "an integer", other, entry.value.span);
                None
            }
        }
    }

    /// A table `key`, consuming it.
    pub(crate) fn optional_table(&mut self, key: &str) -> Option<&'doc Table> {
        let entry = self.take(key)?;
        match &entry.value.value {
            Value::Table(table) => Some(table),
            other => {
                self.type_mismatch(key, "a table", other, entry.value.span);
                None
            }
        }
    }

    /// A table `key` and where the key is written, consuming it — for a table
    /// a later finding points back at.
    pub(crate) fn optional_keyed_table(&mut self, key: &str) -> Option<(&'doc Table, Span)> {
        let key_span = self.table.get(key)?.key_span;
        Some((self.optional_table(key)?, key_span))
    }

    /// A table `key` the table must carry.
    pub(crate) fn required_table(&mut self, key: &str) -> Option<&'doc Table> {
        if self.table.get(key).is_none() {
            self.missing(key);
            return None;
        }
        self.optional_table(key)
    }

    /// An array `key`, consuming it: its elements and the array's own span.
    pub(crate) fn optional_array(&mut self, key: &str) -> Option<(&'doc [Node], Span)> {
        let entry = self.take(key)?;
        match &entry.value.value {
            Value::Array(elements) => Some((elements, entry.value.span)),
            other => {
                self.type_mismatch(key, "an array", other, entry.value.span);
                None
            }
        }
    }

    /// An array `key` the table must carry.
    pub(crate) fn required_array(&mut self, key: &str) -> Option<(&'doc [Node], Span)> {
        if !self.has(key) {
            self.missing(key);
            return None;
        }
        self.optional_array(key)
    }

    /// An array of strings `key`, consuming it. A non-string element, or one
    /// no generated file can carry, is a finding of its own and is left out of
    /// the result.
    pub(crate) fn optional_string_array(&mut self, key: &str) -> Option<Vec<Spanned<String>>> {
        let (elements, _) = self.optional_array(key)?;
        let mut strings = Vec::new();
        for element in elements {
            match &element.value {
                Value::String(text) => strings.extend(self.writable(key, text, element.span)),
                other => {
                    let message = format!(
                        "every element of `{key}` on {} must be a string, got {}",
                        self.context,
                        other.describe()
                    );
                    self.diagnostics
                        .at(Code::TypeMismatch, element.span, message);
                }
            }
        }
        Some(strings)
    }

    /// An array of strings `key` the table must carry.
    pub(crate) fn required_string_array(&mut self, key: &str) -> Option<Vec<Spanned<String>>> {
        if !self.has(key) {
            self.missing(key);
            return None;
        }
        self.optional_string_array(key)
    }

    /// A string `key` whose text must name one member of a closed vocabulary.
    /// A miss is a finding that lists the vocabulary and the nearest member.
    pub(crate) fn required_keyword<T>(
        &mut self,
        key: &str,
        vocabulary: &[&str],
        parse: impl Fn(&str) -> Option<T>,
    ) -> Option<Spanned<T>> {
        let declared = self.required_string(key)?;
        if let Some(parsed) = parse(&declared.value) {
            return Some(Spanned {
                value: parsed,
                span: declared.span,
            });
        }
        let message = format!(
            "`{key}` on {} must be one of {}; got `{}`{}",
            self.context,
            vocabulary.join(" | "),
            declared.value,
            did_you_mean(&candidates_for(&declared.value, vocabulary.iter().copied()))
        );
        self.diagnostics
            .at(Code::InvalidValue, declared.span, message);
        None
    }

    /// Reports every key no accessor consumed, with candidates from `known`.
    pub(crate) fn finish(self, known: &[&str]) {
        for entry in &self.table.entries {
            if self.consumed.contains(entry.key.as_str()) {
                continue;
            }
            let message = format!(
                "unknown field `{}` on {}{}",
                entry.key,
                self.context,
                did_you_mean(&candidates_for(&entry.key, known.iter().copied()))
            );
            self.diagnostics
                .at(Code::UnknownField, entry.key_span, message);
        }
    }

    fn missing(&mut self, key: &str) {
        let message = format!("{} is missing its required `{key}`", self.context);
        self.diagnostics
            .at(Code::MissingField, self.table.span, message);
    }

    fn type_mismatch(&mut self, key: &str, expected: &str, got: &Value, span: Span) {
        let message = format!(
            "`{key}` on {} must be {expected}, got {}",
            self.context,
            got.describe()
        );
        self.diagnostics.at(Code::TypeMismatch, span, message);
    }
}

/// What in `text` no generated file can carry, as a finding says it, or `None`.
///
/// A NUL, which no PostgreSQL value stores, and which ends a C string where
/// the text does not. And a carriage return no line feed follows: a
/// description is written one comment line per line, and a lone CR ends a
/// `--` or `//` comment early, making the rest of its line code — SQL a
/// `queries/*.sql` file would run. A CR before a LF is a line ending.
fn unwritable(text: &str) -> Option<&'static str> {
    if text.contains('\0') {
        return Some("a NUL, which no generated file can carry");
    }
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' && characters.peek() != Some(&'\n') {
            return Some(
                "a carriage return no line feed follows, which would end a generated `--` or `//` \
                 comment early",
            );
        }
    }
    None
}
