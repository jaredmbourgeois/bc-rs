//! Ported from `libs/foundation/date_time/test/date_formatter_test.cpp`.
//!
//! Every expected string is the C++ suite's, character for character.

use bc_date_time::{
    DateComponents, DateFormatBuilder, DayStyle, Epoch, HourStyle, MinSecStyle, Month, MonthStyle,
    WeekdayStyle, YearStyle, components_to_epoch, format_iso_date, format_iso_timestamp,
    format_iso_week_version,
};

/// 2026-02-17T14:30:45.123Z, a Tuesday.
const FEB_17_2026: Epoch = Epoch::from_millis(1_771_338_645_123);
/// 2000-01-01T00:00:00.000Z, a Saturday.
const Y2K: Epoch = Epoch::from_millis(946_684_800_000);

/// The epoch for a wall-clock date, so a test can name an hour directly.
fn at(year: i32, month: Month, day: u8, hour: u8) -> Epoch {
    components_to_epoch(&DateComponents::new(year, month, day, hour, 0, 0, 0))
}

// --- Static factories ----------------------------------------------------

#[test]
fn iso8601_renders_the_full_instant() {
    let formatter = DateFormatBuilder::iso8601();
    assert_eq!(formatter.format(FEB_17_2026), "2026-02-17T14:30:45.123Z");
}

#[test]
fn iso8601_keeps_every_field_at_midnight() {
    let formatter = DateFormatBuilder::iso8601();
    assert_eq!(formatter.format(Y2K), "2000-01-01T00:00:00.000Z");
}

#[test]
fn us_short_is_slash_separated_and_unpadded() {
    assert_eq!(
        DateFormatBuilder::us_short().format(FEB_17_2026),
        "2/17/2026"
    );
}

#[test]
fn us_short_does_not_pad_a_single_digit_month_or_day() {
    let epoch = at(2026, Month::January, 3, 0);
    assert_eq!(DateFormatBuilder::us_short().format(epoch), "1/3/2026");
}

#[test]
fn us_long_spells_the_month() {
    assert_eq!(
        DateFormatBuilder::us_long().format(FEB_17_2026),
        "February 17, 2026"
    );
}

#[test]
fn us_long_with_weekday_leads_with_the_day_name() {
    assert_eq!(
        DateFormatBuilder::us_long_with_weekday().format(FEB_17_2026),
        "Tuesday, February 17, 2026"
    );
}

#[test]
fn us_datetime_uses_a_twelve_hour_clock() {
    assert_eq!(
        DateFormatBuilder::us_datetime().format(FEB_17_2026),
        "February 17, 2026 2:30 PM"
    );
}

#[test]
fn us_datetime_seconds_adds_the_seconds() {
    assert_eq!(
        DateFormatBuilder::us_datetime_seconds().format(FEB_17_2026),
        "February 17, 2026 2:30:45 PM"
    );
}

#[test]
fn iso_date_renders_the_calendar_date() {
    let formatter = DateFormatBuilder::iso_date();
    assert_eq!(formatter.format(FEB_17_2026), "2026-02-17");
    assert_eq!(formatter.format(Y2K), "2000-01-01");
    assert_eq!(formatter.format(Epoch::ZERO), "1970-01-01");
}

#[test]
fn iso_datetime_minute_stops_at_minutes() {
    let formatter = DateFormatBuilder::iso_datetime_minute();
    assert_eq!(formatter.format(FEB_17_2026), "2026-02-17 14:30");
    assert_eq!(formatter.format(Y2K), "2000-01-01 00:00");
}

// --- The twelve-hour boundaries ------------------------------------------

#[test]
fn noon_is_twelve_pm() {
    let formatter = DateFormatBuilder::us_datetime();
    assert_eq!(
        formatter.format(at(2026, Month::January, 1, 12)),
        "January 1, 2026 12:00 PM"
    );
}

#[test]
fn midnight_is_twelve_am() {
    assert_eq!(
        DateFormatBuilder::us_datetime().format(Y2K),
        "January 1, 2000 12:00 AM"
    );
}

#[test]
fn thirteen_hundred_is_one_pm() {
    let formatter = DateFormatBuilder::us_datetime();
    assert_eq!(
        formatter.format(at(2026, Month::January, 1, 13)),
        "January 1, 2026 1:00 PM"
    );
}

#[test]
fn eleven_hundred_is_eleven_am() {
    let formatter = DateFormatBuilder::us_datetime();
    assert_eq!(
        formatter.format(at(2026, Month::January, 1, 11)),
        "January 1, 2026 11:00 AM"
    );
}

// --- Degenerate builders -------------------------------------------------

