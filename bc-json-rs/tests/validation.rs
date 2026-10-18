//! Ported from `libs/foundation/json/test/json_validation_test.cpp`.
//!
//! One Rust test per source `TEST`, same name in `snake_case`, same inputs,
//! adapted only where the Rust API spells a case differently.

use bc_json::{
    JsonReader, JsonValidationError, JsonValidationErrorKind, MAX_JSON_NESTING_DEPTH,
    validate_json, validate_json_bytes,
};

#[test]
fn accepts_every_root_value_kind_and_json_number_grammar() {
    for input in [
        "null",
        "true",
        "false",
        r#""text""#,
        "0",
        "-0",
        "12",
        "-12.5",
        "1e10",
        "1E+10",
        "1e-10",
        // Grammatically a number, even though no f64 can hold it. The validator
        // checks grammar; the reader is what refuses to hand back an infinity.
        "1e400",
        "[]",
        "{}",
        r#"{"items":[1,true,null,{"name":"valid"}]}"#,
    ] {
        assert!(validate_json(input).is_ok(), "{input}");
    }
}

#[test]
fn rejects_invalid_numbers() {
    for input in [
        "01", "-01", "1.", "1e", "1e+", ".5", "+1", "NaN", "Infinity", "0x10",
    ] {
        assert!(validate_json(input).is_err(), "{input}");
    }
}

#[test]
fn rejects_structural_errors_and_trailing_input() {
    for input in [
        "",
        " ",
        "[1,]",
        "[1 2]",
        r#"{"a":1,}"#,
        r#"{"a" 1}"#,
        r#"{"a":1 "b":2}"#,
        "{}{}",
        "true false",
        "[",
        "{",
    ] {
        assert!(validate_json(input).is_err(), "{input}");
    }

    assert_eq!(
        validate_json("{}  x").expect_err("trailing input"),
        JsonValidationError {
            kind: JsonValidationErrorKind::TrailingInput,
            byte_offset: 4,
        }
    );
}

#[test]
fn decodes_all_escapes_and_unicode_surrogate_pairs() {
    let input = r#"{"text":"\"\\\/\b\f\n\r\t\u0041\u00DF\u6771\uD834\uDD1E"}"#;
    let document = validate_json(input).expect("every escape here is legal");

    let decoded = JsonReader::new(document).string("text");
    assert_eq!(
        decoded.as_deref(),
        Some("\"\\/\u{8}\u{c}\n\r\tA\u{df}\u{6771}\u{1d11e}")
    );
}

