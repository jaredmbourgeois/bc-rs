//! The canonical time type, the injectable clock seam, and Gregorian calendar
//! conversions.
//!
//! Ported from `libs/foundation/date_time/src/time.{hpp,cpp}`. Everything here
//! is UTC; there is no timezone database and no local-time concept, by design.

use std::ops::{Add, Neg, Sub};
use std::sync::Arc;

use crate::civil::{civil_from_days, days_from_civil, weekday_index_from_days};

const MILLIS_PER_SECOND: i64 = 1_000;
const MILLIS_PER_MINUTE: i64 = 60 * MILLIS_PER_SECOND;
const MILLIS_PER_HOUR: i64 = 60 * MILLIS_PER_MINUTE;
const MILLIS_PER_DAY: i64 = 24 * MILLIS_PER_HOUR;

// --- Epoch ---------------------------------------------------------------

/// A point in time: milliseconds since 1970-01-01T00:00:00Z, UTC.
///
/// Integer milliseconds avoid every floating-point comparison hazard, and `i64`
/// reaches roughly ±292 million years — far past any date this will represent.
/// The unit matches JavaScript's `Date.now()` and most database timestamp
/// columns, and stays readable in a log.
///
/// `#[repr(transparent)]` makes this ABI-identical to `int64_t`, so a future C
/// ABI passes it through unchanged; the newtype exists to keep milliseconds
/// from being confused with seconds or with a *span* of time
/// ([`EpochDelta`]) — a distinction the C++ original, where `Epoch` is a bare
/// `int64_t` alias, could not make.
///
/// ```
/// use bc_date_time::{Epoch, EpochDelta};
///
/// let launch = Epoch::from_millis(1_771_338_645_123);
/// let later = launch + EpochDelta::from_hours(3);
/// assert_eq!(later.delta_since(launch), EpochDelta::from_hours(3));
/// ```
///
/// Deliberately **not** `Default`. A defaulted instant would be 1970-01-01,
/// and a `..Default::default()` in some future struct literal would put that
/// on the wire as if it were a real observation. [`Epoch::ZERO`] says the same
/// thing where it is actually meant. ([`EpochDelta`] *is* `Default`, because a
/// zero-length span is a meaningful value rather than a stand-in for one.)
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Epoch(i64);

// Crosses a foreign boundary verbatim, as a plain `i64` (the binding's value
// layer, described in `ffi.rs`), the same thing `#[repr(transparent)]` already
// makes it for a future C ABI. The arithmetic stays Rust-side: a foreign caller
// holds an integer and adds to it natively, so there is nothing here for a
// wrapper to add.
#[cfg(feature = "uniffi")]
uniffi::custom_newtype!(Epoch, i64);

impl Epoch {
    /// The Unix epoch itself, 1970-01-01T00:00:00.000Z.
    ///
    /// Replaces the C++ `EPOCH_ZERO` constant.
    pub const ZERO: Self = Self(0);

    /// Wraps a millisecond count.
    #[must_use]
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// The wrapped millisecond count.
    #[must_use]
    pub const fn millis(self) -> i64 {
        self.0
    }

    /// The signed span from `earlier` to `self`.
    ///
    /// Negative when `self` precedes `earlier`, which is what makes this usable
    /// for clock-regression checks rather than only for forward elapsed time.
    #[must_use]
    pub const fn delta_since(self, earlier: Self) -> EpochDelta {
        EpochDelta(self.0.saturating_sub(earlier.0))
    }

    /// Truncates to whole seconds, toward the past.
    ///
    /// `div_euclid`, not `/`: for a pre-1970 instant the two disagree, and
    /// truncating toward zero would round a negative epoch *forward* in time.
    /// Consumers that persist second-precision timestamps render through this.
    #[must_use]
    pub const fn truncated_to_seconds(self) -> Self {
        Self(self.0.div_euclid(MILLIS_PER_SECOND) * MILLIS_PER_SECOND)
    }
}

impl Add<EpochDelta> for Epoch {
    type Output = Self;

    fn add(self, delta: EpochDelta) -> Self {
        Self(self.0.saturating_add(delta.0))
    }
}

