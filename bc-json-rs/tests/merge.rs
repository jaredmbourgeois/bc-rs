//! Override-merge, ported from `json_merge_test.cpp`.

use bc_error::AnyErrorKind;
use bc_json::{
    JsonValidationErrorKind, VALIDATION_CATEGORY, merge_object_overrides, validate_json,
};

fn merged(base: &str, overrides: &str) -> String {
    merge_object_overrides(base, overrides)
        .expect("the fixture merges")
        .json
}

#[test]
fn substitutes_in_place_and_keeps_base_order() {
    let result = merge_object_overrides(r#"{"first":1,"second":2,"third":3}"#, r#"{"second":20}"#)
        .expect("the fixture merges");

    assert_eq!(result.replaced, ["second"]);
    assert!(result.appended.is_empty(), "{:?}", result.appended);

    // Order is the base's, so a diff against the original is minimal.
    let keys: Vec<&str> = ["first", "second", "third"]
        .into_iter()
        .filter(|key| result.json.contains(&format!("\"{key}\"")))
        .collect();
    assert_eq!(keys, ["first", "second", "third"]);

    let reader = bc_json::JsonReader::new(validate_json(&result.json).expect("valid JSON"));
    assert_eq!(reader.integer("first"), Some(1));
    assert_eq!(reader.integer("second"), Some(20));
    assert_eq!(reader.integer("third"), Some(3));
}

#[test]
fn passes_untouched_values_through_verbatim() {
    // A nested object and a number spelling this crate would normalize if it
    // round-tripped them through a value model. Verbatim means neither moves.
    let base = r#"{"kept":{"b":2,"a":1},"scale":1.500,"touched":0}"#;
    let result = merged(base, r#"{"touched":9}"#);

    assert!(
        result.contains(r#"{"b":2,"a":1}"#),
        "nested order preserved"
    );
    assert!(result.contains("1.500"), "number spelling preserved");
}

#[test]
fn reports_keys_the_base_did_not_carry() {
    let result = merge_object_overrides(r#"{"known":1}"#, r#"{"known":2,"invented":3}"#)
        .expect("the fixture merges");

    assert_eq!(result.replaced, ["known"]);
    assert_eq!(result.appended, ["invented"]);
}

#[test]
fn empty_overrides_reproduces_the_base() {
    let result = merge_object_overrides(r#"{"a":1,"b":[1,2]}"#, "{}").expect("the fixture merges");

    assert!(result.replaced.is_empty(), "{:?}", result.replaced);
    assert!(result.appended.is_empty(), "{:?}", result.appended);

    let reader = bc_json::JsonReader::new(validate_json(&result.json).expect("valid JSON"));
    assert_eq!(reader.integer("a"), Some(1));
    assert!(reader.find_array("b").is_some());
}

#[test]
fn produces_valid_json() {
    let result = merged(
        r#"{"a":1,"nested":{"x":[1,{"y":null}]},"s":"text"}"#,
        r#"{"a":2,"added":true}"#,
    );

    validate_json(&result).expect("a merge always produces valid JSON");
}

#[test]
fn escaped_keys_re_emit_as_valid_json() {
    // `extract_object_fields` decodes keys, so `a\"b` arrives as the bytes
    // `a"b`. Pasted raw, that quote would terminate the key string and produce
    // invalid JSON this crate had already blessed on the way in.
    let base = r#"{"a\"b":1,"c\\d":2,"e\nf":3}"#;
    let result = merged(base, r#"{"c\\d":20}"#);

    validate_json(&result).expect("re-escaped keys stay valid");
    let reader = bc_json::JsonReader::new(validate_json(&result).expect("valid JSON"));
    assert_eq!(reader.integer("a\"b"), Some(1));
    assert_eq!(reader.integer("c\\d"), Some(20));
    assert_eq!(reader.integer("e\nf"), Some(3));
}

#[test]
fn braces_and_quotes_inside_string_values_pass_verbatim() {
    let base = r#"{"tricky":"a{\"b\":1}c","other":1}"#;
    let result = merged(base, r#"{"other":2}"#);

    let reader = bc_json::JsonReader::new(validate_json(&result).expect("valid JSON"));
    assert_eq!(reader.string("tricky").as_deref(), Some(r#"a{"b":1}c"#));
}

#[test]
fn a_key_that_is_a_prefix_of_another_matches_only_itself() {
    let result = merge_object_overrides(r#"{"key":1,"keyword":2}"#, r#"{"key":10}"#)
        .expect("the fixture merges");

    assert_eq!(result.replaced, ["key"]);
    let reader = bc_json::JsonReader::new(validate_json(&result.json).expect("valid JSON"));
    assert_eq!(reader.integer("key"), Some(10));
    assert_eq!(reader.integer("keyword"), Some(2));
}

#[test]
fn refuses_a_truncated_base_rather_than_merging_what_it_could_read() {
    // The failure the validation gate exists for: `extract_object_fields` fails
    // open, so without the gate this would "succeed" into a document missing
    // every key after the truncation point.
    let error = merge_object_overrides(r#"{"a":1,"b":"#, r#"{"a":2}"#)
        .expect_err("a truncated base is refused");

    // Malformed input crosses as the *validation* taxonomy, carrying its kind
    // and byte offset — not as one of this crate's structured kinds. Naming the
    // wrong category would send the reader looking for a shape problem in a
    // document whose problem is syntax.
    assert!(error.matches(&AnyErrorKind::new(
        VALIDATION_CATEGORY,
        JsonValidationErrorKind::UnexpectedEnd as i32
    )));
}

#[test]
fn refuses_a_duplicate_keyed_base() {
    // The one case where the lazy keyed accessors and the enumerator would
    // legitimately disagree about a document.
    let error = merge_object_overrides(r#"{"a":1,"a":2}"#, "{}")
        .expect_err("a duplicate-keyed base is refused");

    assert!(error.matches(&AnyErrorKind::new(
        VALIDATION_CATEGORY,
        JsonValidationErrorKind::DuplicateKey as i32
    )));
}

#[test]
fn refuses_non_objects_on_either_side() {
    for (base, overrides) in [("[1,2]", "{}"), ("{}", r#""text""#), ("null", "{}")] {
        let error =
            merge_object_overrides(base, overrides).expect_err("a non-object side is refused");
        assert!(
            error.matches(&bc_json::kinds::TYPE_MISMATCH),
            "well-formed but wrong-shaped is a type mismatch, not a syntax error"
        );
    }
}
