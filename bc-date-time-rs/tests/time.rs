//! Ported from `libs/foundation/date_time/test/time_test.cpp`.
//!
//! Fixture epochs are the C++ suite's, unchanged, so a disagreement between the
//! two implementations shows up as a failing assertion rather than as a
//! difference nobody looks at.

use bc_date_time::{
    DateComponents, Epoch, EpochDelta, EpochRange, EpochSource, Month, Weekday,
    components_to_epoch, days_in_month, epoch_to_components, fixed_epoch_source, is_leap_year,
    iso_week_of, system_epoch_source,
};

/// 2026-02-17T14:30:45.123Z, a Tuesday.
const FEB_17_2026: Epoch = Epoch::from_millis(1_771_338_645_123);
/// 2000-01-01T00:00:00.000Z, a Saturday.
const Y2K: Epoch = Epoch::from_millis(946_684_800_000);
/// 2024-02-29T12:00:00.000Z, a Thursday — a leap day.
const LEAP_DAY_2024: Epoch = Epoch::from_millis(1_709_208_000_000);

// --- Epoch ---------------------------------------------------------------

#[test]
fn epoch_is_a_transparent_i64() {
    // The C++ asserts `std::is_same_v<Epoch, int64_t>`. Rust's equivalent claim
    // is about layout, not identity: `#[repr(transparent)]` is what keeps the
    // newtype ABI-compatible with `int64_t` for a future C ABI.
    assert_eq!(size_of::<Epoch>(), size_of::<i64>());
    assert_eq!(align_of::<Epoch>(), align_of::<i64>());
    assert_eq!(size_of::<EpochDelta>(), size_of::<i64>());
}

#[test]
fn zero_is_the_unix_epoch() {
    assert_eq!(Epoch::ZERO.millis(), 0);
    assert_eq!(Epoch::ZERO, Epoch::from_millis(0));
}

#[test]
fn millis_round_trips_the_wrapped_value() {
    for millis in [i64::MIN, -1_000, -1, 0, 1, 1_000, i64::MAX] {
        assert_eq!(Epoch::from_millis(millis).millis(), millis);
    }
}

// --- EpochDelta ----------------------------------------------------------

#[test]
fn delta_constructors_agree_on_milliseconds() {
    assert_eq!(EpochDelta::from_seconds(1), EpochDelta::from_millis(1_000));
    assert_eq!(EpochDelta::from_minutes(1), EpochDelta::from_seconds(60));
    assert_eq!(EpochDelta::from_hours(1), EpochDelta::from_minutes(60));
    assert_eq!(EpochDelta::from_days(1), EpochDelta::from_hours(24));
    assert_eq!(EpochDelta::ZERO, EpochDelta::from_millis(0));
}

#[test]
fn delta_arithmetic_moves_an_epoch_both_ways() {
    let start = Epoch::from_millis(1_000);
    assert_eq!(
        start + EpochDelta::from_seconds(1),
        Epoch::from_millis(2_000)
    );
    assert_eq!(start - EpochDelta::from_seconds(1), Epoch::ZERO);
    assert_eq!(
        Epoch::from_millis(2_000) - start,
        EpochDelta::from_seconds(1)
    );
    // A negative span is representable, which is what makes clock-regression
    // checks expressible rather than saturating at zero.
    assert_eq!(
        start - Epoch::from_millis(2_000),
        -EpochDelta::from_seconds(1)
    );
}

#[test]
fn delta_arithmetic_saturates_rather_than_wrapping() {
    // Debug and release must agree — the suite runs under both profiles, and a
    // plain `+` would panic in one and wrap in the other.
    assert_eq!(
        Epoch::from_millis(i64::MAX) + EpochDelta::from_days(1),
        Epoch::from_millis(i64::MAX)
    );
    assert_eq!(
        Epoch::from_millis(i64::MIN) - EpochDelta::from_days(1),
        Epoch::from_millis(i64::MIN)
    );
    assert_eq!(EpochDelta::from_days(i64::MAX).millis(), i64::MAX);
}

