//! The serde `Deserializer`.
//!
//! # Why this does not sit on `JsonReader`
//!
//! The reader is a lazy per-key scanner: every lookup restarts at the top of the
//! object and skips values until the key matches. That is exactly right when a
//! caller wants two fields out of a large body, and exactly wrong for
//! deserializing, which wants every field — `1 + K` scans of the same object to
//! read `K` fields. So this module has its own forward cursor, which reads each
//! token once.
//!
//! What it does **not** have is its own idea of what a string or a number is:
//! escapes go through [`validate::decode_json_string`] and numbers through
//! [`number::json_number_end`], the same definitions the validator, the reader,
//! and the writer share. That is the discipline that keeps the two halves of this
//! crate agreeing; a third parser with its own opinions would break it quietly.
//!
//! # Borrowing
//!
//! A string with no escapes is handed back as a slice of the caller's input, so
//! `#[derive(Deserialize)]` on a type holding `&str` costs no allocation. A
//! string that *has* an escape cannot be borrowed — the decoded text does not
//! exist anywhere in the input — so it is owned. Both are `Cow`, and serde's
//! `visit_borrowed_str` / `visit_string` split reports which one happened.

use std::borrow::Cow;
use std::fmt;

use serde::Deserialize;
use serde::de::value::CowStrDeserializer;
use serde::de::{self, DeserializeSeed, IntoDeserializer, Visitor};

use crate::detail::skip_ws;
use crate::number::json_number_end;
use crate::validate::{
    JsonValidationError, JsonValidationErrorKind, Validated, decode_json_string, validate_json,
};

/// Why a document could not be deserialized.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonDeserializeError {
    /// The input is not valid JSON. Only [`from_str_unchecked`] can produce
    /// this — [`from_str`] validates first, so by the time a visitor runs, the
    /// document has already passed every check the crate makes.
    Invalid(JsonValidationError),
    /// Well-formed JSON that is not the shape the target type wanted.
    TypeMismatch {
        /// What the target type expected, in serde's vocabulary.
        expected: &'static str,
        /// Where in the input the mismatch begins.
        byte_offset: usize,
    },
    /// A grammatically valid number that does not fit the target type — `1e400`
    /// into an `f64`, or `300` into a `u8`.
    NumberOutOfRange {
        /// Where the number begins.
        byte_offset: usize,
    },
    /// Input remained after a complete value.
    TrailingInput {
        /// Where the leftover input begins.
        byte_offset: usize,
    },
    /// A `Deserialize` implementation reported its own failure — a missing
    /// field, an unknown variant, a failed `try_from`.
    Message(String),
}

impl fmt::Display for JsonDeserializeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => error.fmt(formatter),
            Self::TypeMismatch {
                expected,
                byte_offset,
            } => write!(formatter, "expected {expected} at byte {byte_offset}"),
            Self::NumberOutOfRange { byte_offset } => write!(
                formatter,
                "the number at byte {byte_offset} does not fit the target type"
            ),
            Self::TrailingInput { byte_offset } => {
                write!(formatter, "trailing input at byte {byte_offset}")
            }
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for JsonDeserializeError {}

impl From<JsonValidationError> for JsonDeserializeError {
    fn from(error: JsonValidationError) -> Self {
        Self::Invalid(error)
    }
}

impl de::Error for JsonDeserializeError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::Message(message.to_string())
    }
}

/// Deserializes `T` from a JSON document, validating it first.
///
/// Validation is not an optional extra step here: it is what makes the rest of
/// the crate's guarantees hold, and it is cheap relative to building `T`.
///
/// # Errors
///
/// [`JsonDeserializeError`] — invalid JSON, a shape `T` did not expect, a number
/// out of range, or a failure `T`'s own `Deserialize` reported.
///
/// ```
/// # #[derive(serde::Deserialize, PartialEq, Debug)]
/// # struct Order<'a> { id: &'a str, total: f64 }
/// let body = r#"{"id":"a1","total":12.5}"#;
/// let order: Order<'_> = bc_json::from_str(body)?;
/// assert_eq!(order, Order { id: "a1", total: 12.5 });
/// # Ok::<(), bc_json::JsonDeserializeError>(())
/// ```
///
/// # Errors on a document that is not valid JSON
///
/// ```
/// let duplicate = r#"{"id":1,"id":2}"#;
/// assert!(bc_json::from_str::<serde_json::Value>(duplicate).is_err());
/// ```
pub fn from_str<'a, T>(input: &'a str) -> Result<T, JsonDeserializeError>
where
    T: Deserialize<'a>,
{
    from_validated(validate_json(input)?)
}