impl Sub<EpochDelta> for Epoch {
    type Output = Self;

    fn sub(self, delta: EpochDelta) -> Self {
        Self(self.0.saturating_sub(delta.0))
    }
}

impl Sub for Epoch {
    type Output = EpochDelta;

    fn sub(self, earlier: Self) -> EpochDelta {
        self.delta_since(earlier)
    }
}

// --- EpochDelta ----------------------------------------------------------

/// A signed span of time, in milliseconds.
///
/// The C++ original has no duration type — `EpochRange::duration()` returns a
/// bare `Epoch`, so a span and a point in time share one type. Separating them
/// is what lets the compiler reject `epoch_a + epoch_b`, and it gives consumers
/// an exact unit to state retention windows in.
///
/// Arithmetic saturates rather than wrapping or panicking, so a debug build and
/// a release build agree — which matters because the property tests run under
/// both profiles.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct EpochDelta(i64);

#[cfg(feature = "uniffi")]
uniffi::custom_newtype!(EpochDelta, i64);

impl EpochDelta {
    /// A zero-length span.
    pub const ZERO: Self = Self(0);

    /// A span of `millis` milliseconds.
    #[must_use]
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// A span of `seconds` seconds.
    #[must_use]
    pub const fn from_seconds(seconds: i64) -> Self {
        Self(seconds.saturating_mul(MILLIS_PER_SECOND))
    }

    /// A span of `minutes` minutes.
    #[must_use]
    pub const fn from_minutes(minutes: i64) -> Self {
        Self(minutes.saturating_mul(MILLIS_PER_MINUTE))
    }

    /// A span of `hours` hours.
    #[must_use]
    pub const fn from_hours(hours: i64) -> Self {
        Self(hours.saturating_mul(MILLIS_PER_HOUR))
    }

    /// A span of `days` 24-hour days.
    ///
    /// Calendar days, not civil days: this crate has no timezone, so every day
    /// is exactly 24 hours and no DST transition can shorten one.
    #[must_use]
    pub const fn from_days(days: i64) -> Self {
        Self(days.saturating_mul(MILLIS_PER_DAY))
    }

    /// The span in milliseconds.
    #[must_use]
    pub const fn millis(self) -> i64 {
        self.0
    }

    /// The span in whole seconds, truncated toward negative infinity.
    ///
    /// `div_euclid` again, for the same reason [`Epoch::truncated_to_seconds`]
    /// uses it: it is the only truncation with no flat spot straddling zero.
    #[must_use]
    pub const fn whole_seconds(self) -> i64 {
        self.0.div_euclid(MILLIS_PER_SECOND)
    }

    /// The span in whole 24-hour days, truncated toward negative infinity.
    #[must_use]
    pub const fn whole_days(self) -> i64 {
        self.0.div_euclid(MILLIS_PER_DAY)
    }
}

impl Add for EpochDelta {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl Sub for EpochDelta {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

impl Neg for EpochDelta {
    type Output = Self;