#[test]
fn rejects_invalid_escapes_controls_and_surrogates() {
    for input in [
        r#""\x""#,
        r#""\u12G4""#,
        // A high surrogate with no low surrogate after it — at end of input,
        // and followed by an escape that is not one.
        r#""\uD800""#,
        r#""\uD800\u0041""#,
        // A low surrogate with no high surrogate before it.
        r#""\uDC00""#,
        "\"line\nfeed\"",
    ] {
        assert!(validate_json(input).is_err(), "{input}");
    }

    // An unterminated string is not a document, so it cannot be validated —
    // reading it at all requires saying so.
    let unterminated = JsonReader::from_trusted_unchecked(r#"{"value":"unterminated}"#);
    assert_eq!(unterminated.string("value"), None);
}

#[test]
fn validates_raw_utf8() {
    assert!(validate_json_bytes(b"\"\xc3\xa9\xf0\x9f\x98\x80\"").is_ok());

    for input in [
        // Overlong encoding of U+0000.
        b"\"\xc0\x80\"".as_slice(),
        // A lead byte followed by a non-continuation byte.
        b"\"\xe2\x28\xa1\"".as_slice(),
        // A surrogate encoded as raw UTF-8 (CESU-8).
        b"\"\xed\xa0\x80\"".as_slice(),
        // Above U+10FFFF.
        b"\"\xf4\x90\x80\x80\"".as_slice(),
        // Truncated sequence at end of input.
        b"\"\xf0\x9f\x98".as_slice(),
    ] {
        let error = validate_json_bytes(input).expect_err("invalid UTF-8");
        assert_eq!(error.kind, JsonValidationErrorKind::InvalidUtf8);
    }
}

#[test]
fn rejects_duplicate_decoded_object_keys_at_every_depth() {
    assert_eq!(
        validate_json(r#"{"id":1,"id":2}"#).expect_err("duplicate key"),
        JsonValidationError {
            kind: JsonValidationErrorKind::DuplicateKey,
            byte_offset: 8,
        }
    );

    // Keys are compared decoded, so an escaped spelling of the same key still
    // collides. This is the case a byte-comparing parser misses.
    let escaped = validate_json(r#"{"id":1,"\u0069d":2}"#).expect_err("duplicate key");
    assert_eq!(escaped.kind, JsonValidationErrorKind::DuplicateKey);

    // The same key in sibling objects is not a duplicate...
    assert!(validate_json(r#"{"left":{"id":1},"right":{"id":2}}"#).is_ok());
    // ...but a duplicate inside a nested object still is.
    assert!(validate_json(r#"{"outer":{"id":1,"id":2}}"#).is_err());
}

#[test]
fn enforces_maximum_container_depth() {
    let nested = |depth: usize| format!("{}0{}", "[".repeat(depth), "]".repeat(depth));

    assert!(validate_json(&nested(MAX_JSON_NESTING_DEPTH)).is_ok());
    assert_eq!(
        validate_json(&nested(MAX_JSON_NESTING_DEPTH + 1)).expect_err("too deep"),
        JsonValidationError {
            kind: JsonValidationErrorKind::NestingTooDeep,
            byte_offset: MAX_JSON_NESTING_DEPTH,
        }
    );
}

#[test]
fn reports_the_first_offending_byte() {
    // `\xe2` opens a three-byte sequence that `(` does not continue. The source
    // reports the continuation byte (4); `str::from_utf8` reports where validity
    // ended, which is the start of the bad sequence (3).
    let bad_utf8 = validate_json_bytes(b"\"ok\xe2(\xa1\"").expect_err("invalid UTF-8");
    assert_eq!(bad_utf8.byte_offset, 3);

    assert_eq!(
        validate_json(r#"{"a":"#).expect_err("cut short"),
        JsonValidationError {
            kind: JsonValidationErrorKind::UnexpectedEnd,
            byte_offset: 5,
        }
    );
}

// --- Beyond the source suite -------------------------------------------------

#[test]
fn a_document_that_validates_is_valid_utf8_by_construction() {
    // The byte entry point proves UTF-8 up front, so `Validated` can hand out a
    // `&str` with no second scan and no unsafe.
    let document = validate_json_bytes("{\"k\":\"é\"}".as_bytes()).expect("valid");
    assert_eq!(document.as_str(), "{\"k\":\"é\"}");
}

#[test]
fn nesting_limit_counts_objects_and_arrays_alike() {
    let alternating = |containers: usize| {
        let mut input = String::new();
        for index in 0..containers {
            input.push(if index % 2 == 0 { '[' } else { '{' });
            if index % 2 == 1 {
                input.push_str("\"k\":");
            }
        }
        input.push('0');
        for index in (0..containers).rev() {
            input.push(if index % 2 == 0 { ']' } else { '}' });
        }
        input
    };

    assert!(validate_json(&alternating(MAX_JSON_NESTING_DEPTH)).is_ok());
    assert_eq!(
        validate_json(&alternating(MAX_JSON_NESTING_DEPTH + 1))
            .expect_err("too deep")
            .kind,
        JsonValidationErrorKind::NestingTooDeep
    );
}

#[test]
fn line_and_column_locate_the_failure_for_a_human() {
    let input = "{\n  \"a\": 1,\n  \"a\": 2\n}";
    let error = validate_json(input).expect_err("duplicate key");
    let position = error.line_column(input);
    assert_eq!((position.line, position.column), (3, 3));
}

// Ported from `libs/foundation/json/test/json_nul_refusal_test.cpp`: the one
// escape whose decoding is a NUL byte is refused, in the decoder, for every
// document; every other escaped control stays accepted by the grammar.
#[test]
fn the_nul_escape_is_refused_at_its_offset_in_a_value_and_in_a_key() {
    let refused =
        validate_json(r#"{"email":"victim@x.com\u0000attacker"}"#).expect_err("the NUL escape");
    assert_eq!(refused.kind, JsonValidationErrorKind::NulCharacter);
    // The backslash of the escape, which is what a caller reports.
    assert_eq!(refused.byte_offset, r#"{"email":"victim@x.com"#.len());

    let refused = validate_json(r#"{"a\u0000b":1}"#).expect_err("the NUL escape in a key");
    assert_eq!(refused.kind, JsonValidationErrorKind::NulCharacter);
    assert_eq!(refused.byte_offset, 3);
}

#[test]
fn every_other_escaped_control_stays_accepted_by_the_decoder() {
    assert!(validate_json(r#"{"a":"\n\t"}"#).is_ok());
    assert!(validate_json(r#"{"a":"\u0001\u001f"}"#).is_ok());
}
