//! The strict read tier: accessors that keep absent, null, and wrong-type apart.
//!
//! [`JsonReader`](crate::JsonReader)'s ordinary accessors answer with
//! `Option<T>`, which folds three different wire facts into one `None`: the key
//! was not there, the key was there holding `null`, and the key was there
//! holding something of another type. That fold is right for a permissive read
//! and wrong at a trust boundary, where "no such account" and "a corrupt
//! account id" call for different answers and only one of them is safe to
//! treat as a value.
//!
//! Callers have been paying for the difference by hand — a
//! [`has_field`](crate::JsonReader::has_field) probe before every accessor, or
//! a whole second pass over the document to reject present-but-unreadable keys
//! before the real read trusts `None` to mean absence. This module is that
//! pattern, stated once.
//!
//! The source library spells the tiers as three families of named accessors —
//! `required_*`, `optional_*`, and `optional_nullable_*`. Rust does not need
//! the third family: `Option<T>` already carries absence in the return type, so
//! the two shapes here are "the key must be there" and "the key may be there",
//! and how an explicit `null` reads is the one remaining decision, taken as a
//! named [`NullReads`] argument rather than as a third set of method names.
//!
//! **Arrays read on the same two shapes.** They briefly did not: an array-only
//! tier carried its own error type, its own kinds, and an `AbsentArray` policy
//! for the question `required_`/`optional_` already answers. One idea with two
//! vocabularies is what every consumer then has to translate between, so the
//! arrays are here, on these names, carrying this error.

use bc_error::AnyError;

use crate::detail;
use crate::{JsonReader, kinds};

/// How a strict read treats a key whose value is an explicit `null`.
///
/// The source library makes this the difference between its `optional_*` and
/// `optional_nullable_*` families. It is a real decision and has no safe
/// default: over a PATCH body `null` conventionally means "clear this field",
/// which is a value and not an absence, while over a config document it is
/// usually a mistake the reader should refuse rather than silently ignore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NullReads {
    /// An explicit `null` is refused, as any other wrong type would be.
    Refused,
    /// An explicit `null` reads exactly as an absent key does.
    AsAbsent,
}

/// Why a strict read refused.
///
/// `#[non_exhaustive]`: a stricter read may need a further kind — a length or
/// depth bound is the obvious candidate — and a consumer mapping these into its
/// own refusal vocabulary should not break when one arrives. Matching inside
/// this crate stays exhaustive. A consumer's `_` arm should refuse in its own
/// words rather than fold an unknown kind into one of these, because a refusal
/// naming the wrong cause sends the reader to the wrong file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum JsonReadErrorKind {
    /// The key is not present, and the read required it.
    Absent,
    /// The key is present, holding a value of some other type. For an array
    /// read this is the "present, but not an array" case.
    NotOfType,
    /// The key is present, holding an explicit `null`, and the read was made
    /// with [`NullReads::Refused`].
    Null,
    /// The key holds an array whose element at this index is not of the
    /// requested type.
    ///
    /// Distinct from [`NotOfType`](Self::NotOfType) because the fixes differ:
    /// one says the field is the wrong shape, the other says the shape is right
    /// and one entry in it is not.
    ElementType {
        /// Zero-based index of the first element of the wrong type.
        index: usize,
    },
}

/// A strict read that refused, which key it refused, and why.
///
/// Carries the key so a diagnostic can name it. The source library reaches the
/// same end by returning an `AnyError` whose metadata holds the field name;
/// this type is that information with a Rust `match` over the cause still
/// available to the caller, and it converts into an `AnyError` for callers who
/// funnel every failure through one error type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonReadError {
    kind: JsonReadErrorKind,
    key: String,
}

impl JsonReadError {
    fn new(kind: JsonReadErrorKind, key: &str) -> Self {
        Self {
            kind,
            key: key.to_owned(),
        }
    }

    /// Why the read refused.
    #[must_use]
    pub const fn kind(&self) -> JsonReadErrorKind {
        self.kind
    }

