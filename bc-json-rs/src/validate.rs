//! The strict RFC 8259 gate for untrusted input.
//!
//! [`validate_json`] answers one question — is this whole byte range a single
//! well-formed JSON document — and answers it completely: structure, number
//! grammar, string escapes, Unicode, duplicate object keys, nesting depth, and
//! full consumption of the input.
//!
//! It returns a [`Validated`] rather than `()`, and a [`JsonReader`] can only
//! be built from one. That turns a composition the C++ source could only
//! document into something the compiler enforces — see the crate docs for why
//! the reader's laziness depends on it.
//!
//! [`JsonReader`]: crate::JsonReader

use std::collections::HashSet;
use std::fmt;

use crate::detail;
use crate::number::json_number_end;

/// The maximum number of nested containers a document may contain.
///
/// This is a resource limit, not a syntax rule: validation is recursive, and an
/// attacker-supplied document of unbounded nesting is a stack-exhaustion vector
/// without it. RFC 8259 explicitly permits an implementation to set one.
pub const MAX_JSON_NESTING_DEPTH: usize = 128;

/// The `AnyError` category [`JsonValidationErrorKind`] crosses under.
///
/// Its own, and deliberately not the one [`crate::kinds`] uses: the numbering
/// below is a committed wire value that grows by taking the next free number,
/// which stays safe only while nothing else numbers under this category.
pub const VALIDATION_CATEGORY: &str = "json";

/// Why a document failed validation.
///
/// `#[non_exhaustive]`: a future RFC-conformance fix may need a new kind, and a
/// consumer mapping these into its own taxonomy should not break when one
/// arrives. Matching in this crate stays exhaustive, guarded by a test.
/// The discriminants are written out and `#[repr(i32)]` because they are a wire
/// value, not an implementation detail: they are the numeric kind carried into
/// `bc-error-rs`'s `AnyError` and across any language binding. Reordering the
/// variants must not silently renumber them; new kinds take the next number —
/// which is safe because [`VALIDATION_CATEGORY`] is this enum's alone, so the
/// next number cannot land on another taxonomy's.
#[non_exhaustive]
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JsonValidationErrorKind {
    /// Input ended in the middle of a value.
    UnexpectedEnd = 1,
    /// A byte appeared where no value or separator could begin.
    UnexpectedToken = 2,
    /// A number violated the RFC 8259 grammar.
    InvalidNumber = 3,
    /// A string was unterminated or held a raw control byte.
    InvalidString = 4,
    /// A `\` escape named no legal escape character.
    InvalidEscape = 5,
    /// A `\u` escape was malformed, or a surrogate was unpaired.
    InvalidUnicodeEscape = 6,
    /// The input was not valid UTF-8.
    InvalidUtf8 = 7,
    /// An object had two members whose **decoded** keys are equal.
    DuplicateKey = 8,
    /// Nesting exceeded [`MAX_JSON_NESTING_DEPTH`].
    NestingTooDeep = 9,
    /// A complete document was followed by more non-whitespace input.
    TrailingInput = 10,
    /// The `\u0000` escape. RFC 8259 admits it, and it decodes to a NUL byte — which
    /// every C-string boundary downstream truncates at rather than failing, so
    /// the one escape whose decoding is a NUL is refused in the decoder for every
    /// document, as the C++ source refuses it.
    /// Every other control character stays accepted by the grammar on purpose: a
    /// decoder-level rule would make an ungated reader of a provider document
    /// read the refused field and every later one as absent.
    NulCharacter = 11,
}

impl JsonValidationErrorKind {
    /// A short human description, used by [`JsonValidationError`]'s `Display`.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::UnexpectedEnd => "unexpected end of input",
            Self::UnexpectedToken => "unexpected token",
            Self::InvalidNumber => "invalid number",
            Self::InvalidString => "invalid string",
            Self::InvalidEscape => "invalid escape",
            Self::InvalidUnicodeEscape => "invalid unicode escape",
            Self::InvalidUtf8 => "invalid UTF-8",
            Self::DuplicateKey => "duplicate object key",
            Self::NestingTooDeep => "nesting too deep",
            Self::TrailingInput => "trailing input after the document",
            Self::NulCharacter => "the U+0000 escape decodes to a NUL byte",
        }
    }
}

