//! The lazy field accessor.
//!
//! A [`JsonReader`] borrows a document and scans it on demand: no value tree is
//! built, and every slice it hands back points into the caller's buffer. That
//! is only unambiguous because [`validate_json`](crate::validate_json) has
//! already rejected duplicate keys — see the crate docs.

use crate::detail;
use crate::number::{parse_json_f64, parse_json_i64};
use crate::validate::Validated;

/// Reads fields from one JSON object.
///
/// Every keyed accessor is **top-level only**: it looks at the immediate
/// members of the object this reader was built on, and never searches inside a
/// nested object or array. To reach a nested key, take the nested value with
/// [`find_object`](Self::find_object) or [`find_array`](Self::find_array) and
/// read from that — one hop per level, explicitly.
///
/// Deliberately not comparable. A derived `PartialEq` here would compare the
/// underlying bytes, so `{"a":1}` and `{ "a" : 1 }` would be unequal — a
/// plausible-looking `==` that answers a question about formatting when the
/// caller meant to ask one about JSON. Compare the values you read instead.
#[derive(Clone, Copy, Debug)]
pub struct JsonReader<'a> {
    json: &'a str,
}

impl<'a> From<Validated<'a>> for JsonReader<'a> {
    fn from(document: Validated<'a>) -> Self {
        Self::new(document)
    }
}

impl<'a> JsonReader<'a> {
    /// Reads a document that has passed validation.
    ///
    /// This is the ordinary constructor, and it takes a [`Validated`] rather
    /// than a string on purpose: the reader returns the **first** member with a
    /// given key, which is only well defined once duplicate keys have been
    /// ruled out.
    ///
    /// ```
    /// let body = r#"{"name":"Alice","age":30}"#;
    /// let reader = bc_json::JsonReader::new(bc_json::validate_json(body)?);
    /// assert_eq!(reader.string("name").as_deref(), Some("Alice"));
    /// assert_eq!(reader.integer("age"), Some(30));
    /// # Ok::<(), bc_json::JsonValidationError>(())
    /// ```
    ///
    /// The gate is structural, not advisory — a document that has not been
    /// validated has no way in:
    ///
    /// ```compile_fail
    /// // `&str` is not `Validated`.
    /// let reader = bc_json::JsonReader::new(r#"{"id":1}"#);
    /// ```
    ///
    /// And, unlike the C++ this is ported from, a reader cannot outlive the
    /// buffer it reads. That shape shipped to production twice there before a
    /// deleted constructor overload turned it into a compile error; here the
    /// borrow checker has always known:
    ///
    /// ```compile_fail
    /// let reader = {
    ///     let body = String::from(r#"{"id":1}"#);
    ///     bc_json::JsonReader::new(bc_json::validate_json(&body).unwrap())
    /// };
    /// let _ = reader.integer("id");
    /// ```
    #[must_use]
    pub const fn new(document: Validated<'a>) -> Self {
        Self {
            json: document.as_str(),
        }
    }

    /// Reads a document without validating it.
    ///
    /// Named to be unmissable in review, because it gives up the guarantee the
    /// type is for. Two honest uses: input this process just produced itself,
    /// and a slice already carved out of a validated document. On anything that
    /// crossed a trust boundary, validate instead — a duplicate key makes
    /// "first match" an attacker's choice rather than the document's.
    #[must_use]
    pub const fn from_trusted_unchecked(json: &'a str) -> Self {
        Self { json }
    }