    fn neg(self) -> Self {
        Self(self.0.saturating_neg())
    }
}

// --- The clock seam ------------------------------------------------------

/// The injectable time provider: a stored, cloneable "what time is it".
///
/// A clock is I/O. It belongs at a composition root, injected into everything
/// below, so the logic that depends on "now" stays a pure function of its
/// input. Nothing else in this crate reads the system clock.
///
/// This is an erased closure rather than a trait, for the same reason the C++
/// original is a `std::function<Epoch()>`: where a single function signature
/// already *is* the interface, a one-method trait is ceremony. It is a named
/// struct rather than a bare `Arc<dyn Fn…>` alias because a stored `Arc` does
/// not itself implement `Fn`, so the alias could be held but not called
/// without wrapping it in another closure at every use.
///
/// Cloning is cheap and shares one underlying closure — `std::function` is
/// copyable and consumers keep the same source in several places.
///
/// ```
/// use bc_date_time::{Epoch, EpochSource};
///
/// // Any closure is a clock; the two factories below are the common ones.
/// let ticks = std::sync::atomic::AtomicI64::new(0);
/// let counting = EpochSource::new(move || {
///     Epoch::from_millis(ticks.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
/// });
/// assert_eq!(counting.now(), Epoch::from_millis(0));
/// assert_eq!(counting.now(), Epoch::from_millis(1));
/// ```
#[derive(Clone)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Object))]
pub struct EpochSource(Arc<dyn Fn() -> Epoch + Send + Sync>);

// Split from the block below so the exported half holds only what can cross:
// `new` takes an `impl Fn`, which has no foreign representation. A foreign
// caller reaches the same stored closure through `EpochSource::from_foreign`
// (see `crate::ffi`), never through a second one.
#[cfg_attr(feature = "uniffi", uniffi::export)]
impl EpochSource {
    /// Reads the current instant.
    #[must_use]
    pub fn now(&self) -> Epoch {
        (self.0)()
    }
}

impl EpochSource {
    /// Wraps any closure as a clock.
    #[must_use]
    pub fn new(read: impl Fn() -> Epoch + Send + Sync + 'static) -> Self {
        Self(Arc::new(read))
    }
}

impl std::fmt::Debug for EpochSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A closure has nothing printable, and reading the clock to render a
        // `Debug` line would make formatting an observation.
        formatter.write_str("EpochSource(..)")
    }
}

impl<F: Fn() -> Epoch + Send + Sync + 'static> From<F> for EpochSource {
    fn from(read: F) -> Self {
        Self::new(read)
    }
}

/// An [`EpochSource`] backed by the system wall clock.
///
/// Wall clock, never monotonic — an `Epoch` is a calendar instant, and this
/// crate deliberately has no steady-clock concept (see the crate docs). The
/// returned source can therefore jump backwards when the host's clock is
/// corrected; consumers that care must say so explicitly.
///
/// # Platform
///
/// On `wasm32-unknown-unknown` std has no clock and [`EpochSource::now`]
/// **panics** on a source from this factory. It still compiles, which is
/// exactly why the seam matters: a wasm consumer injects a different source
/// rather than taking this one.
#[must_use]
pub fn system_epoch_source() -> EpochSource {
    EpochSource::new(|| {
        let since_epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        match since_epoch {
            Ok(elapsed) => Epoch::from_millis(clamp_i64(elapsed.as_millis())),
            // The host clock reads before 1970. `duration_since` reports the
            // magnitude, so negate it rather than discarding the reading: a
            // wrong-but-faithful clock is the caller's problem to notice, and
            // silently substituting "now" would hide it.
            Err(before_epoch) => {
                Epoch::from_millis(-clamp_i64(before_epoch.duration().as_millis()))
            }
        }
    })
}

/// An [`EpochSource`] that always returns `value` — the test clock.
#[must_use]
pub fn fixed_epoch_source(value: Epoch) -> EpochSource {
    EpochSource::new(move || value)
}

/// Narrows a `u128` millisecond count from `std::time::Duration`.
///
/// Unreachable in practice: `i64::MAX` milliseconds is the year 292'277'026, so
/// a system clock would have to be absurdly misconfigured to overflow. Clamping
/// keeps a release build from wrapping into a negative instant if it ever were.
fn clamp_i64(millis: u128) -> i64 {
    debug_assert!(
        i64::try_from(millis).is_ok(),
        "system clock reports {millis} ms, past the year 292'277'026"
    );
    i64::try_from(millis).unwrap_or(i64::MAX)
}

// --- Calendar enums ------------------------------------------------------

/// Day of the week, one-indexed: Sunday = 1 through Saturday = 7.
///
/// One-indexed to match [`Month`], so neither enum needs special-casing at a
/// call site. `#[repr(u8)]` with written-out discriminants because these cross
/// a future C ABI as values — the number is part of the contract, not an
/// artifact of declaration order.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum Weekday {
    /// Sunday.
    Sunday = 1,
    /// Monday.
    Monday = 2,
    /// Tuesday.
    Tuesday = 3,
    /// Wednesday.
    Wednesday = 4,
    /// Thursday.
    Thursday = 5,
    /// Friday.
    Friday = 6,
    /// Saturday.
    Saturday = 7,
}

