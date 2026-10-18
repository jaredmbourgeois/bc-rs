//! Recursive-descent traversal helpers — the structural layer that
//! [`JsonReader`](crate::JsonReader)'s flat key lookups do not cover.
//!
//! These walk nested objects and arrays of objects a step at a time. They are
//! the primitives every higher-level accessor is built from, exposed because
//! the C++ source exposes them: a consumer with a bespoke scalar accessor
//! (`bourgeois_concern`'s `floating_strict`) reuses the scan rather than
//! re-deriving it.
//!
//! Every function here works on **unvalidated** input and reports failure as
//! `None`. Soundness of a *lazy* lookup is the validator's job: see the
//! `Validated` typestate on [`validate_json`](crate::validate_json).

use crate::validate::decode_json_string;

/// The bytes JSON counts as whitespace between tokens.
const WHITESPACE: &[u8] = b" \t\n\r";

/// The bytes that end an unquoted scalar token (number, `true`, `false`, `null`).
const SCALAR_DELIMITERS: &[u8] = b",}] \t\n\r";

/// Returns the first position at or after `position` that is not JSON whitespace.
///
/// Returns `input.len()` when only whitespace remains.
#[must_use]
pub fn skip_ws(input: &str, position: usize) -> usize {
    let bytes = input.as_bytes();
    let mut position = position.min(bytes.len());
    while position < bytes.len() && WHITESPACE.contains(&bytes[position]) {
        position += 1;
    }
    position
}

/// Decodes the JSON string token at `position` into `output`, returning the
/// position just past its closing quote.
///
/// `output` is cleared first. It is an out-parameter rather than a return value
/// so a caller scanning many strings can reuse one buffer, which is what the
/// validator does for every key in a document.
///
/// Returns `None` if `position` does not start a well-formed string token.
pub fn parse_json_string(input: &str, position: usize, output: &mut String) -> Option<usize> {
    let position = skip_ws(input, position);
    decode_json_string(input, position, output).ok()
}

/// Reads the delimiter-bounded token at `position` as an `i64`, returning the
/// value and the position just past it.
///
/// The token must be a complete RFC 8259 number that fits an `i64`; anything
/// else is `None`.
#[must_use]
pub fn parse_json_number(input: &str, position: usize) -> Option<(i64, usize)> {
    let position = skip_ws(input, position);
    let end = scalar_token_end(input, position);
    let value = crate::number::parse_json_i64(&input[position..end])?;
    Some((value, end))
}

/// Reads the literal `true` or `false` at `position`, returning the value and
/// the position just past it.
#[must_use]
pub fn parse_json_bool(input: &str, position: usize) -> Option<(bool, usize)> {
    let position = skip_ws(input, position);
    let end = scalar_token_end(input, position);
    match &input[position..end] {
        "true" => Some((true, end)),
        "false" => Some((false, end)),
        _ => None,
    }
}

/// Returns the position just past the colon of a **direct** `"key":` member of
/// the object starting at `position`, or `None` if the object has no such
/// member.
///
/// The scan skips nested values rather than searching the whole slice, so a
/// matching key inside a nested object is not found — this answers "what is
/// this object's `key`", never "is `key` anywhere below here". To reach a
/// nested key, take the nested slice first and search that.
///
/// Only the **first** member with a matching key is reported. That is
/// unambiguous exactly when the document has no duplicate keys, which is what
/// [`validate_json`](crate::validate_json) guarantees and why the reader is
/// gated on it.
#[must_use]
pub fn find_field(input: &str, position: usize, key: &str) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut position = skip_ws(input, position);
    if bytes.get(position) != Some(&b'{') {
        return None;
    }
    position += 1;

    let mut field_name = String::new();
    while position < bytes.len() {
        position = skip_ws(input, position);
        match bytes.get(position) {
            Some(&b'"') => {}
            _ => return None,
        }

        let name_end = parse_json_string(input, position, &mut field_name)?;
        position = skip_ws(input, name_end);
        if bytes.get(position) != Some(&b':') {
            return None;
        }
        let value_position = position + 1;
        if field_name == key {
            return Some(value_position);
        }

        position = skip_json_value(input, value_position)?;
        position = skip_ws(input, position);
        if bytes.get(position) == Some(&b',') {
            position += 1;
        }
    }

    None
}

/// Returns the position just past the complete JSON value starting at
/// `position`, or `None` on malformed input.
///
/// Handles every value kind. Objects and arrays are brace/bracket counted with
/// string awareness, so a `}` inside a string value does not end the object
/// early — the defect every hand-rolled scanner this replaced had.
#[must_use]
pub fn skip_json_value(input: &str, position: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let position = skip_ws(input, position);

    match bytes.get(position)? {
        b'"' => {
            let mut ignored = String::new();
            parse_json_string(input, position, &mut ignored)
        }
        b'{' => skip_container(bytes, position, b'{', b'}'),
        b'[' => skip_container(bytes, position, b'[', b']'),
        // Number, `true`, `false`, or `null`: bounded by the next delimiter.
        _ => Some(scalar_token_end(input, position)),
    }
}

/// Returns the end of the unquoted scalar token starting at `position` — the
/// next delimiter, or the end of input.
pub(crate) fn scalar_token_end(input: &str, position: usize) -> usize {
    let bytes = input.as_bytes();
    let position = position.min(bytes.len());
    bytes[position..]
        .iter()
        .position(|byte| SCALAR_DELIMITERS.contains(byte))
        .map_or(bytes.len(), |offset| position + offset)
}

/// Counts balanced `open`/`close` bytes from the container opening at
/// `position`, ignoring any that appear inside a string.
///
/// Returns the index **after** the balancing close, or `None` if the input runs
/// out first — or if `position` is not the `open` byte, which is the caller's
/// precondition made explicit rather than assumed.
///
/// **The crate's one string-aware brace matcher.** `reader::find_matching`
/// was a second, differing only in whether it returned the close's index or the
/// one after it, and in how it spelled the same string-and-escape skipping —
/// two implementations of one scan in the module whose own header says the
/// structural primitives live here and are reused rather than re-derived.
pub(crate) fn skip_container(bytes: &[u8], position: usize, open: u8, close: u8) -> Option<usize> {
    if bytes.get(position) != Some(&open) {
        return None;
    }
    let mut position = position + 1;
    let mut depth = 1_usize;

    while position < bytes.len() && depth > 0 {
        let byte = bytes[position];
        if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
        } else if byte == b'"' {
            position += 1;
            while position < bytes.len() && bytes[position] != b'"' {
                // A backslash consumes the byte after it, so an escaped quote
                // does not end the string.
                if bytes[position] == b'\\' {
                    position += 1;
                }
                position += 1;
            }
        }
        position += 1;
    }

    if depth == 0 { Some(position) } else { None }
}
