//! The fluent JSON builder.

#[cfg(debug_assertions)]
use std::collections::HashSet;
// `fmt::Write` is what lets `write!` append to a `String` without an intermediate `format!`.
use std::fmt::{self, Write};

/// The compact-mode float budget, in characters.
///
/// Rust's `Display` for `f64` is shortest-round-trip but never uses an
/// exponent, so `1e300` would print as 301 digits. Past this width the port
/// switches to exponent form, which is where the source's 32-byte `snprintf`
/// buffer put the same boundary.
const FIXED_NOTATION_BUDGET: usize = 32;

/// Why a document could not be finished.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JsonWriteError {
    /// A `f64` field or element was `NaN` or an infinity, neither of which JSON
    /// can represent.
    NonFiniteNumber,
    /// The same key was written twice into one object — **in debug builds only**.
    ///
    /// [`validate_json`](crate::validate_json) rejects duplicate keys, which is
    /// what makes [`JsonReader`](crate::JsonReader)'s first-match lookup
    /// unambiguous. A writer able to emit one is therefore a hole in this
    /// crate's contract with itself, and the two halves of that contract are
    /// separate code that nothing else forces to agree.
    ///
    /// Detection costs a set of keys per open object, so a release build does
    /// not pay for it. That makes this a bug caught in the test run that wrote
    /// the code — every `cargo test` is a debug build — rather than a runtime
    /// guarantee a release caller can lean on.
    DuplicateKey,
}

impl fmt::Display for JsonWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteNumber => {
                formatter.write_str("a NaN or infinite number cannot be written as JSON")
            }
            Self::DuplicateKey => {
                formatter.write_str("the same key was written twice into one object")
            }
        }
    }
}

impl std::error::Error for JsonWriteError {}

/// A value that can be written as a JSON scalar.
///
/// C++ spells this as a set of `field` overloads. Rust has no overloading, so
/// the set becomes one trait and `field` / [`array_element`] take any of them —
/// which also gives [`optional_field`] and [`nullable_field`] one definition
/// each instead of one per type.
///
/// Implementing this outside the crate is allowed, and carries an obligation:
/// [`write_json_scalar`](Self::write_json_scalar) must emit exactly one
/// well-formed JSON value. [`JsonWriter::raw_field_unchecked`] is the honest way
/// to splice in a fragment you have already built.
///
/// [`array_element`]: JsonWriter::array_element
/// [`optional_field`]: JsonWriter::optional_field
/// [`nullable_field`]: JsonWriter::nullable_field
pub trait JsonScalar {
    /// Appends this value to `writer`'s buffer. The separator and any key have
    /// already been written.
    fn write_json_scalar(&self, writer: &mut JsonWriter);
}

impl JsonScalar for str {
    fn write_json_scalar(&self, writer: &mut JsonWriter) {
        writer.write_quoted(self);
    }
}

impl JsonScalar for String {
    fn write_json_scalar(&self, writer: &mut JsonWriter) {
        writer.write_quoted(self);
    }
}

impl JsonScalar for bool {
    fn write_json_scalar(&self, writer: &mut JsonWriter) {
        writer.out.push_str(if *self { "true" } else { "false" });
    }
}

impl JsonScalar for f64 {
    fn write_json_scalar(&self, writer: &mut JsonWriter) {
        writer.write_f64(*self);
    }
}

/// Every integer width writes the same way; they differ only in what they hold.
macro_rules! impl_json_scalar_for_integers {
    ($($integer:ty),+ $(,)?) => {
        $(
            impl JsonScalar for $integer {
                fn write_json_scalar(&self, writer: &mut JsonWriter) {
                    writer.out.push_str(&self.to_string());
                }
            }
        )+
    };
}

// `usize` / `isize` are here because JSON numbers have no width and callers
// hold lengths and indices in them constantly — without these, every
// `field("count", items.len())` needs an `as i64` the reader would then have to
// justify.
impl_json_scalar_for_integers!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

/// A blanket impl so `&T` works wherever `T` does — `w.field("k", &name)` and
/// `w.field("k", name.as_str())` should not be different APIs.
impl<T: JsonScalar + ?Sized> JsonScalar for &T {
    fn write_json_scalar(&self, writer: &mut JsonWriter) {
        (*self).write_json_scalar(writer);
    }
}