impl Weekday {
    /// Every weekday, in order, Sunday first.
    pub const ALL: [Self; 7] = [
        Self::Sunday,
        Self::Monday,
        Self::Tuesday,
        Self::Wednesday,
        Self::Thursday,
        Self::Friday,
        Self::Saturday,
    ];

    /// The one-indexed number, 1 (Sunday) through 7 (Saturday).
    #[must_use]
    pub const fn number(self) -> u8 {
        self as u8
    }

    /// The zero-indexed value, 0 through 6, for array indexing.
    ///
    /// A named conversion rather than raw arithmetic at the call site, so the
    /// intent is legible where it is used.
    #[must_use]
    pub const fn zero_indexed(self) -> u8 {
        self as u8 - 1
    }

    /// The weekday for a one-indexed number, or `None` if it is not 1..=7.
    #[must_use]
    pub const fn from_number(number: u8) -> Option<Self> {
        match number {
            1 => Some(Self::Sunday),
            2 => Some(Self::Monday),
            3 => Some(Self::Tuesday),
            4 => Some(Self::Wednesday),
            5 => Some(Self::Thursday),
            6 => Some(Self::Friday),
            7 => Some(Self::Saturday),
            _ => None,
        }
    }

    /// The full US English name, e.g. `"Sunday"`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sunday => "Sunday",
            Self::Monday => "Monday",
            Self::Tuesday => "Tuesday",
            Self::Wednesday => "Wednesday",
            Self::Thursday => "Thursday",
            Self::Friday => "Friday",
            Self::Saturday => "Saturday",
        }
    }

    /// The three-letter US English abbreviation, e.g. `"Sun"`.
    #[must_use]
    pub const fn abbr(self) -> &'static str {
        match self {
            Self::Sunday => "Sun",
            Self::Monday => "Mon",
            Self::Tuesday => "Tue",
            Self::Wednesday => "Wed",
            Self::Thursday => "Thu",
            Self::Friday => "Fri",
            Self::Saturday => "Sat",
        }
    }

    /// Whether this is Saturday or Sunday.
    #[must_use]
    pub const fn is_weekend(self) -> bool {
        matches!(self, Self::Saturday | Self::Sunday)
    }
}

/// Month of the year, one-indexed: January = 1 through December = 12.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum Month {
    /// January.
    January = 1,
    /// February.
    February = 2,
    /// March.
    March = 3,
    /// April.
    April = 4,
    /// May.
    May = 5,
    /// June.
    June = 6,
    /// July.
    July = 7,
    /// August.
    August = 8,
    /// September.
    September = 9,
    /// October.
    October = 10,
    /// November.
    November = 11,
    /// December.
    December = 12,
}

impl Month {
    /// Every month, in order, January first.
    pub const ALL: [Self; 12] = [
        Self::January,
        Self::February,
        Self::March,
        Self::April,
        Self::May,
        Self::June,
        Self::July,
        Self::August,
        Self::September,
        Self::October,
        Self::November,
        Self::December,
    ];

    /// The one-indexed number, 1 (January) through 12 (December).
    #[must_use]
    pub const fn number(self) -> u8 {
        self as u8
    }

    /// The zero-indexed value, 0 through 11, for array indexing.
    #[must_use]
    pub const fn zero_indexed(self) -> u8 {
        self as u8 - 1
    }

    /// The month for a one-indexed number, or `None` if it is not 1..=12.
    #[must_use]
    pub const fn from_number(number: u8) -> Option<Self> {
        match number {
            1 => Some(Self::January),
            2 => Some(Self::February),
            3 => Some(Self::March),
            4 => Some(Self::April),
            5 => Some(Self::May),
            6 => Some(Self::June),
            7 => Some(Self::July),
            8 => Some(Self::August),
            9 => Some(Self::September),
            10 => Some(Self::October),
            11 => Some(Self::November),
            12 => Some(Self::December),
            _ => None,
        }
    }

