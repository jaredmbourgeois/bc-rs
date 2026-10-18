//! Weekday-only date arithmetic.
//!
//! Ported from `libs/foundation/date_time/src/business_days.hpp`. Its original
//! caller translates a "ships in N business days" promise into a calendar date
//! a customer sees on a receipt, which is why the result may never land on a
//! weekend — outside the saturation clamp documented on
//! [`add_business_days`], which trades that invariant for termination.

use crate::time::{Epoch, EpochDelta, epoch_to_components};

/// Advances `start` by `days` business days, skipping Saturdays and Sundays.
///
/// The result keeps `start`'s time of day — it is the same clock time, some
/// number of weekdays later — and never lands on a weekend, with the one
/// exception spelled out under [Range](#range): at the end of the representable
/// range the instant stops advancing and is returned as-is, and that saturated
/// instant is a Sunday. A zero or negative `days` returns `start` unchanged.
///
/// Weekends are the only skipped days: there is no holiday calendar, because
/// holidays are jurisdictional and this crate has no locale.
///
/// ```
/// use bc_date_time::{Epoch, add_business_days, epoch_to_components, Weekday};
///
/// // 2026-05-15 is a Friday; one business day later is Monday the 18th.
/// let friday = Epoch::from_millis(1_778_889_600_000);
/// let next = add_business_days(friday, 1);
/// assert_eq!(epoch_to_components(next).weekday, Weekday::Monday);
/// ```
/// # Range
///
/// Clamps rather than wrapping or hanging at the end of the representable
/// range: an instant that can no longer advance is returned as-is. The C++
/// steps one calendar day per business day, which at the saturation point
/// spins forever — the saturated instant is a Sunday, so its weekend check
/// never lets the counter fall.
#[must_use]
pub fn add_business_days(start: Epoch, days: i32) -> Epoch {
    if days <= 0 {
        return start;
    }

    // Step onto the first business day strictly after `start`. That consumes
    // one of the requested days and — the point — leaves us on a weekday,
    // which is the precondition the week arithmetic below needs.
    let Some(first) = next_business_day(start) else {
        return start;
    };
    let mut epoch = first;
    let mut remaining = i64::from(days) - 1;

    // From a weekday, five business days are *exactly* seven calendar days, so
    // the bulk of any span is one multiplication instead of one iteration per
    // day. This shortcut is only valid because of the normalisation above:
    // from a Saturday it would be wrong (Saturday plus five business days is
    // the following Friday, not the following Saturday), which is what
    // `the_fast_path_agrees_with_the_naive_walk_on_every_start_weekday` pins.
    let whole_weeks = remaining / 5;
    remaining %= 5;
    if whole_weeks > 0 {
        let stepped = epoch + EpochDelta::from_days(whole_weeks.saturating_mul(7));
        if stepped == epoch {
            return epoch;
        }
        epoch = stepped;
    }

    // At most four business days are left.
    while remaining > 0 {
        let Some(next) = next_business_day(epoch) else {
            return epoch;
        };
        epoch = next;
        remaining -= 1;
    }
    epoch
}

/// The first business day strictly after `epoch`, or `None` when the instant
/// can no longer advance because it has saturated.
fn next_business_day(epoch: Epoch) -> Option<Epoch> {
    let mut candidate = epoch;
    // Three steps is the worst case: from a Friday, Saturday and Sunday are
    // both skipped. The bound is what makes this unable to spin.
    for _ in 0..3 {
        let stepped = candidate + EpochDelta::from_days(1);
        if stepped == candidate {
            return None;
        }
        candidate = stepped;
        if !epoch_to_components(candidate).weekday.is_weekend() {
            return Some(candidate);
        }
    }
    // Unreachable: no run of three consecutive days is entirely weekend.
    debug_assert!(false, "three consecutive weekend days");
    None
}