#[test]
fn an_empty_builder_renders_nothing() {
    assert_eq!(DateFormatBuilder::new().build().format(FEB_17_2026), "");
}

#[test]
fn a_literal_only_builder_ignores_the_epoch() {
    let formatter = DateFormatBuilder::new().literal("hello").build();
    assert_eq!(formatter.format(FEB_17_2026), "hello");
    assert_eq!(formatter.format(Y2K), "hello");
}

// --- Individual parts, one style at a time -------------------------------

#[test]
fn year_full_writes_every_digit() {
    assert_eq!(
        DateFormatBuilder::new().year().build().format(FEB_17_2026),
        "2026"
    );
}

#[test]
fn year_short_writes_the_last_two_digits() {
    let formatter = DateFormatBuilder::new()
        .year_styled(YearStyle::Short2)
        .build();
    assert_eq!(formatter.format(FEB_17_2026), "26");
    assert_eq!(formatter.format(Y2K), "00");
}

#[test]
fn year_padded4_pads_a_year_below_a_thousand() {
    // Not in the C++, which has no padded year style — so `iso8601()` there
    // emits `999-…`, which no ISO 8601 parser accepts. For 1000 onward the two
    // styles are identical, which is why every C++ vector still passes.
    let padded = DateFormatBuilder::new()
        .year_styled(YearStyle::Padded4)
        .build();
    let full = DateFormatBuilder::new().year().build();
    let year_999 = at(999, Month::June, 1, 0);
    assert_eq!(padded.format(year_999), "0999");
    assert_eq!(full.format(year_999), "999");
    assert_eq!(padded.format(FEB_17_2026), full.format(FEB_17_2026));
}

#[test]
fn month_renders_in_each_style() {
    let cases = [
        (MonthStyle::Numeric, "2"),
        (MonthStyle::Padded, "02"),
        (MonthStyle::Abbreviated, "Feb"),
        (MonthStyle::FullName, "February"),
    ];
    for (style, expected) in cases {
        let formatter = DateFormatBuilder::new().month_styled(style).build();
        assert_eq!(formatter.format(FEB_17_2026), expected, "{style:?}");
    }
    assert_eq!(
        DateFormatBuilder::new().month().build().format(FEB_17_2026),
        "2"
    );
}

#[test]
fn day_renders_in_each_style() {
    assert_eq!(
        DateFormatBuilder::new().day().build().format(FEB_17_2026),
        "17"
    );
    let third = at(2026, Month::January, 3, 0);
    let padded = DateFormatBuilder::new()
        .day_styled(DayStyle::Padded)
        .build();
    assert_eq!(padded.format(third), "03");
}

#[test]
fn weekday_renders_in_each_style() {
    assert_eq!(
        DateFormatBuilder::new()
            .weekday()
            .build()
            .format(FEB_17_2026),
        "Tuesday"
    );
    let abbreviated = DateFormatBuilder::new()
        .weekday_styled(WeekdayStyle::Abbreviated)
        .build();
    assert_eq!(abbreviated.format(FEB_17_2026), "Tue");
}

#[test]
fn hour_renders_in_each_style() {
    assert_eq!(
        DateFormatBuilder::new().hour().build().format(FEB_17_2026),
        "14"
    );

    let h24_padded = DateFormatBuilder::new()
        .hour_styled(HourStyle::H24Padded)
        .build();
    assert_eq!(h24_padded.format(Y2K), "00");
    assert_eq!(h24_padded.format(FEB_17_2026), "14");

    let h12 = DateFormatBuilder::new().hour_styled(HourStyle::H12).build();
    assert_eq!(h12.format(FEB_17_2026), "2");
    assert_eq!(h12.format(Y2K), "12");

    let h12_padded = DateFormatBuilder::new()
        .hour_styled(HourStyle::H12Padded)
        .build();
    assert_eq!(h12_padded.format(FEB_17_2026), "02");
}

#[test]
fn minute_and_second_render_in_each_style() {
    let minute_padded = DateFormatBuilder::new().minute().build();
    assert_eq!(minute_padded.format(FEB_17_2026), "30");
    assert_eq!(minute_padded.format(Y2K), "00");

    let minute_numeric = DateFormatBuilder::new()
        .minute_styled(MinSecStyle::Numeric)
        .build();
    assert_eq!(minute_numeric.format(Y2K), "0");

    assert_eq!(
        DateFormatBuilder::new()
            .second()
            .build()
            .format(FEB_17_2026),
        "45"
    );
    let second_numeric = DateFormatBuilder::new()
        .second_styled(MinSecStyle::Numeric)
        .build();
    assert_eq!(second_numeric.format(Y2K), "0");
}