#[test]
fn whole_unit_accessors_truncate_toward_the_past() {
    assert_eq!(EpochDelta::from_millis(1_999).whole_seconds(), 1);
    assert_eq!(EpochDelta::from_millis(-1_999).whole_seconds(), -2);
    assert_eq!(EpochDelta::from_hours(47).whole_days(), 1);
}

#[test]
fn truncating_to_seconds_rounds_toward_the_past_on_both_sides_of_the_epoch() {
    assert_eq!(
        Epoch::from_millis(1_500).truncated_to_seconds(),
        Epoch::from_millis(1_000)
    );
    // The case a naive `/ 1000` gets wrong: truncating toward zero would move a
    // pre-1970 instant *forward* in time.
    assert_eq!(
        Epoch::from_millis(-1_500).truncated_to_seconds(),
        Epoch::from_millis(-2_000)
    );
    assert_eq!(Epoch::ZERO.truncated_to_seconds(), Epoch::ZERO);
}

// --- EpochSource ---------------------------------------------------------

#[test]
fn the_system_source_returns_a_plausible_instant() {
    // The C++ bounds this both ways, with an upper bound of 2030-01-01 — so the
    // suite hard-fails on that date. Only the lower bound is a real assertion
    // about correctness; an upper bound is an expiry date, not a test.
    let now = system_epoch_source().now();
    let year_2025 = Epoch::from_millis(1_735_689_600_000);
    assert!(
        now > year_2025,
        "system clock reports {}, before 2025-01-01",
        now.millis()
    );
}

#[test]
fn the_system_source_is_monotonically_non_decreasing() {
    // No sleep: two reads in sequence already establish the ordering, and a
    // sleep would only make the test slower and flakier.
    let source = system_epoch_source();
    let first = source.now();
    let second = source.now();
    assert!(second >= first);
}

#[test]
fn the_fixed_source_returns_its_value_every_time() {
    let source = fixed_epoch_source(Epoch::from_millis(42));
    assert_eq!(source.now(), Epoch::from_millis(42));
    assert_eq!(source.now(), Epoch::from_millis(42));
    assert_eq!(source.now(), Epoch::from_millis(42));
}

#[test]
fn a_source_can_be_stored_and_cloned() {
    // The C++ pins that `EpochSource` is copyable; the shared `Arc` inside is
    // what preserves that (a `Box<dyn Fn>` would not be).
    let source = fixed_epoch_source(Epoch::from_millis(100));
    let copy: EpochSource = source.clone();
    assert_eq!(copy.now(), Epoch::from_millis(100));
    assert_eq!(source.now(), Epoch::from_millis(100));
}

#[test]
fn any_closure_is_a_clock() {
    // A stored source is callable directly — the reason `EpochSource` is a
    // named type rather than an `Arc<dyn Fn…>` alias, which could be held but
    // not called without re-wrapping it.
    fn read(clock: &EpochSource) -> Epoch {
        clock.now()
    }
    let literal = EpochSource::new(|| Epoch::from_millis(7));
    assert_eq!(read(&literal), Epoch::from_millis(7));
    assert_eq!(read(&fixed_epoch_source(Y2K)), Y2K);
    // And a bare closure converts without naming the constructor.
    let converted: EpochSource = (|| LEAP_DAY_2024).into();
    assert_eq!(converted.now(), LEAP_DAY_2024);
}

// --- EpochRange ----------------------------------------------------------

fn range(start: i64, end: i64) -> EpochRange {
    EpochRange::new(Epoch::from_millis(start), Epoch::from_millis(end))
}

#[test]
fn range_start_is_inclusive() {
    assert!(range(100, 200).contains(Epoch::from_millis(100)));
}

#[test]
fn range_end_is_exclusive() {
    assert!(!range(100, 200).contains(Epoch::from_millis(200)));
}

#[test]
fn range_contains_the_middle() {
    assert!(range(100, 200).contains(Epoch::from_millis(150)));
}

#[test]
fn range_excludes_values_outside_it() {
    let interval = range(100, 200);
    assert!(!interval.contains(Epoch::from_millis(99)));
    assert!(!interval.contains(Epoch::from_millis(201)));
}

#[test]
fn range_duration_is_end_minus_start() {
    assert_eq!(range(100, 350).duration(), EpochDelta::from_millis(250));
}

