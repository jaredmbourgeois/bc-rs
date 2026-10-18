use std::collections::BTreeMap;

use crate::AnyError;

/// An ordered collection of metadata values.
pub type MetadataArray = Vec<MetadataValue>;

/// A metadata dictionary ordered lexicographically by key.
pub type MetadataDictionary = BTreeMap<String, MetadataValue>;

/// The active type stored by a [`MetadataValue`].
///
/// Retained from the C++ source, where the variant set is only reachable through
/// a tag plus typed accessors. Rust callers should prefer matching on
/// [`MetadataValue`] directly; this enum exists so ported code and foreign
/// bindings keep a stable discriminant. The values match the source
/// `std::variant` alternative indices exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[repr(u8)]
pub enum MetadataType {
    /// No value.
    Null = 0,
    /// A Boolean value.
    Boolean = 1,
    /// A signed 64-bit integer.
    Int64 = 2,
    /// An unsigned 64-bit integer.
    Uint64 = 3,
    /// A 64-bit floating-point value.
    Float64 = 4,
    /// A UTF-8 string.
    String = 5,
    /// An ordered array.
    Array = 6,
    /// A key-ordered dictionary.
    Dictionary = 7,
    /// A recursive error value.
    Error = 8,
}

/// A recursive value suitable for attaching structured metadata to an error.
///
/// The variant set is public so callers can `match` instead of probing through
/// the `as_*` accessors. It is `#[non_exhaustive]` because this type crosses
/// version and language boundaries, so downstream matches need a wildcard arm.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum MetadataValue {
    /// No value, which is also what `MetadataValue::default()` is.
    #[default]
    Null,
    /// A Boolean value.
    Boolean(bool),
    /// A signed 64-bit integer.
    Int64(i64),
    /// An unsigned 64-bit integer.
    Uint64(u64),
    /// A 64-bit floating-point value.
    Float64(f64),
    /// A UTF-8 string.
    String(String),
    /// An ordered array.
    Array(MetadataArray),
    /// A key-ordered dictionary.
    Dictionary(MetadataDictionary),
    /// A recursive error value.
    Error(Box<AnyError>),
}

impl MetadataValue {
    /// Creates a null metadata value.
    #[must_use]
    pub const fn null() -> Self {
        Self::Null
    }

