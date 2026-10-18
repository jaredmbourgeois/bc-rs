use std::panic::Location;

/// The separator used by [`format_location`].
pub const LOCATION_SEPARATOR: &str = " > ";

/// Formats the caller's location using [`LOCATION_SEPARATOR`].
#[must_use]
#[track_caller]
pub fn format_location() -> String {
    format_location_from(Location::caller())
}

/// Formats the caller's location with an explicit separator.
#[must_use]
#[track_caller]
pub fn format_location_with_separator(separator: &str) -> String {
    format_location_from_with_separator(Location::caller(), separator)
}

/// Formats an explicit location using [`LOCATION_SEPARATOR`].
#[must_use]
pub fn format_location_from(location: &Location<'_>) -> String {
    format_location_from_with_separator(location, LOCATION_SEPARATOR)
}

/// Formats an explicit location as `<file><separator>#<line>:<column>`.
#[must_use]
pub fn format_location_from_with_separator(location: &Location<'_>, separator: &str) -> String {
    format!(
        "{}{separator}#{}:{}",
        location.file(),
        location.line(),
        location.column()
    )
}