/// Deserializes `T` from a document already proven valid.
///
/// The sibling of [`JsonReader::new`](crate::JsonReader::new), and there for the
/// same reason: a [`Validated`] is proof the work has been done, so a caller who
/// validated at the boundary does not pay for it twice.
///
/// # Errors
///
/// As [`from_str`], minus [`JsonDeserializeError::Invalid`], which a validated
/// document cannot produce.
pub fn from_validated<'a, T>(document: Validated<'a>) -> Result<T, JsonDeserializeError>
where
    T: Deserialize<'a>,
{
    from_str_unchecked(document.as_str())
}

/// Deserializes `T` without validating the input first.
///
/// The named escape hatch, alongside
/// [`JsonReader::from_trusted_unchecked`](crate::JsonReader::from_trusted_unchecked)
/// and [`JsonWriter::raw_field_unchecked`](crate::JsonWriter::raw_field_unchecked).
/// It is safe — a malformed document produces an error, never a panic — but it
/// gives up the guarantees validation buys, most importantly duplicate-key
/// rejection: `{"a":1,"a":2}` deserializes here, with the **last** value
/// winning, because that is what serde's own `Deserialize` impls do with a
/// repeated key.
///
/// # Errors
///
/// As [`from_str`].
pub fn from_str_unchecked<'a, T>(input: &'a str) -> Result<T, JsonDeserializeError>
where
    T: Deserialize<'a>,
{
    let mut deserializer = Deserializer { input, position: 0 };
    let value = T::deserialize(&mut deserializer)?;
    deserializer.finish()?;
    Ok(value)
}

/// A forward cursor over a JSON document.
struct Deserializer<'de> {
    input: &'de str,
    position: usize,
}

impl<'de> Deserializer<'de> {
    /// Confirms nothing but whitespace follows the value just read.
    fn finish(&mut self) -> Result<(), JsonDeserializeError> {
        let end = skip_ws(self.input, self.position);
        if end == self.input.len() {
            Ok(())
        } else {
            Err(JsonDeserializeError::TrailingInput { byte_offset: end })
        }
    }

    /// The next meaningful byte, with the cursor moved to it.
    fn peek(&mut self) -> Option<u8> {
        self.position = skip_ws(self.input, self.position);
        self.input.as_bytes().get(self.position).copied()
    }

    fn expected(&self, expected: &'static str) -> JsonDeserializeError {
        JsonDeserializeError::TypeMismatch {
            expected,
            byte_offset: self.position,
        }
    }

    /// Consumes `byte`, or reports what was expected instead.
    fn expect(&mut self, byte: u8, expected: &'static str) -> Result<(), JsonDeserializeError> {
        if self.peek() == Some(byte) {
            self.position += 1;
            Ok(())
        } else {
            Err(self.expected(expected))
        }
    }

    /// Consumes `literal` when it is the complete token at the cursor.
    ///
    /// Delimiter-bounded, not a prefix match: `truexyz` is not `true`. The C++
    /// source's `substr(pos, 4) == "true"` is the defect this avoids, and it is
    /// avoided here for the same reason the reader avoids it.
    fn expect_literal(&mut self, literal: &str) -> bool {
        self.position = skip_ws(self.input, self.position);
        let rest = &self.input[self.position.min(self.input.len())..];
        if !rest.starts_with(literal) {
            return false;
        }
        let after = self.position + literal.len();
        let bounded = self
            .input
            .as_bytes()
            .get(after)
            .is_none_or(|byte| !byte.is_ascii_alphanumeric());
        if bounded {
            self.position = after;
        }
        bounded
    }

    /// The string token at the cursor, borrowed when it holds no escape.
    fn parse_string(&mut self) -> Result<Cow<'de, str>, JsonDeserializeError> {
        // Every token reader skips its own leading whitespace, rather than
        // relying on the caller having peeked. Pretty-printed input is the case
        // that proves the difference: `"k": 1` puts a space where compact output
        // puts the digit.
        let start = skip_ws(self.input, self.position);
        self.position = start;
        if self.input.as_bytes().get(start) != Some(&b'"') {
            return Err(self.expected("a string"));
        }

