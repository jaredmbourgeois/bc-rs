//! Invariants that must hold for **every** input, and a deterministic generator
//! to throw inputs at them.
//!
//! Split from `tests/properties.rs` so the two halves stay separable: an
//! invariant is a claim about the crate, while a seed count and a generator are
//! a testing strategy. Keeping the claims in one place means a new way of
//! generating input — or a new caller, as the serde bridge will be — checks the
//! same things rather than a lookalike set.

use bc_json::{
    JsonReader, JsonValidationErrorKind, JsonWriter, validate_json, validate_json_bytes,
};

/// No input, however malformed, may make the validator panic.
///
/// Recursive descent over attacker-controlled nesting is the classic way to
/// fail this; the depth limit is what stands in the way.
pub(crate) fn validator_never_panics(bytes: &[u8]) {
    let _ = validate_json_bytes(bytes);
    if let Ok(text) = std::str::from_utf8(bytes) {
        let _ = validate_json(text);
    }
}

/// The unchecked escape hatch must stay panic-free on input that never passed
/// validation.
///
/// Every accessor slices the document by byte offset, so this is where an
/// off-by-one or a mid-character boundary would surface as a panic.
pub(crate) fn unchecked_reader_never_panics(text: &str) {
    let reader = JsonReader::from_trusted_unchecked(text);

    for key in ["k", "", "a\"b", "\u{1f600}", text] {
        let _ = reader.has_field(key);
        let _ = reader.is_null(key);
        let _ = reader.string(key);
        let _ = reader.integer(key);
        let _ = reader.floating(key);
        let _ = reader.boolean(key);
        let _ = reader.number_token(key);
        let _ = reader.find_object(key);
        let _ = reader.find_array(key);
        let _ = reader.string_array(key);
        let _ = reader.object_array(key);
        let _ = reader.element_array(key);
    }

    let _ = reader.is_object();
    let _ = reader.find_first_array();

    // The bare-slice extractors take whatever a caller hands them, so they get
    // the raw input too, not just slices this crate produced.
    let _ = JsonReader::extract_array_elements(text);
    let _ = JsonReader::extract_array_objects(text);
    let _ = JsonReader::extract_string_array(text);
    let _ = JsonReader::extract_number_array(text);
    let _ = JsonReader::extract_object_fields(text);
    let _ = JsonReader::extract_string_map(text);
    let _ = JsonReader::extract_number_map(text);
}

/// Whatever the writer emits must validate, and must read back unchanged.
///
/// This is the crate's two-sided wire contract: the escaper and the parser are
/// separate code that has to agree about every character, and nothing else
/// checks that they do.
pub(crate) fn writer_output_round_trips(value: &str) {
    // Both writer modes. Pretty has its own separator and indent path, and the
    // validator is the only thing that would notice it emitting a stray comma or
    // an indent inside a string.
    round_trips_through(JsonWriter::new(), value);
    round_trips_through(JsonWriter::pretty(), value);
}

/// The key the generated text is written under. Any fixed string would do; this
/// one is short enough to keep a failure message readable.
const FIXED_KEY: &str = "value";
const SENTINEL: &str = "sentinel";

fn round_trips_through(mut writer: JsonWriter, value: &str) {
    // The generated text goes in as a value AND as a key, so key escaping and the
    // reader's decoded-key comparison are exercised too. The one text that cannot
    // do both is FIXED_KEY itself: writing it twice is a duplicate key, which the
    // validator rightly refuses. Guarding the write rather than only the
    // assertion is the point — the other way round, this helper fails on a
    // perfectly good input and blames the writer for it.
    let doubles_as_the_fixed_key = value != FIXED_KEY;
    writer.object(|writer| {
        writer.field(FIXED_KEY, value);
        if doubles_as_the_fixed_key {
            writer.field(value, SENTINEL);
        }
    });
    let json = writer
        .finish()
        .expect("no floats and no duplicate keys are written here");

    let document = validate_json(&json)
        .unwrap_or_else(|error| panic!("writer emitted invalid JSON ({error}): {json}"));
    let reader = JsonReader::new(document);

    // The one lossy character, by design: a NUL is written as U+FFFD because the
    // validator refuses the escape that would carry it, so the text reads back
    // with the replacement character where the NUL was.
    let expected = value.replace('\0', "\u{FFFD}");
    assert_eq!(
        reader.string(FIXED_KEY).as_deref(),
        Some(expected.as_str()),
        "{json}"
    );
    if doubles_as_the_fixed_key {
        assert_eq!(
            reader.string(&expected).as_deref(),
            Some(SENTINEL),
            "{json}"
        );
    }
}

/// Anything `serde_json` accepts, this crate must accept — unless it rejected
/// the input for the one reason it is deliberately stricter about.
///
/// Stated in this direction on purpose. The reverse ("anything this accepts,
/// `serde_json` accepts") is false in two known places: a document nested exactly
/// 128 deep, and a number whose exponent overflows `f64`. Both are measured
/// against the `JSONTestSuite` corpus in `tests/conformance.rs`, where naming them
/// is useful; here they would only make the invariant vague.
pub(crate) fn never_rejects_what_serde_json_accepts(bytes: &[u8]) {
    if serde_json::from_slice::<serde_json::Value>(bytes).is_err() {
        return;
    }

    match validate_json_bytes(bytes) {
        Ok(_) => {}
        Err(error) => assert_eq!(
            error.kind,
            JsonValidationErrorKind::DuplicateKey,
            "serde_json accepted input this crate rejected for an undocumented reason: {:?}",
            String::from_utf8_lossy(bytes)
        ),
    }
}

