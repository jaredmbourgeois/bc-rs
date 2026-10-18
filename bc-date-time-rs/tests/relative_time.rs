//! Coverage for `format_time_ago`.
//!
//! The C++ original has **no tests at all** for this function — it is the only
//! public entry point in `libs/foundation/date_time` with none, despite being
//! the one two dashboards render. These are new, not ported.

use bc_date_time::{DateFormatBuilder, Epoch, EpochDelta, format_time_ago};

/// 2026-02-17T14:30:45.123Z.
const NOW: Epoch = Epoch::from_millis(1_771_338_645_123);

fn ago(delta: EpochDelta) -> String {
    format_time_ago(NOW - delta, NOW)
}

#[test]
fn the_present_instant_is_just_now() {
    assert_eq!(ago(EpochDelta::ZERO), "just now");
}

#[test]
fn anything_under_a_minute_is_just_now() {
    assert_eq!(ago(EpochDelta::from_millis(1)), "just now");
    assert_eq!(ago(EpochDelta::from_seconds(1)), "just now");
    assert_eq!(ago(EpochDelta::from_seconds(59)), "just now");
}

#[test]
fn a_future_instant_reads_as_just_now_rather_than_a_negative_span() {
    // Two machines with skewed clocks should not render "-3 minutes ago".
    assert_eq!(
        format_time_ago(NOW + EpochDelta::from_hours(1), NOW),
        "just now"
    );
    assert_eq!(
        format_time_ago(NOW + EpochDelta::from_millis(1), NOW),
        "just now"
    );
}

#[test]
fn one_minute_is_singular() {
    assert_eq!(ago(EpochDelta::from_minutes(1)), "1 minute ago");
    // Still one minute at 119 seconds — truncation, not rounding.
    assert_eq!(ago(EpochDelta::from_seconds(119)), "1 minute ago");
}

#[test]
fn minutes_are_plural_up_to_an_hour() {
    assert_eq!(ago(EpochDelta::from_minutes(2)), "2 minutes ago");
    assert_eq!(ago(EpochDelta::from_minutes(59)), "59 minutes ago");
}

#[test]
fn one_hour_is_singular() {
    assert_eq!(ago(EpochDelta::from_minutes(60)), "1 hour ago");
    assert_eq!(ago(EpochDelta::from_minutes(119)), "1 hour ago");
}

#[test]
fn hours_are_plural_up_to_a_day() {
    assert_eq!(ago(EpochDelta::from_hours(2)), "2 hours ago");
    assert_eq!(ago(EpochDelta::from_hours(23)), "23 hours ago");
}

#[test]
fn one_day_is_singular() {
    assert_eq!(ago(EpochDelta::from_hours(24)), "1 day ago");
    assert_eq!(ago(EpochDelta::from_hours(47)), "1 day ago");
}

#[test]
fn days_are_plural_up_to_a_week() {
    assert_eq!(ago(EpochDelta::from_days(2)), "2 days ago");
    assert_eq!(ago(EpochDelta::from_days(6)), "6 days ago");
}

#[test]
fn a_week_or_more_falls_back_to_an_absolute_timestamp() {
    // "37 days ago" is less useful to a reader than the date itself.
    let a_week = EpochDelta::from_days(7);
    let event = NOW - a_week;
    assert_eq!(
        ago(a_week),
        DateFormatBuilder::iso_datetime_minute().format(event)
    );
    assert_eq!(ago(a_week), "2026-02-10 14:30");
    assert_eq!(ago(EpochDelta::from_days(365)), "2025-02-17 14:30");
}

#[test]
fn every_boundary_flips_exactly_one_millisecond_late() {
    // The cascade is a chain of `<` comparisons; an off-by-one in any of them
    // shows up here rather than in whichever tier happens to get exercised.
    let boundaries = [
        (EpochDelta::from_minutes(1), "just now", "1 minute ago"),
        (EpochDelta::from_minutes(2), "1 minute ago", "2 minutes ago"),
        (EpochDelta::from_hours(1), "59 minutes ago", "1 hour ago"),
        (EpochDelta::from_hours(2), "1 hour ago", "2 hours ago"),
        (EpochDelta::from_days(1), "23 hours ago", "1 day ago"),
        (EpochDelta::from_days(2), "1 day ago", "2 days ago"),
    ];
    for (boundary, before, at_boundary) in boundaries {
        assert_eq!(
            ago(boundary - EpochDelta::from_millis(1)),
            before,
            "just before {}ms",
            boundary.millis()
        );
        assert_eq!(ago(boundary), at_boundary, "at {}ms", boundary.millis());
    }
}

#[test]
fn the_fallback_boundary_is_seven_days() {
    assert_eq!(
        ago(EpochDelta::from_days(7) - EpochDelta::from_millis(1)),
        "6 days ago"
    );
    assert!(ago(EpochDelta::from_days(7)).starts_with("2026-"));
}

#[test]
fn the_function_holds_no_state_between_calls() {
    // The C++ caches its fallback formatter in a function-local `static`, which
    // contradicts the module's own documented no-hidden-state stance. This
    // builds fresh, so repeated calls cannot drift.
    let first = ago(EpochDelta::from_days(30));
    let _other = ago(EpochDelta::from_minutes(5));
    assert_eq!(ago(EpochDelta::from_days(30)), first);
}