    /// The key the read refused.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl core::fmt::Display for JsonReadError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.kind {
            JsonReadErrorKind::Absent => write!(formatter, "the key {:?} is missing", self.key),
            JsonReadErrorKind::NotOfType => write!(
                formatter,
                "the key {:?} is present but is not of the requested type",
                self.key
            ),
            JsonReadErrorKind::Null => {
                write!(formatter, "the key {:?} is present but is null", self.key)
            }
            JsonReadErrorKind::ElementType { index } => write!(
                formatter,
                "element {index} of the key {:?} is not of the requested type",
                self.key
            ),
        }
    }
}

impl std::error::Error for JsonReadError {}

impl From<JsonReadError> for AnyError {
    fn from(error: JsonReadError) -> Self {
        let kind = match error.kind {
            JsonReadErrorKind::Absent => kinds::MISSING_FIELD,
            JsonReadErrorKind::NotOfType | JsonReadErrorKind::ElementType { .. } => {
                kinds::TYPE_MISMATCH
            }
            JsonReadErrorKind::Null => kinds::INVALID_VALUE,
        };
        let erased =
            Self::from_detail(kind, error.to_string()).with_metadata("field", error.key.into());
        // The index is not recoverable from the kind, so it travels beside the
        // field name rather than only inside the rendered prose.
        match error.kind {
            JsonReadErrorKind::ElementType { index } => {
                erased.with_metadata("element_index", (index as u64).into())
            }
            _ => erased,
        }
    }
}

/// What a key holds, before any tier decides whether that is acceptable.
///
/// Every accessor below resolves the key to one of these four states once, so
/// the tiers differ only in which states they refuse — rather than each
/// re-deriving presence from a probe and a value read that can disagree.
enum Held<T> {
    Value(T),
    Null,
    Absent,
    NotOfType,
}

impl JsonReader<'_> {
    /// Resolves what `key` holds, given a decoder for the requested type.
    ///
    /// `has_field` and a value accessor each answer half the question and
    /// disagree on `null` — `has_field` calls it present, the accessor calls it
    /// `None`. Consulting both here, once, is what makes the states separable
    /// at all.
    ///
    /// The decoder is **handed** the key rather than allowed to capture one.
    /// Two independent arguments that must name the same field is the shape of
    /// the positional `(category, kind, message)` hazard the family's
    /// `AnyErrorKind` exists to remove: it type-checks either way, and only
    /// reading both arguments together tells you whether it is right.
    fn held<T>(&self, key: &str, decode: impl FnOnce(&Self, &str) -> Option<T>) -> Held<T> {
        if let Some(value) = decode(self, key) {
            return Held::Value(value);
        }
        if !self.has_field(key) {
            return Held::Absent;
        }
        if self.is_null(key) {
            return Held::Null;
        }
        Held::NotOfType
    }

    /// `key`'s value, refusing an absent key, a `null`, and a wrong type.
    ///
    /// The source library's `required_*` family.
    fn required<T>(
        &self,
        key: &str,
        decode: impl FnOnce(&Self, &str) -> Option<T>,
    ) -> Result<T, JsonReadError> {
        match self.held(key, decode) {
            Held::Value(value) => Ok(value),
            Held::Null => Err(JsonReadError::new(JsonReadErrorKind::Null, key)),
            Held::Absent => Err(JsonReadError::new(JsonReadErrorKind::Absent, key)),
            Held::NotOfType => Err(JsonReadError::new(JsonReadErrorKind::NotOfType, key)),
        }
    }

    /// `key`'s value if it is there, refusing only a wrong type — and a `null`
    /// when `null_reads` says so.
    ///
    /// The source library's `optional_*` family with [`NullReads::Refused`],
    /// and its `optional_nullable_*` family with [`NullReads::AsAbsent`].
    fn optional<T>(
        &self,
        key: &str,
        null_reads: NullReads,
        decode: impl FnOnce(&Self, &str) -> Option<T>,
    ) -> Result<Option<T>, JsonReadError> {
        match self.held(key, decode) {
            Held::Value(value) => Ok(Some(value)),
            Held::Absent => Ok(None),
            Held::Null => match null_reads {
                NullReads::AsAbsent => Ok(None),
                NullReads::Refused => Err(JsonReadError::new(JsonReadErrorKind::Null, key)),
            },
            Held::NotOfType => Err(JsonReadError::new(JsonReadErrorKind::NotOfType, key)),
        }
    }
}