#[test]
fn an_inverted_range_contains_nothing_and_reports_a_negative_duration() {
    let inverted = range(350, 100);
    assert!(!inverted.contains(Epoch::from_millis(200)));
    assert_eq!(inverted.duration(), EpochDelta::from_millis(-250));
}

#[test]
fn ranges_compare_by_value() {
    assert_eq!(range(100, 200), range(100, 200));
    assert_ne!(range(100, 200), range(100, 300));
}

// --- zero_indexed --------------------------------------------------------

#[test]
fn weekday_zero_indexing_spans_sunday_to_saturday() {
    assert_eq!(Weekday::Sunday.zero_indexed(), 0);
    assert_eq!(Weekday::Saturday.zero_indexed(), 6);
}

#[test]
fn month_zero_indexing_spans_january_to_december() {
    assert_eq!(Month::January.zero_indexed(), 0);
    assert_eq!(Month::December.zero_indexed(), 11);
}

#[test]
fn zero_indexing_is_available_in_const_context() {
    // The C++ proves this with `static_assert`; a const item is the equivalent,
    // and it fails at compile time the same way.
    const MONDAY: u8 = Weekday::Monday.zero_indexed();
    const MARCH: u8 = Month::March.zero_indexed();
    assert_eq!(MONDAY, 1);
    assert_eq!(MARCH, 2);
}

#[test]
fn numbers_and_enums_convert_both_ways() {
    for (index, weekday) in Weekday::ALL.iter().enumerate() {
        assert_eq!(usize::from(weekday.zero_indexed()), index);
        assert_eq!(Weekday::from_number(weekday.number()), Some(*weekday));
    }
    for (index, month) in Month::ALL.iter().enumerate() {
        assert_eq!(usize::from(month.zero_indexed()), index);
        assert_eq!(Month::from_number(month.number()), Some(*month));
    }
    assert_eq!(Weekday::from_number(0), None);
    assert_eq!(Weekday::from_number(8), None);
    assert_eq!(Month::from_number(0), None);
    assert_eq!(Month::from_number(13), None);
}

// --- Name lookups --------------------------------------------------------

#[test]
fn every_weekday_name_and_abbreviation_is_correct() {
    let expected = [
        (Weekday::Sunday, "Sunday", "Sun"),
        (Weekday::Monday, "Monday", "Mon"),
        (Weekday::Tuesday, "Tuesday", "Tue"),
        (Weekday::Wednesday, "Wednesday", "Wed"),
        (Weekday::Thursday, "Thursday", "Thu"),
        (Weekday::Friday, "Friday", "Fri"),
        (Weekday::Saturday, "Saturday", "Sat"),
    ];
    for (weekday, name, abbr) in expected {
        assert_eq!(weekday.name(), name);
        assert_eq!(weekday.abbr(), abbr);
    }
}

#[test]
fn every_month_name_and_abbreviation_is_correct() {
    let expected = [
        (Month::January, "January", "Jan"),
        (Month::February, "February", "Feb"),
        (Month::March, "March", "Mar"),
        (Month::April, "April", "Apr"),
        (Month::May, "May", "May"),
        (Month::June, "June", "Jun"),
        (Month::July, "July", "Jul"),
        (Month::August, "August", "Aug"),
        (Month::September, "September", "Sep"),
        (Month::October, "October", "Oct"),
        (Month::November, "November", "Nov"),
        (Month::December, "December", "Dec"),
    ];
    for (month, name, abbr) in expected {
        assert_eq!(month.name(), name);
        assert_eq!(month.abbr(), abbr);
    }
}

#[test]
fn only_saturday_and_sunday_are_the_weekend() {
    for weekday in Weekday::ALL {
        let expected = matches!(weekday, Weekday::Saturday | Weekday::Sunday);
        assert_eq!(weekday.is_weekend(), expected, "{weekday:?}");
    }
}

// --- epoch_to_components -------------------------------------------------

