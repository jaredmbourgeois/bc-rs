//! Ported from `libs/foundation/date_time/test/business_days_test.cpp`.
//!
//! Every case asserts both the resulting weekday and the exact epoch, as the
//! C++ does — the weekday alone would pass for a result a week off.

use bc_date_time::{
    DateComponents, Epoch, EpochDelta, Month, Weekday, add_business_days, components_to_epoch,
    epoch_to_components,
};

/// Midnight UTC on a calendar date.
fn date(year: i32, month: Month, day: u8) -> Epoch {
    components_to_epoch(&DateComponents::new(year, month, day, 0, 0, 0, 0))
}

fn weekday_of(epoch: Epoch) -> Weekday {
    epoch_to_components(epoch).weekday
}

/// The C++ algorithm, transliterated: step one calendar day at a time and
/// count the ones that are not weekends.
///
/// This is the *reference*, not the implementation. `add_business_days` walks
/// whole weeks at a time so it cannot hang on a large `days`, and an
/// optimisation that changes results is a defect rather than a speed-up — so
/// the two are compared directly over every start weekday below. Keeping the
/// naive version here, in the test, is what makes that comparison meaningful.
fn reference_add_business_days(start: Epoch, days: i32) -> Epoch {
    if days <= 0 {
        return start;
    }
    let mut epoch = start;
    let mut remaining = days;
    while remaining > 0 {
        epoch = epoch + EpochDelta::from_days(1);
        if !epoch_to_components(epoch).weekday.is_weekend() {
            remaining -= 1;
        }
    }
    epoch
}

#[test]
fn the_fast_path_agrees_with_the_naive_walk_on_every_start_weekday() {
    // 2026-05-11 is a Monday, so seven consecutive days cover every weekday —
    // including both weekend starts, which is exactly where a naive
    // "divide by five" shortcut goes wrong (Saturday + 5 business days is the
    // following Friday, not the following Saturday).
    let week_start = date(2026, Month::May, 11);
    assert_eq!(weekday_of(week_start), Weekday::Monday);

    for offset in 0..7 {
        let start = week_start + EpochDelta::from_days(offset);
        for days in 0..=60 {
            assert_eq!(
                add_business_days(start, days),
                reference_add_business_days(start, days),
                "{days} business days from {:?} disagreed with the naive walk",
                weekday_of(start)
            );
        }
    }
}

#[test]
fn the_fast_path_agrees_with_the_naive_walk_at_a_non_midnight_time_of_day() {
    // Time of day must ride along untouched, and a start late in the day is
    // where an off-by-one in the week arithmetic would surface. The claim is
    // that time of day and start weekday do not interact, so all seven are
    // swept rather than one — the same coverage the midnight sibling has.
    let week_start =
        components_to_epoch(&DateComponents::new(2026, Month::May, 11, 23, 59, 59, 999));
    assert_eq!(weekday_of(week_start), Weekday::Monday);

    for offset in 0..7 {
        let start = week_start + EpochDelta::from_days(offset);
        for days in 0..=40 {
            assert_eq!(
                add_business_days(start, days),
                reference_add_business_days(start, days),
                "{days} business days from {:?} at 23:59:59.999 disagreed with the naive walk",
                weekday_of(start)
            );
        }
    }
}

#[test]
fn a_saturated_start_terminates_rather_than_spinning() {
    // The naive walk hangs here: `Epoch + EpochDelta` saturates, so the instant
    // stops advancing, and the saturated instant happens to be a Sunday — so
    // the weekend check never lets the counter decrement. Reaching this
    // assertion at all is the test.
    let saturated = Epoch::from_millis(i64::MAX);
    assert_eq!(weekday_of(saturated), Weekday::Sunday);
    assert_eq!(add_business_days(saturated, 1), saturated);
    assert_eq!(add_business_days(saturated, i32::MAX), saturated);
}

