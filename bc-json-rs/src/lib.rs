//! Strict JSON primitives: a validator, a reader, and a writer.
//!
//! Ported from `bourgeois_concern`'s `libs/foundation/json`: the same API and
//! behaviour, stricter where the README says so.
//!
//! # The two halves, and why they stay two halves
//!
//! [`validate_json`] is a strict whole-document gate. [`JsonReader`] is a lazy
//! per-key scanner that builds no value tree and returns slices of the caller's
//! buffer. They are separate on purpose, and the separation is what makes the
//! reader sound rather than merely cheap:
//!
//! > The reader returns the **first** member matching a key. The validator
//! > rejects duplicate decoded keys. So a document that reaches the reader has
//! > exactly one member per key, and "first" is indistinguishable from any
//! > other choice.
//!
//! In C++ that composition was a documented convention, honored at 92 of 475
//! call sites. Here it is a type: [`validate_json`] returns a [`Validated`], and
//! [`JsonReader::new`] takes one. The escape hatch
//! ([`JsonReader::from_trusted_unchecked`]) is named so it cannot be reached by
//! accident.
//!
//! ```
//! // Validate at the boundary, then read.
//! let body = r#"{"id":"a1","items":[{"n":1},{"n":2}]}"#;
//! let reader = bc_json::JsonReader::new(bc_json::validate_json(body)?);
//!
//! assert_eq!(reader.string("id").as_deref(), Some("a1"));
//! let items = reader.object_array("items");
//! assert_eq!(items.len(), 2);
//! assert_eq!(items[1].integer("n"), Some(2));
//!
//! // Build a reply.
//! let mut writer = bc_json::JsonWriter::new();
//! writer.object(|writer| {
//!     writer.field("ok", true);
//!     writer.field("count", items.len());
//! });
//! assert_eq!(writer.finish()?, r#"{"ok":true,"count":2}"#);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # What "strict" means here
//!
//! [`validate_json`] rejects, where a permissive parser would not:
//!
//! - **duplicate object keys** — most parsers silently keep one of them
//! - **nesting past [`MAX_JSON_NESTING_DEPTH`]** — a stack-exhaustion limit
//! - **trailing input** after a complete document
//! - numbers outside the RFC 8259 grammar: `01`, `+1`, `.5`, `1.`, `NaN`, `0x10`
//! - unpaired surrogates and unknown escapes
//! - raw control bytes inside strings, and (via [`validate_json_bytes`])
//!   anything that is not valid UTF-8
//!
//! That list is the point of the crate: it is what an untrusted boundary needs
//! and what a general-purpose parser deliberately does not do.

#![deny(missing_docs)]
// The serde section documents API that only exists with the feature on, so it is
// attached with the feature rather than written unconditionally — otherwise the
// default build's rustdoc has links pointing at items it did not compile.
#![cfg_attr(
    feature = "serde",
    doc = r"
# Serde

With the `serde` feature, this crate is also a serde **format**: [`to_string`] /
[`to_string_pretty`] and [`from_str`] work for any `Serialize` / `Deserialize`
type, over the same escaper, number grammar, and validator as the native API — so
the two halves cannot disagree about what JSON is.

[`from_str`] validates first, so a deserialized document has already passed every
check listed above, duplicate keys included. [`from_validated`] takes the proof
directly, for a caller who validated at the boundary; [`from_str_unchecked`] is
the named hatch for input that has not been validated at all.
"
)]

pub mod detail;
pub mod kinds;

mod emit;
mod merge;
mod number;
mod read;
mod reader;
mod validate;
mod writer;

#[cfg(feature = "serde")]
mod de;
#[cfg(feature = "serde")]
mod ser;

pub use emit::{WriteJson, emit_rows, serialize_object};
pub use merge::{ObjectMergeResult, merge_object_overrides};
pub use read::{JsonReadError, JsonReadErrorKind, NullReads};
pub use reader::JsonReader;
pub use validate::{
    JsonValidationError, JsonValidationErrorKind, LineColumn, MAX_JSON_NESTING_DEPTH,
    VALIDATION_CATEGORY, Validated, validate_json, validate_json_bytes,
};
pub use writer::{JsonScalar, JsonWriteError, JsonWriter, json_error, json_ok};

#[cfg(feature = "serde")]
pub use de::{JsonDeserializeError, from_str, from_str_unchecked, from_validated};
#[cfg(feature = "serde")]
pub use ser::{JsonSerializeError, to_string, to_string_pretty};

// Compiles every ```rust block in README.md as a doctest, so the documented
// examples cannot drift from the API they document.
//
// Gated on `serde` because one of those examples is the serde bridge, which does
// not exist in a default build. `scripts/verify.sh` runs the doctests twice, once
// with all features, so every README example is still compiled on every run.
#[cfg(all(doctest, feature = "serde"))]
#[doc = include_str!("../README.md")]
struct ReadmeExamples;
