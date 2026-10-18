//! The UniFFI binding: this crate's entire foreign contract, in one file.
//!
//! Present only under the `uniffi` feature. Every bound crate in the family
//! splits its binding into three layers, and this module is where two of them
//! are spelled:
//!
//! 1. **Values cross verbatim.** [`Epoch`] and [`EpochDelta`] are custom
//!    newtypes over `i64`, [`Month`] and [`Weekday`] are enums, and
//!    [`DateComponents`], [`IsoWeek`], and [`EpochRange`] are records. Those
//!    declarations live at each type's definition, because UniFFI's derives
//!    must; everything else is here.
//! 2. **Closures stay Rust's.** [`EpochSource`] and [`DateFormatter`] keep
//!    their `impl Fn` constructors for Rust callers, untouched.
//! 3. **Foreign implementors get a trait.** [`ForeignEpochSource`] and
//!    [`ForeignDateFormatter`] exist for callers that cannot hand Rust a
//!    closure, and each `from_foreign` collapses one into the *same* stored
//!    closure layer 2 already holds — one representation, never two paths.
//!
//! The functions below are thin wrappers rather than `#[uniffi::export]`
//! attributes scattered across the crate. Two reasons: a function returning an
//! object must return `Arc<T>`, so several would need a wrapper regardless; and
//! one file that is the whole foreign contract can be read and audited as a
//! unit, which a contract spread over six modules cannot.
//!
//! # What deliberately does not cross
//!
//! [`DateFormatBuilder`]'s 17 part methods take `&mut self` and chain, a shape
//! UniFFI has no form for. Its eight canonical formats are exported as free
//! functions instead, so a foreign caller gets the formats without the builder;
//! the six style enums stay Rust-only for the same reason. Arithmetic on
//! [`Epoch`] and [`EpochDelta`] also stays Rust-side — they arrive as integers,
//! and a foreign caller adds to them natively.

use std::sync::Arc;

use crate::{
    DateComponents, DateFormatBuilder, DateFormatter, Epoch, EpochDelta, EpochRange, EpochSource,
    IsoWeek, Month, Weekday,
};

// --- Layer 3: seams a foreign caller implements --------------------------

/// A clock implemented on the foreign side.
///
/// The foreign counterpart of the `impl Fn() -> Epoch` [`EpochSource::new`]
/// takes. Rust callers never see this trait.
#[uniffi::export(with_foreign)]
pub trait ForeignEpochSource: Send + Sync {
    /// Reads the current instant.
    fn now(&self) -> Epoch;
}

/// A formatter implemented on the foreign side.
///
/// The foreign counterpart of the `impl Fn(Epoch) -> String`
/// [`DateFormatter::new`] takes.
#[uniffi::export(with_foreign)]
pub trait ForeignDateFormatter: Send + Sync {
    /// Renders `instant`.
    fn format(&self, instant: Epoch) -> String;
}

#[uniffi::export]
impl EpochSource {
    /// Wraps a foreign clock, collapsing it into the closure this type stores.
    #[uniffi::constructor]
    #[must_use]
    pub fn from_foreign(source: Arc<dyn ForeignEpochSource>) -> Arc<Self> {
        Arc::new(Self::new(move || source.now()))
    }
}

#[uniffi::export]
impl DateFormatter {
    /// Wraps a foreign formatter, collapsing it into the closure this type
    /// stores.
    #[uniffi::constructor]
    #[must_use]
    pub fn from_foreign(formatter: Arc<dyn ForeignDateFormatter>) -> Arc<Self> {
        Arc::new(Self::new(move |instant| formatter.format(instant)))
    }
}

// --- Clocks ---------------------------------------------------------------

/// An [`EpochSource`] backed by the system wall clock.
///
/// See [`crate::system_epoch_source`]; on `wasm32-unknown-unknown` reading it
/// panics, and a wasm consumer injects [`EpochSource::from_foreign`] instead.
#[uniffi::export]
#[must_use]
pub fn system_clock() -> Arc<EpochSource> {
    Arc::new(crate::system_epoch_source())
}

/// An [`EpochSource`] that always reports `value` — the test clock.
#[uniffi::export]
#[must_use]
pub fn fixed_clock(value: Epoch) -> Arc<EpochSource> {
    Arc::new(crate::fixed_epoch_source(value))
}

// --- Calendar -------------------------------------------------------------

/// Builds [`DateComponents`] with `weekday` derived from the date.
///
/// The only correct way to construct one from a foreign caller: `weekday` is
/// derived rather than input, so a record built field-by-field can disagree
/// with its own date. [`crate::components_to_epoch`] ignores the field, but
/// anything that reads it would be reading a lie.
#[uniffi::export]
#[must_use]
pub fn date_components(
    year: i32,
    month: Month,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    millisecond: u16,
) -> DateComponents {
    DateComponents::new(year, month, day, hour, minute, second, millisecond)
}

/// Decomposes an instant into UTC calendar fields.
#[uniffi::export]
#[must_use]
pub fn epoch_to_components(epoch: Epoch) -> DateComponents {
    crate::epoch_to_components(epoch)
}

/// Recomposes calendar fields into an instant. `weekday` is ignored.
#[uniffi::export]
#[must_use]
pub fn components_to_epoch(components: DateComponents) -> Epoch {
    crate::components_to_epoch(&components)
}

/// The ISO 8601 week date for these components.
#[uniffi::export]
#[must_use]
pub fn iso_week_of(components: DateComponents) -> IsoWeek {
    crate::iso_week_of(&components)
}

/// The number of days in `month` of `year`.
#[uniffi::export]
#[must_use]
pub fn days_in_month(year: i32, month: Month) -> u8 {
    crate::days_in_month(year, month)
}

