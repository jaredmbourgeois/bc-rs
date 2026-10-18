//! Percent-encoding, against `libs/foundation/core/test/url_encode_test.cpp`.
//!
//! Each test is named for the property it asserts rather than for the C++ case
//! it came from. The tests account for each of the original's 11 cases.

use bc_core::{form_urldecode, form_urlencode, percent_decode, percent_encode};

/// RFC 3986 §2.3's unreserved set, which neither encoder may touch.
const UNRESERVED: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

// --- Encoding ---

#[test]
fn encoding_nothing_gives_an_empty_string() {
    assert_eq!(percent_encode(b""), "");
    assert_eq!(form_urlencode(b""), "");
}

#[test]
fn the_unreserved_set_passes_through_both_encoders() {
    assert_eq!(percent_encode(UNRESERVED.as_bytes()), UNRESERVED);
    assert_eq!(form_urlencode(UNRESERVED.as_bytes()), UNRESERVED);
}

#[test]
fn a_reserved_byte_becomes_an_escape_in_uppercase_hex() {
    assert_eq!(percent_encode(b"/?&="), "%2F%3F%26%3D");
    // '+' is reserved, so it escapes rather than standing for itself.
    assert_eq!(percent_encode(b"a+b"), "a%2Bb");
    assert_eq!(percent_encode(&[0xFF]), "%FF");
}

#[test]
fn a_multi_byte_character_encodes_one_byte_at_a_time() {
    assert_eq!(percent_encode("café".as_bytes()), "caf%C3%A9");
}

#[test]
fn a_space_is_percent_twenty() {
    assert_eq!(percent_encode(b"a b c"), "a%20b%20c");
}

#[test]
fn the_form_variant_writes_a_space_as_plus_and_differs_in_nothing_else() {
    assert_eq!(form_urlencode(b"a b c"), "a+b+c");
    // Including the literal '+', which still escapes — so the two never collide.
    assert_eq!(form_urlencode(b"a+b"), "a%2Bb");
    assert_eq!(form_urlencode(b"/?&="), "%2F%3F%26%3D");
}

// --- Decoding ---

#[test]
fn everything_the_encoder_writes_decodes_back_to_what_it_was_given() {
    let originals: [&[u8]; 7] = [
        b"",
        b"plain",
        b"a b c",
        b"/?&=",
        "café".as_bytes(),
        b"a+b",
        &[0xFF],
    ];

    for original in originals {
        assert_eq!(
            percent_decode(&percent_encode(original)).as_deref(),
            Some(original),
            "round trip failed for {original:?}"
        );
    }
}

#[test]
fn an_escape_decodes_from_either_hex_case() {
    // A decoder identifies bytes, so both spellings name the same one.
    assert_eq!(percent_decode("%64bname").as_deref(), Some(&b"dbname"[..]));
    assert_eq!(percent_decode("%44BNAME").as_deref(), Some(&b"DBNAME"[..]));
}

#[test]
fn a_plus_stays_a_literal_plus() {
    // RFC 3986 does not read '+' as a space; that is the form-body convention.
    assert_eq!(percent_decode("a+b").as_deref(), Some(&b"a+b"[..]));
}

#[test]
fn a_malformed_escape_is_refused_rather_than_guessed() {
    // A caller of a decoder is deciding something about the decoded bytes; a
    // best-effort answer hands that decision bytes the real consumer never saw.
    assert_eq!(percent_decode("%"), None);
    assert_eq!(percent_decode("%4"), None);
    assert_eq!(percent_decode("%zz"), None);
    assert_eq!(percent_decode("ok%4z"), None);
}

#[test]
fn a_form_body_round_trips_and_its_plus_is_a_space() {
    for original in [
        "",
        "plain",
        "a b c",
        "/?&=",
        "caf\u{e9}",
        "a+b",
        "metadata[bc_idempotency_key]",
    ] {
        assert_eq!(
            form_urldecode(&form_urlencode(original.as_bytes())),
            Some(original.as_bytes().to_vec()),
            "{original}"
        );
    }
    // The one place it differs from `percent_decode`: a literal `+` on the wire
    // is a space.
    assert_eq!(form_urldecode("a+b"), Some(b"a b".to_vec()));
    assert_eq!(percent_decode("a+b"), Some(b"a+b".to_vec()));
    assert_eq!(form_urldecode("%zz"), None);
}
