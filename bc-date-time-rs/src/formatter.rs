//! Composable date formatting.
//!
//! Ported from `libs/foundation/date_time/src/date_formatter.{hpp,cpp}`. There
//! is no format-string language: a format is a list of typed parts, so a typo
//! is a compile error rather than a silently wrong rendering. All output is
//! UTC and US English.

// `fmt::Write` is what lets `write!` append to a `String` without an intermediate `format!`.
use std::fmt::Write;
use std::sync::Arc;

use crate::time::{DateComponents, Epoch, Month, epoch_to_components, iso_week_of};

/// A compiled formatter: turns an [`Epoch`] into a string.
///
/// Produced by [`DateFormatBuilder::build`]. Callers own and inject these —
/// there is deliberately no registry of cached formatters, because hidden
/// static state is exactly what the injectable-clock design avoids. Cloning is
/// cheap and shares one compiled part list, matching the copyable C++
/// `std::function` this ports.
///
/// A named type rather than an `Arc<dyn Fn…>` alias, for the same reason
/// [`crate::EpochSource`] is one: an alias can be *called* (deref coercion
/// makes `formatter(epoch)` work) but it cannot satisfy an `impl Fn` bound, it
/// has no `Debug`, and it can never grow a method or an FFI representation
/// without breaking every call site. A nominal type keeps all three doors open.
///
/// ```
/// use bc_date_time::{DateFormatBuilder, Epoch};
///
/// let formatter = DateFormatBuilder::iso_date();
/// assert_eq!(formatter.format(Epoch::ZERO), "1970-01-01");
///
/// // Holding one in an ordinary struct works, which a bare closure alias
/// // would not allow.
/// #[derive(Debug, Clone)]
/// struct Renderer { stamp: bc_date_time::DateFormatter }
/// let renderer = Renderer { stamp: formatter };
/// assert_eq!(renderer.stamp.format(Epoch::ZERO), "1970-01-01");
/// ```
#[derive(Clone)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Object))]
pub struct DateFormatter(Arc<dyn Fn(Epoch) -> String + Send + Sync>);

// Split from the block below so the exported half holds only what can cross a
// foreign boundary: `new` takes an `impl Fn`, which has no representation
// there. A foreign caller reaches the same stored closure through
// `DateFormatter::from_foreign` (see `crate::ffi`), never through a second one.
#[cfg_attr(feature = "uniffi", uniffi::export)]
impl DateFormatter {
    /// Renders `instant`.
    #[must_use]
    pub fn format(&self, instant: Epoch) -> String {
        (self.0)(instant)
    }
}

impl DateFormatter {
    /// Wraps any closure as a formatter.
    ///
    /// The counterpart to [`EpochSource::new`](crate::EpochSource::new), which
    /// this type had gone without: every formatter came from
    /// [`DateFormatBuilder`], so a caller whose format the builder cannot spell
    /// — a locale table, a lookup, anything conditional — had no way in at all.
    ///
    /// ```
    /// use bc_date_time::{DateFormatter, Epoch};
    ///
    /// let ordinal = DateFormatter::new(|epoch| format!("day {}", epoch.millis() / 86_400_000));
    /// assert_eq!(ordinal.format(Epoch::from_millis(172_800_000)), "day 2");
    /// ```
    #[must_use]
    pub fn new(render: impl Fn(Epoch) -> String + Send + Sync + 'static) -> Self {
        Self(Arc::new(render))
    }
}

impl std::fmt::Debug for DateFormatter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The compiled part list is captured in a closure and cannot be read
        // back; rendering a sample epoch to describe it would make `Debug` a
        // computation rather than a description.
        formatter.write_str("DateFormatter(..)")
    }
}

/// How to render a year.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum YearStyle {
    /// The year as written: `2026`, `999`, `-44`.
    Full,
    /// The last two digits, zero-padded: `26`.
    Short2,
    /// Zero-padded to four digits: `2026`, `0999`.
    ///
    /// Not in the C++ original, which has only `full`. ISO 8601 requires four
    /// digits, so `full` renders a year below 1000 as text no ISO parser will
    /// accept; the ISO-named factories below use this instead. For every year
    /// from 1000 on, the two are identical.
    Padded4,
}

/// How to render a month.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MonthStyle {
    /// `2`
    Numeric,
    /// `02`
    Padded,
    /// `Feb`
    Abbreviated,
    /// `February`
    FullName,
}

