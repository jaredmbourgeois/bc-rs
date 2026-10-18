//! Reader and traversal tests, ported from
//! `libs/foundation/json/test/json_test.cpp`.

use bc_json::{JsonReader, detail, validate_json};

/// Builds a reader the way callers are meant to. It doubles as a check that
/// every fixture below really is a well-formed document — the source suite
/// never validated its own inputs, because it had no way to.
fn open(document: &str) -> JsonReader<'_> {
    JsonReader::new(validate_json(document).expect("test fixture must be valid JSON"))
}

#[test]
fn reads_scalars() {
    let reader = open(r#"{"name":"Alice","age":30,"active":true,"ratio":1.5}"#);
    assert_eq!(reader.string("name").as_deref(), Some("Alice"));
    assert_eq!(reader.integer("age"), Some(30));
    assert_eq!(reader.boolean("active"), Some(true));
    assert_eq!(reader.floating("ratio"), Some(1.5));
}

#[test]
fn missing_key_returns_none() {
    let reader = open(r#"{"name":"Alice"}"#);
    assert_eq!(reader.integer("age"), None);
    assert_eq!(reader.string("missing"), None);
}

#[test]
fn scalar_lookup_ignores_nested_same_key() {
    let reader = open(r#"{"services":{"payments":{"status":"disabled"}},"status":"ok"}"#);
    assert_eq!(reader.string("status").as_deref(), Some("ok"));
}

#[test]
fn object_lookup_ignores_nested_same_key() {
    let reader = open(r#"{"outer":{"target":{"v":1}},"target":{"v":2}}"#);
    let target = reader.find_object("target").expect("top-level target");
    assert_eq!(target.integer("v"), Some(2));
}

#[test]
fn keyed_accessors_are_top_level_only() {
    let reader = open(r#"{"a":{"b":{"target":1}}}"#);
    assert_eq!(reader.integer("target"), None);
    assert!(reader.find_object("a").is_some());
}

#[test]
fn has_field_distinguishes_present_null_from_absent() {
    let reader = open(r#"{"name":"Alice","nickname":null}"#);
    assert!(reader.has_field("name"));
    assert!(reader.has_field("nickname"));
    assert!(!reader.has_field("missing"));
    assert!(reader.is_null("nickname"));
    assert!(!reader.is_null("name"));
}

/// The source carries a `_or` accessor tier — `string_or(key, fallback)` and
/// friends — because three call sites had each grown their own `value_or`
/// wrapper family and needed one home. In Rust that home is `Option` itself, so
/// the tier is not ported. This is the same table of cases against std.
#[test]
// Exact float comparison is the assertion here, not an oversight: these values
// are written as literals and read back through the same parser, so anything
// other than bit equality is a defect.
#[allow(clippy::float_cmp)]
fn absent_and_malformed_fall_back_through_option() {
    let reader = open(r#"{"name":"Alice","age":30,"active":true,"ratio":1.5}"#);
    assert_eq!(reader.string("name").unwrap_or_default(), "Alice");
    assert_eq!(reader.integer("age").unwrap_or_default(), 30);
    assert!(reader.boolean("active").unwrap_or_default());
    assert_eq!(reader.floating("ratio").unwrap_or_default(), 1.5);

    assert_eq!(reader.string("missing").unwrap_or_default(), "");
    assert_eq!(reader.integer("missing").unwrap_or_default(), 0);
    assert!(!reader.boolean("missing").unwrap_or_default());
    assert_eq!(reader.floating("missing").unwrap_or_default(), 0.0);

    assert_eq!(
        reader.string("missing").unwrap_or("none".to_owned()),
        "none"
    );
    assert_eq!(reader.integer("missing").unwrap_or(-1), -1);
    assert!(reader.boolean("missing").unwrap_or(true));
    assert_eq!(reader.floating("missing").unwrap_or(9.0), 9.0);
}

#[test]
fn floating_rejects_non_number() {
    let reader = open(r#"{"name":"Alice","ratio":1.5}"#);
    assert_eq!(reader.floating("ratio"), Some(1.5));
    assert_eq!(reader.floating("name"), None);
}

#[test]
fn unescapes_string_escapes() {
    let reader = open(r#"{"text":"line1\nline2\t\"q\""}"#);
    assert_eq!(
        reader.string("text").as_deref(),
        Some("line1\nline2\t\"q\"")
    );
}

#[test]
fn number_token_returns_raw_numeric_token() {
    // The delimiter-bounded raw token, for exact-decimal money parsing where
    // f64 would round.
    let reader = open(r#"{"amount":-125.50,"qty":3,"signed":"+0.5"}"#);
    assert_eq!(reader.number_token("amount"), Some("-125.50"));
    assert_eq!(reader.number_token("qty"), Some("3"));
    assert_eq!(reader.number_token("missing"), None);

    // A trailing member is bounded by the closing brace rather than a comma.
    assert_eq!(open(r#"{"v":42}"#).number_token("v"), Some("42"));
}

#[test]
fn nested_navigation() {
    let reader = open(r#"{"obj":{"k":"v"},"arr":[{"n":1},{"n":2}],"nada":null}"#);

    let object = reader.find_object("obj").expect("obj");
    assert_eq!(object.raw(), r#"{"k":"v"}"#);

    let array = reader.find_array("arr").expect("arr");
    let objects = JsonReader::extract_array_objects(array);
    assert_eq!(objects.len(), 2);
    assert_eq!(objects[1].integer("n"), Some(2));

    assert!(reader.is_null("nada"));
    assert!(!reader.is_null("obj"));
    assert!(reader.raw().starts_with('{'));
}

#[test]
fn extract_array_elements_handles_mixed_types() {
    let reader = open(r#"{"mixed":[ {"n":1}, "str", 42, [1,2], true ]}"#);
    let array = reader.find_array("mixed").expect("mixed");
    let elements = JsonReader::extract_array_elements(array);
    assert_eq!(
        elements,
        vec![r#"{"n":1}"#, r#""str""#, "42", "[1,2]", "true"]
    );

    assert!(
        JsonReader::extract_array_elements("{}").is_empty(),
        "{:?}",
        JsonReader::extract_array_elements("{}")
    );
    assert!(
        JsonReader::extract_array_elements("[]").is_empty(),
        "{:?}",
        JsonReader::extract_array_elements("[]")
    );

    // A deeply nested element, and a brace inside a string, must each come back
    // as one whole slice.
    let deep = JsonReader::extract_array_elements(r#"[{"a":{"b":[1,2]}},{"s":"}]"}]"#);
    assert_eq!(deep, vec![r#"{"a":{"b":[1,2]}}"#, r#"{"s":"}]"}"#]);
}

#[test]
fn a_permissive_array_read_returns_the_prefix_before_a_wrong_element() {
    // The regression this file was missing. Every existing case put the wrong
    // element FIRST, so the prefix was empty and "returns empty" and "returns
    // the prefix" were indistinguishable — which is how two doc comments spent
    // three commits claiming the wrong one.
    let reader = open(r#"{"x5c":["a","b",1,"c"]}"#);
    assert_eq!(reader.string_array("x5c"), vec!["a", "b"]);
    assert_eq!(
        JsonReader::extract_string_array(r#"["a","b",1,"c"]"#),
        vec!["a", "b"]
    );

    let objects = open(r#"{"rows":[{"a":1},{"b":2},7,{"c":3}]}"#).object_array("rows");
    assert_eq!(objects.len(), 2);
}

#[test]
fn string_array_extracts_elements() {
    let reader = open(r#"{"x5c":[ "AAA", "B\/B", "c\"c" ],"alg":"ES256"}"#);
    assert_eq!(reader.string_array("x5c"), vec!["AAA", "B/B", "c\"c"]);

    // Absent key, non-array value, and array-of-non-strings all yield empty.
    assert!(
        reader.string_array("missing").is_empty(),
        "{:?}",
        reader.string_array("missing")
    );
    assert!(
        open(r#"{"x5c":"notarray"}"#).string_array("x5c").is_empty(),
        "{:?}",
        open(r#"{"x5c":"notarray"}"#).string_array("x5c")
    );
    assert!(
        open(r#"{"x5c":[1,2]}"#).string_array("x5c").is_empty(),
        "{:?}",
        open(r#"{"x5c":[1,2]}"#).string_array("x5c")
    );
}

#[test]
fn extract_string_array_decodes_bare_slice() {
    assert_eq!(
        JsonReader::extract_string_array(r#"[ "a", "b\/c" ]"#),
        vec!["a", "b/c"]
    );
    assert!(
        JsonReader::extract_string_array("[]").is_empty(),
        "{:?}",
        JsonReader::extract_string_array("[]")
    );
    assert!(
        JsonReader::extract_string_array("{}").is_empty(),
        "{:?}",
        JsonReader::extract_string_array("{}")
    );
    assert!(
        JsonReader::extract_string_array("[1,2]").is_empty(),
        "{:?}",
        JsonReader::extract_string_array("[1,2]")
    );

    // Consecutive escapes: `\\\\` decodes to two backslashes.
    assert_eq!(
        JsonReader::extract_string_array(r#"["\\\\","a\"b"]"#),
        vec!["\\\\", "a\"b"]
    );
}

#[test]
fn object_array_extracts_elements() {
    let reader = open(r#"{"arr":[ {"n":1}, {"n":2}, {"n":3} ],"k":"v"}"#);
    let objects = reader.object_array("arr");
    assert_eq!(objects.len(), 3);
    assert_eq!(objects[0].integer("n"), Some(1));
    assert_eq!(objects[2].integer("n"), Some(3));

    // A brace inside a string value must not split the element — the defect the
    // hand-rolled brace scanners this replaced all had.
    let braces = open(r#"{"items":[ {"name":"a}b"}, {"name":"c{d"} ]}"#);
    let items = braces.object_array("items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].string("name").as_deref(), Some("a}b"));
    assert_eq!(items[1].string("name").as_deref(), Some("c{d"));

    assert!(reader.object_array("missing").is_empty());
    assert!(open(r#"{"arr":"notarray"}"#).object_array("arr").is_empty());
}

#[test]
fn find_first_array_locates_array_slice_without_key() {
    // The keyless sibling of find_array: the first top-level array, skipping the
    // scalar members before it, without knowing which key holds the list.
    let paged = open(r#"{"limit":50,"offset":0,"requests":[ {"id":"a"}, {"id":"b"} ]}"#);
    let found = paged.find_first_array().expect("requests");
    let objects = JsonReader::extract_array_objects(found);
    assert_eq!(objects.len(), 2);
    assert_eq!(objects[0].string("id").as_deref(), Some("a"));
    assert_eq!(objects[1].string("id").as_deref(), Some("b"));

    // A string value containing "[" must not false-match: the walk is
    // structural, not a substring search.
    let decoy = open(r#"{"note":"see: [ref]","orders":[ {"n":1} ]}"#);
    let not_fooled = decoy.find_first_array().expect("orders");
    let orders = JsonReader::extract_array_objects(not_fooled);
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].integer("n"), Some(1));

    // Escaped quotes and braces inside object string values do not split them.
    let escaped = open(r#"{"items":[ {"name":"a\"}b"}, {"name":"c{d"} ]}"#);
    let items = JsonReader::extract_array_objects(escaped.find_first_array().expect("items"));
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].string("name").as_deref(), Some("a\"}b"));

    // No array at all is None, which is distinct from a present-but-empty array.
    assert_eq!(open(r#"{"a":1,"b":"x"}"#).find_first_array(), None);
    let empty = open(r#"{"orders":[]}"#).find_first_array().expect("orders");
    assert!(JsonReader::extract_array_objects(empty).is_empty());
}

#[test]
fn element_array_extracts_any_type_elements() {
    let reader = open(r#"{"mixed":[ {"n":1}, "str", 42, [1,2], true ],"k":"v"}"#);
    assert_eq!(
        reader.element_array("mixed"),
        vec![r#"{"n":1}"#, r#""str""#, "42", "[1,2]", "true"]
    );

    assert!(
        reader.element_array("missing").is_empty(),
        "{:?}",
        reader.element_array("missing")
    );
    let mixed = open(r#"{"mixed":"notarray"}"#).element_array("mixed");
    assert!(mixed.is_empty(), "{mixed:?}");
}

#[test]
fn extract_number_array_decodes_bare_slice() {
    assert_eq!(
        JsonReader::extract_number_array("[1, 2.5, -3]"),
        vec![1.0, 2.5, -3.0]
    );
    assert!(
        JsonReader::extract_number_array("[]").is_empty(),
        "{:?}",
        JsonReader::extract_number_array("[]")
    );
    assert!(
        JsonReader::extract_number_array("{}").is_empty(),
        "{:?}",
        JsonReader::extract_number_array("{}")
    );
}

#[test]
fn extract_string_map_enumerates_entries() {
    let entries = JsonReader::extract_string_map(r#"{"color":"red","size":"L"}"#);
    assert_eq!(
        entries,
        vec![
            ("color".to_owned(), "red".to_owned()),
            ("size".to_owned(), "L".to_owned()),
        ]
    );

    // Non-string values are skipped, and the enumeration continues past them —
    // including past whole nested containers.
    let keys = |json: &str| -> Vec<String> {
        JsonReader::extract_string_map(json)
            .into_iter()
            .map(|(key, _)| key)
            .collect()
    };
    assert_eq!(keys(r#"{"a":"1","n":5,"b":"2"}"#), vec!["a", "b"]);
    assert_eq!(
        keys(r#"{"a":"1","n":{"x":2},"arr":[1,2],"b":"2"}"#),
        vec!["a", "b"]
    );

    assert!(
        JsonReader::extract_string_map("[]").is_empty(),
        "{:?}",
        JsonReader::extract_string_map("[]")
    );
    assert!(
        JsonReader::extract_string_map("{}").is_empty(),
        "{:?}",
        JsonReader::extract_string_map("{}")
    );
}

#[test]
fn extract_object_fields_enumerates_raw_direct_values() {
    let fields =
        JsonReader::extract_object_fields(r#"{"a":"1","n":5,"o":{"status":"nested"},"b":[true]}"#);
    assert_eq!(
        fields,
        vec![
            ("a".to_owned(), r#""1""#),
            ("n".to_owned(), "5"),
            ("o".to_owned(), r#"{"status":"nested"}"#),
            ("b".to_owned(), "[true]"),
        ]
    );

    assert!(
        JsonReader::extract_object_fields("[]").is_empty(),
        "{:?}",
        JsonReader::extract_object_fields("[]")
    );
    assert!(
        JsonReader::extract_object_fields("{}").is_empty(),
        "{:?}",
        JsonReader::extract_object_fields("{}")
    );
}

#[test]
fn extract_number_map_enumerates_numeric_entries() {
    assert_eq!(
        JsonReader::extract_number_map(r#"{"width":173,"depth":41.1}"#),
        vec![("width".to_owned(), 173.0), ("depth".to_owned(), 41.1)]
    );
    assert_eq!(
        JsonReader::extract_number_map(r#"{"a":-3,"b":0}"#),
        vec![("a".to_owned(), -3.0), ("b".to_owned(), 0.0)]
    );

    // Strings, objects, and arrays are skipped; numeric siblings survive.
    assert_eq!(
        JsonReader::extract_number_map(r#"{"s":"x","w":80,"o":{"k":1},"arr":[1,2],"d":12.5}"#),
        vec![("w".to_owned(), 80.0), ("d".to_owned(), 12.5)]
    );

    assert!(
        JsonReader::extract_number_map("[]").is_empty(),
        "{:?}",
        JsonReader::extract_number_map("[]")
    );
    assert!(
        JsonReader::extract_number_map("{}").is_empty(),
        "{:?}",
        JsonReader::extract_number_map("{}")
    );
}

// --- detail:: traversal ------------------------------------------------------

#[test]
fn find_field_and_skip_value() {
    let input = r#"{"a":{"nested":1},"b":2}"#;
    let position = detail::find_field(input, 0, "a").expect("a");
    let position = detail::skip_ws(input, position);
    assert_eq!(input.as_bytes()[position], b'{');

    let end = detail::skip_json_value(input, position).expect("well formed");
    assert_eq!(&input[position..end], r#"{"nested":1}"#);
}

#[test]
fn find_field_ignores_nested_fields() {
    let input = r#"{"outer":{"status":"nested"},"status":"top"}"#;
    let position = detail::find_field(input, 0, "status").expect("top-level status");

    let mut value = String::new();
    detail::parse_json_string(input, position, &mut value).expect("a string");
    assert_eq!(value, "top");
}

#[test]
fn parse_string_number_and_bool() {
    let input = r#""hi", 42, true"#;
    let mut value = String::new();
    let position = detail::parse_json_string(input, 0, &mut value).expect("a string");
    assert_eq!(value, "hi");

    let (number, position) = detail::parse_json_number(input, position + 1).expect("a number");
    assert_eq!(number, 42);

    // `parse_json_bool` is public traversal API like its two siblings; the source
    // suite covers only those two.
    let (flag, end) = detail::parse_json_bool(input, position + 1).expect("a bool");
    assert!(flag);
    assert_eq!(end, input.len());

    let (flag, end) =
        detail::parse_json_bool("  false,", 0).expect("leading whitespace is skipped");
    assert!(!flag);
    assert_eq!(end, 7);

    // A prefix match is not a token. The source read `truexyz` as `true`, which
    // is the defect the reader's `boolean()` fixes; the primitive underneath has
    // to agree or the fix is only skin deep.
    for not_a_bool in ["truexyz", "tru", "TRUE", "falsey", "null", ""] {
        assert_eq!(detail::parse_json_bool(not_a_bool, 0), None, "{not_a_bool}");
    }
}

// --- Beyond the source suite -------------------------------------------------

/// The source deletes `JsonReader(std::string&&)` because a reader built from a
/// temporary reads freed memory — a shape that shipped twice in production
/// before the deleted overload turned it into a compile error. Rust needs no
/// such guard: the reader borrows, so the borrow checker rejects it. This is the
/// runtime half of that story — a reader outliving nothing, still correct.
#[test]
fn a_reader_borrows_its_document() {
    let owned = String::from(r#"{"k":"v"}"#);
    let reader = open(&owned);
    assert_eq!(reader.string("k").as_deref(), Some("v"));
    // `drop(owned)` here would not compile, which is the entire guarantee.
}

#[test]
fn the_reader_and_the_validator_agree_on_what_a_number_is() {
    // The source's two halves disagree here: `validate_json` enforces the RFC
    // grammar while the reader delegated to `from_chars`/`strtod`, which have
    // their own ideas. In the port they share one definition.
    for token in ["+42", "0x1p3", "NaN", "Infinity", "01", ".5", "1."] {
        let document = format!(r#"{{"v":{token}}}"#);
        assert!(
            validate_json(&document).is_err(),
            "{token} must not validate"
        );

        let reader = JsonReader::from_trusted_unchecked(&document);
        assert_eq!(reader.integer("v"), None, "{token} must not read as i64");
        assert_eq!(reader.floating("v"), None, "{token} must not read as f64");
    }
}

#[test]
fn a_number_too_large_for_f64_is_not_an_infinity() {
    // `1e400` is grammatically a JSON number, so it validates. Handing back an
    // infinity would produce a value no JSON writer can emit again, so the
    // reader reports it as unreadable instead.
    let document = r#"{"v":1e400}"#;
    assert!(validate_json(document).is_ok());
    assert_eq!(open(document).floating("v"), None);
}

#[test]
fn accessors_tolerate_whitespace_the_source_required_to_be_absent() {
    // Pretty-printed input reaches every accessor, including the ones the source
    // wrote as `front() == '['`.
    let reader = open("{\n  \"items\" : [\n    { \"n\" : 1 }\n  ]\n}");
    assert_eq!(reader.object_array("items")[0].integer("n"), Some(1));
    assert!(reader.find_first_array().is_some());
    assert!(reader.is_object());
}

#[test]
fn a_delimiter_where_a_number_belongs_terminates_rather_than_spins() {
    // Found by the seeded property tests: `[}` puts a delimiter where the scan
    // expects a token, so the token is empty and the cursor never advances.
    // Every extractor has to terminate on malformed input, not just reject it.
    assert!(
        JsonReader::extract_number_array("[}").is_empty(),
        "{:?}",
        JsonReader::extract_number_array("[}")
    );
    assert!(
        JsonReader::extract_number_array("[,]").is_empty(),
        "{:?}",
        JsonReader::extract_number_array("[,]")
    );
    assert_eq!(JsonReader::extract_number_array("[1,}2]"), vec![1.0]);
    assert!(
        JsonReader::extract_number_array("[}1]").is_empty(),
        "{:?}",
        JsonReader::extract_number_array("[}1]")
    );
}