#[test]
fn an_enormous_count_terminates_promptly() {
    // The naive walk is O(days): ~3e9 iterations here, which is a hang in
    // practice even without saturation.
    let start = date(2026, Month::May, 15);
    let far = add_business_days(start, i32::MAX);
    assert!(far > start);
    assert!(!weekday_of(far).is_weekend());
}

// 2026-05-15 is a Friday and 2026-05-18 the following Monday; the assertions
// below check that premise before relying on it.

#[test]
fn friday_plus_one_lands_on_monday() {
    let friday = date(2026, Month::May, 15);
    assert_eq!(weekday_of(friday), Weekday::Friday);
    let result = add_business_days(friday, 1);
    assert_eq!(weekday_of(result), Weekday::Monday);
    assert_eq!(result, date(2026, Month::May, 18));
}

#[test]
fn friday_plus_two_lands_on_tuesday() {
    let result = add_business_days(date(2026, Month::May, 15), 2);
    assert_eq!(weekday_of(result), Weekday::Tuesday);
    assert_eq!(result, date(2026, Month::May, 19));
}

#[test]
fn friday_plus_three_lands_on_wednesday() {
    let result = add_business_days(date(2026, Month::May, 15), 3);
    assert_eq!(weekday_of(result), Weekday::Wednesday);
    assert_eq!(result, date(2026, Month::May, 20));
}

#[test]
fn monday_plus_five_lands_on_the_following_monday() {
    let monday = date(2026, Month::May, 18);
    assert_eq!(weekday_of(monday), Weekday::Monday);
    let result = add_business_days(monday, 5);
    assert_eq!(weekday_of(result), Weekday::Monday);
    assert_eq!(result, date(2026, Month::May, 25));
}

#[test]
fn saturday_plus_one_lands_on_monday() {
    let saturday = date(2026, Month::May, 16);
    assert_eq!(weekday_of(saturday), Weekday::Saturday);
    let result = add_business_days(saturday, 1);
    assert_eq!(weekday_of(result), Weekday::Monday);
    assert_eq!(result, date(2026, Month::May, 18));
}

#[test]
fn zero_days_returns_the_input() {
    let friday = date(2026, Month::May, 15);
    assert_eq!(add_business_days(friday, 0), friday);
}

#[test]
fn a_negative_count_returns_the_input() {
    // Not "goes backwards" — the C++ contract is explicitly a no-op, because
    // the caller's domain ("ships in N business days") has no negative case.
    let friday = date(2026, Month::May, 15);
    assert_eq!(add_business_days(friday, -3), friday);
}

#[test]
fn the_time_of_day_is_preserved() {
    let friday_afternoon =
        components_to_epoch(&DateComponents::new(2026, Month::May, 15, 14, 30, 45, 123));
    let result = add_business_days(friday_afternoon, 1);
    let components = epoch_to_components(result);
    assert_eq!(components.weekday, Weekday::Monday);
    assert_eq!(components.hour, 14);
    assert_eq!(components.minute, 30);
    assert_eq!(components.second, 45);
    assert_eq!(components.millisecond, 123);
}

#[test]
fn the_result_never_lands_on_a_weekend() {
    // Sweeps a full year of start dates against every count up to three weeks,
    // which is the invariant the store-facing caller actually depends on.
    let mut start = date(2026, Month::January, 1);
    for _ in 0..365 {
        for days in 1..=15 {
            let landed = weekday_of(add_business_days(start, days));
            assert!(
                !landed.is_weekend(),
                "{days} business days from {:?} landed on {landed:?}",
                epoch_to_components(start)
            );
        }
        start = start + EpochDelta::from_days(1);
    }
}

#[test]
fn each_added_day_advances_by_at_least_one_calendar_day() {
    // Monotonic and strictly increasing: a count of N is always further out
    // than N-1, which is what makes the loop terminate.
    let start = date(2026, Month::May, 15);
    let mut previous = start;
    for days in 1..=30 {
        let next = add_business_days(start, days);
        assert!(next > previous, "{days} business days did not advance");
        previous = next;
    }
}
