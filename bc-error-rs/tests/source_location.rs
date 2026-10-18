use std::panic::Location;

use bc_error::{
    LOCATION_SEPARATOR, format_location, format_location_from, format_location_from_with_separator,
    format_location_with_separator,
};

#[test]
fn format_location_captures_callsite_with_file_line_and_column() {
    let expected_line = line!() + 1;
    let formatted = format_location();

    assert!(formatted.contains("source_location.rs"));
    assert!(formatted.contains(LOCATION_SEPARATOR));
    assert!(formatted.contains(&format!("#{expected_line}:")));
}

#[test]
fn format_location_accepts_a_custom_separator() {
    let expected_line = line!() + 1;
    let formatted = format_location_with_separator(" :: ");

    assert!(formatted.contains("source_location.rs"));
    assert!(formatted.contains(" :: "));
    assert!(formatted.contains(&format!("#{expected_line}:")));
}

#[test]
fn explicit_location_formatting_uses_the_supplied_location() {
    let location = Location::caller();

    assert_eq!(
        format_location_from(location),
        format!(
            "{}{}#{}:{}",
            location.file(),
            LOCATION_SEPARATOR,
            location.line(),
            location.column()
        )
    );
    assert_eq!(
        format_location_from_with_separator(location, " :: "),
        format!(
            "{} :: #{}:{}",
            location.file(),
            location.line(),
            location.column()
        )
    );
}

#[test]
fn location_separator_constant_matches_source() {
    assert_eq!(LOCATION_SEPARATOR, " > ");
}