/// Whether `year` is a Gregorian leap year.
#[uniffi::export]
#[must_use]
pub fn is_leap_year(year: i32) -> bool {
    crate::is_leap_year(year)
}

/// Advances `start` by `days` business days, skipping weekends.
#[uniffi::export]
#[must_use]
pub fn add_business_days(start: Epoch, days: i32) -> Epoch {
    crate::add_business_days(start, days)
}

/// The day of the week for `weekday`'s numeric form, or `None` out of range.
#[uniffi::export]
#[must_use]
pub fn weekday_from_number(number: u8) -> Option<Weekday> {
    Weekday::from_number(number)
}

/// The month for its numeric form, or `None` out of range.
#[uniffi::export]
#[must_use]
pub fn month_from_number(number: u8) -> Option<Month> {
    Month::from_number(number)
}

// --- Parsing --------------------------------------------------------------

/// Parses `YYYY-MM-DD` at midnight UTC, or `None` if it is not one.
#[uniffi::export]
#[must_use]
pub fn parse_iso_date(text: &str) -> Option<Epoch> {
    crate::parse_iso_date(text)
}

/// Parses an ISO 8601 UTC timestamp, or `None` if it is not one.
#[uniffi::export]
#[must_use]
pub fn parse_iso_timestamp(text: &str) -> Option<Epoch> {
    crate::parse_iso_timestamp(text)
}

// --- One-shot formatting --------------------------------------------------

/// Renders `YYYY-MM-DD`.
#[uniffi::export]
#[must_use]
pub fn format_iso_date(epoch: Epoch) -> String {
    crate::format_iso_date(epoch)
}

/// Renders the crate's canonical human timestamp.
#[uniffi::export]
#[must_use]
pub fn format_iso_timestamp(epoch: Epoch) -> String {
    crate::format_iso_timestamp(epoch)
}

/// Renders the ISO week-date version stamp.
#[uniffi::export]
#[must_use]
pub fn format_iso_week_version(epoch: Epoch) -> String {
    crate::format_iso_week_version(epoch)
}

/// Renders how long before `now` the `event` was, in words.
#[uniffi::export]
#[must_use]
pub fn format_time_ago(event: Epoch, now: Epoch) -> String {
    crate::format_time_ago(event, now)
}

// --- The eight canonical formats ------------------------------------------
//
// `DateFormatBuilder` itself does not cross (see the module docs), so these
// stand in for it. Each returns the same `DateFormatter` object a Rust caller
// gets, so `format` behaves identically on both sides.

/// `2026-02-17T14:30:45.123Z`
#[uniffi::export]
#[must_use]
pub fn iso8601_formatter() -> Arc<DateFormatter> {
    Arc::new(DateFormatBuilder::iso8601())
}

/// `2026-02-17`
#[uniffi::export]
#[must_use]
pub fn iso_date_formatter() -> Arc<DateFormatter> {
    Arc::new(DateFormatBuilder::iso_date())
}

/// `2026-02-17 14:30`
#[uniffi::export]
#[must_use]
pub fn iso_datetime_minute_formatter() -> Arc<DateFormatter> {
    Arc::new(DateFormatBuilder::iso_datetime_minute())
}

/// `Feb 17, 2026`
#[uniffi::export]
#[must_use]
pub fn us_short_formatter() -> Arc<DateFormatter> {
    Arc::new(DateFormatBuilder::us_short())
}

/// `February 17, 2026`
#[uniffi::export]
#[must_use]
pub fn us_long_formatter() -> Arc<DateFormatter> {
    Arc::new(DateFormatBuilder::us_long())
}

/// `Tuesday, February 17, 2026`
#[uniffi::export]
#[must_use]
pub fn us_long_with_weekday_formatter() -> Arc<DateFormatter> {
    Arc::new(DateFormatBuilder::us_long_with_weekday())
}

/// `Feb 17, 2026 2:30 PM`
#[uniffi::export]
#[must_use]
pub fn us_datetime_formatter() -> Arc<DateFormatter> {
    Arc::new(DateFormatBuilder::us_datetime())
}

/// `Feb 17, 2026 2:30:45 PM`
#[uniffi::export]
#[must_use]
pub fn us_datetime_seconds_formatter() -> Arc<DateFormatter> {
    Arc::new(DateFormatBuilder::us_datetime_seconds())
}

// --- Intervals ------------------------------------------------------------
//
// These exist so the module documentation above is true. `EpochDelta` and
// `EpochRange` were declared as crossing the boundary while appearing in no
// exported signature, which in proc-macro mode means a foreign caller has the
// types and no way to obtain or use one — a contract that reads complete and is
// not. Every operation the Rust side offers on them is here.

/// The signed interval from `earlier` to `later`.
#[uniffi::export]
#[must_use]
pub fn epoch_delta_between(earlier: Epoch, later: Epoch) -> EpochDelta {
    later.delta_since(earlier)
}

/// A half-open range `[start, end)`.
///
/// No ordering is enforced: an inverted range contains nothing and reports a
/// negative duration, which is what the C++ does.
#[uniffi::export]
#[must_use]
pub fn epoch_range(start: Epoch, end: Epoch) -> EpochRange {
    EpochRange::new(start, end)
}

/// Whether `value` falls in `[range.start, range.end)`.
#[uniffi::export]
#[must_use]
// UniFFI lifts a record argument into an owned value; a reference is not this
// signature's to choose.
#[allow(clippy::needless_pass_by_value)]
pub fn epoch_range_contains(range: EpochRange, value: Epoch) -> bool {
    range.contains(value)
}

/// `end - start`, negative for an inverted range.
#[uniffi::export]
#[must_use]
#[allow(clippy::needless_pass_by_value)]
pub fn epoch_range_duration(range: EpochRange) -> EpochDelta {
    range.duration()
}
