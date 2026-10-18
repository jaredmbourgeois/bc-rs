//! The serde `Serializer`, over [`JsonWriter`].
//!
//! This is a thin adapter, deliberately: the escaping, the float formatting, the
//! separator discipline, and the duplicate-key check all stay in the writer, so
//! `to_string` and a hand-built document cannot disagree about what JSON is.
//!
//! The one structural difference is how a container is opened. The native API
//! pairs open and close inside a single closure-taking call, which is why a
//! caller cannot forget the close. Serde's protocol hands out a begin call and an
//! end call with the caller's code in between, so this module uses the writer's
//! split `begin_container` / `end_container` pair instead — the same discipline
//! reached a second way, not a second implementation of it.

use std::fmt;

use serde::Serialize;
use serde::ser::{self, Impossible};

use crate::writer::{JsonWriteError, JsonWriter};

/// Why a value could not be serialized.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonSerializeError {
    /// The document could not be written — a non-finite float, or (in a debug
    /// build) a duplicate key. See [`JsonWriteError`].
    Write(JsonWriteError),
    /// A map key was not something JSON can spell as an object key.
    ///
    /// JSON object keys are strings. Strings, `char`, and every integer width
    /// are accepted and written as strings; anything else — a float, a bool, a
    /// struct — has no faithful JSON form and is refused rather than guessed at.
    NonStringKey,
    /// A `Serialize` implementation reported its own failure.
    Message(String),
}

impl fmt::Display for JsonSerializeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Write(error) => error.fmt(formatter),
            Self::NonStringKey => {
                formatter.write_str("a JSON object key must be a string, a char, or an integer")
            }
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for JsonSerializeError {}

impl From<JsonWriteError> for JsonSerializeError {
    fn from(error: JsonWriteError) -> Self {
        Self::Write(error)
    }
}

impl ser::Error for JsonSerializeError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::Message(message.to_string())
    }
}

/// Serializes `value` as compact JSON.
///
/// # Errors
///
/// [`JsonSerializeError`] — a non-finite float, a map key JSON cannot spell, or
/// a failure reported by `value`'s own `Serialize`.
///
/// ```
/// # #[derive(serde::Serialize)]
/// # struct Order { id: String, total: f64, tags: Vec<String> }
/// let order = Order { id: "a1".into(), total: 12.5, tags: vec!["rush".into()] };
/// assert_eq!(
///     bc_json::to_string(&order)?,
///     r#"{"id":"a1","total":12.5,"tags":["rush"]}"#
/// );
/// # Ok::<(), bc_json::JsonSerializeError>(())
/// ```
pub fn to_string<T>(value: &T) -> Result<String, JsonSerializeError>
where
    T: ?Sized + Serialize,
{
    write_document(JsonWriter::new(), value)
}

/// Serializes `value` as two-space-indented JSON.
///
/// # Errors
///
/// As [`to_string`].
pub fn to_string_pretty<T>(value: &T) -> Result<String, JsonSerializeError>
where
    T: ?Sized + Serialize,
{
    write_document(JsonWriter::pretty(), value)
}

fn write_document<T>(mut writer: JsonWriter, value: &T) -> Result<String, JsonSerializeError>
where
    T: ?Sized + Serialize,
{
    value.serialize(&mut Serializer {
        writer: &mut writer,
    })?;
    // The writer is poisoned rather than failing at the offending call, so this
    // is where a non-finite float or a duplicate key actually surfaces —
    // including one written by a `Serialize` impl that reported success.
    writer.finish().map_err(JsonSerializeError::Write)
}

/// Writes a `Serialize` value into a [`JsonWriter`].
///
/// Not public: a bare value written mid-document needs the enclosing separator
/// and key already emitted, and those are the writer's internals. Callers
/// compose through [`to_string`] instead.
struct Serializer<'w> {
    writer: &'w mut JsonWriter,
}

