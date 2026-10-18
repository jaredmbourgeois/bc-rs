//! Human "N ago" formatting.
//!
//! Ported from `libs/foundation/date_time/src/relative_time.hpp`. Anything
//! older than a week falls back to an absolute timestamp, because "37 days ago"
//! is less useful to a reader than the date itself.

use crate::formatter::DateFormatBuilder;
use crate::time::Epoch;

/// Renders how long before `now` the instant `event` was, in words.
///
/// The cascade, in order: anything under a minute (including a future instant)
/// is `"just now"`; then minutes, hours, and days, singular and plural; from a
/// week out it renders the absolute `2026-02-17 14:30` form instead.
///
/// A future `event` reads as `"just now"` rather than a negative duration —
/// the C++ behavior, and the right one for a display string, since a clock skew
/// between two machines should not render as `"-3 minutes ago"`.
///
/// ```
/// use bc_date_time::{Epoch, EpochDelta, format_time_ago};
///
/// let now = Epoch::from_millis(1_771_338_645_123);
/// assert_eq!(format_time_ago(now - EpochDelta::from_minutes(1), now), "1 minute ago");
/// assert_eq!(format_time_ago(now - EpochDelta::from_hours(5), now), "5 hours ago");
/// assert_eq!(format_time_ago(now + EpochDelta::from_hours(1), now), "just now");
/// ```
#[must_use]
pub fn format_time_ago(event: Epoch, now: Epoch) -> String {
    let elapsed = now.delta_since(event);
    if elapsed.millis() < 0 {
        return "just now".to_owned();
    }

    let minutes = elapsed.millis() / 60_000;
    let hours = minutes / 60;
    let days = hours / 24;

    match (minutes, hours, days) {
        (0, _, _) => "just now".to_owned(),
        (1, _, _) => "1 minute ago".to_owned(),
        (minutes, _, _) if minutes < 60 => format!("{minutes} minutes ago"),
        (_, 1, _) => "1 hour ago".to_owned(),
        (_, hours, _) if hours < 24 => format!("{hours} hours ago"),
        (_, _, 1) => "1 day ago".to_owned(),
        (_, _, days) if days < 7 => format!("{days} days ago"),
        _ => DateFormatBuilder::iso_datetime_minute().format(event),
    }
}
