//! The RFC 8259 number grammar, in one place.
//!
//! The C++ source has two number policies that disagree: `validate_json`
//! enforces this grammar directly, while `JsonReader`'s scalar accessors
//! delegate to `bc::parse_i64` / `bc::parse_f64` (`from_chars` / `strtod`),
//! which accept `0x1p3` and reject a leading `+` for different reasons than the
//! grammar does. The port gives both halves the same definition, so what the
//! reader will parse is exactly what the validator will accept.

use crate::validate::{JsonValidationError, JsonValidationErrorKind};

/// Returns the position just past the RFC 8259 number starting at `position`.
///
/// The grammar is `-? (0 | [1-9][0-9]*) (. [0-9]+)? ([eE] [+-]? [0-9]+)?` —
/// no leading `+`, no leading zeros, no bare `.5`, no hex, no `NaN`/`Infinity`.
pub(crate) fn json_number_end(input: &str, position: usize) -> Result<usize, JsonValidationError> {
    let bytes = input.as_bytes();
    let start = position;
    let mut position = position;

    let failure = |kind, offset| {
        Err(JsonValidationError {
            kind,
            byte_offset: offset,
        })
    };

    if position >= bytes.len() {
        return failure(JsonValidationErrorKind::InvalidNumber, position);
    }

    if bytes[position] == b'-' {
        position += 1;
        if position == bytes.len() {
            return failure(JsonValidationErrorKind::InvalidNumber, position);
        }
    }

    // Integer part: a lone `0`, or a nonzero digit and its run. A `0` followed
    // by another digit is the leading-zero rejection.
    if bytes[position] == b'0' {
        position += 1;
        if position < bytes.len() && bytes[position].is_ascii_digit() {
            return failure(JsonValidationErrorKind::InvalidNumber, position);
        }
    } else if bytes[position].is_ascii_digit() {
        while position < bytes.len() && bytes[position].is_ascii_digit() {
            position += 1;
        }
    } else {
        return failure(JsonValidationErrorKind::InvalidNumber, start);
    }

    if position < bytes.len() && bytes[position] == b'.' {
        position += 1;
        if position == bytes.len() || !bytes[position].is_ascii_digit() {
            return failure(JsonValidationErrorKind::InvalidNumber, position);
        }
        while position < bytes.len() && bytes[position].is_ascii_digit() {
            position += 1;
        }
    }

    if position < bytes.len() && (bytes[position] == b'e' || bytes[position] == b'E') {
        position += 1;
        if position < bytes.len() && (bytes[position] == b'+' || bytes[position] == b'-') {
            position += 1;
        }
        if position == bytes.len() || !bytes[position].is_ascii_digit() {
            return failure(JsonValidationErrorKind::InvalidNumber, position);
        }
        while position < bytes.len() && bytes[position].is_ascii_digit() {
            position += 1;
        }
    }

    Ok(position)
}

/// Whether `token` is a complete RFC 8259 number and nothing else.
fn is_json_number(token: &str) -> bool {
    matches!(json_number_end(token, 0), Ok(end) if end == token.len())
}

/// Parses a delimiter-bounded token as an `i64`.
///
/// `None` unless the token is a complete JSON number that is an exact `i64` —
/// so `1.5`, `1e2`, and `9223372036854775808` are all `None`, matching the
/// source's `from_chars`-backed accessor.
pub(crate) fn parse_json_i64(token: &str) -> Option<i64> {
    if !is_json_number(token) {
        return None;
    }
    token.parse::<i64>().ok()
}

/// Parses a delimiter-bounded token as an `f64`.
///
/// `None` unless the token is a complete JSON number **and** converts to a
/// finite `f64`. The finiteness filter is what keeps `1e400` — which the
/// grammar accepts — from becoming an infinity that no JSON writer can emit
/// back; the source rejects the same input via `strtod`'s `ERANGE`.
pub(crate) fn parse_json_f64(token: &str) -> Option<f64> {
    if !is_json_number(token) {
        return None;
    }
    token.parse::<f64>().ok().filter(|value| value.is_finite())
}
