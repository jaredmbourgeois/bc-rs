//! The serde bridge: `to_string` / `from_str` over the native primitives.
//!
//! Nothing here is derived from the C++ source, which has no serde. What it
//! checks instead is that the bridge and the native API cannot disagree: the
//! same escaper, the same number grammar, the same validator, and the same
//! duplicate-key rejection.

#![cfg(feature = "serde")]

use std::collections::BTreeMap;

use bc_json::{
    JsonDeserializeError, JsonSerializeError, JsonWriteError, from_str, from_str_unchecked,
    from_validated, to_string, to_string_pretty, validate_json,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Order {
    id: String,
    total: f64,
    quantity: u32,
    rush: bool,
    note: Option<String>,
    tags: Vec<String>,
}

fn sample() -> Order {
    Order {
        id: "a\"1\n".to_owned(),
        total: 12.5,
        quantity: 3,
        rush: true,
        note: None,
        tags: vec!["é".to_owned(), "東".to_owned()],
    }
}

#[test]
fn a_struct_round_trips() {
    let json = to_string(&sample()).expect("finite and unique-keyed");
    assert_eq!(
        json,
        r#"{"id":"a\"1\n","total":12.5,"quantity":3,"rush":true,"note":null,"tags":["é","東"]}"#
    );
    assert_eq!(from_str::<Order>(&json).expect("round trip"), sample());
}

#[test]
fn everything_the_serializer_emits_validates() {
    // The same two-sided contract the native writer is held to. The serializer
    // reaches the writer's escaper by a different route, so it gets its own
    // check rather than inheriting the writer's.
    let json = to_string(&sample()).expect("valid");
    assert!(validate_json(&json).is_ok(), "{json}");

    let pretty = to_string_pretty(&sample()).expect("valid");
    assert!(validate_json(&pretty).is_ok(), "{pretty}");
    assert!(pretty.contains('\n'), "{pretty}");
    // Both modes carry the same data, whatever the whitespace.
    assert_eq!(
        from_str::<Order>(&pretty).expect("pretty parses"),
        from_str::<Order>(&json).expect("compact parses")
    );
}

#[test]
fn strings_without_escapes_are_borrowed() {
    #[derive(Debug, PartialEq, Deserialize)]
    struct Borrowing<'a> {
        plain: &'a str,
    }

    let document = r#"{"plain":"no escapes here"}"#;
    let borrowed: Borrowing<'_> = from_str(document).expect("borrows");
    assert_eq!(borrowed.plain, "no escapes here");
    // The proof it borrowed rather than copied: the slice points into `document`.
    assert!(
        document.as_ptr() <= borrowed.plain.as_ptr()
            && borrowed.plain.as_ptr() < document.as_ptr().wrapping_add(document.len())
    );
}

#[test]
fn strings_with_escapes_cannot_be_borrowed_and_say_so() {
    #[derive(Debug, Deserialize)]
    struct Borrowing<'a> {
        #[expect(dead_code, reason = "this type exists to fail to deserialize")]
        escaped: &'a str,
    }

    #[derive(Debug, PartialEq, Deserialize)]
    struct Owning {
        escaped: String,
    }

    // The decoded text does not exist in the input, so there is nothing to point
    // at. Serde reports that as a failure to borrow rather than silently copying
    // into a temporary that dies.
    let error = from_str::<Borrowing<'_>>(r#"{"escaped":"a\nb"}"#).unwrap_err();
    assert!(matches!(error, JsonDeserializeError::Message(_)), "{error}");

    // An owned target reads the same document fine.
    let owned: Owning = from_str(r#"{"escaped":"a\nb"}"#).expect("owns");
    assert_eq!(owned.escaped, "a\nb");
}