    /// The document this reader was built on.
    #[must_use]
    pub const fn raw(&self) -> &'a str {
        self.json
    }

    /// Whether `key` is present, regardless of its value.
    ///
    /// A key whose value is `null` is present — that is the distinction the
    /// typed accessors cannot make.
    #[must_use]
    pub fn has_field(&self, key: &str) -> bool {
        self.value_position(key).is_some()
    }

    /// Whether `key`'s value is the literal `null`. `false` when absent.
    #[must_use]
    pub fn is_null(&self, key: &str) -> bool {
        self.scalar_token(key) == Some("null")
    }

    /// Whether the document is a non-empty JSON object.
    ///
    /// A structural check only — the first non-whitespace byte is `{` and the
    /// last is `}` — so it can tell a malformed body from an object that simply
    /// lacks every field the caller wanted, without a full parse.
    #[must_use]
    pub fn is_object(&self) -> bool {
        let bytes = self.json.as_bytes();
        let begin = detail::skip_ws(self.json, 0);
        if bytes.get(begin) != Some(&b'{') {
            return false;
        }
        let mut end = bytes.len();
        while end > begin && matches!(bytes[end - 1], b' ' | b'\t' | b'\n' | b'\r') {
            end -= 1;
        }
        bytes[end - 1] == b'}'
    }

    /// `key`'s value as a decoded string, or `None` if it is absent or not a
    /// string.
    #[must_use]
    pub fn string(&self, key: &str) -> Option<String> {
        let position = detail::skip_ws(self.json, self.value_position(key)?);
        if self.json.as_bytes().get(position) != Some(&b'"') {
            return None;
        }
        let mut value = String::new();
        detail::parse_json_string(self.json, position, &mut value)?;
        Some(value)
    }

    /// `key`'s value as an `i64`, or `None` if it is absent or is not a JSON
    /// number that fits exactly.
    #[must_use]
    pub fn integer(&self, key: &str) -> Option<i64> {
        parse_json_i64(self.scalar_token(key)?)
    }

    /// `key`'s value as an `f64`, or `None` if it is absent or is not a finite
    /// JSON number.
    ///
    /// Absent and malformed are both `None`. When the difference matters,
    /// combine [`has_field`](Self::has_field) with this.
    #[must_use]
    pub fn floating(&self, key: &str) -> Option<f64> {
        parse_json_f64(self.scalar_token(key)?)
    }

    /// `key`'s value as a `bool`, or `None` if it is absent or not a boolean.
    #[must_use]
    pub fn boolean(&self, key: &str) -> Option<bool> {
        match self.scalar_token(key)? {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }

    /// `key`'s raw numeric token — the value bytes up to the next delimiter,
    /// undecoded and unconverted, e.g. `-125.50`.
    ///
    /// For exact-decimal work where `f64` would round: money, above all. The
    /// token is delimiter-bounded, so this is for numbers and bare literals —
    /// a quoted string containing a comma would come back truncated. Use
    /// [`string`](Self::string) for those.
    #[must_use]
    pub fn number_token(&self, key: &str) -> Option<&'a str> {
        self.scalar_token(key)
    }

    /// `key`'s value as a nested object to read from.
    #[must_use]
    pub fn find_object(&self, key: &str) -> Option<Self> {
        let position = detail::skip_ws(self.json, self.value_position(key)?);
        if self.json.as_bytes().get(position) != Some(&b'{') {
            return None;
        }
        let end = find_matching(self.json, position, b'{', b'}')?;
        Some(Self {
            json: &self.json[position..=end],
        })
    }

    /// `key`'s value as an array slice, including its brackets.
    ///
    /// Pair with [`extract_array_elements`](Self::extract_array_elements) or a
    /// sibling extractor; [`element_array`](Self::element_array) and friends do
    /// both steps at once.
    #[must_use]
    pub fn find_array(&self, key: &str) -> Option<&'a str> {
        let position = detail::skip_ws(self.json, self.value_position(key)?);
        if self.json.as_bytes().get(position) != Some(&b'[') {
            return None;
        }
        let end = find_matching(self.json, position, b'[', b']')?;
        Some(&self.json[position..=end])
    }

    /// The first array-valued member, whatever its key.
    ///
    /// For a caller that wants "the list" out of a single-list response without
    /// knowing which key holds it. The walk is structural, so a string value
    /// containing `[` never matches. `None` means there is no array member;
    /// a present-but-empty array yields the `[]` slice.
    #[must_use]
    pub fn find_first_array(&self) -> Option<&'a str> {
        let json = self.json;
        let bytes = json.as_bytes();
        let mut position = detail::skip_ws(json, 0);
        if bytes.get(position) != Some(&b'{') {
            return None;
        }
        position += 1;

        let mut key = String::new();
        while position < bytes.len() {
            position = detail::skip_ws(json, position);
            if bytes.get(position) != Some(&b'"') {
                return None;
            }
            position = detail::parse_json_string(json, position, &mut key)?;
            position = detail::skip_ws(json, position);
            if bytes.get(position) != Some(&b':') {
                return None;
            }
            position = detail::skip_ws(json, position + 1);
            if position >= bytes.len() {
                return None;
            }
            if bytes[position] == b'[' {
                let end = find_matching(json, position, b'[', b']')?;
                return Some(&json[position..=end]);
            }
            position = detail::skip_json_value(json, position)?;
            position = detail::skip_ws(json, position);
            if bytes.get(position) == Some(&b',') {
                position += 1;
            }
        }

        None
    }

    /// `key`'s value as decoded string elements, **stopping at the first
    /// element that is not a string**.
    ///
    /// Empty if the key is absent or the value is not an array; otherwise the
    /// decoded prefix. `["a", "b", 1, "c"]` yields `["a", "b"]`, not `[]` and
    /// not an error — a shortened list is indistinguishable from a shorter one
    /// here. A caller that must tell those apart wants
    /// [`required_string_array`](Self::required_string_array).
    #[must_use]
    pub fn string_array(&self, key: &str) -> Vec<String> {
        self.find_array(key)
            .map(Self::extract_string_array)
            .unwrap_or_default()
    }

    /// `key`'s value as readers over its object elements, **stopping at the
    /// first element that is not an object**.
    ///
    /// Empty if the key is absent or the value is not an array; otherwise the
    /// prefix, on the same terms as [`string_array`](Self::string_array). See
    /// [`required_object_array`](Self::required_object_array) to refuse instead.
    #[must_use]
    pub fn object_array(&self, key: &str) -> Vec<Self> {
        self.find_array(key)
            .map(Self::extract_array_objects)
            .unwrap_or_default()
    }

    /// `key`'s value as one raw slice per element, of whatever type. Empty if
    /// the key is absent or the value is not an array.
    #[must_use]
    pub fn element_array(&self, key: &str) -> Vec<&'a str> {
        self.find_array(key)
            .map(Self::extract_array_elements)
            .unwrap_or_default()
    }

    /// Splits an array slice into one raw slice per element, of any type.
    #[must_use]
    pub fn extract_array_elements(array_json: &'a str) -> Vec<&'a str> {
        let Some(mut position) = array_body_start(array_json) else {
            return Vec::new();
        };
        let bytes = array_json.as_bytes();
        let mut elements = Vec::new();

        position = detail::skip_ws(array_json, position);
        while position < bytes.len() && bytes[position] != b']' {
            let Some(end) = detail::skip_json_value(array_json, position) else {
                break;
            };
            if end <= position {
                break;
            }
            elements.push(&array_json[position..end]);
            position = detail::skip_ws(array_json, end);
            if bytes.get(position) == Some(&b',') {
                position = detail::skip_ws(array_json, position + 1);
            }
        }

        elements
    }

    /// Splits an array slice into a reader per object element, stopping at the
    /// first element that is not an object.
    #[must_use]
    pub fn extract_array_objects(array_json: &'a str) -> Vec<Self> {
        let Some(mut position) = array_body_start(array_json) else {
            return Vec::new();
        };
        let bytes = array_json.as_bytes();
        let mut objects = Vec::new();

        while position < bytes.len() {
            position = detail::skip_ws(array_json, position);
            if bytes.get(position) != Some(&b'{') {
                break;
            }
            let Some(end) = find_matching(array_json, position, b'{', b'}') else {
                break;
            };
            objects.push(Self {
                json: &array_json[position..=end],
            });
            position = detail::skip_ws(array_json, end + 1);
            if bytes.get(position) == Some(&b',') {
                position += 1;
            }
        }

        objects
    }

    /// Decodes an array slice of strings, stopping at the first element that is
    /// not a string.
    ///
    /// Empty if the slice is not an array; otherwise the decoded prefix — the
    /// same prefix rule [`string_array`](Self::string_array) documents, and the
    /// reason a strict caller wants
    /// [`required_string_array`](Self::required_string_array).
    #[must_use]
    pub fn extract_string_array(array_json: &str) -> Vec<String> {
        let Some(mut position) = array_body_start(array_json) else {
            return Vec::new();
        };
        let bytes = array_json.as_bytes();
        let mut values = Vec::new();

        while position < bytes.len() {
            position = detail::skip_ws(array_json, position);
            if bytes.get(position) != Some(&b'"') {
                break;
            }
            let mut value = String::new();
            let Some(end) = detail::parse_json_string(array_json, position, &mut value) else {
                break;
            };
            values.push(value);
            position = detail::skip_ws(array_json, end);
            if bytes.get(position) == Some(&b',') {
                position += 1;
            }
        }

        values
    }

    /// Decodes an array slice of numbers, skipping any element that is not one.
    #[must_use]
    pub fn extract_number_array(array_json: &str) -> Vec<f64> {
        let Some(mut position) = array_body_start(array_json) else {
            return Vec::new();
        };
        let bytes = array_json.as_bytes();
        let mut values = Vec::new();

        while position < bytes.len() {
            position = detail::skip_ws(array_json, position);
            if position >= bytes.len() || bytes[position] == b']' {
                break;
            }
            let end = detail::scalar_token_end(array_json, position);
            if end == position {
                // The byte here is itself a delimiter, so there is no token to
                // read — `[}` and friends. Without this the loop makes no
                // progress and spins forever on malformed input. (The source
                // avoids it by accident: its delimiter set for this one function
                // omits `}`, so the scan runs on to the real delimiter.)
                break;
            }
            if let Some(value) = parse_json_f64(&array_json[position..end]) {
                values.push(value);
            }
            position = detail::skip_ws(array_json, end);
            if bytes.get(position) == Some(&b',') {
                position += 1;
            }
        }

        values
    }

    /// Enumerates an object slice's **direct** members as key → raw value
    /// slice, in document order.
    ///
    /// The primitive for open-ended keys, where the typed accessors cannot help
    /// because the caller does not know the key names. Values come back as raw
    /// JSON; wrap an object value in a reader to read it.
    #[must_use]
    pub fn extract_object_fields(object_json: &'a str) -> Vec<(String, &'a str)> {
        let Some(mut position) = object_body_start(object_json) else {
            return Vec::new();
        };
        let bytes = object_json.as_bytes();
        let mut fields = Vec::new();

        while position < bytes.len() {
            let Some((key, after_key)) = next_key(object_json, position) else {
                break;
            };
            let Some(value_start) = value_start(object_json, after_key) else {
                break;
            };
            let Some(end) = detail::skip_json_value(object_json, value_start) else {
                break;
            };
            if end < value_start {
                break;
            }
            fields.push((key, &object_json[value_start..end]));
            position = advance_past_comma(object_json, end);
        }

        fields
    }

    /// Enumerates an object slice's string-valued members, skipping the rest.
    #[must_use]
    pub fn extract_string_map(object_json: &str) -> Vec<(String, String)> {
        let Some(mut position) = object_body_start(object_json) else {
            return Vec::new();
        };
        let bytes = object_json.as_bytes();
        let mut entries = Vec::new();

        while position < bytes.len() {
            let Some((key, after_key)) = next_key(object_json, position) else {
                break;
            };
            let Some(value_start) = value_start(object_json, after_key) else {
                break;
            };

            let end = if bytes[value_start] == b'"' {
                let mut value = String::new();
                let Some(end) = detail::parse_json_string(object_json, value_start, &mut value)
                else {
                    break;
                };
                entries.push((key, value));
                end
            } else {
                // Not a string: skip the whole value, nested containers and all,
                // and keep enumerating.
                let Some(end) = detail::skip_json_value(object_json, value_start) else {
                    break;
                };
                end
            };

            position = advance_past_comma(object_json, end);
        }

        entries
    }

    /// Enumerates an object slice's number-valued members, skipping the rest.
    #[must_use]
    pub fn extract_number_map(object_json: &str) -> Vec<(String, f64)> {
        let Some(mut position) = object_body_start(object_json) else {
            return Vec::new();
        };
        let bytes = object_json.as_bytes();
        let mut entries = Vec::new();

        while position < bytes.len() {
            let Some((key, after_key)) = next_key(object_json, position) else {
                break;
            };
            let Some(value_start) = value_start(object_json, after_key) else {
                break;
            };

            let end = if bytes[value_start] == b'"' {
                // Skip through the string decoder rather than by delimiter, so
                // an escaped quote inside the value cannot end it early.
                let mut ignored = String::new();
                let Some(end) = detail::parse_json_string(object_json, value_start, &mut ignored)
                else {
                    break;
                };
                end
            } else {
                let Some(end) = detail::skip_json_value(object_json, value_start) else {
                    break;
                };
                if let Some(value) = parse_json_f64(&object_json[value_start..end]) {
                    entries.push((key, value));
                }
                end
            };

            position = advance_past_comma(object_json, end);
        }

        entries
    }

    /// The position just past the colon of `key`'s member, or `None`.
    fn value_position(&self, key: &str) -> Option<usize> {
        detail::find_field(self.json, 0, key)
    }

    /// `key`'s value as its raw delimiter-bounded token — the shared scan
    /// behind every unquoted-scalar accessor.
    fn scalar_token(&self, key: &str) -> Option<&'a str> {
        let position = detail::skip_ws(self.json, self.value_position(key)?);
        if position >= self.json.len() {
            return None;
        }
        let end = detail::scalar_token_end(self.json, position);
        Some(&self.json[position..end])
    }
}

