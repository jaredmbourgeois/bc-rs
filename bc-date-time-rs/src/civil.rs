//! Proleptic-Gregorian day-number arithmetic — the private engine under every
//! conversion in this crate.
//!
//! The C++ original reaches for C++20 `<chrono>`'s calendar types
//! (`year_month_day`, `sys_days`, `weekday`). Those types *are* Howard
//! Hinnant's `days_from_civil` / `civil_from_days` algorithms, standardized;
//! Rust's std has no calendar, so the algorithms are written out here rather
//! than taken as a dependency. That keeps the crate dependency-free, which is
//! the whole reason it exists (see the comment in `Cargo.toml`).
//!
//! Everything works in **days since 1970-01-01**, signed, with eras starting in
//! March so the leap day lands at the end of a year and needs no special case.

/// Days from 1970-01-01 to the given proleptic-Gregorian date.
///
/// Accepts out-of-range month and day values and normalizes them the same way
/// `<chrono>`'s `sys_days` arithmetic does — March 0 is the last day of
/// February, month 13 is next January. Callers that must reject an impossible
/// date (the parsers do) range-check *before* calling.
pub(crate) const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // Shift the year so it starts in March: February's leap day becomes the
    // last day of the year, which is what removes the special case.
    let adjusted_year = if month <= 2 { year - 1 } else { year };
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let month_of_era = if month > 2 { month - 3 } else { month + 9 };
    // (153 * m + 2) / 5 is the exact count of days in the months before `m` in
    // a March-started year — the classic closed form for the 31/30 pattern.
    let day_of_year = (153 * month_of_era + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    // 146_097 days per 400-year era; 719_468 shifts the origin from
    // 0000-03-01 to 1970-01-01.
    era * 146_097 + day_of_era - 719_468
}

/// The inverse of [`days_from_civil`]: `(year, month, day)` for a day number.
pub(crate) const fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    // Undo the March-start shift: indices 0..9 are March..December, 10 and 11
    // are January and February of the following calendar year.
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Day of week for a day number, in `<chrono>`'s encoding: Sunday = 0 through
/// Saturday = 6.
///
/// Day 0 is 1970-01-01, a Thursday, which is index 4 — that constant is the
/// whole calibration.
pub(crate) const fn weekday_index_from_days(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unix_epoch_is_day_zero_and_a_thursday() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // Sunday = 0, so Thursday = 4.
        assert_eq!(weekday_index_from_days(0), 4);
    }

    #[test]
    fn conversions_round_trip_across_eras() {
        // Every ~1.9 years across roughly 800 years, straddling the year-1970
        // origin so the negative branch of every div_euclid is exercised.
        let mut days = -150_000_i64;
        while days < 150_000 {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(
                days_from_civil(year, month, day),
                days,
                "round-trip failed at day {days} ({year:04}-{month:02}-{day:02})"
            );
            days += 701; // deliberately coprime with 7, 365, and 146_097
        }
    }

    #[test]
    fn weekdays_advance_by_one_per_day_without_a_seam_at_the_epoch() {
        for days in -400_i64..400 {
            let today = weekday_index_from_days(days);
            let tomorrow = weekday_index_from_days(days + 1);
            assert_eq!(tomorrow, (today + 1) % 7, "weekday seam at day {days}");
        }
    }

    #[test]
    fn leap_days_land_where_the_gregorian_rules_put_them() {
        // 2024 is a leap year (divisible by 4, not by 100).
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        // 2000 is a leap year (divisible by 400).
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
        // 1900 is not (divisible by 100, not by 400): Feb 29 normalizes to Mar 1.
        assert_eq!(civil_from_days(days_from_civil(1900, 2, 29)), (1900, 3, 1));
    }
}
