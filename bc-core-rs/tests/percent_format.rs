//! Basis-point formatting, against
//! `libs/foundation/core/test/percent_format_test.cpp`.
//!
//! Each test is named for the property it asserts rather than for the C++ case
//! it came from. The tests account for each of the original's 5 cases.

use bc_core::basis_points_to_percent_string;

#[test]
fn a_whole_percent_carries_no_fraction() {
    assert_eq!(basis_points_to_percent_string(0), "0%");
    assert_eq!(basis_points_to_percent_string(1500), "15%");
    assert_eq!(basis_points_to_percent_string(10_000), "100%");
}

#[test]
fn a_tenth_drops_its_trailing_zero() {
    assert_eq!(basis_points_to_percent_string(1550), "15.5%");
    assert_eq!(basis_points_to_percent_string(50), "0.5%");
}

#[test]
fn a_hundredth_keeps_both_digits() {
    assert_eq!(basis_points_to_percent_string(1505), "15.05%");
    assert_eq!(basis_points_to_percent_string(1), "0.01%");
    assert_eq!(basis_points_to_percent_string(1234), "12.34%");
}

#[test]
fn a_negative_carries_exactly_one_leading_minus() {
    // The implementation this replaced interleaved the minus into the fraction
    // and rendered -1550 as "-15.0-5%". Negative margins are real.
    assert_eq!(basis_points_to_percent_string(-1550), "-15.5%");
    assert_eq!(basis_points_to_percent_string(-1505), "-15.05%");
    assert_eq!(basis_points_to_percent_string(-1500), "-15%");
    assert_eq!(basis_points_to_percent_string(-1), "-0.01%");
}

#[test]
fn the_most_negative_value_does_not_overflow() {
    // Its negation does not fit an i64, which is why the magnitude is unsigned.
    assert_eq!(
        basis_points_to_percent_string(i64::MIN),
        "-92233720368547758.08%"
    );
}