// --- The typed scalar accessors ------------------------------------------
//
// One `required_*` and one `optional_*` per type the permissive reader already
// decodes, each handing that permissive accessor straight to the tier. The
// source library additionally carries a family of range-checked integer readers
// (`required_i32`, `required_nonnegative_integer`, `required_positive_integer`);
// those are deliberately not ported — in Rust the range check is
// `u32::try_from(value)` at the call site, one visible line that names the bound
// it is enforcing, rather than a named accessor per bound.

impl JsonReader<'_> {
    /// `key`'s decoded string, refusing absent, `null`, and any other type.
    ///
    /// # Errors
    ///
    /// [`JsonReadErrorKind::Absent`] when the key is missing,
    /// [`JsonReadErrorKind::Null`] when it holds `null`, and
    /// [`JsonReadErrorKind::NotOfType`] when it holds another type.
    pub fn required_string(&self, key: &str) -> Result<String, JsonReadError> {
        self.required(key, Self::string)
    }

    /// `key`'s decoded string if present, refusing any other type.
    ///
    /// # Errors
    ///
    /// [`JsonReadErrorKind::NotOfType`] when the key holds another type, and
    /// [`JsonReadErrorKind::Null`] when it holds `null` under
    /// [`NullReads::Refused`]. An absent key is never an error here.
    pub fn optional_string(
        &self,
        key: &str,
        null_reads: NullReads,
    ) -> Result<Option<String>, JsonReadError> {
        self.optional(key, null_reads, Self::string)
    }

    /// `key`'s `i64`, refusing absent, `null`, and any other type.
    ///
    /// # Errors
    ///
    /// As [`required_string`](Self::required_string), for an integer.
    pub fn required_integer(&self, key: &str) -> Result<i64, JsonReadError> {
        self.required(key, Self::integer)
    }

    /// `key`'s `i64` if present, refusing any other type.
    ///
    /// # Errors
    ///
    /// As [`optional_string`](Self::optional_string), for an integer.
    pub fn optional_integer(
        &self,
        key: &str,
        null_reads: NullReads,
    ) -> Result<Option<i64>, JsonReadError> {
        self.optional(key, null_reads, Self::integer)
    }

    /// `key`'s `bool`, refusing absent, `null`, and any other type.
    ///
    /// # Errors
    ///
    /// As [`required_string`](Self::required_string), for a boolean.
    pub fn required_boolean(&self, key: &str) -> Result<bool, JsonReadError> {
        self.required(key, Self::boolean)
    }

    /// `key`'s `bool` if present, refusing any other type.
    ///
    /// # Errors
    ///
    /// As [`optional_string`](Self::optional_string), for a boolean.
    pub fn optional_boolean(
        &self,
        key: &str,
        null_reads: NullReads,
    ) -> Result<Option<bool>, JsonReadError> {
        self.optional(key, null_reads, Self::boolean)
    }

    /// `key`'s finite `f64`, refusing absent, `null`, and any other type.
    ///
    /// The permissive [`floating`](Self::floating) conflates "key absent" with
    /// "present but not a finite number"; this is the accessor whose own doc
    /// comment told callers to combine it with `has_field`.
    ///
    /// # Errors
    ///
    /// As [`required_string`](Self::required_string), for a finite number.
    pub fn required_floating(&self, key: &str) -> Result<f64, JsonReadError> {
        self.required(key, Self::floating)
    }

    /// `key`'s finite `f64` if present, refusing any other type.
    ///
    /// # Errors
    ///
    /// As [`optional_string`](Self::optional_string), for a finite number.
    pub fn optional_floating(
        &self,
        key: &str,
        null_reads: NullReads,
    ) -> Result<Option<f64>, JsonReadError> {
        self.optional(key, null_reads, Self::floating)
    }
}