#[test]
fn the_unix_epoch_decomposes_to_a_thursday_midnight() {
    let components = epoch_to_components(Epoch::ZERO);
    assert_eq!(components.year, 1970);
    assert_eq!(components.month, Month::January);
    assert_eq!(components.day, 1);
    assert_eq!(components.hour, 0);
    assert_eq!(components.minute, 0);
    assert_eq!(components.second, 0);
    assert_eq!(components.millisecond, 0);
    assert_eq!(components.weekday, Weekday::Thursday);
}

#[test]
fn y2k_decomposes_to_a_saturday_midnight() {
    let components = epoch_to_components(Y2K);
    assert_eq!(components.year, 2000);
    assert_eq!(components.month, Month::January);
    assert_eq!(components.day, 1);
    assert_eq!(components.hour, 0);
    assert_eq!(components.minute, 0);
    assert_eq!(components.second, 0);
    assert_eq!(components.millisecond, 0);
    assert_eq!(components.weekday, Weekday::Saturday);
}

#[test]
fn a_mid_afternoon_instant_decomposes_field_by_field() {
    let components = epoch_to_components(FEB_17_2026);
    assert_eq!(components.year, 2026);
    assert_eq!(components.month, Month::February);
    assert_eq!(components.day, 17);
    assert_eq!(components.hour, 14);
    assert_eq!(components.minute, 30);
    assert_eq!(components.second, 45);
    assert_eq!(components.millisecond, 123);
    assert_eq!(components.weekday, Weekday::Tuesday);
}

#[test]
fn a_leap_day_decomposes_to_february_twenty_ninth() {
    let components = epoch_to_components(LEAP_DAY_2024);
    assert_eq!(components.year, 2024);
    assert_eq!(components.month, Month::February);
    assert_eq!(components.day, 29);
    assert_eq!(components.hour, 12);
    assert_eq!(components.minute, 0);
    assert_eq!(components.second, 0);
    assert_eq!(components.millisecond, 0);
    assert_eq!(components.weekday, Weekday::Thursday);
}

#[test]
fn sub_second_milliseconds_survive_decomposition() {
    let components = epoch_to_components(Y2K + EpochDelta::from_millis(456));
    assert_eq!(components.millisecond, 456);
    assert_eq!(components.second, 0);
}

#[test]
fn pre_epoch_instants_decompose_without_a_seam() {
    // Not covered by the C++ suite, which never tests a negative epoch — and
    // the case where truncation-toward-zero silently produces a wrong date.
    let one_second_before = epoch_to_components(Epoch::from_millis(-1_000));
    assert_eq!(one_second_before.year, 1969);
    assert_eq!(one_second_before.month, Month::December);
    assert_eq!(one_second_before.day, 31);
    assert_eq!(one_second_before.hour, 23);
    assert_eq!(one_second_before.minute, 59);
    assert_eq!(one_second_before.second, 59);
    assert_eq!(one_second_before.millisecond, 0);
    assert_eq!(one_second_before.weekday, Weekday::Wednesday);

    // One millisecond before the epoch is still 1969, with 999ms.
    let one_milli_before = epoch_to_components(Epoch::from_millis(-1));
    assert_eq!(one_milli_before.year, 1969);
    assert_eq!(one_milli_before.second, 59);
    assert_eq!(one_milli_before.millisecond, 999);
}

#[test]
fn an_absurd_year_saturates_rather_than_panicking_or_wrapping() {
    // `DateComponents.year` is an `i32` the caller sets freely, and a year near
    // its limit is roughly seven times past what `i64` milliseconds hold.
    // Plain arithmetic would panic here in debug and wrap to a plausible wrong
    // instant in release — the suite runs both profiles precisely to catch that
    // kind of divergence.
    let far_future = DateComponents::new(i32::MAX, Month::December, 31, 23, 59, 59, 999);
    assert_eq!(
        components_to_epoch(&far_future),
        Epoch::from_millis(i64::MAX)
    );

    let far_past = DateComponents::new(i32::MIN, Month::January, 1, 0, 0, 0, 0);
    assert_eq!(components_to_epoch(&far_past), Epoch::from_millis(i64::MIN));

    // The clamp is monotonic, not a wrap: a later year never reads as earlier.
    let merely_distant = DateComponents::new(1_000_000, Month::January, 1, 0, 0, 0, 0);
    assert!(components_to_epoch(&merely_distant) < components_to_epoch(&far_future));
    assert!(components_to_epoch(&far_past) < components_to_epoch(&merely_distant));
}

