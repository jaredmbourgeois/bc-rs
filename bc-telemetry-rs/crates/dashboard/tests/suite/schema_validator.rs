//! The schema validator the dashboards are held to (`tests/support/json_schema.rs`),
//! shown able to refuse what each keyword it holds forbids — a validator that
//! cannot fail would make every dashboard's green worthless.

use crate::json_schema::Schema;

fn violations(schema: &str, instance: &str) -> Vec<String> {
    Schema::new(schema).violations(instance)
}

/// What an instance the schema admits yields.
const NO_VIOLATIONS: [&str; 0] = [];

#[test]
fn type_admits_its_kind_and_refuses_others() {
    let integer = r#"{"type": "integer"}"#;
    assert_eq!(violations(integer, "3"), NO_VIOLATIONS);
    assert_eq!(violations(integer, "3.5"), [": is not of type integer"]);
    assert_eq!(violations(integer, r#""3""#), [": is not of type integer"]);
    let string_or_null = r#"{"type": ["string", "null"]}"#;
    assert_eq!(violations(string_or_null, "null"), NO_VIOLATIONS);
    assert_eq!(violations(string_or_null, r#""a""#), NO_VIOLATIONS);
    assert_eq!(
        violations(string_or_null, "true"),
        [": is not of type string or null"]
    );
    for (type_name, instance) in [
        ("object", "{}"),
        ("array", "[]"),
        ("number", "1.5"),
        ("boolean", "false"),
    ] {
        let schema = format!(r#"{{"type": "{type_name}"}}"#);
        assert!(violations(&schema, instance).is_empty(), "{type_name}");
        assert_eq!(
            violations(&schema, r#""text""#),
            [format!(": is not of type {type_name}")]
        );
    }
}

#[test]
fn properties_required_and_additional_properties_hold_an_object_shut() {
    let schema = r#"{
        "type": "object",
        "required": ["h"],
        "additionalProperties": false,
        "properties": {"h": {"type": "integer"}, "w": {"type": "integer"}}
    }"#;
    assert_eq!(violations(schema, r#"{"h": 1, "w": 2}"#), NO_VIOLATIONS);
    assert_eq!(violations(schema, r#"{"w": 2}"#), [": is missing `h`"]);
    assert_eq!(
        violations(schema, r#"{"h": 1, "x": 0}"#),
        ["/x: is not a property this object has"]
    );
    assert_eq!(
        violations(schema, r#"{"h": "tall"}"#),
        ["/h: is not of type integer"]
    );
    // A schema for what is not listed, rather than `false`.
    let open = r#"{"properties": {"a": {}}, "additionalProperties": {"type": "string"}}"#;
    assert_eq!(violations(open, r#"{"a": 1, "b": "x"}"#), NO_VIOLATIONS);
    assert_eq!(
        violations(open, r#"{"b": 2}"#),
        ["/b: is not of type string"]
    );
}

#[test]
fn items_holds_every_element_and_says_which() {
    let schema = r#"{"type": "array", "items": {"type": "string"}}"#;
    assert_eq!(violations(schema, r#"["a", "b"]"#), NO_VIOLATIONS);
    assert_eq!(
        violations(schema, r#"["a", 2, "c", false]"#),
        ["/1: is not of type string", "/3: is not of type string"]
    );
}

#[test]
fn enum_and_const_compare_values_not_spellings() {
    let schema = r#"{"enum": ["h", "v", 2]}"#;
    assert_eq!(violations(schema, r#""v""#), NO_VIOLATIONS);
    assert_eq!(violations(schema, "2.0"), NO_VIOLATIONS);
    assert_eq!(violations(schema, r#""x""#).len(), 1);
    let constant = r#"{"const": "row"}"#;
    assert_eq!(violations(constant, r#""row""#), NO_VIOLATIONS);
    assert_eq!(violations(constant, r#""row""#), NO_VIOLATIONS);
    assert_eq!(
        violations(constant, r#""panel""#),
        [r#": `"panel"` is not "row""#]
    );
}

#[test]
fn a_ref_resolves_to_its_definition_and_stands_alone() {
    let schema = r##"{
        "definitions": {"Width": {"type": "integer", "maximum": 24}},
        "type": "object",
        "properties": {"w": {"$ref": "#/definitions/Width", "type": "string"}}
    }"##;
    // The `type` beside the `$ref` is ignored, as draft-07 says.
    assert_eq!(violations(schema, r#"{"w": 12}"#), NO_VIOLATIONS);
    assert_eq!(
        violations(schema, r#"{"w": 25}"#),
        ["/w: 25 breaks `maximum` 24"]
    );
}

#[test]
fn all_of_any_of_and_one_of_combine_as_they_say() {
    let all_of = r#"{"allOf": [{"type": "integer"}, {"minimum": 1}]}"#;
    assert_eq!(violations(all_of, "2"), NO_VIOLATIONS);
    assert_eq!(violations(all_of, "0"), [": 0 breaks `minimum` 1"]);
    let any_of = r#"{"anyOf": [{"type": "integer"}, {"type": "null"}]}"#;
    assert_eq!(violations(any_of, "null"), NO_VIOLATIONS);
    assert_eq!(
        violations(any_of, r#""x""#),
        [": holds none of `anyOf`'s schemas"]
    );
    let one_of = r#"{"oneOf": [{"type": "integer"}, {"type": "number"}]}"#;
    assert_eq!(violations(one_of, "1.5"), NO_VIOLATIONS);
    assert_eq!(
        violations(one_of, "1"),
        [": holds 2 of `oneOf`'s schemas, not exactly one"]
    );
    assert_eq!(
        violations(one_of, "true"),
        [": holds 0 of `oneOf`'s schemas, not exactly one"]
    );
}

#[test]
fn numeric_bounds_hold_at_their_edges() {
    let schema = r#"{"minimum": 0, "exclusiveMaximum": 24}"#;
    assert_eq!(violations(schema, "0"), NO_VIOLATIONS);
    assert_eq!(violations(schema, "23"), NO_VIOLATIONS);
    assert_eq!(violations(schema, "-1"), [": -1 breaks `minimum` 0"]);
    assert_eq!(
        violations(schema, "24"),
        [": 24 breaks `exclusiveMaximum` 24"]
    );
    let other = r#"{"exclusiveMinimum": 0, "maximum": 24}"#;
    assert_eq!(violations(other, "24"), NO_VIOLATIONS);
    assert_eq!(violations(other, "0"), [": 0 breaks `exclusiveMinimum` 0"]);
    assert_eq!(violations(other, "25"), [": 25 breaks `maximum` 24"]);
}

#[test]
fn min_length_counts_characters() {
    let schema = r#"{"minLength": 2}"#;
    assert_eq!(violations(schema, r#""ab""#), NO_VIOLATIONS);
    assert_eq!(violations(schema, r#""é€""#), NO_VIOLATIONS);
    assert_eq!(
        violations(schema, r#""a""#),
        [": is shorter than 2 characters"]
    );
}

#[test]
fn true_holds_anything_and_false_holds_nothing() {
    let schema = r#"{"properties": {"anything": true, "nothing": false}}"#;
    assert_eq!(violations(schema, r#"{"anything": [1]}"#), NO_VIOLATIONS);
    assert_eq!(
        violations(schema, r#"{"nothing": 1}"#),
        ["/nothing: no value is allowed here"]
    );
}

#[test]
fn annotations_refuse_nothing() {
    let schema = r#"{"description": "d", "default": 1, "format": "date-time", "deprecated": true, "x-note": 1}"#;
    assert_eq!(violations(schema, r#""not a date""#), NO_VIOLATIONS);
}

#[test]
#[should_panic(expected = "the schema uses `pattern`, a keyword this validator does not hold")]
fn a_keyword_it_does_not_hold_is_refused_rather_than_skipped() {
    let _violations = violations(r#"{"pattern": "^a"}"#, r#""b""#);
}