    /// The full US English name, e.g. `"January"`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::January => "January",
            Self::February => "February",
            Self::March => "March",
            Self::April => "April",
            Self::May => "May",
            Self::June => "June",
            Self::July => "July",
            Self::August => "August",
            Self::September => "September",
            Self::October => "October",
            Self::November => "November",
            Self::December => "December",
        }
    }

    /// The three-letter US English abbreviation, e.g. `"Jan"`.
    #[must_use]
    pub const fn abbr(self) -> &'static str {
        match self {
            Self::January => "Jan",
            Self::February => "Feb",
            Self::March => "Mar",
            Self::April => "Apr",
            Self::May => "May",
            Self::June => "Jun",
            Self::July => "Jul",
            Self::August => "Aug",
            Self::September => "Sep",
            Self::October => "Oct",
            Self::November => "Nov",
            Self::December => "Dec",
        }
    }
}

// --- Aggregate types -----------------------------------------------------

/// A half-open interval over [`Epoch`] values: `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct EpochRange {
    /// Inclusive lower bound.
    pub start: Epoch,
    /// Exclusive upper bound.
    pub end: Epoch,
}

impl EpochRange {
    /// Builds a range. No ordering is enforced — an inverted range simply
    /// contains nothing and reports a negative duration, matching the C++.
    #[must_use]
    pub const fn new(start: Epoch, end: Epoch) -> Self {
        Self { start, end }
    }

    /// Whether `value` falls in `[start, end)` — start inclusive, end exclusive.
    #[must_use]
    pub const fn contains(&self, value: Epoch) -> bool {
        value.0 >= self.start.0 && value.0 < self.end.0
    }

    /// `end - start`. Negative for an inverted range.
    #[must_use]
    pub const fn duration(&self) -> EpochDelta {
        self.end.delta_since(self.start)
    }
}

/// A UTC instant decomposed into Gregorian calendar fields.
///
/// `weekday` is **derived**, not input: [`components_to_epoch`] ignores it,
/// because the date already determines it. Build these with
/// [`DateComponents::new`], which fills it in correctly, rather than by struct
/// literal — the fields stay public only so a caller can read them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct DateComponents {
    /// Full year, e.g. 2026. Proleptic Gregorian; may be zero or negative.
    pub year: i32,
    /// Month of year.
    pub month: Month,
    /// Day of month, 1..=31.
    pub day: u8,
    /// Hour of day, 0..=23.
    pub hour: u8,
    /// Minute of hour, 0..=59.
    pub minute: u8,
    /// Second of minute, 0..=59.
    pub second: u8,
    /// Millisecond of second, 0..=999.
    pub millisecond: u16,
    /// Day of week. Derived from the date; ignored when converting to an epoch.
    pub weekday: Weekday,
}

impl DateComponents {
    /// Builds components with `weekday` derived from the date.
    ///
    /// Out-of-range fields are not rejected here — they normalize on conversion
    /// exactly as `<chrono>` normalizes them in the C++ original (second 60
    /// rolls into the next minute, month 13 into the next January). Callers
    /// that must reject an impossible date use the parsers in
    /// [`crate::parse_iso_date`] and [`crate::parse_iso_timestamp`], which
    /// range-check first.
    #[must_use]
    pub fn new(
        year: i32,
        month: Month,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        millisecond: u16,
    ) -> Self {
        let day_number =
            days_from_civil(i64::from(year), i64::from(month.number()), i64::from(day));
        Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
            millisecond,
            weekday: weekday_from_day_number(day_number),
        }
    }
}

/// An ISO 8601 week date.
///
/// `year` is the ISO week-numbering year, which differs from the calendar year
/// for days in early January (they belong to the prior ISO year) and late
/// December (they belong to the next).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct IsoWeek {
    /// ISO week-numbering year.
    pub year: i32,
    /// Week of that year, 1..=53.
    pub week: u8,
}

// --- Calendar utilities --------------------------------------------------

/// The number of days in `month` of `year`, accounting for leap years.
///
/// Takes a [`Month`] rather than the C++ `uint8_t`, which makes the original's
/// out-of-range branch — it silently returned 30 — unrepresentable.
#[must_use]
pub const fn days_in_month(year: i32, month: Month) -> u8 {
    match month {
        Month::February if is_leap_year(year) => 29,
        Month::February => 28,
        Month::April | Month::June | Month::September | Month::November => 30,
        _ => 31,
    }
}

