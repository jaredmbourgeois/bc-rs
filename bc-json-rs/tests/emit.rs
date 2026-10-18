//! The emit shapes, ported from `json_read_test.cpp`'s `JsonEmit` cases.

use bc_json::{JsonWriter, WriteJson, emit_rows, serialize_object, validate_json};

struct Row {
    id: i64,
    name: &'static str,
}

impl WriteJson for Row {
    fn write_json(&self, writer: &mut JsonWriter) {
        writer.field("id", self.id);
        writer.field("name", self.name);
    }
}

#[test]
fn emit_rows_separates_elements_with_a_comma_two_rows_prove_it() {
    // Two rows, because one row cannot show a missing separator. The defect
    // this shape exists to prevent -- a bare `object()` per element -- emits
    // `[{…}{…}]`, which a brace-matching reader parses back perfectly, so only
    // a validator catches it.
    let rows = [Row { id: 1, name: "a" }, Row { id: 2, name: "b" }];

    let mut writer = JsonWriter::new();
    writer.object(|writer| emit_rows(writer, "rows", &rows));
    let json = writer.finish().expect("the rows are writable");

    assert!(json.contains("},{"), "the elements are comma-separated");
    validate_json(&json).expect("emit_rows produces valid JSON");
}

#[test]
fn serialize_object_opens_and_closes_the_document() {
    let json = serialize_object(&Row {
        id: 7,
        name: "solo",
    })
    .expect("the row is writable");

    assert!(json.starts_with('{') && json.ends_with('}'));
    let reader = bc_json::JsonReader::new(validate_json(&json).expect("valid JSON"));
    assert_eq!(reader.integer("id"), Some(7));
    assert_eq!(reader.string("name").as_deref(), Some("solo"));
}

#[test]
fn an_empty_row_set_still_emits_a_well_formed_array() {
    let rows: [Row; 0] = [];

    let mut writer = JsonWriter::new();
    writer.object(|writer| emit_rows(writer, "rows", &rows));
    let json = writer.finish().expect("an empty array is writable");

    validate_json(&json).expect("an empty row set is still valid JSON");
    assert!(json.contains(r#""rows":[]"#));
}