/// Anything the serde bridge writes must validate and read back equal.
///
/// The serializer reaches the writer's escaper and the deserializer reaches the
/// validator's number grammar by different routes than the native API does, so
/// "the two halves agree" has to be checked on this path too rather than
/// inherited from [`writer_output_round_trips`]. Stated as agreement with
/// `serde_json` because that fixes what the round trip is *supposed* to
/// preserve, rather than letting this crate grade its own homework.
#[cfg(feature = "serde")]
pub(crate) fn serde_round_trip_agrees_with_serde_json(bytes: &[u8]) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return;
    };

    let json = bc_json::to_string(&value)
        .unwrap_or_else(|error| panic!("a value serde_json parsed could not be written: {error}"));
    validate_json(&json)
        .unwrap_or_else(|error| panic!("serializer emitted invalid JSON ({error}): {json}"));

    let read_back: serde_json::Value = bc_json::from_str(&json)
        .unwrap_or_else(|error| panic!("could not read back what was written ({error}): {json}"));
    assert!(
        json_values_agree(&read_back, &value),
        "{json}: read back {read_back} from {value}"
    );
}

/// Whether two `serde_json` values are the same **JSON**.
///
/// Not `==`, because `serde_json::Value` carries an integer-vs-float tag that
/// JSON itself does not have: `61e6` parses as a float and this crate writes it
/// back as `61000000`, since the writer emits a whole-valued float with no
/// fractional part — the C++ source's behavior, and what keeps a wire format
/// from shifting when a value happens to be round. The number survives exactly;
/// only the tag does not, so the comparison is on the value.
#[cfg(feature = "serde")]
fn json_values_agree(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => {
            match (left.as_i64(), right.as_i64()) {
                (Some(left), Some(right)) => left == right,
                // Not both signed: try unsigned before falling back to `f64`, so
                // two distinct integers above 2^53 are not made equal by the
                // conversion that compares them.
                _ => match (left.as_u64(), right.as_u64()) {
                    (Some(left), Some(right)) => left == right,
                    _ => left.as_f64() == right.as_f64(),
                },
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| json_values_agree(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .get(key)
                        .is_some_and(|right| json_values_agree(left, right))
                })
        }
        _ => left == right,
    }
}

/// A seeded xorshift64\* generator.
///
/// Hand-rolled rather than pulled in: it is nine lines, the property tests need
/// reproducibility far more than statistical quality, and a dev-dependency here
/// would be a dependency this crate's whole point is doing without.
pub(crate) struct Generator {
    state: u64,
}

impl Generator {
    /// Starts from `seed`. The same seed always produces the same inputs, so a
    /// failure reported by CI reproduces exactly.
    pub(crate) const fn new(seed: u64) -> Self {
        // Any nonzero state works; xorshift is stuck at zero.
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        let bound = u64::try_from(bound).expect("a usize fits a u64 on every target");
        usize::try_from(self.next_u64() % bound).expect("a value below a usize bound is a usize")
    }

    fn pick<T: Copy>(&mut self, choices: &[T]) -> T {
        choices[self.below(choices.len())]
    }

    /// Uniformly random bytes — mostly noise, which is what proves the validator
    /// rejects rather than panics.
    pub(crate) fn bytes(&mut self, max_len: usize) -> Vec<u8> {
        (0..self.below(max_len + 1))
            .map(|_| u8::try_from(self.next_u64() & 0xFF).expect("masked to one byte"))
            .collect()
    }

    /// Bytes drawn from JSON's own alphabet, so the generator reaches structural
    /// states that uniform noise essentially never does.
    pub(crate) fn json_ish(&mut self, max_len: usize) -> Vec<u8> {
        const ALPHABET: &[u8] = b"{}[]\",:0123456789.eE+-\\ \t\n/truefalsn\x00\x1f\xc3\xa9\xed\xa0";
        (0..self.below(max_len + 1))
            .map(|_| self.pick(ALPHABET))
            .collect()
    }

    /// Arbitrary Unicode text, weighted toward the characters an escaper has to
    /// get right.
    pub(crate) fn text(&mut self, max_chars: usize) -> String {
        const AWKWARD: &[char] = &[
            '"', '\\', '/', '\n', '\r', '\t', '\u{0}', '\u{1}', '\u{1f}', '\u{7f}', '\u{a0}',
            '\u{2028}', '\u{2029}', 'é', '東', '𝄞', '\u{fffd}', ' ', 'a', '0',
        ];
        (0..self.below(max_chars + 1))
            .map(|_| {
                if self.next_u64() % 4 == 0 {
                    // Any scalar value, including the ones no escaper expects.
                    let code_point = u32::try_from(self.next_u64() % 0x11_0000)
                        .expect("below the Unicode range");
                    char::from_u32(code_point).unwrap_or('\u{fffd}')
                } else {
                    self.pick(AWKWARD)
                }
            })
            .collect()
    }
}