/// A validation failure, with the offset of the first offending byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JsonValidationError {
    /// What was wrong.
    pub kind: JsonValidationErrorKind,
    /// The zero-based byte offset where it was detected.
    pub byte_offset: usize,
}

impl JsonValidationError {
    /// Resolves [`byte_offset`](Self::byte_offset) to a one-based line and
    /// column within `input`.
    ///
    /// The offset alone is what a machine wants; a human reading a diagnostic
    /// wants the position. Derived on demand so the error stays two words wide.
    ///
    /// `column` counts **characters**, not bytes, so it lines up with what a
    /// text editor shows. Pass the same input that was validated; an offset
    /// past the end clamps to the end.
    #[must_use]
    pub fn line_column(&self, input: &str) -> LineColumn {
        let mut offset = self.byte_offset.min(input.len());
        // An offset always lands on a boundary for input this crate validated;
        // walking back keeps a mismatched input from panicking on a slice.
        while !input.is_char_boundary(offset) {
            offset -= 1;
        }

        let prefix = &input[..offset];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let column = prefix
            .rfind('\n')
            .map_or(prefix, |newline| &prefix[newline + 1..])
            .chars()
            .count()
            + 1;
        LineColumn { line, column }
    }
}

impl fmt::Display for JsonValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} at byte {}",
            self.kind.message(),
            self.byte_offset
        )
    }
}

impl std::error::Error for JsonValidationError {}

/// A one-based line and column, as [`JsonValidationError::line_column`] reports
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineColumn {
    /// One-based line number.
    pub line: usize,
    /// One-based column, counted in characters.
    pub column: usize,
}

impl fmt::Display for LineColumn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.line, self.column)
    }
}

/// A document that has passed [`validate_json`].
///
/// The only way to obtain one, which is the point: it is the key that
/// [`JsonReader::new`](crate::JsonReader::new) requires.
///
/// Equality here compares the document's **bytes**, not its JSON: two documents
/// that differ only in whitespace or key order are not equal. That is the useful
/// meaning for a validated buffer — "is this the same input?" — and it is stated
/// because it is not the meaning a reader might assume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Validated<'a> {
    json: &'a str,
}

impl<'a> Validated<'a> {
    /// The validated document.
    #[must_use]
    pub const fn as_str(&self) -> &'a str {
        self.json
    }
}

/// Validates a complete JSON document.
///
/// # Errors
///
/// Returns the kind of the first violation and the byte offset where it was
/// found. Validation is all-or-nothing: a document is either wholly well-formed
/// or rejected.
///
/// ```
/// let document = bc_json::validate_json(r#"{"id":1}"#)?;
/// assert_eq!(document.as_str(), r#"{"id":1}"#);
///
/// // Duplicate keys are rejected, where most parsers silently keep one.
/// assert!(bc_json::validate_json(r#"{"id":1,"id":2}"#).is_err());
/// # Ok::<(), bc_json::JsonValidationError>(())
/// ```
pub fn validate_json(input: &str) -> Result<Validated<'_>, JsonValidationError> {
    let mut validator = Validator {
        input,
        position: 0,
        scratch: String::new(),
    };
    validator.validate()?;
    Ok(Validated { json: input })
}

/// Validates a complete JSON document that has not yet been proven UTF-8.
///
/// This is the shape an untrusted boundary actually has — a request body, a
/// webhook payload, a file — so the UTF-8 check belongs here rather than in the
/// caller, where it would be a second scan whose failure offset gets discarded.
///
/// # Errors
///
/// [`JsonValidationErrorKind::InvalidUtf8`] at the first byte that is not part
/// of a valid sequence, or any error [`validate_json`] reports.
pub fn validate_json_bytes(input: &[u8]) -> Result<Validated<'_>, JsonValidationError> {
    let text = std::str::from_utf8(input).map_err(|error| JsonValidationError {
        kind: JsonValidationErrorKind::InvalidUtf8,
        byte_offset: error.valid_up_to(),
    })?;
    validate_json(text)
}

