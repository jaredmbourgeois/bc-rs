//! The JSON front end, over bc-json-rs.
//!
//! The text is validated first — which refuses duplicate keys, trailing input
//! and non-RFC-8259 numbers, and bounds nesting depth — and then walked once with
//! bc-json-rs's own token scanners, so every node keeps its byte span. That
//! split is why the walk below treats a structural surprise as a defect in this
//! generator rather than as a syntax error: the validator has already ruled
//! every syntax error out.

use bc_json::detail;

use crate::diagnostics::{Code, Diagnostic};
use crate::document::{Entry, Node, Table, Value};
use crate::source::{FileId, Span};

pub(super) fn parse(file: FileId, text: &str) -> Result<Table, Diagnostic> {
    let validated = bc_json::validate_json(text).map_err(|error| Diagnostic {
        code: Code::JsonSyntax,
        message: format!("JSON syntax error: {}", error.kind.message()),
        span: Some(Span {
            file,
            start: error.byte_offset,
            end: error.byte_offset,
        }),
    })?;
    let document = JsonText {
        file,
        text: validated.as_str(),
    };
    match document.node(0) {
        Some((
            Node {
                value: Value::Table(mut table),
                ..
            },
            _,
        )) => {
            // The root object is named by nothing; a whole-file finding points
            // at the start of the file, as it does for TOML.
            table.span = Span::start_of(file);
            Ok(table)
        }
        Some((node, _)) => Err(Diagnostic {
            code: Code::TypeMismatch,
            message: format!(
                "a JSON definition file must be one object, got {}",
                node.value.describe()
            ),
            span: Some(node.span),
        }),
        None => Err(walk_defect(file)),
    }
}

/// A validated document and the file it came from.
struct JsonText<'text> {
    file: FileId,
    text: &'text str,
}

impl JsonText<'_> {
    fn span(&self, start: usize, end: usize) -> Span {
        Span {
            file: self.file,
            start,
            end,
        }
    }

    /// The value that starts at `position` (after any whitespace), and the
    /// position just past it. `None` only for text the validator would have
    /// refused.
    fn node(&self, position: usize) -> Option<(Node, usize)> {
        let start = detail::skip_ws(self.text, position);
        match self.text.as_bytes().get(start)? {
            b'{' => self.object(start),
            b'[' => self.array(start),
            b'"' => {
                let mut decoded = String::new();
                let end = detail::parse_json_string(self.text, start, &mut decoded)?;
                Some((self.leaf(start, end, Value::String(decoded)), end))
            }
            b't' | b'f' => {
                let (_, end) = detail::parse_json_bool(self.text, start)?;
                Some((self.leaf(start, end, Value::Boolean), end))
            }
            b'n' => {
                let end = detail::skip_json_value(self.text, start)?;
                Some((self.leaf(start, end, Value::Unrepresented("null")), end))
            }
            _ => self.number(start),
        }
    }

    fn leaf(&self, start: usize, end: usize, value: Value) -> Node {
        Node {
            span: self.span(start, end),
            value,
        }
    }

    fn object(&self, open: usize) -> Option<(Node, usize)> {
        let bytes = self.text.as_bytes();
        let mut entries = Vec::new();
        let mut position = detail::skip_ws(self.text, open + 1);
        if bytes.get(position) != Some(&b'}') {
            loop {
                let key_start = position;
                let mut key = String::new();
                let key_end = detail::parse_json_string(self.text, key_start, &mut key)?;
                let colon = detail::skip_ws(self.text, key_end);
                if bytes.get(colon) != Some(&b':') {
                    return None;
                }
                let (value, after_value) = self.node(colon + 1)?;
                entries.push(Entry {
                    key,
                    key_span: self.span(key_start, key_end),
                    value,
                });
                position = detail::skip_ws(self.text, after_value);
                match bytes.get(position)? {
                    b',' => position = detail::skip_ws(self.text, position + 1),
                    b'}' => break,
                    _ => return None,
                }
            }
        }
        let end = position + 1;
        let span = self.span(open, end);
        Some((
            Node {
                span,
                value: Value::Table(Table { span, entries }),
            },
            end,
        ))
    }

    fn array(&self, open: usize) -> Option<(Node, usize)> {
        let bytes = self.text.as_bytes();
        let mut elements = Vec::new();
        let mut position = detail::skip_ws(self.text, open + 1);
        if bytes.get(position) != Some(&b']') {
            loop {
                let (element, after_element) = self.node(position)?;
                elements.push(element);
                position = detail::skip_ws(self.text, after_element);
                match bytes.get(position)? {
                    b',' => position = detail::skip_ws(self.text, position + 1),
                    b']' => break,
                    _ => return None,
                }
            }
        }
        let end = position + 1;
        Some((self.leaf(open, end, Value::Array(elements)), end))
    }

    /// A number token: an integer when it has no fraction or exponent, a float
    /// otherwise — the distinction TOML draws, so `1` and `1.0` mean in JSON
    /// what they mean there.
    fn number(&self, start: usize) -> Option<(Node, usize)> {
        let end = detail::skip_json_value(self.text, start)?;
        let token = &self.text[start..end];
        // An empty token means the walk stands on a delimiter: it has lost its
        // place, and an element made of no bytes would hide that.
        if token.is_empty() {
            return None;
        }
        let value = if token.contains(['.', 'e', 'E']) {
            // The validator has already held the token to RFC 8259's number
            // grammar, which is a subset of what `f64`'s parser accepts, so this
            // parse reads the number exactly; it does not re-validate it.
            match token.parse::<f64>() {
                Ok(number) if number.is_finite() => Value::Float,
                _ => Value::Unrepresented("a number outside the 64-bit float range"),
            }
        } else {
            match detail::parse_json_number(self.text, start) {
                Some((number, _)) => Value::Integer(number),
                None => Value::Unrepresented("an integer outside the 64-bit range"),
            }
        };
        Some((self.leaf(start, end, value), end))
    }
}

fn walk_defect(file: FileId) -> Diagnostic {
    debug_assert!(
        false,
        "bc-json-rs validated a document this walk could not read"
    );
    Diagnostic {
        code: Code::Internal,
        message: "the JSON validator accepted this file but bc-telemetry-gen could not walk it \
                  — a defect in bc-telemetry-gen, not in the file"
            .to_string(),
        span: Some(Span::start_of(file)),
    }
}