#[test]
fn the_extremes_of_the_epoch_range_decompose_to_sane_years() {
    // `i64` milliseconds reach about ±292 million years, which fits `i32`
    // sevenfold — this pins that the narrowing can never truncate.
    let latest = epoch_to_components(Epoch::from_millis(i64::MAX));
    let earliest = epoch_to_components(Epoch::from_millis(i64::MIN));
    assert!(latest.year > 292_000_000, "{}", latest.year);
    assert!(earliest.year < -292_000_000, "{}", earliest.year);
    assert!((1..=12).contains(&latest.month.number()));
    assert!((1..=31).contains(&earliest.day));
}

// --- components_to_epoch -------------------------------------------------

#[test]
fn the_unix_epoch_composes_from_its_components() {
    let components = DateComponents::new(1970, Month::January, 1, 0, 0, 0, 0);
    assert_eq!(components.weekday, Weekday::Thursday);
    assert_eq!(components_to_epoch(&components), Epoch::ZERO);
}

#[test]
fn a_mid_afternoon_instant_composes_from_its_components() {
    let components = DateComponents::new(2026, Month::February, 17, 14, 30, 45, 123);
    assert_eq!(components.weekday, Weekday::Tuesday);
    assert_eq!(components_to_epoch(&components), FEB_17_2026);
}

#[test]
fn out_of_range_fields_normalize_the_way_chrono_does() {
    // Second 60 is what a leap-second timestamp parses to; it becomes second 0
    // of the next minute rather than being rejected here. The C++ relies on the
    // same `<chrono>` normalization.
    let leap_second = DateComponents::new(2026, Month::February, 17, 14, 30, 60, 0);
    let next_minute = DateComponents::new(2026, Month::February, 17, 14, 31, 0, 0);
    assert_eq!(
        components_to_epoch(&leap_second),
        components_to_epoch(&next_minute)
    );
}

#[test]
fn the_weekday_field_is_ignored_when_composing() {
    // The date determines the weekday, so a wrong one changes nothing. This
    // pins the C++ behavior rather than the C++ *documentation*, which does not
    // mention it.
    let correct = DateComponents::new(2026, Month::February, 17, 0, 0, 0, 0);
    let mut lying = correct;
    lying.weekday = Weekday::Friday;
    assert_eq!(components_to_epoch(&lying), components_to_epoch(&correct));
}

// --- Round-trips ---------------------------------------------------------

#[test]
fn every_fixture_epoch_round_trips_through_components() {
    for epoch in [Epoch::ZERO, Y2K, FEB_17_2026, LEAP_DAY_2024] {
        assert_eq!(
            components_to_epoch(&epoch_to_components(epoch)),
            epoch,
            "{} failed to round-trip",
            epoch.millis()
        );
    }
}

#[test]
fn components_round_trip_through_an_epoch() {
    let components = DateComponents::new(2026, Month::February, 17, 14, 30, 45, 123);
    assert_eq!(
        epoch_to_components(components_to_epoch(&components)),
        components
    );
}

#[test]
fn components_compare_by_value() {
    assert_eq!(
        epoch_to_components(FEB_17_2026),
        epoch_to_components(FEB_17_2026)
    );
    assert_ne!(epoch_to_components(FEB_17_2026), epoch_to_components(Y2K));
}

// --- Calendar utilities --------------------------------------------------

#[test]
fn month_lengths_follow_the_gregorian_rules() {
    // 2026 is not a leap year; 2024 and 2000 are; 1900 is not.
    assert_eq!(days_in_month(2026, Month::February), 28);
    assert_eq!(days_in_month(2024, Month::February), 29);
    assert_eq!(days_in_month(2000, Month::February), 29);
    assert_eq!(days_in_month(1900, Month::February), 28);

    for month in [Month::April, Month::June, Month::September, Month::November] {
        assert_eq!(days_in_month(2026, month), 30, "{month:?}");
    }
    for month in [
        Month::January,
        Month::March,
        Month::May,
        Month::July,
        Month::August,
        Month::October,
        Month::December,
    ] {
        assert_eq!(days_in_month(2026, month), 31, "{month:?}");
    }
}