        // Scan for a closing quote with no backslash before it. That is the
        // common case by a wide margin, and it is the whole reason a caller can
        // deserialize into a `&str`.
        let bytes = self.input.as_bytes();
        let mut scan = start + 1;
        while scan < bytes.len() {
            match bytes[scan] {
                b'"' => {
                    self.position = scan + 1;
                    return Ok(Cow::Borrowed(&self.input[start + 1..scan]));
                }
                b'\\' => break,
                byte if byte < 0x20 => {
                    return Err(JsonDeserializeError::Invalid(JsonValidationError {
                        kind: JsonValidationErrorKind::InvalidString,
                        byte_offset: scan,
                    }));
                }
                _ => scan += 1,
            }
        }

        // An escape, or a string that never closes. Either way the shared decoder
        // owns the answer — this module does not get its own escape table.
        let mut decoded = String::new();
        self.position = decode_json_string(self.input, start, &mut decoded)?;
        Ok(Cow::Owned(decoded))
    }

    /// The raw number token at the cursor, as text.
    fn parse_number_token(&mut self) -> Result<&'de str, JsonDeserializeError> {
        let start = skip_ws(self.input, self.position);
        self.position = start;
        let end = json_number_end(self.input, start)?;
        self.position = end;
        Ok(&self.input[start..end])
    }

    fn parse_f64(&mut self) -> Result<f64, JsonDeserializeError> {
        let start = self.position;
        let token = self.parse_number_token()?;
        // The grammar admits exponents no `f64` can hold. Refusing them here
        // matches `JsonReader::floating`, which returns `None` rather than an
        // infinity for the same input.
        token
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or(JsonDeserializeError::NumberOutOfRange { byte_offset: start })
    }

    /// Reads any value and throws it away, without building it.
    ///
    /// Needed by `#[serde(deny_unknown_fields)]`'s absence — an unknown field's
    /// value still has to be stepped over — and it is the cheapest possible
    /// implementation of `deserialize_ignored_any`.
    fn skip_value(&mut self) -> Result<(), JsonDeserializeError> {
        match self.peek().ok_or_else(|| self.expected("a value"))? {
            b'"' => {
                self.parse_string()?;
            }
            b'{' | b'[' => {
                let end = crate::detail::skip_json_value(self.input, self.position)
                    .ok_or_else(|| self.expected("a complete value"))?;
                self.position = end;
            }
            b't' | b'f' | b'n' => {
                if !(self.expect_literal("true")
                    || self.expect_literal("false")
                    || self.expect_literal("null"))
                {
                    return Err(self.expected("a literal"));
                }
            }
            _ => {
                self.parse_number_token()?;
            }
        }
        Ok(())
    }
}

/// The integer accessors differ only in the type they land in, and the range
/// check is the same shape every time.
macro_rules! deserialize_integer {
    ($($method:ident => $visit:ident, $integer:ty;)+) => {
        $(
            fn $method<V>(self, visitor: V) -> Result<V::Value, Self::Error>
            where
                V: Visitor<'de>,
            {
                let start = self.position;
                let token = self.parse_number_token()?;
                let value = token.parse::<$integer>().map_err(|_| {
                    JsonDeserializeError::NumberOutOfRange { byte_offset: start }
                })?;
                visitor.$visit(value)
            }
        )+
    };
}