/// An open container, holding the separator state to restore on close.
struct Container<'a, 'w> {
    serializer: &'a mut Serializer<'w>,
    outer: bool,
    closer: char,
}

impl Serializer<'_> {
    fn open(&mut self, delimiter: char) -> (bool, char) {
        let closer = if delimiter == '{' { '}' } else { ']' };
        (self.writer.begin_container(delimiter), closer)
    }

    /// Opens the `{"Variant":` wrapper serde's externally-tagged enum
    /// representation puts around a non-unit variant's payload.
    fn open_variant(&mut self, variant: &'static str) -> bool {
        let outer = self.writer.begin_container('{');
        self.writer.separator();
        self.writer.write_key(variant);
        outer
    }
}

impl<'a, 'w> ser::Serializer for &'a mut Serializer<'w> {
    type Ok = ();
    type Error = JsonSerializeError;

    type SerializeSeq = Container<'a, 'w>;
    type SerializeTuple = Container<'a, 'w>;
    type SerializeTupleStruct = Container<'a, 'w>;
    type SerializeTupleVariant = Variant<'a, 'w>;
    type SerializeMap = Container<'a, 'w>;
    type SerializeStruct = Container<'a, 'w>;
    type SerializeStructVariant = Variant<'a, 'w>;

    fn serialize_bool(self, value: bool) -> Result<(), Self::Error> {
        self.writer.write_bool(value);
        Ok(())
    }

    fn serialize_i8(self, value: i8) -> Result<(), Self::Error> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_i16(self, value: i16) -> Result<(), Self::Error> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_i32(self, value: i32) -> Result<(), Self::Error> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_i64(self, value: i64) -> Result<(), Self::Error> {
        self.writer.write_integer(value);
        Ok(())
    }

    fn serialize_u8(self, value: u8) -> Result<(), Self::Error> {
        self.serialize_u64(u64::from(value))
    }

    fn serialize_u16(self, value: u16) -> Result<(), Self::Error> {
        self.serialize_u64(u64::from(value))
    }

    fn serialize_u32(self, value: u32) -> Result<(), Self::Error> {
        self.serialize_u64(u64::from(value))
    }

    fn serialize_u64(self, value: u64) -> Result<(), Self::Error> {
        self.writer.write_unsigned(value);
        Ok(())
    }

    fn serialize_f32(self, value: f32) -> Result<(), Self::Error> {
        // Widened only for formatting: `f32::to_string` is shortest-round-trip
        // for an f32, so `0.1_f32` stays `0.1` instead of becoming the 17-digit
        // f64 that `value as f64` would print.
        self.writer.write_f32(value);
        Ok(())
    }

    fn serialize_f64(self, value: f64) -> Result<(), Self::Error> {
        self.writer.write_f64(value);
        Ok(())
    }

    fn serialize_char(self, value: char) -> Result<(), Self::Error> {
        let mut buffer = [0_u8; 4];
        self.serialize_str(value.encode_utf8(&mut buffer))
    }

    fn serialize_str(self, value: &str) -> Result<(), Self::Error> {
        self.writer.write_quoted(value);
        Ok(())
    }

    /// An array of byte values, matching what a `Vec<u8>` would produce.
    ///
    /// JSON has no byte-string type, and inventing base64 here would make this
    /// crate's output disagree with every other JSON encoder about what a
    /// `serialize_bytes` call means.
    fn serialize_bytes(self, value: &[u8]) -> Result<(), Self::Error> {
        let outer = self.writer.begin_container('[');
        for byte in value {
            self.writer.separator();
            self.writer.write_unsigned(u64::from(*byte));
        }
        self.writer.end_container(']', outer);
        Ok(())
    }

    fn serialize_none(self) -> Result<(), Self::Error> {
        self.serialize_unit()
    }

