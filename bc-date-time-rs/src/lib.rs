//! Canonical UTC time for the `bc` family: one epoch type, one injectable
//! clock, and the Gregorian calendar math that goes with them.
//!
//! A semantics-preserving Rust port of `libs/foundation/date_time` from
//! `bourgeois_concern`. Three ideas carry the whole crate:
//!
//! 1. **[`Epoch`] is the only way to say when.** Milliseconds since
//!    1970-01-01T00:00:00Z, UTC, as an `i64`. Integer milliseconds have no
//!    floating-point comparison hazards, `i64` covers ±292 million years, and
//!    the unit matches JavaScript and most database timestamp columns. Every
//!    consumer stores this rather than growing a local timestamp type.
//! 2. **The clock is injected, never read.** Nothing here calls the system
//!    clock except [`system_epoch_source`], which exists so a composition root
//!    can hand one down. [`fixed_epoch_source`] is its test counterpart, and
//!    the reason time-dependent logic can be a pure function.
//! 3. **UTC, and only UTC.** There is no timezone database and no local-time
//!    concept. The parsers reject an offset suffix rather than interpreting it.
//!
//! ```
//! use bc_date_time::{
//!     DateFormatBuilder, Epoch, EpochDelta, fixed_epoch_source, format_iso_timestamp,
//!     parse_iso_timestamp,
//! };
//!
//! // A clock a test controls completely.
//! let clock = fixed_epoch_source(Epoch::from_millis(1_771_338_645_123));
//! let now = clock.now();
//!
//! assert_eq!(format_iso_timestamp(now), "2026-02-17 14:30:45 UTC");
//! assert_eq!(DateFormatBuilder::us_long().format(now), "February 17, 2026");
//!
//! // Formatting and parsing are inverses over the ISO 8601 form.
//! let rendered = DateFormatBuilder::iso8601().format(now);
//! assert_eq!(rendered, "2026-02-17T14:30:45.123Z");
//! assert_eq!(parse_iso_timestamp(&rendered), Some(now));
//!
//! // Spans are their own type, so a point in time cannot be added to another.
//! let deadline = now + EpochDelta::from_days(3);
//! assert_eq!(deadline.delta_since(now), EpochDelta::from_hours(72));
//! ```
//!
//! # What is deliberately out of scope
//!
//! Non-Gregorian calendars; locale beyond US English; timezone conversion; and
//! **monotonic clocks** — an [`Epoch`] is wall-clock time, which can jump when
//! the host's clock is corrected. Measuring elapsed time across such a jump is
//! a different facility, and conflating the two is how duration bugs start.

#![deny(missing_docs)]

// Must sit at the crate root: the derives on the types below expand to
// references to `crate::UniFfiTag`, which this defines. The rest of the
// binding lives in `ffi`.
#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

mod business_days;
mod civil;
#[cfg(feature = "uniffi")]
pub mod ffi;
mod formatter;
mod parser;
mod relative_time;
mod time;

pub use business_days::add_business_days;
pub use formatter::{
    DateFormatBuilder, DateFormatter, DayStyle, HourStyle, MinSecStyle, MonthStyle, WeekdayStyle,
    YearStyle, format_iso_date, format_iso_timestamp, format_iso_week_version,
};
pub use parser::{parse_iso_date, parse_iso_timestamp};
pub use relative_time::format_time_ago;
pub use time::{
    DateComponents, Epoch, EpochDelta, EpochRange, EpochSource, IsoWeek, Month, Weekday,
    components_to_epoch, days_in_month, epoch_to_components, fixed_epoch_source, is_leap_year,
    iso_week_of, system_epoch_source,
};

/// Compiles every ```rust block in README.md as a doctest, so the examples the
/// README shows cannot drift from the API it documents.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeExamples;
