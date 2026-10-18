//! Percent-encoding (RFC 3986 §2.1) and its strict inverse.

use crate::hex::decode_hex;

/// RFC 3986 §2.3 unreserved set: ALPHA / DIGIT / `-` / `.` / `_` / `~`.
const fn is_url_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

/// The shared loop behind both encoders.
///
/// Unreserved bytes pass through; everything else becomes `%HH` with uppercase
/// hex, which RFC 3986 §2.1 says producers should emit. `space_as_plus` selects
/// the `application/x-www-form-urlencoded` convention.
fn encode(input: &[u8], space_as_plus: bool) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";

    let mut result = String::with_capacity(input.len());
    for &byte in input {
        if is_url_unreserved(byte) {
            result.push(byte as char);
        } else if space_as_plus && byte == b' ' {
            result.push('+');
        } else {
            result.push('%');
            result.push(DIGITS[usize::from(byte >> 4)] as char);
            result.push(DIGITS[usize::from(byte & 0x0F)] as char);
        }
    }
    result
}

/// Percent-encodes bytes for a URL or query (RFC 3986 §2.1; space becomes `%20`).
///
/// The canonical percent-encoder: reach for this rather than a hand-rolled
/// formatting loop. Empty input gives an empty string.
#[must_use]
pub fn percent_encode(input: &[u8]) -> String {
    encode(input, false)
}

/// The `application/x-www-form-urlencoded` variant.
///
/// Identical to [`percent_encode`] except that a space becomes `+` rather than
/// `%20`. A literal `+` still escapes to `%2B`, so the two are never confused.
#[must_use]
pub fn form_urlencode(input: &[u8]) -> String {
    encode(input, true)
}

/// The inverse of [`percent_encode`]: each `%HH` becomes the byte it names.
///
/// Strict rather than best-effort: a `%` not followed by two hex digits yields
/// `None`. A caller of a decoder is *deciding* something about the decoded bytes
/// — which key a connection string names, say — and a decoder that guesses hands
/// that decision bytes the real consumer never saw.
///
/// `+` decodes to a literal `+`. That is RFC 3986's rule; space-for-plus belongs
/// to [`form_urldecode`] alone, and folding the two conventions into one decoder
/// would make it disagree with whichever caller is right.
#[must_use]
pub fn percent_decode(input: &str) -> Option<Vec<u8>> {
    let encoded = input.as_bytes();
    let mut decoded = Vec::with_capacity(encoded.len());

    let mut index = 0;
    while index < encoded.len() {
        if encoded[index] != b'%' {
            decoded.push(encoded[index]);
            index += 1;
            continue;
        }

        let escape = encoded.get(index + 1..index + 3)?;
        // The digits go through the hex decoder, which accepts either case: a
        // decoder identifies bytes, so both spellings name the same one.
        let byte = decode_hex(std::str::from_utf8(escape).ok()?)?;
        decoded.extend_from_slice(&byte);
        index += 3;
    }
    Some(decoded)
}

/// The inverse of [`form_urlencode`] (`application/x-www-form-urlencoded`): `+`
/// is a space, then every `%HH` is the byte it names — strict, as
/// [`percent_decode`] is.
#[must_use]
pub fn form_urldecode(input: &str) -> Option<Vec<u8>> {
    percent_decode(&input.replace('+', " "))
}