    fn serialize_some<T>(self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<(), Self::Error> {
        self.writer.write_null();
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Self::Error> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<(), Self::Error> {
        self.serialize_str(variant)
    }

    fn serialize_newtype_struct<T>(self, _name: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        let outer = self.open_variant(variant);
        value.serialize(&mut *self)?;
        self.writer.end_container('}', outer);
        Ok(())
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        let (outer, closer) = self.open('[');
        Ok(Container {
            serializer: self,
            outer,
            closer,
        })
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        let wrapper = self.open_variant(variant);
        let inner = self.writer.begin_container('[');
        Ok(Variant {
            serializer: self,
            wrapper,
            inner,
            closer: ']',
        })
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        let (outer, closer) = self.open('{');
        Ok(Container {
            serializer: self,
            outer,
            closer,
        })
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.serialize_map(Some(len))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        let wrapper = self.open_variant(variant);
        let inner = self.writer.begin_container('{');
        Ok(Variant {
            serializer: self,
            wrapper,
            inner,
            closer: '}',
        })
    }
}

impl Container<'_, '_> {
    fn element<T>(&mut self, value: &T) -> Result<(), JsonSerializeError>
    where
        T: ?Sized + Serialize,
    {
        self.serializer.writer.separator();
        value.serialize(&mut *self.serializer)
    }

    fn close(self) {
        self.serializer
            .writer
            .end_container(self.closer, self.outer);
    }
}

impl ser::SerializeSeq for Container<'_, '_> {
    type Ok = ();
    type Error = JsonSerializeError;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.element(value)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.close();
        Ok(())
    }
}

impl ser::SerializeTuple for Container<'_, '_> {
    type Ok = ();
    type Error = JsonSerializeError;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.element(value)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.close();
        Ok(())
    }
}

impl ser::SerializeTupleStruct for Container<'_, '_> {
    type Ok = ();
    type Error = JsonSerializeError;

    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.element(value)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.close();
        Ok(())
    }
}

impl ser::SerializeMap for Container<'_, '_> {
    type Ok = ();
    type Error = JsonSerializeError;

    fn serialize_key<T>(&mut self, key: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.serializer.writer.separator();
        // Rendered to a string first, then handed to the writer's own `write_key`
        // — which is what escapes it and, in a debug build, notices it is the
        // second time this object has seen it. A map is exactly where that check
        // earns its keep: the keys are caller-driven, and `HashMap` cannot
        // protect you from a `Serialize` impl that yields the same one twice.
        let key = key.serialize(MapKeySerializer)?;
        self.serializer.writer.write_key(&key);
        Ok(())
    }

    fn serialize_value<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(&mut *self.serializer)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.close();
        Ok(())
    }
}

impl ser::SerializeStruct for Container<'_, '_> {
    type Ok = ();
    type Error = JsonSerializeError;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.serializer.writer.separator();
        self.serializer.writer.write_key(key);
        value.serialize(&mut *self.serializer)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.close();
        Ok(())
    }
}

/// A non-unit enum variant: serde's externally-tagged form nests the payload
/// inside a one-member object, so there are two containers to close.
struct Variant<'a, 'w> {
    serializer: &'a mut Serializer<'w>,
    wrapper: bool,
    inner: bool,
    closer: char,
}

impl Variant<'_, '_> {
    fn close(self) {
        self.serializer
            .writer
            .end_container(self.closer, self.inner);
        self.serializer.writer.end_container('}', self.wrapper);
    }
}

impl ser::SerializeTupleVariant for Variant<'_, '_> {
    type Ok = ();
    type Error = JsonSerializeError;

    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.serializer.writer.separator();
        value.serialize(&mut *self.serializer)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.close();
        Ok(())
    }
}

impl ser::SerializeStructVariant for Variant<'_, '_> {
    type Ok = ();
    type Error = JsonSerializeError;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.serializer.writer.separator();
        self.serializer.writer.write_key(key);
        value.serialize(&mut *self.serializer)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.close();
        Ok(())
    }
}

