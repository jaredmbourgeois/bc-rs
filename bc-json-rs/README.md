# bc-json-rs

Strict RFC 8259 JSON: a validator that refuses everything the RFC refuses, and duplicate keys and the `\u0000` escape
besides; a lazy zero-copy reader over validated input; and a fluent writer that cannot emit invalid JSON.

The default build has **no dependencies** and builds for `wasm32-unknown-unknown`; the optional `serde` feature makes
it a serde format over the same escaper, number grammar and validator. Its conformance suite runs the whole
JSONTestSuite parsing corpus.

Depend on it by git and release tag, as the [workspace README](../README.md#depending-on-bc-rs) shows; the package is
`bc-json-rs` and the library is imported as `bc_json`.

It is a semantics-preserving port of the C++ `libs/foundation/json` library of Bourgeois Concern's codebase: the same
API and behaviour, with every case of the original's test suite carried into this crate's tests or set aside for a
recorded reason.

Licensed under Apache-2.0 ([LICENSE.txt](LICENSE.txt)); copyright 2026 Jared Bourgeois.

## The two halves, and why they stay two halves

`validate_json` is a strict whole-document gate. `JsonReader` is a lazy per-key scanner that
builds no value tree and hands back slices of the caller's buffer. They are separate on
purpose, and the separation is what makes the reader **sound**, not merely cheap:

> The reader returns the **first** member matching a key. The validator rejects duplicate
> decoded keys. So a document that reaches the reader has exactly one member per key, and
> "first" is indistinguishable from any other choice.

In C++ that composition was a documented convention, honored at 92 of 475 call sites. Here it
is a type — and that is this port's headline change.

```rust
let body = r#"{"id":"a1","items":[{"n":1},{"n":2}]}"#;
let reader = bc_json::JsonReader::new(bc_json::validate_json(body)?);

assert_eq!(reader.string("id").as_deref(), Some("a1"));
let items = reader.object_array("items");
assert_eq!(items[1].integer("n"), Some(2));

let mut writer = bc_json::JsonWriter::new();
writer.object(|writer| {
    writer.field("ok", true);
    writer.field("count", items.len());
});
assert_eq!(writer.finish()?, r#"{"ok":true,"count":2}"#);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Public API

- **`validate_json(&str)`** and **`validate_json_bytes(&[u8])`** return a `Validated<'_>` or a
  `JsonValidationError { kind, byte_offset }`. `JsonValidationErrorKind` is a
  `#[non_exhaustive]` `#[repr(i32)]` enum with written-out discriminants, because they are a
  wire value carried into `AnyError` and across any language binding.
  `JsonValidationError::line_column(input)` resolves the offset for a human.
- **`JsonReader<'a>`** is built with `new(Validated<'a>)` or the named escape hatch
  `from_trusted_unchecked(&'a str)`. Keyed accessors — `string`, `integer`, `floating`,
  `boolean`, `number_token`, `is_null`, `has_field`, `find_object`, `find_array`,
  `string_array`, `object_array`, `element_array` — are **top-level only**: they read the
  immediate members of the object the reader was built on and never search inside a nested
  one. `find_first_array` is the keyless sibling of `find_array`. The bare-slice extractors
  (`extract_array_elements`, `extract_array_objects`, `extract_string_array`,
  `extract_number_array`, `extract_object_fields`, `extract_string_map`, `extract_number_map`)
  operate on a slice a reader produced.
- **`JsonWriter`** builds compact (`new`) or indented (`pretty`) output. Containers take a
  closure that receives the writer. `field` and `array_element` accept anything implementing
  `JsonScalar` (`&str`, `String`, every integer width including `usize`/`isize`, `f64`, `bool`).
  `as_str` and `finish` return `Result<_, JsonWriteError>`, because the writer cannot silently
  emit invalid JSON: a non-finite number (`NaN`, an infinity) poisons it and reports
  `NonFiniteNumber`, checked once at the end rather than at every call site, and in a debug build
  writing one key twice into an object reports `DuplicateKey` — the pair `validate_json` would
  refuse. `raw_field_unchecked` is the named escape hatch for splicing in a pre-built fragment,
  and with `JsonReader::from_trusted_unchecked` it is one of the two ways to defeat "everything
  this crate emits validates". `json_ok()` and `json_error(message)` are the canonical one-line
  bodies.
- **The strict read tier** keeps absent, `null`, and present-but-wrong-type apart, which the
  `Option`-returning accessors above deliberately fold together. `required_string`,
  `required_integer`, `required_boolean`, `required_floating` refuse all three; the
  `optional_*` siblings excuse absence and take a `NullReads` argument saying whether an
  explicit `null` is excused with it. A refusal is a `JsonReadError` carrying a
  `#[non_exhaustive]` `JsonReadErrorKind` and the key it refused, and converts into an
  `AnyError` under the named descriptors in `kinds`. Lists read on the same two shapes —
  `required_string_array` / `required_object_array` and their `optional_*` siblings —
  carrying the same error, with `JsonReadErrorKind::ElementType { index }` naming the
  first element of the wrong type.
- **`merge_object_overrides(base, overrides)`** substitutes one flat-keyed object into another,
  re-emitting untouched values as the exact bytes they arrived as and keeping the base's key
  order, with appended keys last. It reports which override keys `replaced` an existing one and
  which were `appended`, rather than deciding for the caller whether inventing a key is
  legitimate. Both sides are validated first, because `extract_object_fields` fails open.
- **`serialize_object`** and **`emit_rows`** are the two emit shapes a caller of `JsonWriter`
  otherwise re-spells, over a `WriteJson` implementation. `emit_rows`' nesting is load-bearing:
  `array_value` writes the comma between elements and `object` never does, so a row written as a
  bare `object` gives `[{…}{…}]` — invalid JSON that a brace-matching reader parses back
  without complaint.
- **`detail`** exposes the traversal primitives (`skip_ws`, `parse_json_string`,
  `parse_json_number`, `parse_json_bool`, `find_field`, `skip_json_value`) for a consumer
  building a bespoke accessor, as the source does.
- **Features.** None by default. The default build is free of **third-party** code — enforced
  by `scripts/verify.sh` against an explicit allowlist, because a consumer reads JSON in its
  core library *and* in the small runtime it ships to remote hosts, the most
  dependency-sensitive place in its build. The one non-optional dependency is the sibling
  `bc-error-rs`, which the strict read tier returns errors under and which is itself
  third-party-free, so a consumer's third-party closure is unchanged by it. `serde` adds the
  bridge below.

## Serde

With the `serde` feature this crate is a serde **format**, not an island:

```rust
# use serde::{Serialize, Deserialize};
#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Order<'a> { id: &'a str, total: f64 }

let order = Order { id: "a1", total: 12.5 };
let json = bc_json::to_string(&order)?;
assert_eq!(json, r#"{"id":"a1","total":12.5}"#);
assert_eq!(bc_json::from_str::<Order<'_>>(&json)?, order);
# Ok::<(), Box<dyn std::error::Error>>(())
```

- **`to_string` / `to_string_pretty`** write through `JsonWriter`, so the escaping, the float
  formatting, the separator discipline, and the debug-build duplicate-key check are the same
  code the native API uses — the two cannot drift into disagreeing about what JSON is.
- **`from_str`** validates first. That is the point rather than an overhead: a deserialized
  document has passed every check listed above, duplicate keys included. **`from_validated`**
  takes a `Validated` for a caller who already validated at the boundary, and
  **`from_str_unchecked`** is the named hatch for input that has not been validated at all.
- **Strings borrow.** A string with no escapes is handed back as a slice of the input, so a
  type holding `&str` costs no allocation. One that *has* an escape cannot be — the decoded
  text exists nowhere in the input — so it is owned, and serde reports which happened.
- **The deserializer is not built on `JsonReader`.** The reader restarts at the top of the
  object for every key, which is right for reading two fields out of a large body and wrong for
  reading all of them. It has its own forward cursor, sharing `decode_json_string` and
  `json_number_end` so escapes and number grammar keep exactly one definition each.

## What "strict" means, measured

The claim that this validator is stricter than a general-purpose parser is checked rather than
asserted: `tests/conformance.rs` runs the whole [JSONTestSuite][suite] corpus (318 cases,
vendored under `tests/fixtures/JSONTestSuite`, MIT) through both this crate and `serde_json`
and pins the difference.

[suite]: https://github.com/nst/JSONTestSuite

**Stricter, in exactly two places.** Four corpus cases marked "must accept" are rejected here:

| Case | Why |
|---|---|
| `y_object_duplicated_key.json` | RFC 8259 permits duplicate keys and leaves the outcome undefined. `serde_json` silently keeps the last. Rejecting them is what makes the lazy reader's first-match lookup unambiguous. |
| `y_object_duplicated_key_and_value.json` | Same. |
| `y_object_escaped_null_in_key.json` | The `\u0000` escape is valid by the grammar and decodes to a NUL byte, which every C-string boundary downstream truncates at rather than failing on, so the decoder refuses it (`JsonValidationErrorKind::NulCharacter`). |
| `y_string_null_escape.json` | Same. |

**Looser, in exactly one place.** Five corpus cases `serde_json` rejects are accepted here —
`i_number_huge_exp`, `i_number_neg_int_huge_exp`, `i_number_pos_double_huge_exp`,
`i_number_real_neg_overflow`, `i_number_real_pos_overflow`. `serde_json` parses numbers to
values while validating, so an exponent that overflows `f64` fails there. This crate validates
the **grammar** and leaves range to whoever reads the value; `JsonReader::floating` returns
`None` rather than an infinity. That is the source's behavior too — its own suite asserts that
`1e400` validates. A document can therefore validate and still hold a number no `f64` can
represent, which is a real limitation and is stated here rather than buried.

There is one more asymmetry worth naming: `serde_json`'s recursion limit rejects a document
nested exactly 128 deep, where `MAX_JSON_NESTING_DEPTH` allows it. Both cap at 128; they count
the boundary differently.

Beyond the corpus, this crate rejects what a permissive parser accepts:

- numbers outside the RFC 8259 grammar — `01`, `+1`, `.5`, `1.`, `1e`, `NaN`, `Infinity`, `0x10`
- unpaired surrogates, unknown escapes, and raw control bytes inside strings
- text that is not valid UTF-8 (via `validate_json_bytes`)
- trailing input after a complete document
- nesting past `MAX_JSON_NESTING_DEPTH` (128), which is a stack-exhaustion limit, not a style rule

## Verification

```sh
scripts/verify.sh                             # everything below, in one command
cargo tree --edges normal                     # must list only bc-json-rs
cargo build --no-default-features
cargo build --all-features
cargo build --target wasm32-unknown-unknown   # the C++ original is WASM-safe; so is this
```

`scripts/verify.sh` is this crate's pipeline: formatting, tests in debug and release, doctests,
clippy, rustdoc, the feature matrix, the wasm32 build, and three guards — the dependency surface,
test collection, and the conformance corpus's licence. Every step runs on the pinned toolchain;
nothing in it needs nightly. bc-rs's root `scripts/verify.sh` runs it with every other member's,
and `.forgejo/workflows/ci.yml` and the tracked `.idea/runConfigurations/` both invoke that one, so
"green locally" and "green in CI" cannot drift apart by editing one and forgetting the other.

bc-rs's `rust-toolchain.toml` pins the MSRV on a developer's machine; CI overrides it per-leg with
`RUSTUP_TOOLCHAIN` so the pinned-stable leg runs a newer compiler. `cargo-nextest` is pinned to
0.9.100 and terminates any test exceeding 30 seconds; the CI job's outer bound is bc-rs's
`.forgejo/workflows/ci.yml` `timeout-minutes`.
`verify.sh` falls back to `cargo test` when nextest is not installed locally, and says so.
