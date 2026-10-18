//! Ported from `libs/foundation/date_time/test/date_parser_test.cpp`.
//!
//! The theme is refusal: every rejection case here is a shape that *looks*
//! parseable, and a lenient parser would return a plausible wrong answer for.

use bc_date_time::{
    DateFormatBuilder, Epoch, format_iso_date, format_iso_timestamp, parse_iso_date,
    parse_iso_timestamp,
};

/// 2026-02-17T14:30:45.123Z, a Tuesday.
const FEB_17_2026: Epoch = Epoch::from_millis(1_771_338_645_123);
/// 2026-02-17T00:00:00.000Z — midnight of that day.
const FEB_17_MIDNIGHT: Epoch = Epoch::from_millis(1_771_286_400_000);

/// An instant a fixed offset from the fixture, so a test can name a delta in
/// milliseconds instead of restating a 13-digit literal.
fn offset_from_feb_17(millis: i64) -> Epoch {
    Epoch::from_millis(FEB_17_2026.millis() + millis)
}

// --- parse_iso_date ------------------------------------------------------

#[test]
fn a_valid_date_parses_to_midnight_utc() {
    assert_eq!(parse_iso_date("2026-02-17"), Some(FEB_17_MIDNIGHT));
}

#[test]
fn a_date_round_trips_with_format_iso_date() {
    // `format_iso_date` drops the time of day, so parsing it back is midnight.
    assert_eq!(
        parse_iso_date(&format_iso_date(FEB_17_2026)),
        Some(FEB_17_MIDNIGHT)
    );
}

#[test]
fn the_epoch_date_parses_to_zero() {
    assert_eq!(parse_iso_date("1970-01-01"), Some(Epoch::ZERO));
}

#[test]
fn a_malformed_date_is_refused_rather_than_guessed() {
    let refused = [
        "2026-2-17",   // not zero-padded, so the wrong length
        "2026/02/17",  // wrong separators
        "2026-13-01",  // month out of range
        "2026-02-30",  // February has no 30th
        "2026-02-1x",  // a non-digit inside a field
        "2026-02-17T", // trailing junk
        "",
    ];
    for text in refused {
        assert_eq!(parse_iso_date(text), None, "{text:?} should not parse");
    }
}

#[test]
fn every_impossible_day_of_february_is_refused() {
    // The day check runs against `days_in_month`, so it is leap-year aware
    // rather than a flat 1..=31 range.
    assert!(parse_iso_date("2024-02-29").is_some()); // 2024 is a leap year
    assert_eq!(parse_iso_date("2026-02-29"), None); // 2026 is not
    assert_eq!(parse_iso_date("1900-02-29"), None); // nor is 1900
    assert!(parse_iso_date("2000-02-29").is_some()); // but 2000 is
    assert_eq!(parse_iso_date("2026-04-31"), None); // April has 30 days
}

// --- parse_iso_timestamp -------------------------------------------------

#[test]
fn a_timestamp_with_z_and_no_fraction_parses() {
    assert_eq!(
        parse_iso_timestamp("2026-02-17T14:30:45Z"),
        Some(offset_from_feb_17(-123))
    );
}

#[test]
fn a_timestamp_with_milliseconds_parses() {
    assert_eq!(
        parse_iso_timestamp("2026-02-17T14:30:45.123Z"),
        Some(FEB_17_2026)
    );
}

#[test]
fn a_short_fraction_right_pads_to_milliseconds() {
    // ".5" is 500ms, not 5ms — the single most likely place to get this wrong.
    assert_eq!(
        parse_iso_timestamp("2026-02-17T14:30:45.5Z"),
        Some(offset_from_feb_17(-123 + 500))
    );
    assert_eq!(
        parse_iso_timestamp("2026-02-17T14:30:45.05Z"),
        Some(offset_from_feb_17(-123 + 50))
    );
}

#[test]
fn a_long_fraction_truncates_at_milliseconds() {
    assert_eq!(
        parse_iso_timestamp("2026-02-17T14:30:45.123456Z"),
        Some(FEB_17_2026)
    );
    // Truncation, never rounding: .1239 stays 123ms.
    assert_eq!(
        parse_iso_timestamp("2026-02-17T14:30:45.1239Z"),
        Some(FEB_17_2026)
    );
}

#[test]
fn the_trailing_z_is_optional() {
    assert_eq!(
        parse_iso_timestamp("2026-02-17T14:30:45"),
        Some(offset_from_feb_17(-123))
    );
    assert_eq!(
        parse_iso_timestamp("2026-02-17T14:30:45.123"),
        Some(FEB_17_2026)
    );
}

#[test]
fn a_timezone_offset_is_refused_never_reinterpreted_as_utc() {
    // The contract is UTC. Treating "+05:00" as UTC would be a wrong answer;
    // refusing it makes the caller say what they meant.
    let refused = [
        "2026-02-17T14:30:45+05:00",
        "2026-02-17T14:30:45.123+0000",
        "2026-02-17T14:30:45-08:00",
        "2026-02-17T14:30:45z", // lowercase is not the ISO designator
    ];
    for text in refused {
        assert_eq!(parse_iso_timestamp(text), None, "{text:?} should not parse");
    }
}

