//! Writer tests, ported from `libs/foundation/json/test/json_test.cpp`.

use bc_json::{JsonReader, JsonWriteError, JsonWriter, json_error, json_ok, validate_json};

/// Builds a document and returns it, failing the test if the writer was poisoned.
fn build(body: impl FnOnce(&mut JsonWriter)) -> String {
    let mut writer = JsonWriter::new();
    body(&mut writer);
    writer
        .finish()
        .expect("no non-finite numbers in this fixture")
}

#[test]
fn builds_object_with_fields() {
    let json = build(|writer| {
        writer.object(|writer| {
            writer.field("id", "x1");
            writer.field("count", 3_i64);
            writer.field("ok", true);
        });
    });
    assert_eq!(json, r#"{"id":"x1","count":3,"ok":true}"#);
}

#[test]
fn finish_moves_the_completed_document() {
    let mut writer = JsonWriter::new();
    writer.object(|writer| writer.field("id", "x1"));
    assert_eq!(writer.finish().expect("finite"), r#"{"id":"x1"}"#);
}

#[test]
fn escapes_string_values() {
    let json = build(|writer| {
        writer.object(|writer| writer.field("text", String::from("a\"b\n")));
    });
    assert_eq!(json, r#"{"text":"a\"b\n"}"#);
}

#[test]
fn nested_array_of_objects() {
    let json = build(|writer| {
        writer.object(|writer| {
            writer.array("items", |writer| {
                writer.array_value(|writer| writer.object(|writer| writer.field("n", 1_i64)));
                writer.array_value(|writer| writer.object(|writer| writer.field("n", 2_i64)));
            });
        });
    });
    assert_eq!(json, r#"{"items":[{"n":1},{"n":2}]}"#);
}

#[test]
fn pretty_mode_indents() {
    let mut writer = JsonWriter::pretty();
    writer.object(|writer| {
        writer.field("id", "x1");
        writer.array("items", |writer| {
            writer.array_value(|writer| writer.object(|writer| writer.field("n", 1_i64)));
        });
        writer.array("empty", |_| {});
    });

    assert_eq!(
        writer.finish().expect("finite"),
        concat!(
            "{\n",
            "  \"id\": \"x1\",\n",
            "  \"items\": [\n",
            "    {\n",
            "      \"n\": 1\n",
            "    }\n",
            "  ],\n",
            "  \"empty\": []\n",
            "}"
        )
    );
}

#[test]
fn pretty_output_round_trips_through_the_reader() {
    let mut writer = JsonWriter::pretty();
    writer.object(|writer| {
        writer.field("name", "Alice");
        writer.field("age", 30_i64);
        writer.field("ratio", 1.5);
    });
    let pretty = writer.finish().expect("finite");

    let reader = JsonReader::new(validate_json(&pretty).expect("pretty output is valid JSON"));
    assert_eq!(reader.string("name").as_deref(), Some("Alice"));
    assert_eq!(reader.integer("age"), Some(30));
    assert_eq!(reader.floating("ratio"), Some(1.5));
}

#[test]
fn default_stays_compact() {
    let json = build(|writer| {
        writer.object(|writer| {
            writer.field("id", "x1");
            writer.nested_object("inner", |writer| writer.field("n", 1_i64));
        });
    });
    assert_eq!(json, r#"{"id":"x1","inner":{"n":1}}"#);
}

#[test]
fn f64_uses_the_shortest_round_tripping_form() {
    let emit = |value: f64| build(|writer| writer.object(|writer| writer.field("v", value)));

    assert_eq!(emit(0.555), r#"{"v":0.555}"#);
    assert_eq!(emit(0.1), r#"{"v":0.1}"#);
    assert_eq!(emit(12_345.678), r#"{"v":12345.678}"#);
    assert_eq!(emit(-2.5), r#"{"v":-2.5}"#);
    assert_eq!(emit(1.0), r#"{"v":1}"#);
    assert_eq!(emit(0.0), r#"{"v":0}"#);
    // Round magnitudes stay in fixed notation rather than going scientific, so
    // a wire format does not shift when a value happens to be whole.
    assert_eq!(emit(50.0), r#"{"v":50}"#);
    assert_eq!(emit(200.0), r#"{"v":200}"#);
    assert_eq!(emit(152.4), r#"{"v":152.4}"#);
    // Magnitudes a six-significant-digit default would have truncated.
    assert_eq!(emit(1_234_567.89), r#"{"v":1234567.89}"#);
}

#[test]
fn f64_round_trips_through_the_reader() {
    for value in [
        0.555,
        0.1,
        12_345.678,
        -2.5,
        1_234_567.89,
        9_999_999.999,
        0.123_456_789_012_345,
        f64::MAX,
        f64::MIN_POSITIVE,
        -0.0,
    ] {
        let json = build(|writer| writer.object(|writer| writer.field("v", value)));
        let reader = JsonReader::new(
            validate_json(&json).unwrap_or_else(|error| panic!("{json} is invalid: {error}")),
        );
        assert_eq!(reader.floating("v"), Some(value), "emitted: {json}");
    }
}

#[test]
fn empty_container_still_comma_separates_its_sibling() {
    // The save/restore separator discipline: an empty container must not eat the
    // comma its sibling needs.
    let json = build(|writer| {
        writer.object(|writer| {
            writer.array("items", |_| {});
            writer.nested_object("meta", |_| {});
            writer.field("total", 3_i64);
        });
    });
    assert_eq!(json, r#"{"items":[],"meta":{},"total":3}"#);
}

#[test]
fn superset_extras() {
    let json = build(|writer| {
        writer.object(|writer| {
            writer.string_array("tags", ["a", "b"]);
            writer.optional_field("present", Some(7_i64));
            writer.optional_field("absent", None::<i64>);
            writer.array("nums", |writer| {
                writer.array_element(1_i64);
                writer.array_element(2.5);
            });
        });
    });
    assert_eq!(json, r#"{"tags":["a","b"],"present":7,"nums":[1,2.5]}"#);
}

#[test]
fn bare_array_of_bools() {
    let json = build(|writer| {
        writer.bare_array(|writer| {
            for value in [true, false, true] {
                writer.array_element(value);
            }
        });
    });
    assert_eq!(json, "[true,false,true]");
}

#[test]
fn raw_field_embeds_verbatim() {
    let json = build(|writer| {
        writer.object(|writer| {
            writer.field("id", "abc");
            writer.raw_field_unchecked("event", r#"{"type":"x","n":3}"#);
            writer.field("ok", true);
        });
    });
    assert_eq!(json, r#"{"id":"abc","event":{"type":"x","n":3},"ok":true}"#);
}

#[test]
fn json_ok_and_json_error() {
    assert_eq!(json_ok(), r#"{"ok":true}"#);
    assert_eq!(json_error("boom"), r#"{"error":"boom"}"#);
}

// --- Beyond the source suite -------------------------------------------------

#[test]
fn a_non_finite_number_is_reported_rather_than_written() {
    // The source emits `nan`/`inf` verbatim and produces invalid JSON. Here the
    // writer is poisoned and says so once, at the end.
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut writer = JsonWriter::new();
        writer.object(|writer| writer.field("v", value));
        assert_eq!(writer.as_str(), Err(JsonWriteError::NonFiniteNumber));
        assert_eq!(writer.finish(), Err(JsonWriteError::NonFiniteNumber));
    }
}

#[test]
fn extreme_magnitudes_fall_back_to_exponent_notation() {
    // Rust's `Display` for f64 never uses an exponent, so 1e300 would otherwise
    // print as 301 digits. Past the source's 32-character budget the port
    // switches forms — and the result is still a valid JSON number.
    let json = build(|writer| writer.object(|writer| writer.field("v", 1e300)));
    assert_eq!(json, r#"{"v":1e300}"#);
    assert!(validate_json(&json).is_ok());

    let tiny = build(|writer| writer.object(|writer| writer.field("v", 5e-324)));
    assert!(tiny.contains('e'), "{tiny}");
    assert!(validate_json(&tiny).is_ok());
}

#[test]
fn everything_the_writer_emits_validates() {
    // The writer and the validator are the two halves of this crate's wire
    // contract; nothing else checks that they agree.
    let json = build(|writer| {
        writer.object(|writer| {
            writer.field("text", "quotes \" backslash \\ control \u{1} tab \t");
            writer.field("unicode", "é 東 𝄞");
            writer.field("negative", -42_i64);
            writer.field("wide", u64::MAX);
            writer.field("float", -0.000_001_5);
            writer.field_null("nothing");
            writer.nullable_field("explicit", None::<&str>);
            writer.optional_field_null("conditional", true);
            writer.array("mixed", |writer| {
                writer.array_element("a");
                writer.array_element(1_i64);
                writer.array_element(true);
                writer.array_null();
                writer.array_value(|writer| writer.bare_array(|_| {}));
            });
            writer.string_array("tags", Vec::<String>::new());
        });
    });

    let document =
        validate_json(&json).unwrap_or_else(|error| panic!("{json} is invalid: {error}"));
    let reader = JsonReader::new(document);
    assert_eq!(reader.integer("wide"), None, "u64::MAX does not fit an i64");
    assert!(reader.is_null("nothing"));
    assert!(reader.is_null("explicit"));
    assert!(reader.is_null("conditional"));
    assert_eq!(reader.element_array("mixed").len(), 5);
}

/// The writer's half of the duplicate-key contract.
///
/// `validate_json` rejects duplicate keys, which is what makes the reader's
/// first-match lookup unambiguous — so a writer that can emit a pair is a hole in
/// that contract from the inside. Detection is debug-only, and this whole test
/// therefore only exists in a debug build; `cargo test` is one.
#[cfg(debug_assertions)]
#[test]
fn a_duplicate_key_poisons_the_writer() {
    let mut writer = JsonWriter::new();
    writer.object(|writer| {
        writer.field("a", 1_i64);
        writer.field("b", 2_i64);
        writer.field("a", 3_i64);
    });
    assert_eq!(writer.as_str(), Err(JsonWriteError::DuplicateKey));
    assert_eq!(writer.finish(), Err(JsonWriteError::DuplicateKey));

    // Every keyed form goes through the same check, including the escape hatch.
    let mut mixed = JsonWriter::new();
    mixed.object(|writer| {
        writer.field_null("k");
        writer.raw_field_unchecked("k", "[]");
    });
    assert_eq!(mixed.finish(), Err(JsonWriteError::DuplicateKey));

    // Scoping: the same key in a sibling object, or in a nested one, is not a
    // duplicate — an object's keys are its own.
    let mut scoped = JsonWriter::new();
    scoped.object(|writer| {
        writer.nested_object("a", |writer| writer.field("k", 1_i64));
        writer.nested_object("b", |writer| writer.field("k", 2_i64));
        writer.array("items", |writer| {
            writer.array_value(|writer| writer.object(|writer| writer.field("k", 3_i64)));
            writer.array_value(|writer| writer.object(|writer| writer.field("k", 4_i64)));
        });
    });
    let json = scoped.finish().expect("no key repeats within one object");
    assert!(validate_json(&json).is_ok(), "{json}");

    // The first cause is the one reported, not the last.
    let mut both = JsonWriter::new();
    both.object(|writer| {
        writer.field("k", 1_i64);
        writer.field("k", 2_i64);
        writer.field("v", f64::NAN);
    });
    assert_eq!(both.finish(), Err(JsonWriteError::DuplicateKey));
}

#[test]
fn lengths_and_indices_write_without_a_cast() {
    // JSON numbers have no width, so `usize`/`isize` are scalars like any other.
    // Without them the README's own example needs an `as i64`.
    let items = ["a", "b", "c"];
    let json = build(|writer| {
        writer.object(|writer| {
            writer.field("count", items.len());
            writer.field("offset", -1_isize);
        });
    });
    assert_eq!(json, r#"{"count":3,"offset":-1}"#);
}

#[test]
fn control_characters_are_escaped_as_the_shortest_legal_form() {
    let json = build(|writer| {
        writer.object(|writer| writer.field("v", "\u{1f}\u{8}\u{c}"));
    });
    assert_eq!(json, r#"{"v":"\u001f\b\f"}"#);
    assert!(validate_json(&json).is_ok());
}

// A NUL is the one control the validator refuses as an escape, so the writer
// emits the replacement character for it and its output still validates.
#[test]
fn a_nul_is_written_as_the_replacement_character() {
    let json = build(|writer| {
        writer.object(|writer| writer.field("s", "a\0b"));
    });
    assert_eq!(json, "{\"s\":\"a\u{FFFD}b\"}");
    assert!(validate_json(&json).is_ok());
}
