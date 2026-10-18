//! Round-trip coverage for the optional `serde` feature.
//!
//! `MetadataValue::stringify` is a human diagnostic that deliberately does not
//! escape (pinned in `metadata_value.rs`). serde is the machine-readable path,
//! so these tests assert the property `stringify` does not have: that a value
//! survives a round trip unchanged, including the characters `stringify` would
//! mangle.

#![cfg(feature = "serde")]

use bc_error::{AnyError, AnyErrorKind, MetadataDictionary, MetadataValue};

fn round_trip(value: &MetadataValue) -> MetadataValue {
    let encoded = serde_json::to_string(value).expect("value should serialize");
    serde_json::from_str(&encoded).expect("value should deserialize")
}

#[test]
fn every_scalar_variant_round_trips() {
    for value in [
        MetadataValue::Null,
        MetadataValue::from(true),
        MetadataValue::from(-7_i64),
        MetadataValue::from(7_u64),
        MetadataValue::from(0.5_f64),
        MetadataValue::from("text"),
    ] {
        assert_eq!(round_trip(&value), value);
    }
}

#[test]
fn signed_and_unsigned_stay_distinct_across_a_round_trip() {
    // The representation is externally tagged rather than untagged precisely so
    // this holds: an untagged encoding could not tell 7i64 from 7u64.
    let signed = MetadataValue::from(7_i64);
    let unsigned = MetadataValue::from(7_u64);

    assert_eq!(round_trip(&signed), signed);
    assert_eq!(round_trip(&unsigned), unsigned);
    assert_ne!(round_trip(&signed), unsigned);
}

#[test]
fn quotes_and_newlines_survive_where_stringify_would_mangle_them() {
    let hostile = MetadataValue::from("say \"hi\"\nnext");

    assert_eq!(round_trip(&hostile), hostile);
    // The human path is unescaped by design; this is the contrast.
    assert_eq!(hostile.stringify(), "\"say \"hi\"\nnext\"");
}

#[test]
fn nested_containers_and_errors_round_trip() {
    let error = AnyError::at_location(AnyErrorKind::new("db", 3), "loc", MetadataDictionary::new())
        .with_context("outer")
        .with_metadata("detail", MetadataValue::from("locked"));
    let nested = MetadataValue::from(MetadataDictionary::from([
        (
            String::from("array"),
            MetadataValue::from(vec![MetadataValue::from(1_i64), MetadataValue::Null]),
        ),
        (String::from("error"), MetadataValue::from(error)),
    ]));

    assert_eq!(round_trip(&nested), nested);
}

#[test]
fn any_error_round_trips_with_its_context_chain() {
    let error = AnyError::at_location(
        AnyErrorKind::new("net", 404),
        "file.rs > #1",
        MetadataDictionary::new(),
    )
    .with_context("inner")
    .with_context("outer");

    let encoded = serde_json::to_string(&error).expect("error should serialize");
    let decoded: AnyError = serde_json::from_str(&encoded).expect("error should deserialize");

    assert_eq!(decoded, error);
    assert_eq!(decoded.context(), ["outer", "inner"]);
}