impl<'de> de::Deserializer<'de> for &mut Deserializer<'de> {
    type Error = JsonDeserializeError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match self.peek().ok_or_else(|| self.expected("a value"))? {
            b'"' => self.deserialize_str(visitor),
            b'{' => self.deserialize_map(visitor),
            b'[' => self.deserialize_seq(visitor),
            b't' | b'f' => self.deserialize_bool(visitor),
            b'n' => self.deserialize_unit(visitor),
            _ => {
                // Integer or float is a JSON distinction only in the token's
                // spelling, so it is read from the token rather than guessed at
                // from the value.
                let start = self.position;
                let token = self.parse_number_token()?;
                if token.contains(['.', 'e', 'E']) {
                    let value = token
                        .parse::<f64>()
                        .ok()
                        .filter(|value| value.is_finite())
                        .ok_or(JsonDeserializeError::NumberOutOfRange { byte_offset: start })?;
                    visitor.visit_f64(value)
                } else if let Ok(value) = token.parse::<i64>() {
                    visitor.visit_i64(value)
                } else if let Ok(value) = token.parse::<u64>() {
                    visitor.visit_u64(value)
                } else {
                    // Wider than 64 bits but still a valid JSON integer. `f64` is
                    // lossy here and says so, rather than failing on a document
                    // that is not wrong.
                    let value = token
                        .parse::<f64>()
                        .ok()
                        .filter(|value| value.is_finite())
                        .ok_or(JsonDeserializeError::NumberOutOfRange { byte_offset: start })?;
                    visitor.visit_f64(value)
                }
            }
        }
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.peek();
        if self.expect_literal("true") {
            visitor.visit_bool(true)
        } else if self.expect_literal("false") {
            visitor.visit_bool(false)
        } else {
            Err(self.expected("a bool"))
        }
    }

    deserialize_integer! {
        deserialize_i8 => visit_i8, i8;
        deserialize_i16 => visit_i16, i16;
        deserialize_i32 => visit_i32, i32;
        deserialize_i64 => visit_i64, i64;
        deserialize_i128 => visit_i128, i128;
        deserialize_u8 => visit_u8, u8;
        deserialize_u16 => visit_u16, u16;
        deserialize_u32 => visit_u32, u32;
        deserialize_u64 => visit_u64, u64;
        deserialize_u128 => visit_u128, u128;
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "narrowing to the width the target type asked for is the operation"
        )]
        visitor.visit_f32(self.parse_f64()? as f32)
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_f64(self.parse_f64()?)
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.peek();
        let text = self.parse_string()?;
        let mut characters = text.chars();
        match (characters.next(), characters.next()) {
            (Some(character), None) => visitor.visit_char(character),
            _ => Err(self.expected("a one-character string")),
        }
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.peek();
        match self.parse_string()? {
            // `visit_borrowed_str`, not `visit_str`: it is what lets a target
            // type hold a `&'de str` instead of a `String`.
            Cow::Borrowed(text) => visitor.visit_borrowed_str(text),
            Cow::Owned(text) => visitor.visit_string(text),
        }
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_str(visitor)
    }

    /// An array of byte values, matching what the serializer emits.
    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if self.peek() == Some(b'n') && self.expect_literal("null") {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.peek();
        if self.expect_literal("null") {
            visitor.visit_unit()
        } else {
            Err(self.expected("null"))
        }
    }

    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.expect(b'[', "an array")?;
        let value = visitor.visit_seq(Elements {
            deserializer: &mut *self,
            first: true,
        })?;
        self.expect(b']', "the end of the array")?;
        Ok(value)
    }

    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.expect(b'{', "an object")?;
        let value = visitor.visit_map(Members {
            deserializer: &mut *self,
            first: true,
        })?;
        self.expect(b'}', "the end of the object")?;
        Ok(value)
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_map(visitor)
    }

    /// Serde's externally-tagged representation: a bare string for a unit
    /// variant, a one-member object for everything else.
    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match self.peek().ok_or_else(|| self.expected("an enum"))? {
            b'"' => {
                let variant = self.parse_string()?;
                visitor.visit_enum(variant.into_deserializer())
            }
            b'{' => {
                self.position += 1;
                let value = visitor.visit_enum(Tagged {
                    deserializer: &mut *self,
                })?;
                self.expect(b'}', "the end of the variant object")?;
                Ok(value)
            }
            _ => Err(self.expected("an enum")),
        }
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.skip_value()?;
        visitor.visit_unit()
    }
}

/// The elements of one array.
struct Elements<'a, 'de> {
    deserializer: &'a mut Deserializer<'de>,
    first: bool,
}

impl<'de> de::SeqAccess<'de> for Elements<'_, 'de> {
    type Error = JsonDeserializeError;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: DeserializeSeed<'de>,
    {
        if self.deserializer.peek() == Some(b']') {
            return Ok(None);
        }
        if !self.first {
            self.deserializer.expect(b',', "a comma or the end")?;
        }
        self.first = false;
        seed.deserialize(&mut *self.deserializer).map(Some)
    }
}

/// The members of one object.
struct Members<'a, 'de> {
    deserializer: &'a mut Deserializer<'de>,
    first: bool,
}

