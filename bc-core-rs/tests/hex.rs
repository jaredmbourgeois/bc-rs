//! Hex encoding and decoding, against `libs/foundation/core/test/hex_test.cpp`.
//!
//! Each test is named for the property it asserts rather than for the C++ case
//! it came from. The tests account for each of the original's 23 cases.

use bc_core::{
    SHORT_HEX_WIDTH, abbreviate_hex, decode_hex, decode_hex_into, encode_hex, hex_digit_value,
    is_lowercase_hex,
};

/// Every byte value, in order — the input several cases below are built from.
fn every_byte() -> Vec<u8> {
    (0..=u8::MAX).collect()
}

// --- Encoding ---

#[test]
fn encoding_nothing_gives_an_empty_string() {
    assert_eq!(encode_hex(&[]), "");
}

#[test]
fn a_known_vector_encodes_digit_for_digit() {
    assert_eq!(
        encode_hex(&[0x00, 0x01, 0x0F, 0x10, 0xAB, 0xFF]),
        "00010f10abff"
    );
}

#[test]
fn encoding_is_lowercase() {
    assert_eq!(encode_hex(&[0xAB, 0xCD, 0xEF]), "abcdef");
}

#[test]
fn every_byte_becomes_exactly_two_lowercase_digits() {
    let encoded = encode_hex(&every_byte());

    assert_eq!(encoded.len(), 512);
    assert_eq!(&encoded[..4], "0001");
    assert_eq!(&encoded[508..], "feff");
    assert!(
        encoded
            .bytes()
            .all(|digit| digit.is_ascii_digit() || (b'a'..=b'f').contains(&digit))
    );
}

// --- Decoding ---

#[test]
fn decoding_an_empty_string_gives_no_bytes() {
    assert_eq!(decode_hex(""), Some(Vec::new()));
}

#[test]
fn a_known_lowercase_vector_decodes_byte_for_byte() {
    assert_eq!(
        decode_hex("00010f10abff"),
        Some(vec![0x00, 0x01, 0x0F, 0x10, 0xAB, 0xFF])
    );
}

#[test]
fn an_uppercase_vector_decodes_to_the_same_bytes() {
    // A decoder identifies bytes, not strings, so both spellings name one value.
    assert_eq!(
        decode_hex("00010F10ABFF"),
        Some(vec![0x00, 0x01, 0x0F, 0x10, 0xAB, 0xFF])
    );
}

#[test]
fn every_byte_survives_a_round_trip() {
    let bytes = every_byte();
    assert_eq!(decode_hex(&encode_hex(&bytes)), Some(bytes));
}

#[test]
fn an_odd_number_of_digits_is_refused() {
    assert_eq!(decode_hex("0"), None);
    assert_eq!(decode_hex("abc"), None);
}

#[test]
fn a_non_hex_character_is_refused() {
    assert_eq!(decode_hex("00xz"), None);
    assert_eq!(decode_hex("00 1"), None);
}

#[test]
fn decoding_into_a_buffer_of_the_wrong_length_leaves_it_untouched() {
    let mut output = [0xAA, 0xBB];
    assert!(!decode_hex_into("00", &mut output));
    assert_eq!(output, [0xAA, 0xBB]);
}

#[test]
fn decoding_a_bad_digit_into_a_buffer_leaves_it_untouched() {
    let mut output = [0xAA, 0xBB];
    assert!(!decode_hex_into("00xz", &mut output));
    assert_eq!(output, [0xAA, 0xBB]);
}

#[test]
fn decoding_into_an_exactly_sized_buffer_writes_it() {
    let mut output = [0; 3];
    assert!(decode_hex_into("010Fff", &mut output));
    assert_eq!(output, [0x01, 0x0F, 0xFF]);
}

// --- Identifier forms ---

#[test]
fn abbreviating_takes_the_first_digits_at_the_short_width() {
    assert_eq!(
        abbreviate_hex("abc123def456789012345678901234567890abcd", SHORT_HEX_WIDTH),
        "abc123de"
    );
}