#[test]
fn the_deserializer_shares_the_validator_s_number_grammar() {
    // Every token a permissive parser would take and this crate will not. The
    // point is that `from_str` reaches the same answer as `validate_json`,
    // because it reaches the same code.
    for token in ["01", "+1", ".5", "1.", "1e", "NaN", "Infinity", "0x10"] {
        let document = format!(r#"{{"v":{token}}}"#);
        assert!(validate_json(&document).is_err(), "{document}");
        assert!(
            from_str::<BTreeMap<String, f64>>(&document).is_err(),
            "{document}"
        );
        assert!(
            from_str_unchecked::<BTreeMap<String, f64>>(&document).is_err(),
            "{document} through the unchecked path"
        );
    }
}

#[test]
fn a_number_the_grammar_allows_but_f64_cannot_hold_is_refused() {
    // `1e400` validates — it is a well-formed JSON number — and then does not
    // fit. `JsonReader::floating` returns `None` for the same input; here it is
    // an error, because a deserializer has no `None` to return.
    let document = r#"{"v":1e400}"#;
    assert!(validate_json(document).is_ok());
    let error = from_str::<BTreeMap<String, f64>>(document).unwrap_err();
    assert!(
        matches!(error, JsonDeserializeError::NumberOutOfRange { .. }),
        "{error}"
    );
}

#[test]
fn duplicate_keys_are_rejected_by_from_str_and_accepted_by_the_hatch() {
    let document = r#"{"a":1,"a":2}"#;

    // The validating path refuses it, which is this crate's whole differentiator.
    let error = from_str::<BTreeMap<String, i64>>(document).unwrap_err();
    assert!(matches!(error, JsonDeserializeError::Invalid(_)), "{error}");

    // The named hatch does what serde does: last key wins. Stated as a test
    // because it is the observable difference between the two entry points.
    let permissive: BTreeMap<String, i64> = from_str_unchecked(document).expect("no validation");
    assert_eq!(permissive.get("a"), Some(&2));
}

#[test]
fn a_validated_document_is_not_validated_twice() {
    // The typestate's payoff on the serde side: proof of validation is a value,
    // so a caller who validated at the boundary hands it along.
    let document =
        validate_json(r#"{"id":"a1","total":1.0,"quantity":1,"rush":false,"note":null,"tags":[]}"#)
            .expect("valid");
    let order: Order = from_validated(document).expect("deserializes");
    assert_eq!(order.id, "a1");
}

#[test]
fn a_map_key_written_as_a_string_reads_back_as_its_type() {
    // The serializer stringifies integer keys because JSON object keys are
    // strings; the deserializer has to invert exactly that.
    let mut counts = BTreeMap::new();
    counts.insert(7_u32, "seven");
    counts.insert(11_u32, "eleven");

    let json = to_string(&counts).expect("valid");
    assert_eq!(json, r#"{"7":"seven","11":"eleven"}"#);
    assert_eq!(
        from_str::<BTreeMap<u32, String>>(&json).expect("round trip"),
        BTreeMap::from([(7, "seven".to_owned()), (11, "eleven".to_owned())])
    );
}

#[test]
fn a_map_key_json_cannot_spell_is_refused_rather_than_guessed_at() {
    // A float or a struct has no round-trippable string form. Coercing one
    // anyway would put keys on the wire that nothing reads back, so the key
    // serializer refuses instead of guessing.
    struct KeyedBy<K>(K);

    impl<K: Serialize> Serialize for KeyedBy<K> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeMap;
            let mut map = serializer.serialize_map(Some(1))?;
            map.serialize_entry(&self.0, &1_i64)?;
            map.end()
        }
    }

    #[derive(Serialize)]
    struct Composite {
        inner: i32,
    }

    assert_eq!(
        to_string(&KeyedBy(1.5_f64)).unwrap_err(),
        JsonSerializeError::NonStringKey
    );
    assert_eq!(
        to_string(&KeyedBy(true)).unwrap_err(),
        JsonSerializeError::NonStringKey
    );
    assert_eq!(
        to_string(&KeyedBy(Composite { inner: 1 })).unwrap_err(),
        JsonSerializeError::NonStringKey
    );

    // The forms that do have one are written, and validate.
    for accepted in [
        to_string(&KeyedBy("text")),
        to_string(&KeyedBy('c')),
        to_string(&KeyedBy(-7_i32)),
    ] {
        let json = accepted.expect("a spellable key");
        assert!(validate_json(&json).is_ok(), "{json}");
    }
}

#[test]
fn non_finite_floats_fail_the_document_rather_than_corrupting_it() {
    // The source's defect, reached through serde this time: the writer is
    // poisoned and `to_string` reports it, so no caller ever sees `inf` on a wire.
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let error = to_string(&vec![value]).unwrap_err();
        assert_eq!(
            error,
            JsonSerializeError::Write(JsonWriteError::NonFiniteNumber)
        );
    }
}