impl From<JsonValidationError> for bc_error::AnyError {
    /// Carries the failure into the shared error type, keeping the kind and the
    /// offset as structured metadata rather than flattening them into prose.
    ///
    /// Built through `from_detail`, so the error records the **caller's** source
    /// location. It used to be built through `from_location` with the literal
    /// `"bc-json-rs"` in the location slot — a crate name where a source
    /// position belongs — which is the precise misuse the source library
    /// removed its raw positional constructors to prevent.
    fn from(error: JsonValidationError) -> Self {
        bc_error::AnyError::from_detail(
            // This taxonomy's own category, not `kinds::CATEGORY`: these
            // discriminants are a committed wire value, and holding the two
            // taxonomies in separate categories is what makes a numeric
            // collision between them unwritable (see `crate::kinds`).
            bc_error::AnyErrorKind::new(VALIDATION_CATEGORY, error.kind as i32),
            error.kind.message(),
        )
        .with_metadata(
            "byte_offset",
            bc_error::MetadataValue::Uint64(error.byte_offset as u64),
        )
    }
}

/// Decodes the JSON string token at `start` into `output`, returning the
/// position just past its closing quote.
///
/// `output` is cleared first. Shared by the validator (which decodes every key
/// to compare them) and the reader (which decodes the strings a caller asks
/// for), so escape handling has exactly one definition.
pub(crate) fn decode_json_string(
    input: &str,
    start: usize,
    output: &mut String,
) -> Result<usize, JsonValidationError> {
    let bytes = input.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return Err(failure(JsonValidationErrorKind::InvalidString, start));
    }

    output.clear();
    let mut position = start + 1;

    while position < bytes.len() {
        let byte = bytes[position];
        if byte == b'"' {
            return Ok(position + 1);
        }
        if byte < 0x20 {
            return Err(failure(JsonValidationErrorKind::InvalidString, position));
        }
        if byte != b'\\' {
            // Copy the whole run of ordinary bytes at once. It ends at the next
            // quote, backslash, or control byte — all ASCII, so the slice never
            // splits a character. Multi-byte sequences ride through untouched:
            // the input is a `&str`, so they are already valid by construction,
            // which is where the source's hand-written UTF-8 decoder went.
            let run_end = bytes[position..]
                .iter()
                .position(|byte| *byte == b'"' || *byte == b'\\' || *byte < 0x20)
                .map_or(bytes.len(), |offset| position + offset);
            output.push_str(&input[position..run_end]);
            position = run_end;
            continue;
        }

        let escape_offset = position;
        position += 1;
        let Some(&escape) = bytes.get(position) else {
            return Err(failure(
                JsonValidationErrorKind::InvalidEscape,
                escape_offset,
            ));
        };
        position += 1;

        match escape {
            b'"' => output.push('"'),
            b'\\' => output.push('\\'),
            b'/' => output.push('/'),
            b'b' => output.push('\u{8}'),
            b'f' => output.push('\u{c}'),
            b'n' => output.push('\n'),
            b'r' => output.push('\r'),
            b't' => output.push('\t'),
            b'u' => decode_unicode_escape(input, &mut position, escape_offset, output)?,
            _ => {
                return Err(failure(
                    JsonValidationErrorKind::InvalidEscape,
                    escape_offset,
                ));
            }
        }
    }

    Err(failure(JsonValidationErrorKind::InvalidString, bytes.len()))
}