#[test]
fn milliseconds_are_always_three_digits() {
    let formatter = DateFormatBuilder::new().millisecond().build();
    assert_eq!(formatter.format(FEB_17_2026), "123");
    assert_eq!(formatter.format(Y2K), "000");
}

#[test]
fn ampm_splits_at_noon() {
    let formatter = DateFormatBuilder::new().ampm().build();
    assert_eq!(formatter.format(FEB_17_2026), "PM");
    assert_eq!(formatter.format(Y2K), "AM");
}

// --- Composites and reuse ------------------------------------------------

#[test]
fn a_composite_date_format_renders_in_order() {
    let formatter = DateFormatBuilder::new()
        .month_styled(MonthStyle::Abbreviated)
        .literal(" ")
        .day()
        .literal(", ")
        .year()
        .build();
    assert_eq!(formatter.format(FEB_17_2026), "Feb 17, 2026");
}

#[test]
fn a_composite_datetime_format_renders_in_order() {
    let formatter = DateFormatBuilder::new()
        .year()
        .literal("/")
        .month_styled(MonthStyle::Padded)
        .literal("/")
        .day_styled(DayStyle::Padded)
        .literal(" ")
        .hour_styled(HourStyle::H24Padded)
        .literal(":")
        .minute()
        .build();
    assert_eq!(formatter.format(FEB_17_2026), "2026/02/17 14:30");
}

#[test]
fn a_composite_weekday_format_renders_in_order() {
    let formatter = DateFormatBuilder::new()
        .weekday()
        .literal(", ")
        .month_styled(MonthStyle::FullName)
        .literal(" ")
        .day()
        .build();
    assert_eq!(formatter.format(FEB_17_2026), "Tuesday, February 17");
}

#[test]
fn one_formatter_serves_many_epochs() {
    let formatter = DateFormatBuilder::new()
        .year()
        .literal("-")
        .month_styled(MonthStyle::Padded)
        .literal("-")
        .day_styled(DayStyle::Padded)
        .build();
    assert_eq!(formatter.format(FEB_17_2026), "2026-02-17");
    assert_eq!(formatter.format(Y2K), "2000-01-01");
    assert_eq!(formatter.format(Epoch::ZERO), "1970-01-01");
}

#[test]
fn building_twice_from_one_builder_yields_independent_formatters() {
    // This is why `build` takes `&self` and snapshots rather than consuming the
    // builder, against the usual preference for a consuming builder: the C++
    // returns `*this` from every part method and copies in a `const build()`,
    // and this behavior is the observable consequence.
    let mut builder = DateFormatBuilder::new();
    builder.year().literal("-").month_styled(MonthStyle::Padded);
    let year_month = builder.build();

    builder.literal("-").day_styled(DayStyle::Padded);
    let full_date = builder.build();

    assert_eq!(year_month.format(FEB_17_2026), "2026-02");
    assert_eq!(full_date.format(FEB_17_2026), "2026-02-17");
}

// --- One-shot helpers ----------------------------------------------------

#[test]
fn format_iso_date_matches_the_iso_date_factory() {
    assert_eq!(format_iso_date(FEB_17_2026), "2026-02-17");
    assert_eq!(format_iso_date(Y2K), "2000-01-01");
    assert_eq!(format_iso_date(Epoch::ZERO), "1970-01-01");
}

#[test]
fn format_iso_timestamp_is_space_separated_and_utc_labelled() {
    assert_eq!(format_iso_timestamp(FEB_17_2026), "2026-02-17 14:30:45 UTC");
    assert_eq!(format_iso_timestamp(Y2K), "2000-01-01 00:00:00 UTC");
}

#[test]
fn format_iso_week_version_zero_pads_the_week() {
    // 2026-03-19 is ISO week 12.
    assert_eq!(
        format_iso_week_version(Epoch::from_millis(1_774_060_800_000)),
        "2026.12.0"
    );
    // 2026-01-01 is a Thursday, so ISO week 1 — padded to two digits.
    assert_eq!(
        format_iso_week_version(Epoch::from_millis(1_767_225_600_000)),
        "2026.01.0"
    );
}

#[test]
fn the_one_shot_helpers_hold_no_state_between_calls() {
    // The C++ builds its formatter fresh per call for exactly this reason; the
    // original `format_time_ago` is the one place that kept a static, and this
    // port's does not.
    let first = format_iso_timestamp(FEB_17_2026);
    let _other = format_iso_timestamp(Y2K);
    assert_eq!(format_iso_timestamp(FEB_17_2026), first);
}