#[cfg(debug_assertions)]
#[test]
fn a_serialize_impl_that_repeats_a_key_is_caught_in_debug() {
    // A `HashMap` cannot produce this, but a hand-written `Serialize` can — and
    // a map is exactly where the writer's debug-only duplicate-key check has to
    // reach, because the keys are caller-driven rather than field names.
    struct RepeatsAKey;

    impl Serialize for RepeatsAKey {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeMap;
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("k", &1)?;
            map.serialize_entry("k", &2)?;
            map.end()
        }
    }

    assert_eq!(
        to_string(&RepeatsAKey).unwrap_err(),
        JsonSerializeError::Write(JsonWriteError::DuplicateKey)
    );
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
enum Shape {
    Empty,
    Radius(f64),
    Pair(i64, i64),
    Rect { width: i64, height: i64 },
}

#[test]
fn every_enum_representation_round_trips() {
    let cases = [
        (Shape::Empty, r#""Empty""#),
        (Shape::Radius(1.5), r#"{"Radius":1.5}"#),
        (Shape::Pair(1, 2), r#"{"Pair":[1,2]}"#),
        (
            Shape::Rect {
                width: 3,
                height: 4,
            },
            r#"{"Rect":{"width":3,"height":4}}"#,
        ),
    ];

    for (value, expected) in cases {
        let json = to_string(&value).expect("valid");
        assert_eq!(json, expected);
        assert!(validate_json(&json).is_ok(), "{json}");
        assert_eq!(from_str::<Shape>(&json).expect("round trip"), value);
    }
}

#[test]
fn flatten_and_unknown_fields_work() {
    // `#[serde(flatten)]` buffers through `deserialize_any`, so it only works if
    // the format is fully self-describing — this is the test that says it is.
    #[derive(Debug, PartialEq, Deserialize)]
    struct Envelope {
        kind: String,
        #[serde(flatten)]
        rest: BTreeMap<String, i64>,
    }

    #[derive(Debug, PartialEq, Deserialize)]
    struct JustKind {
        kind: String,
    }

    let envelope: Envelope =
        from_str(r#"{"kind":"metric","a":1,"b":2}"#).expect("flatten deserializes");
    assert_eq!(envelope.kind, "metric");
    assert_eq!(
        envelope.rest,
        BTreeMap::from([("a".into(), 1), ("b".into(), 2)])
    );

    // An unknown field's value must be stepped over, whatever its shape.
    let skipped: JustKind =
        from_str(r#"{"ignored":{"deep":[1,{"x":"}"},null]},"kind":"metric","also":"gone"}"#)
            .expect("skips unknown values");
    assert_eq!(skipped.kind, "metric");
}

#[test]
fn trailing_input_is_an_error_on_both_paths() {
    assert!(from_str::<i64>("1 2").is_err());
    let error = from_str_unchecked::<i64>("1 2").unwrap_err();
    assert!(
        matches!(error, JsonDeserializeError::TrailingInput { .. }),
        "{error}"
    );
}

#[test]
fn the_unchecked_path_errors_rather_than_panicking_on_malformed_input() {
    // The escape hatch is safe, just unguaranteed. Every one of these is a shape
    // the validator would have caught.
    for malformed in [
        "",
        "{",
        "[",
        "{\"a\"",
        "{\"a\":",
        "[1,",
        "\"unterminated",
        "tru",
        "{\"a\":1,}",
        "[,]",
        "\u{1}",
        "{\"a\":1}}",
    ] {
        let _ = from_str_unchecked::<serde_json::Value>(malformed);
    }
}

#[test]
fn f32_keeps_its_own_shortest_form() {
    // Widening to f64 before formatting would print 0.1_f32 as 0.10000000149011612.
    let json = to_string(&0.1_f32).expect("finite");
    assert_eq!(json, "0.1");
    assert!((from_str::<f32>(&json).expect("round trip") - 0.1_f32).abs() < f32::EPSILON);
}
