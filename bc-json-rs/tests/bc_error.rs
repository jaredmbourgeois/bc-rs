//! The bridge into the family's shared error type.
//!
//! No longer feature-gated: the strict read tier returns errors carrying named
//! `AnyErrorKind` descriptors, so `bc-error` is part of this crate's surface
//! rather than an opt-in bolt-on. The dependency-free claim it used to protect
//! was about third-party code, and bc-error-rs is itself third-party-free.

use bc_error::AnyError;
use bc_json::{JsonValidationErrorKind, validate_json};

#[test]
fn a_validation_failure_converts_into_an_any_error() {
    let error = validate_json(r#"{"id":1,"id":2}"#).expect_err("duplicate key");
    let any_error: AnyError = error.into();

    assert_eq!(any_error.category(), "json");
    assert_eq!(
        any_error.kind(),
        JsonValidationErrorKind::DuplicateKey as i32
    );
    assert_eq!(
        any_error
            .metadata()
            .get("detail")
            .and_then(|value| value.as_string()),
        Some("duplicate object key")
    );
    assert_eq!(
        any_error
            .metadata()
            .get("byte_offset")
            .and_then(bc_error::MetadataValue::as_uint64),
        Some(8)
    );
}

#[test]
fn the_conversion_composes_with_the_question_mark_operator() {
    // The reason the bridge exists: a boundary that funnels every failure into
    // one error type should not need a manual `map_err` at each JSON call.
    fn parse(body: &str) -> Result<String, AnyError> {
        let document = validate_json(body)?;
        Ok(document.as_str().to_owned())
    }

    assert!(parse(r#"{"ok":true}"#).is_ok());
    assert_eq!(
        parse("{").expect_err("unterminated object").category(),
        "json"
    );
}

#[test]
fn the_numeric_kind_is_stable_across_reordering() {
    // These are a wire value carried into AnyError and across any binding, so
    // they are written out in the enum rather than left to declaration order.
    assert_eq!(JsonValidationErrorKind::UnexpectedEnd as i32, 1);
    assert_eq!(JsonValidationErrorKind::InvalidUtf8 as i32, 7);
    assert_eq!(JsonValidationErrorKind::TrailingInput as i32, 10);
    assert_eq!(JsonValidationErrorKind::NulCharacter as i32, 11);
}