/// How to render a day of month.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DayStyle {
    /// `3`
    Numeric,
    /// `03`
    Padded,
}

/// How to render a weekday.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeekdayStyle {
    /// `Tue`
    Abbreviated,
    /// `Tuesday`
    FullName,
}

/// How to render an hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HourStyle {
    /// 24-hour, unpadded: `14`, `2`
    H24,
    /// 24-hour, zero-padded: `14`, `02`
    H24Padded,
    /// 12-hour, unpadded: `2`
    H12,
    /// 12-hour, zero-padded: `02`
    H12Padded,
}

/// How to render a minute or second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MinSecStyle {
    /// `05`
    Padded,
    /// `5`
    Numeric,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FormatPart {
    Year(YearStyle),
    Month(MonthStyle),
    Day(DayStyle),
    Weekday(WeekdayStyle),
    Hour(HourStyle),
    Minute(MinSecStyle),
    Second(MinSecStyle),
    Millisecond,
    AmPm,
    Literal(String),
}

/// Builds a [`DateFormatter`] from a list of parts.
///
/// Each method appends a part and returns `&mut Self`, so calls chain. The
/// builder is **not** consuming: [`build`](DateFormatBuilder::build) takes
/// `&self` and snapshots the current parts, so one builder can be extended and
/// rebuilt to produce independent formatters — the C++ semantics, which its own
/// test suite pins.
///
/// Where the C++ uses a default argument, this has a pair: `month()` takes the
/// default style, `month_styled(..)` takes an explicit one.
///
/// ```
/// use bc_date_time::{DateFormatBuilder, Epoch, MonthStyle};
///
/// let formatter = DateFormatBuilder::new()
///     .month_styled(MonthStyle::Abbreviated)
///     .literal(" ")
///     .day()
///     .literal(", ")
///     .year()
///     .build();
/// assert_eq!(formatter.format(Epoch::from_millis(1_771_338_645_123)), "Feb 17, 2026");
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DateFormatBuilder {
    parts: Vec<FormatPart>,
}

impl DateFormatBuilder {
    /// An empty builder. Building it yields a formatter that returns `""`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends the year, written out in full.
    pub fn year(&mut self) -> &mut Self {
        self.year_styled(YearStyle::Full)
    }

    /// Appends the year in the given style.
    pub fn year_styled(&mut self, style: YearStyle) -> &mut Self {
        self.parts.push(FormatPart::Year(style));
        self
    }

    /// Appends the month as an unpadded number.
    pub fn month(&mut self) -> &mut Self {
        self.month_styled(MonthStyle::Numeric)
    }

    /// Appends the month in the given style.
    pub fn month_styled(&mut self, style: MonthStyle) -> &mut Self {
        self.parts.push(FormatPart::Month(style));
        self
    }

    /// Appends the day of month as an unpadded number.
    pub fn day(&mut self) -> &mut Self {
        self.day_styled(DayStyle::Numeric)
    }

    /// Appends the day of month in the given style.
    pub fn day_styled(&mut self, style: DayStyle) -> &mut Self {
        self.parts.push(FormatPart::Day(style));
        self
    }

    /// Appends the weekday's full name.
    pub fn weekday(&mut self) -> &mut Self {
        self.weekday_styled(WeekdayStyle::FullName)
    }

    /// Appends the weekday in the given style.
    pub fn weekday_styled(&mut self, style: WeekdayStyle) -> &mut Self {
        self.parts.push(FormatPart::Weekday(style));
        self
    }

    /// Appends the hour, 24-hour and unpadded.
    pub fn hour(&mut self) -> &mut Self {
        self.hour_styled(HourStyle::H24)
    }

    /// Appends the hour in the given style.
    pub fn hour_styled(&mut self, style: HourStyle) -> &mut Self {
        self.parts.push(FormatPart::Hour(style));
        self
    }

    /// Appends the minute, zero-padded.
    pub fn minute(&mut self) -> &mut Self {
        self.minute_styled(MinSecStyle::Padded)
    }

    /// Appends the minute in the given style.
    pub fn minute_styled(&mut self, style: MinSecStyle) -> &mut Self {
        self.parts.push(FormatPart::Minute(style));
        self
    }

    /// Appends the second, zero-padded.
    pub fn second(&mut self) -> &mut Self {
        self.second_styled(MinSecStyle::Padded)
    }