    /// Returns the type of the stored value.
    #[must_use]
    pub const fn r#type(&self) -> MetadataType {
        match self {
            Self::Null => MetadataType::Null,
            Self::Boolean(_) => MetadataType::Boolean,
            Self::Int64(_) => MetadataType::Int64,
            Self::Uint64(_) => MetadataType::Uint64,
            Self::Float64(_) => MetadataType::Float64,
            Self::String(_) => MetadataType::String,
            Self::Array(_) => MetadataType::Array,
            Self::Dictionary(_) => MetadataType::Dictionary,
            Self::Error(_) => MetadataType::Error,
        }
    }

    /// Returns whether this value is null.
    #[must_use]
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Returns the stored Boolean, or `None` for another type.
    #[must_use]
    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns the stored signed integer, or `None` for another type.
    #[must_use]
    pub const fn as_int64(&self) -> Option<i64> {
        match self {
            Self::Int64(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns the stored unsigned integer, or `None` for another type.
    #[must_use]
    pub const fn as_uint64(&self) -> Option<u64> {
        match self {
            Self::Uint64(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns the stored float, or `None` for another type.
    #[must_use]
    pub const fn as_float64(&self) -> Option<f64> {
        match self {
            Self::Float64(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns the stored string, or `None` for another type.
    #[must_use]
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the stored array, or `None` for another type.
    #[must_use]
    pub const fn as_array(&self) -> Option<&MetadataArray> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the stored dictionary, or `None` for another type.
    #[must_use]
    pub const fn as_dictionary(&self) -> Option<&MetadataDictionary> {
        match self {
            Self::Dictionary(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the stored error, or `None` for another type.
    #[must_use]
    pub fn as_error(&self) -> Option<&AnyError> {
        match self {
            Self::Error(value) => Some(value),
            _ => None,
        }
    }

    /// Renders the value for humans with six decimal places for finite floats.
    ///
    /// Finite-float formatting is locale-independent. The output is JSON-shaped
    /// but is not JSON and does not escape strings. Enable the `serde` feature
    /// for a machine-readable representation.
    #[must_use]
    pub fn stringify(&self) -> String {
        self.stringify_with_decimals(6)
    }

    /// Renders the value for humans with the requested finite-float precision.
    ///
    /// Finite-float formatting is locale-independent. The output is JSON-shaped
    /// but is not JSON and does not escape strings. Enable the `serde` feature
    /// for a machine-readable representation.
    #[must_use]
    pub fn stringify_with_decimals(&self, decimals: u8) -> String {
        match self {
            Self::Null => "null".to_owned(),
            Self::Boolean(value) => value.to_string(),
            Self::Int64(value) => value.to_string(),
            Self::Uint64(value) => value.to_string(),
            Self::Float64(value) => stringify_float(*value, decimals),
            Self::String(value) => format!("\"{value}\""),
            Self::Array(value) => stringify_array(value, decimals),
            Self::Dictionary(value) => stringify_dictionary(value, decimals),
            Self::Error(value) => stringify_error(value, decimals),
        }
    }
}

impl From<bool> for MetadataValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<i64> for MetadataValue {
    fn from(value: i64) -> Self {
        Self::Int64(value)
    }
}

impl From<u64> for MetadataValue {
    fn from(value: u64) -> Self {
        Self::Uint64(value)
    }
}

impl From<f64> for MetadataValue {
    fn from(value: f64) -> Self {
        Self::Float64(value)
    }
}

impl From<String> for MetadataValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for MetadataValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<MetadataArray> for MetadataValue {
    fn from(value: MetadataArray) -> Self {
        Self::Array(value)
    }
}

impl From<MetadataDictionary> for MetadataValue {
    fn from(value: MetadataDictionary) -> Self {
        Self::Dictionary(value)
    }
}

impl From<AnyError> for MetadataValue {
    fn from(value: AnyError) -> Self {
        Self::Error(Box::new(value))
    }
}

/// Chainable construction for a [`MetadataDictionary`].
///
/// An extension trait rather than inherent methods because the dictionary is a
/// type alias for [`BTreeMap`], which this crate does not own. [`ResultOptionalExt`]
/// is the same shape for the same reason.
///
/// It exists because the raw spelling is noise at every call site:
/// `insert(key.to_string(), MetadataValue::String(value))` names a conversion
/// twice — once for the key, once for the value — that the `From` impls above
/// already know how to do. [`AnyError::with_metadata`] has always taken
/// `impl Into<String>`; this is the same courtesy for the dictionary a caller
/// builds before it has an error to hang it on.
///
/// [`ResultOptionalExt`]: crate::ResultOptionalExt
/// [`AnyError::with_metadata`]: crate::AnyError::with_metadata
///
/// ```
/// use bc_error::{MetadataDictionary, MetadataDictionaryExt, MetadataValue};
///
/// let metadata = MetadataDictionary::new()
///     .with("detail", "the node did not answer")
///     .with("attempts", 3_u64);
///
/// assert_eq!(
///     metadata.get("detail"),
///     Some(&MetadataValue::String("the node did not answer".to_owned())),
/// );
/// assert_eq!(metadata.get("attempts"), Some(&MetadataValue::Uint64(3)));
/// ```
pub trait MetadataDictionaryExt: Sized {
    /// Inserts one entry and returns the dictionary, for building by chain.
    #[must_use]
    fn with(self, key: impl Into<String>, value: impl Into<MetadataValue>) -> Self;

    /// Inserts one entry in place.
    ///
    /// The same conversion courtesy as [`with`](Self::with), for a builder that
    /// cannot chain — one whose entries depend on which variant it is
    /// describing, say. [`BTreeMap::insert`] remains the primitive underneath
    /// both, and returning nothing is deliberate: the displaced value is never
    /// what such a builder wants.
    fn set(&mut self, key: impl Into<String>, value: impl Into<MetadataValue>);
}

impl MetadataDictionaryExt for MetadataDictionary {
    fn with(mut self, key: impl Into<String>, value: impl Into<MetadataValue>) -> Self {
        self.set(key, value);
        self
    }

    fn set(&mut self, key: impl Into<String>, value: impl Into<MetadataValue>) {
        self.insert(key.into(), value.into());
    }
}

fn stringify_float(value: f64, decimals: u8) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value == f64::INFINITY {
        return "inf".to_owned();
    }
    if value == f64::NEG_INFINITY {
        return "-inf".to_owned();
    }

    format!("{value:.precision$}", precision = usize::from(decimals))
}

fn stringify_array(array: &MetadataArray, decimals: u8) -> String {
    let values = array
        .iter()
        .map(|value| value.stringify_with_decimals(decimals))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{values}]")
}

fn stringify_dictionary(dictionary: &MetadataDictionary, decimals: u8) -> String {
    let values = dictionary
        .iter()
        .map(|(key, value)| format!("\"{key}\": {}", value.stringify_with_decimals(decimals)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{values}}}")
}

fn stringify_error(error: &AnyError, decimals: u8) -> String {
    let mut fields = vec![
        format!("\"category\": \"{}\"", error.category()),
        format!("\"kind\": {}", error.kind()),
        format!("\"location\": \"{}\"", error.location()),
    ];

    if !error.context().is_empty() {
        let context = error
            .context()
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ");
        fields.push(format!("\"context\": [{context}]"));
    }

    fields.push(format!(
        "\"metadata\": {}",
        stringify_dictionary(error.metadata(), decimals)
    ));
    format!("{{{}}}", fields.join(", "))
}

#[cfg(test)]
mod tests {
    use super::{MetadataType, MetadataValue};
    use crate::{AnyError, AnyErrorKind, MetadataDictionary};

    /// Exhaustive — deliberately no wildcard arm.
    ///
    /// `MetadataValue` is `#[non_exhaustive]`, so an integration test in another
    /// crate is forced to write `_ =>` and could never catch a new variant. This
    /// match is inside the crate, where the attribute does not apply, so adding a
    /// variant without deciding its `MetadataType` fails to compile here.
    fn type_of(value: &MetadataValue) -> MetadataType {
        match value {
            MetadataValue::Null => MetadataType::Null,
            MetadataValue::Boolean(_) => MetadataType::Boolean,
            MetadataValue::Int64(_) => MetadataType::Int64,
            MetadataValue::Uint64(_) => MetadataType::Uint64,
            MetadataValue::Float64(_) => MetadataType::Float64,
            MetadataValue::String(_) => MetadataType::String,
            MetadataValue::Array(_) => MetadataType::Array,
            MetadataValue::Dictionary(_) => MetadataType::Dictionary,
            MetadataValue::Error(_) => MetadataType::Error,
        }
    }

    #[test]
    fn every_variant_agrees_with_its_reported_type() {
        let every_variant = [
            MetadataValue::Null,
            MetadataValue::from(true),
            MetadataValue::from(1_i64),
            MetadataValue::from(1_u64),
            MetadataValue::from(1.0_f64),
            MetadataValue::from("text"),
            MetadataValue::from(vec![MetadataValue::Null]),
            MetadataValue::from(super::MetadataDictionary::new()),
            MetadataValue::from(AnyError::at_location(
                AnyErrorKind::new("c", 1),
                "loc",
                MetadataDictionary::new(),
            )),
        ];

        for value in &every_variant {
            assert_eq!(value.r#type(), type_of(value));
        }
    }
}
