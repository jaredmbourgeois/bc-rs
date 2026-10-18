use std::collections::BTreeMap;

use bc_error::{
    AnyError, AnyErrorKind, MetadataArray, MetadataDictionary, MetadataDictionaryExt, MetadataType,
    MetadataValue,
};

#[test]
fn metadata_type_default_is_null() {
    assert_eq!(MetadataValue::default().r#type(), MetadataType::Null);
}

#[test]
fn metadata_type_all_types_correct() {
    assert_eq!(MetadataValue::null().r#type(), MetadataType::Null);
    assert_eq!(MetadataValue::from(true).r#type(), MetadataType::Boolean);
    assert_eq!(MetadataValue::from(1_i64).r#type(), MetadataType::Int64);
    assert_eq!(MetadataValue::from(1_u64).r#type(), MetadataType::Uint64);
    assert_eq!(MetadataValue::from(1.0).r#type(), MetadataType::Float64);
    assert_eq!(MetadataValue::from("x").r#type(), MetadataType::String);
    assert_eq!(
        MetadataValue::from(MetadataArray::new()).r#type(),
        MetadataType::Array
    );
    assert_eq!(
        MetadataValue::from(MetadataDictionary::new()).r#type(),
        MetadataType::Dictionary
    );
    assert_eq!(
        MetadataValue::from(AnyError::at_location(
            AnyErrorKind::new("cat", 1),
            "loc",
            MetadataDictionary::new()
        ))
        .r#type(),
        MetadataType::Error
    );
}

#[test]
fn metadata_value_default_construction_is_null() {
    let value = MetadataValue::default();
    assert!(value.is_null());
    assert_eq!(value.r#type(), MetadataType::Null);
}

#[test]
fn metadata_value_null_factory_is_null() {
    assert!(MetadataValue::null().is_null());
}

#[test]
fn metadata_value_bool_round_trip() {
    let true_value = MetadataValue::from(true);
    assert_eq!(true_value.as_bool(), Some(true));
    assert!(!true_value.is_null());
    assert_eq!(MetadataValue::from(false).as_bool(), Some(false));
}

#[test]
fn metadata_value_int64_round_trip() {
    assert_eq!(MetadataValue::from(-42_i64).as_int64(), Some(-42));
}

#[test]
fn metadata_value_uint64_round_trip() {
    assert_eq!(MetadataValue::from(999_u64).as_uint64(), Some(999));
}

#[test]
#[allow(clippy::approx_constant)] // This exact value comes from the source conformance test.
fn metadata_value_float64_round_trip() {
    assert_eq!(MetadataValue::from(3.14).as_float64(), Some(3.14));
}

#[test]
fn metadata_value_string_round_trip() {
    assert_eq!(MetadataValue::from("hello").as_string(), Some("hello"));
    assert_eq!(
        MetadataValue::from(String::from("owned")).as_string(),
        Some("owned")
    );
}

#[test]
fn metadata_value_array_round_trip() {
    let array = vec![MetadataValue::from(1_i64), MetadataValue::from("two")];
    let value = MetadataValue::from(array);
    let stored = value.as_array().unwrap();

    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].as_int64(), Some(1));
    assert_eq!(stored[1].as_string(), Some("two"));
}

#[test]
fn metadata_value_dictionary_round_trip() {
    let dictionary = BTreeMap::from([(String::from("key"), MetadataValue::from("value"))]);
    let value = MetadataValue::from(dictionary);
    let stored = value.as_dictionary().unwrap();

    assert_eq!(stored.len(), 1);
    assert_eq!(stored["key"].as_string(), Some("value"));
}

#[test]
fn metadata_value_error_round_trip() {
    let value = MetadataValue::from(AnyError::at_location(
        AnyErrorKind::new("net", 404),
        "test_location",
        MetadataDictionary::new(),
    ));
    let error = value.as_error().unwrap();

    assert_eq!(error.category(), "net");
    assert_eq!(error.kind(), 404);
}

#[test]
fn metadata_value_wrong_type_accessors_return_empty() {
    let value = MetadataValue::from(42_i64);

    assert_eq!(value.as_bool(), None);
    assert_eq!(value.as_uint64(), None);
    assert_eq!(value.as_float64(), None);
    assert_eq!(value.as_string(), None);
    assert_eq!(value.as_array(), None);
    assert_eq!(value.as_dictionary(), None);
    assert_eq!(value.as_error(), None);
    assert!(!value.is_null());
}

#[test]
fn metadata_value_null_accessors_return_empty() {
    let value = MetadataValue::null();

    assert_eq!(value.as_bool(), None);
    assert_eq!(value.as_int64(), None);
    assert_eq!(value.as_uint64(), None);
    assert_eq!(value.as_float64(), None);
    assert_eq!(value.as_string(), None);
    assert_eq!(value.as_array(), None);
    assert_eq!(value.as_dictionary(), None);
    assert_eq!(value.as_error(), None);
}

#[test]
fn metadata_value_stringify_null() {
    assert_eq!(MetadataValue::null().stringify(), "null");
}

#[test]
fn metadata_value_stringify_bool() {
    assert_eq!(MetadataValue::from(true).stringify(), "true");
    assert_eq!(MetadataValue::from(false).stringify(), "false");
}

#[test]
fn metadata_value_stringify_int64() {
    assert_eq!(MetadataValue::from(-42_i64).stringify(), "-42");
}

#[test]
fn metadata_value_stringify_uint64() {
    assert_eq!(MetadataValue::from(999_u64).stringify(), "999");
}

#[test]
#[allow(clippy::approx_constant)] // This exact value and rendering come from the source test.
fn metadata_value_stringify_float64() {
    let value = MetadataValue::from(3.14);
    assert_eq!(value.stringify_with_decimals(2), "3.14");
    assert_eq!(value.stringify(), "3.140000");
}

#[test]
fn metadata_value_stringify_string() {
    assert_eq!(MetadataValue::from("hello").stringify(), "\"hello\"");
}

#[test]
fn metadata_value_stringify_array() {
    let array = vec![MetadataValue::from(1_i64), MetadataValue::from("two")];
    assert_eq!(MetadataValue::from(array).stringify(), "[1, \"two\"]");
}

#[test]
fn metadata_value_stringify_dictionary() {
    let dictionary = BTreeMap::from([
        (String::from("b"), MetadataValue::from("two")),
        (String::from("a"), MetadataValue::from(1_i64)),
    ]);
    assert_eq!(
        MetadataValue::from(dictionary).stringify(),
        "{\"a\": 1, \"b\": \"two\"}"
    );
}

#[test]
fn metadata_value_stringify_error() {
    let value = MetadataValue::from(AnyError::at_location(
        AnyErrorKind::new("net", 404),
        "somewhere",
        MetadataDictionary::new(),
    ));
    assert_eq!(
        value.stringify(),
        "{\"category\": \"net\", \"kind\": 404, \"location\": \"somewhere\", \"metadata\": {}}"
    );
}

#[test]
fn metadata_value_stringify_error_with_metadata() {
    let metadata = BTreeMap::from([(String::from("url"), MetadataValue::from("/api"))]);
    let error = AnyError::at_location(AnyErrorKind::new("net", 404), "loc", metadata);
    assert_eq!(
        MetadataValue::from(error).stringify(),
        "{\"category\": \"net\", \"kind\": 404, \"location\": \"loc\", \"metadata\": {\"url\": \"/api\"}}"
    );
}

#[test]
fn metadata_value_stringify_empty_array() {
    assert_eq!(MetadataValue::from(MetadataArray::new()).stringify(), "[]");
}

#[test]
fn metadata_value_stringify_empty_dictionary() {
    assert_eq!(
        MetadataValue::from(MetadataDictionary::new()).stringify(),
        "{}"
    );
}

#[test]
fn metadata_value_stringify_nested_structures() {
    let inner = BTreeMap::from([(String::from("x"), MetadataValue::from(1_i64))]);
    let array = vec![MetadataValue::from(inner), MetadataValue::null()];
    assert_eq!(MetadataValue::from(array).stringify(), "[{\"x\": 1}, null]");
}

#[test]
fn metadata_value_deep_copy_with_error() {
    let original = MetadataValue::from(AnyError::at_location(
        AnyErrorKind::new("net", 404),
        "loc",
        MetadataDictionary::new(),
    ));
    let copy = original.clone();

    let original_error = original.as_error().unwrap();
    let copied_error = copy.as_error().unwrap();
    assert_eq!(copied_error.category(), "net");
    assert_eq!(copied_error.kind(), 404);
    assert!(!std::ptr::eq(original_error, copied_error));
    assert_eq!(original, copy);
}

#[test]
fn metadata_value_deep_copy_array() {
    let original = MetadataValue::from(vec![MetadataValue::from(42_i64)]);
    let copy = original.clone();

    let array = copy.as_array().unwrap();
    assert_eq!(array.len(), 1);
    assert_eq!(array[0].as_int64(), Some(42));
    assert_eq!(original, copy);
}

#[test]
fn metadata_value_equality_same_type_same_value() {
    assert_eq!(MetadataValue::from(42_i64), MetadataValue::from(42_i64));
    assert_eq!(MetadataValue::from("abc"), MetadataValue::from("abc"));
    assert_eq!(MetadataValue::null(), MetadataValue::null());
}

#[test]
fn metadata_value_equality_different_value() {
    assert_ne!(MetadataValue::from(1_i64), MetadataValue::from(2_i64));
}

#[test]
fn metadata_value_equality_different_type() {
    assert_ne!(MetadataValue::from(1_i64), MetadataValue::from(1_u64));
    assert_ne!(MetadataValue::from(1_i64), MetadataValue::from("1"));
}

#[test]
fn metadata_value_move_construction_preserves_destination_value() {
    let source = MetadataValue::from(String::from("hello"));
    let source_text_address = source.as_string().unwrap().as_ptr();

    let destination = source;

    assert_eq!(destination.as_string(), Some("hello"));
    assert_eq!(
        destination.as_string().unwrap().as_ptr(),
        source_text_address
    );
}

#[test]
fn metadata_value_move_assignment_replaces_destination_value() {
    let source = MetadataValue::from(vec![MetadataValue::from(String::from("moved"))]);
    let source_text_address = source.as_array().unwrap()[0].as_string().unwrap().as_ptr();
    let mut destination = MetadataValue::from("replaced");
    assert_eq!(destination.as_string(), Some("replaced"));

    destination = source;

    let moved_array = destination.as_array().unwrap();
    assert_eq!(moved_array[0].as_string(), Some("moved"));
    assert_eq!(
        moved_array[0].as_string().unwrap().as_ptr(),
        source_text_address
    );
}

#[test]
fn metadata_type_discriminants_match_source_order() {
    assert_eq!(MetadataType::Null as u8, 0);
    assert_eq!(MetadataType::Boolean as u8, 1);
    assert_eq!(MetadataType::Int64 as u8, 2);
    assert_eq!(MetadataType::Uint64 as u8, 3);
    assert_eq!(MetadataType::Float64 as u8, 4);
    assert_eq!(MetadataType::String as u8, 5);
    assert_eq!(MetadataType::Array as u8, 6);
    assert_eq!(MetadataType::Dictionary as u8, 7);
    assert_eq!(MetadataType::Error as u8, 8);
}

#[test]
fn stringify_deliberately_does_not_escape_quotes_or_newlines() {
    assert_eq!(
        MetadataValue::from("say \"hi\"\nnext").stringify(),
        "\"say \"hi\"\nnext\""
    );

    let dictionary =
        BTreeMap::from([(String::from("quoted\"\nkey"), MetadataValue::from("value"))]);
    assert_eq!(
        MetadataValue::from(dictionary).stringify(),
        "{\"quoted\"\nkey\": \"value\"}"
    );
}

#[test]
fn stringify_normalizes_nonfinite_float_spelling() {
    assert_eq!(MetadataValue::from(f64::NAN).stringify(), "nan");
    assert_eq!(MetadataValue::from(f64::INFINITY).stringify(), "inf");
    assert_eq!(MetadataValue::from(f64::NEG_INFINITY).stringify(), "-inf");
}

#[test]
fn stringify_error_places_context_before_metadata() {
    let error = AnyError::at_location(
        AnyErrorKind::new("net", 1),
        "loc",
        MetadataDictionary::new(),
    )
    .with_context("inner")
    .with_context("outer");
    assert_eq!(
        MetadataValue::from(error).stringify(),
        "{\"category\": \"net\", \"kind\": 1, \"location\": \"loc\", \"context\": [\"outer\", \"inner\"], \"metadata\": {}}"
    );
}

#[test]
fn dictionary_extension_builds_the_same_map_insert_does() {
    let chained = MetadataDictionary::new()
        .with("detail", "unreachable")
        .with("attempts", 3_u64);

    let mut inserted = MetadataDictionary::new();
    inserted.insert(
        "detail".to_owned(),
        MetadataValue::String("unreachable".to_owned()),
    );
    inserted.insert("attempts".to_owned(), MetadataValue::Uint64(3));

    assert_eq!(chained, inserted);
}

#[test]
fn dictionary_extension_takes_every_value_the_from_impls_cover() {
    let metadata = MetadataDictionary::new()
        .with("flag", true)
        .with("signed", -1_i64)
        .with("unsigned", 1_u64)
        .with("ratio", 0.5)
        .with("borrowed", "text")
        .with("owned", String::from("text"))
        .with("list", MetadataArray::from([MetadataValue::from(1_i64)]))
        .with("value", MetadataValue::null());

    assert_eq!(metadata.len(), 8);
    assert_eq!(metadata["flag"], MetadataValue::Boolean(true));
    assert_eq!(metadata["signed"], MetadataValue::Int64(-1));
    assert_eq!(metadata["borrowed"], metadata["owned"]);
    assert_eq!(metadata["value"], MetadataValue::Null);
}

#[test]
fn set_inserts_in_place_for_a_builder_that_cannot_chain() {
    // The shape a per-variant builder needs: entries decided by a branch, so
    // there is no single expression to chain onto.
    let mut metadata = MetadataDictionary::new();
    for (index, name) in ["first", "second"].into_iter().enumerate() {
        if index == 1 {
            metadata.set(name, index as u64);
        }
    }

    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata["second"], MetadataValue::Uint64(1));
}

#[test]
fn a_repeated_key_replaces_as_insert_does() {
    let metadata = MetadataDictionary::new()
        .with("detail", "first")
        .with("detail", "second");

    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata["detail"], MetadataValue::from("second"));
}
