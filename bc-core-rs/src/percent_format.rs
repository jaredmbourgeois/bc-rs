//! Basis points as a percent string.

/// Renders basis points as a percent string, dropping insignificant zeros.
///
/// 100 basis points is one percent, so `1500` is `"15%"`, `1550` is `"15.5%"`,
/// and `1505` is `"15.05%"`. The canonical basis-points-to-percent primitive, so
/// one margin cannot render two ways on two surfaces.
///
/// Sign-correct: the minus leads, once. The implementation this replaced
/// interleaved it into the fraction and rendered `-1550` as `"-15.0-5%"`.
#[must_use]
pub fn basis_points_to_percent_string(basis_points: i64) -> String {
    // The magnitude is taken in `u64` so that `i64::MIN`, whose negation does not
    // fit an `i64`, needs no special case.
    let negative = basis_points < 0;
    let magnitude = basis_points.unsigned_abs();

    let mut rendered = String::new();
    if negative {
        rendered.push('-');
    }
    rendered.push_str(&(magnitude / 100).to_string());

    let hundredths = magnitude % 100;
    if hundredths != 0 {
        rendered.push('.');
        rendered.push_str(format!("{hundredths:02}").trim_end_matches('0'));
    }
    rendered.push('%');
    rendered
}
