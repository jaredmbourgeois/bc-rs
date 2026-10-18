//! Hex encoding, strict decoding, and the identifier forms built on them.

/// The short-SHA width: the first eight digits, as a table cell or log line wants
/// a commit or digest.
///
/// C++ carries this as `abbreviate_hex`'s default argument. Rust has no default
/// arguments, so it is a constant the call site names instead — which also puts
/// the two real widths (this and the release-label 12) side by side.
pub const SHORT_HEX_WIDTH: usize = 8;

/// The value of ONE hexadecimal digit — `0`–`9` → 0–9, `a`–`f` and `A`–`F` →
/// 10–15 — or `None` for any other byte.
///
/// The canonical per-character hex primitive: the whole-string decoders below
/// are built on it, and a decoder that must say *where* a bad digit sits calls
/// it digit by digit instead of hand-rolling a `digit - b'a' + 10` table.
/// Accepts both cases because it identifies a value, not a spelling. A byte
/// rather than a `char`, because every hex digit is one ASCII byte.
#[must_use]
pub const fn hex_digit_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

/// Lowercase hex for a byte slice — each byte becomes two digits.
///
/// The canonical bytes-to-hex primitive: reach for this rather than a nibble
/// loop, a digit table, or a formatting call. Pure and infallible; empty input
/// gives an empty string.
#[must_use]
pub fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";

    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(DIGITS[usize::from(byte >> 4)] as char);
        result.push(DIGITS[usize::from(byte & 0x0F)] as char);
    }
    result
}

/// Decodes a whole hex string, or refuses it.
///
/// Accepts either case; rejects an odd length and any non-hex character. Strict
/// rather than best-effort, because a caller of a decoder is deciding something
/// about the bytes, and a decoder that guesses hands that decision bytes the
/// real consumer never saw.
#[must_use]
pub fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    let digits = hex.as_bytes();
    if digits.len() % 2 != 0 {
        return None;
    }

    let mut bytes = Vec::with_capacity(digits.len() / 2);
    for pair in digits.chunks_exact(2) {
        let high = hex_digit_value(pair[0])?;
        let low = hex_digit_value(pair[1])?;
        bytes.push((high << 4) | low);
    }
    Some(bytes)
}

/// Decodes into a caller's buffer, which must be exactly the right length.
///
/// Returns whether it decoded. `out` is left untouched on either refusal — a
/// wrong length or a bad digit — so a failed decode cannot leave a caller
/// holding half a value.
#[must_use]
pub fn decode_hex_into(hex: &str, out: &mut [u8]) -> bool {
    if hex.len() / 2 != out.len() {
        return false;
    }
    let Some(bytes) = decode_hex(hex) else {
        return false;
    };
    out.copy_from_slice(&bytes);
    true
}

/// Whether this is a hex identifier of exactly `length` lowercase digits.
///
/// The validating counterpart to [`abbreviate_hex`], for deciding whether a
/// caller-supplied string *is* a commit SHA (40) or a digest (64) before
/// trusting it as one.
///
/// Lowercase only, deliberately: these identifiers are produced by
/// [`encode_hex`], so an uppercase spelling is a different string for the same
/// bytes — and one that is compared as a string must have exactly one spelling.
/// That is why this is stricter than [`decode_hex`], which identifies bytes.
#[must_use]
pub fn is_lowercase_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|digit| digit.is_ascii_digit() || (b'a'..=b'f').contains(&digit))
}

/// The first `width` characters of a hex identifier, for display.
///
/// The canonical "shorten a SHA for a table cell, log line, or branch label"
/// primitive. Returns the whole string when it is already that short, and
/// borrows rather than allocating. It does **not** check that the input is hex —
/// [`is_lowercase_hex`] is the function that decides that.
#[must_use]
pub fn abbreviate_hex(hex: &str, width: usize) -> &str {
    match hex.char_indices().nth(width) {
        Some((end, _)) => &hex[..end],
        None => hex,
    }
}
