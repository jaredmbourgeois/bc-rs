//! The strict read tier, ported from `json_read_test.cpp`.
//!
//! The property under test throughout is that absent, `null`, and
//! present-but-wrong-type stay three separable answers rather than collapsing
//! into one `None`.

use bc_error::AnyError;
use bc_json::{
    JsonReadErrorKind, JsonReader, JsonValidationErrorKind, NullReads, VALIDATION_CATEGORY, kinds,
    validate_json,
};

fn reader(json: &str) -> JsonReader<'_> {
    JsonReader::new(validate_json(json).expect("the fixture is valid JSON"))
}

#[test]
fn required_refuses_absent_and_wrong_type() {
    let document = reader(r#"{"name":"Alice","age":30}"#);

    assert_eq!(document.required_string("name").unwrap(), "Alice");
    assert_eq!(
        document.required_string("missing").unwrap_err().kind(),
        JsonReadErrorKind::Absent
    );
    // Present, and not a string. The permissive `string()` answers `None` here
    // as well as above, which is the fold this tier exists to undo.
    assert_eq!(
        document.required_string("age").unwrap_err().kind(),
        JsonReadErrorKind::NotOfType
    );
}

#[test]
fn optional_tier_refuses_an_explicit_null() {
    let document = reader(r#"{"note":null}"#);

    assert_eq!(
        document
            .optional_string("note", NullReads::Refused)
            .unwrap_err()
            .kind(),
        JsonReadErrorKind::Null
    );
    // Absence is still not an error in this tier -- only the null is.
    assert_eq!(
        document
            .optional_string("absent", NullReads::Refused)
            .unwrap(),
        None
    );
}

#[test]
fn nullable_tier_reads_an_explicit_null_as_absent() {
    let document = reader(r#"{"note":null,"kept":"here"}"#);

    assert_eq!(
        document
            .optional_string("note", NullReads::AsAbsent)
            .unwrap(),
        None
    );
    assert_eq!(
        document
            .optional_string("absent", NullReads::AsAbsent)
            .unwrap(),
        None
    );
    // Reading null as absent excuses null, never a type.
    assert_eq!(
        document
            .optional_string("kept", NullReads::AsAbsent)
            .unwrap()
            .as_deref(),
        Some("here")
    );
}

#[test]
fn integer_and_boolean_carry_the_same_three_answers() {
    let document = reader(r#"{"count":7,"enabled":true,"wrong":"text","void":null}"#);

    assert_eq!(document.required_integer("count").unwrap(), 7);
    assert!(document.required_boolean("enabled").unwrap());

    for kind in [
        document.required_integer("wrong").unwrap_err().kind(),
        document.required_boolean("wrong").unwrap_err().kind(),
    ] {
        assert_eq!(kind, JsonReadErrorKind::NotOfType);
    }
    for kind in [
        document.required_integer("absent").unwrap_err().kind(),
        document.required_boolean("absent").unwrap_err().kind(),
    ] {
        assert_eq!(kind, JsonReadErrorKind::Absent);
    }
    for kind in [
        document.required_integer("void").unwrap_err().kind(),
        document.required_boolean("void").unwrap_err().kind(),
    ] {
        assert_eq!(kind, JsonReadErrorKind::Null);
    }
}

#[test]
fn floating_strict_separates_absent_null_and_malformed() {
    // The permissive `floating()` answers `None` to all three of these, and
    // says so in its own doc comment; this is that comment's advice as an API.
    let document = reader(r#"{"ratio":0.5,"void":null,"text":"0.5"}"#);

    assert!((document.required_floating("ratio").unwrap() - 0.5).abs() < f64::EPSILON);
    assert_eq!(
        document.required_floating("absent").unwrap_err().kind(),
        JsonReadErrorKind::Absent
    );
    assert_eq!(
        document.required_floating("void").unwrap_err().kind(),
        JsonReadErrorKind::Null
    );
    // A quoted number is a string, not a number.
    assert_eq!(
        document.required_floating("text").unwrap_err().kind(),
        JsonReadErrorKind::NotOfType
    );
}

#[test]
fn a_refusal_names_the_key_it_refused() {
    let document = reader(r#"{"present":1}"#);

    let error = document.required_string("absent").unwrap_err();
    assert_eq!(error.key(), "absent");
    assert!(error.to_string().contains("absent"));
}

#[test]
fn a_refusal_crosses_into_any_error_under_its_named_kind() {
    let document = reader(r#"{"age":30}"#);

    let error: AnyError = document.required_string("age").unwrap_err().into();

    assert!(error.matches(&kinds::TYPE_MISMATCH));
    // The location is this test's line, not a crate name pasted into the slot.
    assert!(error.location().contains("read.rs"));
    assert_eq!(
        error.metadata()["field"].as_string(),
        Some("age"),
        "the refused key travels as metadata, not only inside the prose"
    );
}

#[test]
fn the_two_taxonomies_cannot_collide_because_their_categories_differ() {
    // `AnyError`'s identity is (category, value) and its value is a bare
    // integer *within* a category, so two taxonomies sharing a category must
    // never share a number. `JsonValidationErrorKind` is `#[non_exhaustive]`
    // and its own doc tells the next author to take the next free number, so a
    // shared category would be a collision the crate instructs someone to
    // create. Separate categories make it unwritable instead of watched for.
    //
    // The evidence is taken from the subjects themselves -- two real errors and
    // the validation enum's own values -- rather than from a remembered bound.
    // An earlier version asserted `kind > 10` against a literal, which would
    // have stayed green through the exact collision it was named for.
    let validation: AnyError = validate_json("{").unwrap_err().into();
    let read: AnyError = reader(r#"{"a":1}"#)
        .required_string("a")
        .unwrap_err()
        .into();

    assert_eq!(validation.category(), VALIDATION_CATEGORY);
    assert_eq!(read.category(), kinds::CATEGORY);
    assert_ne!(VALIDATION_CATEGORY, kinds::CATEGORY);

    // Numbers may overlap freely across the two categories -- that is the point
    // of separating them -- but a kind from one must never match the other.
    let validation_values = [
        JsonValidationErrorKind::UnexpectedEnd,
        JsonValidationErrorKind::UnexpectedToken,
        JsonValidationErrorKind::InvalidNumber,
        JsonValidationErrorKind::InvalidString,
        JsonValidationErrorKind::InvalidEscape,
        JsonValidationErrorKind::InvalidUnicodeEscape,
        JsonValidationErrorKind::InvalidUtf8,
        JsonValidationErrorKind::DuplicateKey,
        JsonValidationErrorKind::NestingTooDeep,
        JsonValidationErrorKind::TrailingInput,
    ];
    for kind in validation_values {
        let crossed: AnyError = bc_json::JsonValidationError {
            kind,
            byte_offset: 0,
        }
        .into();
        for read_kind in [
            &kinds::MISSING_FIELD,
            &kinds::TYPE_MISMATCH,
            &kinds::INVALID_VALUE,
            &kinds::EMIT_FAILED,
        ] {
            assert!(
                !crossed.matches(read_kind),
                "the validation kind {kind:?} matches a structured kind; the two \
                 taxonomies must stay in separate categories"
            );
        }
    }
}

#[test]
fn the_optional_tier_still_refuses_a_wrong_typed_value() {
    // Excusing absence never excuses a type. This is what makes an
    // `optional_*(…)?.unwrap_or(default)` read safe: the default stands in for
    // a key that was not there, never for one that was there and corrupt.
    let document = reader(r#"{"count":"seven"}"#);

    for null_reads in [NullReads::Refused, NullReads::AsAbsent] {
        assert_eq!(
            document
                .optional_integer("count", null_reads)
                .unwrap_err()
                .kind(),
            JsonReadErrorKind::NotOfType
        );
    }
    assert_eq!(
        document
            .optional_integer("absent", NullReads::Refused)
            .unwrap()
            .unwrap_or(-1),
        -1,
        "an absent key is what the caller's default is for"
    );
}

// --- Arrays, on the same two shapes --------------------------------------
//
// These moved here from `tests/reader.rs` when the array-only tier folded into
// this one. What changed with them is one answer: `"entries": null` used to be
// `NotAnArray`, indistinguishable from a number in that slot. It is now `Null`,
// because an array is read through the same four states every scalar is.

#[test]
fn a_strict_array_read_refuses_the_element_the_permissive_one_would_drop() {
    let refusal = reader(r#"{"x5c":["a","b",1,"c"]}"#)
        .required_string_array("x5c")
        .expect_err("an element of the wrong type must refuse, not truncate");
    assert_eq!(refusal.kind(), JsonReadErrorKind::ElementType { index: 2 });
    assert_eq!(
        refusal.key(),
        "x5c",
        "the refusal names the field, not only the index"
    );

    let rows = reader(r#"{"rows":[{"a":1},7]}"#)
        .required_object_array("rows")
        .expect_err("a non-object element must refuse");
    assert_eq!(rows.kind(), JsonReadErrorKind::ElementType { index: 1 });
}

#[test]
fn a_strict_array_read_tells_absent_from_null_from_mistyped() {
    assert_eq!(
        reader(r#"{"entries":[ "a" ]}"#)
            .required_string_array("entries")
            .unwrap(),
        vec!["a"]
    );

    // The three ways a read can come back empty-handed, kept apart. Under the
    // array-only tier the first two were one answer.
    let null_valued = reader(r#"{"entries":null}"#);
    assert_eq!(
        null_valued
            .required_string_array("entries")
            .expect_err("a required array is not satisfied by null")
            .kind(),
        JsonReadErrorKind::Null
    );
    assert_eq!(
        null_valued
            .optional_string_array("entries", NullReads::Refused)
            .expect_err("Refused means refused")
            .kind(),
        JsonReadErrorKind::Null
    );
    assert_eq!(
        null_valued
            .optional_string_array("entries", NullReads::AsAbsent)
            .expect("AsAbsent reads null exactly as absence"),
        None
    );

    let mistyped = reader(r#"{"entries":7}"#);
    for null_reads in [NullReads::Refused, NullReads::AsAbsent] {
        assert_eq!(
            mistyped
                .optional_string_array("entries", null_reads)
                .expect_err("excusing null never excuses a type")
                .kind(),
            JsonReadErrorKind::NotOfType
        );
    }

    let missing = reader(r#"{"other":1}"#);
    assert_eq!(
        missing
            .required_string_array("entries")
            .expect_err("a required array refuses an absent key")
            .kind(),
        JsonReadErrorKind::Absent
    );
    assert_eq!(
        missing
            .optional_string_array("entries", NullReads::Refused)
            .expect("an absent key is not an error for the optional tier"),
        None,
        "absent stays distinguishable from an empty list, which the old \
         AbsentArray::IsEmpty policy folded together"
    );
}

#[test]
fn an_empty_array_reads_as_an_empty_list_under_either_shape() {
    let empty = reader(r#"{"entries":[]}"#);
    assert!(
        empty.required_string_array("entries").unwrap().is_empty(),
        "{:?}",
        empty.required_string_array("entries").unwrap()
    );
    assert!(empty.required_object_array("entries").unwrap().is_empty());
    for null_reads in [NullReads::Refused, NullReads::AsAbsent] {
        assert_eq!(
            empty.optional_string_array("entries", null_reads).unwrap(),
            Some(Vec::new())
        );
        assert!(
            empty
                .optional_object_array("entries", null_reads)
                .unwrap()
                .is_some_and(|rows| rows.is_empty())
        );
    }
}

#[test]
fn an_element_index_crosses_into_any_error_beside_the_field() {
    let error: AnyError = reader(r#"{"x5c":["a",1]}"#)
        .required_string_array("x5c")
        .unwrap_err()
        .into();

    assert!(error.matches(&kinds::TYPE_MISMATCH));
    assert_eq!(error.metadata()["field"].as_string(), Some("x5c"));
    assert_eq!(
        error.metadata()["element_index"],
        bc_error::MetadataValue::Uint64(1),
        "the index is not recoverable from the kind, so it travels as metadata"
    );
}