impl<'de> de::MapAccess<'de> for Members<'_, 'de> {
    type Error = JsonDeserializeError;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error>
    where
        K: DeserializeSeed<'de>,
    {
        if self.deserializer.peek() == Some(b'}') {
            return Ok(None);
        }
        if !self.first {
            self.deserializer.expect(b',', "a comma or the end")?;
        }
        self.first = false;
        seed.deserialize(MapKey {
            deserializer: &mut *self.deserializer,
        })
        .map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Self::Error>
    where
        V: DeserializeSeed<'de>,
    {
        self.deserializer.expect(b':', "a colon")?;
        seed.deserialize(&mut *self.deserializer)
    }
}

/// An object key.
///
/// Always a string in the document, but not always one in the target type:
/// `HashMap<u32, T>` serializes its keys as strings, so it has to read them back
/// the same way. Every non-string request is answered by parsing the key's text,
/// which is the exact inverse of what the serializer's `MapKeySerializer` wrote.
struct MapKey<'a, 'de> {
    deserializer: &'a mut Deserializer<'de>,
}

/// Each integer key parses its own text; the shape is identical per width.
macro_rules! deserialize_integer_key {
    ($($method:ident => $visit:ident, $integer:ty;)+) => {
        $(
            fn $method<V>(self, visitor: V) -> Result<V::Value, Self::Error>
            where
                V: Visitor<'de>,
            {
                let byte_offset = self.deserializer.position;
                let text = self.deserializer.parse_string()?;
                let value = text.parse::<$integer>().map_err(|_| {
                    JsonDeserializeError::NumberOutOfRange { byte_offset }
                })?;
                visitor.$visit(value)
            }
        )+
    };
}

impl<'de> de::Deserializer<'de> for MapKey<'_, 'de> {
    type Error = JsonDeserializeError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserializer.peek();
        match self.deserializer.parse_string()? {
            Cow::Borrowed(text) => visitor.visit_borrowed_str(text),
            Cow::Owned(text) => visitor.visit_string(text),
        }
    }

    deserialize_integer_key! {
        deserialize_i8 => visit_i8, i8;
        deserialize_i16 => visit_i16, i16;
        deserialize_i32 => visit_i32, i32;
        deserialize_i64 => visit_i64, i64;
        deserialize_i128 => visit_i128, i128;
        deserialize_u8 => visit_u8, u8;
        deserialize_u16 => visit_u16, u16;
        deserialize_u32 => visit_u32, u32;
        deserialize_u64 => visit_u64, u64;
        deserialize_u128 => visit_u128, u128;
    }

    /// A newtype key is the inner type's key — `serialize_newtype_struct` on the
    /// way out did the same unwrapping.
    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    serde::forward_to_deserialize_any! {
        bool f32 f64 char str string bytes byte_buf option unit unit_struct seq
        tuple tuple_struct map struct enum identifier ignored_any
    }
}

/// The `{"Variant": payload}` form of a non-unit enum variant.
struct Tagged<'a, 'de> {
    deserializer: &'a mut Deserializer<'de>,
}

impl<'de> de::EnumAccess<'de> for Tagged<'_, 'de> {
    type Error = JsonDeserializeError;
    type Variant = Self;

    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self), Self::Error>
    where
        V: DeserializeSeed<'de>,
    {
        self.deserializer.peek();
        let variant = self.deserializer.parse_string()?;
        // Annotated because `into_deserializer` is generic over the error type
        // and nothing downstream pins it here.
        let name: CowStrDeserializer<'de, JsonDeserializeError> = variant.into_deserializer();
        let value = seed.deserialize(name)?;
        self.deserializer.expect(b':', "a colon")?;
        Ok((value, self))
    }
}

impl<'de> de::VariantAccess<'de> for Tagged<'_, 'de> {
    type Error = JsonDeserializeError;

    /// A unit variant inside an object is `{"Variant":null}` — the shape serde
    /// produces when a unit variant is reached through a `Content` buffer.
    fn unit_variant(self) -> Result<(), Self::Error> {
        de::Deserializer::deserialize_unit(self.deserializer, de::IgnoredAny).map(|_| ())
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value, Self::Error>
    where
        T: DeserializeSeed<'de>,
    {
        seed.deserialize(&mut *self.deserializer)
    }

    fn tuple_variant<V>(self, len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        de::Deserializer::deserialize_tuple(self.deserializer, len, visitor)
    }

    fn struct_variant<V>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        de::Deserializer::deserialize_struct(self.deserializer, "", fields, visitor)
    }
}