#[test]
fn a_malformed_timestamp_is_refused_rather_than_guessed() {
    // The separator rows carry weight nothing else does. The mutation property
    // in tests/properties.rs canonicalizes by slicing fields at fixed positions
    // and never looks at bytes 4/7/10/13/16, so a parser that accepted
    // "2026x02-17T14:30:45Z" would yield the same instant and pass it. Grammar
    // strictness is pinned here or nowhere.
    let refused = [
        "2026-02-17 14:30:45",   // a space where the 'T' belongs
        "2026x02-17T14:30:45Z",  // a wrong byte where a date '-' belongs
        "2026-02-17T14x30:45Z",  // a wrong byte where a time ':' belongs
        "2026-02-17T14:30",      // too short
        "2026-02-17T25:30:45Z",  // hour out of range
        "2026-02-17T14:61:45Z",  // minute out of range
        "2026-02-17T14:30:61Z",  // second past the leap-second allowance
        "2026-02-17T14:30:45.Z", // a lone fractional dot
        "2026-02-17T14:30:45.",  // ...with nothing after it at all
        "2026-02-30T14:30:45Z",  // February has no 30th
        "garbage",
        "",
    ];
    for text in refused {
        assert_eq!(parse_iso_timestamp(text), None, "{text:?} should not parse");
    }
}

#[test]
fn a_leap_second_is_accepted_and_normalizes_into_the_next_minute() {
    // 60 is legal ISO 8601 text. This crate has no leap-second table, so it
    // normalizes rather than pretending to represent one — the same thing the
    // C++ `<chrono>` arithmetic does.
    assert_eq!(
        parse_iso_timestamp("2026-02-17T14:30:60Z"),
        parse_iso_timestamp("2026-02-17T14:31:00Z")
    );
}

#[test]
fn the_epoch_timestamp_parses_to_zero_in_both_spellings() {
    assert_eq!(
        parse_iso_timestamp("1970-01-01T00:00:00Z"),
        Some(Epoch::ZERO)
    );
    assert_eq!(
        parse_iso_timestamp("1970-01-01T00:00:00.000Z"),
        Some(Epoch::ZERO)
    );
}

#[test]
fn a_timestamp_round_trips_with_the_iso8601_formatter() {
    // The formatter emits exactly the shape the parser accepts; that they are
    // inverses is the pair's whole contract.
    let formatter = DateFormatBuilder::iso8601();
    for epoch in [FEB_17_2026, Epoch::ZERO, FEB_17_MIDNIGHT] {
        let rendered = formatter.format(epoch);
        assert_eq!(
            parse_iso_timestamp(&rendered),
            Some(epoch),
            "{rendered} failed to round-trip"
        );
    }
}

#[test]
fn multibyte_text_is_refused_without_panicking() {
    // Byte-indexed parsers are where a `&str` slice on a non-boundary panics.
    // These are refusals, not crashes.
    for text in ["2026-02-17T14:30:45é", "2026-02-1é", "日本語ですよね", "🕐"] {
        assert_eq!(parse_iso_timestamp(text), None, "{text:?}");
        assert_eq!(parse_iso_date(text), None, "{text:?}");
    }
}

/// The invert-pair table `parser.rs`'s header states, pinned.
///
/// The header used to claim this module was "the inverse of the `format_iso_*`
/// helpers" — true of one of them. `format_iso_timestamp` renders a *human*
/// timestamp, `2026-02-17 14:30:45 UTC`, and `parse_iso_timestamp` reads the
/// *wire* form with a `T` and no suffix, so a caller who logged with one and
/// read back with the other got `None` on every line. Prose could say that
/// wrongly for as long as nothing checked; this is the check.
#[test]
fn each_parser_inverts_exactly_the_formatter_the_module_header_names() {
    let epoch = FEB_17_2026;

    // `format_iso_date` ↔ `parse_iso_date`: a genuine pair.
    assert_eq!(
        parse_iso_date(&format_iso_date(epoch)),
        Some(FEB_17_MIDNIGHT),
        "the date formatter and the date parser invert each other"
    );

    // `format_iso_timestamp` is human-facing and this module does not read it.
    let human = format_iso_timestamp(epoch);
    // The separator by position, not by searching for a `T` — `UTC` has one,
    // which is exactly the kind of near-miss this whole test exists to catch.
    assert_eq!(
        human.as_bytes()[10],
        b' ',
        "the human timestamp separates date from time with a space, not a `T`: {human}"
    );
    assert!(
        human.ends_with(" UTC"),
        "and carries a ` UTC` suffix the wire form never has: {human}"
    );
    assert_eq!(
        parse_iso_timestamp(&human),
        None,
        "and it is refused rather than half-read: {human}"
    );

    // What `parse_iso_timestamp` *does* invert is the wire form, which this
    // crate does not emit — so the round trip is stated against literal text.
    assert_eq!(
        parse_iso_timestamp("2026-02-17T14:30:45.123Z"),
        Some(epoch),
        "the wire form is the one this parser is the inverse of"
    );
}