/// Builds a JSON document.
///
/// Containers take a closure that receives the writer, so nesting is structural
/// rather than a matched pair of calls you can get wrong:
///
/// ```
/// let mut writer = bc_json::JsonWriter::new();
/// writer.object(|writer| {
///     writer.field("id", "x1");
///     writer.array("items", |writer| {
///         writer.array_value(|writer| writer.object(|writer| writer.field("n", 1_i64)));
///     });
/// });
/// assert_eq!(writer.finish()?, r#"{"id":"x1","items":[{"n":1}]}"#);
/// # Ok::<(), bc_json::JsonWriteError>(())
/// ```
///
/// # Separator discipline
///
/// Every container form saves the outer "needs a comma" flag and restores it on
/// close, so an empty `{}` or `[]` followed by a sibling still gets its comma —
/// the bug each of the three writers this consolidates worked around
/// differently.
#[derive(Debug)]
pub struct JsonWriter {
    out: String,
    first: bool,
    pretty: bool,
    depth: usize,
    /// The first reason this document cannot be written. Checked once, at
    /// [`finish`](Self::finish), rather than at every call site — a fluent
    /// builder whose every call returns a `Result` is not fluent.
    invalid: Option<JsonWriteError>,
    /// The keys already written into each currently-open object, innermost last.
    ///
    /// Debug builds only; see [`JsonWriteError::DuplicateKey`] for why the check
    /// exists at all and why a release build does not pay for it.
    #[cfg(debug_assertions)]
    open_object_keys: Vec<HashSet<String>>,
}

impl Default for JsonWriter {
    /// Hand-written rather than derived: the separator flag starts `true`, and
    /// a derived `Default` would start it `false` and drop the first comma.
    fn default() -> Self {
        Self::new()
    }
}

