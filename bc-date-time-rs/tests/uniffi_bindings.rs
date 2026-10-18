//! The `uniffi` feature's binding surface, as `src/ffi.rs` spells it.
//!
//! Not ported from the C++ suite: the original has no FFI layer. What these
//! tests exist to catch is the one failure this binding can have that a
//! compile cannot: a wrapper in `ffi` drifting from the Rust function it
//! stands in for, and a `from_foreign` growing a
//! *second* stored representation instead of collapsing into the one the type
//! already holds.
#![cfg(feature = "uniffi")]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use bc_date_time::{
    DateFormatBuilder, DateFormatter, Epoch, EpochDelta, EpochSource, Month, Weekday, ffi,
    ffi::{ForeignDateFormatter, ForeignEpochSource},
};

/// A clock a foreign caller would supply: it counts rather than reading a
/// system clock, so a stale or duplicated reading is visible in the result.
struct CountingClock(AtomicI64);

impl ForeignEpochSource for CountingClock {
    fn now(&self) -> Epoch {
        Epoch::from_millis(self.0.fetch_add(1, Ordering::Relaxed))
    }
}

/// A formatter a foreign caller would supply.
struct ShoutingFormatter;

impl ForeignDateFormatter for ShoutingFormatter {
    fn format(&self, instant: Epoch) -> String {
        format!("AT {}", instant.millis())
    }
}

#[test]
fn a_foreign_clock_is_read_through_the_closure_the_type_already_stores() {
    let source = EpochSource::from_foreign(Arc::new(CountingClock(AtomicI64::new(10))));

    // Each read reaches the foreign object exactly once. A `from_foreign` that
    // cached, or that wrapped a second time, would repeat or skip a tick.
    assert_eq!(source.now(), Epoch::from_millis(10));
    assert_eq!(source.now(), Epoch::from_millis(11));
    assert_eq!(source.now(), Epoch::from_millis(12));
}

#[test]
fn a_foreign_source_is_indistinguishable_from_a_rust_closure() {
    // The point of collapsing into one representation: nothing downstream can
    // tell which side built the clock.
    let foreign = EpochSource::from_foreign(Arc::new(CountingClock(AtomicI64::new(7))));
    let native = EpochSource::new(|| Epoch::from_millis(7));

    assert_eq!(foreign.now(), native.now());
}

#[test]
fn a_foreign_formatter_is_read_through_the_stored_closure() {
    let formatter = DateFormatter::from_foreign(Arc::new(ShoutingFormatter));

    assert_eq!(formatter.format(Epoch::from_millis(42)), "AT 42");
}

#[test]
fn date_components_derives_the_weekday_a_foreign_caller_cannot_supply() {
    // A record built field-by-field on the foreign side could carry any
    // weekday; this constructor is the only way to get the right one.
    let components = ffi::date_components(2026, Month::February, 17, 14, 30, 45, 123);

    assert_eq!(components.weekday, Weekday::Tuesday);
    assert_eq!(components.year, 2026);
    assert_eq!(components.millisecond, 123);
}

#[test]
fn every_exported_format_matches_its_builder_counterpart() {
    // The wrappers in `ffi` are the only place the foreign side gets these, so
    // a wrapper pointing at the wrong factory would be invisible to the rest of
    // the suite. Pairing each against the builder is what makes that visible.
    let instant = Epoch::from_millis(1_771_338_645_123);
    let pairs: [(Arc<DateFormatter>, DateFormatter); 8] = [
        (ffi::iso8601_formatter(), DateFormatBuilder::iso8601()),
        (ffi::iso_date_formatter(), DateFormatBuilder::iso_date()),
        (
            ffi::iso_datetime_minute_formatter(),
            DateFormatBuilder::iso_datetime_minute(),
        ),
        (ffi::us_short_formatter(), DateFormatBuilder::us_short()),
        (ffi::us_long_formatter(), DateFormatBuilder::us_long()),
        (
            ffi::us_long_with_weekday_formatter(),
            DateFormatBuilder::us_long_with_weekday(),
        ),
        (
            ffi::us_datetime_formatter(),
            DateFormatBuilder::us_datetime(),
        ),
        (
            ffi::us_datetime_seconds_formatter(),
            DateFormatBuilder::us_datetime_seconds(),
        ),
    ];

    for (exported, native) in pairs {
        assert_eq!(exported.format(instant), native.format(instant));
    }

    // And the set is distinct, so the loop above cannot pass by every wrapper
    // pointing at the same factory.
    let rendered: Vec<String> = [
        ffi::iso8601_formatter(),
        ffi::iso_date_formatter(),
        ffi::iso_datetime_minute_formatter(),
        ffi::us_short_formatter(),
        ffi::us_long_formatter(),
        ffi::us_long_with_weekday_formatter(),
        ffi::us_datetime_formatter(),
        ffi::us_datetime_seconds_formatter(),
    ]
    .iter()
    .map(|formatter| formatter.format(instant))
    .collect();
    let mut distinct = rendered.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        rendered.len(),
        "two wrappers render identically"
    );
}

#[test]
fn the_exported_wrappers_agree_with_the_functions_they_stand_in_for() {
    let instant = Epoch::from_millis(1_771_338_645_123);
    let components = bc_date_time::epoch_to_components(instant);

    assert_eq!(ffi::epoch_to_components(instant), components);
    assert_eq!(
        ffi::components_to_epoch(components),
        bc_date_time::components_to_epoch(&components)
    );
    assert_eq!(
        ffi::iso_week_of(components),
        bc_date_time::iso_week_of(&components)
    );
    assert_eq!(ffi::days_in_month(2024, Month::February), 29);
    assert!(ffi::is_leap_year(2024));
    assert_eq!(ffi::month_from_number(2), Some(Month::February));
    assert_eq!(ffi::month_from_number(13), None);
    assert_eq!(ffi::weekday_from_number(3), Some(Weekday::Tuesday));
    assert_eq!(
        ffi::parse_iso_timestamp("2026-02-17T14:30:45.123Z"),
        Some(instant)
    );
    assert_eq!(ffi::parse_iso_timestamp("not a timestamp"), None);
    assert_eq!(
        ffi::format_iso_date(instant),
        bc_date_time::format_iso_date(instant)
    );
    assert_eq!(
        ffi::format_time_ago(instant, instant + EpochDelta::from_hours(2)),
        bc_date_time::format_time_ago(instant, instant + EpochDelta::from_hours(2))
    );
}

#[test]
fn the_exported_clocks_behave_as_their_free_function_counterparts() {
    let fixed = ffi::fixed_clock(Epoch::from_millis(99));
    assert_eq!(fixed.now(), Epoch::from_millis(99));

    // The system clock is only asserted to be readable and monotone-ish here;
    // its value is the host's, which no test may pin.
    let system = ffi::system_clock();
    assert!(system.now().millis() > 0);
}
