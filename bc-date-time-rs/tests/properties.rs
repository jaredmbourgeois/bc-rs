//! Seeded property tests over named invariants.
//!
//! This family declines coverage-guided fuzzing: cargo-fuzz needs nightly
//! while every family crate pins its MSRV, so a fuzz target can be built but
//! never made a CI gate. These are the replacement — on stable, on every push,
//! and run under both the debug and release profiles because the arithmetic
//! below saturates differently if that ever changes.
//!
//! Seeds are fixed, so a failure reproduces exactly. Each test states the
//! invariant it is checking in its name.

use bc_date_time::{
    DateComponents, DateFormatBuilder, Epoch, EpochDelta, EpochRange, Month, components_to_epoch,
    days_in_month, epoch_to_components, format_iso_date, iso_week_of, parse_iso_date,
    parse_iso_timestamp,
};

/// xorshift64*, ~20 lines and no dependency.
///
/// Family crates deliberately hand-roll this rather than growing a test
/// dependency on a shared testkit — the dependency guard admits nothing, and a
/// shared test crate would be the first crack in that.
struct SeededRng {
    state: u64,
}

impl SeededRng {
    fn new(seed: u64) -> Self {
        // A zero state is a fixed point of xorshift; any nonzero seed is fine.
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A signed epoch in milliseconds, spread across roughly ±3000 years so
    /// every sample straddles the 1970 origin rather than clustering after it.
    fn next_epoch(&mut self) -> Epoch {
        const RANGE_MILLIS: i64 = 3_000 * 365 * 86_400_000;
        let magnitude = i64::try_from(self.next_u64() % u64::try_from(RANGE_MILLIS).expect("fits"))
            .expect("in range");
        if self.next_u64() % 2 == 0 {
            Epoch::from_millis(magnitude)
        } else {
            Epoch::from_millis(-magnitude)
        }
    }
}

/// How many samples each property draws. Large enough to walk every month and
/// weekday many times over, small enough that the suite stays instant.
const SAMPLES: usize = 20_000;

#[test]
fn an_epoch_round_trips_through_its_components() {
    let mut rng = SeededRng::new(0x0DA7_E71E_0001);
    for _ in 0..SAMPLES {
        let epoch = rng.next_epoch();
        let components = epoch_to_components(epoch);
        assert_eq!(
            components_to_epoch(&components),
            epoch,
            "{} decomposed to {components:?} and did not compose back",
            epoch.millis()
        );
    }
}

#[test]
fn components_round_trip_through_an_epoch() {
    let mut rng = SeededRng::new(0x0DA7_E71E_0002);
    for _ in 0..SAMPLES {
        let components = epoch_to_components(rng.next_epoch());
        assert_eq!(
            epoch_to_components(components_to_epoch(&components)),
            components
        );
    }
}

#[test]
fn every_decomposition_is_in_range_and_self_consistent() {
    let mut rng = SeededRng::new(0x0DA7_E71E_0003);
    for _ in 0..SAMPLES {
        let epoch = rng.next_epoch();
        let components = epoch_to_components(epoch);
        assert!(
            (1..=days_in_month(components.year, components.month)).contains(&components.day),
            "{components:?}"
        );
        assert!(components.hour <= 23, "{components:?}");
        assert!(components.minute <= 59, "{components:?}");
        assert!(components.second <= 59, "{components:?}");
        assert!(components.millisecond <= 999, "{components:?}");
        // `DateComponents::new` derives the weekday independently of the
        // decomposition path, so agreement between them is a real check.
        let rebuilt = DateComponents::new(
            components.year,
            components.month,
            components.day,
            components.hour,
            components.minute,
            components.second,
            components.millisecond,
        );
        assert_eq!(
            rebuilt, components,
            "weekday disagreement at {components:?}"
        );
    }
}

#[test]
fn the_iso8601_formatter_and_the_timestamp_parser_are_inverses() {
    let mut rng = SeededRng::new(0x0DA7_E71E_0004);
    let formatter = DateFormatBuilder::iso8601();
    for _ in 0..SAMPLES {
        // The wire format has no expanded-year syntax, so restrict to the four
        // digit years it can represent — the domain the format is defined on.
        let epoch = four_digit_year_epoch(&mut rng);
        let rendered = formatter.format(epoch);
        assert_eq!(
            parse_iso_timestamp(&rendered),
            Some(epoch),
            "{rendered} did not parse back"
        );
    }
}

#[test]
fn the_date_formatter_and_the_date_parser_agree_on_midnight() {
    let mut rng = SeededRng::new(0x0DA7_E71E_0005);
    for _ in 0..SAMPLES {
        let epoch = four_digit_year_epoch(&mut rng);
        let midnight = Epoch::from_millis(epoch.millis().div_euclid(86_400_000) * 86_400_000);
        assert_eq!(parse_iso_date(&format_iso_date(epoch)), Some(midnight));
    }
}

#[test]
fn no_mutation_of_a_valid_timestamp_ever_parses_as_a_different_instant() {
    // The strict-parse invariant: a parser that is lenient about *one* byte is
    // a parser that silently accepts a different instant than the text names.
    let mut rng = SeededRng::new(0x0DA7_E71E_0006);
    let formatter = DateFormatBuilder::iso8601();
    // One replacement per byte, the first a space: ' ', '-', '+', 'T', 'Z', ':', '.', '/', 'x', '0'.
    let replacements = *b" -+TZ:./x0";
    for _ in 0..2_000 {
        let epoch = four_digit_year_epoch(&mut rng);
        let rendered = formatter.format(epoch);
        let bytes = rendered.as_bytes();
        for index in 0..bytes.len() {
            for replacement in replacements {
                if bytes[index] == replacement {
                    continue;
                }
                let mut mutated = bytes.to_vec();
                mutated[index] = replacement;
                let Ok(text) = std::str::from_utf8(&mutated) else {
                    continue;
                };
                match parse_iso_timestamp(text) {
                    // Refusal is always fine.
                    None => {}
                    // Accepting is fine only if the text really does name that
                    // instant: the fields it spells, canonicalized as text,
                    // must be the fields the returned instant renders as.
                    Some(parsed) => {
                        let named = canonical_iso8601(text).unwrap_or_else(|| {
                            panic!(
                                "{text} is an accepted spelling this test cannot canonicalize \
                                 as text — a `60` second normalizes into the next minute. \
                                 The replacement set cannot currently produce one; widening \
                                 it means giving this match a second arm."
                            )
                        });
                        assert_eq!(
                            formatter.format(parsed),
                            named,
                            "{text} parsed to an instant it does not name"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn appending_an_offset_suffix_never_parses() {
    let mut rng = SeededRng::new(0x0DA7_E71E_0007);
    let formatter = DateFormatBuilder::iso8601();
    for _ in 0..SAMPLES {
        let rendered = formatter.format(four_digit_year_epoch(&mut rng));
        let without_z = rendered.trim_end_matches('Z');
        for suffix in ["+00:00", "-05:00", "+0000", "UTC", " ", "z", "ZZ"] {
            let text = format!("{without_z}{suffix}");
            assert_eq!(parse_iso_timestamp(&text), None, "{text} should not parse");
        }
    }
}

#[test]
fn a_range_contains_exactly_the_instants_between_its_bounds() {
    let mut rng = SeededRng::new(0x0DA7_E71E_0008);
    for _ in 0..SAMPLES {
        let first = rng.next_epoch();
        let second = rng.next_epoch();
        let (start, end) = if first <= second {
            (first, second)
        } else {
            (second, first)
        };
        let range = EpochRange::new(start, end);
        assert_eq!(range.duration(), end - start);
        assert!(!range.contains(end), "end must be exclusive");
        if start < end {
            assert!(range.contains(start), "start must be inclusive");
            assert!(!range.contains(start - EpochDelta::from_millis(1)));
        } else {
            // An empty range contains nothing at all, its start included.
            assert!(!range.contains(start));
        }
    }
}

#[test]
fn epoch_ordering_matches_calendar_ordering() {
    let mut rng = SeededRng::new(0x0DA7_E71E_0009);
    for _ in 0..SAMPLES {
        let earlier = rng.next_epoch();
        let span = EpochDelta::from_millis(
            i64::try_from(rng.next_u64() % 1_000_000_000).expect("under a billion milliseconds"),
        );
        let later = earlier + span;
        assert!(later >= earlier);
        assert_eq!(later.delta_since(earlier), span);
        let (a, b) = (epoch_to_components(earlier), epoch_to_components(later));
        assert!(
            (a.year, a.month.number(), a.day) <= (b.year, b.month.number(), b.day),
            "{a:?} should not sort after {b:?}"
        );
    }
}

#[test]
fn the_iso_week_never_leaves_its_declared_range() {
    let mut rng = SeededRng::new(0x0DA7_E71E_000A);
    for _ in 0..SAMPLES {
        let components = epoch_to_components(rng.next_epoch());
        let week = iso_week_of(&components);
        assert!((1..=53).contains(&week.week), "{components:?} -> {week:?}");
        assert!(
            (week.year - components.year).abs() <= 1,
            "{components:?} -> {week:?}"
        );
    }
}

#[test]
fn truncating_to_seconds_never_moves_an_instant_forward() {
    let mut rng = SeededRng::new(0x0DA7_E71E_000B);
    for _ in 0..SAMPLES {
        let epoch = rng.next_epoch();
        let truncated = epoch.truncated_to_seconds();
        assert!(truncated <= epoch, "{} moved forward", epoch.millis());
        assert!(
            epoch.delta_since(truncated) < EpochDelta::from_seconds(1),
            "{} lost more than a second",
            epoch.millis()
        );
        assert_eq!(truncated.millis() % 1_000, 0);
        assert_eq!(truncated.truncated_to_seconds(), truncated);
    }
}

/// A random epoch restricted to years 1000..=9999.
///
/// The ISO 8601 wire syntax has a four-digit year field and no expanded-year
/// form, so this is the domain the format/parse pair is defined on. Years
/// outside it are still representable as an `Epoch`; they just have no ISO 8601
/// spelling, which the README's "Behaviour worth knowing" says.
fn four_digit_year_epoch(rng: &mut SeededRng) -> Epoch {
    let year = 1_000 + i32::try_from(rng.next_u64() % 9_000).expect("in range");
    let month = Month::ALL[usize::try_from(rng.next_u64() % 12).expect("in range")];
    let day =
        1 + u8::try_from(rng.next_u64() % u64::from(days_in_month(year, month))).expect("under 31");
    let hour = u8::try_from(rng.next_u64() % 24).expect("under 24");
    let minute = u8::try_from(rng.next_u64() % 60).expect("under 60");
    let second = u8::try_from(rng.next_u64() % 60).expect("under 60");
    let milli = u16::try_from(rng.next_u64() % 1_000).expect("under 1000");
    components_to_epoch(&DateComponents::new(
        year, month, day, hour, minute, second, milli,
    ))
}

/// The canonical `iso8601()` spelling of already-accepted timestamp text,
/// derived from the text and nothing else.
///
/// A mutation may legitimately produce a *different but valid* timestamp (say
/// changing a digit), or the same instant spelled differently (dropping the
/// trailing `Z`). Both are correct acceptances; the defect this exists to catch
/// is accepting text whose fields do not match the instant returned.
///
/// Which is why it reads the string itself instead of calling
/// `format(parse(text))`. That pair is the thing under test, and a checker
/// assembled out of it agrees with itself for any parser at all — including one
/// that read `2026-x2-17` as February. Fixed offsets are safe to hardcode here
/// because the grammar `parse_iso_timestamp` accepts is fixed-width up to the
/// fraction; a canonicalizer that re-derived the field positions would be
/// re-deriving the parser, which is the same mistake one layer down.
///
/// `None` for the one accepted spelling that is not a re-spacing of its own
/// text: a `60` second is legal ISO 8601 and normalizes into second 0 of the
/// next minute, which is calendar arithmetic rather than string work.
fn canonical_iso8601(text: &str) -> Option<String> {
    let year = text.get(0..4)?;
    let month = text.get(5..7)?;
    let day = text.get(8..10)?;
    let hour = text.get(11..13)?;
    let minute = text.get(14..16)?;
    let second = text.get(17..19)?;
    if second == "60" {
        return None;
    }

    // ".5" is 500 milliseconds and ".1234" truncates: right-pad, then cut.
    let mut fraction = match text.get(19..) {
        Some(rest) if rest.starts_with('.') => rest[1..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .map(char::from)
            .collect(),
        _ => String::new(),
    };
    while fraction.len() < 3 {
        fraction.push('0');
    }
    let fraction = &fraction[..3];

    Some(format!(
        "{year}-{month}-{day}T{hour}:{minute}:{second}.{fraction}Z"
    ))
}
