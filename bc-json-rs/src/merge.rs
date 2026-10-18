//! Override-merge of one flat-keyed JSON object onto another, preserving the
//! base verbatim.
//!
//! The motivating caller emits a configuration document owned by an external
//! tool: hundreds of keys, of which the caller means to set a few dozen.
//! Authoring only those few *is* a partial config, and a partial config is what
//! the tool then read as missing everything else. The safe shape — take a
//! complete tool-authored base and substitute into it — is not specific to any
//! one tool; it is "apply a patch document to a config document", which a PATCH
//! handler, a settings override, and a profile-with-defaults all want.
//!
//! **Verbatim is the load-bearing property.** Base values are re-emitted as the
//! exact byte slices they arrived as, so the keys the caller does not touch pass
//! through without this code understanding a single one of them — no round trip
//! through a value model that could normalize a number, reorder a nested
//! object, or drop a shape it did not anticipate. Key order is the base's, with
//! appended keys last, so a diff against the original base is minimal.
//!
//! **Keys are the one deliberate exception to verbatim.**
//! [`JsonReader::extract_object_fields`] *decodes* them, so a base key
//! `"a\"b"` arrives as the bytes `a"b`; they are re-emitted re-escaped through
//! the writer's own escaper rather than pasted raw. Pasted raw, a decoded quote
//! would terminate the key string and produce invalid JSON that validation had
//! already blessed on the way in. The cost is spelling normalization only.

use bc_error::AnyError;

use crate::kinds;
use crate::{JsonReader, JsonWriter, validate_json};

/// What a merge produced, and which override keys landed how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectMergeResult {
    /// The merged document.
    pub json: String,
    /// Override keys the base already carried — the intended case.
    pub replaced: Vec<String>,
    /// Override keys the base did **not** carry. Reported rather than refused,
    /// because only the caller knows whether inventing a key is legitimate
    /// here: a caller holding a tool-authored export usually refuses them,
    /// since a key absent from that export is a key it made up.
    pub appended: Vec<String>,
}

/// Substitutes `overrides` into `base`, both of which must be JSON objects.
///
/// # Errors
///
/// Refuses a side that is not well-formed JSON, carrying the validation kind
/// and byte offset, and a side that is well-formed but is not an object.
///
/// The validation is not defensive boilerplate — it is required for
/// correctness, because [`JsonReader::extract_object_fields`] **fails open**:
/// its parse failures end enumeration and return the fields found so far with
/// no error channel, so a truncated base would merge "successfully" into a
/// document missing every key after the truncation point. That is the precise
/// failure this function exists to prevent, so it is checked before enumeration
/// rather than trusted. Validation also rejects duplicate keys, the one case
/// where the lazy keyed accessors and the enumerator would legitimately
/// disagree about a document.
pub fn merge_object_overrides(base: &str, overrides: &str) -> Result<ObjectMergeResult, AnyError> {
    check_object(base, "merge base")?;
    check_object(overrides, "merge overrides")?;

    let base_fields = JsonReader::extract_object_fields(base);
    let override_fields = JsonReader::extract_object_fields(overrides);

    let mut result = ObjectMergeResult {
        json: String::new(),
        replaced: Vec::new(),
        appended: Vec::new(),
    };
    let mut consumed = vec![false; override_fields.len()];
    let mut writer = JsonWriter::pretty();

    writer.object(|writer| {
        for (key, raw_value) in &base_fields {
            match override_fields.iter().position(|(name, _)| name == key) {
                Some(index) => {
                    consumed[index] = true;
                    result.replaced.push(key.clone());
                    writer.raw_field_unchecked(key, override_fields[index].1);
                }
                None => writer.raw_field_unchecked(key, raw_value),
            }
        }
        for (index, (key, raw_value)) in override_fields.iter().enumerate() {
            if consumed[index] {
                continue;
            }
            result.appended.push(key.clone());
            writer.raw_field_unchecked(key, raw_value);
        }
    });

    result.json = writer.finish().map_err(|error| {
        // Unreachable in practice: every value written here came out of a
        // validated document. `debug_assert!` plus a real error rather than a
        // silent fallback, because a merge that could not be written must not
        // report a document it did not produce.
        debug_assert!(false, "merging validated documents produced invalid JSON");
        AnyError::from_detail(kinds::EMIT_FAILED, error.to_string())
    })?;
    Ok(result)
}

/// Refuses a side that is not a well-formed JSON *object*.
///
/// Two failures with two different meanings: malformed input carries the
/// validator's own kind and offset, while a well-formed non-object is a type
/// mismatch. Folding them would tell the caller to look for a syntax error in a
/// document that has none.
fn check_object(json: &str, subject: &str) -> Result<(), AnyError> {
    let validated = validate_json(json).map_err(AnyError::from)?;
    if JsonReader::new(validated).is_object() {
        return Ok(());
    }
    Err(AnyError::from_detail(
        kinds::TYPE_MISMATCH,
        format!("{subject} is not a JSON object"),
    )
    .with_metadata("subject", subject.into()))
}
