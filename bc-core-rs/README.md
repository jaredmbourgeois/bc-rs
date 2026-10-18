# bc-core-rs

The pieces of a foundation library that Rust's standard library does not already supply: a generational arena, a
contiguous map, hex, URL, XML and percent encoding, and the process's assertion handler.

The crate has **no dependencies**, and its pipeline fails if it gains one: it sits underneath the rest of the
workspace — `bc-nodular-architecture-rs` holds an arena on the hottest path it has — so an edge taken here is an edge
taken by every consumer downstream. It builds for `wasm32-unknown-unknown`.

Depend on it by git and release tag, as the [workspace README](../README.md#depending-on-bc-rs) shows; the package is
`bc-core-rs` and the library is imported as `bc_core`.

It ports the parts of the C++ `libs/foundation/core` library of Bourgeois Concern's codebase that Rust does not cover,
keeping that library's API and behaviour; every case of its test suite was carried into this crate's tests or proved
by the compiler.

Licensed under Apache-2.0 ([LICENSE.txt](LICENSE.txt)); copyright 2026 Jared Bourgeois.

## Assertion handler

What the process does about a violated expectation: a rule this process's own code states and did
not keep — a telemetry call that needs a span made where none is active, an error raised with the
`invariant_breach` fact — as opposed to a failed operation, which is a `Result` a caller handles.
There is one handler per process, installed by the composition root; a process starts with none.

```rust
use bc_core::{AssertionHandler, report_assertion_failure, set_assertion_handler};

let previous = set_assertion_handler(Some(AssertionHandler::new(|failure| {
    eprintln!("{} at {}", failure.expectation, failure.location);
})));
report_assertion_failure("a span is active", "add_span_event(\"orphan\")");
drop(set_assertion_handler(previous));
```

A report never fails the code that makes it: a handler that panics is contained, and the handler
is called outside the slot's lock, so one that reports or reinstalls cannot deadlock. A failure's
`detail` is for a person at a debugger and is not safe to export; a handler that sends failures
beyond the process sends `expectation` and `location` only. `aborting_assertion_handler()` aborts in
every build — the decision to install it in a debug build belongs to the composition root.

## Arena

A pool of reusable slots addressed by stable, generation-checked handles. Insert, remove, and lookup
are constant time; a removed value's slot returns to a free list, so the arena is sized by the values
alive at once rather than by every value it has ever held.

The generation counter is the whole difference between this and a `Vec<Option<T>>`. A handle whose
value has been removed stops resolving, instead of silently naming whatever is inserted into the slot
next:

```rust
use bc_core::Arena;

let mut arena = Arena::new();
let first = arena.insert("a");
let second = arena.insert("b");

assert_eq!(arena.remove(first), Some("a"));

// The freed slot is reused, but the handle that named it is not fooled.
let third = arena.insert("c");
assert_eq!(third.index(), first.index());
assert_eq!(arena.get(first), None);
assert_eq!(arena.get(third), Some(&"c"));
assert_eq!(arena.get(second), Some(&"b"));
```

`ArenaRef` is eight bytes and `Copy`. A default one — index 0, generation 0 — never matches a live
value, because generations start at one; that is what lets it stand in for a null handle without an
`Option` wrapper. `to_u64`/`from_u64` carry a handle through anything that stores a single integer,
exactly over the full `u32` range of both halves.

## Hex

Lowercase encoding, strict decoding, and the two identifier forms the tree builds on them — the
abbreviated display SHA and the "is this actually a digest" check.

```rust
use bc_core::{SHORT_HEX_WIDTH, abbreviate_hex, decode_hex, encode_hex, is_lowercase_hex};

assert_eq!(encode_hex(&[0x0F, 0xAB]), "0fab");

// Decoding identifies bytes, so it accepts either case.
assert_eq!(decode_hex("0FAB"), Some(vec![0x0F, 0xAB]));

// And refuses rather than guesses.
assert_eq!(decode_hex("0fa"), None);

let sha = "abc123def456789012345678901234567890abcd";
assert!(is_lowercase_hex(sha, 40));
assert_eq!(abbreviate_hex(sha, SHORT_HEX_WIDTH), "abc123de");
```

Encoding is lowercase and identifying is lowercase-only, deliberately: these identifiers are
compared *as strings*, so one byte sequence must have exactly one spelling. Decoding is the one
half that takes either case, because it names bytes rather than strings.

## URL encoding

RFC 3986 percent-encoding, its `application/x-www-form-urlencoded` variant, and a strict inverse.

```rust
use bc_core::{form_urlencode, percent_decode, percent_encode};

assert_eq!(percent_encode(b"a b/c"), "a%20b%2Fc");
assert_eq!(form_urlencode(b"a b/c"), "a+b%2Fc");

assert_eq!(percent_decode("a%20b%2Fc").as_deref(), Some(&b"a b/c"[..]));

// A '%' not followed by two hex digits is refused, never guessed at.
assert_eq!(percent_decode("ok%4z"), None);
```

Encoding emits uppercase hex, which RFC 3986 §2.1 says producers should do; decoding accepts either
case, because it names bytes rather than strings. `+` decodes to a literal `+` — space-for-plus is
the form-body convention alone, and one decoder that honoured both would disagree with whichever
caller was right.

## XML escaping

The five XML predefined entities, for text going into an XML or SVG document.

```rust
use bc_core::escape_xml;

assert_eq!(escape_xml("a<b> & \"c\""), "a&lt;b&gt; &amp; &quot;c&quot;");
assert_eq!(escape_xml("it's"), "it&apos;s");

// Not idempotent, by design: the input is text, not markup.
assert_eq!(escape_xml("&amp;"), "&amp;amp;");
```

The apostrophe takes the XML named entity `&apos;`, never the HTML5 numeric form `&#x27;` — that is
the contract separating this from an HTML escaper, which cannot use `&apos;` because it is not a
named HTML4 entity. Escape once, where text becomes a document.

For a writer that puts caller text only between tags, `escape_xml_content` escapes `&`, `<` and `>`
and leaves the quotes as they were; and `is_xml_char` answers whether XML 1.0 can carry a character
at all, which no amount of escaping decides.

```rust
use bc_core::{escape_xml_content, is_xml_char};

// Quotes carry no meaning between tags, so the bytes stay as written.
assert_eq!(escape_xml_content("say \"hi\" & 'bye'"), "say \"hi\" &amp; 'bye'");

// Not an escaping question: XML has no representation for these at all.
assert!(!is_xml_char('\u{1}'));
assert!(!is_xml_char('\u{FFFF}'));
assert!(is_xml_char('\u{7F}')); // legal in 1.0; only 1.1 asks for it escaped
```

**Reach for `escape_xml` when in doubt** — it is safe in an attribute too, and costs only those
bytes. A caller who takes the narrow rule and later grows an attribute has written an injection.

## Percent formatting

Basis points as a percent string, with insignificant zeros dropped.

```rust
use bc_core::basis_points_to_percent_string;

assert_eq!(basis_points_to_percent_string(1500), "15%");
assert_eq!(basis_points_to_percent_string(1550), "15.5%");
assert_eq!(basis_points_to_percent_string(1505), "15.05%");

// One leading minus, never interleaved into the fraction.
assert_eq!(basis_points_to_percent_string(-1550), "-15.5%");
```

Existing so one margin cannot render two ways on two surfaces. The magnitude is taken unsigned, so
`i64::MIN` — whose negation does not fit an `i64` — needs no special case.

## Contiguous map

Hashed lookup over dense key and value runs, so iteration is a linear walk and the values can be
handed out as a slice.

```rust
use bc_core::ContiguousMap;

let mut map = ContiguousMap::new();
map.insert("alpha", 1);
map.insert("beta", 2);
map.insert("gamma", 3);

assert_eq!(map.get(&"beta"), Some(&2));
assert_eq!(map.values(), &[1, 2, 3]);

// `retain` compacts, so what it keeps stays in order.
map.retain(|_, value| value % 2 == 1);
assert_eq!(map.keys(), &["alpha", "gamma"]);
```

Removal is the trade: the last entry is swapped into the hole to keep storage dense, so **`remove`
does not preserve order** while `retain` does. Reach for this where iteration and slice access are
worth more than a stable order, and for a stable one reach for a `Vec` of pairs or an ordered map.

## Not portable here: `secure_zero_memory`

The canonical header zeroes a buffer through a `volatile` pointer, so the optimizer may not remove
the writes. Rust reaches a volatile write only through `unsafe`, and this crate carries
`unsafe_code = "forbid"`; the alternative — depending on `zeroize` — is closed off by the
dependency-free contract that `scripts/verify.sh` enforces.

A plain zeroing loop would compile and would be *deleted* by the optimizer whenever the buffer is
not read afterwards, which is exactly when it is called. Shipping one would claim a security
property this crate cannot provide, so it is not shipped: a consumer that genuinely needs the
guarantee should depend on `zeroize` directly, where the `unsafe` lives with the crate that owns it.