/// Whether `year` is a Gregorian leap year.
#[must_use]
pub const fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// The ISO 8601 week-numbering year and week for a calendar date.
///
/// Week 1 is the week containing the year's first Thursday; days before it
/// belong to the last week (52 or 53) of the prior ISO year.
///
/// The C++ original reads `components.weekday` and trusts it, so a struct built
/// by hand with a wrong weekday yields a wrong week. This derives the weekday
/// from the date instead, which can only differ when the input was already
/// self-inconsistent.
#[must_use]
pub fn iso_week_of(components: &DateComponents) -> IsoWeek {
    let year = i64::from(components.year);
    let mut ordinal = i64::from(components.day);
    for month in Month::ALL {
        if month.number() >= components.month.number() {
            break;
        }
        ordinal += i64::from(days_in_month(components.year, month));
    }

    let day_number = days_from_civil(
        year,
        i64::from(components.month.number()),
        i64::from(components.day),
    );
    // ISO numbers weekdays Monday = 1 .. Sunday = 7; ours is Sunday = 1.
    let weekday = weekday_from_day_number(day_number);
    let iso_weekday = if matches!(weekday, Weekday::Sunday) {
        7
    } else {
        i64::from(weekday.zero_indexed())
    };

    let mut week = (ordinal - iso_weekday + 10) / 7;
    let mut iso_year = year;
    if week < 1 {
        iso_year = year - 1;
        week = iso_weeks_in_year(iso_year);
    } else if week > iso_weeks_in_year(year) {
        iso_year = year + 1;
        week = 1;
    }

    IsoWeek {
        year: narrow_year(iso_year),
        week: clamp_u8(week, 53),
    }
}

/// Weekday index of 31 December in `year`, Sunday = 0.
///
/// The C++ computes this with a Gauss-style closed form over `year % 7`, which
/// is wrong for negative years because C++ (and Rust) truncate the remainder
/// toward zero. Going through the day-number conversion is both correct across
/// the sign change and one fewer formula to trust.
fn december_31_weekday_index(year: i64) -> i64 {
    weekday_index_from_days(days_from_civil(year, 12, 31))
}

/// 53 when `year` has 53 ISO weeks, 52 otherwise.
///
/// A year has 53 ISO weeks exactly when it contains 53 Thursdays: either its
/// own 31 December is a Thursday, or the prior year's was a Wednesday (making
/// this one a leap year that gained the extra Thursday).
fn iso_weeks_in_year(year: i64) -> i64 {
    // Sunday = 0, so Thursday = 4 and Wednesday = 3.
    if december_31_weekday_index(year) == 4 || december_31_weekday_index(year - 1) == 3 {
        53
    } else {
        52
    }
}

// --- Calendar conversions ------------------------------------------------

/// Decomposes a UTC epoch into Gregorian calendar components.
#[must_use]
pub fn epoch_to_components(epoch: Epoch) -> DateComponents {
    let millis = epoch.millis();
    let day_number = millis.div_euclid(MILLIS_PER_DAY);
    let millis_of_day = millis.rem_euclid(MILLIS_PER_DAY);
    let (year, month_number, day) = civil_from_days(day_number);

    DateComponents {
        year: narrow_year(year),
        month: month_from_civil(month_number),
        day: clamp_u8(day, 31),
        hour: clamp_u8(millis_of_day / MILLIS_PER_HOUR, 23),
        minute: clamp_u8((millis_of_day % MILLIS_PER_HOUR) / MILLIS_PER_MINUTE, 59),
        second: clamp_u8((millis_of_day % MILLIS_PER_MINUTE) / MILLIS_PER_SECOND, 59),
        millisecond: clamp_u16(millis_of_day % MILLIS_PER_SECOND, 999),
        weekday: weekday_from_day_number(day_number),
    }
}