impl JsonWriter {
    /// A writer emitting compact, single-line JSON.
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: String::new(),
            first: true,
            pretty: false,
            depth: 0,
            invalid: None,
            #[cfg(debug_assertions)]
            open_object_keys: Vec::new(),
        }
    }

    /// A writer emitting two-space-indented JSON, for output a human reads or a
    /// `git diff` has to show field by field.
    ///
    /// Empty containers still collapse to `{}` / `[]`, and the result parses
    /// identically — the reader skips whitespace.
    #[must_use]
    pub fn pretty() -> Self {
        Self {
            pretty: true,
            ..Self::new()
        }
    }

    /// Writes an object, calling `body` to write its members.
    pub fn object(&mut self, body: impl FnOnce(&mut Self)) {
        let outer = self.begin_container('{');
        body(self);
        self.end_container('}', outer);
    }

    /// Writes `key`'s value as an object, calling `body` to write its members.
    pub fn nested_object(&mut self, key: &str, body: impl FnOnce(&mut Self)) {
        self.separator();
        self.write_key(key);
        let outer = self.begin_container('{');
        body(self);
        self.end_container('}', outer);
    }

    /// Writes `key`'s value as an array, calling `body` to write its elements.
    pub fn array(&mut self, key: &str, body: impl FnOnce(&mut Self)) {
        self.separator();
        self.write_key(key);
        let outer = self.begin_container('[');
        body(self);
        self.end_container(']', outer);
    }

    /// Writes an array with no key — a document whose root is an array.
    pub fn bare_array(&mut self, body: impl FnOnce(&mut Self)) {
        let outer = self.begin_container('[');
        body(self);
        self.end_container(']', outer);
    }

    /// Writes one element of the enclosing array, calling `body` to write the
    /// value. Use this for elements that are objects or arrays;
    /// [`array_element`](Self::array_element) is the scalar form.
    pub fn array_value(&mut self, body: impl FnOnce(&mut Self)) {
        self.separator();
        body(self);
    }

    /// Writes one scalar element of the enclosing array.
    ///
    /// Replaces the source's `array_string` / `array_number` / `array_bool`,
    /// which existed only because C++ needed one overload per type.
    pub fn array_element(&mut self, value: impl JsonScalar) {
        self.separator();
        value.write_json_scalar(self);
    }

    /// Writes `null` as one element of the enclosing array.
    pub fn array_null(&mut self) {
        self.separator();
        self.out.push_str("null");
    }

    /// Writes `key: value`.
    pub fn field(&mut self, key: &str, value: impl JsonScalar) {
        self.separator();
        self.write_key(key);
        value.write_json_scalar(self);
    }

    /// Writes `key: null`.
    pub fn field_null(&mut self, key: &str) {
        self.separator();
        self.write_key(key);
        self.out.push_str("null");
    }

    /// Writes `key`'s value as a pre-built JSON fragment, verbatim.
    ///
    /// Nothing is quoted or escaped, and nothing is checked. The caller
    /// guarantees `raw_json` is one well-formed JSON value — the intended use
    /// is re-wrapping a body that has already been validated.
    ///
    /// Named `_unchecked` for the same reason as
    /// [`JsonReader::from_trusted_unchecked`](crate::JsonReader::from_trusted_unchecked):
    /// these are the two ways to defeat the guarantee that everything this crate
    /// emits validates, and a reviewer should see both at the call site rather
    /// than have to know which innocuous-looking name is the escape hatch.
    pub fn raw_field_unchecked(&mut self, key: &str, raw_json: &str) {
        self.separator();
        self.write_key(key);
        self.out.push_str(raw_json);
    }

    /// Writes `key` only when `value` is `Some` — absent means the key is
    /// omitted entirely.
    ///
    /// Which of the three "no value" spellings to use is a per-field decision,
    /// because every client can see the difference: omitting lets a parser's
    /// own default win, while [`nullable_field`](Self::nullable_field) asserts
    /// positively that the value is unset.
    pub fn optional_field(&mut self, key: &str, value: Option<impl JsonScalar>) {
        if let Some(value) = value {
            self.field(key, value);
        }
    }

    /// Writes `key`, using an explicit `null` when `value` is `None`.
    pub fn nullable_field(&mut self, key: &str, value: Option<impl JsonScalar>) {
        match value {
            Some(value) => self.field(key, value),
            None => self.field_null(key),
        }
    }

    /// Writes `key: null` when `include`, and nothing otherwise — the form for
    /// a caller whose decision is a condition rather than an `Option`.
    pub fn optional_field_null(&mut self, key: &str, include: bool) {
        if include {
            self.field_null(key);
        }
    }

    /// Writes `key`'s value as an array of `values`.
    pub fn string_array<I>(&mut self, key: &str, values: I)
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        self.array(key, |writer| {
            for value in values {
                writer.array_element(value.as_ref());
            }
        });
    }

    /// The document written so far.
    ///
    /// # Errors
    ///
    /// [`JsonWriteError::NonFiniteNumber`] if any `f64` written was `NaN` or an
    /// infinity. The source emits those verbatim and produces invalid JSON; the
    /// port makes it observable instead, at one place rather than at every call.
    ///
    /// In a debug build, also [`JsonWriteError::DuplicateKey`].
    pub fn as_str(&self) -> Result<&str, JsonWriteError> {
        match self.invalid {
            Some(error) => Err(error),
            None => Ok(&self.out),
        }
    }

    /// Consumes the writer and returns the document.
    ///
    /// # Errors
    ///
    /// As [`as_str`](Self::as_str).
    pub fn finish(self) -> Result<String, JsonWriteError> {
        match self.invalid {
            Some(error) => Err(error),
            None => Ok(self.out),
        }
    }

    /// Opens `delimiter` and resets the separator state, returning the outer
    /// state for [`end_container`](Self::end_container) to restore.
    ///
    /// The public API pairs these inside one closure-taking method, so a caller
    /// cannot forget the close. The serde `Serializer` cannot: serde's protocol
    /// hands out a begin call and an end call with the caller's own code in
    /// between, so the pair is split here rather than duplicated there — one
    /// separator discipline, two ways to reach it.
    pub(crate) fn begin_container(&mut self, delimiter: char) -> bool {
        self.out.push(delimiter);
        let outer = self.first;
        self.first = true;
        self.depth += 1;
        #[cfg(debug_assertions)]
        self.open_object_keys.push(HashSet::new());
        outer
    }

    /// Closes a container opened by [`begin_container`](Self::begin_container),
    /// restoring the separator state it returned — the discipline that keeps an
    /// empty `{}` or `[]` from swallowing its sibling's comma.
    pub(crate) fn end_container(&mut self, closer: char, outer: bool) {
        #[cfg(debug_assertions)]
        self.open_object_keys.pop();
        self.depth -= 1;
        if !self.first {
            self.newline_indent();
        }
        self.first = outer;
        self.out.push(closer);
    }

    pub(crate) fn separator(&mut self) {
        if !self.first {
            self.out.push(',');
        }
        self.first = false;
        self.newline_indent();
    }

    /// Pretty mode only: a line break and two spaces per nesting level.
    fn newline_indent(&mut self) {
        if !self.pretty {
            return;
        }
        self.out.push('\n');
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
    }

    pub(crate) fn write_key(&mut self, key: &str) {
        #[cfg(debug_assertions)]
        self.record_key(key);
        self.write_quoted(key);
        self.out.push(':');
        if self.pretty {
            self.out.push(' ');
        }
    }

    /// Poisons the document if `key` has already been written into the object
    /// currently open.
    ///
    /// The key is compared **decoded**, which is what
    /// [`validate_json`](crate::validate_json) does, because the writer escapes
    /// on the way out: the `&str` a caller passes is already the decoded form.
    /// Keys written with no object open — a caller building a fragment by hand —
    /// are not tracked, since there is no object for them to collide within.
    #[cfg(debug_assertions)]
    fn record_key(&mut self, key: &str) {
        let duplicate = self
            .open_object_keys
            .last_mut()
            .is_some_and(|keys| !keys.insert(key.to_owned()));
        if duplicate {
            self.poison(JsonWriteError::DuplicateKey);
        }
    }

    /// Records why the document cannot be written. First cause wins: a later
    /// failure is often downstream of the first, and the first is the one to go
    /// fix.
    pub(crate) fn poison(&mut self, error: JsonWriteError) {
        if self.invalid.is_none() {
            self.invalid = Some(error);
        }
    }

    pub(crate) fn write_quoted(&mut self, value: &str) {
        self.out.push('"');
        for character in value.chars() {
            match character {
                '"' => self.out.push_str("\\\""),
                '\\' => self.out.push_str("\\\\"),
                '\n' => self.out.push_str("\\n"),
                '\r' => self.out.push_str("\\r"),
                '\t' => self.out.push_str("\\t"),
                // The validator refuses the U+0000 escape (`NulCharacter`), so a NUL
                // is written as the replacement character — the ill-formed-UTF-8
                // shape — and everything this writer emits still validates.
                '\0' => self.out.push(char::REPLACEMENT_CHARACTER),
                '\u{8}' => self.out.push_str("\\b"),
                '\u{c}' => self.out.push_str("\\f"),
                character if u32::from(character) < 0x20 => {
                    // Writing into a `String` cannot fail, so its `fmt::Result` carries nothing.
                    let _ = write!(self.out, "\\u{:04x}", u32::from(character));
                }
                character => self.out.push(character),
            }
        }
        self.out.push('"');
    }

    pub(crate) fn write_f64(&mut self, value: f64) {
        self.write_float(value, value.is_finite());
    }

    #[cfg(feature = "serde")]
    /// Written at `f32` width on purpose: `f32::to_string` is shortest-round-trip
    /// for an `f32`, so `0.1_f32` stays `0.1` where widening to `f64` first would
    /// print the seventeen digits the widening introduced.
    pub(crate) fn write_f32(&mut self, value: f32) {
        self.write_float(value, value.is_finite());
    }

    fn write_float<F>(&mut self, value: F, is_finite: bool)
    where
        F: fmt::Display + fmt::LowerExp,
    {
        if !is_finite {
            // Poison the document and write a placeholder, so a Debug dump of a
            // failed build is still readable. `finish` is what reports it.
            self.poison(JsonWriteError::NonFiniteNumber);
            self.out.push_str("null");
            return;
        }

        let fixed = value.to_string();
        if fixed.len() < FIXED_NOTATION_BUDGET {
            self.out.push_str(&fixed);
        } else {
            // Writing into a `String` cannot fail, so its `fmt::Result` carries nothing.
            let _ = write!(self.out, "{value:e}");
        }
    }

    // --- Raw value writers, for the serde bridge --------------------------------
    //
    // Each writes one JSON value and nothing else — no separator, no key. Serde's
    // protocol emits those itself, one layer up, which is the same split the
    // `JsonScalar` impls above rely on. Feature-gated because they have exactly
    // one caller, and an uncompiled `serde` build should not carry it.

    #[cfg(feature = "serde")]
    pub(crate) fn write_bool(&mut self, value: bool) {
        self.out.push_str(if value { "true" } else { "false" });
    }

    #[cfg(feature = "serde")]
    pub(crate) fn write_integer(&mut self, value: i64) {
        self.out.push_str(&value.to_string());
    }

    #[cfg(feature = "serde")]
    pub(crate) fn write_unsigned(&mut self, value: u64) {
        self.out.push_str(&value.to_string());
    }

    #[cfg(feature = "serde")]
    pub(crate) fn write_null(&mut self) {
        self.out.push_str("null");
    }
}

/// `{"error":"<message>"}` — the canonical one-line error body.
#[must_use]
pub fn json_error(message: &str) -> String {
    let mut writer = JsonWriter::new();
    writer.object(|writer| writer.field("error", message));
    // These two are the only places that take the buffer without going through
    // `finish`, which is what turns a poisoned writer into an error. They can do
    // that because they write one string into one object and nothing else — the
    // assertion is here so that stays true if the body ever grows.
    debug_assert!(
        writer.invalid.is_none(),
        "json_error writes a single string field and cannot fail"
    );
    writer.out
}

/// `{"ok":true}` — the canonical one-line success body.
#[must_use]
pub fn json_ok() -> String {
    let mut writer = JsonWriter::new();
    writer.object(|writer| writer.field("ok", true));
    debug_assert!(
        writer.invalid.is_none(),
        "json_ok writes a single bool field and cannot fail"
    );
    writer.out
}