/// Handles a `\uXXXX` escape, including the surrogate pair that a code point
/// above the BMP has to be written as.
fn decode_unicode_escape(
    input: &str,
    position: &mut usize,
    escape_offset: usize,
    output: &mut String,
) -> Result<(), JsonValidationError> {
    const HIGH_SURROGATES: std::ops::RangeInclusive<u32> = 0xD800..=0xDBFF;
    const LOW_SURROGATES: std::ops::RangeInclusive<u32> = 0xDC00..=0xDFFF;

    let bytes = input.as_bytes();
    let code_unit = decode_hex_quad(bytes, position)?;

    if HIGH_SURROGATES.contains(&code_unit) {
        let low_offset = *position;
        if bytes.len().saturating_sub(*position) < 2
            || bytes[*position] != b'\\'
            || bytes[*position + 1] != b'u'
        {
            return Err(failure(
                JsonValidationErrorKind::InvalidUnicodeEscape,
                low_offset,
            ));
        }
        *position += 2;
        let low_surrogate = decode_hex_quad(bytes, position)?;
        if !LOW_SURROGATES.contains(&low_surrogate) {
            return Err(failure(
                JsonValidationErrorKind::InvalidUnicodeEscape,
                low_offset,
            ));
        }
        let code_point = 0x1_0000 + ((code_unit - 0xD800) << 10_u32) + (low_surrogate - 0xDC00);
        push_code_point(output, code_point, low_offset)
    } else if LOW_SURROGATES.contains(&code_unit) {
        // A low surrogate with no high surrogate before it encodes nothing.
        Err(failure(
            JsonValidationErrorKind::InvalidUnicodeEscape,
            escape_offset,
        ))
    } else if code_unit == 0 {
        // The one escape whose decoding is a NUL byte; reported at the `\`
        // that opened it, like every other escape error.
        Err(failure(
            JsonValidationErrorKind::NulCharacter,
            escape_offset,
        ))
    } else {
        push_code_point(output, code_unit, escape_offset)
    }
}

/// Reads exactly four hex digits, advancing `position` past them.
fn decode_hex_quad(bytes: &[u8], position: &mut usize) -> Result<u32, JsonValidationError> {
    if bytes.len().saturating_sub(*position) < 4 {
        return Err(failure(
            JsonValidationErrorKind::InvalidUnicodeEscape,
            bytes.len(),
        ));
    }

    let mut code_unit = 0_u32;
    for index in 0..4 {
        let Some(digit) = hex_digit(bytes[*position + index]) else {
            return Err(failure(
                JsonValidationErrorKind::InvalidUnicodeEscape,
                *position + index,
            ));
        };
        code_unit = (code_unit << 4_u32) | digit;
    }
    *position += 4;
    Ok(code_unit)
}

fn hex_digit(byte: u8) -> Option<u32> {
    let digit = match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => return None,
    };
    Some(u32::from(digit))
}

fn push_code_point(
    output: &mut String,
    code_point: u32,
    offset: usize,
) -> Result<(), JsonValidationError> {
    // Both callers have already excluded the surrogate range, and the pair
    // arithmetic cannot exceed 0x10FFFF, so this conversion cannot fail. It is
    // checked rather than unwrapped anyway: an unwrap would be a panic path
    // reachable only through a future bug in the code above, and returning the
    // error the caller already handles costs nothing.
    let character = char::from_u32(code_point)
        .ok_or_else(|| failure(JsonValidationErrorKind::InvalidUnicodeEscape, offset))?;
    output.push(character);
    Ok(())
}

const fn failure(kind: JsonValidationErrorKind, byte_offset: usize) -> JsonValidationError {
    JsonValidationError { kind, byte_offset }
}

/// The recursive-descent whole-document validator.
///
/// Recursion is safe here only because [`MAX_JSON_NESTING_DEPTH`] is checked
/// before descending — that limit is a stack guard, not a style choice.
struct Validator<'a> {
    input: &'a str,
    position: usize,
    /// Reused across every non-key string, so validating a large document does
    /// not allocate once per string. Keys get their own buffers: they are moved
    /// into the duplicate-detection set.
    scratch: String,
}