/// The position just inside `[`, or `None` if the slice does not open an array.
fn array_body_start(array_json: &str) -> Option<usize> {
    let start = detail::skip_ws(array_json, 0);
    if array_json.as_bytes().get(start) == Some(&b'[') {
        Some(start + 1)
    } else {
        None
    }
}

/// The position just inside `{`, or `None` if the slice does not open an object.
fn object_body_start(object_json: &str) -> Option<usize> {
    let start = detail::skip_ws(object_json, 0);
    if object_json.as_bytes().get(start) == Some(&b'{') {
        Some(start + 1)
    } else {
        None
    }
}

/// Decodes the next member key at `position`, returning it and the position
/// just past the closing quote. `None` at the object's end or on malformed
/// input — the enumerators treat both the same way, by stopping.
fn next_key(object_json: &str, position: usize) -> Option<(String, usize)> {
    let position = detail::skip_ws(object_json, position);
    if object_json.as_bytes().get(position) != Some(&b'"') {
        return None;
    }
    let mut key = String::new();
    let end = detail::parse_json_string(object_json, position, &mut key)?;
    Some((key, end))
}

/// The position of the value after a member key, having consumed the colon.
fn value_start(object_json: &str, after_key: usize) -> Option<usize> {
    let position = detail::skip_ws(object_json, after_key);
    if object_json.as_bytes().get(position) != Some(&b':') {
        return None;
    }
    let start = detail::skip_ws(object_json, position + 1);
    if start >= object_json.len() {
        return None;
    }
    Some(start)
}

/// Steps past the separator between members, if there is one.
fn advance_past_comma(object_json: &str, position: usize) -> usize {
    let position = detail::skip_ws(object_json, position);
    if object_json.as_bytes().get(position) == Some(&b',') {
        position + 1
    } else {
        position
    }
}

/// The index of the `close` that balances the `open` at `start`, ignoring any
/// that appear inside a string.
///
/// A thin forward to [`detail::skip_container`], which answers the same
/// question one byte further on. This was a second string-aware brace matcher
/// with its own escape handling and its own depth arithmetic; the two agreed by
/// authorship alone, and a fix to either would have left the other unchanged.
fn find_matching(input: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    // `skip_container` returns the position *after* the close, and only ever a
    // position it actually reached, so the close is always at `after - 1`.
    detail::skip_container(input.as_bytes(), start, open, close).map(|after| after - 1)
}