    /// Appends the second in the given style.
    pub fn second_styled(&mut self, style: MinSecStyle) -> &mut Self {
        self.parts.push(FormatPart::Second(style));
        self
    }

    /// Appends the millisecond, always zero-padded to three digits.
    pub fn millisecond(&mut self) -> &mut Self {
        self.parts.push(FormatPart::Millisecond);
        self
    }

    /// Appends `AM` or `PM`.
    pub fn ampm(&mut self) -> &mut Self {
        self.parts.push(FormatPart::AmPm);
        self
    }

    /// Appends fixed text.
    pub fn literal(&mut self, text: impl Into<String>) -> &mut Self {
        self.parts.push(FormatPart::Literal(text.into()));
        self
    }

    /// Snapshots the current parts into a reusable formatter.
    ///
    /// Takes `&self` and copies, so the builder stays usable afterwards and a
    /// later `build()` is unaffected by what this one captured.
    #[must_use]
    pub fn build(&self) -> DateFormatter {
        let parts = self.parts.clone();
        DateFormatter(Arc::new(move |epoch| {
            // Decompose once per format call, not once per part.
            let components = epoch_to_components(epoch);
            let mut rendered = String::new();
            for part in &parts {
                render_part(part, &components, &mut rendered);
            }
            rendered
        }))
    }

    // --- Static factories for common formats ---

    /// `2026-02-17T14:30:45.123Z`
    #[must_use]
    pub fn iso8601() -> DateFormatter {
        Self::new()
            .year_styled(YearStyle::Padded4)
            .literal("-")
            .month_styled(MonthStyle::Padded)
            .literal("-")
            .day_styled(DayStyle::Padded)
            .literal("T")
            .hour_styled(HourStyle::H24Padded)
            .literal(":")
            .minute()
            .literal(":")
            .second()
            .literal(".")
            .millisecond()
            .literal("Z")
            .build()
    }

    /// `2/17/2026`
    #[must_use]
    pub fn us_short() -> DateFormatter {
        Self::new()
            .month()
            .literal("/")
            .day()
            .literal("/")
            .year()
            .build()
    }

    /// `February 17, 2026`
    #[must_use]
    pub fn us_long() -> DateFormatter {
        Self::new()
            .month_styled(MonthStyle::FullName)
            .literal(" ")
            .day()
            .literal(", ")
            .year()
            .build()
    }

    /// `Tuesday, February 17, 2026`
    #[must_use]
    pub fn us_long_with_weekday() -> DateFormatter {
        Self::new()
            .weekday()
            .literal(", ")
            .month_styled(MonthStyle::FullName)
            .literal(" ")
            .day()
            .literal(", ")
            .year()
            .build()
    }

    /// `February 17, 2026 2:30 PM`
    #[must_use]
    pub fn us_datetime() -> DateFormatter {
        Self::new()
            .month_styled(MonthStyle::FullName)
            .literal(" ")
            .day()
            .literal(", ")
            .year()
            .literal(" ")
            .hour_styled(HourStyle::H12)
            .literal(":")
            .minute()
            .literal(" ")
            .ampm()
            .build()
    }

    /// `February 17, 2026 2:30:45 PM`
    #[must_use]
    pub fn us_datetime_seconds() -> DateFormatter {
        Self::new()
            .month_styled(MonthStyle::FullName)
            .literal(" ")
            .day()
            .literal(", ")
            .year()
            .literal(" ")
            .hour_styled(HourStyle::H12)
            .literal(":")
            .minute()
            .literal(":")
            .second()
            .literal(" ")
            .ampm()
            .build()
    }

    /// `2026-02-17`
    #[must_use]
    pub fn iso_date() -> DateFormatter {
        Self::new()
            .year_styled(YearStyle::Padded4)
            .literal("-")
            .month_styled(MonthStyle::Padded)
            .literal("-")
            .day_styled(DayStyle::Padded)
            .build()
    }

    /// `2026-02-17 14:30`
    #[must_use]
    pub fn iso_datetime_minute() -> DateFormatter {
        Self::new()
            .year_styled(YearStyle::Padded4)
            .literal("-")
            .month_styled(MonthStyle::Padded)
            .literal("-")
            .day_styled(DayStyle::Padded)
            .literal(" ")
            .hour_styled(HourStyle::H24Padded)
            .literal(":")
            .minute()
            .build()
    }
}