/// Composes Gregorian calendar components into a UTC epoch.
///
/// The inverse of [`epoch_to_components`]. `components.weekday` is ignored —
/// the date determines it. Out-of-range fields normalize rather than failing,
/// matching the `<chrono>` arithmetic the C++ builds on: second 60 becomes
/// second 0 of the next minute.
#[must_use]
pub fn components_to_epoch(components: &DateComponents) -> Epoch {
    let day_number = days_from_civil(
        i64::from(components.year),
        i64::from(components.month.number()),
        i64::from(components.day),
    );
    // Saturating, like every other arithmetic operation in this crate, and for
    // the same reason: `year` is an `i32` the caller may set freely, and a year
    // near `i32::MAX` is about seven times past what `i64` milliseconds can
    // hold. Plain `*` would panic in debug and silently wrap to a wrong instant
    // in release — the two behaviors this crate's tests deliberately keep in
    // agreement, and the silent-wrong-value it exists to avoid.
    Epoch::from_millis(
        day_number
            .saturating_mul(MILLIS_PER_DAY)
            .saturating_add(i64::from(components.hour).saturating_mul(MILLIS_PER_HOUR))
            .saturating_add(i64::from(components.minute).saturating_mul(MILLIS_PER_MINUTE))
            .saturating_add(i64::from(components.second).saturating_mul(MILLIS_PER_SECOND))
            .saturating_add(i64::from(components.millisecond)),
    )
}

// --- Narrowing helpers ---------------------------------------------------
//
// Each of these narrows a value the surrounding arithmetic has already bounded.
// The bounds are structural — `rem_euclid` and `civil_from_days` cannot produce
// anything else — so the `debug_assert!` turns a future change to that
// arithmetic into a test failure, and the clamp keeps a release build from
// wrapping into a plausible-looking wrong answer if one ever slipped through.

/// Narrows a civil year to the `i32` [`DateComponents`] carries.
///
/// Cannot truncate: `i64::MAX` milliseconds is about ±292 million years, and
/// `i32` reaches ±2.1 billion — seven times further. The assert pins that
/// reasoning against a future change to the epoch's unit.
fn narrow_year(year: i64) -> i32 {
    debug_assert!(
        i32::try_from(year).is_ok(),
        "civil year {year} does not fit i32; has the epoch unit changed?"
    );
    match i32::try_from(year) {
        Ok(narrow) => narrow,
        Err(_) if year.is_negative() => i32::MIN,
        Err(_) => i32::MAX,
    }
}

fn clamp_u8(value: i64, ceiling: u8) -> u8 {
    debug_assert!(
        (0..=i64::from(ceiling)).contains(&value),
        "{value} is outside the expected range 0..={ceiling}"
    );
    match u8::try_from(value) {
        Ok(narrow) => narrow.min(ceiling),
        Err(_) if value.is_negative() => 0,
        Err(_) => ceiling,
    }
}

fn clamp_u16(value: i64, ceiling: u16) -> u16 {
    debug_assert!(
        (0..=i64::from(ceiling)).contains(&value),
        "{value} is outside the expected range 0..={ceiling}"
    );
    match u16::try_from(value) {
        Ok(narrow) => narrow.min(ceiling),
        Err(_) if value.is_negative() => 0,
        Err(_) => ceiling,
    }
}

/// The [`Month`] for a `civil_from_days` month number.
fn month_from_civil(month_number: i64) -> Month {
    if let Some(month) = u8::try_from(month_number).ok().and_then(Month::from_number) {
        return month;
    }
    // Unreachable: `civil_from_days` returns 1..=12 by construction.
    debug_assert!(false, "civil_from_days produced month {month_number}");
    // A graceful path still has to name a month. January is the one choice that
    // is obviously wrong in any rendered output rather than quietly plausible,
    // which is what a fallback should be.
    Month::January
}

/// The [`Weekday`] for a day number.
fn weekday_from_day_number(day_number: i64) -> Weekday {
    // `weekday_index_from_days` returns 0..=6 (Sunday = 0) and our enum is
    // one-indexed, so this is always in range.
    let numbered = u8::try_from(weekday_index_from_days(day_number) + 1)
        .ok()
        .and_then(Weekday::from_number);
    if let Some(weekday) = numbered {
        return weekday;
    }
    debug_assert!(false, "weekday index outside 0..=6 for day {day_number}");
    Weekday::Sunday
}