#[test]
fn abbreviating_something_already_short_returns_it_whole() {
    assert_eq!(abbreviate_hex("abc123", SHORT_HEX_WIDTH), "abc123");
    assert_eq!(abbreviate_hex("", SHORT_HEX_WIDTH), "");
    assert_eq!(abbreviate_hex("abc123de", SHORT_HEX_WIDTH), "abc123de");
}

#[test]
fn abbreviating_honors_the_width_it_is_given() {
    let sha = "abc123def456789012345678901234567890abcd";
    assert_eq!(abbreviate_hex(sha, 12), "abc123def456");
    assert_eq!(abbreviate_hex(sha, 0), "");
}

#[test]
fn an_identifier_of_exactly_the_right_length_and_case_is_accepted() {
    assert!(is_lowercase_hex(
        "abc123def456789012345678901234567890abcd",
        40
    ));
    assert!(is_lowercase_hex(&"a".repeat(64), 64));
    assert!(is_lowercase_hex("0123456789abcdef", 16));
}

#[test]
fn the_wrong_length_is_refused_even_when_the_digits_are_good() {
    // Length is the point: an abbreviated digest is well-formed hex and still
    // not the identifier a caller is about to trust as one.
    assert!(!is_lowercase_hex("abc123de", 40));
    assert!(!is_lowercase_hex(&"a".repeat(41), 40));
    assert!(!is_lowercase_hex("", 40));
    assert!(is_lowercase_hex("", 0));
}

#[test]
fn uppercase_and_non_hex_identifiers_are_refused() {
    // Uppercase is a different string for the same bytes, and these identifiers
    // are compared as strings, so accepting both would make equality unreliable.
    assert!(!is_lowercase_hex(
        "ABC123DEF456789012345678901234567890ABCD",
        40
    ));
    assert!(!is_lowercase_hex(
        "abc123def45678901234567890123456789Zabcd",
        40
    ));
    assert!(!is_lowercase_hex("unknown", 40));
    assert!(!is_lowercase_hex(
        "abc123def456789012345678901234567-dirty",
        40
    ));
}

// --- One digit ---

#[test]
fn every_digit_has_its_value_in_both_cases() {
    assert_eq!(hex_digit_value(b'0'), Some(0));
    assert_eq!(hex_digit_value(b'9'), Some(9));
    assert_eq!(hex_digit_value(b'a'), Some(10));
    assert_eq!(hex_digit_value(b'f'), Some(15));
    assert_eq!(hex_digit_value(b'A'), Some(10));
    assert_eq!(hex_digit_value(b'F'), Some(15));
}

#[test]
fn the_bytes_beside_each_range_are_not_digits() {
    // One below and one above each accepted range — an off-by-one bound accepts
    // exactly one of these — and the letters a decimal-or-alphabet reader would
    // wrongly take.
    // Each byte of this string on its own: `/` `:` `` ` `` `g` `@` `G` `x`, a space, NUL.
    for outsider in *b"/:`g@Gx \0" {
        assert_eq!(hex_digit_value(outsider), None, "byte {outsider}");
    }
}

#[test]
fn a_digit_and_a_decoded_pair_agree_over_every_byte() {
    // The whole-string decoder is built on the per-digit primitive, so the two
    // can never disagree about what a digit is. A byte past ASCII spells a
    // two-byte character, which is no hex digit to either.
    for byte in every_byte() {
        let pair: String = [char::from(byte), '0'].iter().collect();
        assert_eq!(
            hex_digit_value(byte).is_some(),
            decode_hex(&pair).is_some(),
            "byte {byte}"
        );
    }
}

// fidelity-compile-proof: `HexDigitValue.IsUsableInConstantExpressions` — a
// `const` evaluation of the function, which fails to compile if it ever stops
// being `const`.
const _: () = {
    assert!(matches!(hex_digit_value(b'c'), Some(12)));
    assert!(hex_digit_value(b'z').is_none());
};