/// Renders a map key to the string a JSON object key has to be.
///
/// Everything it refuses, it refuses loudly. A silent coercion here would put a
/// `HashMap<f64, _>` on the wire as keys no round-trip can recover.
struct MapKeySerializer;

/// The compound forms are all impossible for a key: none of them is a string.
macro_rules! key_must_be_a_string {
    ($($method:ident($($argument:ty),*) -> $result:ty;)+) => {
        $(
            fn $method(self $(, _: $argument)*) -> Result<$result, Self::Error> {
                Err(JsonSerializeError::NonStringKey)
            }
        )+
    };
}

impl ser::Serializer for MapKeySerializer {
    type Ok = String;
    type Error = JsonSerializeError;

    type SerializeSeq = Impossible<String, JsonSerializeError>;
    type SerializeTuple = Impossible<String, JsonSerializeError>;
    type SerializeTupleStruct = Impossible<String, JsonSerializeError>;
    type SerializeTupleVariant = Impossible<String, JsonSerializeError>;
    type SerializeMap = Impossible<String, JsonSerializeError>;
    type SerializeStruct = Impossible<String, JsonSerializeError>;
    type SerializeStructVariant = Impossible<String, JsonSerializeError>;

    fn serialize_str(self, value: &str) -> Result<String, Self::Error> {
        Ok(value.to_owned())
    }

    fn serialize_char(self, value: char) -> Result<String, Self::Error> {
        Ok(value.to_string())
    }

    fn serialize_i8(self, value: i8) -> Result<String, Self::Error> {
        Ok(value.to_string())
    }

    fn serialize_i16(self, value: i16) -> Result<String, Self::Error> {
        Ok(value.to_string())
    }

    fn serialize_i32(self, value: i32) -> Result<String, Self::Error> {
        Ok(value.to_string())
    }

    fn serialize_i64(self, value: i64) -> Result<String, Self::Error> {
        Ok(value.to_string())
    }

    fn serialize_u8(self, value: u8) -> Result<String, Self::Error> {
        Ok(value.to_string())
    }

    fn serialize_u16(self, value: u16) -> Result<String, Self::Error> {
        Ok(value.to_string())
    }

    fn serialize_u32(self, value: u32) -> Result<String, Self::Error> {
        Ok(value.to_string())
    }

    fn serialize_u64(self, value: u64) -> Result<String, Self::Error> {
        Ok(value.to_string())
    }

    /// A newtype around a string is still a string key — the wrapper is a Rust
    /// type distinction, not a JSON one.
    fn serialize_newtype_struct<T>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<String, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }

    /// A unit variant's name is a string, which is how `#[serde(rename_all)]`
    /// enums end up as map keys.
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<String, Self::Error> {
        Ok(variant.to_owned())
    }

    key_must_be_a_string! {
        serialize_bool(bool) -> String;
        serialize_f32(f32) -> String;
        serialize_f64(f64) -> String;
        serialize_bytes(&[u8]) -> String;
        serialize_none() -> String;
        serialize_unit() -> String;
        serialize_unit_struct(&'static str) -> String;
        serialize_seq(Option<usize>) -> Self::SerializeSeq;
        serialize_tuple(usize) -> Self::SerializeTuple;
        serialize_tuple_struct(&'static str, usize) -> Self::SerializeTupleStruct;
        serialize_tuple_variant(&'static str, u32, &'static str, usize) -> Self::SerializeTupleVariant;
        serialize_map(Option<usize>) -> Self::SerializeMap;
        serialize_struct(&'static str, usize) -> Self::SerializeStruct;
        serialize_struct_variant(&'static str, u32, &'static str, usize) -> Self::SerializeStructVariant;
    }

    fn serialize_some<T>(self, _value: &T) -> Result<String, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        Err(JsonSerializeError::NonStringKey)
    }

    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<String, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        Err(JsonSerializeError::NonStringKey)
    }
}