// --- The typed array accessors -------------------------------------------
//
// The strict counterparts of `string_array` / `object_array`: where those return
// the prefix they could decode, these refuse and say which element stopped
// them. At a trust boundary a truncated list is not a smaller answer, it is a
// different one — a shortened argument vector runs another command — so the two
// shapes must not be confusable.
//
// `find_array` is the decoder handed to the tier, so an array answers the same
// four states every scalar does: a present non-array is `NotOfType` and a
// present `null` is `Null`, rather than both collapsing into one "not an array".
//
// This block keeps its named lifetime, unlike the two above: an object array
// hands back readers that borrow the document, so `Self` here is tied to it.

impl<'a> JsonReader<'a> {
    /// `key`'s array of decoded strings, whole or refused.
    ///
    /// # Errors
    ///
    /// [`JsonReadErrorKind::Absent`] when the key is missing,
    /// [`JsonReadErrorKind::Null`] when it holds `null`,
    /// [`JsonReadErrorKind::NotOfType`] when it holds a non-array, and
    /// [`JsonReadErrorKind::ElementType`] naming the first non-string element.
    pub fn required_string_array(&self, key: &str) -> Result<Vec<String>, JsonReadError> {
        let array = self.required(key, Self::find_array)?;
        Self::decode_string_elements(array, key)
    }

    /// `key`'s array of decoded strings if present, whole or refused.
    ///
    /// # Errors
    ///
    /// As [`required_string_array`](Self::required_string_array), except that
    /// an absent key yields `Ok(None)` and a `null` follows `null_reads`.
    pub fn optional_string_array(
        &self,
        key: &str,
        null_reads: NullReads,
    ) -> Result<Option<Vec<String>>, JsonReadError> {
        match self.optional(key, null_reads, Self::find_array)? {
            Some(array) => Self::decode_string_elements(array, key).map(Some),
            None => Ok(None),
        }
    }

    /// `key`'s array of object readers, whole or refused.
    ///
    /// Each reader borrows the caller's buffer, so the elements cost no copy.
    ///
    /// # Errors
    ///
    /// As [`required_string_array`](Self::required_string_array), with
    /// [`JsonReadErrorKind::ElementType`] naming the first non-object element.
    pub fn required_object_array(&self, key: &str) -> Result<Vec<Self>, JsonReadError> {
        let array = self.required(key, Self::find_array)?;
        Self::decode_object_elements(array, key)
    }

    /// `key`'s array of object readers if present, whole or refused.
    ///
    /// # Errors
    ///
    /// As [`required_object_array`](Self::required_object_array), except that
    /// an absent key yields `Ok(None)` and a `null` follows `null_reads`.
    pub fn optional_object_array(
        &self,
        key: &str,
        null_reads: NullReads,
    ) -> Result<Option<Vec<Self>>, JsonReadError> {
        match self.optional(key, null_reads, Self::find_array)? {
            Some(array) => Self::decode_object_elements(array, key).map(Some),
            None => Ok(None),
        }
    }

    /// Decodes every element as a string, or names the first that is not one.
    fn decode_string_elements(array: &'a str, key: &str) -> Result<Vec<String>, JsonReadError> {
        let mut values = Vec::new();
        for (index, element) in Self::extract_array_elements(array).into_iter().enumerate() {
            let mut value = String::new();
            let start = detail::skip_ws(element, 0);
            if element.as_bytes().get(start) != Some(&b'"')
                || detail::parse_json_string(element, start, &mut value).is_none()
            {
                return Err(JsonReadError::new(
                    JsonReadErrorKind::ElementType { index },
                    key,
                ));
            }
            values.push(value);
        }
        Ok(values)
    }

    /// Reads every element as an object, or names the first that is not one.
    fn decode_object_elements(array: &'a str, key: &str) -> Result<Vec<Self>, JsonReadError> {
        let mut objects = Vec::new();
        for (index, element) in Self::extract_array_elements(array).into_iter().enumerate() {
            let start = detail::skip_ws(element, 0);
            if element.as_bytes().get(start) != Some(&b'{') {
                return Err(JsonReadError::new(
                    JsonReadErrorKind::ElementType { index },
                    key,
                ));
            }
            // The element came out of a document this reader already holds, so
            // it is exactly as trusted as that document is.
            objects.push(Self::from_trusted_unchecked(element));
        }
        Ok(objects)
    }
}