fn render_part(part: &FormatPart, components: &DateComponents, out: &mut String) {
    match part {
        FormatPart::Year(YearStyle::Full) => out.push_str(&components.year.to_string()),
        FormatPart::Year(YearStyle::Short2) => {
            out.push_str(&pad2(components.year.rem_euclid(100)));
        }
        FormatPart::Year(YearStyle::Padded4) => {
            // `{:04}` counts the sign as one of the four, which is what ISO
            // 8601's expanded form does too. Writing into a `String` cannot
            // fail, so its `fmt::Result` carries nothing.
            let _ = write!(out, "{:04}", components.year);
        }
        FormatPart::Month(style) => out.push_str(&render_month(components.month, *style)),
        FormatPart::Day(DayStyle::Numeric) => out.push_str(&components.day.to_string()),
        FormatPart::Day(DayStyle::Padded) => out.push_str(&pad2(i32::from(components.day))),
        FormatPart::Weekday(WeekdayStyle::Abbreviated) => out.push_str(components.weekday.abbr()),
        FormatPart::Weekday(WeekdayStyle::FullName) => out.push_str(components.weekday.name()),
        FormatPart::Hour(style) => out.push_str(&render_hour(components.hour, *style)),
        FormatPart::Minute(style) => out.push_str(&render_min_sec(components.minute, *style)),
        FormatPart::Second(style) => out.push_str(&render_min_sec(components.second, *style)),
        FormatPart::Millisecond => {
            // Writing into a `String` cannot fail, so its `fmt::Result` carries nothing.
            let _ = write!(out, "{:03}", components.millisecond);
        }
        FormatPart::AmPm => out.push_str(if components.hour < 12 { "AM" } else { "PM" }),
        FormatPart::Literal(text) => out.push_str(text),
    }
}

fn render_month(month: Month, style: MonthStyle) -> String {
    match style {
        MonthStyle::Numeric => month.number().to_string(),
        MonthStyle::Padded => pad2(i32::from(month.number())),
        MonthStyle::Abbreviated => month.abbr().to_string(),
        MonthStyle::FullName => month.name().to_string(),
    }
}

fn render_hour(hour: u8, style: HourStyle) -> String {
    match style {
        HourStyle::H24 => hour.to_string(),
        HourStyle::H24Padded => pad2(i32::from(hour)),
        HourStyle::H12 => twelve_hour(hour).to_string(),
        HourStyle::H12Padded => pad2(i32::from(twelve_hour(hour))),
    }
}

fn render_min_sec(value: u8, style: MinSecStyle) -> String {
    match style {
        MinSecStyle::Padded => pad2(i32::from(value)),
        MinSecStyle::Numeric => value.to_string(),
    }
}

/// Midnight and noon both render as 12 on a 12-hour clock.
const fn twelve_hour(hour: u8) -> u8 {
    match hour % 12 {
        0 => 12,
        other => other,
    }
}

fn pad2(value: i32) -> String {
    format!("{value:02}")
}

// --- One-shot string formatters ------------------------------------------
//
// Each builds its formatter fresh per call rather than caching it in a static.
// They stay pure functions of their argument, which is the same no-hidden-state
// stance that makes the clock injectable. The per-call build is a handful of
// `Vec` pushes.

/// `2026-02-17` — the ISO calendar date, UTC.
#[must_use]
pub fn format_iso_date(epoch: Epoch) -> String {
    DateFormatBuilder::iso_date().format(epoch)
}

/// `2026-02-17 14:30:45 UTC` — a human-facing timestamp, seconds precision.
#[must_use]
pub fn format_iso_timestamp(epoch: Epoch) -> String {
    DateFormatBuilder::new()
        .year_styled(YearStyle::Padded4)
        .literal("-")
        .month_styled(MonthStyle::Padded)
        .literal("-")
        .day_styled(DayStyle::Padded)
        .literal(" ")
        .hour_styled(HourStyle::H24Padded)
        .literal(":")
        .minute()
        .literal(":")
        .second()
        .literal(" UTC")
        .build()
        .format(epoch)
}

/// `2026.12.0` — an ISO week-version string: ISO year, zero-padded ISO week,
/// and a fixed `.0` patch revision.
#[must_use]
pub fn format_iso_week_version(epoch: Epoch) -> String {
    let week = iso_week_of(&epoch_to_components(epoch));
    format!("{}.{}.0", week.year, pad2(i32::from(week.week)))
}