#[test]
fn month_lengths_agree_with_the_calendar_itself() {
    // Independence has to be earned, and measuring the span from the 1st to
    // the Nth does not earn it: `components_to_epoch` is linear in the day
    // field, so "the Nth is N-1 days after the 1st" holds for whatever number
    // the table returns — February 30 included. That shape passes against a
    // deliberately broken table, which makes it an identity rather than a
    // check.
    //
    // The honest counterparty is decomposition. `epoch_to_components` runs the
    // inverse civil-days algorithm and knows nothing about the table, so
    // asking it where the day after the last one lands is a real question: a
    // month one day too long answers with the 2nd, one day too short with the
    // last day over again.
    for year in [1900, 1999, 2000, 2024, 2026] {
        for month in Month::ALL {
            let last = components_to_epoch(&DateComponents::new(
                year,
                month,
                days_in_month(year, month),
                0,
                0,
                0,
                0,
            ));
            let next = epoch_to_components(last + EpochDelta::from_days(1));
            let expected = if month == Month::December {
                (year + 1, 1)
            } else {
                (year, month.number() + 1)
            };
            assert_eq!(
                (next.year, next.month.number(), next.day),
                (expected.0, expected.1, 1),
                "{year} {month:?} does not end where days_in_month says it does",
            );
        }
    }
}

#[test]
fn leap_years_follow_the_four_hundred_year_cycle() {
    assert!(is_leap_year(2024));
    assert!(!is_leap_year(2026));
    assert!(!is_leap_year(1900));
    assert!(is_leap_year(2000));
}

// --- ISO week ------------------------------------------------------------

fn iso_week_at(millis: i64) -> (i32, u8) {
    let week = iso_week_of(&epoch_to_components(Epoch::from_millis(millis)));
    (week.year, week.week)
}

#[test]
fn a_monday_january_first_is_week_one() {
    // 2007-01-01 is a Monday.
    assert_eq!(iso_week_at(1_167_609_600_000), (2007, 1));
}

#[test]
fn a_thursday_january_first_is_week_one() {
    // 2026-01-01 is a Thursday.
    assert_eq!(iso_week_at(1_767_225_600_000), (2026, 1));
}

#[test]
fn early_january_can_belong_to_the_prior_iso_year() {
    // 2005-01-01 is a Saturday, so it falls in week 53 of 2004.
    assert_eq!(iso_week_at(1_104_537_600_000), (2004, 53));
}

#[test]
fn late_december_of_a_fifty_three_week_year_is_week_fifty_three() {
    // 2026-12-31 is a Thursday.
    assert_eq!(iso_week_at(1_798_675_200_000), (2026, 53));
}

#[test]
fn a_mid_year_date_lands_on_its_ordinary_week() {
    // 2026-03-19.
    assert_eq!(iso_week_at(1_774_060_800_000), (2026, 12));
}

#[test]
fn the_iso_week_ignores_a_wrong_weekday_field() {
    // The C++ reads `components.weekday` and trusts it, so a hand-built struct
    // with the wrong weekday yields the wrong week. This derives it instead.
    let mut components = epoch_to_components(Epoch::from_millis(1_104_537_600_000));
    components.weekday = Weekday::Monday;
    assert_eq!(
        iso_week_of(&components),
        iso_week_of(&epoch_to_components(Epoch::from_millis(1_104_537_600_000)))
    );
}

#[test]
fn iso_weeks_stay_in_range_across_a_century() {
    // Every week number is 1..=53, and the ISO year never strays more than one
    // from the calendar year.
    let mut epoch = Epoch::from_millis(0);
    let day = EpochDelta::from_days(1);
    for _ in 0..36_525 {
        let components = epoch_to_components(epoch);
        let week = iso_week_of(&components);
        assert!(
            (1..=53).contains(&week.week),
            "week {} on {:?}",
            week.week,
            components
        );
        assert!(
            (week.year - components.year).abs() <= 1,
            "iso year {} vs calendar year {}",
            week.year,
            components.year
        );
        epoch = epoch + day;
    }
}
