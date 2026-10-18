//! The two emit shapes every caller of [`JsonWriter`] otherwise re-spells.
//!
//! Neither is specific to any one schema: both depend on the writer and on a
//! row's own [`WriteJson`], and that is all, so they belong beside the writer
//! where every caller can reach them.
//!
//! The nesting in [`emit_rows`] is load-bearing rather than decorative.
//! [`JsonWriter::array_value`] is the call that writes the **comma** between
//! elements, while [`JsonWriter::object`] writes `{` immediately and never
//! separates. An element emitted as a bare `writer.object(…)` therefore
//! produces `[{…}{…}]` — invalid JSON that a brace-matching reader parses back
//! perfectly, so a round trip through this crate would not notice. In the
//! source library two hand-rolled loops had it wrong on the wire for exactly
//! that reason, which is why the shape is a function here rather than a habit.

use crate::JsonWriter;

/// A value that can write itself as the body of a JSON object.
///
/// The source library states this obligation as an unwritten convention — any
/// type with a `write_json` member satisfies its function templates, and a type
/// that spells it wrong fails to compile at each call site. A trait says the
/// same thing once, and names it.
///
/// Implementors write **fields only**: the enclosing `{` and `}` belong to
/// whoever called them, which is what lets the same body serve a standalone
/// document and one element of an array.
pub trait WriteJson {
    /// Writes this value's fields into `writer`.
    fn write_json(&self, writer: &mut JsonWriter);
}

/// Serializes one value as a complete JSON document.
///
/// # Errors
///
/// Propagates [`JsonWriter::finish`]'s refusal — a non-finite number, or a
/// duplicate key in a debug build.
pub fn serialize_object<T: WriteJson + ?Sized>(value: &T) -> Result<String, crate::JsonWriteError> {
    let mut writer = JsonWriter::new();
    writer.object(|writer| value.write_json(writer));
    writer.finish()
}

/// Writes `key` as an array whose elements are `rows`, each an object.
///
/// The one correct spelling of the comma-separated array of objects — see this
/// module's own documentation for what the alternative silently produces.
pub fn emit_rows<'a, T: WriteJson + 'a>(
    writer: &mut JsonWriter,
    key: &str,
    rows: impl IntoIterator<Item = &'a T>,
) {
    writer.array(key, |writer| {
        for row in rows {
            writer.array_value(|writer| {
                writer.object(|writer| row.write_json(writer));
            });
        }
    });
}
