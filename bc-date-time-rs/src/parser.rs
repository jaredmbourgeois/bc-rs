//! Strict ISO 8601 parsing: text → [`Epoch`].
//!
//! Ported from `libs/foundation/date_time/src/date_parser.{hpp,cpp}`, and
//! deliberately unforgiving: the contract is UTC, so a timezone offset is
//! rejected rather than interpreted. `None` means "this is not the format" —
//! never a guess at what the caller might have meant.
//!
//! **Which formatter each parser inverts**, because "the inverse of the
//! `format_iso_*` helpers" was the claim here and it is not true of all three:
//!
//! - [`parse_iso_date`] inverts [`format_iso_date`](crate::format_iso_date).
//! - [`parse_iso_timestamp`] inverts **nothing this crate emits**. It reads the
//!   wire form, `YYYY-MM-DDTHH:MM:SS[.fff][Z]`, which is what other systems
//!   produce; [`format_iso_timestamp`](crate::format_iso_timestamp) renders
//!   `2026-02-17 14:30:45 UTC` for a person to read — a space instead of the
//!   `T` and a ` UTC` suffix, both of which this module rejects. A caller that
//!   logged with one and read back with the other got `None` on every line.
//! - [`format_iso_week_version`](crate::format_iso_week_version) has no parser
//!   at all; it is a version stamp, not an instant.
//!
//! `roundtrip` in this crate's test suite pins that table, so the prose above
//! cannot drift from the code again.
//!
//! These return [`Option`] rather than a `Result` carrying a message, matching
//! the C++: a caller that wants a diagnostic wraps the `None`, and keeping the
//! failure shapeless is what lets this crate stay dependency-free.

use crate::time::{DateComponents, Epoch, Month, components_to_epoch, days_in_month};

/// Parses `YYYY-MM-DD` as midnight UTC.
///
/// ```
/// use bc_date_time::{Epoch, parse_iso_date};
///
/// assert_eq!(parse_iso_date("1970-01-01"), Some(Epoch::ZERO));
/// assert_eq!(parse_iso_date("2026-02-30"), None); // February has no 30th
/// assert_eq!(parse_iso_date("2026-2-17"), None); // fields are fixed-width
/// ```
#[must_use]
pub fn parse_iso_date(text: &str) -> Option<Epoch> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = digits(text, 0, 4)?;
    let month = digits(text, 5, 2)?;
    let day = digits(text, 8, 2)?;
    to_epoch(year, month, day, 0, 0, 0, 0)
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fff][Z]` as a UTC instant.
///
/// Fractional seconds are optional: the first three digits become
/// milliseconds, right-padded if fewer and truncated if more. A trailing `Z` is
/// optional. **Any other suffix is rejected** — including a `+05:00` offset,
/// because this crate has no timezone concept and silently treating an offset
/// timestamp as UTC would be a wrong answer rather than a refused one.
///
/// ```
/// use bc_date_time::parse_iso_timestamp;
///
/// let base = parse_iso_timestamp("2026-02-17T14:30:45Z").expect("valid");
/// assert_eq!(parse_iso_timestamp("2026-02-17T14:30:45"), Some(base)); // Z optional
/// assert_eq!(parse_iso_timestamp("2026-02-17T14:30:45.5Z").map(|e| e.millis() - base.millis()),
///            Some(500)); // ".5" is 500ms, not 5ms
/// assert_eq!(parse_iso_timestamp("2026-02-17T14:30:45+05:00"), None); // offsets refused
/// ```
#[must_use]
pub fn parse_iso_timestamp(text: &str) -> Option<Epoch> {
    let bytes = text.as_bytes();
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = digits(text, 0, 4)?;
    let month = digits(text, 5, 2)?;
    let day = digits(text, 8, 2)?;
    let hour = digits(text, 11, 2)?;
    let minute = digits(text, 14, 2)?;
    let second = digits(text, 17, 2)?;

    let mut rest = text.get(19..)?;
    let mut millis = 0_i64;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digit_count = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digit_count == 0 {
            // A lone '.' with no digits is malformed, not "zero milliseconds".
            return None;
        }
        let (fraction_digits, remainder) = fraction.split_at(digit_count);
        // Truncate past three digits, right-pad below three: ".5" is 500ms.
        let mut scaled = String::with_capacity(3);
        scaled.push_str(fraction_digits.get(..3).unwrap_or(fraction_digits));
        while scaled.len() < 3 {
            scaled.push('0');
        }
        millis = scaled.parse().ok()?;
        rest = remainder;
    }

    if !rest.is_empty() && rest != "Z" {
        return None;
    }

    to_epoch(year, month, day, hour, minute, second, millis)
}

/// Reads a fixed-width, all-digit field.
///
/// Requiring every byte to be a digit is what keeps each field non-negative,
/// and it rejects a stray sign or letter that the separator checks would let
/// through (`2026-02-1x` has a `-` in the right places).
fn digits(text: &str, offset: usize, width: usize) -> Option<i64> {
    let field = text.get(offset..offset + width)?;
    if field.len() != width || !field.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    field.parse().ok()
}

/// Range-checks calendar fields and converts to a UTC epoch.
///
/// Every argument is non-negative because [`digits`] guarantees it.
fn to_epoch(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    millis: i64,
) -> Option<Epoch> {
    let year = i32::try_from(year).ok()?;
    let month = u8::try_from(month).ok().and_then(Month::from_number)?;
    let day = u8::try_from(day).ok()?;
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }
    // 60 is allowed: a leap second is legal ISO 8601 text. It normalizes into
    // second 0 of the next minute on conversion, which is the same thing the
    // C++ `<chrono>` arithmetic does — this crate has no leap-second table and
    // does not pretend to.
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let hour = u8::try_from(hour).ok()?;
    let minute = u8::try_from(minute).ok()?;
    let second = u8::try_from(second).ok()?;
    let millis = u16::try_from(millis).ok()?;

    Some(components_to_epoch(&DateComponents::new(
        year, month, day, hour, minute, second, millis,
    )))
}