impl Validator<'_> {
    fn validate(&mut self) -> Result<(), JsonValidationError> {
        self.position = detail::skip_ws(self.input, self.position);
        if self.position == self.input.len() {
            return Err(failure(
                JsonValidationErrorKind::UnexpectedEnd,
                self.position,
            ));
        }

        self.parse_value(0)?;

        self.position = detail::skip_ws(self.input, self.position);
        if self.position == self.input.len() {
            Ok(())
        } else {
            Err(failure(
                JsonValidationErrorKind::TrailingInput,
                self.position,
            ))
        }
    }

    fn parse_value(&mut self, container_depth: usize) -> Result<(), JsonValidationError> {
        let input = self.input;
        self.position = detail::skip_ws(input, self.position);

        let Some(&token) = input.as_bytes().get(self.position) else {
            return Err(failure(
                JsonValidationErrorKind::UnexpectedEnd,
                self.position,
            ));
        };

        match token {
            b'{' => self.parse_object(container_depth),
            b'[' => self.parse_array(container_depth),
            b'"' => {
                let mut scratch = std::mem::take(&mut self.scratch);
                let result = decode_json_string(input, self.position, &mut scratch);
                self.scratch = scratch;
                self.position = result?;
                Ok(())
            }
            b'-' | b'0'..=b'9' => {
                self.position = json_number_end(input, self.position)?;
                Ok(())
            }
            b't' => self.parse_literal("true"),
            b'f' => self.parse_literal("false"),
            b'n' => self.parse_literal("null"),
            _ => Err(failure(
                JsonValidationErrorKind::UnexpectedToken,
                self.position,
            )),
        }
    }

    fn parse_object(&mut self, container_depth: usize) -> Result<(), JsonValidationError> {
        if container_depth >= MAX_JSON_NESTING_DEPTH {
            return Err(failure(
                JsonValidationErrorKind::NestingTooDeep,
                self.position,
            ));
        }

        let input = self.input;
        let bytes = input.as_bytes();
        self.position += 1;
        self.position = detail::skip_ws(input, self.position);
        if bytes.get(self.position) == Some(&b'}') {
            self.position += 1;
            return Ok(());
        }

        // Keys are compared **decoded**, so `"a"` and `"a"` collide. That
        // is what makes the reader's first-match lookup unambiguous.
        let mut keys = HashSet::new();

        while self.position < bytes.len() {
            self.position = detail::skip_ws(input, self.position);
            if self.position == bytes.len() {
                return Err(failure(
                    JsonValidationErrorKind::UnexpectedEnd,
                    self.position,
                ));
            }

            let key_offset = self.position;
            let mut key = String::new();
            self.position = decode_json_string(input, self.position, &mut key)?;
            if !keys.insert(key) {
                return Err(failure(JsonValidationErrorKind::DuplicateKey, key_offset));
            }

            self.expect_byte(b':')?;
            self.parse_value(container_depth + 1)?;

            self.position = detail::skip_ws(input, self.position);
            if self.position == bytes.len() {
                return Err(failure(
                    JsonValidationErrorKind::UnexpectedEnd,
                    self.position,
                ));
            }
            if bytes[self.position] == b'}' {
                self.position += 1;
                return Ok(());
            }
            if bytes[self.position] != b',' {
                return Err(failure(
                    JsonValidationErrorKind::UnexpectedToken,
                    self.position,
                ));
            }
            self.position += 1;
        }

        Err(failure(
            JsonValidationErrorKind::UnexpectedEnd,
            self.position,
        ))
    }

    fn parse_array(&mut self, container_depth: usize) -> Result<(), JsonValidationError> {
        if container_depth >= MAX_JSON_NESTING_DEPTH {
            return Err(failure(
                JsonValidationErrorKind::NestingTooDeep,
                self.position,
            ));
        }

        let input = self.input;
        let bytes = input.as_bytes();
        self.position += 1;
        self.position = detail::skip_ws(input, self.position);
        if bytes.get(self.position) == Some(&b']') {
            self.position += 1;
            return Ok(());
        }

        while self.position < bytes.len() {
            self.parse_value(container_depth + 1)?;

            self.position = detail::skip_ws(input, self.position);
            if self.position == bytes.len() {
                return Err(failure(
                    JsonValidationErrorKind::UnexpectedEnd,
                    self.position,
                ));
            }
            if bytes[self.position] == b']' {
                self.position += 1;
                return Ok(());
            }
            if bytes[self.position] != b',' {
                return Err(failure(
                    JsonValidationErrorKind::UnexpectedToken,
                    self.position,
                ));
            }
            self.position += 1;
        }

        Err(failure(
            JsonValidationErrorKind::UnexpectedEnd,
            self.position,
        ))
    }

    /// Skips whitespace and consumes `expected`, or reports why it could not.
    fn expect_byte(&mut self, expected: u8) -> Result<(), JsonValidationError> {
        self.position = detail::skip_ws(self.input, self.position);
        match self.input.as_bytes().get(self.position) {
            None => Err(failure(
                JsonValidationErrorKind::UnexpectedEnd,
                self.position,
            )),
            Some(&byte) if byte == expected => {
                self.position += 1;
                Ok(())
            }
            Some(_) => Err(failure(
                JsonValidationErrorKind::UnexpectedToken,
                self.position,
            )),
        }
    }

    fn parse_literal(&mut self, literal: &str) -> Result<(), JsonValidationError> {
        let remaining = &self.input[self.position..];

        if remaining.len() < literal.len() {
            // A prefix of the literal means the document was cut short; anything
            // else was never going to be this literal.
            return Err(if literal.starts_with(remaining) {
                failure(JsonValidationErrorKind::UnexpectedEnd, self.input.len())
            } else {
                failure(JsonValidationErrorKind::UnexpectedToken, self.position)
            });
        }
        if !remaining.starts_with(literal) {
            return Err(failure(
                JsonValidationErrorKind::UnexpectedToken,
                self.position,
            ));
        }

        self.position += literal.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{JsonValidationError, JsonValidationErrorKind, validate_json};

    /// Every kind must be reachable from a match with no wildcard arm, so
    /// adding a variant fails to compile until it is handled here. The enum is
    /// `#[non_exhaustive]` for consumers; in-crate it stays exhaustive.
    #[test]
    fn every_error_kind_has_a_message() {
        let kinds = [
            JsonValidationErrorKind::UnexpectedEnd,
            JsonValidationErrorKind::UnexpectedToken,
            JsonValidationErrorKind::InvalidNumber,
            JsonValidationErrorKind::InvalidString,
            JsonValidationErrorKind::InvalidEscape,
            JsonValidationErrorKind::InvalidUnicodeEscape,
            JsonValidationErrorKind::InvalidUtf8,
            JsonValidationErrorKind::DuplicateKey,
            JsonValidationErrorKind::NestingTooDeep,
            JsonValidationErrorKind::TrailingInput,
            JsonValidationErrorKind::NulCharacter,
        ];

        // Deliberately no wildcard arm: this is the guard.
        for kind in kinds {
            let described = match kind {
                JsonValidationErrorKind::UnexpectedEnd
                | JsonValidationErrorKind::UnexpectedToken
                | JsonValidationErrorKind::InvalidNumber
                | JsonValidationErrorKind::InvalidString
                | JsonValidationErrorKind::InvalidEscape
                | JsonValidationErrorKind::InvalidUnicodeEscape
                | JsonValidationErrorKind::InvalidUtf8
                | JsonValidationErrorKind::DuplicateKey
                | JsonValidationErrorKind::NestingTooDeep
                | JsonValidationErrorKind::TrailingInput
                | JsonValidationErrorKind::NulCharacter => kind.message(),
            };
            assert!(!described.is_empty(), "{kind:?} has an empty message");
        }
    }

    #[test]
    fn line_column_counts_characters_not_bytes() {
        let input = "{\n  \"é\": nope\n}";
        let error = validate_json(input).expect_err("`nope` is not a JSON value");
        let position = error.line_column(input);
        assert_eq!(position.line, 2);
        // `  "é": ` is 7 characters, so the offending byte is the 8th column
        // even though `é` occupies two bytes.
        assert_eq!(position.column, 8);
    }

    #[test]
    fn display_names_the_kind_and_the_offset() {
        let error = JsonValidationError {
            kind: JsonValidationErrorKind::DuplicateKey,
            byte_offset: 8,
        };
        assert_eq!(error.to_string(), "duplicate object key at byte 8");
    }
}
